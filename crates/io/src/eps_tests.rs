use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::{Document, Layer, LayerContent};
use photocraft_geom::{Rect, Size};
use photocraft_raster::Surface;

use super::*;

/// A one-layer document of `mode` with a different value in every pixel and channel.
fn pattern(mode: ColorMode, depth: SampleType, w: u32, h: u32) -> Document {
    let mut d = Document::new("Poster.psd", Size::new(w, h), mode, depth);
    let fmt = d.pixel_format();
    let n = fmt.channels();
    let mut vals = Vec::with_capacity((w * h) as usize * n);
    for y in 0..h {
        for x in 0..w {
            for c in 0..n - 1 {
                let v = ((x * 37 + y * 11 + c as u32 * 71) % 256) as f32 / 255.0;
                // Bitmap pixels are black or white.
                vals.push(if mode == ColorMode::Bitmap { (v >= 0.5) as u8 as f32 } else { v });
            }
            vals.push(1.0);
        }
    }
    let mut s = Surface::new(fmt);
    s.write_region(Rect::new(0, 0, w as i32, h as i32), &vals);
    let mut bg = Layer::new("Background", LayerContent::Raster(s));
    bg.locks.transparency = true;
    d.layers.push(bg);
    d
}

/// A smooth gradient (what JPEG is for), one channel per direction.
fn gradient(mode: ColorMode, w: u32, h: u32) -> Document {
    let mut d = pattern(mode, SampleType::U8, w, h);
    let n = d.pixel_format().channels();
    let mut vals = Vec::with_capacity((w * h) as usize * n);
    for y in 0..h {
        for x in 0..w {
            for c in 0..n - 1 {
                vals.push([x as f32 / w as f32, y as f32 / h as f32, 0.5, 0.2][c % 4]);
            }
            vals.push(1.0);
        }
    }
    if let Some(LayerContent::Raster(s)) = d.layers.first_mut().map(|l| &mut l.content) {
        s.write_region(Rect::new(0, 0, w as i32, h as i32), &vals);
    }
    d
}

fn eps(doc: &Document, eps: EpsOptions) -> ExportResult {
    let opts = ExportOptions { eps, ..Default::default() };
    crate::export(doc, "out.eps", &opts).unwrap_or_else(|e| panic!("export: {e}"))
}

fn open(bytes: &[u8]) -> ImportResult {
    crate::import("out.eps", bytes).unwrap_or_else(|e| panic!("import: {e}"))
}

fn pixels(d: &Document) -> Vec<f32> {
    d.layers[0].surface().map(|s| s.read_region(d.bounds())).unwrap_or_default()
}

fn max_diff(a: &Document, b: &Document) -> f32 {
    let (pa, pb) = (pixels(a), pixels(b));
    assert_eq!(pa.len(), pb.len());
    pa.iter().zip(&pb).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}

fn mean_diff(a: &Document, b: &Document) -> f32 {
    let (pa, pb) = (pixels(a), pixels(b));
    pa.iter().zip(&pb).map(|(x, y)| (x - y).abs()).sum::<f32>() / pa.len() as f32
}

const PLAIN: EpsOptions = EpsOptions { encoding: EpsEncoding::Binary, preview: EpsPreview::None, interpolate: false };

#[test]
fn every_mode_and_lossless_encoding_round_trips_exactly() {
    for mode in [ColorMode::Grayscale, ColorMode::Rgb, ColorMode::Cmyk, ColorMode::Lab, ColorMode::Bitmap] {
        let d = pattern(mode, SampleType::U8, 23, 9);
        for encoding in [EpsEncoding::Binary, EpsEncoding::Ascii, EpsEncoding::Ascii85] {
            for preview in [EpsPreview::None, EpsPreview::Tiff] {
                let r = eps(&d, EpsOptions { encoding, preview, interpolate: false });
                let back = open(&r.bytes);
                assert!(!back.preview_only);
                let back = back.document;
                assert_eq!((back.mode, back.size, back.depth), (mode, d.size, SampleType::U8), "{mode:?} {encoding:?}");
                assert!(max_diff(&d, &back) <= 1e-6, "{mode:?} {encoding:?} {preview:?}: {}", max_diff(&d, &back));
            }
        }
    }
}

