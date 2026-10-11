//! Floating documents, as in Photoshop: drag a document's tab off the tab strip and it floats in
//! a window of its own over the workspace (Window › Arrange › Float in Window does the same);
//! drag that window by its title bar onto the tab strip and it snaps back in as a tab where the
//! insertion line shows (Window › Arrange › Consolidate All to Tabs docks them all).
//!
//! A floating document leaves the tab strip; the tab area shows the docked document last
//! active. Whichever document is active, docked or floating, takes the tools. Floating windows
//! are kept by document id, so closing or reordering other documents doesn't move them; they
//! last for the session (a workspace restores the panels, not documents).
//!
//! Window › Arrange › New Window for Document still opens a separate OS window (another
//! monitor), see `canvas::extra_windows`.

use egui::{Rect, Sense, Stroke, pos2, vec2};
use photocraft_doc::DocId;
use serde::{Deserialize, Serialize};

use crate::PhotocraftApp;
use crate::theme::Tokens;

/// A floating document: which one, and its window (left, top, width, height in points).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FloatingDoc {
    pub doc: DocId,
    pub rect: [f32; 4],
}

/// How far (points) above or below the tab strip a dragged tab must be released to float.
pub const TEAR_OFF: f32 = 28.0;

/// The title bar's height.
pub const HEADER_H: f32 = 24.0;

fn index_of(app: &PhotocraftApp, doc: DocId) -> Option<usize> {
    app.session.documents().iter().position(|st| st.doc.id == doc)
}

/// Is document `idx` floating?
pub fn is_floating(app: &PhotocraftApp, idx: usize) -> bool {
    app.session.documents().get(idx).is_some_and(|st| app.ui.floating_docs.iter().any(|f| f.doc == st.doc.id))
}

/// The documents on the tab strip, in order.
pub fn docked(app: &PhotocraftApp) -> Vec<usize> {
    (0..app.session.documents().len()).filter(|&i| !is_floating(app, i)).collect()
}

/// The document the tab area shows: the active one when it's docked, else the docked one last
/// active, else the first docked one.
pub fn shown_docked(app: &PhotocraftApp) -> Option<usize> {
    if let Some(a) = app.session.active_index()
        && !is_floating(app, a)
    {
        return Some(a);
    }
    if let Some(i) = app.ui.docked_doc.and_then(|d| index_of(app, d))
        && !is_floating(app, i)
    {
        return Some(i);
    }
    docked(app).first().copied()
}

/// Once a frame, before the tab area draws: forget windows of closed documents, and remember
/// the active document while it's docked.
pub fn track(app: &mut PhotocraftApp) {
    let ids: Vec<DocId> = app.session.documents().iter().map(|st| st.doc.id).collect();
    app.ui.floating_docs.retain(|f| ids.contains(&f.doc));
    if let Some(a) = app.session.active_index()
        && !is_floating(app, a)
    {
        app.ui.docked_doc = app.session.documents().get(a).map(|st| st.doc.id);
    }
}

/// The window a document floats in when dropped at `at` (or cascaded as the `n`th): about half
/// the workspace, the pointer on its title bar.
pub fn window_rect(canvas: Rect, at: Option<egui::Pos2>, n: usize) -> [f32; 4] {
    let canvas = if canvas.is_positive() { canvas } else { Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0)) };
    let w = (canvas.width() * 0.55).clamp(320.0, 1400.0);
    let h = (canvas.height() * 0.6).clamp(240.0, 1000.0);
    let p = at.map_or_else(|| canvas.min + vec2(30.0 + 26.0 * n as f32, 30.0 + 26.0 * n as f32), |p| p - vec2(60.0, HEADER_H / 2.0));
    [p.x, p.y, w, h]
}

/// Float document `idx` (Float in Window, or its tab dropped off the strip at `at`) and make it
/// active.
pub fn float(app: &mut PhotocraftApp, idx: usize, at: Option<egui::Pos2>) -> Result<(), String> {
    let id = app.session.documents().get(idx).map(|st| st.doc.id).ok_or_else(|| format!("no document {idx}"))?;
    let n = app.ui.floating_docs.len();
    let rect = window_rect(app.last_canvas_rect, at, n);
    match app.ui.floating_docs.iter_mut().find(|f| f.doc == id) {
        Some(f) => f.rect = rect,
        None => app.ui.floating_docs.push(FloatingDoc { doc: id, rect }),
    }
    if let Some(v) = app.ui.views.get_mut(idx) {
        v.fit_pending = true;
    }
    app.session.set_active(idx);
    Ok(())
}

