//! Perspective Crop Tool (C), as in Photoshop: drag a frame, or click its four corners one by
//! one, then drag each corner onto the corners of something seen at an angle (a painting on a
//! wall, a page on a desk). ↵, a double-click inside or the options bar's ✓ straightens it into
//! the new rectangular canvas (`image.perspectiveCrop`); Esc or ⊘ drops the frame.
//!
//! Dragging a corner moves it alone, a side's middle handle moves that side, and inside the frame
//! moves it all. A frame whose corners don't make a convex shape is drawn red and can't be
//! committed. The options bar has W, H and Resolution (the Crop tool's fields; empty W or H keeps
//! the frame's own size), Front Image, Clear and Show Grid. Leaving the tool or the document
//! drops the frame.

use egui::{Color32, CursorIcon, Mesh, Modifiers, Pos2, Stroke};
use photocraft_doc::DocId;
use photocraft_engine::perspective_crop_cmds::{natural_size, valid};
use serde_json::json;

use crate::PhotocraftApp;
use crate::canvas::{ToolEvent, ViewXform};
use crate::state::Tool;

/// Handle grab distance (screen px).
const HANDLE_PX: f64 = 8.0;
/// A press that moves less than this (screen px) is a click: it places a corner.
const CLICK_PX: f64 = 3.0;
/// Grid divisions per side.
const GRID: usize = 4;

#[derive(Clone, Debug, PartialEq)]
enum Drag {
    /// Drawing a new rectangular frame from `anchor` (or a click, placing a corner).
    Draw { anchor: [f64; 2], moved: bool },
    /// Corner `i` (clockwise from top-left).
    Corner { i: usize, start: [f64; 2], quad: [[f64; 2]; 4] },
    /// Side `i` (from corner `i` to `i + 1`).
    Side { i: usize, start: [f64; 2], quad: [[f64; 2]; 4] },
    Move { start: [f64; 2], quad: [[f64; 2]; 4] },
}

/// The tool's frame and gesture (view state, not History).
#[derive(Clone, Debug, Default)]
pub struct State {
    doc: Option<DocId>,
    /// The frame (document px), corners clockwise from top-left.
    quad: Option<[[f64; 2]; 4]>,
    /// Corners placed by clicking, until there are four.
    placing: Vec<[f64; 2]>,
    drag: Option<Drag>,
}

impl State {
    pub fn quad(&self) -> Option<[[f64; 2]; 4]> {
        self.quad
    }
}

fn tolerance(app: &PhotocraftApp) -> f64 {
    HANDLE_PX / f64::from(app.point_zoom()).max(0.01)
}

/// Drop the frame and any corners placed.
pub fn cancel(app: &mut PhotocraftApp) {
    app.persp = State::default();
}

/// A frame or corners are pending on the active document.
pub fn pending(app: &PhotocraftApp) -> bool {
    app.ui.tool == Tool::PerspectiveCrop && (app.persp.quad.is_some() || !app.persp.placing.is_empty())
}

/// Leaving the tool, or another document becoming active, drops the frame.
pub fn end_if_left(app: &mut PhotocraftApp) {
    let doc = app.session.active().map(|st| st.doc.id);
    let has = app.persp.quad.is_some() || !app.persp.placing.is_empty() || app.persp.drag.is_some();
    if has && (app.ui.tool != Tool::PerspectiveCrop || app.persp.doc != doc) {
        cancel(app);
    }
}

/// Four points in the frame's order: clockwise on screen, from the top-left one (smallest x + y).
pub fn ordered(pts: [[f64; 2]; 4]) -> [[f64; 2]; 4] {
    let c = [pts.iter().map(|p| p[0]).sum::<f64>() / 4.0, pts.iter().map(|p| p[1]).sum::<f64>() / 4.0];
    let mut v = pts.to_vec();
    // Screen angle (y down): increasing atan2 goes clockwise.
    v.sort_by(|a, b| (a[1] - c[1]).atan2(a[0] - c[0]).total_cmp(&(b[1] - c[1]).atan2(b[0] - c[0])));
    let first = (0..4).min_by(|&i, &j| (v[i][0] + v[i][1]).total_cmp(&(v[j][0] + v[j][1]))).unwrap_or(0);
    [v[first], v[(first + 1) % 4], v[(first + 2) % 4], v[(first + 3) % 4]]
}