#[test]
fn jpeg_encodings_are_close_and_get_smaller() {
    let d = gradient(ColorMode::Rgb, 64, 48);
    let mut last = usize::MAX;
    for q in [EpsJpegQuality::Maximum, EpsJpegQuality::High, EpsJpegQuality::Medium, EpsJpegQuality::Low] {
        let r = eps(&d, EpsOptions { encoding: EpsEncoding::Jpeg(q), ..PLAIN });
        assert!(r.bytes.len() < last, "{q:?}");
        last = r.bytes.len();
        let back = open(&r.bytes).document;
        assert_eq!(back.mode, ColorMode::Rgb);
        assert!(mean_diff(&d, &back) < 0.02, "{q:?}: {}", mean_diff(&d, &back));
    }
    // CMYK JPEG is Adobe-inverted, read back with the Decode array the program carries.
    let c = gradient(ColorMode::Cmyk, 32, 32);
    let r = eps(&c, EpsOptions { encoding: EpsEncoding::Jpeg(EpsJpegQuality::Maximum), ..PLAIN });
    assert!(String::from_utf8_lossy(&r.bytes).contains("/Decode [1 0 1 0 1 0 1 0]"));
    let back = open(&r.bytes).document;
    assert_eq!(back.mode, ColorMode::Cmyk);
    assert!(mean_diff(&c, &back) < 0.02, "mean ink error {}", mean_diff(&c, &back));
}

#[test]
fn header_names_the_image_for_photoshop() {
    let mut d = pattern(ColorMode::Cmyk, SampleType::U8, 300, 150);
    d.resolution_dpi = 300.0;
    let text = String::from_utf8_lossy(&eps(&d, PLAIN).bytes).into_owned();
    assert!(text.starts_with("%!PS-Adobe-3.0 EPSF-3.0\n"));
    assert!(text.contains("%%BoundingBox: 0 0 72 36\n"), "{text:.400}");
    assert!(text.contains("%%HiResBoundingBox: 0 0 72.0000 36.0000\n"));
    assert!(text.contains("%%Title: (Poster.psd)\n"));
    assert!(text.contains("%%DocumentProcessColors: Cyan Magenta Yellow Black\n"));
    // Right after the comments, as Photoshop requires.
    assert!(text.contains("%%EndComments\n%ImageData: 300 150 8 4 0 1 1 \"beginimage\"\n"));
    assert!(text.contains("%%BeginBinary: "));
    assert!(text.trim_end().ends_with("%%EOF"));
    let ascii = String::from_utf8_lossy(&eps(&d, EpsOptions { encoding: EpsEncoding::Ascii, ..PLAIN }).bytes).into_owned();
    assert!(ascii.contains("\"beginimage\"") && ascii.contains("%%DocumentData: Clean7Bit") && !ascii.contains("%%BeginBinary"));
    assert!(ascii.is_ascii());
    let a85 = eps(&d, EpsOptions { encoding: EpsEncoding::Ascii85, ..PLAIN }).bytes;
    assert!(a85.is_ascii() && String::from_utf8_lossy(&a85).contains("0 1 7 \"beginimage\""));
}

#[test]
fn resolution_and_profile_survive() {
    let mut d = pattern(ColorMode::Rgb, SampleType::U8, 40, 20);
    d.resolution_dpi = 240.0;
    let icc = photocraft_cms::Builtin::AdobeRgbCompat.profile().to_bytes();
    d.icc_profile = Some(icc.clone());
    let back = open(&eps(&d, PLAIN).bytes).document;
    assert!((back.resolution_dpi - 240.0).abs() < 0.01, "{}", back.resolution_dpi);
    assert_eq!(back.icc_profile.as_deref().map(|p| p.as_slice()), Some(icc.as_slice()));
}

#[test]
fn dos_eps_preview_is_a_tiff_beside_the_program() {
    let mut d = pattern(ColorMode::Rgb, SampleType::U8, 600, 300);
    d.resolution_dpi = 300.0;
    let b = eps(&d, EpsOptions { preview: EpsPreview::Tiff, ..PLAIN }).bytes;
    assert!(b.starts_with(&DOS_EPS));
    let (ps, tiff) = sections(&b).unwrap();
    assert!(ps.starts_with(b"%!PS-Adobe-3.0 EPSF-3.0"));
    let preview = photocraft_codecs::decode(tiff.unwrap()).unwrap();
    // 72 ppi: 600 px at 300 ppi is 144 points.
    assert_eq!(preview.dimensions(), (144, 72));
}

