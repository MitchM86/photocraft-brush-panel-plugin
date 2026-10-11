//! File › Open on a PDF: Photoshop's Import PDF dialog. A PDF PhotoCraft saved with its editing
//! data opens straight away as the layered document (no dialog). Any other PDF asks first: the
//! pages (previews; click, ⇧-click and ⌘/Ctrl-click to choose several), Crop To, the resolution,
//! the colour mode and the bit depth. Each chosen page opens as its own document.
//!
//! The dialog is an ordinary dialog in the UI state (`ui.inspect` shows it, `ui.dialog.confirm`
//! answers it); its fields hold the choices, the PDF's bytes wait in
//! [`PhotocraftApp::pdf_imports`] under the dialog's id.
//!
//! A one-page PDF keeps its path, so File › Save writes the PDF again (a PhotoCraft PDF with the
//! layers inside, as Photoshop's Save does); a page of a longer PDF opens untitled, so saving it
//! never replaces the other pages.

use std::sync::Arc;

use photocraft_io::pdf_import::{self, PdfCrop, PdfInfo, PdfRaster};
use serde_json::{Map, Value, json};

use crate::PhotocraftApp;
use crate::state::DialogKind;
use crate::theme::Tokens;

const MARK: &str = "__pdfImport";

/// A PDF waiting in the Import PDF dialog.
pub struct Source {
    pub name: String,
    pub path: Option<String>,
    pub bytes: Arc<Vec<u8>>,
    pub info: PdfInfo,
}

pub fn owns(fields: &Map<String, Value>) -> bool {
    fields.contains_key(MARK)
}

/// Is `name` / `bytes` a PDF Import PDF should ask about (one without PhotoCraft's editing data)?
fn wants_dialog(name: &str, bytes: &[u8]) -> Option<PdfInfo> {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    if !(ext.as_deref() == Some("pdf") || (ext.as_deref() == Some("ai") && pdf_import::is_pdf(bytes))) {
        return None;
    }
    // Unreadable files go the ordinary way, which reports why.
    pdf_import::info(bytes).ok().filter(|i| !i.editable)
}

/// Opens Import PDF for a PDF being opened (`path` when it came from disk). `None` when the file
/// isn't a PDF that needs it (the caller opens it as usual).
pub fn intercept(app: &mut PhotocraftApp, name: &str, path: Option<&str>, bytes: &[u8]) -> Option<Result<(), String>> {
    let info = wants_dialog(name, bytes)?;
    let mut f = Map::new();
    f.insert(MARK.into(), json!(true));
    f.insert("__label".into(), json!("Import PDF"));
    f.insert("pages".into(), json!([0]));
    f.insert("crop".into(), json!(PdfCrop::CropBox.id()));
    f.insert("resolution".into(), json!(300));
    f.insert("mode".into(), json!("rgb"));
    f.insert("depth".into(), json!(8));
    f.insert("pageCount".into(), json!(info.pages.len()));
    let id = app.ui.open_dialog(DialogKind::Command, f);
    if let Some(d) = app.ui.dialog_mut(id) {
        d.fields.insert("__id".into(), json!(id));
    }
    app.pdf_imports.insert(id, Source { name: name.to_string(), path: path.map(str::to_string), bytes: Arc::new(bytes.to_vec()), info });
    Some(Ok(()))
}

/// Forget the PDF of a dialog that closed.
pub fn closed(app: &mut PhotocraftApp, id: u64) {
    app.pdf_imports.remove(&id);
}

fn selected(f: &Map<String, Value>, count: usize) -> Vec<usize> {
    let mut v: Vec<usize> = f
        .get("pages")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_u64).map(|p| p as usize).filter(|&p| p < count).collect())
        .unwrap_or_default();
    v.sort_unstable();
    v.dedup();
    v
}

fn ppi(f: &Map<String, Value>) -> f32 {
    f.get("resolution").and_then(Value::as_f64).map_or(300.0, |v| v as f32)
}

fn crop(f: &Map<String, Value>) -> PdfCrop {
    f.get("crop").and_then(Value::as_str).and_then(PdfCrop::from_id).unwrap_or_default()
}

/// The dialog's id, kept in its fields (the PDF waits under it).
fn dialog_id(f: &Map<String, Value>) -> Option<u64> {
    f.get("__id").and_then(Value::as_u64)
}

