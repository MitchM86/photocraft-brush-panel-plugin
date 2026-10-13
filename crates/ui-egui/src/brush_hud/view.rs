//! Brush HUD drawing and interaction: a floating modal overlay centred on the image canvas
//! (PhotoCraft's menus, toolbar and panels stay in view around it) with its floating toolbar,
//! the wheel (master hub, hub orbit, the family orbit and the open family's variations on the
//! outer orbit), Edit HUD drag and drop with the bin, the search tracer and hub pulses. The parameter
//! panel is [`super::panel`]. Every change is collected as an [`Act`] while the frame paints and
//! applied afterwards, so painting only ever reads the app.

use egui::{Align2, Color32, FontId, Id, Order, Pos2, Rect, Sense, Shape, Stroke, Ui, UiBuilder, Vec2, pos2, vec2};
use photocraft_engine::paint::BrushPreset;

use super::Ring;
use super::geometry as g;
use super::model::{self, Library, fam_scope, var_scope};
use crate::PhotocraftApp;
use crate::theme::{self, Tokens};

/// What a wheel node is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Hub,
    Center,
    Recent,
    Family,
    Var,
    MoreFam,
    MoreVar,
}

/// A laid-out node, in screen points.
#[derive(Clone, Debug)]
struct Node {
    kind: Kind,
    /// Category name, family id or preset name.
    id: String,
    center: Pos2,
    r: f32,
    /// The order scope a drag can reorder it in.
    scope: Option<String>,
}

impl Node {
    fn key(&self) -> String {
        match self.kind {
            Kind::Hub => format!("hub:{}", self.id),
            Kind::Recent => format!("rec:{}", self.id),
            _ => self.id.clone(),
        }
    }
}

/// A change requested this frame.
#[derive(Clone, Debug, PartialEq)]
enum Act {
    Category(String),
    Family(String),
    Back,
    Pick(String),
    Hide(Kind, String),
    Move { scope: String, id: String, target: String },
    Favorite(String),
    PageFam(i32),
    PageVar(i32),
    Close,
    Edit,
    Undo,
    Redo,
    Restore,
    Ring,
    Search,
    Import,
}

/// HUD colours, all derived from the theme tokens.
#[derive(Clone, Copy)]
struct Pal {
    bg: Color32,
    disc: Color32,
    disc_hi: Color32,
    ring: Color32,
    ring_dim: Color32,
    bright: Color32,
    glow: Color32,
    text: Color32,
    dim: Color32,
    faint: Color32,
    danger: Color32,
    target: Color32,
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

fn palette(t: &Tokens) -> Pal {
    let night = mix(t.chrome, Color32::BLACK, 0.55);
    Pal {
        bg: mix(night, t.accent, 0.04),
        disc: mix(night, t.accent, 0.08),
        disc_hi: mix(night, t.accent, 0.16),
        ring: t.accent,
        ring_dim: mix(night, t.accent, 0.32),
        bright: mix(t.accent, Color32::WHITE, 0.35),
        glow: t.accent.gamma_multiply(0.22),
        text: t.text,
        dim: t.text_dim,
        faint: t.text_faint,
        danger: t.danger,
        target: mix(t.accent, Color32::WHITE, 0.6),
    }
}

fn category_icon(category: &str) -> &'static str {
    match category {
        "Sketch" => "pencil",
        "Paint" => "brush",
        "Atmosphere" => "cloud",
        "Flora" => "@leaf",
        "Surface" => "@waves",
        _ => "@cube",
    }
}

/// A category glyph: a bundled icon, or (for leaf, waves and cube, which the icon set lacks) a
/// few painted strokes.
fn glyph(ui: &Ui, center: Pos2, size: f32, category: &str, color: Color32) {
    let p = ui.painter();
    let s = size / 24.0;
    let at = |x: f32, y: f32| center + vec2((x - 12.0) * s, (y - 12.0) * s);
    let stroke = Stroke::new(1.8 * s.max(0.6), color);
    match category_icon(category) {
        "@leaf" => {
            let mut pts = Vec::new();
            for i in 0..=16 {
                let t = i as f32 / 16.0;
                pts.push(at(5.0 + 15.0 * t, 19.0 - 15.0 * t - 5.5 * (t * std::f32::consts::PI).sin()));
            }
            for i in 0..=16 {
                let t = 1.0 - i as f32 / 16.0;
                pts.push(at(5.0 + 15.0 * t, 19.0 - 15.0 * t + 5.5 * (t * std::f32::consts::PI).sin()));
            }
            p.add(Shape::closed_line(pts, stroke));
            p.line_segment([at(4.0, 20.0), at(15.0, 9.0)], stroke);
        }
        "@waves" => {
            for row in [7.0, 12.0, 17.0] {
                let pts: Vec<Pos2> =
                    (0..=24).map(|i| at(3.0 + 18.0 * i as f32 / 24.0, row + 1.6 * (i as f32 / 24.0 * std::f32::consts::TAU * 1.5).sin())).collect();
                p.add(Shape::line(pts, stroke));
            }
        }
        "@cube" => {
            let hex = [at(12.0, 3.0), at(20.0, 7.5), at(20.0, 16.5), at(12.0, 21.0), at(4.0, 16.5), at(4.0, 7.5)];
            p.add(Shape::closed_line(hex.to_vec(), stroke));
            p.line_segment([at(4.0, 7.5), at(12.0, 12.0)], stroke);
            p.line_segment([at(20.0, 7.5), at(12.0, 12.0)], stroke);
            p.line_segment([at(12.0, 12.0), at(12.0, 21.0)], stroke);
        }
        name => crate::icons::paint(ui, Rect::from_center_size(center, Vec2::splat(size)), name, size, color),
    }
}

/// The tip thumbnail of a preset.
fn tip(ui: &Ui, p: &BrushPreset, rect: Rect, color: Color32) {
    let b = &p.brush;
    let tex = crate::brush_preview::tip_texture(ui.ctx(), &format!("brush-hud-tip:{}", p.name), &b.tip, (b.hardness, b.angle, b.roundness), 48, color);
    ui.painter().image(tex.id(), rect, crate::brush_panel::full_uv(), Color32::WHITE);
}

fn page<T: Clone>(list: &[T], index: usize, max: usize) -> (Vec<T>, usize, usize, usize) {
    if list.len() <= max {
        return (list.to_vec(), 0, 1, 0);
    }
    let per = max.saturating_sub(1).max(1);
    let pages = list.len().div_ceil(per);
    let p = index % pages.max(1);
    let items: Vec<T> = list.iter().skip(p * per).take(per).cloned().collect();
    let more = list.len() - items.len();
    (items, more, pages, p)
}

