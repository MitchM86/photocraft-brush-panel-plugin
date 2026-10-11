use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::{AlphaChannel, Document, Layer, LayerContent};
use photocraft_geom::{Rect, Size};
use photocraft_raster::Surface;

use super::*;

fn pattern(mode: ColorMode, depth: SampleType, w: u32, h: u32) -> Document {
    let mut d = Document::new("Scan.psd", Size::new(w, h), mode, depth);
    let fmt = d.pixel_format();
    let n = fmt.channels();
    let max = match depth {
        SampleType::U8 => 255.0,
        _ => 65535.0,
    };
    let mut vals = Vec::new();
    for y in 0..h {
        for x in 0..w {
            for c in 0..n - 1 {
                vals.push(((x * 7919 + y * 104_729 + c as u32 * 31) % 65536) as f32 / 65535.0 * max / max);
            }
            vals.push(1.0);
        }
    }
    // Round to the depth so the file can hold every value exactly.
    let vals: Vec<f32> = vals.into_iter().map(|v| (v * max).round() / max).collect();
    let mut s = Surface::new(fmt);
    s.write_region(Rect::new(0, 0, w as i32, h as i32), &vals);
    let mut bg = Layer::new("Background", LayerContent::Raster(s));
    bg.locks.transparency = true;
    d.layers.push(bg);
    d
}

fn save(doc: &Document, o: RawSaveOptions) -> ExportResult {
    let opts = ExportOptions { photoshop_raw: o, ..Default::default() };
    crate::export(doc, "out.raw", &opts).unwrap_or_else(|e| panic!("export: {e}"))
}

fn layer_pixels(d: &Document) -> Vec<f32> {
    d.layers[0].surface().map(|s| s.read_region(d.bounds())).unwrap_or_default()
}

#[test]
fn every_mode_depth_and_order_round_trips() {
    for (mode, channels) in [(ColorMode::Grayscale, 1u16), (ColorMode::Rgb, 3), (ColorMode::Cmyk, 4)] {
        for (depth, bits) in [(SampleType::U8, 8u8), (SampleType::U16, 16), (SampleType::F32, 32)] {
            for interleaved in [true, false] {
                for little_endian in [true, false] {
                    let d = pattern(mode, depth, 13, 7);
                    let o = RawSaveOptions { header: 5, interleaved, little_endian };
                    let r = save(&d, o);
                    let bytes_per = usize::from(bits / 8);
                    assert_eq!(r.bytes.len(), 5 + 13 * 7 * usize::from(channels) * bytes_per);
                    assert!(r.bytes[..5].iter().all(|&b| b == 0), "a header of zeros");
                    let layout = RawLayout { width: 13, height: 7, channels, depth: bits, interleaved, little_endian, header: 5 };
                    let back = import("x.raw", &r.bytes, &layout).unwrap().document;
                    assert_eq!((back.mode, back.depth, back.size), (mode, depth, d.size));
                    assert_eq!(layer_pixels(&back), layer_pixels(&d), "{mode:?} {depth:?} interleaved {interleaved} le {little_endian}");
                }
            }
        }
    }
}

#[test]
fn byte_layout_is_photoshops() {
    let mut d = Document::new("x", Size::new(2, 1), ColorMode::Rgb, SampleType::U16);
    let mut s = Surface::new(d.pixel_format());
    s.write_region(Rect::new(0, 0, 2, 1), &[1.0, 0.0, 256.0 / 65535.0, 1.0, 0.5, 1.0 / 65535.0, 0.0, 1.0]);
    d.layers.push(Layer::new("Background", LayerContent::Raster(s)));
    let le = save(&d, RawSaveOptions { header: 0, interleaved: true, little_endian: true }).bytes;
    assert_eq!(le, [0xFF, 0xFF, 0, 0, 0, 1, 0, 0x80, 1, 0, 0, 0]);
    let planar_be = save(&d, RawSaveOptions { header: 0, interleaved: false, little_endian: false }).bytes;
    // R of both pixels, then G, then B.
    assert_eq!(planar_be, [0xFF, 0xFF, 0x80, 0, 0, 0, 0, 1, 1, 0, 0, 0]);
}

