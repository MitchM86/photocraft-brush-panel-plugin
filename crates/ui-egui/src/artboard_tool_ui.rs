//! Artboard Tool (V, with the Move tool), as in Photoshop:
//!
//! - drag on the pasteboard (anywhere outside a board) to draw a new artboard;
//! - click a board to select it, drag inside it to move it with its contents;
//! - drag the selected board's handles to resize it (its contents stay put);
//! - click a **+** beside the selected board to add a board of the same size on that side.
//!
//! The options bar has the size presets, W and H, portrait / landscape and Add New Artboard. The
//! board changes on release (`layer.new.artboard`, `layer.artboard.set`), one History step each;
//! while dragging, the canvas shows the new outline.

use egui::{Color32, CursorIcon, Modifiers, Pos2, Stroke};
use photocraft_doc::LayerId;
use photocraft_geom::Rect as DRect;
use serde_json::json;

use crate::PhotocraftApp;
use crate::canvas::{ToolEvent, ViewXform};
use crate::state::Tool;

/// Handle grab distance (screen px).
const HANDLE_PX: f64 = 8.0;
/// How far outside the board its + buttons sit (screen px), and their radius.
const PLUS_OFFSET: f64 = 22.0;
const PLUS_RADIUS: f64 = 8.0;
/// Space between a board and one added beside it (document px), as `layer.new.artboard` leaves.
const GAP: f64 = 100.0;

#[derive(Clone, Debug, PartialEq)]
enum Drag {
    Draw { anchor: [f64; 2], cur: [f64; 2] },
    Move { id: LayerId, start: [f64; 2], rect: [f64; 4], cur: [f64; 2] },
    /// `hx`, `hy`: which edges the handle moves (-1 left/top, 1 right/bottom, 0 neither).
    Resize { id: LayerId, hx: i8, hy: i8, start: [f64; 2], rect: [f64; 4], cur: [f64; 2] },
}

#[derive(Clone, Debug, Default)]
pub struct State {
    drag: Option<Drag>,
}

/// A side of a board: where its + button adds the next one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

const SIDES: [Side; 4] = [Side::Left, Side::Right, Side::Top, Side::Bottom];

fn zoom(app: &PhotocraftApp) -> f64 {
    f64::from(app.point_zoom()).max(0.01)
}

fn to_f(r: DRect) -> [f64; 4] {
    [f64::from(r.x0), f64::from(r.y0), f64::from(r.x1), f64::from(r.y1)]
}

/// The selected artboard: the one the active layer is (or is in).
pub fn selected(app: &PhotocraftApp) -> Option<(LayerId, [f64; 4])> {
    let st = app.session.active()?;
    let id = st.doc.artboard_of(st.active_layer?)?;
    Some((id, to_f(st.doc.layer(id)?.artboard()?.rect)))
}

/// The topmost board containing `p`.
fn board_at(app: &PhotocraftApp, p: [f64; 2]) -> Option<(LayerId, [f64; 4])> {
    let st = app.session.active()?;
    st.doc.artboards().into_iter().rev().map(|(id, _, a)| (id, to_f(a.rect))).find(|(_, r)| p[0] >= r[0] && p[0] < r[2] && p[1] >= r[1] && p[1] < r[3])
}

/// Handle positions of `r`: (hx, hy, point), corners and side middles.
fn handles(r: [f64; 4]) -> [(i8, i8, [f64; 2]); 8] {
    let (mx, my) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    [(-1, -1, [r[0], r[1]]), (1, -1, [r[2], r[1]]), (1, 1, [r[2], r[3]]), (-1, 1, [r[0], r[3]]), (0, -1, [mx, r[1]]), (1, 0, [r[2], my]), (0, 1, [mx, r[3]]), (-1, 0, [r[0], my])]
}

fn handle_at(r: [f64; 4], p: [f64; 2], tol: f64) -> Option<(i8, i8)> {
    handles(r).into_iter().find(|(_, _, h)| (h[0] - p[0]).abs() <= tol && (h[1] - p[1]).abs() <= tol).map(|(x, y, _)| (x, y))
}