/// The wheel's diameter in its square stage: the outer ring encloses every label.
fn wheel_size(stage: Rect) -> f32 {
    stage.width().min(stage.height()).max(240.0)
}

/// Where the Edit HUD bin was drawn last frame (screen points).
#[cfg(test)]
pub(super) fn bin_center(ctx: &egui::Context) -> Option<Pos2> {
    ctx.data(|d| d.get_temp::<Pos2>(hud_id().with("bin")))
}

fn hud_id() -> Id {
    Id::new("brush-hud")
}

pub fn show(app: &mut PhotocraftApp, ctx: &egui::Context) {
    let lib = super::library(ctx, &app.session.tools.presets);
    let screen = ctx.content_rect();
    // The HUD floats over the image canvas, not the whole window.
    let canvas = app.last_canvas_rect.intersect(screen);
    let view = if canvas.width() >= 320.0 && canvas.height() >= 320.0 { canvas } else { screen };
    let t = Tokens::get(ctx);
    let pal = palette(&t);
    let now = ctx.input(|i| i.time);
    let mut acts: Vec<Act> = Vec::new();
    egui::Area::new(hud_id()).order(Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
        ui.set_min_size(screen.size());
        // Layout inside the canvas: a floating toolbar, the wheel, and the parameter card beside
        // it, centred as a group.
        let area = view.shrink(if view.width() > 700.0 { 16.0 } else { 8.0 });
        let bar_h = 40.0;
        let hint_h = 20.0;
        let content = Rect::from_min_max(pos2(area.left(), area.top() + bar_h + 10.0), pos2(area.right(), area.bottom() - hint_h));
        let gap = 18.0;
        let mut panel_w = if content.width() >= 760.0 { (content.width() * 0.3).clamp(290.0, 360.0) } else { 0.0 };
        let mut ws = content.height().min(content.width() - if panel_w > 0.0 { panel_w + gap } else { 0.0 });
        if panel_w > 0.0 && ws < 300.0 {
            panel_w = 0.0;
            ws = content.height().min(content.width());
        }
        // The wheel takes 80% of the room it has.
        let ws = (ws * 0.8).max(240.0);
        let group_w = ws + if panel_w > 0.0 { gap + panel_w } else { 0.0 };
        let left = content.center().x - group_w / 2.0;
        let stage = Rect::from_min_size(pos2(left, content.center().y - ws / 2.0), Vec2::splat(ws));
        let panel_h = content.height().min(ws.max(560.0));
        let panel = Rect::from_min_size(pos2(stage.right() + gap, content.center().y - panel_h / 2.0), vec2(panel_w, panel_h));
        let bar = Rect::from_min_max(pos2(left, area.top()), pos2(left + group_w, area.top() + bar_h));

        // Modal: the window outside the HUD takes no input while it is open, and a click there
        // dismisses it. Only the canvas is dimmed; the rest of PhotoCraft stays as it is.
        let scrim = ui.interact(screen, hud_id().with("scrim"), Sense::click());
        ui.painter().rect_filled(view, 0.0, t.scrim.gamma_multiply(0.9));
        let wheel_r = ws / 2.0;
        let on_hud = |p: Pos2| bar.contains(p) || (panel_w > 0.0 && panel.contains(p)) || (p - stage.center()).length() <= wheel_r;
        if (scrim.clicked() || scrim.secondary_clicked()) && scrim.interact_pointer_pos().is_none_or(|p| !on_hud(p)) {
            acts.push(Act::Close);
        }
        // The wheel's own translucent disc, the canvas faintly showing through.
        ui.painter().circle_filled(stage.center(), wheel_r, pal.bg.gamma_multiply(0.9));

        let search_rect = top_bar(app, ui, bar, stage.center().x, &lib, &pal, &t, &mut acts);
        let nodes = wheel(app, ui, stage, &lib, &pal, &t, now, &mut acts);
        if panel_w > 0.0 {
            super::panel::show(app, ui, panel, &lib, &pal_panel(&pal));
        }
        if app.ui.brush_hud.edit {
            ui.painter().text(
                pos2(stage.center().x, content.bottom() + hint_h / 2.0),
                Align2::CENTER_CENTER,
                tl!("●  EDIT MODE · DRAG TO REARRANGE · DRAG TO BIN OR RIGHT-CLICK TO REMOVE"),
                FontId::proportional(10.5),
                pal.danger,
            );
        }
        tracer(app, ui, search_rect, &nodes, stage, &pal, now);
    });
    apply(app, &lib, acts, now);
}

/// The few colours the panel shares with the wheel.
pub(super) struct PanelPal {
    pub card: Color32,
    pub ring: Color32,
    pub bright: Color32,
    pub dim: Color32,
}

fn pal_panel(p: &Pal) -> PanelPal {
    PanelPal { card: p.bg.gamma_multiply(0.92), ring: mix(p.ring_dim, p.ring, 0.6), bright: p.bright, dim: p.dim }
}

// ------------------------------------------------------------------ top bar

