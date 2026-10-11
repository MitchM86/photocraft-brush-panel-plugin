//! The Color Wheel panel (Color group, between Color and Swatches): a hue ring around a
//! saturation / brightness square or triangle, colour harmonies, recent colours, and an RGB,
//! painter's RYB or CMYK wheel. It edits the foreground or background colour, the chip the Color
//! panel edits (`color_panel.background`), so both panels stay in step.
//!
//! The CMYK wheel is for print: everything on it is shown as it prints (each colour through the
//! CMYK profile and back: the active CMYK document's, else the working CMYK), a colour picked
//! on it is the printable one, its ink percentages can be typed, and a colour that won't print
//! shows Photoshop's ⚠ (a click takes the nearest printable colour).
//!
//! The geometry and colour maths are plain functions, tested on their own; the panel draws them
//! with egui meshes.

use egui::{Color32, Mesh, Pos2, Rect, Sense, Stroke, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::PhotocraftApp;
use crate::color_picker_ui::{hsv_to_rgb, rgb_to_hsv};
use crate::theme::Tokens;
use crate::widgets;

/// The shape inside the ring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WheelShape {
    #[default]
    Square,
    /// Pure hue, white and black at its corners; it turns with the hue.
    Triangle,
}

/// Which hues sit opposite each other on the ring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WheelKind {
    /// The light (RGB / HSV) wheel: red opposite cyan.
    #[default]
    Rgb,
    /// The painter's wheel: red opposite green, yellow opposite violet.
    Ryb,
    /// The ink wheel: cyan, magenta and yellow opposite red, green and blue, every colour shown
    /// and picked as it prints.
    Cmyk,
}

/// Colours chosen to go with the current one, at fixed angles on the ring.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Harmony {
    #[default]
    None,
    Complementary,
    Analogous,
    Triad,
    SplitComplementary,
    Tetrad,
}

impl Harmony {
    pub const ALL: [Harmony; 6] = [Harmony::None, Harmony::Complementary, Harmony::Analogous, Harmony::Triad, Harmony::SplitComplementary, Harmony::Tetrad];

    pub fn label(self) -> &'static str {
        match self {
            Harmony::None => "No Harmony",
            Harmony::Complementary => "Complementary",
            Harmony::Analogous => "Analogous",
            Harmony::Triad => "Triad",
            Harmony::SplitComplementary => "Split Complementary",
            Harmony::Tetrad => "Tetrad",
        }
    }

    /// The other colours' angles from the current one, in degrees around the (displayed) ring.
    pub fn offsets(self) -> &'static [f32] {
        match self {
            Harmony::None => &[],
            Harmony::Complementary => &[180.0],
            Harmony::Analogous => &[-30.0, 30.0],
            Harmony::Triad => &[120.0, 240.0],
            Harmony::SplitComplementary => &[150.0, 210.0],
            Harmony::Tetrad => &[90.0, 180.0, 270.0],
        }
    }
}

/// The panel's settings and recent colours (saved with the UI state).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorWheelState {
    pub shape: WheelShape,
    pub kind: WheelKind,
    pub harmony: Harmony,
    /// Colours picked here, newest first (sRGB, at most [`RECENT`]).
    pub recent: Vec<[f32; 4]>,
}

/// How many recent colours the panel keeps.
pub const RECENT: usize = 12;

// ----------------------------------------------------------------------------- maths

/// Painter's (RYB) wheel angle → RGB hue, both in degrees: red 0°, orange 60° (RGB 35°), yellow
/// 120° (RGB 60°), green 180° (RGB 120°), blue 240° (RGB 225°), violet 300° (RGB 275°).
const RYB_RGB: [(f32, f32); 7] = [(0.0, 0.0), (60.0, 35.0), (120.0, 60.0), (180.0, 120.0), (240.0, 225.0), (300.0, 275.0), (360.0, 360.0)];

fn piecewise(x: f32, table: &[(f32, f32)], inverse: bool) -> f32 {
    let x = if x.is_finite() { x.rem_euclid(360.0) } else { 0.0 };
    for w in table.windows(2) {
        let ((a0, b0), (a1, b1)) = if inverse { ((w[0].1, w[0].0), (w[1].1, w[1].0)) } else { (w[0], w[1]) };
        if x <= a1 {
            return b0 + (x - a0) / (a1 - a0) * (b1 - b0);
        }
    }
    x
}

