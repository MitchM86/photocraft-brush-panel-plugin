//! Photoshop PDF (`.pdf`, Save As / Export As): the flattened image on one page the size of the
//! document's print size (pixels ÷ resolution), as Photoshop writes it without editing
//! capabilities.
//!
//! The image keeps the document's colour model: CMYK stays CMYK (a single plain layer keeps its
//! ink values exactly; a layered composite is converted back through the document's CMYK profile,
//! as the PSD merged image is), grayscale stays gray, everything else is RGB. The colour profile
//! is embedded (`/ICCBased`; untagged documents get the profile they are shown with), 16-bit
//! documents keep 16 bits with ZIP, and transparency is kept as a soft mask. JPEG compression is
//! 8-bit; Adobe-style CMYK JPEGs store inverted ink, so they carry a `/Decode` array as
//! Photoshop's do.
//!
//! With editing data (Photoshop's "Preserve Photoshop Editing Capabilities", on for Save As) the
//! layered document goes in too, as a `.pcraft` bundle the first page's `/PieceInfo` names (see
//! [`crate::pdf_import`]), so the PDF reopens with its layers and saves back to itself.

use std::io::Write as _;

use photocraft_cms::{Builtin, ColorSpace, Profile};
use photocraft_codecs::{self as codecs, ChannelLayout, EncodeOptions, Format, Image, SampleType as CSample};
use photocraft_color::ColorMode;
use photocraft_doc::Document;

use crate::flat::{document_to_image, duotone_as_rgb};
use crate::{ExportOptions, ExportResult, IoError, XmpEmbed};

/// How the image is compressed (Photoshop's PDF Compression options).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PdfCompression {
    /// Lossless (Flate), any depth.
    #[default]
    Zip,
    /// JPEG at this quality (1–100), 8-bit.
    Jpeg(u8),
}

/// The document as a one-page PDF.
pub fn export(doc: &Document, opts: &ExportOptions) -> Result<ExportResult, IoError> {
    if doc.mode == ColorMode::Duotone
        && let Some(shown) = duotone_as_rgb(doc)
    {
        let mut r = export(&shown, opts)?;
        r.warnings.push("Duotone written as RGB".into());
        return Ok(r);
    }
    let mut warnings = Vec::new();
    let img = document_to_image(doc, &mut warnings)?;
    let jpeg = match opts.pdf {
        PdfCompression::Jpeg(q) => Some(q.clamp(1, 100)),
        PdfCompression::Zip => None,
    };
    if jpeg.is_some() && img.sample_type() != CSample::U8 {
        warnings.push("JPEG compression is 8-bit: the image was written at 8 bits per channel".into());
    }
    let xmp = (opts.xmp == XmpEmbed::All).then_some(doc.metadata.xmp.as_deref()).flatten();
    let editing = if opts.pdf_editable { Some(photocraft_format::save_to_bytes(doc, &photocraft_format::SaveOptions::default())?) } else { None };
    let bytes = write_pdf(&img, page_size(doc), jpeg, &doc.name, xmp, editing.as_deref())?;
    Ok(ExportResult { bytes, warnings })
}

/// The page size in points: the document's print size at its resolution (72 ppi when unset).
fn page_size(doc: &Document) -> (f64, f64) {
    let dpi = f64::from(doc.resolution_dpi);
    let dpi = if dpi.is_finite() && dpi > 0.0 { dpi } else { 72.0 };
    (f64::from(doc.size.width) * 72.0 / dpi, f64::from(doc.size.height) * 72.0 / dpi)
}

/// The profile to embed for `layout`: the image's own when it describes that colour space, else
/// the one PhotoCraft shows untagged pixels with (sRGB, sGray, the built-in coated CMYK).
fn embedded_profile(img: &Image) -> Vec<u8> {
    let layout = img.layout();
    let (space, fallback) = if layout.is_cmyk() {
        (ColorSpace::Cmyk, Builtin::CoatedCmyk)
    } else if layout.is_rgb() {
        (ColorSpace::Rgb, Builtin::Srgb)
    } else {
        (ColorSpace::Gray, Builtin::SGray)
    };
    match img.icc.as_ref().filter(|b| Profile::parse(b).is_ok_and(|p| p.color_space == space)) {
        Some(b) => b.clone(),
        None => fallback.profile().to_bytes().to_vec(),
    }
}

/// Now as a PDF date (`D:YYYYMMDDHHmmSSZ`, UTC); the epoch where there is no clock (the web).
pub(crate) fn pdf_date() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    #[cfg(target_arch = "wasm32")]
    let secs = 0u64;
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("(D:{y:04}{m:02}{d:02}{:02}{:02}{:02}Z)", rem / 3600, rem / 60 % 60, rem % 60)
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::with_capacity(data.len() / 2), flate2::Compression::default());
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

/// A literal string (non-ASCII as `?`, as the PDF's standard encoding can't hold it).
fn pdf_string(s: &str) -> String {
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

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut o = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    o.extend_from_slice(data);
    o.extend_from_slice(b"\nendstream");
    o
}