#[allow(clippy::too_many_arguments)]
fn top_bar(app: &mut PhotocraftApp, ui: &mut Ui, r: Rect, wheel_x: f32, lib: &Library, pal: &Pal, t: &Tokens, acts: &mut Vec<Act>) -> Rect {
    let p = ui.painter().clone();
    // Search field with its breadcrumb chip.
    // The search field centred over the wheel, the Edit HUD switch and the icon buttons on the
    // right.
    let controls_left = r.right() - (146.0 + 6.0 * 34.0 + 10.0) - 12.0;
    let half = (controls_left - wheel_x).min(wheel_x - r.left()).clamp(90.0, 220.0);
    let search = Rect::from_min_max(pos2(wheel_x - half, r.center().y - 17.0), pos2((wheel_x + half).min(controls_left), r.center().y + 17.0));
    p.rect_filled(search, 17.0, pal.bg.gamma_multiply(0.92));
    p.rect_filled(search, 17.0, pal.disc);
    p.rect_stroke(search, 17.0, Stroke::new(1.2, pal.ring), egui::StrokeKind::Inside);
    crate::icons::paint(ui, Rect::from_center_size(pos2(search.left() + 18.0, search.center().y), Vec2::splat(16.0)), "search", 16.0, pal.text);
    let crumb = {
        let st = &app.ui.brush_hud;
        st.target
            .as_ref()
            .and_then(|n| lib.entry(n))
            .map(|e| format!("{} › {}", e.category, e.family))
            .or_else(|| (!st.query.trim().is_empty()).then(|| tl!("No match").to_string()))
    };
    let crumb_w = crumb.as_ref().map_or(0.0, |c| (c.chars().count() as f32 * 6.4 + 22.0).min(150.0));
    let field = Rect::from_min_max(pos2(search.left() + 32.0, search.top() + 4.0), pos2(search.right() - 10.0 - crumb_w, search.bottom() - 4.0));
    let st = &mut app.ui.brush_hud;
    let resp = ui
        .scope_builder(UiBuilder::new().max_rect(field), |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut st.query)
                    .id(hud_id().with("search"))
                    .hint_text(tl!("Search…"))
                    .frame(egui::Frame::NONE)
                    .desired_width(field.width())
                    .text_color(pal.text),
            )
        })
        .inner;
    if std::mem::take(&mut st.focus_search) {
        resp.request_focus();
    }
    if resp.changed() {
        acts.push(Act::Search);
    }
    if let Some(c) = crumb {
        let cr = Rect::from_min_max(pos2(search.right() - 6.0 - crumb_w, search.top() + 6.0), pos2(search.right() - 6.0, search.bottom() - 6.0));
        p.rect_stroke(cr, 10.0, Stroke::new(1.0, if st.target.is_some() { pal.ring } else { pal.danger }), egui::StrokeKind::Inside);
        p.text(cr.center(), Align2::CENTER_CENTER, c, FontId::proportional(10.5), pal.text);
    }
    // Search results under the field while it has focus.
    if resp.has_focus() && !st.query.trim().is_empty() {
        let hits = lib.search(&st.query, &app.session.tools.presets, &st.layout, 8);
        if !hits.is_empty() {
            egui::Area::new(hud_id().with("results")).order(Order::Tooltip).fixed_pos(pos2(search.left(), search.bottom() + 4.0)).show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(search.width() - 12.0);
                    for &i in &hits {
                        let Some(e) = lib.entries.get(i) else { continue };
                        let row = ui.add(egui::Button::new(format!("{}   ·   {} › {}", e.name, e.category, e.family)).frame(false));
                        if row.clicked() {
                            acts.push(Act::Pick(e.name.clone()));
                        }
                    }
                });
            });
        }
    }
    // Right side: Edit HUD switch, then restore, undo/redo, ring mode, Import Brushes and close
    // on one floating strip.
    let strip = Rect::from_min_max(pos2(r.right() - 6.0 * 34.0 - 10.0, r.center().y - 18.0), pos2(r.right(), r.center().y + 18.0));
    p.rect(strip, 18.0, pal.bg.gamma_multiply(0.92), Stroke::new(1.0, pal.ring_dim), egui::StrokeKind::Inside);
    let mut x = r.right() - 6.0;
    let mut button = |ui: &mut Ui, icon: &str, tip: &str, on: bool, enabled: bool| -> bool {
        let rect = Rect::from_center_size(pos2(x - 15.0, r.center().y), Vec2::splat(30.0));
        x -= 34.0;
        let resp = ui.interact(rect, hud_id().with(("top", icon)), if enabled { Sense::click() } else { Sense::hover() }).on_hover_text(tl!(tip));
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, tl!(tip)));
        if on || resp.hovered() {
            ui.painter().rect_filled(rect, t.radius_sm, if on { t.accent_soft } else { t.hover });
        }
        crate::icons::paint(ui, rect, icon, 18.0, if enabled { pal.text } else { pal.faint });
        enabled && resp.clicked()
    };
    if button(ui, "x", "Close (Esc)", false, true) {
        acts.push(Act::Close);
    }
    if button(ui, "folder-open", "Import Brushes… (.abr)", false, true) {
        acts.push(Act::Import);
    }
    if button(ui, if st.ring == Ring::Favorites { "star-filled" } else { "clock" }, "Recent / favorite brushes", st.ring == Ring::Favorites, true) {
        acts.push(Act::Ring);
    }
    if button(ui, "redo-2", "Redo layout change (Ctrl+Shift+Z)", false, !st.redo.is_empty()) {
        acts.push(Act::Redo);
    }
    if button(ui, "undo-2", "Undo layout change (Ctrl+Z)", false, !st.undo.is_empty()) {
        acts.push(Act::Undo);
    }
    if button(ui, "rotate-cw", "Put every removed brush back on the HUD", false, !st.layout.hidden.is_empty()) {
        acts.push(Act::Restore);
    }
    // Edit HUD: a pill with a switch.
    let pill = Rect::from_min_max(pos2(x - 146.0, r.center().y - 17.0), pos2(x - 14.0, r.center().y + 17.0));
    let pr =
        ui.interact(pill, hud_id().with("edit"), Sense::click()).on_hover_text(tl!("Edit HUD: drag to reorder, drop on the bin or right-click to remove (E)"));
    pr.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, st.edit, tl!("Edit HUD")));
    p.rect_filled(pill, 17.0, if pr.hovered() { pal.disc_hi } else { pal.disc });
    p.rect_stroke(pill, 17.0, Stroke::new(1.2, pal.ring), egui::StrokeKind::Inside);
    crate::icons::paint(ui, Rect::from_center_size(pos2(pill.left() + 16.0, pill.center().y), Vec2::splat(16.0)), "move", 16.0, pal.text);
    p.text(pos2(pill.left() + 30.0, pill.center().y), Align2::LEFT_CENTER, tl!("Edit HUD"), FontId::proportional(12.5), pal.text);
    let sw_r = Rect::from_center_size(pos2(pill.right() - 22.0, pill.center().y), vec2(28.0, 15.0));
    p.rect_filled(sw_r, 7.5, if st.edit { t.accent } else { t.field_border });
    let knob = if st.edit { sw_r.right() - 7.5 } else { sw_r.left() + 7.5 };
    p.circle_filled(pos2(knob, sw_r.center().y), 5.5, Color32::WHITE);
    if pr.clicked() {
        acts.push(Act::Edit);
    }
    search
}

// ------------------------------------------------------------------ the wheel