/// The RGB hue (degrees) shown at `angle` (degrees) on a `kind` wheel.
pub fn wheel_to_hue(kind: WheelKind, angle: f32) -> f32 {
    match kind {
        WheelKind::Rgb | WheelKind::Cmyk => {
            if angle.is_finite() {
                angle.rem_euclid(360.0)
            } else {
                0.0
            }
        }
        WheelKind::Ryb => piecewise(angle, &RYB_RGB, false),
    }
}

/// Where RGB hue `hue` (degrees) sits on a `kind` wheel (degrees).
pub fn hue_to_wheel(kind: WheelKind, hue: f32) -> f32 {
    match kind {
        WheelKind::Rgb | WheelKind::Cmyk => {
            if hue.is_finite() {
                hue.rem_euclid(360.0)
            } else {
                0.0
            }
        }
        WheelKind::Ryb => piecewise(hue, &RYB_RGB, true),
    }
}

/// A point on the ring (`angle` in degrees, 0 at the top, clockwise) at `radius` from `c`.
pub fn ring_point(c: Pos2, radius: f32, angle: f32) -> Pos2 {
    let a = (angle - 90.0).to_radians();
    pos2(c.x + radius * a.cos(), c.y + radius * a.sin())
}

/// The ring angle (degrees, 0 at the top, clockwise) of `p` around `c`.
pub fn ring_angle(c: Pos2, p: Pos2) -> f32 {
    ((p.y - c.y).atan2(p.x - c.x).to_degrees() + 90.0).rem_euclid(360.0)
}

/// The triangle's corners for `angle`: pure hue (at the angle), white, black (120° further each).
pub fn triangle(c: Pos2, radius: f32, angle: f32) -> [Pos2; 3] {
    [ring_point(c, radius, angle), ring_point(c, radius, angle + 120.0), ring_point(c, radius, angle + 240.0)]
}

/// Saturation and value of a point in the triangle (clamped into it): weights `a` (pure hue),
/// `b` (white), `c` (black) give value `a + b` and saturation `a / (a + b)`.
pub fn triangle_sv(tri: [Pos2; 3], p: Pos2) -> (f32, f32) {
    let [pa, pb, pc] = tri;
    let det = (pb.y - pc.y) * (pa.x - pc.x) + (pc.x - pb.x) * (pa.y - pc.y);
    if det.abs() < 1e-6 || !det.is_finite() {
        return (0.0, 0.0);
    }
    let mut a = ((pb.y - pc.y) * (p.x - pc.x) + (pc.x - pb.x) * (p.y - pc.y)) / det;
    let mut b = ((pc.y - pa.y) * (p.x - pc.x) + (pa.x - pc.x) * (p.y - pc.y)) / det;
    // Clamp into the triangle and renormalise.
    a = a.max(0.0);
    b = b.max(0.0);
    let cc = (1.0 - a - b).max(0.0);
    let sum = a + b + cc;
    let (a, b) = if sum > 0.0 { (a / sum, b / sum) } else { (0.0, 0.0) };
    let v = (a + b).clamp(0.0, 1.0);
    let s = if v > 1e-6 { (a / v).clamp(0.0, 1.0) } else { 0.0 };
    (s, v)
}

/// The point of the triangle with saturation `s` and value `v`.
pub fn triangle_point(tri: [Pos2; 3], s: f32, v: f32) -> Pos2 {
    let (s, v) = (s.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
    let (a, b) = (s * v, v * (1.0 - s));
    let c = 1.0 - a - b;
    pos2(a * tri[0].x + b * tri[1].x + c * tri[2].x, a * tri[0].y + b * tri[1].y + c * tri[2].y)
}

/// `rgb` as the wheel shows and picks it: as it prints on a CMYK wheel (through the CMYK profile
/// in effect, see [`photocraft_color::convert::with_cmyk_space`]), else as it is.
pub fn shown(kind: WheelKind, rgb: [f32; 3]) -> [f32; 3] {
    use photocraft_color::convert::{cmyk_to_rgb, rgb_to_cmyk};
    let proof = |c: [f32; 3]| cmyk_to_rgb(rgb_to_cmyk(c)).map(|v| v.clamp(0.0, 1.0));
    match kind {
        // Twice: at the edge of the gamut (full cyan and magenta) one trip through the profile
        // still lands ~3 ΔE from where the next would, enough for the ⚠ to flag a colour the
        // wheel itself offered; the second trip settles it.
        WheelKind::Cmyk => proof(proof(rgb)),
        _ => rgb,
    }
}

fn srgb32(rgb: [f32; 3]) -> Color32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    Color32::from_rgb(q(rgb[0]), q(rgb[1]), q(rgb[2]))
}