fn rect_quad(a: [f64; 2], b: [f64; 2]) -> [[f64; 2]; 4] {
    let (x0, x1, y0, y1) = (a[0].min(b[0]), a[0].max(b[0]), a[1].min(b[1]), a[1].max(b[1]));
    [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn mid(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
}

/// Point `p` inside the (convex, clockwise) quad.
fn inside(q: &[[f64; 2]; 4], p: [f64; 2]) -> bool {
    (0..4).all(|i| {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]) >= 0.0
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Corner(usize),
    Side(usize),
    Inside,
    Outside,
}

fn hit(q: &[[f64; 2]; 4], p: [f64; 2], tol: f64) -> Hit {
    if let Some(i) = (0..4).find(|&i| dist(q[i], p) <= tol) {
        return Hit::Corner(i);
    }
    if let Some(i) = (0..4).find(|&i| dist(mid(q[i], q[(i + 1) % 4]), p) <= tol) {
        return Hit::Side(i);
    }
    if inside(q, p) { Hit::Inside } else { Hit::Outside }
}

/// Pointer input for the tool. Returns true when the event was its.
pub fn pointer(app: &mut PhotocraftApp, ev: ToolEvent, _mods: Modifiers) -> bool {
    if app.ui.tool != Tool::PerspectiveCrop {
        return false;
    }
    let p = match ev {
        ToolEvent::Down { x, y, .. } | ToolEvent::Move { x, y, .. } | ToolEvent::Up { x, y } => [x, y],
    };
    if app.session.active().is_none() || !p[0].is_finite() || !p[1].is_finite() {
        return true;
    }
    app.persp.doc = app.session.active().map(|st| st.doc.id);
    let tol = tolerance(app);
    match ev {
        ToolEvent::Down { .. } => {
            app.persp.drag = Some(match app.persp.quad {
                Some(q) => match hit(&q, p, tol) {
                    Hit::Corner(i) => Drag::Corner { i, start: p, quad: q },
                    Hit::Side(i) => Drag::Side { i, start: p, quad: q },
                    Hit::Inside => Drag::Move { start: p, quad: q },
                    // Outside a frame: nothing (as in Photoshop; ↵ or Esc ends the frame).
                    Hit::Outside => return true,
                },
                None => Drag::Draw { anchor: p, moved: false },
            });
        }
        ToolEvent::Move { .. } => match app.persp.drag.clone() {
            Some(Drag::Draw { anchor, moved }) => {
                let moved = moved || dist(anchor, p) > CLICK_PX / f64::from(app.point_zoom()).max(0.01);
                // Dragging a frame replaces any corners placed by clicking.
                if moved {
                    app.persp.placing.clear();
                    app.persp.quad = Some(rect_quad(anchor, p));
                }
                app.persp.drag = Some(Drag::Draw { anchor, moved });
            }
            Some(Drag::Corner { i, start, mut quad }) => {
                quad[i] = [quad[i][0] + p[0] - start[0], quad[i][1] + p[1] - start[1]];
                app.persp.quad = Some(quad);
            }
            Some(Drag::Side { i, start, mut quad }) => {
                for k in [i, (i + 1) % 4] {
                    quad[k] = [quad[k][0] + p[0] - start[0], quad[k][1] + p[1] - start[1]];
                }
                app.persp.quad = Some(quad);
            }
            Some(Drag::Move { start, quad }) => app.persp.quad = Some(quad.map(|c| [c[0] + p[0] - start[0], c[1] + p[1] - start[1]])),
            None => {}
        },
        ToolEvent::Up { .. } => {
            if let Some(Drag::Draw { moved: false, .. }) = app.persp.drag.take() {
                // A click places the next corner; the fourth completes the frame.
                app.persp.placing.push(p);
                if let Ok(four) = <[[f64; 2]; 4]>::try_from(app.persp.placing.as_slice()) {
                    app.persp.quad = Some(ordered(four));
                    app.persp.placing.clear();
                }
            }
            // A frame too small to see is dropped.
            if app.persp.quad.is_some_and(|q| natural_size(&q).0 < 1.0 || natural_size(&q).1 < 1.0) {
                app.persp.quad = None;
            }
        }
    }
    true
}

/// The `image.perspectiveCrop` params for the frame: the options bar's W × H (px) and resolution.
pub fn commit_params(app: &PhotocraftApp, q: [[f64; 2]; 4]) -> serde_json::Value {
    let o = &app.ui.tool_options;
    let mut p = json!({"quad": q, "interpolation": crate::sizing::pref_resample(app.session.prefs().general.image_interpolation)});
    let doc_dpi = app.session.active().map_or(72.0, |st| f64::from(st.doc.resolution_dpi));
    if let Some((w, h, _)) = crate::crop_size::target_of(&o.persp_width, &o.persp_height, &o.persp_resolution, &o.persp_resolution_unit, doc_dpi) {
        p["width"] = json!(w.round().max(1.0));
        p["height"] = json!(h.round().max(1.0));
    }
    if let Some(r) = crate::crop_size::resolution_ppi_of(&o.persp_resolution, &o.persp_resolution_unit) {
        p["resolution"] = json!(r);
    }
    p
}

/// ↵, a double-click inside or ✓: straighten the frame into the new canvas.
pub fn commit(app: &mut PhotocraftApp) {
    end_if_left(app);
    let Some(q) = app.persp.quad else { return };
    if !valid(&q) {
        app.ui.status = tl!("The corners must make a convex shape: move them so no two sides cross").into();
        app.ui.status_error = true;
        return;
    }
    let p = commit_params(app, q);
    match app.run("image.perspectiveCrop", p) {
        Ok(_) => cancel(app),
        Err(e) => {
            app.ui.status = e;
            app.ui.status_error = true;
        }
    }
}

/// A double-click at document point `p` commits (inside the frame, not on a handle).
pub fn commits_at(app: &PhotocraftApp, p: [f64; 2]) -> bool {
    app.ui.tool == Tool::PerspectiveCrop && app.persp.drag.is_none() && app.persp.quad.is_some_and(|q| hit(&q, p, tolerance(app)) == Hit::Inside)
}

/// ↵ commits and Esc cancels while a frame or corners are pending. Returns true when used.
pub fn keys(app: &mut PhotocraftApp, ctx: &egui::Context) -> bool {
    if !pending(app) {
        return false;
    }
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, egui::Key::Enter)) {
        commit(app);
        return true;
    }
    if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, egui::Key::Escape)) {
        cancel(app);
        return true;
    }
    false
}

