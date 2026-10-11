//! Photoshop Raw (`.raw`): the bare channel samples, with no header of their own, for moving
//! images between applications and platforms. Not a camera raw: those are recognised by their
//! contents first (see [`crate::raw`]).
//!
//! Saving writes the flattened image in the document's model (Grayscale, RGB, CMYK or Lab) at its
//! depth (8, 16 or 32 bits), followed by its alpha and spot channels, as Photoshop does; layer
//! transparency is flattened over white. [`RawSaveOptions`] are Photoshop Raw Options: a header of
//! zero bytes, pixel- or channel-interleaved order, and the byte order of 16- and 32-bit samples.
//!
//! The file says nothing about its size, so opening needs a [`RawLayout`] (Photoshop's Raw Options
//! dialog), which [`guess`] can propose from the file's length. One channel opens as Grayscale,
//! three as RGB and four as CMYK; two add an alpha channel to Grayscale, and more than four add
//! alpha channels to CMYK.

use photocraft_codecs::{ChannelLayout, Image, SampleType as CSample};
use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_doc::{AlphaChannel, Document, Layer, LayerContent};
use photocraft_geom::{Rect, Size};
use photocraft_raster::Surface;

use crate::flat::{document_to_image, matte_over_white, single_layer, try_buffer};
use crate::{ExportOptions, ExportResult, ImportResult, IoError};

/// Photoshop Raw Options when saving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawSaveOptions {
    /// Bytes of zeros before the image (a placeholder some programs fill in).
    pub header: u32,
    /// Pixel by pixel (every channel of a pixel together) rather than channel by channel.
    pub interleaved: bool,
    /// 16- and 32-bit samples low byte first (Photoshop's "IBM PC"), else "Macintosh".
    pub little_endian: bool,
}

impl Default for RawSaveOptions {
    fn default() -> Self {
        RawSaveOptions { header: 0, interleaved: true, little_endian: !cfg!(target_os = "macos") }
    }
}

/// How a Photoshop Raw file is laid out (Photoshop Raw Options when opening).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawLayout {
    pub width: u32,
    pub height: u32,
    /// 1 to 56.
    pub channels: u16,
    /// Bits per channel: 8, 16 or 32 (float).
    pub depth: u8,
    pub interleaved: bool,
    /// For 16 and 32 bits.
    pub little_endian: bool,
    /// Bytes to skip before the image.
    pub header: u64,
}

impl Default for RawLayout {
    fn default() -> Self {
        RawLayout { width: 0, height: 0, channels: 3, depth: 8, interleaved: true, little_endian: !cfg!(target_os = "macos"), header: 0 }
    }
}

/// The most channels Photoshop opens.
pub const MAX_CHANNELS: u16 = 56;

impl RawLayout {
    fn bytes_per_sample(&self) -> usize {
        usize::from(self.depth / 8)
    }

    /// The file length this layout needs (header included); `None` on overflow.
    pub fn file_len(&self) -> Option<u64> {
        let px = u64::from(self.width).checked_mul(u64::from(self.height))?;
        px.checked_mul(u64::from(self.channels))?.checked_mul(self.bytes_per_sample() as u64)?.checked_add(self.header)
    }

    fn check(&self) -> Result<(), IoError> {
        if self.width == 0 || self.height == 0 {
            return Err(IoError::Unsupported("Photoshop Raw: width and height must be at least 1 px".into()));
        }
        if !(1..=MAX_CHANNELS).contains(&self.channels) {
            return Err(IoError::Unsupported(format!("Photoshop Raw: 1 to {MAX_CHANNELS} channels")));
        }
        if !matches!(self.depth, 8 | 16 | 32) {
            return Err(IoError::Unsupported("Photoshop Raw: 8, 16 or 32 bits per channel".into()));
        }
        Ok(())
    }
}

/// Photoshop's Guess: the width and height (as near square as the pixel count allows, the wider
/// side first) that fill a file of `len` bytes with `hint`'s header, channels and depth. When
/// those don't divide the file, the other common channel counts (3, 1, 4, 2) and depths (8, 16)
/// are tried in turn. A width or height already set in `hint` is kept when it divides the pixels.
pub fn guess(len: u64, hint: &RawLayout) -> Option<RawLayout> {
    let mut tries = vec![(hint.channels, hint.depth)];
    for depth in [hint.depth, 8, 16] {
        for ch in [hint.channels, 3, 1, 4, 2] {
            if !tries.contains(&(ch, depth)) {
                tries.push((ch, depth));
            }
        }
    }
    let body = len.checked_sub(hint.header)?;
    tries.into_iter().find_map(|(channels, depth)| {
        let per = u64::from(channels) * u64::from(depth / 8);
        if per == 0 || body == 0 || !body.is_multiple_of(per) {
            return None;
        }
        let px = body / per;
        let (w, h) = match (hint.width, hint.height) {
            (w, _) if w > 0 && px.is_multiple_of(u64::from(w)) => (u64::from(w), px / u64::from(w)),
            (_, h) if h > 0 && px.is_multiple_of(u64::from(h)) => (px / u64::from(h), u64::from(h)),
            _ => near_square(px)?,
        };
        let (w, h) = (u32::try_from(w).ok()?, u32::try_from(h).ok()?);
        Some(RawLayout { width: w, height: h, channels, depth, ..*hint })
    })
}

