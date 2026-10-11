//! Photoshop EPS (`.eps`, Save As / Export As): the flattened image as an Encapsulated PostScript
//! (Level 2) program, with Photoshop's `%ImageData:` comment so Photoshop and PhotoCraft open it
//! back as pixels instead of rasterising the PostScript.
//!
//! The image keeps the document's colour model where PostScript has it: Bitmap (1 bit), Grayscale,
//! RGB, CMYK (a single plain layer keeps its ink values exactly) and Lab (`CIEBasedABC`, D50).
//! Duotone, Indexed and Multichannel are written as RGB. EPS is 8 bits per channel and has no
//! transparency, so deeper documents are reduced and transparent ones flattened over white, as
//! Photoshop does. The colour profile goes in a `%%BeginICCProfile` comment; the encodings are
//! Photoshop's (ASCII hex, ASCII85, binary, JPEG at four qualities); a TIFF preview puts the
//! program in a DOS EPS binary header for page-layout programs that show it.
//!
//! Opening reads the `%ImageData:` pixels of Photoshop EPS files (any of those encodings, line-
//! or pixel-interleaved). A generic EPS (vector PostScript from a drawing program) can't be
//! rendered here; one with a TIFF preview opens that preview, with a warning.

use std::sync::Arc;

use photocraft_codecs::{self as codecs, ChannelLayout, EncodeOptions, Format, Image, SampleType as CSample};
use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_doc::{Document, Layer, LayerContent};
use photocraft_geom::{Rect, Size};
use photocraft_raster::Surface;

use crate::flat::{document_to_image, duotone_as_rgb, matte_over_white};
use crate::{ExportOptions, ExportResult, ImportResult, IoError};

/// How the image data is encoded (Photoshop's EPS Options › Encoding).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EpsEncoding {
    /// Hexadecimal text: twice the size, but any channel can carry it.
    Ascii,
    /// ASCII85 text: a quarter larger than binary.
    Ascii85,
    /// Raw bytes (Photoshop's default).
    #[default]
    Binary,
    /// JPEG (lossy, binary) at one of Photoshop's four qualities.
    Jpeg(EpsJpegQuality),
}

/// Photoshop's EPS JPEG qualities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpsJpegQuality {
    Low,
    Medium,
    High,
    Maximum,
}

impl EpsJpegQuality {
    /// The JPEG quality (1–100): Photoshop's 3, 5, 8 and 10 on its 0–12 scale.
    fn percent(self) -> u8 {
        let q = match self {
            EpsJpegQuality::Low => 3.0,
            EpsJpegQuality::Medium => 5.0,
            EpsJpegQuality::High => 8.0,
            EpsJpegQuality::Maximum => 10.0,
        };
        (q / 12.0 * 99.0 + 1.0_f64).round() as u8
    }
}

impl EpsEncoding {
    /// The `%ImageData:` binary/hex code: 1 binary, 2 hex, 3–6 JPEG low to maximum, 7 ASCII85.
    fn code(self) -> u8 {
        match self {
            EpsEncoding::Binary => 1,
            EpsEncoding::Ascii => 2,
            EpsEncoding::Jpeg(EpsJpegQuality::Low) => 3,
            EpsEncoding::Jpeg(EpsJpegQuality::Medium) => 4,
            EpsEncoding::Jpeg(EpsJpegQuality::High) => 5,
            EpsEncoding::Jpeg(EpsJpegQuality::Maximum) => 6,
            EpsEncoding::Ascii85 => 7,
        }
    }
}

/// The preview a page-layout program shows in place of the PostScript (EPS Options › Preview).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EpsPreview {
    /// No preview.
    None,
    /// An 8-bit RGB TIFF at 72 ppi, in a DOS EPS binary header (Photoshop's Windows default).
    #[default]
    Tiff,
}

/// EPS Options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EpsOptions {
    pub encoding: EpsEncoding,
    pub preview: EpsPreview,
    /// Ask the printer to smooth the image when it is scaled up (Image Interpolation).
    pub interpolate: bool,
}

/// The name of the line the image data follows (the `%ImageData:` data start).
const DATA_START: &str = "beginimage";
/// The DOS EPS binary header's signature.
const DOS_EPS: [u8; 4] = [0xC5, 0xD0, 0xD3, 0xC6];

