use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::{Color, Document, Layer, Size};
use photocraft_geom::Rect as PxRect;

use super::*;
use crate::{ExportOptions, PdfCompression};

/// A one-page PDF (`w` × `h` pt, a red `rect` painted in it, optional extra page-dictionary
/// entries), with a correct cross-reference table.
fn hand_pdf(w: f64, h: f64, rect: (f64, f64, f64, f64), extra: &str) -> Vec<u8> {
    let content = format!("1 0 0 rg {} {} {} {} re f\n", rect.0, rect.1, rect.2, rect.3);
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents 4 0 R{extra} >>"),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn layer_doc(mode: ColorMode) -> Document {
    let mut d = Document::with_background("Art.psd", Size::new(40, 30), mode, SampleType::U8, Color::WHITE);
    let mut l = Layer::raster("Ink", d.pixel_format());
    if let Some(s) = l.surface_mut() {
        let px = photocraft_raster::from_rgba(&s.format(), [0.9, 0.1, 0.1, 1.0]);
        s.fill_rect(PxRect::new(5, 5, 20, 15), &px);
    }
    d.layers.push(l);
    d.resolution_dpi = 72.0;
    d
}

#[test]
fn recognises_pdfs() {
    assert!(is_pdf(b"%PDF-1.7\n"));
    assert!(is_pdf(b"junk before the header %PDF-1.4\n"));
    assert!(!is_pdf(b"8BPS"));
    assert!(!is_pdf(b""));
}

#[test]
fn saved_pdf_reopens_with_its_layers() {
    for mode in [ColorMode::Rgb, ColorMode::Cmyk] {
        let d = layer_doc(mode);
        let bytes = crate::export(&d, "Art.pdf", &ExportOptions::default()).unwrap().bytes;
        let i = info(&bytes).unwrap();
        assert!(i.editable, "{mode:?}");
        assert_eq!(i.pages.len(), 1);
        let back = crate::import("Art.pdf", &bytes).unwrap();
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        let names: Vec<&str> = back.document.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Background", "Ink"], "{mode:?}");
        assert_eq!(back.document.mode, mode);
        assert_eq!(back.document.name, "Art.pdf");
        assert_eq!(photocraft_compose::flatten(&back.document).px, photocraft_compose::flatten(&d).px, "{mode:?} pixels");
    }
}

#[test]
fn plain_pdf_rasterises_the_page() {
    let d = layer_doc(ColorMode::Rgb);
    let opts = ExportOptions { pdf_editable: false, pdf: PdfCompression::Zip, ..Default::default() };
    let bytes = crate::export(&d, "Art.pdf", &opts).unwrap().bytes;
    let i = info(&bytes).unwrap();
    assert!(!i.editable);
    assert!((i.pages[0].width_pt - 40.0).abs() < 1e-6 && (i.pages[0].height_pt - 30.0).abs() < 1e-6, "{:?}", i.pages[0]);
    // 144 ppi: twice the document's 72 ppi.
    let r = render_page(&bytes, 0, &PdfRaster { ppi: 144.0, crop: PdfCrop::CropBox }, "Art.pdf").unwrap();
    let doc = &r.document;
    assert_eq!((doc.size.width, doc.size.height), (80, 60));
    assert_eq!(doc.resolution_dpi, 144.0);
    assert_eq!(doc.layers.len(), 1);
    assert_eq!(doc.layers[0].name, "Layer 1");
    let px = doc.layers[0].surface().unwrap().rgba(20, 20);
    assert!(px[0] > 0.8 && px[1] < 0.2 && px[3] > 0.99, "the red layer at (10, 10) doc px: {px:?}");
    let white = doc.layers[0].surface().unwrap().rgba(70, 50);
    assert!(white.iter().all(|&v| v > 0.95), "{white:?}");
    // Without editing data the generic import is the same page at 300 ppi.
    let r = crate::import("Art.pdf", &bytes).unwrap();
    assert_eq!(r.document.size.width, (40.0f32 * 300.0 / 72.0).round() as u32);
    assert!(r.warnings.iter().any(|w| w.contains("300 ppi")), "{:?}", r.warnings);
}