#[allow(clippy::too_many_arguments)]
fn wheel(app: &mut PhotocraftApp, ui: &mut Ui, stage: Rect, lib: &Library, pal: &Pal, t: &Tokens, now: f64, acts: &mut Vec<Act>) -> Vec<Node> {
    let size = wheel_size(stage);
    let s = size / g::U;
    let c = stage.center();
    let at = |deg: f32, r: f32| c + g::polar(deg, r) * s;
    let ctx = ui.ctx().clone();
    let presets = &app.session.tools.presets;
    let st = &app.ui.brush_hud;
    let current = app.session.tools.current_preset.as_deref();
    let preset = |name: &str| lib.entry(name).and_then(|e| presets.get(e.index));

    // A drag that ended outside any frame of the HUD (it closed mid-drag) leaves nothing behind.
    if !ctx.input(|i| i.pointer.any_down()) {
        ctx.data_mut(|d| d.remove::<String>(hud_id().with("drag")));
    }
    // Background right-clicks on the empty stage close the HUD (it opened the same way).
    let stage_resp = ui.interact(stage, hud_id().with("stage"), Sense::click());
    if stage_resp.secondary_clicked() {
        acts.push(Act::Close);
    }

    // ---------------------------------------------------------- layout
    let cats = lib.categories(&st.layout);
    let angles = g::hub_angles(cats.len());
    let active = cats.iter().position(|(n, _)| *n == st.category).unwrap_or(0);
    let active_cat = cats.get(active).map_or("Paint", |(n, _)| n.as_str()).to_string();
    let fams = lib.families(&active_cat, &st.layout);
    // The outer orbit holds the active hub's families, or the open family's variations.
    let open_fam = st.family.as_ref().and_then(|id| fams.iter().find(|f| &f.id == id)).cloned();
    let ring_names: &Vec<String> = if st.ring == Ring::Favorites { &st.favorites } else { &st.recent };
    let ring: Vec<String> = ring_names
        .iter()
        .filter(|n| lib.entry(n).is_some_and(|e| !st.layout.hidden.contains(&e.name) && !st.layout.hidden.contains(&e.family_id)))
        .take(g::RECENT_MAX)
        .cloned()
        .collect();

    let mut nodes: Vec<Node> = Vec::new();
    for (i, (name, _)) in cats.iter().enumerate() {
        let deg = angles.get(i).copied().unwrap_or(0.0);
        nodes.push(Node { kind: Kind::Hub, id: name.clone(), center: at(deg, g::R_HUB), r: g::HUB_R * s, scope: Some("cat".into()) });
    }
    nodes.push(Node { kind: Kind::Center, id: "center".into(), center: c, r: g::R_CENTER * s, scope: None });
    for (n, slot) in ring.iter().zip(g::recent_arc(ring.len())) {
        nodes.push(Node { kind: Kind::Recent, id: n.clone(), center: c + slot.offset() * s, r: g::RECENT_R * s, scope: None });
    }
    let node_r = g::NODE_R * s;
    // The families of the active hub on their orbit, and the open family's variations on the
    // outer orbit.
    // Each orbit gathers by what it belongs to: the families by their hub, the variations by
    // their family.
    let hub_deg = angles.get(active).copied().unwrap_or(0.0);
    let (shown, fam_more, _, _) = page(&fams, st.fam_page, g::FAM_MAX);
    let slots = g::orbit(shown.len() + usize::from(fam_more > 0), g::R_FAM, hub_deg);
    let fam_head = g::header_deg(&slots, hub_deg);
    let fam_deg = open_fam.as_ref().and_then(|o| shown.iter().position(|f| f.id == o.id)).and_then(|i| slots.get(i)).map_or(hub_deg, |s| s.deg);
    for (f, slot) in shown.iter().zip(&slots) {
        nodes.push(Node { kind: Kind::Family, id: f.id.clone(), center: c + slot.offset() * s, r: node_r, scope: Some(fam_scope(&active_cat)) });
    }
    if let (true, Some(slot)) = (fam_more > 0, slots.get(shown.len())) {
        nodes.push(Node { kind: Kind::MoreFam, id: "more:fam".into(), center: c + slot.offset() * s, r: node_r, scope: None });
    }
    let mut var_more = 0;
    let mut var_head = fam_deg;
    if let Some(f) = &open_fam {
        let (vars, more, _, _) = page(&f.members, st.var_page, g::VAR_MAX);
        let slots = g::orbit(vars.len() + usize::from(more > 0), g::R_VAR, fam_deg);
        var_head = g::header_deg(&slots, fam_deg);
        for (&m, slot) in vars.iter().zip(&slots) {
            if let Some(e) = lib.entries.get(m) {
                nodes.push(Node { kind: Kind::Var, id: e.name.clone(), center: c + slot.offset() * s, r: node_r, scope: Some(var_scope(&f.id)) });
            }
        }
        if let (true, Some(slot)) = (more > 0, slots.get(vars.len())) {
            nodes.push(Node { kind: Kind::MoreVar, id: "more:var".into(), center: c + slot.offset() * s, r: node_r, scope: None });
        }
        var_more = more;
    }

    // ---------------------------------------------------------- interaction
    let draggable = |k: Kind| matches!(k, Kind::Hub | Kind::Family | Kind::Var | Kind::Recent);
    let mut hovered: Option<usize> = None;
    let mut hover_preset: Option<String> = None;
    let pointer = ctx.pointer_interact_pos();
    let bin = Rect::from_center_size(pos2(stage.right() - 52.0, stage.bottom() - 58.0), Vec2::splat(62.0));
    ctx.data_mut(|d| d.insert_temp(hud_id().with("bin"), bin.center()));
    let mut drop_target: Option<usize> = None;
    let mut over_bin = false;
    for (i, n) in nodes.iter().enumerate() {
        let rect = Rect::from_center_size(n.center, Vec2::splat(n.r * 2.0));
        let sense = if st.edit && draggable(n.kind) { Sense::click_and_drag() } else { Sense::click() };
        let resp = ui.interact(rect, hud_id().with(("node", n.key())), sense);
        let label = match n.kind {
            Kind::Family => lib.family(&n.id, &st.layout).map_or_else(|| n.id.clone(), |f| f.name),
            Kind::Center => tl!("Master Tool").to_string(),
            Kind::MoreFam | Kind::MoreVar => tl!("More").to_string(),
            _ => n.id.clone(),
        };
        let selected = matches!(n.kind, Kind::Var | Kind::Recent) && current == Some(n.id.as_str()) || (n.kind == Kind::Hub && n.id == active_cat);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, &label));
        if resp.hovered() {
            hovered = Some(i);
        }
        if st.edit && draggable(n.kind) {
            // The ✕ badge removes the node from the HUD.
            let badge = Rect::from_center_size(n.center + vec2(n.r * 0.72, -n.r * 0.72), Vec2::splat((n.r * 0.55).clamp(12.0, 18.0)));
            if !matches!(n.kind, Kind::Hub) && ui.interact(badge, hud_id().with(("x", n.key())), Sense::click()).clicked() {
                acts.push(Act::Hide(n.kind, n.id.clone()));
                continue;
            }
            if resp.drag_started() {
                app_drag_start(&ctx, n);
            }
            if resp.drag_stopped() {
                let (target, bin_hit) = drop_at(&nodes, n, pointer, bin);
                if bin_hit {
                    acts.push(Act::Hide(n.kind, n.id.clone()));
                } else if let (Some(scope), Some(ti)) = (&n.scope, target)
                    && let Some(tn) = nodes.get(ti)
                {
                    acts.push(Act::Move { scope: scope.clone(), id: n.id.clone(), target: tn.id.clone() });
                }
                ctx.data_mut(|d| d.remove::<String>(hud_id().with("drag")));
            }
        }
        if resp.clicked() {
            match n.kind {
                Kind::Hub => acts.push(Act::Category(n.id.clone())),
                Kind::Family => acts.push(Act::Family(n.id.clone())),
                Kind::Var | Kind::Recent => acts.push(Act::Pick(n.id.clone())),
                Kind::Center => acts.push(Act::Close),
                Kind::MoreFam => acts.push(Act::PageFam(1)),
                Kind::MoreVar => acts.push(Act::PageVar(1)),
            }
        }
        if resp.secondary_clicked() {
            if st.edit && draggable(n.kind) && n.kind != Kind::Hub {
                acts.push(Act::Hide(n.kind, n.id.clone()));
            } else if matches!(n.kind, Kind::Var | Kind::Recent) {
                acts.push(Act::Favorite(n.id.clone()));
            }
        }
        if resp.hovered() {
            let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.5 {
                match n.kind {
                    Kind::Var | Kind::MoreVar => acts.push(Act::PageVar(if scroll < 0.0 { 1 } else { -1 })),
                    Kind::Family | Kind::MoreFam => acts.push(Act::PageFam(if scroll < 0.0 { 1 } else { -1 })),
                    _ => {}
                }
            }
        }
    }
    let drag_key = ctx.data(|d| d.get_temp::<String>(hud_id().with("drag")));
    let drag_node = drag_key.as_ref().and_then(|k| nodes.iter().position(|n| &n.key() == k));
    if let Some(di) = drag_node
        && let Some(dn) = nodes.get(di)
    {
        let (tgt, b) = drop_at(&nodes, dn, pointer, bin);
        drop_target = tgt;
        over_bin = b;
    }
    if let Some(i) = hovered
        && drag_node.is_none()
        && let Some(n) = nodes.get(i)
    {
        hover_preset = match n.kind {
            Kind::Var | Kind::Recent => Some(n.id.clone()),
            Kind::Family => lib.family(&n.id, &st.layout).and_then(|f| f.members.first().and_then(|&m| lib.entries.get(m)).map(|e| e.name.clone())),
            _ => None,
        };
    }

    // ---------------------------------------------------------- paint
    // Concentric bands divided by glowing rings: the centre, the hub orbit, the outer orbit and
    // the frame. Each band is shaded a step lighter outwards; the nodes' tracks are faint.
    let p = ui.painter().clone();
    let ring_at = |r: f32, w: f32, col: Color32| p.circle_stroke(c, r * s, Stroke::new(w, col));
    // A glowing line: a soft halo of widening, fading strokes under a bright core.
    let glow_ring = |r: f32, strength: f32| {
        for (w, a) in [(22.0, 0.05), (13.0, 0.09), (7.0, 0.16), (3.5, 0.32)] {
            ring_at(r, w * s.max(0.55), pal.ring.gamma_multiply(a * strength));
        }
        ring_at(r, 1.6, mix(pal.ring, Color32::WHITE, 0.25 * strength).gamma_multiply(0.95));
    };
    p.circle_filled(c, g::RING_OUTER * s, pal.disc.gamma_multiply(0.55));
    p.circle_filled(c, g::RING_FAM * s, mix(pal.bg, pal.ring, 0.07).gamma_multiply(0.9));
    p.circle_filled(c, g::RING_MID * s, mix(pal.bg, pal.ring, 0.035).gamma_multiply(0.92));
    p.circle_filled(c, g::RING_INNER * s, pal.bg.gamma_multiply(0.95));
    ring_at(g::R_VAR, 1.0, pal.ring_dim.gamma_multiply(0.8));
    ring_at(g::R_FAM, 1.0, pal.ring_dim.gamma_multiply(0.8));
    ring_at(g::R_HUB, 1.0, pal.ring_dim.gamma_multiply(0.8));
    glow_ring(g::RING_INNER, 0.8);
    glow_ring(g::RING_MID, 1.0);
    glow_ring(g::RING_FAM, 1.0);
    glow_ring(g::RING_OUTER, 1.25);
    // Markers on the frame: diamonds at the quarters, small dots between.
    for k in 0..16 {
        let a = k as f32 * 22.5;
        let m = at(a, g::RING_OUTER);
        if k % 4 == 0 {
            let d = 4.5;
            p.add(Shape::convex_polygon(vec![m + vec2(0.0, -d), m + vec2(d, 0.0), m + vec2(0.0, d), m + vec2(-d, 0.0)], pal.target, Stroke::NONE));
            p.circle_filled(m, 9.0, pal.ring.gamma_multiply(0.18));
        } else {
            p.circle_filled(m, 1.8, pal.bright.gamma_multiply(0.7));
        }
    }

    // Nodes.
    let pulse = ((now % 1.5) / 1.5) as f32;
    let target_cats: Vec<&str> = if st.query.trim().is_empty() {
        Vec::new()
    } else {
        lib.search(&st.query, presets, &st.layout, 200).into_iter().filter_map(|i| lib.entries.get(i).map(|e| e.category)).collect()
    };
    let target_cat = st.target.as_ref().and_then(|n| lib.entry(n)).map(|e| e.category);
    let label_font = FontId::proportional(11.0 * (s / 0.52).clamp(0.9, 1.15));
    // The brush in hand pings: a ring that grows out of its node and fades, every 1.5 s (by
    // `spread` of the node's radius).
    let ping = |center: Pos2, r: f32, spread: f32| {
        let k = 1.0 + spread * pulse;
        let fade = 1.0 - pulse;
        p.circle_stroke(center, r * k, Stroke::new(12.0 * s.max(0.6), pal.ring.gamma_multiply(0.35 * fade)));
        p.circle_stroke(center, r * k, Stroke::new(3.0, mix(pal.target, Color32::WHITE, 0.4).gamma_multiply(fade)));
        ctx.request_repaint();
    };
    for (i, n) in nodes.iter().enumerate() {
        let hot = hovered == Some(i) && drag_node.is_none();
        let is_drag = drag_node == Some(i);
        let is_drop = drop_target == Some(i);
        if is_drag {
            p.extend(Shape::dashed_line(
                &g::arc_points(n.r / s, 0.0, 360.0).into_iter().map(|v| n.center + v * s).collect::<Vec<_>>(),
                Stroke::new(1.5, pal.ring),
                5.0,
                5.0,
            ));
            continue;
        }
        match n.kind {
            Kind::Hub => {
                let active = n.id == active_cat;
                let matched = target_cats.contains(&n.id.as_str());
                if matched {
                    let strong = target_cat == Some(n.id.as_str());
                    let k = 1.0 + (if strong { 0.32 } else { 0.18 }) * pulse;
                    p.circle_stroke(n.center, n.r * k, Stroke::new(3.0, pal.target.gamma_multiply((if strong { 0.9 } else { 0.5 }) * (1.0 - pulse))));
                    ctx.request_repaint();
                }
                let strength = if active {
                    1.6
                } else if hot {
                    1.2
                } else {
                    0.7
                };
                for (w, a) in [(16.0, 0.08), (8.0, 0.16), (4.0, 0.3)] {
                    p.circle_stroke(n.center, n.r, Stroke::new(w * s.max(0.6), pal.ring.gamma_multiply(a * strength)));
                }
                let edge = if active || hot { mix(pal.ring, Color32::WHITE, 0.35) } else { pal.ring };
                p.circle(n.center, n.r, if active { pal.disc_hi } else { pal.disc }, Stroke::new(if active { 3.0 } else { 2.0 }, edge));
                if is_drop {
                    p.circle_stroke(n.center, n.r + 4.0, Stroke::new(3.0, t.warning));
                }
                let ink = if active || hot { Color32::WHITE } else { pal.text };
                glyph(ui, n.center - vec2(0.0, n.r * 0.2), n.r * 0.5, &n.id, ink);
                p.text(n.center + vec2(0.0, n.r * 0.36), Align2::CENTER_CENTER, tl!(n.id.as_str()), theme::semibold((n.r * 0.26).clamp(10.0, 17.0)), ink);
            }
            Kind::Center => {
                for (w, a) in [(30.0, 0.07), (18.0, 0.14), (9.0, 0.26), (4.5, 0.45)] {
                    p.circle_stroke(c, n.r, Stroke::new(w * s.max(0.6), pal.ring.gamma_multiply(a)));
                }
                p.circle(c, n.r, pal.disc_hi, Stroke::new(3.0, if hot { Color32::WHITE } else { mix(pal.ring, Color32::WHITE, 0.45) }));
                // The master hub is the brush in hand, preset or not: its ping rides over its glow.
                ping(c, n.r, 0.4);
                let cur = current.and_then(|name| lib.entry(name));
                glyph(ui, c - vec2(0.0, n.r * 0.32), n.r * 0.36, cur.map_or("Paint", |e| e.category), Color32::WHITE);
                let label = current.unwrap_or("Brush");
                let font = theme::semibold((n.r * 0.19).clamp(10.0, 16.0));
                let galley = p.layout(label.to_string(), font, Color32::WHITE, n.r * 1.6);
                p.galley(c + vec2(-galley.size().x / 2.0, -n.r * 0.02), galley, Color32::WHITE);
                p.text(c + vec2(0.0, n.r * 0.5), Align2::CENTER_CENTER, tl!("Master Tool"), FontId::proportional((n.r * 0.14).clamp(9.0, 12.0)), pal.dim);
            }
            Kind::Recent => {
                if current == Some(n.id.as_str()) {
                    ping(n.center, n.r, 0.45);
                }
                p.circle(n.center, n.r, pal.disc_hi, Stroke::new(if hot { 2.0 } else { 1.2 }, if hot { pal.bright } else { pal.ring_dim }));
                if let Some(pr) = preset(&n.id) {
                    tip(ui, pr, Rect::from_center_size(n.center, Vec2::splat(n.r * 1.5)), pal.text);
                }
            }
            Kind::Family | Kind::Var => {
                let selected = n.kind == Kind::Var && current == Some(n.id.as_str());
                let targeted = (n.kind == Kind::Var && st.target.as_deref() == Some(n.id.as_str()))
                    || (n.kind == Kind::Family && st.target.as_ref().and_then(|t2| lib.entry(t2)).is_some_and(|e| e.family_id == n.id));
                let opened = n.kind == Kind::Family && st.family.as_deref() == Some(n.id.as_str());
                let lit = hot || selected || targeted || opened;
                if selected {
                    ping(n.center, n.r, 0.45);
                }
                if lit {
                    p.circle_stroke(
                        n.center,
                        n.r + 4.0 * s.max(0.6),
                        Stroke::new(10.0 * s.max(0.6), if targeted && !hot { pal.target.gamma_multiply(0.45) } else { pal.glow.gamma_multiply(1.8) }),
                    );
                }
                let edge = if selected || opened {
                    Color32::WHITE
                } else if hot {
                    pal.bright
                } else if targeted {
                    pal.target
                } else {
                    pal.ring.gamma_multiply(0.7)
                };
                p.circle(n.center, n.r, pal.disc, Stroke::new(if lit { 2.5 } else { 1.5 }, edge));
                if is_drop {
                    p.circle_stroke(n.center, n.r + 4.0, Stroke::new(3.0, t.warning));
                }
                let rep = if n.kind == Kind::Var {
                    preset(&n.id)
                } else {
                    lib.family(&n.id, &st.layout).and_then(|f| f.members.first().and_then(|&m| lib.entries.get(m)).and_then(|e| presets.get(e.index)))
                };
                if let Some(pr) = rep {
                    tip(ui, pr, Rect::from_center_size(n.center, Vec2::splat(n.r * 1.4)), pal.text);
                }
                let text = if n.kind == Kind::Family { lib.family(&n.id, &st.layout).map_or_else(|| n.id.clone(), |f| f.name) } else { n.id.clone() };
                let col = if lit { Color32::WHITE } else { pal.text };
                let max = if n.kind == Kind::Family { 14 } else { 18 };
                let (at_label, align) = label_spot(n, c);
                p.text(at_label, align, elide(&text, max), label_font.clone(), col);
                if st.edit {
                    let bc = n.center + vec2(n.r * 0.72, -n.r * 0.72);
                    let br = (n.r * 0.55).clamp(12.0, 18.0) / 2.0;
                    p.circle(bc, br, pal.danger, Stroke::new(1.0, Color32::WHITE.gamma_multiply(0.6)));
                    let d = br * 0.45;
                    p.line_segment([bc + vec2(-d, -d), bc + vec2(d, d)], Stroke::new(1.5, Color32::WHITE));
                    p.line_segment([bc + vec2(d, -d), bc + vec2(-d, d)], Stroke::new(1.5, Color32::WHITE));
                }
            }
            Kind::MoreFam | Kind::MoreVar => {
                p.circle(n.center, n.r, pal.disc, Stroke::new(if hot { 2.5 } else { 1.5 }, if hot { pal.bright } else { pal.ring.gamma_multiply(0.7) }));
                let more = if n.kind == Kind::MoreFam { fam_more } else { var_more };
                p.text(n.center, Align2::CENTER_CENTER, format!("+{more}"), theme::semibold(12.0), pal.bright);
                let (at_label, align) = label_spot(n, c);
                p.text(at_label, align, tl!("More"), label_font.clone(), pal.dim);
            }
        }
    }

    // The orbits' headers in their gaps beside their groups: the hub's name by its families, and
    // the open family by its variations, which closes them.
    let galley = p.layout_no_wrap(tl!(active_cat.as_str()).to_uppercase(), theme::semibold(10.5), pal.bright);
    let rect = Rect::from_center_size(at(fam_head, g::R_FAM), galley.size() + vec2(18.0, 10.0));
    p.rect_filled(rect, rect.height() / 2.0, pal.bg);
    p.galley(rect.center() - galley.size() / 2.0, galley, pal.bright);
    if let Some(f) = &open_fam {
        let header = crate::i18n::fmt(
            if f.members.len() == 1 { tl!("{family} · {count} VARIATION") } else { tl!("{family} · {count} VARIATIONS") },
            &[("family", &f.name.to_uppercase()), ("count", &f.members.len().to_string())],
        );
        let text = format!("‹  {header}");
        let galley = p.layout_no_wrap(text.clone(), theme::semibold(10.5), pal.bright);
        let rect = Rect::from_center_size(at(var_head, g::R_VAR), galley.size() + vec2(18.0, 10.0));
        let resp = ui.interact(rect, hud_id().with("back"), Sense::click()).on_hover_text(tl!(active_cat.as_str()));
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &text));
        p.rect(rect, rect.height() / 2.0, if resp.hovered() { pal.disc_hi } else { pal.bg }, Stroke::new(1.0, pal.ring), egui::StrokeKind::Inside);
        p.galley(rect.center() - galley.size() / 2.0, galley, pal.bright);
        if resp.clicked() {
            acts.push(Act::Back);
        }
    }

    // Recent / favourites label, which toggles between the two.
    let label_pos = at(g::RECENT_LABEL_DEG, g::R_RECENT_LABEL);
    let label = match (st.ring, ring.is_empty()) {
        (Ring::Recent, false) => tl!("Recent Brushes").to_string(),
        (Ring::Recent, true) => tl!("Recent Brushes · none yet").to_string(),
        (Ring::Favorites, false) => tl!("Favorite Brushes").to_string(),
        (Ring::Favorites, true) => tl!("Favorite Brushes · none yet").to_string(),
    };
    let lr = p.text(label_pos, Align2::CENTER_CENTER, label, FontId::proportional(11.5 * (s / 0.52).clamp(0.9, 1.1)), pal.text);
    if ui.interact(lr.expand(3.0), hud_id().with("ring-label"), Sense::click()).on_hover_text(tl!("Switch between recent and favorite brushes")).clicked() {
        acts.push(Act::Ring);
    }

    // Edit HUD: the bin, and the dragged node following the pointer.
    if st.edit {
        let hot = over_bin;
        p.circle(bin.center(), if hot { 34.0 } else { 29.0 }, pal.danger.gamma_multiply(if hot { 0.45 } else { 0.18 }), Stroke::new(2.0, pal.danger));
        crate::icons::paint(ui, Rect::from_center_size(bin.center(), Vec2::splat(24.0)), "trash", 24.0, if hot { Color32::WHITE } else { pal.danger });
        p.text(bin.center() + vec2(0.0, 40.0), Align2::CENTER_CENTER, tl!("Drop to remove"), FontId::proportional(10.5), pal.danger);
        if let (Some(di), Some(ptr)) = (drag_node, pointer)
            && let Some(dn) = nodes.get(di)
        {
            let ghost = ui.painter().clone().with_layer_id(egui::LayerId::new(Order::Tooltip, hud_id().with("ghost")));
            ghost.circle(ptr, 26.0, pal.disc, Stroke::new(2.0, pal.danger));
            if let Some(pr) = fp_any(lib, presets, dn) {
                tip(&ui.new_child(UiBuilder::new().layer_id(ghost.layer_id())), pr, Rect::from_center_size(ptr, Vec2::splat(36.0)), pal.text);
            } else {
                glyph(&ui.new_child(UiBuilder::new().layer_id(ghost.layer_id())), ptr, 26.0, &dn.id, pal.text);
            }
            ctx.request_repaint();
        }
    }

    // Hover drives the preview panel.
    if app.ui.brush_hud.hover != hover_preset {
        app.ui.brush_hud.hover = hover_preset;
    }
    nodes
}

