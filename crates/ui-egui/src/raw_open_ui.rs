//! File › Open on a Photoshop Raw file (`.raw` that isn't a camera raw): Photoshop's Raw Options
//! dialog. The file holds only samples, so the dialog asks for its width and height (with Swap),
//! its channel count and order, its depth and byte order, and the header to skip; Guess proposes
//! a near-square size from the file's length, and the dialog opens with that guess.
//!
//! The dialog is an ordinary dialog in the UI state (`ui.inspect` shows it, `ui.dialog.confirm`
//! answers it); its fields hold the choices, the file's bytes wait in
//! [`PhotocraftApp::raw_opens`] under the dialog's id.

use std::sync::Arc;

use photocraft_io::photoshop_raw::{self, MAX_CHANNELS, RawLayout};
use serde_json::{Map, Value, json};

use crate::PhotocraftApp;
use crate::state::DialogKind;
use crate::theme::Tokens;

const MARK: &str = "__photoshopRaw";

/// A Photoshop Raw file waiting in the dialog.
pub struct Source {
    pub name: String,
    pub path: Option<String>,
    pub bytes: Arc<Vec<u8>>,
}

pub fn owns(fields: &Map<String, Value>) -> bool {
    fields.contains_key(MARK)
}

/// Whether `name` / `bytes` is a Photoshop Raw file: a `.raw` name that no decoder recognises
/// (camera raws such as Panasonic's and Leica's `.raw` open as camera raws).
pub fn is_photoshop_raw(name: &str, bytes: &[u8]) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    ext.as_deref() == Some("raw") && !photocraft_io::raw::is_raw(bytes) && photocraft_codecs::detect(bytes).is_none()
}

/// The layout the dialog's fields describe.
pub(crate) fn layout(f: &Map<String, Value>) -> RawLayout {
    let u = |k: &str, d: u64| f.get(k).and_then(Value::as_u64).unwrap_or(d);
    let d = RawLayout::default();
    RawLayout {
        width: u("width", 0).min(u64::from(u32::MAX)) as u32,
        height: u("height", 0).min(u64::from(u32::MAX)) as u32,
        channels: u("channels", u64::from(d.channels)).clamp(1, u64::from(MAX_CHANNELS)) as u16,
        depth: match u("depth", 8) {
            16 => 16,
            32 => 32,
            _ => 8,
        },
        interleaved: f.get("interleaved").and_then(Value::as_bool).unwrap_or(d.interleaved),
        little_endian: match f.get("byteOrder").and_then(Value::as_str) {
            Some("mac") => false,
            Some("pc") => true,
            _ => d.little_endian,
        },
        header: u("header", 0),
    }
}

fn set_layout(f: &mut Map<String, Value>, l: &RawLayout) {
    f.insert("width".into(), json!(l.width));
    f.insert("height".into(), json!(l.height));
    f.insert("channels".into(), json!(l.channels));
    f.insert("depth".into(), json!(l.depth));
    f.insert("interleaved".into(), json!(l.interleaved));
    f.insert("byteOrder".into(), json!(if l.little_endian { "pc" } else { "mac" }));
    f.insert("header".into(), json!(l.header));
}

/// Opens Photoshop Raw Options for a file being opened (`path` when it came from disk). `None`
/// when the file isn't Photoshop Raw (the caller opens it as usual).
pub fn intercept(app: &mut PhotocraftApp, name: &str, path: Option<&str>, bytes: &[u8]) -> Option<Result<(), String>> {
    if !is_photoshop_raw(name, bytes) {
        return None;
    }
    let mut f = Map::new();
    f.insert(MARK.into(), json!(true));
    f.insert("__label".into(), json!("Photoshop Raw Options"));
    let start = photoshop_raw::guess(bytes.len() as u64, &RawLayout::default()).unwrap_or_default();
    set_layout(&mut f, &start);
    let id = app.ui.open_dialog(DialogKind::Command, f);
    if let Some(d) = app.ui.dialog_mut(id) {
        d.fields.insert("__id".into(), json!(id));
    }
    app.raw_opens.insert(id, Source { name: name.to_string(), path: path.map(str::to_string), bytes: Arc::new(bytes.to_vec()) });
    Some(Ok(()))
}

/// Forget the file of a dialog that closed.
pub fn closed(app: &mut PhotocraftApp, id: u64) {
    app.raw_opens.remove(&id);
}

fn dialog_id(f: &Map<String, Value>) -> Option<u64> {
    f.get("__id").and_then(Value::as_u64)
}

/// A whole-number field: `label`, the value, its range and unit.
fn number(ui: &mut egui::Ui, t: &Tokens, f: &mut Map<String, Value>, key: &str, label: &str, range: std::ops::RangeInclusive<f32>, unit: &str) {
    let mut v = f.get(key).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    ui.horizontal(|ui| {
        ui.add_sized([70.0, 18.0], egui::Label::new(egui::RichText::new(label).color(t.text_dim)));
        crate::widgets::value_field(ui, &mut v, range, unit, 100.0);
    });
    f.insert(key.into(), json!(v.round().max(0.0) as u64));
}