/// Remember `c` as picked (newest first, no repeats, at most [`RECENT`]).
pub fn remember(recent: &mut Vec<[f32; 4]>, c: [f32; 4]) {
    let same = |a: &[f32; 4]| (0..3).all(|i| (a[i] - c[i]).abs() < 0.5 / 255.0);
    recent.retain(|x| !same(x));
    recent.insert(0, c);
    recent.truncate(RECENT);
}

// ----------------------------------------------------------------------------- panel

/// Hue kept for greys (their own hue is undefined), so the markers don't jump to red.
fn hue_key() -> egui::Id {
    egui::Id::new("color-wheel-hue")
}

fn set_color(app: &mut PhotocraftApp, c: [f32; 4]) {
    if app.ui.color_panel.background {
        app.session.tools.background = c;
    } else {
        app.session.tools.foreground = c;
        crate::type_tool::foreground_changed(app);
    }
}

/// The wheel's rectangle, and the ring's and the inner shape's interactive areas, as drawn last
/// frame (for tests and automation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WheelRects {
    pub center: Pos2,
    pub outer: f32,
    pub inner: f32,
    /// The square's rectangle (Square shape).
    pub square: Rect,
    /// The harmony swatches, current colour first.
    pub harmony: [Option<Rect>; 4],
}

pub fn last_rects(ctx: &egui::Context) -> Option<WheelRects> {
    ctx.data(|d| d.get_temp(egui::Id::new("color-wheel-rects")))
}

pub fn panel(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    // The CMYK wheel proofs through the active CMYK document's profile (the working CMYK
    // otherwise).
    let space = app.session.active().and_then(|st| photocraft_compose::cmyk_space(&st.doc));
    photocraft_color::convert::with_cmyk_space(space.as_ref(), || panel_in(app, ui));
}

/// The ink key for the CMYK fields: the values typed, kept while they still make the colour (a
/// round trip through the profile would otherwise move them as you type).
fn ink_key() -> egui::Id {
    egui::Id::new("color-wheel-inks")
}