pub fn cursor(app: &PhotocraftApp, p: [f64; 2]) -> CursorIcon {
    match app.persp.quad.map(|q| hit(&q, p, tolerance(app))) {
        Some(Hit::Corner(_) | Hit::Side(_)) => CursorIcon::Crosshair,
        Some(Hit::Inside) => CursorIcon::Move,
        Some(Hit::Outside) => CursorIcon::Default,
        None => CursorIcon::Crosshair,
    }
}

/// The frame over the canvas: the image dimmed outside it, its outline (red while it can't be
/// straightened), the perspective grid and the handles; or the corners placed so far.
pub fn draw(app: &PhotocraftApp, painter: &egui::Painter, xf: &ViewXform) {
    if app.ui.tool != Tool::PerspectiveCrop {
        return;
    }
    let t = crate::theme::Tokens::get(painter.ctx());
    let scr = |p: [f64; 2]| xf.to_screen(p[0] as f32, p[1] as f32);
    if let Some(q) = app.persp.quad {
        let s = q.map(scr);
        let ok = valid(&q);
        if ok {
            shield(painter, s);
        }
        let line = if ok { Color32::WHITE } else { Color32::from_rgb(230, 40, 40) };
        if ok && app.ui.tool_options.persp_grid {
            grid(painter, &q, &scr);
        }
        painter.add(egui::Shape::closed_line(s.to_vec(), Stroke::new(1.0, Color32::from_black_alpha(160))));
        painter.add(egui::Shape::closed_line(s.to_vec(), Stroke::new(1.0, line)));
        let handles = (0..4).map(|i| s[i]).chain((0..4).map(|i| s[i].lerp(s[(i + 1) % 4], 0.5)));
        for h in handles {
            let r = egui::Rect::from_center_size(h, egui::vec2(7.0, 7.0));
            painter.rect_filled(r, 0.0, Color32::WHITE);
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
        }
    } else if !app.persp.placing.is_empty() {
        let pts: Vec<Pos2> = app.persp.placing.iter().map(|p| scr(*p)).collect();
        painter.add(egui::Shape::line(pts.clone(), Stroke::new(1.0, Color32::WHITE)));
        for p in pts {
            painter.circle_filled(p, 3.5, Color32::WHITE);
            painter.circle_stroke(p, 3.5, Stroke::new(1.0, t.accent));
        }
    }
}