pub fn body(app: &mut PhotocraftApp, ui: &mut egui::Ui, f: &mut Map<String, Value>) {
    let t = Tokens::get(ui.ctx());
    let Some(id) = dialog_id(f) else { return };
    let Some(src) = app.pdf_imports.get(&id) else { return };
    let count = src.info.pages.len();
    let mut pages = selected(f, count);
    ui.horizontal_top(|ui| {
        // Left: page previews.
        ui.vertical(|ui| {
            ui.set_width(300.0);
            ui.label(egui::RichText::new(tl!("Pages")).font(crate::theme::semibold(12.0)).color(t.text));
            ui.label(egui::RichText::new(format!("{} {count}", tl!("Pages in file:"))).color(t.text_dim).size(11.5));
            let mut rendered = 0;
            egui::ScrollArea::vertical().id_salt("pdf-import-pages").max_height(360.0).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                    for page in 0..count {
                        let key = egui::Id::new(("pdf-import-thumb", id, page));
                        let mut tex: Option<egui::TextureHandle> = ui.data(|d| d.get_temp(key));
                        // A few previews a frame, so a long PDF doesn't freeze the dialog.
                        if tex.is_none() && rendered < 2 {
                            rendered += 1;
                            let img = pdf_import::thumbnail(&src.bytes, page, 128).ok();
                            if let Some(img) = img {
                                let color = egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.pixels);
                                let h = ui.ctx().load_texture(format!("pdf-page-{id}-{page}"), color, egui::TextureOptions::LINEAR);
                                ui.data_mut(|d| d.insert_temp(key, h.clone()));
                                tex = Some(h);
                            }
                        }
                        let (r, resp) = ui.allocate_exact_size(egui::vec2(132.0, 152.0), egui::Sense::click());
                        let on = pages.contains(&page);
                        let frame = egui::Rect::from_min_size(r.min, egui::vec2(132.0, 132.0));
                        if let Some(tex) = &tex {
                            let s = tex.size_vec2();
                            let k = 128.0 / s.x.max(s.y).max(1.0);
                            let ir = egui::Rect::from_center_size(frame.center(), s * k);
                            ui.painter().image(tex.id(), ir, egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
                        } else {
                            ui.painter().rect_filled(frame.shrink(2.0), 2.0, t.field);
                        }
                        ui.painter().rect_stroke(
                            frame,
                            2.0,
                            egui::Stroke::new(if on { 2.0 } else { 1.0 }, if on { t.accent } else { t.separator }),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().text(
                            egui::pos2(r.center().x, r.bottom() - 9.0),
                            egui::Align2::CENTER_CENTER,
                            format!("{}", page + 1),
                            egui::FontId::proportional(11.5),
                            if on { t.text } else { t.text_dim },
                        );
                        if resp.clicked() {
                            let m = ui.input(|i| i.modifiers);
                            if m.command {
                                if on {
                                    pages.retain(|&p| p != page);
                                } else {
                                    pages.push(page);
                                }
                            } else if m.shift
                                && let Some(&anchor) = pages.first()
                            {
                                pages = (anchor.min(page)..=anchor.max(page)).collect();
                            } else {
                                pages = vec![page];
                            }
                        }
                    }
                });
            });
            if count > 2 {
                ui.ctx().request_repaint();
            }
            if count > 1 && crate::widgets::secondary_button(ui, tl!("Select All"), 90.0).clicked() {
                pages = (0..count).collect();
            }
        });
        ui.add_space(12.0);
        // Right: page options.
        ui.vertical(|ui| {
            ui.set_width(240.0);
            ui.label(egui::RichText::new(tl!("Page Options")).font(crate::theme::semibold(12.0)).color(t.text));
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tl!("Crop To")).color(t.text_dim));
                let mut c = crop(f);
                let opts: Vec<(PdfCrop, &str)> = PdfCrop::ALL.iter().map(|c| (*c, tl!(c.label()))).collect();
                if crate::widgets::dropdown(ui, "pdf-import-crop", &mut c, &opts, 140.0) {
                    f.insert("crop".into(), json!(c.id()));
                }
            });
            ui.add_space(6.0);
            ui.label(egui::RichText::new(tl!("Image Size")).font(crate::theme::semibold(12.0)).color(t.text));
            let mut res = ppi(f);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tl!("Resolution")).color(t.text_dim));
                crate::widgets::value_field(ui, &mut res, 1.0..=2400.0, " ppi", 90.0);
            });
            f.insert("resolution".into(), json!(res.round()));
            if let Some(p) = pages.first().and_then(|&i| src.info.pages.get(i)) {
                let k = f64::from(res.round()) / 72.0;
                ui.label(egui::RichText::new(format!("{} × {} px", (p.width_pt * k).round(), (p.height_pt * k).round())).color(t.text_dim).size(11.5));
            }
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tl!("Mode")).color(t.text_dim));
                let mut m = f.get("mode").and_then(Value::as_str).unwrap_or("rgb").to_string();
                let opts: Vec<(String, &str)> = vec![
                    ("rgb".into(), tl!("RGB Color")),
                    ("cmyk".into(), tl!("CMYK Color")),
                    ("grayscale".into(), tl!("Grayscale")),
                    ("lab".into(), tl!("Lab Color")),
                ];
                if crate::widgets::dropdown(ui, "pdf-import-mode", &mut m, &opts, 140.0) {
                    f.insert("mode".into(), json!(m));
                }
            });
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tl!("Bit Depth")).color(t.text_dim));
                let mut d = f.get("depth").and_then(Value::as_u64).unwrap_or(8);
                let opts: Vec<(u64, &str)> = vec![(8, tl!("8 bit")), (16, tl!("16 bit"))];
                if crate::widgets::dropdown(ui, "pdf-import-depth", &mut d, &opts, 90.0) {
                    f.insert("depth".into(), json!(d));
                }
            });
        });
    });
    f.insert("pages".into(), json!(pages));
}

