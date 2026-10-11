use std::io::Read as _;

use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::{Color, Document, Layer, Size};
use photocraft_geom::Rect;

use super::*;

fn doc(mode: ColorMode, depth: SampleType, w: u32, h: u32) -> Document {
    Document::with_background("Poster.psd", Size::new(w, h), mode, depth, Color::WHITE)
}

fn pdf(doc: &Document, compression: PdfCompression) -> Vec<u8> {
    let opts = ExportOptions { pdf: compression, ..Default::default() };
    crate::export(doc, "out.pdf", &opts).map(|r| r.bytes).unwrap_or_default()
}

/// Object `n`'s text and stream data (the stream inflated when Flate-compressed).
fn object(pdf: &[u8], n: usize) -> (String, Vec<u8>) {
    let head = format!("\n{n} 0 obj\n");
    let at = pdf.windows(head.len()).position(|w| w == head.as_bytes()).map(|p| p + head.len()).unwrap_or_else(|| panic!("no object {n}"));
    let end = pdf[at..].windows(7).position(|w| w == b"endobj\n").map(|p| at + p).unwrap_or(pdf.len());
    let body = &pdf[at..end];
    let Some(s) = body.windows(8).position(|w| w == b"\nstream\n") else { return (String::from_utf8_lossy(body).into_owned(), Vec::new()) };
    let dict = String::from_utf8_lossy(&body[..s]).into_owned();
    let data = &body[s + 8..body.len() - b"\nendstream\n".len()];
    let data = if dict.contains("/FlateDecode") {
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(data).read_to_end(&mut out).unwrap_or_default();
        out
    } else {
        data.to_vec()
    };
    (dict, data)
}

#[test]
fn rgb_document_is_one_page_at_its_print_size() {
    let mut d = doc(ColorMode::Rgb, SampleType::U8, 300, 150);
    d.resolution_dpi = 300.0;
    let bytes = pdf(&d, PdfCompression::Zip);
    assert!(bytes.starts_with(b"%PDF-1.6"));
    assert!(bytes.ends_with(b"%%EOF\n"));
    let (page, _) = object(&bytes, 3);
    assert!(page.contains("/MediaBox [0 0 72.000 36.000]"), "{page}");
    let (img, data) = object(&bytes, 5);
    assert!(img.contains("/Width 300 /Height 150"), "{img}");
    assert!(img.contains("/ColorSpace [/ICCBased 6 0 R] /BitsPerComponent 8"), "{img}");
    assert!(!img.contains("/SMask"), "an opaque image has no soft mask: {img}");
    assert_eq!(data.len(), 300 * 150 * 3);
    assert!(data.iter().all(|&v| v == 255), "white background");
    let (icc, profile) = object(&bytes, 6);
    assert!(icc.contains("/N 3 /Alternate /DeviceRGB"), "{icc}");
    assert!(Profile::parse(&profile).is_ok_and(|p| p.color_space == ColorSpace::Rgb));
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("/Title (Poster)"), "title from the document name");
}

#[test]
fn xref_offsets_point_at_their_objects() {
    let bytes = pdf(&doc(ColorMode::Rgb, SampleType::U8, 4, 4), PdfCompression::Zip);
    let tail = String::from_utf8_lossy(&bytes[bytes.len() - 32..]).into_owned();
    let start: usize = tail.rsplit("startxref\n").next().and_then(|t| t.lines().next()).and_then(|l| l.trim().parse().ok()).unwrap_or(0);
    let xref = String::from_utf8_lossy(&bytes[start..]).into_owned();
    assert!(xref.starts_with("xref\n0 "), "startxref names the xref table");
    let mut seen = 0;
    for (i, line) in xref.lines().skip(3).take_while(|l| l.ends_with(" n ")).enumerate() {
        let off: usize = line[..10].parse().unwrap_or(0);
        assert!(bytes[off..].starts_with(format!("{} 0 obj\n", i + 1).as_bytes()), "object {} at {off}", i + 1);
        seen += 1;
    }
    assert!(seen >= 7, "{seen} objects");
}

#[test]
fn cmyk_single_layer_keeps_its_exact_inks() {
    let mut d = doc(ColorMode::Cmyk, SampleType::U8, 8, 4);
    // 100 % K only (a rich black would come back from an RGB round trip), 40 % cyan on the right.
    if let Some(s) = d.layers[0].surface_mut() {
        s.fill_rect(Rect::new(0, 0, 4, 4), &[0.0, 0.0, 0.0, 1.0, 1.0]);
        s.fill_rect(Rect::new(4, 0, 8, 4), &[0.4, 0.0, 0.0, 0.0, 1.0]);
    }
    let bytes = pdf(&d, PdfCompression::Zip);
    let (img, data) = object(&bytes, 5);
    assert!(img.contains("/BitsPerComponent 8 /Filter /FlateDecode"), "{img}");
    let (icc, profile) = object(&bytes, 6);
    assert!(icc.contains("/N 4 /Alternate /DeviceCMYK"), "{icc}");
    assert!(Profile::parse(&profile).is_ok_and(|p| p.color_space == ColorSpace::Cmyk));
    assert_eq!(&data[..4], &[0, 0, 0, 255], "pure K stays pure K");
    assert_eq!(&data[4 * 4..4 * 4 + 4], &[102, 0, 0, 0], "40 % cyan");
}

