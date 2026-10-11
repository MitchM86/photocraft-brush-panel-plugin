//! JPEG 2000: decode with `hayro-jpeg2000` (JP2 container or bare codestream, any depth, palette
//! and YCC resolved), encode the codestream with `openjpeg2-pure-rs` (OpenJPEG's encoder in safe
//! Rust) and wrap it in the JP2 boxes written here: `ihdr`, `colr` (the ICC profile, or the
//! enumerated sRGB / greyscale / CMYK space), `cdef` for alpha, `res `/`resc` for the resolution
//! and the XMP `uuid` box. Both ways keep 8 and 16 bits, gray / RGB / CMYK, alpha, the profile, the
//! resolution and XMP; a bare codestream (`jpeg2000_codestream`) keeps only the pixels.
//!
//! Lossless uses the reversible 5/3 wavelet; lossy uses the 9/7 one with a compression ratio from
//! the quality (see [`compression_ratio`]).

use crate::Format;
use crate::error::CodecError;
use crate::fidelity::Plan;
use crate::image::{ChannelLayout, Image, Metadata, SampleType};
use crate::options::{EncodeOptions, Limits};

const F: Format = Format::Jpeg2000;

/// The JP2 signature box.
const SIGNATURE: &[u8] = b"\0\0\0\x0CjP  \r\n\x87\n";
/// The `uuid` box that holds XMP (ISO 15444-1 Annex I and Adobe's XMP specification, part 3).
const XMP_UUID: [u8; 16] = [0xBE, 0x7A, 0xCF, 0xCB, 0x97, 0xA9, 0x42, 0xE8, 0x9C, 0x71, 0x99, 0x94, 0x91, 0xE3, 0xAF, 0xAC];
/// Enumerated colour spaces (`colr` method 1).
const ENUM_CMYK: u32 = 12;
const ENUM_SRGB: u32 = 16;
const ENUM_GRAY: u32 = 17;

fn err(e: impl std::fmt::Display) -> CodecError {
    CodecError::malformed(F, e)
}

pub(crate) fn decode(bytes: &[u8], limits: &Limits) -> Result<Image, CodecError> {
    let settings = hayro_jpeg2000::DecodeSettings::default();
    let j2k = hayro_jpeg2000::Image::new(bytes, &settings).map_err(|e| err(format!("{e:?}")))?;
    let (w, h) = (j2k.width(), j2k.height());
    let alpha = j2k.has_alpha();
    let (layout, decoder_profile) = match j2k.color_space() {
        hayro_jpeg2000::ColorSpace::Gray => (ChannelLayout::Gray, None),
        hayro_jpeg2000::ColorSpace::RGB => (ChannelLayout::Rgb, None),
        hayro_jpeg2000::ColorSpace::CMYK => (ChannelLayout::Cmyk, None),
        hayro_jpeg2000::ColorSpace::Unknown { num_channels } => (layout_for(*num_channels)?, None),
        hayro_jpeg2000::ColorSpace::Icc { profile, num_channels } => (layout_for(*num_channels)?, Some(profile)),
    };
    // The file's own profile, as stored (the decoder only reports profiles it can parse). An
    // enumerated ROMM RGB comes back with the decoder's ProPhoto profile, which is kept too;
    // CIELab pixels are already converted to RGB, so its Lab profile is not.
    let icc = file_profile(bytes)
        .or_else(|| decoder_profile.filter(|p| p.get(16..20) == Some(b"RGB ")).cloned())
        .filter(|p| profile_fits(p, layout));
    let layout = if alpha { layout.with_alpha() } else { layout };
    let depth = j2k.original_bit_depth();
    let sample = if depth <= 8 { SampleType::U8 } else { SampleType::U16 };
    limits.check(w, h, layout, sample)?;

    let mut ctx = hayro_jpeg2000::DecoderContext::default();
    let decoded = j2k.decode(&mut ctx).map_err(|e| err(format!("{e:?}")))?;
    let comps = decoded.components();
    let n = layout.channels();
    if comps.len() < n {
        return Err(err(format!("{} components for {layout:?}", comps.len())));
    }
    let pixels = w as usize * h as usize;
    let out_max = if sample == SampleType::U8 { 255.0 } else { 65535.0 };
    // Each component is scaled from its own precision to the output depth.
    let scales: Vec<f32> = comps[..n].iter().map(|c| out_max / ((1u64 << c.bit_depth().clamp(1, 31)) - 1) as f32).collect();
    let mut img = match sample {
        SampleType::U8 => {
            let mut data = vec![0u8; pixels * n];
            for (ch, (c, s)) in comps[..n].iter().zip(&scales).enumerate() {
                for (i, v) in c.samples().iter().take(pixels).enumerate() {
                    data[i * n + ch] = (v * s).round().clamp(0.0, 255.0) as u8;
                }
            }
            Image::from_u8(w, h, layout, data)?
        }
        _ => {
            let mut data = vec![0u16; pixels * n];
            for (ch, (c, s)) in comps[..n].iter().zip(&scales).enumerate() {
                for (i, v) in c.samples().iter().take(pixels).enumerate() {
                    data[i * n + ch] = (v * s).round().clamp(0.0, 65535.0) as u16;
                }
            }
            Image::from_u16(w, h, layout, &data)?
        }
    };
    img.icc = icc;
    img.meta = read_metadata(bytes);
    Ok(img)
}