/// Where the + button of `side` sits (document px) for a board `r` at zoom `z`.
fn plus_at(r: [f64; 4], side: Side, z: f64) -> [f64; 2] {
    let o = PLUS_OFFSET / z;
    let (mx, my) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
    match side {
        Side::Left => [r[0] - o, my],
        Side::Right => [r[2] + o, my],
        Side::Top => [mx, r[1] - o],
        Side::Bottom => [mx, r[3] + o],
    }
}

fn plus_hit(r: [f64; 4], p: [f64; 2], z: f64) -> Option<Side> {
    SIDES.into_iter().find(|s| {
        let c = plus_at(r, *s, z);
        (c[0] - p[0]).hypot(c[1] - p[1]) <= PLUS_RADIUS / z
    })
}

/// The board a + on `side` of `r` adds: the same size, [`GAP`] away. `None` when it would start
/// left of or above the document's origin (boards live at non-negative coordinates).
pub fn beside(r: [f64; 4], side: Side) -> Option<[f64; 4]> {
    let (w, h) = (r[2] - r[0], r[3] - r[1]);
    let (x, y) = match side {
        Side::Left => (r[0] - GAP - w, r[1]),
        Side::Right => (r[2] + GAP, r[1]),
        Side::Top => (r[0], r[1] - GAP - h),
        Side::Bottom => (r[0], r[3] + GAP),
    };
    (x >= 0.0 && y >= 0.0).then_some([x, y, x + w, y + h])
}

/// Whole-pixel board from a rect, its origin kept at or past (0, 0).
fn rounded(r: [f64; 4]) -> [f64; 4] {
    let (x0, y0) = (r[0].min(r[2]).round().max(0.0), r[1].min(r[3]).round().max(0.0));
    let (x1, y1) = (r[0].max(r[2]).round().max(x0 + 1.0), r[1].max(r[3]).round().max(y0 + 1.0));
    [x0, y0, x1, y1]
}

fn xywh(r: [f64; 4]) -> serde_json::Value {
    json!([r[0], r[1], r[2] - r[0], r[3] - r[1]])
}

fn report(app: &mut PhotocraftApp, r: Result<serde_json::Value, String>) {
    if let Err(e) = r {
        app.ui.status = e;
        app.ui.status_error = true;
    }
}

/// Add a board of the selected one's size on `side`.
pub fn add_beside(app: &mut PhotocraftApp, side: Side) {
    let Some((_, r)) = selected(app) else { return };
    match beside(r, side) {
        Some(n) => {
            let res = app.run("layer.new.artboard", json!({"rect": xywh(n)}));
            report(app, res);
        }
        None => {
            app.ui.status = tl!("There is no room for an artboard on that side").into();
            app.ui.status_error = true;
        }
    }
}

/// The rect the current drag shows (document px), if any.
fn dragged_rect(d: &Drag) -> [f64; 4] {
    match d {
        Drag::Draw { anchor, cur } => [anchor[0], anchor[1], cur[0], cur[1]],
        Drag::Move { start, rect, cur, .. } => {
            let (dx, dy) = ((cur[0] - start[0]).round(), (cur[1] - start[1]).round());
            [rect[0] + dx, rect[1] + dy, rect[2] + dx, rect[3] + dy]
        }
        Drag::Resize { hx, hy, start, rect, cur, .. } => {
            let (dx, dy) = (cur[0] - start[0], cur[1] - start[1]);
            let mut r = *rect;
            match hx {
                -1 => r[0] = (r[0] + dx).min(r[2] - 1.0),
                1 => r[2] = (r[2] + dx).max(r[0] + 1.0),
                _ => {}
            }
            match hy {
                -1 => r[1] = (r[1] + dy).min(r[3] - 1.0),
                1 => r[3] = (r[3] + dy).max(r[1] + 1.0),
                _ => {}
            }
            r
        }
    }
}