/// Dock floating document `idx` back on the tab strip, before the tab at insertion index `slot`
/// (the end without one), and make it active.
pub fn dock(app: &mut PhotocraftApp, idx: usize, slot: Option<usize>) -> Result<(), String> {
    let id = app.session.documents().get(idx).map(|st| st.doc.id).ok_or_else(|| format!("no document {idx}"))?;
    app.ui.floating_docs.retain(|f| f.doc != id);
    if let Some(v) = app.ui.views.get_mut(idx) {
        v.fit_pending = true;
    }
    let n = app.session.documents().len();
    let to = slot.map_or(n.saturating_sub(1), |k| if k > idx { k - 1 } else { k }).min(n.saturating_sub(1));
    if to != idx {
        app.run("document.move", serde_json::json!({"document": idx, "to": to}))?;
    }
    app.session.set_active(to);
    Ok(())
}

/// Window › Arrange › Consolidate All to Tabs: floating documents and split panes back into one
/// tab strip.
pub fn dock_all(app: &mut PhotocraftApp) {
    app.ui.floating_docs.clear();
    crate::doc_panes::consolidate(app);
    app.ui.views.iter_mut().for_each(|v| v.fit_pending = true);
}

/// Window › Arrange › Float All in Windows / Cascade.
pub fn float_all(app: &mut PhotocraftApp) {
    let active = app.session.active_index();
    let canvas = app.last_canvas_rect;
    let docs: Vec<DocId> = app.session.documents().iter().map(|st| st.doc.id).collect();
    app.ui.floating_docs = docs.iter().enumerate().map(|(n, &doc)| FloatingDoc { doc, rect: window_rect(canvas, None, n) }).collect();
    app.ui.views.iter_mut().for_each(|v| v.fit_pending = true);
    if let Some(a) = active {
        app.session.set_active(a);
    }
}

/// The window title, as Photoshop's: "name @ 50% (RGB/8)", with " *" when unsaved.
fn title(app: &PhotocraftApp, idx: usize) -> String {
    let Some(st) = app.session.documents().get(idx) else { return String::new() };
    let zoom = app.ui.views.get(idx).map_or(100.0, |v| v.zoom / app.ppp.max(0.01) * 100.0);
    let mode = crate::canvas::mode_label(&st.doc);
    format!("{} @ {:.0}% ({mode}/{}){}", st.doc.name, zoom, st.doc.depth.bits(), if st.is_dirty() { " *" } else { "" })
}

/// While a tab is dragged off the strip: the window it will float in.
pub fn paint_float_ghost(ctx: &egui::Context, canvas: Rect, p: egui::Pos2) {
    let t = Tokens::get(ctx);
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    let fg = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("doc-float-ghost")));
    let [x, y, w, h] = window_rect(canvas, Some(p), 0);
    let r = Rect::from_min_size(pos2(x, y), vec2(w, h));
    fg.rect_filled(r, t.radius_sm, t.accent.gamma_multiply(0.08));
    fg.rect_stroke(r, t.radius_sm, Stroke::new(1.5, t.accent.gamma_multiply(0.7)), egui::StrokeKind::Inside);
}

/// Is `p` off the strip `strip` (far enough above or below it, or past either end, towards
/// another pane) so a tab released there leaves it?
pub fn torn_off(strip: Rect, p: egui::Pos2) -> bool {
    p.y > strip.bottom() + TEAR_OFF || p.y < strip.top() - TEAR_OFF || p.x < strip.left() || p.x > strip.right()
}