/// The factor pair of `px` closest to square, the larger factor as the width. A prime count
/// gives a one-row image; `None` when the larger side would not fit a `u32`.
fn near_square(px: u64) -> Option<(u64, u64)> {
    let mut h = (px as f64).sqrt() as u64;
    while h > 1 && !px.is_multiple_of(h) {
        h -= 1;
    }
    let h = h.max(1);
    let w = px / h;
    (w <= u64::from(u32::MAX)).then_some((w, h))
}

// ---------- export ----------

/// The document as Photoshop Raw.
pub fn export(doc: &Document, opts: &ExportOptions) -> Result<ExportResult, IoError> {
    let o = opts.photoshop_raw;
    let mut warnings = Vec::new();
    let (w, h) = (doc.size.width, doc.size.height);
    if w == 0 || h == 0 {
        return Err(IoError::Unsupported("Photoshop Raw needs an image at least 1 × 1 px".into()));
    }
    let depth = match doc.depth {
        SampleType::U8 => 8,
        SampleType::U16 => 16,
        SampleType::F32 => 32,
    };
    // The composite's channels as normalised planes, then the document's own channels.
    let mut planes = composite_planes(doc, &mut warnings)?;
    if doc.mode == ColorMode::Bitmap {
        warnings.push("Photoshop Raw has no Bitmap mode: written as an 8-bit gray channel".into());
    }
    let rect = Rect::new(0, 0, w as i32, h as i32);
    for c in &doc.channels {
        let s = &c.surface;
        let n = s.format().channels().max(1);
        planes.push(s.read_region(rect).chunks_exact(n).map(|p| p[0]).collect());
    }
    if usize::from(MAX_CHANNELS) < planes.len() {
        return Err(IoError::Unsupported(format!("Photoshop Raw holds at most {MAX_CHANNELS} channels")));
    }
    let bps = depth / 8;
    let n = w as usize * h as usize;
    let mut out = try_buffer(n, planes.len() * bps)?;
    out.resize(o.header as usize, 0);
    let put = |out: &mut Vec<u8>, v: f32| match depth {
        8 => out.push((v.clamp(0.0, 1.0) * 255.0).round() as u8),
        16 => {
            let s = (v.clamp(0.0, 1.0) * 65535.0).round() as u16;
            out.extend_from_slice(&if o.little_endian { s.to_le_bytes() } else { s.to_be_bytes() });
        }
        _ => out.extend_from_slice(&if o.little_endian { v.to_le_bytes() } else { v.to_be_bytes() }),
    };
    if o.interleaved {
        for i in 0..n {
            for p in &planes {
                put(&mut out, p.get(i).copied().unwrap_or(0.0));
            }
        }
    } else {
        for p in &planes {
            for i in 0..n {
                put(&mut out, p.get(i).copied().unwrap_or(0.0));
            }
        }
    }
    Ok(ExportResult { bytes: out, warnings })
}

/// The flattened image's colour channels as planes of normalised values (Lab as PhotoCraft
/// stores it: L / 100, (a + 128) / 255, (b + 128) / 255).
fn composite_planes(doc: &Document, warnings: &mut Vec<String>) -> Result<Vec<Vec<f32>>, IoError> {
    let (w, h) = (doc.size.width, doc.size.height);
    let rect = Rect::new(0, 0, w as i32, h as i32);
    let (vals, colors, alpha_at) = if doc.mode == ColorMode::Lab {
        match single_layer(doc) {
            Some(s) => (s.read_region(rect), 3, Some(3)),
            None => {
                warnings.push(format!("{} layer(s) flattened; layers, masks and blend modes are not kept", doc.layer_count()));
                let s = photocraft_compose::flatten_to_surface(doc, PixelFormat::new(ColorMode::Lab, SampleType::F32, false), Some([1.0, 1.0, 1.0]));
                (s.read_region(rect), 3, None)
            }
        }
    } else {
        let mut img: Image = document_to_image(doc, warnings)?;
        if img.layout().has_alpha() {
            if img.has_translucency() {
                warnings.push("Photoshop Raw has no transparency: the image was flattened over white".into());
            }
            img = matte_over_white(&img)?;
        }
        let colors = img.layout().color_channels();
        (img.to_normalized(), colors, None)
    };
    let stride = colors + usize::from(alpha_at.is_some());
    let white: [f32; 3] = [1.0, 128.0 / 255.0, 128.0 / 255.0];
    let mut planes = vec![Vec::with_capacity(w as usize * h as usize); colors];
    let mut translucent = false;
    for px in vals.chunks_exact(stride) {
        let a = alpha_at.map_or(1.0, |i| px[i].clamp(0.0, 1.0));
        translucent |= a < 1.0;
        for (c, plane) in planes.iter_mut().enumerate() {
            plane.push(px[c] * a + white[c.min(2)] * (1.0 - a));
        }
    }
    if translucent {
        warnings.push("Photoshop Raw has no transparency: the image was flattened over white".into());
    }
    Ok(planes)
}