/// Pointer input for the tool. Returns true when the event was its.
pub fn pointer(app: &mut PhotocraftApp, ev: ToolEvent, _mods: Modifiers) -> bool {
    if app.ui.tool != Tool::Artboard {
        app.artboard_tool.drag = None;
        return false;
    }
    let p = match ev {
        ToolEvent::Down { x, y, .. } | ToolEvent::Move { x, y, .. } | ToolEvent::Up { x, y } => [x, y],
    };
    if app.session.active().is_none() || !p[0].is_finite() || !p[1].is_finite() {
        return true;
    }
    let z = zoom(app);
    match ev {
        ToolEvent::Down { .. } => {
            let sel = selected(app);
            if let Some((_, r)) = sel
                && let Some(side) = plus_hit(r, p, z)
            {
                add_beside(app, side);
                return true;
            }
            app.artboard_tool.drag = Some(match (sel, sel.and_then(|(_, r)| handle_at(r, p, HANDLE_PX / z))) {
                (Some((id, rect)), Some((hx, hy))) => Drag::Resize { id, hx, hy, start: p, rect, cur: p },
                _ => match board_at(app, p) {
                    Some((id, rect)) => {
                        let _ = app.session.select_layer(id);
                        Drag::Move { id, start: p, rect, cur: p }
                    }
                    None => Drag::Draw { anchor: p, cur: p },
                },
            });
        }
        ToolEvent::Move { .. } => {
            if let Some(Drag::Draw { cur, .. } | Drag::Move { cur, .. } | Drag::Resize { cur, .. }) = app.artboard_tool.drag.as_mut() {
                *cur = p;
            }
        }
        ToolEvent::Up { .. } => {
            let Some(d) = app.artboard_tool.drag.take() else { return true };
            let r = dragged_rect(&d);
            let res = match d {
                // A click on the pasteboard draws nothing.
                Drag::Draw { anchor, .. } if (anchor[0] - p[0]).abs() * z < 3.0 || (anchor[1] - p[1]).abs() * z < 3.0 => return true,
                Drag::Draw { .. } => app.run("layer.new.artboard", json!({"rect": xywh(rounded(r))})),
                Drag::Move { rect, .. } if r == rect => return true,
                Drag::Move { id, .. } => app.run("layer.artboard.set", json!({"layer": id.0, "rect": xywh(rounded(r)), "moveContents": true})),
                Drag::Resize { id, .. } => app.run("layer.artboard.set", json!({"layer": id.0, "rect": xywh(rounded(r)), "moveContents": false})),
            };
            report(app, res);
        }
    }
    true
}

pub fn cursor(app: &PhotocraftApp, p: [f64; 2]) -> CursorIcon {
    let z = zoom(app);
    if let Some((_, r)) = selected(app) {
        if plus_hit(r, p, z).is_some() {
            return CursorIcon::PointingHand;
        }
        if let Some((hx, hy)) = handle_at(r, p, HANDLE_PX / z) {
            return match (hx, hy) {
                (0, _) => CursorIcon::ResizeVertical,
                (_, 0) => CursorIcon::ResizeHorizontal,
                (-1, -1) | (1, 1) => CursorIcon::ResizeNwSe,
                _ => CursorIcon::ResizeNeSw,
            };
        }
    }
    if board_at(app, p).is_some() { CursorIcon::Move } else { CursorIcon::Crosshair }
}