/// Where an orbit node's name goes: on the side away from the wheel's centre `c` (under nodes
/// in the lower half, over those in the upper half), so it never reaches into the ring inside.
fn label_spot(n: &Node, c: Pos2) -> (Pos2, Align2) {
    if n.center.y < c.y { (n.center - vec2(0.0, n.r + 5.0), Align2::CENTER_BOTTOM) } else { (n.center + vec2(0.0, n.r + 5.0), Align2::CENTER_TOP) }
}

/// `text` cut to `max` characters with an ellipsis.
fn elide(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut s: String = text.chars().take(max.saturating_sub(1)).collect();
    s.push('…');
    s
}

/// The preset a node shows (its own, or its family's first).
fn fp_any<'a>(lib: &Library, presets: &'a [BrushPreset], n: &Node) -> Option<&'a BrushPreset> {
    let name = match n.kind {
        Kind::Var | Kind::Recent => Some(n.id.clone()),
        Kind::Family => lib.entries.iter().find(|e| e.family_id == n.id).map(|e| e.name.clone()),
        _ => None,
    }?;
    lib.entry(&name).and_then(|e| presets.get(e.index))
}

fn app_drag_start(ctx: &egui::Context, n: &Node) {
    ctx.data_mut(|d| d.insert_temp(hud_id().with("drag"), n.key()));
}