/// Whether an ICC profile can describe `layout`'s samples: its data colour space matches, or it
/// names none we know (a profile is never attached to the Lab pixels the decoder converted).
fn profile_fits(icc: &[u8], layout: ChannelLayout) -> bool {
    match icc.get(16..20) {
        Some(b"GRAY") => layout.is_gray(),
        Some(b"RGB ") => layout.is_rgb(),
        Some(b"CMYK") => layout.is_cmyk(),
        Some(b"Lab ") => false,
        _ => true,
    }
}

fn layout_for(channels: u8) -> Result<ChannelLayout, CodecError> {
    match channels {
        1 => Ok(ChannelLayout::Gray),
        3 => Ok(ChannelLayout::Rgb),
        4 => Ok(ChannelLayout::Cmyk),
        n => Err(CodecError::unsupported(F, format!("{n}-channel colour space"))),
    }
}

/// A JP2 box: its type and payload.
struct JpBox<'a> {
    kind: [u8; 4],
    data: &'a [u8],
}

/// The boxes in `bytes` (one level); stops at the first malformed length.
fn boxes(mut bytes: &[u8]) -> Vec<JpBox<'_>> {
    let mut out = Vec::new();
    while bytes.len() >= 8 {
        let len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as u64;
        let kind = [bytes[4], bytes[5], bytes[6], bytes[7]];
        let (header, len) = match len {
            0 => (8, bytes.len() as u64),
            1 if bytes.len() >= 16 => {
                let mut b = [0u8; 8];
                b.copy_from_slice(&bytes[8..16]);
                (16, u64::from_be_bytes(b))
            }
            1 => break,
            n => (8, n),
        };
        let Ok(len) = usize::try_from(len) else { break };
        if len < header || len > bytes.len() {
            break;
        }
        out.push(JpBox { kind, data: &bytes[header..len] });
        bytes = &bytes[len..];
    }
    out
}

/// The JP2 header box's children, when `bytes` is a JP2 file.
fn header_boxes(bytes: &[u8]) -> Vec<JpBox<'_>> {
    if !bytes.starts_with(SIGNATURE) {
        return Vec::new();
    }
    boxes(bytes).into_iter().find(|b| &b.kind == b"jp2h").map(|b| boxes(b.data)).unwrap_or_default()
}

/// The ICC profile of the file's first `colr` box (method 2), if any.
fn file_profile(bytes: &[u8]) -> Option<Vec<u8>> {
    let colr = header_boxes(bytes).into_iter().find(|b| &b.kind == b"colr")?;
    (colr.data.first() == Some(&2)).then(|| colr.data.get(3..).map(<[u8]>::to_vec)).flatten()
}

/// Resolution (`res ` → `resc`, else `resd`, in pixels per metre) and XMP (`uuid`).
fn read_metadata(bytes: &[u8]) -> Metadata {
    let mut meta = Metadata::default();
    if let Some(res) = header_boxes(bytes).into_iter().find(|b| &b.kind == b"res ") {
        let inner = boxes(res.data);
        let pick = inner.iter().find(|b| &b.kind == b"resc").or_else(|| inner.iter().find(|b| &b.kind == b"resd"));
        meta.dpi = pick.and_then(|b| parse_resolution(b.data));
    }
    if bytes.starts_with(SIGNATURE) {
        meta.xmp = boxes(bytes)
            .into_iter()
            .find(|b| &b.kind == b"uuid" && b.data.starts_with(&XMP_UUID))
            .and_then(|b| String::from_utf8(b.data[16..].to_vec()).ok())
            .map(|s| s.trim_end_matches('\0').to_string());
    }
    meta
}