/// Dim the view outside the frame: four bands between the view's corners and the frame's.
fn shield(painter: &egui::Painter, s: [Pos2; 4]) {
    let v = painter.clip_rect().expand(4000.0);
    let outer = [v.left_top(), v.right_top(), v.right_bottom(), v.left_bottom()];
    let mut mesh = Mesh::default();
    let fill = Color32::from_black_alpha(130);
    for i in 0..4 {
        let j = (i + 1) % 4;
        let base = mesh.vertices.len() as u32;
        for p in [outer[i], outer[j], s[j], s[i]] {
            mesh.vertices.push(egui::epaint::Vertex { pos: p, uv: egui::epaint::WHITE_UV, color: fill });
        }
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// The frame's perspective grid: the lines a square grid on the straightened image maps to.
fn grid(painter: &egui::Painter, q: &[[f64; 2]; 4], scr: &impl Fn([f64; 2]) -> Pos2) {
    let Some(h) = photocraft_algo::transform::Homography::rect_to_quad([0.0, 0.0, 1.0, 1.0], *q) else { return };
    let at = |u: f64, v: f64| {
        let (x, y) = h.apply(u, v);
        scr([x, y])
    };
    let stroke = Stroke::new(1.0, Color32::from_white_alpha(110));
    for k in 1..GRID {
        let f = k as f64 / GRID as f64;
        painter.line_segment([at(f, 0.0), at(f, 1.0)], stroke);
        painter.line_segment([at(0.0, f), at(1.0, f)], stroke);
    }
}

/// The options bar: W ⇄ H, Resolution, Front Image, Clear, Show Grid and a hint, then ⊘ and ✓.
pub fn options_bar(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    use crate::crop_size::{PX_PER_CM, PX_PER_IN, normalize_length, normalize_resolution, text_field};
    let o = &mut app.ui.tool_options;
    text_field(ui, "persp-width", &mut o.persp_width, tl!("Width"), 64.0, normalize_length);
    if crate::icons::button(ui, "arrow-left-right", 22.0, false, tl!("Swaps height and width")).clicked() {
        std::mem::swap(&mut o.persp_width, &mut o.persp_height);
    }
    text_field(ui, "persp-height", &mut o.persp_height, tl!("Height"), 64.0, normalize_length);
    text_field(ui, "persp-resolution", &mut o.persp_resolution, tl!("Resolution"), 64.0, normalize_resolution);
    let units = [(PX_PER_IN.to_string(), "px/in"), (PX_PER_CM.to_string(), "px/cm")];
    crate::widgets::dropdown(ui, "persp-resolution-unit", &mut o.persp_resolution_unit, &units, 64.0);
    crate::widgets::vline(ui, 22.0);
    if crate::widgets::secondary_button(ui, tl!("Front Image"), 0.0).clicked() {
        front_image(app);
    }
    let o = &mut app.ui.tool_options;
    if crate::widgets::secondary_button(ui, tl!("Clear"), 0.0).clicked() {
        o.persp_width.clear();
        o.persp_height.clear();
        o.persp_resolution.clear();
    }
    crate::widgets::checkbox(ui, &mut o.persp_grid, tl!("Show Grid"));
    crate::widgets::vline(ui, 22.0);
    let dim = crate::theme::Tokens::get(ui.ctx()).text_dim;
    ui.label(egui::RichText::new(hint(app)).color(dim).size(12.0));
    if pending(app) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let tip = crate::i18n::fmt(tl!("Commit current crop operation  ({key})"), &[("key", &crate::shortcuts::pretty("Enter"))]);
            if crate::icons::button(ui, "check", 26.0, false, &tip).clicked() {
                commit(app);
            }
            if crate::icons::button(ui, "ban", 26.0, false, tl!("Cancel current crop operation  (Esc)")).clicked() {
                cancel(app);
            }
        });
    }
}

/// Front Image: W, H (px) and the resolution of the active document.
pub fn front_image(app: &mut PhotocraftApp) {
    let Some((size, dpi)) = app.session.active().map(|st| (st.doc.size, f64::from(st.doc.resolution_dpi))) else { return };
    let o = &mut app.ui.tool_options;
    o.persp_width = crate::crop_size::format_length(f64::from(size.width), photocraft_engine::prefs::Unit::Pixels);
    o.persp_height = crate::crop_size::format_length(f64::from(size.height), photocraft_engine::prefs::Unit::Pixels);
    let res = if o.persp_resolution_unit == crate::crop_size::PX_PER_CM { dpi / 2.54 } else { dpi };
    o.persp_resolution = crate::widgets::fmt_num2(res);
}

/// What to do next, shown in the options bar.
pub fn hint(app: &PhotocraftApp) -> &'static str {
    match (app.persp.quad, app.persp.placing.len()) {
        (Some(_), _) => tl!("Drag the corners onto the edges of the object, then press Enter"),
        (None, 0) => tl!("Drag a frame, or click the four corners of the object"),
        (None, _) => tl!("Click the next corner"),
    }
}

#[cfg(test)]
#[path = "perspective_crop_ui_tests.rs"]
mod tests;