/// Where a dragged node would land: a sibling in its scope under the pointer, or the bin.
fn drop_at(nodes: &[Node], dragged: &Node, pointer: Option<Pos2>, bin: Rect) -> (Option<usize>, bool) {
    let Some(ptr) = pointer else { return (None, false) };
    if bin.expand(12.0).contains(ptr) {
        return (None, dragged.kind != Kind::Hub);
    }
    let Some(scope) = &dragged.scope else { return (None, false) };
    let mut best: Option<(usize, f32)> = None;
    for (i, n) in nodes.iter().enumerate() {
        if n.key() == dragged.key() || n.scope.as_ref() != Some(scope) {
            continue;
        }
        let d = (n.center - ptr).length();
        if d < n.r * 1.6 && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((i, d));
        }
    }
    (best.map(|b| b.0), false)
}

// ------------------------------------------------------------------ search tracer

fn tracer(app: &PhotocraftApp, ui: &Ui, search: Rect, nodes: &[Node], stage: Rect, pal: &Pal, now: f64) {
    let st = &app.ui.brush_hud;
    let Some(target) = &st.target else { return };
    let lib_entry = nodes.iter().find(|n| n.kind == Kind::Var && &n.id == target);
    let fam_node = || {
        let ids: Vec<&Node> = nodes.iter().filter(|n| n.kind == Kind::Family).collect();
        ids.into_iter().find(|n| st.family.as_deref() == Some(n.id.as_str()))
    };
    let Some(node) = lib_entry.or_else(fam_node) else { return };
    let size = wheel_size(stage);
    let s = size / g::U;
    let c = stage.center();
    let start = pos2(search.left() + 28.0, search.bottom() + 3.0);
    let pts = g::tracer((start - c) / s, (node.center - c) / s, g::RING_OUTER + 22.0, (node.r + 5.0) / s);
    let frac = (((now - st.target_since) / 0.42).clamp(0.0, 1.0)) as f32;
    let eased = 1.0 - (1.0 - frac).powi(3);
    let shown: Vec<Pos2> = g::polyline_prefix(&pts, eased).into_iter().map(|v| c + v * s).collect();
    let p = ui.painter().clone().with_layer_id(egui::LayerId::new(Order::Foreground, hud_id().with("tracer")));
    p.add(Shape::line(shown.clone(), Stroke::new(8.0, pal.glow.gamma_multiply(1.4))));
    p.add(Shape::line(shown.clone(), Stroke::new(2.0, pal.bright)));
    p.circle_filled(start, 3.5, pal.target);
    if let Some(end) = shown.last() {
        p.circle_filled(*end, 3.0, Color32::WHITE);
    }
    // Chevrons every ~110 points along the way, pointing where the line goes.
    let mut acc = 0.0;
    let mut next = 90.0;
    for w in shown.windows(2) {
        let (a, b) = (w[0], w[1]);
        let seg = (b - a).length();
        while seg > 0.0 && acc + seg >= next {
            let m = a + (b - a) * ((next - acc) / seg);
            let dir = (b - a) / seg;
            let back = |k: f32| m - Vec2::angled(dir.angle() + k) * 9.0;
            p.add(Shape::line(vec![back(0.6), m, back(-0.6)], Stroke::new(2.2, pal.target)));
            next += 110.0;
        }
        acc += seg;
    }
    if frac >= 1.0 {
        p.circle_stroke(node.center, node.r + 7.0, Stroke::new(2.5, pal.target));
        p.circle_stroke(node.center, node.r + 13.0, Stroke::new(6.0, pal.glow));
    } else {
        ui.ctx().request_repaint();
    }
}