/// A `resc`/`resd` payload as (horizontal, vertical) dots per inch.
fn parse_resolution(d: &[u8]) -> Option<(f32, f32)> {
    if d.len() < 10 {
        return None;
    }
    let u = |i: usize| f64::from(u16::from_be_bytes([d[i], d[i + 1]]));
    let ppm = |n: f64, den: f64, e: u8| (den > 0.0).then(|| n / den * 10f64.powi(i32::from(e as i8)));
    let v = ppm(u(0), u(2), d[8])?;
    let hz = ppm(u(4), u(6), d[9])?;
    let dpi = |p: f64| (p * 0.0254) as f32;
    (v > 0.0 && hz > 0.0).then(|| (dpi(hz), dpi(v)))
}

/// One axis of a `resc` payload: dots per inch as numerator, denominator and exponent of pixels
/// per metre. `n / 254 × 10²` keeps hundredths of a dot per inch below 655.36 ppi;
/// `n / 254 × 10⁴` keeps whole ones above.
fn resolution_fraction(dpi: f32) -> (u16, u16, i8) {
    let d = f64::from(dpi).max(0.0);
    if d * 100.0 <= f64::from(u16::MAX) {
        ((d * 100.0).round() as u16, 254, 2)
    } else {
        (d.round().min(f64::from(u16::MAX)) as u16, 254, 4)
    }
}

fn jp_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 16);
    match u32::try_from(payload.len() + 8) {
        Ok(len) => out.extend_from_slice(&len.to_be_bytes()),
        Err(_) => {
            // XLBox: a length that needs 64 bits.
            out.extend_from_slice(&1u32.to_be_bytes());
            out.extend_from_slice(kind);
            out.extend_from_slice(&(payload.len() as u64 + 16).to_be_bytes());
            out.extend_from_slice(payload);
            return out;
        }
    }
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

/// The lossy compression ratio for `quality` (0–100): 2:1 per 12.5 points below 100, so 75 is
/// 4:1, 50 is 16:1 and 0 is 256:1. 100 keeps every bit plane (9/7 rounding only).
fn compression_ratio(quality: u8) -> f32 {
    let q = f32::from(quality.min(100));
    if q >= 100.0 { 0.0 } else { 2f32.powf((100.0 - q) / 12.5) }
}

pub(crate) fn encode(src: &Image, plan: Plan, opts: &EncodeOptions) -> Result<Vec<u8>, CodecError> {
    let img = src.converted(plan.layout, plan.sample);
    let (w, h) = img.dimensions();
    let layout = img.layout();
    let n = layout.channels();
    let precision: u8 = if img.sample_type() == SampleType::U8 { 8 } else { 16 };
    let values: Vec<i32> = match img.sample_type() {
        SampleType::U8 => img.data().iter().map(|&v| i32::from(v)).collect(),
        _ => img.to_u16_samples().unwrap_or_default().into_iter().map(i32::from).collect(),
    };
    let pixels = w as usize * h as usize;
    if values.len() != pixels * n {
        return Err(CodecError::encode(F, "sample buffer size mismatch"));
    }
    let mut comps = Vec::with_capacity(n);
    for ch in 0..n {
        let plane: Vec<i32> = values.iter().skip(ch).step_by(n).copied().collect();
        comps.push(openjp2::ImageComponent::new(w, h, precision, false, plane).map_err(|e| CodecError::encode(F, format!("{e:?}")))?);
    }
    let space = if layout.is_cmyk() {
        openjp2::ColorSpace::Cmyk
    } else if layout.is_gray() {
        openjp2::ColorSpace::Greyscale
    } else {
        openjp2::ColorSpace::Srgb
    };
    let j2k_image = openjp2::Image::new(w, h, space, comps).map_err(|e| CodecError::encode(F, format!("{e:?}")))?;
    let lossless = opts.jpeg2000_lossless;
    let enc = openjp2::EncodeOptions {
        format: openjp2::Format::J2k,
        threads: threads(pixels),
        irreversible: !lossless,
        // The colour transform decorrelates R, G and B; it would mix C, M and Y for nothing.
        use_mct: layout.is_rgb(),
        rates: vec![if lossless { 0.0 } else { compression_ratio(opts.jpeg2000_quality) }],
        num_resolutions: None,
    };
    let codestream = openjp2::Encoder::encode(&j2k_image, &enc).map_err(|e| CodecError::encode(F, format!("{e:?}")))?;
    if opts.jpeg2000_codestream {
        return Ok(codestream);
    }
    Ok(wrap_jp2(&img, precision, &codestream, opts))
}