#[test]
fn deep_and_transparent_documents_are_reduced_with_warnings() {
    let d = pattern(ColorMode::Rgb, SampleType::U16, 8, 8);
    let r = eps(&d, PLAIN);
    assert!(r.warnings.iter().any(|w| w.contains("8 bits")), "{:?}", r.warnings);
    assert_eq!(open(&r.bytes).document.depth, SampleType::U8);

    let mut t = Document::new("t", Size::new(4, 4), ColorMode::Rgb, SampleType::U8);
    let mut s = Surface::new(t.pixel_format());
    s.write_region(Rect::new(0, 0, 4, 4), &[0.0, 0.0, 0.0, 0.5].repeat(16));
    t.layers.push(Layer::new("Layer 0", LayerContent::Raster(s)));
    let r = eps(&t, PLAIN);
    assert!(r.warnings.iter().any(|w| w.contains("flattened over white")), "{:?}", r.warnings);
    let px = pixels(&open(&r.bytes).document);
    assert!((px[0] - 128.0 / 255.0).abs() < 1.5 / 255.0, "half black over white: {}", px[0]);
}

#[test]
fn bitmap_jpeg_falls_back_to_binary() {
    let d = pattern(ColorMode::Bitmap, SampleType::U8, 17, 5);
    let r = eps(&d, EpsOptions { encoding: EpsEncoding::Jpeg(EpsJpegQuality::High), ..PLAIN });
    assert!(r.warnings.iter().any(|w| w.contains("binary")));
    assert!(String::from_utf8_lossy(&r.bytes).contains("%ImageData: 17 5 1 1 0 3 1 \"beginimage\""));
    assert_eq!(max_diff(&d, &open(&r.bytes).document), 0.0);
}

#[test]
fn ascii85_round_trips_every_length() {
    for len in 0..40usize {
        let data: Vec<u8> = (0..len).map(|i| if i % 9 < 4 { 0 } else { (i * 97 % 256) as u8 }).collect();
        let text = ascii85(&data);
        assert!(text.split(|&b| b == b'\n').all(|l| l.len() <= 75));
        assert_eq!(unascii85(&text, len), data, "{len}");
    }
    // The classic sample from the PostScript manual's encoding rules.
    assert_eq!(unascii85(b"9jqo^~>", 4), b"Man ");
}

/// A Photoshop-style file: CRLF lines, hex data, one pad (gray) channel, line-interleaved.
#[test]
fn photoshop_line_interleaved_hex_with_pad_channel() {
    let (w, h) = (3u32, 2u32);
    let mut ps = String::from("%!PS-Adobe-3.0 EPSF-3.0\r\n%%BoundingBox: 0 0 3 2\r\n%%EndComments\r\n");
    ps.push_str(&format!("%ImageData: {w} {h} 8 3 1 {w} 2 \"beginimage\"\r\n"));
    ps.push_str("/beginimage { } def\r\nbeginimage\r\n");
    // Per row: the pad (gray) row, then the R, G and B rows.
    let rows = [[[9u8, 9, 9], [10, 20, 30], [40, 50, 60], [70, 80, 90]], [[9, 9, 9], [11, 21, 31], [41, 51, 61], [71, 81, 91]]];
    for row in &rows {
        for ch in row {
            ps.push_str(&hex(ch));
            ps.push_str("\r\n");
        }
    }
    ps.push_str(">\r\n%%EOF\r\n");
    let d = open(ps.as_bytes()).document;
    assert_eq!((d.mode, d.size, d.resolution_dpi), (ColorMode::Rgb, Size::new(w, h), 72.0));
    let s = d.layers[0].surface().unwrap();
    let px = |x, y| s.pixel(x, y).iter().take(3).map(|v| (v * 255.0).round() as u8).collect::<Vec<_>>();
    assert_eq!(px(0, 0), [10, 40, 70]);
    assert_eq!(px(2, 0), [30, 60, 90]);
    assert_eq!(px(1, 1), [21, 51, 81]);
}

#[test]
fn generic_eps_opens_its_preview_or_explains() {
    let vector = b"%!PS-Adobe-3.0 EPSF-3.0\n%%BoundingBox: 0 0 10 10\n%%EndComments\nnewpath 0 0 moveto 10 10 lineto stroke\n%%EOF\n";
    assert!(is_eps(vector));
    let err = crate::import("art.eps", vector).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(err.contains("PostScript"), "{err}");

    let tiff = preview_tiff(&pattern(ColorMode::Rgb, SampleType::U8, 20, 10)).unwrap();
    let r = crate::import("art.eps", &dos_eps(vector, &tiff)).unwrap();
    assert!(r.preview_only);
    assert!(r.warnings[0].contains("preview"));
    assert_eq!(r.document.size, Size::new(20, 10));
}

#[test]
fn truncated_files_do_not_panic() {
    let full = eps(&pattern(ColorMode::Rgb, SampleType::U8, 16, 16), EpsOptions { preview: EpsPreview::Tiff, ..PLAIN }).bytes;
    for cut in [0, 3, 20, 40, 200, full.len() / 2, full.len() - 10] {
        let _ = crate::import("x.eps", &full[..cut]);
    }
}