/// Whether `bytes` look like an EPS: a DOS EPS binary header, or PostScript naming EPSF.
pub fn is_eps(bytes: &[u8]) -> bool {
    bytes.starts_with(&DOS_EPS) || (bytes.starts_with(b"%!PS-Adobe") && bytes.iter().take(64).position(|&b| b == b'\n' || b == b'\r').is_some_and(|end| bytes[..end].windows(4).any(|w| w == b"EPSF")))
}

// ---------- export ----------

/// The pixels the program draws: 8-bit samples in PostScript's order (Lab as L*, a*, b* bytes).
struct EpsImage {
    width: u32,
    height: u32,
    /// `%ImageData:` mode: 1 gray/bitmap, 2 Lab, 3 RGB, 4 CMYK.
    mode: u8,
    /// 1 (Bitmap) or 8.
    depth: u8,
    /// Interleaved samples; for 1 bit, rows packed most significant bit first, 1 = white.
    data: Vec<u8>,
    icc: Option<Vec<u8>>,
}

/// The document as a Photoshop EPS.
pub fn export(doc: &Document, opts: &ExportOptions) -> Result<ExportResult, IoError> {
    if doc.mode == ColorMode::Duotone
        && let Some(shown) = duotone_as_rgb(doc)
    {
        let mut r = export(&shown, opts)?;
        r.warnings.push("Duotone written as RGB".into());
        return Ok(r);
    }
    let mut warnings = Vec::new();
    let img = eps_image(doc, &mut warnings)?;
    let o = opts.eps;
    let mut encoding = o.encoding;
    if img.depth == 1 && matches!(encoding, EpsEncoding::Jpeg(_)) {
        // JPEG has no 1-bit form; Photoshop doesn't offer it for Bitmap documents.
        encoding = EpsEncoding::Binary;
        warnings.push("JPEG encoding is not available for Bitmap images: binary was used".into());
    }
    let ps = postscript(doc, &img, encoding, o.interpolate)?;
    let bytes = match o.preview {
        EpsPreview::None => ps,
        EpsPreview::Tiff => dos_eps(&ps, &preview_tiff(doc)?),
    };
    Ok(ExportResult { bytes, warnings })
}

/// The flattened image in EPS's terms (8 bits, opaque, gray / RGB / CMYK / Lab or 1-bit).
fn eps_image(doc: &Document, warnings: &mut Vec<String>) -> Result<EpsImage, IoError> {
    let (w, h) = (doc.size.width, doc.size.height);
    if w == 0 || h == 0 {
        return Err(IoError::Unsupported("an EPS needs an image at least 1 × 1 px".into()));
    }
    if doc.depth != SampleType::U8 {
        warnings.push("EPS holds 8 bits per channel: the image was written at 8 bits".into());
    }
    if doc.mode == ColorMode::Lab {
        // The file keeps Lab like Photoshop's: a single plain layer's own values (over white
        // where it is transparent), else the compositor's Lab, which can only hold colours sRGB
        // can show.
        let rect = Rect::new(0, 0, w as i32, h as i32);
        let (vals, n, alpha) = match crate::flat::single_layer(doc) {
            Some(s) => (s.read_region(rect), s.format().channels(), true),
            None => {
                warnings.push(format!("{} layer(s) flattened; layers, masks and blend modes are not kept", doc.layer_count()));
                let s = photocraft_compose::flatten_to_surface(doc, PixelFormat::new(ColorMode::Lab, SampleType::F32, false), Some([1.0, 1.0, 1.0]));
                (s.read_region(rect), s.format().channels(), false)
            }
        };
        const WHITE: [f32; 3] = [1.0, 128.0 / 255.0, 128.0 / 255.0];
        let mut data = crate::flat::try_buffer(w as usize * h as usize, 3)?;
        let mut translucent = false;
        for px in vals.chunks_exact(n) {
            let a = if alpha { px[3].clamp(0.0, 1.0) } else { 1.0 };
            translucent |= a < 1.0;
            data.extend(px[..3].iter().zip(WHITE).map(|(v, white)| ((v.clamp(0.0, 1.0) * a + white * (1.0 - a)) * 255.0).round() as u8));
        }
        if translucent {
            warnings.push("EPS has no transparency: the image was flattened over white".into());
        }
        return Ok(EpsImage { width: w, height: h, mode: 2, depth: 8, data, icc: None });
    }
    let mut img = document_to_image(doc, warnings)?;
    if img.layout().has_alpha() {
        if img.has_translucency() {
            warnings.push("EPS has no transparency: the image was flattened over white".into());
        }
        img = matte_over_white(&img)?;
    }
    let img = img.convert(img.layout(), CSample::U8);
    let icc = img.icc.clone();
    let (mode, data) = match img.layout() {
        ChannelLayout::Gray if doc.mode == ColorMode::Bitmap => (1, pack_bits(&img)),
        ChannelLayout::Gray => (1, img.data().to_vec()),
        ChannelLayout::Cmyk => (4, img.data().to_vec()),
        _ => (3, img.data().to_vec()),
    };
    let depth = if doc.mode == ColorMode::Bitmap && img.layout() == ChannelLayout::Gray { 1 } else { 8 };
    Ok(EpsImage { width: w, height: h, mode, depth, data, icc })
}