// ---------- import ----------

/// The colour mode and colour channel count `channels` opens as.
fn mode_for(channels: u16) -> (ColorMode, usize) {
    match channels {
        1 | 2 => (ColorMode::Grayscale, 1),
        3 => (ColorMode::Rgb, 3),
        _ => (ColorMode::Cmyk, 4),
    }
}

/// Opens `bytes` as Photoshop Raw laid out as `layout`. A file shorter than the layout opens with
/// the missing samples empty and a warning; one longer has the excess ignored.
pub fn import(name: &str, bytes: &[u8], layout: &RawLayout) -> Result<ImportResult, IoError> {
    layout.check()?;
    let l = layout;
    let need = l.file_len().ok_or_else(|| IoError::Unsupported("Photoshop Raw: that size overflows".into()))?;
    photocraft_codecs::Limits::default().check(l.width, l.height, ChannelLayout::Gray, CSample::U8)?;
    let n = l.width as usize * l.height as usize;
    let bps = l.bytes_per_sample();
    let chans = usize::from(l.channels);
    if (n as u64).saturating_mul(chans as u64).saturating_mul(bps as u64) > photocraft_codecs::Limits::default().max_alloc {
        return Err(IoError::Unsupported("Photoshop Raw: the image is too large to open".into()));
    }
    let mut warnings = Vec::new();
    let len = bytes.len() as u64;
    if len < need {
        warnings.push(format!("the file is {} bytes shorter than {}×{} px with {} channels needs; the rest is empty", need - len, l.width, l.height, l.channels));
    } else if len > need {
        warnings.push(format!("{} bytes past the image were ignored", len - need));
    }
    let data = bytes.get(usize::try_from(l.header).unwrap_or(usize::MAX)..).unwrap_or(&[]);
    let sample = |i: usize| -> f32 {
        let at = i * bps;
        let Some(b) = data.get(at..at + bps) else { return 0.0 };
        match l.depth {
            8 => f32::from(b[0]) / 255.0,
            16 => f32::from(if l.little_endian { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) }) / 65535.0,
            _ => {
                let v = if l.little_endian { f32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { f32::from_be_bytes([b[0], b[1], b[2], b[3]]) };
                if v.is_finite() { v } else { 0.0 }
            }
        }
    };
    let index = |px: usize, c: usize| if l.interleaved { px * chans + c } else { c * n + px };
    let (mode, colors) = mode_for(l.channels);
    let depth = match l.depth {
        8 => SampleType::U8,
        16 => SampleType::U16,
        _ => SampleType::F32,
    };
    let mut doc = Document::new(name, Size::new(l.width, l.height), mode, depth);
    let fmt = doc.pixel_format();
    let stride = fmt.channels();
    let mut s = Surface::new(fmt);
    let (w, h) = (l.width as usize, l.height as usize);
    let band = ((8 << 20) / (w * stride * 4).max(1)).max(1);
    let mut y0 = 0;
    while y0 < h {
        let y1 = (y0 + band).min(h);
        let mut vals = Vec::with_capacity((y1 - y0) * w * stride);
        for px in y0 * w..y1 * w {
            for c in 0..colors {
                vals.push(sample(index(px, c)));
            }
            vals.push(1.0);
        }
        s.write_region(Rect::new(0, y0 as i32, w as i32, y1 as i32), &vals);
        y0 = y1;
    }
    s.prune();
    let mut bg = Layer::new("Background", LayerContent::Raster(s));
    bg.locks.transparency = true;
    bg.locks.position = true;
    doc.layers.push(bg);
    for c in colors..chans {
        let mut a = Surface::new(PixelFormat::new(ColorMode::Grayscale, depth, false));
        let mut y0 = 0;
        while y0 < h {
            let y1 = (y0 + band).min(h);
            let vals: Vec<f32> = (y0 * w..y1 * w).map(|px| sample(index(px, c))).collect();
            a.write_region(Rect::new(0, y0 as i32, w as i32, y1 as i32), &vals);
            y0 = y1;
        }
        doc.channels.push(AlphaChannel::new(format!("Alpha {}", c - colors + 1), a));
    }
    Ok(ImportResult { document: doc, warnings, source_read_only: false, preview_only: false })
}

#[cfg(test)]
#[path = "photoshop_raw_tests.rs"]
mod tests;
