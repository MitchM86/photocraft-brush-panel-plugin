//! Photoshop Raw Options on open, and the Export As settings of the formats added with it
//! (Photoshop EPS, JPEG 2000, Photoshop Raw).

use std::{cell::RefCell, rc::Rc};

use photocraft_color::{ColorMode, SampleType};
use photocraft_engine::Session;
use serde_json::json;

use crate::{PhotocraftApp, Services};

type Written = Rc<RefCell<Vec<(String, Vec<u8>)>>>;

fn app() -> (PhotocraftApp, Written) {
    let written: Written = Rc::default();
    let w = written.clone();
    let app = PhotocraftApp::new(
        Session::new(),
        Services {
            import: Some(Box::new(|name, bytes, depth| {
                photocraft_io::import_with_svg_group_depth(name, bytes, depth).map(|r| (r.document, r.warnings)).map_err(|e| e.to_string())
            })),
            export: Some(Box::new(|doc, path, s: &crate::ExportSettings| {
                photocraft_io::export(doc, path, &s.export_options()).map(|r| (r.bytes, r.warnings)).map_err(|e| e.to_string())
            })),
            write: Some(Box::new(move |path, bytes| {
                w.borrow_mut().push((path.to_string(), bytes.to_vec()));
                Ok(())
            })),
            ..Default::default()
        },
    );
    (app, written)
}

fn raw_dialog(app: &PhotocraftApp) -> u64 {
    app.ui.dialogs.iter().find(|d| super::owns(&d.fields)).map(|d| d.id).expect("Photoshop Raw Options is open")
}

#[test]
fn a_photoshop_raw_file_asks_with_a_guess_then_opens() {
    let (mut app, _) = app();
    // 40 × 30 RGB, 8 bits: the guess is the nearest-square split of 1200 pixels.
    let bytes: Vec<u8> = (0..40 * 30 * 3).map(|i| (i % 251) as u8).collect();
    app.open_file("/scans/plate.raw", &bytes).unwrap();
    assert!(app.session.documents().is_empty(), "nothing opens before the dialog is answered");
    let id = raw_dialog(&app);
    let l = super::layout(&app.ui.dialog_mut(id).unwrap().fields);
    assert_eq!((l.width * l.height, l.channels, l.depth), (1200, 3, 8));
    let d = app.ui.dialog_mut(id).unwrap();
    d.fields.insert("width".into(), json!(40));
    d.fields.insert("height".into(), json!(30));
    crate::dialogs::confirm(&mut app, id).unwrap();
    assert!(app.raw_opens.is_empty());
    let st = app.session.active().unwrap();
    assert_eq!((st.doc.size.width, st.doc.size.height, st.doc.mode), (40, 30, ColorMode::Rgb));
    assert_eq!(st.path.as_deref(), Some("/scans/plate.raw"));
    let px = st.doc.layers[0].surface().unwrap().pixel(1, 0);
    assert_eq!([px[0], px[1], px[2]].map(|v| (v * 255.0).round() as u8), [3, 4, 5]);
}

#[test]
fn a_wrong_size_keeps_the_dialog_and_cancel_forgets_the_file() {
    let (mut app, _) = app();
    app.open_bytes("odd.raw", &[7u8; 10]).unwrap();
    let id = raw_dialog(&app);
    app.ui.dialog_mut(id).unwrap().fields.insert("width".into(), json!(0));
    assert!(crate::dialogs::confirm(&mut app, id).is_err());
    // The dialog comes back with the file and the entries, for a corrected size.
    let id = raw_dialog(&app);
    assert_eq!(app.raw_opens.keys().copied().collect::<Vec<_>>(), [id]);
    assert_eq!(app.ui.dialog_mut(id).unwrap().fields["width"], json!(0));
    crate::dialogs::cancel(&mut app, id).unwrap();
    assert!(app.raw_opens.is_empty());
    assert!(app.session.documents().is_empty());
}

#[test]
fn other_files_named_raw_open_as_usual() {
    // A PNG renamed .raw is recognised by its contents.
    let img = photocraft_codecs::Image::from_u8(2, 2, photocraft_codecs::ChannelLayout::Rgb, vec![9; 12]).unwrap();
    let png = photocraft_codecs::encode(&img, photocraft_codecs::Format::Png, &Default::default()).unwrap();
    assert!(!super::is_photoshop_raw("x.raw", &png));
    assert!(super::is_photoshop_raw("x.RAW", &[0u8; 12]));
    assert!(!super::is_photoshop_raw("x.bin", &[0u8; 12]));
}

#[test]
fn export_as_settings_for_the_new_formats() {
    let mut f = serde_json::Map::new();
    f.insert("format".into(), json!("jp2"));
    f.insert("lossless".into(), json!(true));
    assert_eq!(crate::export_dialog::settings(&f).jpeg2000_quality, None);
    f.insert("lossless".into(), json!(false));
    f.insert("quality".into(), json!(30));
    assert_eq!(crate::export_dialog::settings(&f).jpeg2000_quality, Some(30));

    let mut f = serde_json::Map::new();
    f.insert("format".into(), json!("eps"));
    f.insert("epsEncoding".into(), json!("jpegHigh"));
    f.insert("epsPreview".into(), json!("none"));
    f.insert("epsInterpolate".into(), json!(true));
    let e = crate::export_dialog::settings(&f).eps;
    assert_eq!(e.encoding, photocraft_io::EpsEncoding::Jpeg(photocraft_io::EpsJpegQuality::High));
    assert_eq!(e.preview, photocraft_io::EpsPreview::None);
    assert!(e.interpolate);

    let mut f = serde_json::Map::new();
    f.insert("format".into(), json!("raw"));
    f.insert("rawHeader".into(), json!(128));
    f.insert("rawInterleaved".into(), json!(false));
    f.insert("rawByteOrder".into(), json!("mac"));
    let r = crate::export_dialog::settings(&f).photoshop_raw;
    assert_eq!((r.header, r.interleaved, r.little_endian), (128, false, false));
}

#[test]
fn exported_raw_reopens_through_the_dialog() {
    let (mut app, written) = app();
    let doc = photocraft_doc::Document::with_background("Art", photocraft_geom::Size::new(6, 4), ColorMode::Grayscale, SampleType::U16, photocraft_doc::Color::WHITE);
    app.session.open_document(doc, None);
    let mut f = serde_json::Map::new();
    f.insert("format".into(), json!("raw"));
    f.insert("rawHeader".into(), json!(16));
    f.insert("rawByteOrder".into(), json!("mac"));
    let settings = crate::export_dialog::settings(&f);
    let export = app.services.export.as_ref().unwrap();
    let (bytes, _) = export(&app.session.active().unwrap().doc, "/out/Art.raw", &settings).unwrap();
    assert_eq!(bytes.len(), 16 + 6 * 4 * 2);
    assert!(written.borrow().is_empty());
    app.open_file("/out/Art.raw", &bytes).unwrap();
    let id = raw_dialog(&app);
    let d = app.ui.dialog_mut(id).unwrap();
    for (k, v) in [("width", json!(6)), ("height", json!(4)), ("channels", json!(1)), ("depth", json!(16)), ("byteOrder", json!("mac")), ("header", json!(16))] {
        d.fields.insert(k.into(), v);
    }
    crate::dialogs::confirm(&mut app, id).unwrap();
    let st = app.session.active().unwrap();
    assert_eq!((st.doc.mode, st.doc.depth), (ColorMode::Grayscale, SampleType::U16));
    assert!(st.doc.layers[0].surface().unwrap().read_region(st.doc.bounds()).chunks(2).all(|p| p[0] == 1.0));
}