/// 8-bit gray as 1-bit rows (MSB first, padded to a byte; 1 = white, as PostScript's image).
fn pack_bits(img: &Image) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let row = (w as usize).div_ceil(8);
    let mut out = vec![0u8; row * h as usize];
    for (y, src) in img.data().chunks_exact(w as usize).enumerate() {
        for (x, &v) in src.iter().enumerate() {
            if v >= 128 {
                out[y * row + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    out
}

/// The page size in points: the document's print size at its resolution (72 ppi when unset).
fn page_size(doc: &Document) -> (f64, f64) {
    let dpi = f64::from(doc.resolution_dpi);
    let dpi = if dpi.is_finite() && dpi > 0.0 { dpi } else { 72.0 };
    (f64::from(doc.size.width) * 72.0 / dpi, f64::from(doc.size.height) * 72.0 / dpi)
}

/// A DSC text value: printable ASCII, parentheses and backslashes escaped.
fn ps_string(s: &str) -> String {
    let mut o = String::from("(");
    for c in s.chars() {
        match c {
            '(' | ')' | '\\' => {
                o.push('\\');
                o.push(c);
            }
            c if c.is_ascii() && !c.is_ascii_control() => o.push(c),
            _ => o.push('?'),
        }
    }
    o.push(')');
    o
}

/// The EPS program: DSC header, `%ImageData:`, profile, the image operator and the data.
fn postscript(doc: &Document, img: &EpsImage, encoding: EpsEncoding, interpolate: bool) -> Result<Vec<u8>, IoError> {
    let (w, h) = (img.width, img.height);
    let (pw, ph) = page_size(doc);
    let binary = matches!(encoding, EpsEncoding::Binary | EpsEncoding::Jpeg(_));
    let data = match encoding {
        EpsEncoding::Jpeg(q) => jpeg(img, q.percent())?,
        _ => img.data.clone(),
    };

    let mut out = String::new();
    let title = doc.name.as_str();
    out.push_str("%!PS-Adobe-3.0 EPSF-3.0\n");
    out.push_str("%%Creator: PhotoCraft\n");
    out.push_str(&format!("%%Title: {}\n", ps_string(title)));
    out.push_str(&format!("%%CreationDate: {}\n", crate::pdf::pdf_date()));
    out.push_str(&format!("%%BoundingBox: 0 0 {} {}\n", pw.ceil() as i64, ph.ceil() as i64));
    out.push_str(&format!("%%HiResBoundingBox: 0 0 {pw:.4} {ph:.4}\n"));
    out.push_str("%%LanguageLevel: 2\n");
    out.push_str(if binary { "%%DocumentData: Binary\n" } else { "%%DocumentData: Clean7Bit\n" });
    out.push_str(match img.mode {
        1 => "%%DocumentProcessColors: Black\n",
        4 => "%%DocumentProcessColors: Cyan Magenta Yellow Black\n",
        _ => "",
    });
    out.push_str("%%EndComments\n");
    // Right after the comments, where Photoshop looks for it: pixel-interleaved (block size 1),
    // or one row of bits per block for a Bitmap.
    let block = if img.depth == 1 { (w as usize).div_ceil(8) } else { 1 };
    out.push_str(&format!("%ImageData: {w} {h} {} {} 0 {block} {} \"{DATA_START}\"\n", img.depth, img.mode, encoding.code()));
    if let Some(icc) = img.icc.as_ref().filter(|_| img.mode != 2) {
        out.push_str("%%BeginICCProfile: (Photoshop Profile) -1 Hex\n");
        for chunk in icc.chunks(32) {
            out.push_str("% ");
            out.push_str(&hex(chunk));
            out.push('\n');
        }
        out.push_str("%%EndICCProfile\n");
    }
    out.push_str("%%BeginProlog\n%%EndProlog\n");
    out.push_str("gsave\n10 dict begin\n");
    out.push_str(&format!("{pw:.4} {ph:.4} scale\n"));
    let (space, decode) = match img.mode {
        1 => ("/DeviceGray", "[0 1]"),
        2 => (LAB_SPACE, "[0 100 -128 127 -128 127]"),
        4 if matches!(encoding, EpsEncoding::Jpeg(_)) => ("/DeviceCMYK", "[1 0 1 0 1 0 1 0]"),
        4 => ("/DeviceCMYK", "[0 1 0 1 0 1 0 1]"),
        _ => ("/DeviceRGB", "[0 1 0 1 0 1]"),
    };
    let filter = match encoding {
        EpsEncoding::Binary => "",
        EpsEncoding::Ascii => " /ASCIIHexDecode filter",
        EpsEncoding::Ascii85 => " /ASCII85Decode filter",
        EpsEncoding::Jpeg(_) => " /DCTDecode filter",
    };
    out.push_str(&format!(
        "/{DATA_START} {{\n{space} setcolorspace\n<< /ImageType 1 /Width {w} /Height {h} /BitsPerComponent {} /Decode {decode}\n/ImageMatrix [{w} 0 0 -{h} 0 {h}] /Interpolate {interpolate}\n/DataSource currentfile{filter} >> image\n}} bind def\n",
        img.depth
    ));
    let mut body = Vec::with_capacity(data.len() * if binary { 1 } else { 2 } + 64);
    body.extend_from_slice(DATA_START.as_bytes());
    body.push(b'\n');
    match encoding {
        EpsEncoding::Ascii => {
            for chunk in data.chunks(32) {
                body.extend_from_slice(hex(chunk).as_bytes());
                body.push(b'\n');
            }
            body.extend_from_slice(b">\n");
        }
        EpsEncoding::Ascii85 => {
            body.extend_from_slice(&ascii85(&data));
            body.extend_from_slice(b"~>\n");
        }
        _ => {
            body.extend_from_slice(&data);
            body.push(b'\n');
        }
    }
    let mut bytes = out.into_bytes();
    if binary {
        bytes.extend_from_slice(format!("%%BeginBinary: {}\n", body.len()).as_bytes());
    }
    bytes.extend_from_slice(&body);
    if binary {
        bytes.extend_from_slice(b"%%EndBinary\n");
    }
    bytes.extend_from_slice(b"end\ngrestore\n%%Trailer\n%%EOF\n");
    Ok(bytes)
}

/// CIE L*a*b* (D50) as PostScript's `CIEBasedABC` space, from the PostScript Language Reference.
const LAB_SPACE: &str = "[/CIEBasedABC <<\n/RangeABC [0 100 -128 127 -128 127]\n/DecodeABC [{16 add 116 div} bind {500 div} bind {200 div} bind]\n/MatrixABC [1 1 1 1 0 0 0 0 -1]\n/DecodeLMN [\n{dup 6 29 div ge {dup dup mul mul} {4 29 div sub 108 841 div mul} ifelse 0.9642 mul} bind\n{dup 6 29 div ge {dup dup mul mul} {4 29 div sub 108 841 div mul} ifelse} bind\n{dup 6 29 div ge {dup dup mul mul} {4 29 div sub 108 841 div mul} ifelse 0.8249 mul} bind]\n/WhitePoint [0.9642 1 0.8249]\n>>]";

/// The samples as a JPEG (Lab as Photoshop's three 8-bit components, read with its Decode).
fn jpeg(img: &EpsImage, quality: u8) -> Result<Vec<u8>, IoError> {
    let layout = match img.mode {
        1 => ChannelLayout::Gray,
        4 => ChannelLayout::Cmyk,
        _ => ChannelLayout::Rgb,
    };
    let plain = Image::from_raw(img.width, img.height, layout, CSample::U8, img.data.clone())?;
    let enc = EncodeOptions { jpeg_quality: quality, jpeg_chroma_subsampling: false, embed_icc: false, embed_metadata: false, ..Default::default() };
    Ok(codecs::encode(&plain, Format::Jpeg, &enc)?)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[usize::from(b >> 4)] as char);
        s.push(DIGITS[usize::from(b & 15)] as char);
    }
    s
}

/// ASCII85 (without the `~>` end marker), in lines of at most 75 characters.
fn ascii85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 5 / 4 + data.len() / 60 + 8);
    let mut col = 0;
    let mut push = |out: &mut Vec<u8>, c: u8| {
        out.push(c);
        col += 1;
        if col == 75 {
            out.push(b'\n');
            col = 0;
        }
    };
    for chunk in data.chunks(4) {
        let mut b = [0u8; 4];
        b[..chunk.len()].copy_from_slice(chunk);
        let v = u32::from_be_bytes(b);
        if v == 0 && chunk.len() == 4 {
            push(&mut out, b'z');
            continue;
        }
        let mut digits = [0u8; 5];
        let mut x = v;
        for d in digits.iter_mut().rev() {
            *d = (x % 85) as u8 + b'!';
            x /= 85;
        }
        for &d in &digits[..chunk.len() + 1] {
            push(&mut out, d);
        }
    }
    out.push(b'\n');
    out
}