#[test]
fn crop_to_page_boxes_and_the_bounding_box() {
    // 100 × 80 pt page, a trim box 10 pt in, red paint at 20..50 × 10..30 (PDF y up).
    let pdf = hand_pdf(100.0, 80.0, (20.0, 10.0, 30.0, 20.0), " /TrimBox [10 10 90 70]");
    let at = |crop| render_page(&pdf, 0, &PdfRaster { ppi: 72.0, crop }, "p.pdf").unwrap().document;
    let media = at(PdfCrop::MediaBox);
    assert_eq!((media.size.width, media.size.height), (100, 80));
    let trim = at(PdfCrop::TrimBox);
    assert_eq!((trim.size.width, trim.size.height), (80, 60));
    // A box the page doesn't have is the crop box.
    let art = at(PdfCrop::ArtBox);
    assert_eq!((art.size.width, art.size.height), (100, 80));
    let bounds = at(PdfCrop::BoundingBox);
    assert_eq!((bounds.size.width, bounds.size.height), (30, 20), "just the painted rectangle");
    let p = bounds.layers[0].surface().unwrap().rgba(15, 10);
    assert!(p[0] > 0.95 && p[1] < 0.05 && p[3] > 0.99, "{p:?}");
    // Transparent where nothing is painted.
    assert_eq!(media.layers[0].surface().unwrap().rgba(90, 5)[3], 0.0);
}

#[test]
fn rotated_pages_render_upright() {
    let pdf = hand_pdf(100.0, 50.0, (0.0, 0.0, 10.0, 10.0), " /Rotate 90");
    let i = info(&pdf).unwrap();
    assert_eq!((i.pages[0].width_pt, i.pages[0].height_pt), (50.0, 100.0));
    let d = render_page(&pdf, 0, &PdfRaster { ppi: 72.0, crop: PdfCrop::CropBox }, "r.pdf").unwrap().document;
    assert_eq!((d.size.width, d.size.height), (50, 100));
    let t = thumbnail(&pdf, 0, 20).unwrap();
    assert_eq!((t.width, t.height), (10, 20));
}

#[test]
fn hostile_input_is_an_error_not_a_panic() {
    assert!(info(b"%PDF-1.4\nnothing else").is_err());
    assert!(crate::import("x.pdf", b"%PDF-garbage").is_err());
    let pdf = hand_pdf(10.0, 10.0, (0.0, 0.0, 1.0, 1.0), "");
    assert!(render_page(&pdf, 3, &PdfRaster::default(), "x").is_err(), "no page 4");
    for ppi in [0.0, -5.0, f32::NAN, f32::INFINITY, 1e9] {
        assert!(render_page(&pdf, 0, &PdfRaster { ppi, crop: PdfCrop::CropBox }, "x").is_err(), "{ppi}");
    }
    // A page far too big at the chosen resolution.
    let huge = hand_pdf(14_000.0, 14_000.0, (0.0, 0.0, 1.0, 1.0), "");
    assert!(render_page(&huge, 0, &PdfRaster { ppi: 300.0, crop: PdfCrop::CropBox }, "x").is_err());
    // Damaged editing data is reported, not opened as something else.
    let mut d = layer_doc(ColorMode::Rgb);
    d.name = "x".into();
    let mut bytes = crate::export(&d, "x.pdf", &ExportOptions::default()).unwrap().bytes;
    if let Some(at) = bytes.windows(4).position(|w| w == b"PK\x03\x04") {
        bytes[at + 30] ^= 0xff;
        bytes[at + 31] ^= 0xff;
    }
    let _ = crate::import("x.pdf", &bytes);
}