/// Worker threads for an image of `pixels`: one below a megapixel, else up to eight.
fn threads(pixels: usize) -> usize {
    #[cfg(not(target_arch = "wasm32"))]
    if pixels >= 1 << 20 {
        return std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
    }
    let _ = pixels;
    0
}

/// `codestream` in a JP2 file describing `img`.
fn wrap_jp2(img: &Image, precision: u8, codestream: &[u8], opts: &EncodeOptions) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let layout = img.layout();
    let n = layout.channels() as u16;
    let mut out = SIGNATURE.to_vec();
    // A CMYK image is outside baseline JP2 (whose enumerated spaces are sRGB and greyscale), so
    // it is labelled JPX, as Photoshop's JPF files are, still listing JP2 for readers that
    // only need the codestream.
    let mut ftyp = Vec::new();
    if layout.is_cmyk() {
        ftyp.extend_from_slice(b"jpx \0\0\0\0jp2 jpx jpxb");
    } else {
        ftyp.extend_from_slice(b"jp2 \0\0\0\0jp2 ");
    }
    out.extend(jp_box(b"ftyp", &ftyp));

    let mut header = Vec::new();
    let mut ihdr = Vec::with_capacity(14);
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&n.to_be_bytes());
    // BPC (precision − 1, unsigned), compression type 7 (JPEG 2000), colour space known, no IPR.
    ihdr.extend_from_slice(&[precision - 1, 7, 0, 0]);
    header.extend(jp_box(b"ihdr", &ihdr));

    let mut colr = Vec::new();
    match img.icc.as_ref().filter(|_| opts.embed_icc) {
        Some(icc) => {
            colr.extend_from_slice(&[2, 0, 0]);
            colr.extend_from_slice(icc);
        }
        None => {
            let e = if layout.is_cmyk() {
                ENUM_CMYK
            } else if layout.is_gray() {
                ENUM_GRAY
            } else {
                ENUM_SRGB
            };
            colr.extend_from_slice(&[1, 0, 0]);
            colr.extend_from_slice(&e.to_be_bytes());
        }
    }
    header.extend(jp_box(b"colr", &colr));

    if layout.has_alpha() {
        // Colour channels are associated with colours 1..; the last one is opacity for the
        // whole image (not premultiplied).
        let mut cdef = n.to_be_bytes().to_vec();
        for i in 0..n {
            let alpha = i + 1 == n;
            for v in [i, u16::from(alpha), if alpha { 0 } else { i + 1 }] {
                cdef.extend_from_slice(&v.to_be_bytes());
            }
        }
        header.extend(jp_box(b"cdef", &cdef));
    }

    if opts.embed_metadata
        && let Some((x, y)) = img.meta.dpi.filter(|(x, y)| x.is_finite() && y.is_finite() && *x > 0.0 && *y > 0.0)
    {
        let (vn, vd, ve) = resolution_fraction(y);
        let (hn, hd, he) = resolution_fraction(x);
        let mut resc = Vec::with_capacity(10);
        for v in [vn, vd, hn, hd] {
            resc.extend_from_slice(&v.to_be_bytes());
        }
        resc.extend_from_slice(&[ve as u8, he as u8]);
        header.extend(jp_box(b"res ", &jp_box(b"resc", &resc)));
    }
    out.extend(jp_box(b"jp2h", &header));

    if opts.embed_metadata
        && let Some(xmp) = &img.meta.xmp
    {
        let mut uuid = XMP_UUID.to_vec();
        uuid.extend_from_slice(xmp.as_bytes());
        out.extend(jp_box(b"uuid", &uuid));
    }
    out.extend(jp_box(b"jp2c", codestream));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_round_trips_through_resc() {
        for dpi in [72.0f32, 300.0, 72.5, 600.0, 2400.0] {
            let (n, d, e) = resolution_fraction(dpi);
            let mut b = Vec::new();
            for v in [n, d, n, d] {
                b.extend_from_slice(&v.to_be_bytes());
            }
            b.extend_from_slice(&[e as u8, e as u8]);
            let (x, y) = parse_resolution(&b).unwrap();
            assert!((x - dpi).abs() < 0.01 && (y - dpi).abs() < 0.01, "{dpi} → {x} {y}");
        }
    }

    #[test]
    fn quality_maps_to_ratios() {
        assert_eq!(compression_ratio(100), 0.0);
        assert!((compression_ratio(75) - 4.0).abs() < 1e-4);
        assert!((compression_ratio(50) - 16.0).abs() < 1e-3);
        assert!((compression_ratio(0) - 256.0).abs() < 1e-2);
    }
}