/// An uncompressed 8-bit RGB TIFF of the composite at 72 ppi (at most 1024 px on its long side).
fn preview_tiff(doc: &Document) -> Result<Vec<u8>, IoError> {
    let (pw, ph) = page_size(doc);
    let long = pw.max(ph).round().clamp(1.0, 1024.0) as u32;
    let thumb = photocraft_compose::thumbnail(doc, long.min(doc.size.width.max(doc.size.height)));
    // Flattened over white: the preview stands in for an opaque image.
    let mut rgb = Vec::with_capacity(thumb.pixels.len() / 4 * 3);
    for p in thumb.pixels.as_chunks::<4>().0 {
        let a = u32::from(p[3]);
        for &c in &p[..3] {
            rgb.push(((u32::from(c) * a + 255 * (255 - a) + 127) / 255) as u8);
        }
    }
    let img = Image::from_u8(thumb.width, thumb.height, ChannelLayout::Rgb, rgb)?;
    let enc = EncodeOptions { tiff_compression: codecs::TiffCompression::None, embed_icc: false, embed_metadata: false, ..Default::default() };
    Ok(codecs::encode(&img, Format::Tiff, &enc)?)
}

/// `ps` and `tiff` behind a DOS EPS binary header (no Windows metafile; checksum unused).
fn dos_eps(ps: &[u8], tiff: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(30 + ps.len() + tiff.len());
    out.extend_from_slice(&DOS_EPS);
    let ps_off = 30u32;
    let tiff_off = ps_off + ps.len() as u32;
    for v in [ps_off, ps.len() as u32, 0, 0, tiff_off, tiff.len() as u32] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    out.extend_from_slice(ps);
    out.extend_from_slice(tiff);
    out
}