/// The colour samples and the alpha samples of `img` (`sample` per channel, big-endian as PDF
/// stores 16-bit samples), split apart.
fn split_samples(img: &Image) -> (Vec<u8>, Option<Vec<u8>>) {
    let layout = img.layout();
    let bps = img.sample_type().bytes();
    let (ch, cc) = (layout.channels(), layout.color_channels());
    let px = ch * bps;
    let n = img.pixel_count();
    let mut color = Vec::with_capacity(n * cc * bps);
    let mut alpha = layout.has_alpha().then(|| Vec::with_capacity(n * bps));
    let put = |out: &mut Vec<u8>, s: &[u8]| {
        if bps == 2 {
            out.extend_from_slice(&u16::from_ne_bytes([s[0], s[1]]).to_be_bytes());
        } else {
            out.extend_from_slice(s);
        }
    };
    for p in img.data().chunks_exact(px) {
        for c in 0..cc {
            put(&mut color, &p[c * bps..(c + 1) * bps]);
        }
        if let Some(a) = alpha.as_mut() {
            put(a, &p[cc * bps..]);
        }
    }
    (color, alpha)
}

/// Writes the one-page PDF: page `(w, h)` points, the image filling it.
fn write_pdf(img: &Image, (pw, ph): (f64, f64), jpeg: Option<u8>, name: &str, xmp: Option<&str>, editing: Option<&[u8]>) -> Result<Vec<u8>, IoError> {
    // 8 or 16 bits (PDF has no float samples); JPEG is 8-bit.
    let sample = match img.sample_type() {
        CSample::U8 => CSample::U8,
        _ if jpeg.is_some() => CSample::U8,
        _ => CSample::U16,
    };
    let img = img.converted(img.layout(), sample);
    let layout = img.layout();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err(IoError::Unsupported("a PDF page needs an image at least 1 × 1 px".into()));
    }
    let cc = layout.color_channels();
    let device = match cc {
        1 => "/DeviceGray",
        4 => "/DeviceCMYK",
        _ => "/DeviceRGB",
    };
    let bpc = sample.bytes() * 8;
    let (color, alpha) = split_samples(&img);
    let (filter, data, decode) = match jpeg {
        Some(q) => {
            let opaque = match layout.color_channels() {
                1 => ChannelLayout::Gray,
                4 => ChannelLayout::Cmyk,
                _ => ChannelLayout::Rgb,
            };
            let plain = Image::from_raw(w, h, opaque, CSample::U8, color)?;
            let enc = EncodeOptions { jpeg_quality: q, jpeg_chroma_subsampling: false, embed_icc: false, embed_metadata: false, ..Default::default() };
            let bytes = codecs::encode(&plain, Format::Jpeg, &enc)?;
            // Adobe CMYK JPEGs hold 255 for no ink.
            ("/DCTDecode", bytes, if cc == 4 { " /Decode [1 0 1 0 1 0 1 0]" } else { "" })
        }
        None => ("/FlateDecode", deflate(&color), ""),
    };

    // Objects: 1 catalog, 2 pages, 3 page, 4 contents, 5 image, 6 ICC profile, then the soft
    // mask, the XMP packet, the editing data and the document info as present.
    let mut next = 7;
    let mut take = || {
        let id = next;
        next += 1;
        id
    };
    let smask_id = alpha.as_ref().map(|_| take());
    let xmp_id = xmp.map(|_| take());
    let editing_id = editing.map(|_| take());
    let info_id = take();

    let mut objs: Vec<Vec<u8>> = Vec::new();
    let meta = xmp_id.map_or(String::new(), |id| format!(" /Metadata {id} 0 R"));
    objs.push(format!("<< /Type /Catalog /Pages 2 0 R{meta} >>").into_bytes());
    objs.push(b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec());
    let piece = editing_id.map_or(String::new(), |id| {
        let date = pdf_date();
        format!(
            " /LastModified {date} /PieceInfo << /{} << /LastModified {date} /Private << /Document {id} 0 R >> >> >>",
            String::from_utf8_lossy(crate::pdf_import::PIECE)
        )
    });
    objs.push(
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.3} {ph:.3}] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R{piece} >>")
            .into_bytes(),
    );
    let content = format!("q {pw:.3} 0 0 {ph:.3} 0 0 cm /Im0 Do Q\n");
    objs.push(stream("", content.as_bytes()));
    let smask = smask_id.map_or(String::new(), |id| format!(" /SMask {id} 0 R"));
    objs.push(stream(
        &format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace [/ICCBased 6 0 R] /BitsPerComponent {bpc} /Filter {filter}{decode}{smask}"),
        &data,
    ));
    let icc = deflate(&embedded_profile(&img));
    objs.push(stream(&format!("/N {cc} /Alternate {device} /Filter /FlateDecode"), &icc));
    if let Some(a) = alpha {
        objs.push(stream(
            &format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent {bpc} /Filter /FlateDecode"),
            &deflate(&a),
        ));
    }
    if let Some(x) = xmp {
        objs.push(stream("/Type /Metadata /Subtype /XML", x.as_bytes()));
    }
    if let Some(e) = editing {
        // Already compressed (zstd tiles in a zip): stored as it is.
        objs.push(stream("", e));
    }
    let title = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    objs.push(format!("<< /Title {} /Producer (PhotoCraft) /Creator (PhotoCraft) >>", pdf_string(title)).into_bytes());

    // PDF 1.6: 16-bit samples (1.5) and ICC v4 profiles (1.6).
    let mut out: Vec<u8> = b"%PDF-1.6\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::with_capacity(objs.len());
    for (i, body) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R /Info {info_id} 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Ok(out)
}

#[cfg(test)]
#[path = "pdf_tests.rs"]
mod tests;