#[test]
fn alpha_channels_follow_the_colour_channels() {
    let mut d = pattern(ColorMode::Grayscale, SampleType::U8, 4, 3);
    let mut a = Surface::new(PixelFormat::new(ColorMode::Grayscale, SampleType::U8, false));
    a.write_region(Rect::new(0, 0, 4, 3), &[0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0]);
    d.channels.push(AlphaChannel::new("Alpha 1", a));
    let r = save(&d, RawSaveOptions::default());
    assert_eq!(r.bytes.len(), 4 * 3 * 2);
    // Two channels open as Grayscale with an alpha channel.
    let back = import("x.raw", &r.bytes, &RawLayout { width: 4, height: 3, channels: 2, ..RawLayout::default() }).unwrap().document;
    assert_eq!(back.mode, ColorMode::Grayscale);
    assert_eq!(back.channels.len(), 1);
    assert_eq!(back.channels[0].surface.read_region(back.bounds()), d.channels[0].surface.read_region(d.bounds()));
    assert_eq!(layer_pixels(&back), layer_pixels(&d));
}

#[test]
fn lab_keeps_its_values() {
    let d = pattern(ColorMode::Lab, SampleType::U8, 9, 4);
    let r = save(&d, RawSaveOptions::default());
    assert_eq!(r.bytes.len(), 9 * 4 * 3);
    let expected: Vec<u8> = layer_pixels(&d).chunks(4).flat_map(|p| p[..3].to_vec()).map(|v| (v * 255.0).round() as u8).collect();
    assert_eq!(r.bytes, expected);
}

#[test]
fn transparency_is_flattened_over_white_with_a_warning() {
    let mut d = Document::new("t", Size::new(2, 2), ColorMode::Rgb, SampleType::U8);
    let mut s = Surface::new(d.pixel_format());
    s.write_region(Rect::new(0, 0, 2, 2), &[0.0, 0.0, 0.0, 0.0].repeat(4));
    d.layers.push(Layer::new("Layer 0", LayerContent::Raster(s)));
    let r = save(&d, RawSaveOptions::default());
    assert!(r.warnings.iter().any(|w| w.contains("over white")), "{:?}", r.warnings);
    assert!(r.bytes.iter().all(|&b| b == 255));
}

#[test]
fn short_and_long_files_open_with_warnings() {
    let layout = RawLayout { width: 4, height: 4, channels: 3, ..RawLayout::default() };
    let short = import("x.raw", &[200u8; 20], &layout).unwrap();
    assert!(short.warnings[0].contains("shorter"), "{:?}", short.warnings);
    let long = import("x.raw", &[200u8; 60], &layout).unwrap();
    assert!(long.warnings[0].contains("ignored"));
    assert!(import("x.raw", &[0u8; 4], &RawLayout { width: 0, ..layout }).is_err());
    assert!(import("x.raw", &[0u8; 4], &RawLayout { channels: 57, ..layout }).is_err());
    assert!(import("x.raw", &[0u8; 4], &RawLayout { depth: 12, ..layout }).is_err());
    assert!(import("x.raw", &[0u8; 4], &RawLayout { width: u32::MAX, height: u32::MAX, ..layout }).is_err());
}

#[test]
fn guess_finds_near_square_sizes() {
    let hint = RawLayout::default();
    let g = guess(640 * 480 * 3, &hint).unwrap();
    assert_eq!((g.width * g.height, g.channels, g.depth), (640 * 480, 3, 8));
    assert!(g.width >= g.height && g.width - g.height < 300, "{}×{}", g.width, g.height);
    // A known width wins.
    let g = guess(640 * 480 * 3, &RawLayout { width: 640, ..hint }).unwrap();
    assert_eq!((g.width, g.height), (640, 480));
    // 7 bytes can't be RGB: one gray row of seven.
    let g = guess(7, &hint).unwrap();
    assert_eq!((g.width, g.height, g.channels), (7, 1, 1));
    // The header is left out of the pixels.
    let g = guess(100 + 10 * 10 * 4, &RawLayout { channels: 4, header: 100, ..hint }).unwrap();
    assert_eq!((g.width, g.height, g.channels), (10, 10, 4));
    assert_eq!(guess(10, &RawLayout { header: 20, ..hint }), None);
}

#[test]
fn camera_raw_extension_without_layout_explains() {
    let err = crate::import("scan.raw", &[1u8; 300]).err().map(|e| e.to_string()).unwrap_or_default();
    assert!(err.contains("Photoshop Raw"), "{err}");
}