// ---------- import ----------

/// The PostScript and the TIFF preview of an EPS (a DOS EPS binary header splits them).
fn sections(bytes: &[u8]) -> Result<(&[u8], Option<&[u8]>), IoError> {
    if !bytes.starts_with(&DOS_EPS) {
        return Ok((bytes, None));
    }
    let u = |i: usize| bytes.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let bad = || IoError::Unsupported("damaged DOS EPS header".into());
    let (ps_off, ps_len, tiff_off, tiff_len) = (u(4).ok_or_else(bad)?, u(8).ok_or_else(bad)?, u(20).ok_or_else(bad)?, u(24).ok_or_else(bad)?);
    let ps = bytes.get(ps_off..ps_off.saturating_add(ps_len)).ok_or_else(bad)?;
    let tiff = (tiff_len > 0).then(|| bytes.get(tiff_off..tiff_off.saturating_add(tiff_len))).flatten();
    Ok((ps, tiff))
}

/// The lines of `ps` with the offset just past each one's terminator (CR, LF or CR LF).
fn lines(ps: &[u8]) -> impl Iterator<Item = (&[u8], usize)> {
    let mut i = 0;
    std::iter::from_fn(move || {
        if i >= ps.len() {
            return None;
        }
        let start = i;
        while i < ps.len() && ps[i] != b'\n' && ps[i] != b'\r' {
            i += 1;
        }
        let line = &ps[start..i];
        if ps.get(i) == Some(&b'\r') {
            i += 1;
            if ps.get(i) == Some(&b'\n') {
                i += 1;
            }
        } else if ps.get(i) == Some(&b'\n') {
            i += 1;
        }
        Some((line, i))
    })
}