/// The selected board's frame, handles and + buttons, and the outline being dragged.
pub fn draw(app: &PhotocraftApp, painter: &egui::Painter, xf: &ViewXform) {
    if app.ui.tool != Tool::Artboard {
        return;
    }
    let t = crate::theme::Tokens::get(painter.ctx());
    let scr = |p: [f64; 2]| xf.to_screen(p[0] as f32, p[1] as f32);
    let quad = |r: [f64; 4]| vec![scr([r[0], r[1]]), scr([r[2], r[1]]), scr([r[2], r[3]]), scr([r[0], r[3]])];
    if let Some(d) = &app.artboard_tool.drag {
        let r = rounded(dragged_rect(d));
        painter.add(egui::Shape::closed_line(quad(r), Stroke::new(2.0, t.accent)));
        return;
    }
    let Some((_, r)) = selected(app) else { return };
    painter.add(egui::Shape::closed_line(quad(r), Stroke::new(2.0, t.accent)));
    for (_, _, h) in handles(r) {
        let rr = egui::Rect::from_center_size(scr(h), egui::vec2(7.0, 7.0));
        painter.rect_filled(rr, 0.0, Color32::WHITE);
        painter.rect_stroke(rr, 0.0, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
    }
    let z = zoom(app);
    for side in SIDES {
        let c: Pos2 = scr(plus_at(r, side, z));
        let on = beside(r, side).is_some();
        let fill = if on { t.accent } else { t.accent.gamma_multiply(0.35) };
        painter.circle_filled(c, PLUS_RADIUS as f32, fill);
        let s = Stroke::new(1.5, Color32::WHITE);
        painter.line_segment([c - egui::vec2(4.0, 0.0), c + egui::vec2(4.0, 0.0)], s);
        painter.line_segment([c - egui::vec2(0.0, 4.0), c + egui::vec2(0.0, 4.0)], s);
    }
}

/// The options bar: size preset, W × H, portrait / landscape, Add New Artboard.
pub fn options_bar(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let sel = selected(app);
    let dim = crate::theme::Tokens::get(ui.ctx()).text_dim;
    ui.add_enabled_ui(sel.is_some(), |ui| {
        let (id, r) = sel.unwrap_or((LayerId(0), [0.0, 0.0, 1.0, 1.0]));
        let (w, h) = (r[2] - r[0], r[3] - r[1]);
        ui.label(egui::RichText::new(tl!("Size:")).color(dim));
        let current = photocraft_engine::artboard_cmds::PRESETS.iter().find(|p| f64::from(p.1) == w && f64::from(p.2) == h).map_or(String::new(), |p| p.0.to_string());
        let mut chosen = current.clone();
        let mut opts: Vec<(String, &str)> = vec![(String::new(), tl!("Custom"))];
        opts.extend(photocraft_engine::artboard_cmds::PRESETS.iter().map(|p| (p.0.to_string(), p.0)));
        if crate::widgets::dropdown(ui, "artboard-size", &mut chosen, &opts, 150.0) && chosen != current && !chosen.is_empty() {
            let res = app.run("layer.artboard.set", json!({"layer": id.0, "preset": chosen, "moveContents": false}));
            report(app, res);
        }
        let (mut nw, mut nh) = (w as f32, h as f32);
        ui.label(egui::RichText::new(tl!("W:")).color(dim));
        let cw = crate::widgets::value_field(ui, &mut nw, 1.0..=300_000.0, "px", 72.0).changed();
        ui.label(egui::RichText::new(tl!("H:")).color(dim));
        let ch = crate::widgets::value_field(ui, &mut nh, 1.0..=300_000.0, "px", 72.0).changed();
        if (cw || ch) && (f64::from(nw.round()) != w || f64::from(nh.round()) != h) {
            let res = app.run("layer.artboard.set", json!({"layer": id.0, "width": nw.round(), "height": nh.round(), "moveContents": false}));
            report(app, res);
        }
        let portrait = h >= w;
        for (icon, tip, want) in [("rectangle-vertical", tl!("Make Portrait"), true), ("rectangle-horizontal", tl!("Make Landscape"), false)] {
            if crate::icons::button(ui, icon, 24.0, portrait == want, tip).clicked() && portrait != want && w != h {
                let res = app.run("layer.artboard.set", json!({"layer": id.0, "width": h, "height": w, "moveContents": false}));
                report(app, res);
            }
        }
    });
    crate::widgets::vline(ui, 22.0);
    if crate::widgets::secondary_button(ui, tl!("Add New Artboard"), 0.0).clicked() {
        let res = app.run("layer.new.artboard", json!({}));
        report(app, res);
    }
    crate::widgets::vline(ui, 22.0);
    let hint = if sel.is_some() {
        tl!("Drag inside to move · drag a handle to resize · click + to add a board beside it")
    } else {
        tl!("Drag to draw an artboard · click one to select it")
    };
    ui.label(egui::RichText::new(hint).color(dim).size(12.0));
}

#[cfg(test)]
#[path = "artboard_tool_ui_tests.rs"]
mod tests;
