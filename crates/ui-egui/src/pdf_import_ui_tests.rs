//! Import PDF: plain PDFs ask (pages, resolution, mode, depth); PhotoCraft PDFs open with their
//! layers, and Save writes the PDF in place.

use std::{cell::RefCell, rc::Rc};

use photocraft_color::{ColorMode, SampleType};
use photocraft_engine::Session;
use serde_json::json;

use crate::{PhotocraftApp, Services, menus};

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
                let opts = photocraft_io::ExportOptions { pdf_editable: s.pdf_editable, ..Default::default() };
                photocraft_io::export(doc, path, &opts).map(|r| (r.bytes, r.warnings)).map_err(|e| e.to_string())
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

/// A plain (not PhotoCraft) PDF of `pages` pages, each 72 × 36 pt.
fn plain_pdf(pages: usize) -> Vec<u8> {
    let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string()];
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {pages} >>", kids.join(" ")));
    for i in 0..pages {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 72 36] /Contents {} 0 R >>", 4 + 2 * i));
        let content = format!("0 0 1 rg 0 0 {} 36 re f\n", 10 * (i + 1));
        objs.push(format!("<< /Length {} >>\nstream\n{content}endstream", content.len()));
    }
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

fn pdf_dialog(app: &PhotocraftApp) -> u64 {
    app.ui.dialogs.iter().find(|d| super::owns(&d.fields)).map(|d| d.id).expect("Import PDF is open")
}

#[test]
fn a_plain_pdf_asks_then_opens_the_chosen_pages() {
    let (mut app, _) = app();
    app.open_file("/docs/Brochure.pdf", &plain_pdf(3)).unwrap();
    assert!(app.session.documents().is_empty(), "nothing opens before Import PDF is answered");
    let id = pdf_dialog(&app);
    let d = app.ui.dialog_mut(id).unwrap();
    d.fields.insert("pages".into(), json!([0, 2]));
    d.fields.insert("resolution".into(), json!(144));
    d.fields.insert("mode".into(), json!("cmyk"));
    d.fields.insert("depth".into(), json!(16));
    crate::dialogs::confirm(&mut app, id).unwrap();
    assert!(app.pdf_imports.is_empty());
    let docs = app.session.documents();
    let names: Vec<&str> = docs.iter().map(|d| d.doc.name.as_str()).collect();
    assert_eq!(names, ["Brochure-1.pdf", "Brochure-3.pdf"]);
    for st in docs {
        assert_eq!((st.doc.size.width, st.doc.size.height), (144, 72));
        assert_eq!(st.doc.mode, ColorMode::Cmyk);
        assert_eq!(st.doc.depth, SampleType::U16);
        assert_eq!(st.doc.resolution_dpi, 144.0);
        assert!(st.path.is_none(), "a page of a longer PDF must not save over the whole PDF");
        assert_eq!(st.revision, st.saved_revision, "opened, not edited");
    }
}

#[test]
fn ok_without_pages_keeps_the_dialog() {
    let (mut app, _) = app();
    app.open_bytes("two.pdf", &plain_pdf(2)).unwrap();
    let id = pdf_dialog(&app);
    app.ui.dialog_mut(id).unwrap().fields.insert("pages".into(), json!([]));
    assert!(crate::dialogs::confirm(&mut app, id).is_err());
    let again = pdf_dialog(&app);
    assert_eq!(app.pdf_imports.keys().copied().collect::<Vec<_>>(), [again]);
    crate::dialogs::cancel(&mut app, again).unwrap();
    assert!(app.pdf_imports.is_empty());
}

#[test]
fn cancel_forgets_the_pdf() {
    let (mut app, _) = app();
    app.open_bytes("one.pdf", &plain_pdf(1)).unwrap();
    let id = pdf_dialog(&app);
    assert_eq!(app.pdf_imports.len(), 1);
    crate::dialogs::cancel(&mut app, id).unwrap();
    assert!(app.pdf_imports.is_empty());
    assert!(app.session.documents().is_empty());
}

#[test]
fn a_one_page_pdf_saves_back_to_itself_with_its_layers() {
    let (mut app, written) = app();
    app.open_file("/docs/Flyer.pdf", &plain_pdf(1)).unwrap();
    let id = pdf_dialog(&app);
    crate::dialogs::confirm(&mut app, id).unwrap();
    assert_eq!(app.session.active().unwrap().path.as_deref(), Some("/docs/Flyer.pdf"));
    app.run("layer.new.layer", json!({"name": "Notes"})).unwrap();
    let ctx = egui::Context::default();
    menus::invoke(&mut app, &ctx, "file.save", json!({})).unwrap();
    let (path, bytes) = written.borrow().last().cloned().expect("saved");
    assert_eq!(path, "/docs/Flyer.pdf", "Save writes the PDF itself, not a copy");
    let st = app.session.active().unwrap();
    assert_eq!(st.revision, st.saved_revision, "the document is saved");
    // Reopening it skips Import PDF and brings the layers back.
    app.open_file("/docs/Flyer.pdf", &bytes).unwrap();
    assert!(app.ui.dialogs.iter().all(|d| !super::owns(&d.fields)));
    let names: Vec<String> = app.session.active().unwrap().doc.layers.iter().map(|l| l.name.clone()).collect();
    assert_eq!(names, ["Layer 1", "Notes"]);
}

#[test]
fn export_as_pdf_is_plain_unless_asked() {
    let mut f = serde_json::Map::new();
    f.insert("format".into(), json!("pdf"));
    assert!(!crate::export_dialog::settings(&f).pdf_editable);
    f.insert("pdfEditable".into(), json!(true));
    assert!(crate::export_dialog::settings(&f).pdf_editable);
    f.insert("pdfCompression".into(), json!("jpeg"));
    f.insert("quality".into(), json!(70));
    assert_eq!(crate::export_dialog::settings(&f).pdf_jpeg_quality, Some(70));
}