/// Photoshop's `%ImageData:` comment.
#[derive(Debug, Clone, PartialEq)]
struct ImageData {
    columns: u32,
    rows: u32,
    depth: u32,
    mode: u32,
    pad: u32,
    block: usize,
    encoding: u32,
    start: String,
}

fn parse_image_data(line: &str) -> Option<ImageData> {
    let rest = line.strip_prefix("%ImageData:")?.trim();
    let (nums, start) = match rest.find('"') {
        Some(q) => (&rest[..q], rest[q + 1..].trim_end().trim_end_matches('"').to_string()),
        None => (rest, String::new()),
    };
    let v: Vec<u64> = nums.split_whitespace().map_while(|t| t.parse().ok()).collect();
    if v.len() < 7 {
        return None;
    }
    let u = |i: usize| u32::try_from(v[i]).ok();
    Some(ImageData { columns: u(0)?, rows: u(1)?, depth: u(2)?, mode: u(3)?, pad: u(4)?, block: usize::try_from(v[5]).ok()?, encoding: u(6)?, start })
}

/// Opens an EPS: its `%ImageData:` pixels, else its TIFF preview.
pub fn import(name: &str, bytes: &[u8]) -> Result<ImportResult, IoError> {
    let (ps, preview) = sections(bytes)?;
    let mut info = None;
    let mut hires: Option<(f64, f64)> = None;
    let mut bbox: Option<(f64, f64)> = None;
    let mut icc: Option<Vec<u8>> = None;
    let mut in_icc = false;
    let mut data_at = None;
    for (line, end) in lines(ps) {
        let text = String::from_utf8_lossy(line);
        if in_icc {
            if text.starts_with("%%EndICCProfile") {
                in_icc = false;
            } else if let Some(h) = text.strip_prefix('%') {
                icc.get_or_insert_with(Vec::new).extend(unhex(h.as_bytes()));
            }
            continue;
        }
        if info.is_none()
            && let Some(d) = parse_image_data(&text)
        {
            info = Some(d);
            continue;
        }
        let corners = |s: &str| {
            let v: Vec<f64> = s.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            (v.len() == 4).then(|| (v[2] - v[0], v[3] - v[1]))
        };
        if let Some(r) = text.strip_prefix("%%HiResBoundingBox:") {
            hires = corners(r);
        } else if let Some(r) = text.strip_prefix("%%BoundingBox:") {
            bbox = bbox.or(corners(r));
        } else if text.starts_with("%%BeginICCProfile") {
            in_icc = true;
            icc = None;
        } else if let Some(d) = info.as_ref()
            && !d.start.is_empty()
            && text.trim_end() == d.start
        {
            data_at = Some(end);
            break;
        }
    }
    let Some((info, at)) = info.zip(data_at) else {
        return import_preview(name, preview);
    };
    let mut warnings = Vec::new();
    let mut doc = decode_image(name, &info, &ps[at..], &mut warnings)?;
    // The print size the bounding box gives, at the pixel size, is the resolution.
    if let Some((pw, _)) = hires.or(bbox).filter(|(w, h)| *w > 0.0 && *h > 0.0) {
        let dpi = f64::from(info.columns) * 72.0 / pw;
        if dpi.is_finite() && dpi > 0.0 {
            doc.resolution_dpi = dpi.clamp(1.0, 1.0e6) as f32;
        }
    }
    let profile_space = |p: &[u8]| p.get(16..20).map(<[u8]>::to_vec);
    let wanted: &[u8] = match doc.mode {
        ColorMode::Grayscale | ColorMode::Bitmap => b"GRAY",
        ColorMode::Cmyk => b"CMYK",
        ColorMode::Lab => b"Lab ",
        _ => b"RGB ",
    };
    if doc.mode != ColorMode::Bitmap
        && let Some(p) = icc.filter(|p| p.len() >= 128 && profile_space(p).as_deref() == Some(wanted))
    {
        doc.icc_profile = Some(Arc::new(p));
    }
    Ok(ImportResult { document: doc, warnings, source_read_only: false, preview_only: false })
}