#[test]
fn layered_cmyk_composite_stays_cmyk_with_transparency() {
    let mut d = Document::new("Layers", Size::new(6, 6), ColorMode::Cmyk, SampleType::U8);
    let mut l = Layer::raster("ink", d.pixel_format());
    if let Some(s) = l.surface_mut() {
        s.fill_rect(Rect::new(0, 0, 3, 6), &[0.0, 1.0, 1.0, 0.0, 1.0]);
    }
    let mut top = Layer::raster("top", d.pixel_format());
    top.opacity = 0.5;
    if let Some(s) = top.surface_mut() {
        s.fill_rect(Rect::new(0, 0, 6, 3), &[1.0, 0.0, 0.0, 0.0, 1.0]);
    }
    d.layers.push(l);
    d.layers.push(top);
    let r = crate::export(&d, "x.pdf", &ExportOptions::default()).unwrap_or_else(|e| panic!("{e}"));
    assert!(!r.warnings.iter().any(|w| w.contains("sRGB")), "{:?}", r.warnings);
    let (img, data) = object(&r.bytes, 5);
    assert!(img.contains("/SMask 7 0 R"), "{img}");
    assert_eq!(data.len(), 6 * 6 * 4, "four inks per pixel");
    // Bottom-left: the magenta + yellow layer alone, a strong red in ink.
    let p = &data[(5 * 6) * 4..(5 * 6) * 4 + 4];
    assert!(p[1] > 200 && p[2] > 200 && p[0] < 40, "{p:?}");
    let (mask, alpha) = object(&r.bytes, 7);
    assert!(mask.contains("/ColorSpace /DeviceGray"), "{mask}");
    assert_eq!(alpha[5 * 6 + 5], 0, "bottom right is empty");
    assert_eq!(alpha[0], 255, "top left is covered");
}

#[test]
fn sixteen_bit_keeps_sixteen_bits_big_endian() {
    let bytes = pdf(&doc(ColorMode::Grayscale, SampleType::U16, 3, 2), PdfCompression::Zip);
    let (img, data) = object(&bytes, 5);
    assert!(img.contains("/BitsPerComponent 16"), "{img}");
    assert_eq!(data.len(), 3 * 2 * 2);
    assert_eq!(&data[..2], &[0xff, 0xff]);
    let (icc, _) = object(&bytes, 6);
    assert!(icc.contains("/N 1 /Alternate /DeviceGray"), "{icc}");
}

#[test]
fn jpeg_compression_is_dct_and_cmyk_jpeg_is_decoded_inverted() {
    let rgb = pdf(&doc(ColorMode::Rgb, SampleType::U16, 16, 16), PdfCompression::Jpeg(80));
    let (img, data) = object(&rgb, 5);
    assert!(img.contains("/BitsPerComponent 8 /Filter /DCTDecode"), "{img}");
    assert!(!img.contains("/Decode"), "{img}");
    assert!(data.starts_with(&[0xff, 0xd8]), "a JPEG stream");
    let cmyk = pdf(&doc(ColorMode::Cmyk, SampleType::U8, 16, 16), PdfCompression::Jpeg(80));
    let (img, _) = object(&cmyk, 5);
    assert!(img.contains("/Filter /DCTDecode /Decode [1 0 1 0 1 0 1 0]"), "{img}");
}

#[test]
fn metadata_follows_the_xmp_option() {
    let mut d = doc(ColorMode::Rgb, SampleType::U8, 2, 2);
    d.metadata.xmp = Some("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>".into());
    let all = pdf(&d, PdfCompression::Zip);
    assert!(String::from_utf8_lossy(&all).contains("/Type /Metadata /Subtype /XML"));
    let none = crate::export(&d, "x.pdf", &ExportOptions { xmp: XmpEmbed::None, ..Default::default() }).map(|r| r.bytes).unwrap_or_default();
    assert!(!String::from_utf8_lossy(&none).contains("/Metadata"));
}

/// `PHOTOCRAFT_PDF_OUT=<dir>` writes sample PDFs for checking in other readers (Poppler,
/// Ghostscript, Acrobat).
#[test]
fn sample_pdfs_for_other_readers() {
    let Some(dir) = std::env::var_os("PHOTOCRAFT_PDF_OUT") else { return };
    let dir = std::path::PathBuf::from(dir);
    let mut cmyk = Document::new("cmyk-layers", Size::new(300, 200), ColorMode::Cmyk, SampleType::U8);
    let mut a = Layer::raster("magenta", cmyk.pixel_format());
    if let Some(s) = a.surface_mut() {
        s.fill_rect(Rect::new(0, 0, 200, 200), &[0.0, 1.0, 0.0, 0.0, 1.0]);
    }
    let mut b = Layer::raster("cyan", cmyk.pixel_format());
    b.opacity = 0.6;
    if let Some(s) = b.surface_mut() {
        s.fill_rect(Rect::new(100, 50, 300, 150), &[1.0, 0.0, 0.0, 0.0, 1.0]);
    }
    cmyk.layers.push(a);
    cmyk.layers.push(b);
    cmyk.resolution_dpi = 150.0;
    let mut rgb = doc(ColorMode::Rgb, SampleType::U8, 400, 300);
    if let Some(s) = rgb.layers[0].surface_mut() {
        s.fill_rect(Rect::new(50, 50, 350, 250), &[0.1, 0.4, 0.9, 1.0]);
    }
    let samples = [
        ("rgb-zip.pdf", rgb.clone(), PdfCompression::Zip),
        ("rgb-jpeg.pdf", rgb, PdfCompression::Jpeg(85)),
        ("cmyk-layers-zip.pdf", cmyk.clone(), PdfCompression::Zip),
        ("cmyk-layers-jpeg.pdf", cmyk, PdfCompression::Jpeg(90)),
        ("gray16.pdf", doc(ColorMode::Grayscale, SampleType::U16, 200, 100), PdfCompression::Zip),
    ];
    for (name, d, c) in samples {
        let _ = std::fs::write(dir.join(name), pdf(&d, c));
    }
}