// ------------------------------------------------------------------ applying changes

fn apply(app: &mut PhotocraftApp, lib: &Library, acts: Vec<Act>, now: f64) {
    for act in acts {
        let st = &mut app.ui.brush_hud;
        match act {
            Act::Category(c) => {
                // The active hub leads back from a family's variations to its families.
                if st.category == c {
                    st.family = None;
                }
                super::set_category(st, &c);
            }
            Act::Back => {
                st.family = None;
                st.var_page = 0;
            }
            Act::Family(id) => {
                if st.family.as_deref() != Some(id.as_str()) {
                    st.family = Some(id);
                    st.var_page = 0;
                }
            }
            Act::Pick(name) => {
                super::pick(app, &name);
            }
            Act::Hide(kind, id) => {
                if kind == Kind::Recent {
                    st.recent.retain(|n| n != &id);
                    st.favorites.retain(|n| n != &id);
                } else if kind != Kind::Hub {
                    let mut next = st.layout.clone();
                    next.hidden.insert(id.clone());
                    st.set_layout(next);
                    if st.family.as_deref() == Some(id.as_str()) {
                        st.family = None;
                    }
                    let name = id.strip_prefix("fam:").and_then(|f| f.split('/').nth(1)).unwrap_or(&id).to_string();
                    app.ui.status = crate::i18n::fmt(tl!("Removed “{name}” from the Brush HUD (Ctrl+Z to undo)"), &[("name", &name)]);
                }
            }
            Act::Move { scope, id, target } => {
                let keys = lib.scope_keys(&scope, &st.layout);
                if let Some(before) = model::drop_before(&keys, &id, &target)
                    && let Some(next) = model::moved(lib, &st.layout, &scope, &id, before.as_deref())
                {
                    st.set_layout(next);
                }
            }
            Act::Favorite(name) => {
                let on = st.toggle_favorite(&name);
                let template = if on { tl!("Added “{name}” to favorite brushes") } else { tl!("Removed “{name}” from favorite brushes") };
                app.ui.status = crate::i18n::fmt(template, &[("name", &name)]);
            }
            Act::PageFam(d) => {
                st.fam_page = (st.fam_page as i64 + i64::from(d)).max(0) as usize;
            }
            Act::PageVar(d) => {
                st.var_page = (st.var_page as i64 + i64::from(d)).max(0) as usize;
            }
            Act::Close => super::close(app),
            Act::Edit => {
                st.edit = !st.edit;
            }
            Act::Undo => {
                st.undo();
            }
            Act::Redo => {
                st.redo();
            }
            Act::Restore => {
                let mut next = st.layout.clone();
                next.hidden.clear();
                st.set_layout(next);
            }
            Act::Ring => {
                st.ring = if st.ring == Ring::Recent { Ring::Favorites } else { Ring::Recent };
            }
            Act::Search => {
                if st.query.trim().is_empty() {
                    st.target = None;
                } else {
                    super::run_search(st, lib, &app.session.tools.presets, now);
                }
            }
            Act::Import => {
                // The same Open dialog as the Brushes panel's import button: an .abr lands in the
                // library, and the wheel picks the new brushes up.
                let _ = app.open_dialog_file();
            }
        }
    }
    // Pages wrap within their count.
    let st = &mut app.ui.brush_hud;
    let fams = lib.families(&st.category, &st.layout);
    if fams.len() > g::FAM_MAX {
        st.fam_page %= fams.len().div_ceil(g::FAM_MAX - 1);
    } else {
        st.fam_page = 0;
    }
    if let Some(f) = st.family.as_ref().and_then(|id| fams.iter().find(|f| &f.id == id)) {
        if f.members.len() > g::VAR_MAX {
            st.var_page %= f.members.len().div_ceil(g::VAR_MAX - 1);
        } else {
            st.var_page = 0;
        }
    }
}