/// A generic EPS: its TIFF preview, else an explanation.
fn import_preview(name: &str, preview: Option<&[u8]>) -> Result<ImportResult, IoError> {
    let Some(tiff) = preview else {
        return Err(IoError::Unsupported(
            "this EPS holds PostScript artwork without Photoshop image data; PhotoCraft can't render PostScript, so open a PDF or SVG export of it instead"
                .into(),
        ));
    };
    let mut r = crate::flat::import_flat(name, tiff)?;
    r.document.name = name.to_string();
    r.preview_only = true;
    r.warnings.insert(0, "this EPS holds PostScript artwork; PhotoCraft opened its low-resolution TIFF preview".into());
    Ok(r)
}

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Hex digits to bytes, skipping everything else; stops at `>`.
fn unhex(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() / 2);
    let mut hi = None;
    for &c in text {
        if c == b'>' {
            break;
        }
        if let Some(v) = hex_value(c) {
            match hi.take() {
                Some(h) => out.push(h << 4 | v),
                None => hi = Some(v),
            }
        }
    }
    out
}

/// At most `limit` bytes of hex data from `text`.
fn unhex_limited(text: &[u8], limit: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(limit);
    let mut hi = None;
    for &c in text {
        if out.len() >= limit || c == b'>' {
            break;
        }
        if let Some(v) = hex_value(c) {
            match hi.take() {
                Some(h) => out.push(h << 4 | v),
                None => hi = Some(v),
            }
        }
    }
    out
}

/// ASCII85 to bytes (whitespace skipped, `z` for four zeros), up to `limit` bytes or `~>`.
fn unascii85(text: &[u8], limit: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(limit);
    let mut group = [0u32; 5];
    let mut n = 0;
    let flush = |out: &mut Vec<u8>, group: &[u32; 5], n: usize| {
        let mut g = *group;
        for d in g.iter_mut().skip(n) {
            *d = 84;
        }
        let v = g.iter().fold(0u64, |acc, &d| acc * 85 + u64::from(d));
        let b = (v.min(u64::from(u32::MAX)) as u32).to_be_bytes();
        out.extend_from_slice(&b[..n.saturating_sub(1)]);
    };
    for &c in text {
        if out.len() >= limit {
            break;
        }
        match c {
            b'~' => break,
            b'z' if n == 0 => out.extend_from_slice(&[0; 4]),
            b'!'..=b'u' => {
                group[n] = u32::from(c - b'!');
                n += 1;
                if n == 5 {
                    flush(&mut out, &group, 5);
                    n = 0;
                }
            }
            _ => {}
        }
    }
    if n > 1 && out.len() < limit {
        flush(&mut out, &group, n);
    }
    out.truncate(limit);
    out
}