/// Draw the floating documents (after the tab area, so the strip is known): each in a resizable
/// window with its title bar (drag to move; onto the tab strip to dock; × closes the document).
pub fn windows(app: &mut PhotocraftApp, ctx: &egui::Context) {
    let floats = app.ui.floating_docs.clone();
    if floats.is_empty() {
        return;
    }
    let t = Tokens::get(ctx);
    let active = app.session.active_index();
    let (mut docking, mut close, mut activate) = (None, None, None);
    for f in floats {
        let Some(idx) = index_of(app, f.doc) else { continue };
        let primary = Some(idx) == active;
        let id = egui::Id::new(("floating-doc", f.doc.0));
        let frame = egui::Frame::NONE
            .fill(t.canvas)
            .stroke(Stroke::new(1.0, if primary { t.accent_border } else { t.card_border }))
            .shadow(egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: t.shadow });
        let r = f.rect;
        let name = title(app, idx);
        let shown = egui::Window::new(&name)
            .id(id)
            .title_bar(false)
            .movable(false)
            .resizable(true)
            .current_pos(pos2(r[0], r[1]))
            .default_size(vec2(r[2], r[3]))
            .min_size(vec2(220.0, 160.0))
            .frame(frame)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H), Sense::hover());
                let head = ui.interact(hr, id.with("head"), Sense::click_and_drag());
                ui.painter().rect_filled(hr, 0.0, if primary { t.chrome } else { t.tab_strip });
                ui.painter().line_segment([hr.left_bottom(), hr.right_bottom()], Stroke::new(1.0, t.separator));
                let xr = Rect::from_center_size(pos2(hr.right() - 12.0, hr.center().y), vec2(16.0, 16.0));
                let font = egui::FontId::proportional(11.5);
                let g = crate::tab_strip::elided(ui, &name, font, t.text, (hr.width() - 40.0).max(1.0));
                ui.painter().galley_with_override_text_color(pos2(hr.left() + 8.0, hr.center().y - g.size().y / 2.0), g, if primary { t.text } else { t.text_dim });
                let x = ui.interact(xr, id.with("close"), Sense::click());
                x.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tl!("Close")));
                if x.hovered() {
                    ui.painter().rect_filled(xr, 4.0, t.hover);
                }
                crate::icons::paint(ui, xr, "x", 10.0, if x.hovered() { t.text } else { t.text_faint });
                let body = ui.available_rect_before_wrap();
                if body.width() > 1.0 && body.height() > 1.0 {
                    let view = app.ui.views.get(idx).cloned().unwrap_or_default();
                    let v = crate::canvas::canvas_view(app, ui, idx, body, view, primary);
                    if !primary && let Some(slot) = app.ui.views.get_mut(idx) {
                        *slot = v;
                    }
                }
                (head, x.clicked())
            });
        let Some(w) = shown else { continue };
        let Some((head, closed)) = w.inner else { continue };
        let wr = w.response.rect;
        if closed {
            close = Some(idx);
            continue;
        }
        // A press anywhere in a window that isn't the active document's makes it active.
        if !primary && ctx.input(|i| i.pointer.primary_pressed() && i.pointer.interact_pos().is_some_and(|p| wr.contains(p))) {
            activate = Some(idx);
        }
        let mut rect = [wr.left(), wr.top(), wr.width(), wr.height()];
        if head.dragged() {
            let d = head.drag_delta();
            rect[0] += d.x;
            rect[1] += d.y;
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        // Over a tab strip, an edge of the document area or of a pane: it would dock there.
        let to = ctx
            .input(|i| i.pointer.interact_pos())
            .map(|p| crate::doc_panes::window_target(app, ctx, idx, p))
            .filter(|to| !matches!(to, crate::doc_panes::Drop::Float(_)));
        if head.dragged()
            && let Some(to) = to
        {
            crate::doc_panes::preview(app, ctx, to);
        }
        if head.drag_stopped()
            && let Some(to) = to
        {
            docking = Some((idx, to));
        }
        if let Some(stored) = app.ui.floating_docs.iter_mut().find(|x| x.doc == f.doc) {
            stored.rect = rect;
        }
    }
    if let Some(i) = activate {
        app.session.set_active(i);
    }
    if let Some((idx, to)) = docking
        && let Err(e) = crate::doc_panes::apply(app, idx, to)
    {
        app.ui.status = e;
        app.ui.status_error = true;
    }
    if let Some(i) = close {
        let _ = crate::menus::invoke(app, ctx, "file.close", serde_json::json!({"document": i}));
    }
}

#[cfg(test)]
#[path = "doc_float_tests.rs"]
mod tests;