/// The name page `page` opens under: the file's own for a one-page import, else "name-N.pdf".
fn page_name(name: &str, page: usize, several: bool) -> String {
    if !several {
        return name.to_string();
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}-{}.{ext}", page + 1),
        None => format!("{name}-{}", page + 1),
    }
}

/// OK: render each chosen page and open it, converted to the chosen mode and depth.
pub fn confirm(app: &mut PhotocraftApp, f: &Map<String, Value>) -> Result<Value, String> {
    let id = dialog_id(f).ok_or("no Import PDF dialog")?;
    let src = app.pdf_imports.remove(&id).ok_or("the PDF is no longer open in Import PDF")?;
    let pages = selected(f, src.info.pages.len());
    if pages.is_empty() {
        // OK closed the dialog: bring it back with the PDF, so a page can be chosen.
        let again = app.ui.open_dialog(DialogKind::Command, f.clone());
        if let Some(d) = app.ui.dialog_mut(again) {
            d.fields.insert("__id".into(), json!(again));
        }
        app.pdf_imports.insert(again, src);
        return Err("choose at least one page".into());
    }
    let opts = PdfRaster { ppi: ppi(f), crop: crop(f) };
    let mode = f.get("mode").and_then(Value::as_str).unwrap_or("rgb").to_string();
    let sixteen = f.get("depth").and_then(Value::as_u64) == Some(16);
    // A page of a longer PDF opens untitled: saving it must not replace the other pages.
    let keep_path = src.info.pages.len() == 1;
    let several = pages.len() > 1;
    let mut opened = Vec::new();
    for &page in &pages {
        let name = page_name(&src.name, page, several);
        let r = photocraft_io::pdf_import::render_page(&src.bytes, page, &opts, &name).map_err(|e| format!("page {}: {e}", page + 1))?;
        app.session.open_document(r.document, None);
        if mode != "rgb" {
            app.run(&format!("image.mode.{mode}"), json!({}))?;
        }
        if sixteen {
            app.run("image.mode.bits16", json!({}))?;
        }
        // Opened, not edited: the conversions aren't steps to undo, and nothing is unsaved.
        if let Some(st) = app.session.active_mut() {
            st.history.clear();
            st.saved_revision = st.revision;
        }
        if keep_path && let Some(p) = &src.path {
            app.opened_from(p);
        }
        app.sync_views();
        crate::notices::io_warnings(app, &format!("Opened {name}"), &r.warnings);
        opened.push(name);
    }
    photocraft_engine::automate_cmds::document_opened(&mut app.session);
    app.ui.status = format!("Opened {}", opened.join(", "));
    app.ui.status_error = false;
    Ok(json!({"opened": opened}))
}

#[cfg(test)]
#[path = "pdf_import_ui_tests.rs"]
mod tests;