/// The `%ImageData:` pixels as a one-layer document.
fn decode_image(name: &str, d: &ImageData, data: &[u8], warnings: &mut Vec<String>) -> Result<Document, IoError> {
    let (w, h) = (d.columns, d.rows);
    let (mode, colors) = match (d.mode, d.depth) {
        (1, 1) => (ColorMode::Bitmap, 1usize),
        (1, 8) => (ColorMode::Grayscale, 1),
        (2, 8) => (ColorMode::Lab, 3),
        (3, 8) => (ColorMode::Rgb, 3),
        (4, 8) => (ColorMode::Cmyk, 4),
        (m, b) => return Err(IoError::Unsupported(format!("EPS image data in mode {m} at {b} bits per channel"))),
    };
    codecs::Limits::default().check(w, h, if colors == 4 { ChannelLayout::Cmyk } else { ChannelLayout::Rgb }, CSample::U8)?;
    let channels = colors + d.pad as usize;
    let row_bytes = if d.depth == 1 { (w as usize).div_ceil(8) } else { w as usize };
    let total = row_bytes * h as usize * channels;
    let samples: Vec<u8> = match d.encoding {
        1 => data.get(..total).map(<[u8]>::to_vec).unwrap_or_else(|| data.to_vec()),
        2 => unhex_limited(data, total),
        7 => unascii85(data, total),
        3..=6 => {
            let img = codecs::decode_as(Format::Jpeg, data)?;
            if img.dimensions() != (w, h) {
                return Err(IoError::Unsupported("the EPS's JPEG data doesn't match its declared size".into()));
            }
            // The decoder has already undone Adobe's inverted CMYK; Lab comes back as the three
            // components it was encoded from, as PostScript's DCTDecode returns them.
            let want = match colors {
                1 => ChannelLayout::Gray,
                4 => ChannelLayout::Cmyk,
                _ => ChannelLayout::Rgb,
            };
            let img = img.convert(want, CSample::U8);
            return build_document(name, mode, w, h, img.data(), colors, 0, 1);
        }
        e => return Err(IoError::Unsupported(format!("EPS image data encoding {e}"))),
    };
    if samples.len() < total {
        warnings.push("the EPS image data ends early; the missing rows are white".into());
    }
    if d.depth == 1 {
        let mut gray = vec![255u8; w as usize * h as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let byte = samples.get(y * row_bytes * channels + d.pad as usize * row_bytes + x / 8).copied().unwrap_or(0xFF);
                gray[y * w as usize + x] = if byte & (0x80 >> (x % 8)) != 0 { 255 } else { 0 };
            }
        }
        return build_document(name, mode, w, h, &gray, 1, 0, 1);
    }
    build_document(name, mode, w, h, &samples, colors, d.pad as usize, d.block)
}

/// A document from 8-bit samples of `colors` channels after `pad` others, pixel-interleaved
/// (`block` 1) or a row of each channel in turn.
#[allow(clippy::too_many_arguments)]
fn build_document(name: &str, mode: ColorMode, w: u32, h: u32, samples: &[u8], colors: usize, pad: usize, block: usize) -> Result<Document, IoError> {
    let channels = colors + pad;
    let (wu, hu) = (w as usize, h as usize);
    let line = block > 1;
    let white = |c: usize| if mode == ColorMode::Cmyk { 0u8 } else if mode == ColorMode::Lab && c > 0 { 128 } else { 255 };
    let at = |x: usize, y: usize, c: usize| -> u8 {
        let i = if line { (y * channels + pad + c) * wu + x } else { (y * wu + x) * channels + pad + c };
        samples.get(i).copied().unwrap_or_else(|| white(c))
    };
    let mut doc = Document::new(name, Size::new(w, h), mode, SampleType::U8);
    let fmt = doc.pixel_format();
    let n = fmt.channels();
    let stored = fmt.mode.color_channels();
    let mut s = Surface::new(fmt);
    let band = ((8 << 20) / (wu * n * 4).max(1)).max(1);
    let mut y0 = 0;
    while y0 < hu {
        let y1 = (y0 + band).min(hu);
        let mut vals = Vec::with_capacity((y1 - y0) * wu * n);
        for y in y0..y1 {
            for x in 0..wu {
                for c in 0..stored {
                    vals.push(f32::from(at(x, y, c.min(colors - 1))) / 255.0);
                }
                vals.push(1.0);
            }
        }
        s.write_region(Rect::new(0, y0 as i32, w as i32, y1 as i32), &vals);
        y0 = y1;
    }
    s.prune();
    let mut bg = Layer::new("Background", LayerContent::Raster(s));
    bg.locks.transparency = true;
    bg.locks.position = true;
    doc.layers.push(bg);
    Ok(doc)
}

#[cfg(test)]
#[path = "eps_tests.rs"]
mod tests;