fn panel_in(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let bg = app.ui.color_panel.background;
    let current = if bg { app.session.tools.background } else { app.session.tools.foreground };
    let [h0, s0, v0] = rgb_to_hsv([current[0], current[1], current[2]]);
    let remembered: Option<f32> = ui.data(|d| d.get_temp(hue_key()));
    let hue = if s0 < 0.005 || v0 < 0.005 { remembered.unwrap_or(h0) } else { h0 };
    let st = app.ui.color_wheel.clone();

    // Top row: the foreground / background chips (which one the wheel edits), the shape, the
    // wheel type and the harmony.
    let mut next = st.clone();
    ui.horizontal(|ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(34.0, 26.0), Sense::click());
        let fg_r = Rect::from_min_size(r.min, vec2(20.0, 20.0));
        let bg_r = Rect::from_min_size(r.min + vec2(12.0, 6.0), vec2(20.0, 20.0));
        let chip = |rect: Rect, c: [f32; 4], active: bool| {
            ui.painter().rect_filled(rect, 3.0, srgb32([c[0], c[1], c[2]]));
            ui.painter().rect_stroke(
                rect,
                3.0,
                Stroke::new(if active { 2.0 } else { 1.0 }, if active { t.accent } else { t.field_border }),
                egui::StrokeKind::Inside,
            );
        };
        // The edited one on top.
        if bg {
            chip(fg_r, app.session.tools.foreground, false);
            chip(bg_r, app.session.tools.background, true);
        } else {
            chip(bg_r, app.session.tools.background, false);
            chip(fg_r, app.session.tools.foreground, true);
        }
        if resp.on_hover_text(tl!("Click to edit the foreground or the background color")).clicked() {
            app.ui.color_panel.background = !bg;
        }
        ui.add_space(4.0);
        if crate::icons::button(ui, "square", 22.0, st.shape == WheelShape::Square, tl!("Square")).clicked() {
            next.shape = WheelShape::Square;
        }
        if crate::icons::button(ui, "triangle", 22.0, st.shape == WheelShape::Triangle, tl!("Triangle")).clicked() {
            next.shape = WheelShape::Triangle;
        }
        ui.add_space(4.0);
        let kinds = [(WheelKind::Rgb, "RGB"), (WheelKind::Ryb, "RYB"), (WheelKind::Cmyk, "CMYK")];
        widgets::dropdown(ui, "color-wheel-kind", &mut next.kind, &kinds, 70.0);
        let harmonies: Vec<(Harmony, &str)> = Harmony::ALL.iter().map(|h| (*h, tl!(h.label()))).collect();
        widgets::dropdown(ui, "color-wheel-harmony", &mut next.harmony, &harmonies, (ui.available_width() - 4.0).clamp(90.0, 170.0));
    });
    ui.add_space(6.0);

    // The wheel: as large as the panel allows above the harmony and recent rows.
    let rows = 32.0
        + if st.harmony == Harmony::None { 0.0 } else { 30.0 }
        + if st.recent.is_empty() { 0.0 } else { 30.0 }
        + if st.kind == WheelKind::Cmyk { 30.0 } else { 0.0 };
    let side = ui.available_width().min((ui.available_height() - rows).max(120.0)).max(120.0);
    let (area, resp) = ui.allocate_exact_size(vec2(ui.available_width(), side), Sense::click_and_drag());
    let c = area.center();
    let outer = side / 2.0 - 2.0;
    let inner = outer * 0.82;
    let painter = ui.painter_at(area);
    // Ring.
    let n = 120;
    let mut ring = Mesh::default();
    for k in 0..=n {
        let a = k as f32 / n as f32 * 360.0;
        let col = srgb32(shown(st.kind, hsv_to_rgb(wheel_to_hue(st.kind, a), 1.0, 1.0)));
        ring.colored_vertex(ring_point(c, inner, a), col);
        ring.colored_vertex(ring_point(c, outer, a), col);
    }
    for k in 0..n as u32 {
        let i = k * 2;
        ring.add_triangle(i, i + 1, i + 3);
        ring.add_triangle(i, i + 3, i + 2);
    }
    painter.add(ring);
    let angle = hue_to_wheel(st.kind, hue);
    // Inner shape.
    let gap = (outer - inner) * 0.35;
    let (square, tri) = match st.shape {
        WheelShape::Square => {
            let half = (inner - gap) / std::f32::consts::SQRT_2;
            let sq = Rect::from_center_size(c, vec2(half * 2.0, half * 2.0));
            let g = 16;
            let mut m = Mesh::default();
            for y in 0..=g {
                for x in 0..=g {
                    let (s, v) = (x as f32 / g as f32, 1.0 - y as f32 / g as f32);
                    m.colored_vertex(pos2(sq.left() + s * sq.width(), sq.top() + (1.0 - v) * sq.height()), srgb32(shown(st.kind, hsv_to_rgb(hue, s, v))));
                }
            }
            for y in 0..g as u32 {
                for x in 0..g as u32 {
                    let i = y * (g as u32 + 1) + x;
                    m.add_triangle(i, i + 1, i + g as u32 + 2);
                    m.add_triangle(i, i + g as u32 + 2, i + g as u32 + 1);
                }
            }
            painter.add(m);
            (sq, None)
        }
        WheelShape::Triangle => {
            let tri = triangle(c, inner - gap, angle);
            let g = 24u32;
            let mut m = Mesh::default();
            // Row i (pure-hue weight i/g) has g - i + 1 vertices; this is vertex (i, j)'s number.
            let at = |i: u32, j: u32| i * (g + 1) - i * i.saturating_sub(1) / 2 + j;
            for i in 0..=g {
                for j in 0..=(g - i) {
                    // Weights: i/g pure hue, j/g white, the rest black.
                    let (a, b) = (i as f32 / g as f32, j as f32 / g as f32);
                    let v = a + b;
                    let s = if v > 1e-6 { a / v } else { 0.0 };
                    let p = pos2(a * tri[0].x + b * tri[1].x + (1.0 - a - b) * tri[2].x, a * tri[0].y + b * tri[1].y + (1.0 - a - b) * tri[2].y);
                    m.colored_vertex(p, srgb32(shown(st.kind, hsv_to_rgb(hue, s, v.min(1.0)))));
                }
            }
            for i in 0..g {
                for j in 0..(g - i) {
                    m.add_triangle(at(i, j), at(i + 1, j), at(i, j + 1));
                    if j + 1 < g - i {
                        m.add_triangle(at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                    }
                }
            }
            painter.add(m);
            (Rect::NOTHING, Some(tri))
        }
    };

    // Interaction: a press on the ring drags the hue, inside it the saturation / value; the part
    // grabbed keeps the drag (egui memory).
    let grab_id = ui.id().with("color-wheel-grab");
    if resp.drag_started() || resp.clicked() {
        let p = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
        let on_ring = p.is_some_and(|p| (p.distance(c) - (inner + outer) / 2.0).abs() <= (outer - inner) / 2.0 + 4.0);
        ui.data_mut(|d| d.insert_temp(grab_id, on_ring));
    }
    let mut picked: Option<[f32; 4]> = None;
    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let on_ring: bool = ui.data(|d| d.get_temp(grab_id)).unwrap_or(false);
        let (mut h, mut s, mut v) = (hue, s0, v0);
        if on_ring {
            h = wheel_to_hue(st.kind, ring_angle(c, p));
            // A grey picks up the hue at full saturation, as on a wheel.
            if s < 0.005 {
                s = 1.0;
                v = v.max(0.5);
            }
        } else if let Some(tri) = tri {
            (s, v) = triangle_sv(tri, p);
        } else {
            s = ((p.x - square.left()) / square.width()).clamp(0.0, 1.0);
            v = 1.0 - ((p.y - square.top()) / square.height()).clamp(0.0, 1.0);
        }
        let rgb = shown(st.kind, hsv_to_rgb(h, s, v));
        let col = [rgb[0], rgb[1], rgb[2], 1.0];
        set_color(app, col);
        ui.data_mut(|d| d.insert_temp(hue_key(), h));
        picked = Some(col);
    }
    if (resp.drag_stopped() || resp.clicked())
        && let Some(col) = picked.or(Some(if app.ui.color_panel.background { app.session.tools.background } else { app.session.tools.foreground }))
    {
        remember(&mut next.recent, col);
    }

    // Markers: the hue on the ring, harmony hues, the colour in the shape.
    let shown_hue = picked.map_or(hue, |col| {
        let [h, s, v] = rgb_to_hsv([col[0], col[1], col[2]]);
        if s < 0.005 || v < 0.005 { ui.data(|d| d.get_temp(hue_key())).unwrap_or(h) } else { h }
    });
    let (cur_s, cur_v) = picked.map_or((s0, v0), |col| {
        let [_, s, v] = rgb_to_hsv([col[0], col[1], col[2]]);
        (s, v)
    });
    let a = hue_to_wheel(st.kind, shown_hue);
    let mid = (inner + outer) / 2.0;
    let knob = (outer - inner) / 2.0 + 1.0;
    for off in st.harmony.offsets() {
        let p = ring_point(c, mid, a + off);
        painter.circle_stroke(p, knob - 2.0, Stroke::new(1.5, Color32::from_white_alpha(220)));
        painter.circle_stroke(p, knob - 0.5, Stroke::new(1.0, Color32::from_black_alpha(150)));
    }
    let hp = ring_point(c, mid, a);
    painter.circle_stroke(hp, knob, Stroke::new(2.0, Color32::WHITE));
    painter.circle_stroke(hp, knob + 1.5, Stroke::new(1.0, Color32::from_black_alpha(180)));
    let sv = match st.shape {
        WheelShape::Square => pos2(square.left() + cur_s * square.width(), square.top() + (1.0 - cur_v) * square.height()),
        WheelShape::Triangle => triangle_point(triangle(c, inner - gap, a), cur_s, cur_v),
    };
    painter.circle_stroke(sv, 5.0, Stroke::new(1.5, Color32::WHITE));
    painter.circle_stroke(sv, 6.5, Stroke::new(1.0, Color32::from_black_alpha(160)));

    // Harmony swatches: the colour and its partners (same saturation and value); a click makes one
    // the edited colour.
    let mut harmony_rects = [None; 4];
    if st.harmony != Harmony::None {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let offsets = std::iter::once(0.0).chain(st.harmony.offsets().iter().copied());
            for (k, off) in offsets.enumerate().take(4) {
                let h = wheel_to_hue(st.kind, a + off);
                let rgb = shown(st.kind, hsv_to_rgb(h, cur_s, cur_v));
                let (r, sr) = ui.allocate_exact_size(vec2(36.0, 22.0), Sense::click());
                ui.painter().rect_filled(r, t.radius_sm, srgb32(rgb));
                ui.painter().rect_stroke(r, t.radius_sm, Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
                if let Some(slot) = harmony_rects.get_mut(k) {
                    *slot = Some(r);
                }
                if sr.on_hover_text(crate::color_picker_ui::hex(rgb)).clicked() {
                    let col = [rgb[0], rgb[1], rgb[2], 1.0];
                    set_color(app, col);
                    ui.data_mut(|d| d.insert_temp(hue_key(), h));
                    remember(&mut next.recent, col);
                }
            }
        });
    }
    // CMYK: the ink percentages (typed values set the colour), and ⚠ when it won't print.
    if st.kind == WheelKind::Cmyk {
        ui.add_space(6.0);
        let now = if app.ui.color_panel.background { app.session.tools.background } else { app.session.tools.foreground };
        let rgb = [now[0], now[1], now[2]];
        let kept: Option<([f32; 4], String)> = ui.data(|d| d.get_temp(ink_key()));
        let mut inks = kept.filter(|(_, hex)| *hex == crate::color_picker_ui::hex(rgb)).map_or_else(|| photocraft_color::convert::rgb_to_cmyk(rgb), |(k, _)| k);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let mut typed = false;
            for (i, name) in ["C", "M", "Y", "K"].into_iter().enumerate() {
                ui.add_space(if i == 0 { 0.0 } else { 3.0 });
                ui.label(egui::RichText::new(name).color(t.text_dim));
                let mut pct = (inks[i] * 100.0).round();
                if widgets::value_field(ui, &mut pct, 0.0..=100.0, "%", 52.0).changed() {
                    inks[i] = (pct / 100.0).clamp(0.0, 1.0);
                    typed = true;
                }
            }
            if typed {
                let rgb = photocraft_color::convert::cmyk_to_rgb(inks).map(|c| c.clamp(0.0, 1.0));
                set_color(app, [rgb[0], rgb[1], rgb[2], 1.0]);
                ui.data_mut(|d| d.insert_temp(ink_key(), (inks, crate::color_picker_ui::hex(rgb))));
            }
            if let Some(near) = crate::color_picker_ui::out_of_gamut(rgb) {
                let (r, resp) = ui.allocate_exact_size(vec2(30.0, 18.0), Sense::click());
                let icon = Rect::from_min_size(r.min + vec2(0.0, 2.0), vec2(13.0, 13.0));
                let chip = Rect::from_min_size(pos2(icon.right() + 4.0, r.top() + 4.0), vec2(10.0, 10.0));
                crate::color_picker_ui::warning_triangle(ui.painter(), icon, t.text, t.chrome);
                ui.painter().rect_filled(chip, 0.0, srgb32(near));
                ui.painter().rect_stroke(chip, 0.0, Stroke::new(1.0, t.text_dim), egui::StrokeKind::Outside);
                let tip = tl!("Out of gamut for printing: click for the nearest printable colour");
                if resp.on_hover_text(tip).clicked() {
                    let col = [near[0], near[1], near[2], 1.0];
                    set_color(app, col);
                    remember(&mut next.recent, col);
                }
            }
        });
    }
    // Recent colours.
    if !next.recent.is_empty() {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            for col in next.recent.clone() {
                let (r, sr) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                ui.painter().rect_filled(r, 3.0, srgb32([col[0], col[1], col[2]]));
                ui.painter().rect_stroke(r, 3.0, Stroke::new(1.0, t.field_border), egui::StrokeKind::Inside);
                if sr.on_hover_text(tl!("Recent Colors")).clicked() {
                    set_color(app, col);
                }
            }
        });
    }
    if next != app.ui.color_wheel {
        app.ui.color_wheel = next;
    }
    ui.data_mut(|d| d.insert_temp(egui::Id::new("color-wheel-rects"), WheelRects { center: c, outer, inner, square, harmony: harmony_rects }));
}

#[cfg(test)]
#[path = "color_wheel_tests.rs"]
mod tests;