pub fn body(app: &mut PhotocraftApp, ui: &mut egui::Ui, f: &mut Map<String, Value>) {
    let t = Tokens::get(ui.ctx());
    let Some(len) = dialog_id(f).and_then(|id| app.raw_opens.get(&id)).map(|s| s.bytes.len() as u64) else { return };
    ui.label(egui::RichText::new(format!("{} {}", tl!("File size:"), crate::sizing::human_bytes(len as f64))).color(t.text_dim).size(11.5));
    ui.add_space(4.0);
    ui.label(egui::RichText::new(tl!("Dimensions")).font(crate::theme::semibold(12.0)).color(t.text));
    number(ui, &t, f, "width", tl!("Width"), 0.0..=300_000.0, " px");
    number(ui, &t, f, "height", tl!("Height"), 0.0..=300_000.0, " px");
    if crate::widgets::secondary_button(ui, tl!("Swap"), 70.0).clicked() {
        let (w, h) = (f.get("width").cloned(), f.get("height").cloned());
        f.insert("width".into(), h.unwrap_or(json!(0)));
        f.insert("height".into(), w.unwrap_or(json!(0)));
    }
    ui.add_space(6.0);
    ui.label(egui::RichText::new(tl!("Channels")).font(crate::theme::semibold(12.0)).color(t.text));
    number(ui, &t, f, "channels", tl!("Count"), 1.0..=f32::from(MAX_CHANNELS), "");
    let mut inter = layout(f).interleaved;
    crate::widgets::checkbox(ui, &mut inter, tl!("Interleaved"));
    f.insert("interleaved".into(), json!(inter));
    ui.horizontal(|ui| {
        ui.add_sized([70.0, 18.0], egui::Label::new(egui::RichText::new(tl!("Depth")).color(t.text_dim)));
        let mut d = u64::from(layout(f).depth);
        let opts: Vec<(u64, &str)> = vec![(8, tl!("8 bit")), (16, tl!("16 bit")), (32, tl!("32 bit"))];
        if crate::widgets::dropdown(ui, "raw-open-depth", &mut d, &opts, 100.0) {
            f.insert("depth".into(), json!(d));
        }
    });
    if layout(f).depth > 8 {
        ui.horizontal(|ui| {
            ui.add_sized([70.0, 18.0], egui::Label::new(egui::RichText::new(tl!("Byte Order")).color(t.text_dim)));
            let mut b = if layout(f).little_endian { "pc".to_string() } else { "mac".to_string() };
            let opts: Vec<(String, &str)> = vec![("pc".into(), tl!("IBM PC")), ("mac".into(), tl!("Macintosh"))];
            if crate::widgets::dropdown(ui, "raw-open-byte-order", &mut b, &opts, 120.0) {
                f.insert("byteOrder".into(), json!(b));
            }
        });
    }
    ui.add_space(6.0);
    ui.label(egui::RichText::new(tl!("Header")).font(crate::theme::semibold(12.0)).color(t.text));
    number(ui, &t, f, "header", tl!("Size"), 0.0..=1_919_999.0, " bytes");
    ui.add_space(6.0);
    let l = layout(f);
    let need = l.file_len().unwrap_or(u64::MAX);
    let (msg, ok) = match need.cmp(&len) {
        std::cmp::Ordering::Equal => (tl!("The size matches the file.").to_string(), true),
        std::cmp::Ordering::Less => (crate::i18n::fmt(tl!("{n} bytes past the image are ignored."), &[("n", &(len - need).to_string())]), false),
        std::cmp::Ordering::Greater => (tl!("The file is too small for this size.").to_string(), false),
    };
    ui.label(egui::RichText::new(msg).color(if ok { t.text_dim } else { t.warning }).size(11.5));
    if crate::widgets::secondary_button(ui, tl!("Guess"), 70.0).clicked()
        && let Some(g) = photoshop_raw::guess(len, &RawLayout { width: 0, height: 0, ..l })
    {
        set_layout(f, &g);
    }
}

/// Opens the dialog again with `f`'s entries and the file (after an OK that couldn't open it).
fn reopen(app: &mut PhotocraftApp, f: &Map<String, Value>, src: Source) {
    let id = app.ui.open_dialog(DialogKind::Command, f.clone());
    if let Some(d) = app.ui.dialog_mut(id) {
        d.fields.insert("__id".into(), json!(id));
    }
    app.raw_opens.insert(id, src);
}

/// OK: open the file with the dialog's layout.
pub fn confirm(app: &mut PhotocraftApp, f: &Map<String, Value>) -> Result<Value, String> {
    let id = dialog_id(f).ok_or("no Photoshop Raw Options dialog")?;
    let src = app.raw_opens.remove(&id).ok_or("the file is no longer open in Photoshop Raw Options")?;
    let l = layout(f);
    let r = match photoshop_raw::import(&src.name, &src.bytes, &l) {
        Ok(r) => r,
        Err(e) => {
            // OK closed the dialog: bring it back with the file, so a corrected size can be tried.
            reopen(app, f, src);
            return Err(e.to_string());
        }
    };
    app.session.open_document(r.document, None);
    if let Some(p) = &src.path {
        app.opened_from(p);
    }
    app.sync_views();
    photocraft_engine::automate_cmds::document_opened(&mut app.session);
    crate::notices::io_warnings(app, &format!("Opened {}", src.name), &r.warnings);
    app.ui.status = format!("Opened {}", src.name);
    app.ui.status_error = false;
    Ok(json!({"opened": src.name, "width": l.width, "height": l.height, "channels": l.channels}))
}

#[cfg(test)]
#[path = "raw_open_ui_tests.rs"]
mod tests;
