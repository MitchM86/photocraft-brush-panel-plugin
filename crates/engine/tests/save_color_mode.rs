//! What colour mode a saved file has: an RGB document converted to CMYK (Image › Mode › CMYK
//! Color) must save as CMYK in every format that holds CMYK, layered or flat, and an RGB document
//! must save as RGB.

use photocraft_color::ColorMode;
use photocraft_engine::Session;
use photocraft_io::ExportOptions;
use serde_json::json;

/// A layered RGB document: a white background and a half-covering, half-opaque red layer.
fn layered_rgb(depth: u32) -> Session {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 24, "height": 16, "depth": depth, "background": "white"})).unwrap();
    s.execute("layer.new.layer", json!({"name": "Red"})).unwrap();
    s.execute("selection.all", json!({})).ok();
    s.execute("edit.fill", json!({"contents": "color", "color": "#e01020"})).unwrap();
    s.execute("layer.opacity", json!({"opacity": 50})).ok();
    s
}

/// The colour mode `ext` reopens as, or for PDF, the image's channel count as a mode.
fn saved_mode(s: &Session, ext: &str, tiff_layers: bool) -> (ColorMode, Vec<String>) {
    let doc = &s.active().unwrap().doc;
    let opts = ExportOptions { tiff_layers, ..Default::default() };
    let r = photocraft_io::export(doc, &format!("x.{ext}"), &opts).unwrap_or_else(|e| panic!("{ext}: {e}"));
    if ext == "pdf" {
        let text = String::from_utf8_lossy(&r.bytes);
        let mode = if text.contains("/N 4 /Alternate /DeviceCMYK") {
            ColorMode::Cmyk
        } else if text.contains("/N 3 /Alternate /DeviceRGB") {
            ColorMode::Rgb
        } else {
            ColorMode::Grayscale
        };
        return (mode, r.warnings);
    }
    let back = photocraft_io::import(&format!("x.{ext}"), &r.bytes).unwrap_or_else(|e| panic!("{ext} reopen: {e}"));
    (back.document.mode, r.warnings)
}

#[test]
fn rgb_converted_to_cmyk_saves_as_cmyk() {
    for depth in [8, 16] {
        let mut s = layered_rgb(depth);
        s.execute("image.mode.cmyk", json!({})).unwrap();
        assert_eq!(s.active().unwrap().doc.mode, ColorMode::Cmyk);
        assert!(s.active().unwrap().doc.layers.len() >= 2, "still layered");
        for (ext, layers) in [("psd", true), ("psb", true), ("pcraft", true), ("tif", true), ("tif", false), ("jpg", false), ("pdf", false)] {
            let (mode, warnings) = saved_mode(&s, ext, layers);
            assert_eq!(mode, ColorMode::Cmyk, "{depth}-bit CMYK saved as {ext} (TIFF layers {layers}): {warnings:?}");
        }
    }
}

#[test]
fn rgb_saves_as_rgb() {
    for depth in [8, 16] {
        let s = layered_rgb(depth);
        for (ext, layers) in [("psd", true), ("pcraft", true), ("tif", true), ("tif", false), ("jpg", false), ("png", false), ("webp", false), ("pdf", false)] {
            let (mode, warnings) = saved_mode(&s, ext, layers);
            assert_eq!(mode, ColorMode::Rgb, "{depth}-bit RGB saved as {ext} (TIFF layers {layers}): {warnings:?}");
        }
    }
}

/// Formats that can't hold CMYK write RGB through the document's profile, and say so.
#[test]
fn cmyk_in_an_rgb_only_format_is_converted_with_a_warning() {
    let mut s = layered_rgb(8);
    s.execute("image.mode.cmyk", json!({})).unwrap();
    for ext in ["png", "webp"] {
        let (mode, warnings) = saved_mode(&s, ext, false);
        assert_eq!(mode, ColorMode::Rgb, "{ext}");
        assert!(warnings.iter().any(|w| w.contains("sRGB")), "{ext}: {warnings:?}");
    }
}
