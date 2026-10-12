//! Brush HUD drawing and interaction: the overlay chrome (top bar, footer), the 3-tier
//! wheel, Edit HUD drag and drop with the bin, the search tracer and hub pulses. The parameter
//! panel is [`super::panel`]. Every change is collected as an [`Act`] while the frame paints and
//! applied afterwards, so painting only ever reads the app.

use egui::{Align2, Color32, FontId, Id, Order, Pos2, Rect, Sense, Shape, Stroke, Ui, UiBuilder, Vec2, pos2, vec2};
use photocraft_engine::paint::BrushPreset;

use super::Ring;
use super::geometry::{self as g, LabelSide};
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
    side: Option<LabelSide>,
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
    Settings,
    Import,
}

/// Footer workflow stages and the hub each jumps to.
const STAGES: [(&str, &str); 3] = [("CREATE", "Sketch"), ("PAINT", "Paint"), ("EXPRESS", "Flora")];

fn stage_of(category: &str) -> &'static str {
    match category {
        "Sketch" => "CREATE",
        "Paint" | "Surface" => "PAINT",
        _ => "EXPRESS",
    }
}

/// HUD colours: PhotoCraft's panel colours (card, field, borders) with the accent only on what
/// is active, hovered or picked, like the panels.
#[derive(Clone, Copy)]
struct Pal {
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
    Pal {
        disc: t.card,
        disc_hi: t.field,
        ring: t.field_border,
        ring_dim: t.card_border,
        bright: t.accent,
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

/// The wheel's diameter in a stage: room is left around it for the spine's labels.
fn wheel_size(stage: Rect) -> f32 {
    (stage.width() * 0.9).min(stage.height() * 0.9).max(260.0)
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
    let t = Tokens::get(ctx);
    let pal = palette(&t);
    let now = ctx.input(|i| i.time);
    let mut acts: Vec<Act> = Vec::new();
    egui::Area::new(hud_id()).order(Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
        ui.set_min_size(screen.size());
        // The scrim takes every click outside the HUD card: a click there closes the HUD.
        let scrim = ui.interact(screen, hud_id().with("scrim"), Sense::click());
        let card = screen.shrink(if screen.width() > 900.0 { 18.0 } else { 6.0 });
        let card_resp = ui.interact(card, hud_id().with("card"), Sense::click());
        if scrim.clicked() && !card_resp.hovered() {
            acts.push(Act::Close);
        }

        let top = Rect::from_min_size(card.min, vec2(card.width(), 52.0));
        let footer = Rect::from_min_max(pos2(card.left(), card.bottom() - 30.0), card.max);
        let panel_w = if card.width() < 760.0 { 0.0 } else { (card.width() * 0.28).clamp(300.0, 400.0) };
        let panel = Rect::from_min_max(pos2(card.right() - panel_w, top.bottom()), pos2(card.right(), footer.top()));
        let stage = Rect::from_min_max(pos2(card.left(), top.bottom()), pos2(panel.left(), footer.top()));

        let search_rect = top_bar(app, ui, top, &lib, &pal, &t, &mut acts);
        let nodes = wheel(app, ui, stage, &lib, &pal, &t, now, &mut acts);
        if panel_w > 0.0 {
            super::panel::show(app, ui, panel.shrink2(vec2(10.0, 10.0)), &lib, &pal_panel(&pal));
        }
        footer_bar(app, ui, footer, &pal, &mut acts);
        tracer(app, ui, search_rect, &nodes, stage, &pal, now);
    });
    apply(app, ctx, &lib, acts, now);
}

/// The few colours the panel shares with the wheel.
pub(super) struct PanelPal {
    pub card: Color32,
    pub ring: Color32,
    pub bright: Color32,
    pub dim: Color32,
}

fn pal_panel(p: &Pal) -> PanelPal {
    PanelPal { card: p.disc, ring: p.ring_dim, bright: p.bright, dim: p.dim }
}

// ------------------------------------------------------------------ top bar

fn top_bar(app: &mut PhotocraftApp, ui: &mut Ui, r: Rect, lib: &Library, pal: &Pal, t: &Tokens, acts: &mut Vec<Act>) -> Rect {
    let p = ui.painter().clone();
    // Brand.
    let logo = pos2(r.left() + 30.0, r.center().y);
    let s = 11.0;
    p.add(Shape::closed_line(vec![logo + vec2(0.0, -s), logo + vec2(s, s), logo + vec2(-s, s)], Stroke::new(2.0, pal.bright)));
    p.line_segment([logo + vec2(-s * 0.45, s * 0.35), logo + vec2(s * 0.45, s * 0.35)], Stroke::new(2.0, pal.bright));
    let name = p.text(pos2(r.left() + 50.0, r.center().y), Align2::LEFT_CENTER, "ArtForge", theme::semibold(18.0), pal.text);
    p.text(pos2(name.right() + 12.0, r.center().y + 1.0), Align2::LEFT_CENTER, tl!("Brush HUD"), FontId::proportional(13.0), pal.dim);

    // Search field with its breadcrumb chip.
    let sw = (r.width() * 0.3).clamp(220.0, 360.0);
    let search = Rect::from_center_size(pos2(r.center().x, r.center().y), vec2(sw, 32.0));
    p.rect_filled(search, 16.0, pal.disc);
    p.rect_stroke(search, 16.0, Stroke::new(1.2, pal.ring), egui::StrokeKind::Inside);
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
    // Right side: Edit HUD switch, undo/redo, ring mode, Brush Settings, close.
    let mut x = r.right() - 14.0;
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
    if button(ui, "sliders-horizontal", "Brush Settings (F5)", false, true) {
        acts.push(Act::Settings);
    }
    if button(ui, "folder-open", "Import Brushes…", false, true) {
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
    // Edit HUD: a pill with a switch.
    let pill = Rect::from_min_max(pos2(x - 122.0, r.center().y - 16.0), pos2(x - 4.0, r.center().y + 16.0));
    let pr =
        ui.interact(pill, hud_id().with("edit"), Sense::click()).on_hover_text(tl!("Edit HUD: drag to reorder, drop on the bin or right-click to remove (E)"));
    pr.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, st.edit, tl!("Edit HUD")));
    p.rect_filled(pill, 16.0, if pr.hovered() { pal.disc_hi } else { pal.disc });
    p.rect_stroke(pill, 16.0, Stroke::new(1.2, pal.ring), egui::StrokeKind::Inside);
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

// ------------------------------------------------------------------ footer

fn footer_bar(app: &PhotocraftApp, ui: &mut Ui, r: Rect, pal: &Pal, acts: &mut Vec<Act>) {
    let p = ui.painter().clone();
    let st = &app.ui.brush_hud;
    let mut x = r.left() + 24.0;
    let current = stage_of(&st.category);
    for (i, (label, cat)) in STAGES.iter().enumerate() {
        if i > 0 {
            let sep = p.text(pos2(x, r.center().y), Align2::LEFT_CENTER, "/", FontId::proportional(11.0), pal.faint);
            x = sep.right() + 10.0;
        }
        let galley = p.layout_no_wrap(tl!(label).to_string(), FontId::proportional(11.5), if *label == current { pal.bright } else { pal.dim });
        let rect = Rect::from_min_size(pos2(x, r.center().y - galley.size().y / 2.0), galley.size());
        let resp = ui.interact(rect.expand(4.0), hud_id().with(("stage", *label)), Sense::click());
        p.galley(rect.min, galley, pal.text);
        if resp.clicked() {
            acts.push(Act::Category(cat.to_string()));
        }
        x = rect.right() + 10.0;
    }
    let hint = if st.edit {
        tl!("●  EDIT MODE · DRAG TO REARRANGE · DRAG TO BIN OR RIGHT-CLICK TO REMOVE").to_string()
    } else {
        tl!("HOVER TO PREVIEW · CLICK TO SELECT · DOUBLE-CLICK TO PAINT · RIGHT-CLICK TO FAVORITE · ESC TO CLOSE").to_string()
    };
    p.text(pos2(x + 30.0, r.center().y), Align2::LEFT_CENTER, hint, FontId::proportional(10.5), if st.edit { pal.danger } else { pal.faint });
    let removed = st.layout.hidden.len();
    if removed > 0 {
        let label = tl!("Restore All").to_string();
        let galley = p.layout_no_wrap(label, FontId::proportional(11.0), pal.bright);
        let rect = Rect::from_min_size(pos2(r.right() - 20.0 - galley.size().x, r.center().y - galley.size().y / 2.0), galley.size());
        let resp = ui.interact(rect.expand(4.0), hud_id().with("restore"), Sense::click()).on_hover_text(tl!("Put every removed brush back on the HUD"));
        p.galley(rect.min, galley, pal.text);
        if resp.clicked() {
            acts.push(Act::Restore);
        }
    }
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
    let center_deg = angles.get(active).copied().unwrap_or(0.0);
    let fams = lib.families(&active_cat, &st.layout);
    let (fams_shown, fam_more, fam_pages, fam_page) = page(&fams, st.fam_page, g::FAM_MAX);
    let (fan, fan_a0, fan_a1) = g::family_fan(center_deg, fams_shown.len() + usize::from(fam_more > 0));
    let fam_idx = st.family.as_ref().and_then(|id| fams_shown.iter().position(|f| &f.id == id));
    let mut spine = None;
    if let Some(fi) = fam_idx
        && let (Some(f), Some(slot)) = (fams_shown.get(fi), fan.get(fi))
    {
        let (vars, more, pages, pg) = page(&f.members, st.var_page, g::VAR_MAX);
        let (nodes, a0, a1) = g::spine(slot.deg, vars.len() + usize::from(more > 0));
        spine = Some((f.clone(), *slot, vars, more, pages, pg, nodes, a0, a1));
    }
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
        nodes.push(Node { kind: Kind::Hub, id: name.clone(), center: at(deg, g::R_HUB), r: g::HUB_R * s, scope: Some("cat".into()), side: None });
    }
    nodes.push(Node { kind: Kind::Center, id: "center".into(), center: c, r: g::R_CENTER * s, scope: None, side: None });
    for (n, slot) in ring.iter().zip(g::recent_arc(ring.len())) {
        nodes.push(Node { kind: Kind::Recent, id: n.clone(), center: c + slot.offset() * s, r: g::RECENT_R * s, scope: None, side: None });
    }
    for (f, slot) in fams_shown.iter().zip(&fan) {
        nodes.push(Node {
            kind: Kind::Family,
            id: f.id.clone(),
            center: c + slot.offset() * s,
            r: g::FAM_R * s,
            scope: Some(fam_scope(&active_cat)),
            side: None,
        });
    }
    if fam_more > 0
        && let Some(slot) = fan.get(fams_shown.len())
    {
        nodes.push(Node { kind: Kind::MoreFam, id: "more:fam".into(), center: c + slot.offset() * s, r: g::FAM_R * s, scope: None, side: None });
    }
    if let Some((f, _, vars, more, _, _, sn, _, _)) = &spine {
        for (&m, (slot, side)) in vars.iter().zip(sn) {
            if let Some(e) = lib.entries.get(m) {
                nodes.push(Node {
                    kind: Kind::Var,
                    id: e.name.clone(),
                    center: c + slot.offset() * s,
                    r: g::VAR_R * s,
                    scope: Some(var_scope(&f.id)),
                    side: Some(*side),
                });
            }
        }
        if *more > 0
            && let Some((slot, side)) = sn.get(vars.len())
        {
            nodes.push(Node { kind: Kind::MoreVar, id: "more:var".into(), center: c + slot.offset() * s, r: g::VAR_R * s, scope: None, side: Some(*side) });
        }
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
        // A double-click picks the brush (its first click already did) and closes the HUD.
        if resp.double_clicked() && !st.edit && matches!(n.kind, Kind::Var | Kind::Recent) {
            acts.push(Act::Close);
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
        // Hover intent: a family's spine opens after a short dwell.
        if n.kind == Kind::Family && st.family.as_deref() != Some(n.id.as_str()) {
            let since = ctx.data_mut(|d| {
                let e = d.get_temp_mut_or_insert_with(hud_id().with("fam-hover"), || (n.id.clone(), now));
                if e.0 != n.id {
                    *e = (n.id.clone(), now);
                }
                e.1
            });
            if now - since >= 0.17 {
                acts.push(Act::Family(n.id.clone()));
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(60));
            }
        }
    }

    // ---------------------------------------------------------- paint
    // Labels may run past the stage into the card's margins, never past the card.
    let p = ui.painter().clone();
    // Every line of the wheel glows, so it stands out over the canvas: a wide accent haze under
    // the line itself.
    let haze = |w: f32| Stroke::new(w * s.max(0.6), pal.glow);
    let ring_at = |r: f32, w: f32, col: Color32| {
        p.circle_stroke(c, r * s, haze(10.0));
        p.circle_stroke(c, r * s, Stroke::new(w, col));
    };
    // The wedge of the open family's fan and spine, in wheel units.
    let fan_wedge = (!fan.is_empty()).then(|| {
        let (w0, w1) = match &spine {
            Some((.., a0, a1)) => (fan_a0.min(*a0 - 6.0), fan_a1.max(*a1 + 6.0)),
            None => (fan_a0, fan_a1),
        };
        let r1 = if spine.is_some() { g::R_SPINE + 34.0 } else { g::RING_MID - 4.0 };
        (g::RING_HUB_OUT + 6.0, r1, w0, w1)
    });
    // The section under the mouse or pen: a see-through tint with a glowing edge.
    if drag_node.is_none()
        && let Some(ptr) = ctx.pointer_hover_pos()
        && let Some((r0, r1, a0, a1)) = section_at((ptr - c) / s, fan_wedge, &angles)
    {
        let inner = g::arc_points(r0, a0, a1);
        let outer = g::arc_points(r1, a0, a1);
        for k in 0..inner.len().saturating_sub(1) {
            if let (Some(a), Some(b), Some(cc), Some(d)) = (inner.get(k), inner.get(k + 1), outer.get(k + 1), outer.get(k)) {
                p.add(Shape::convex_polygon(vec![c + *a * s, c + *b * s, c + *cc * s, c + *d * s], pal.bright.gamma_multiply(0.1), Stroke::NONE));
            }
        }
        let full = a1 - a0 >= 360.0;
        for (r, from, to) in [(r0, a0, a1), (r1, a0, a1)] {
            if r > 0.0 {
                let arc: Vec<Pos2> = g::arc_points(r, from, to).into_iter().map(|v| c + v * s).collect();
                p.add(Shape::line(arc.clone(), haze(14.0)));
                p.add(Shape::line(arc, Stroke::new(1.5, pal.bright)));
            }
        }
        if !full {
            for a in [a0, a1] {
                let edge = [at(a, r0), at(a, r1)];
                p.line_segment(edge, haze(14.0));
                p.line_segment(edge, Stroke::new(1.5, pal.bright));
            }
        }
    }
    // Rings and ticks.
    ring_at(g::RING_OUTER, 1.5, pal.ring_dim);
    for k in 0..24 {
        let a = k as f32 * 15.0;
        let big = k % 3 == 0;
        p.circle_filled(at(a, g::RING_OUTER), if big { 2.6 } else { 1.5 }, pal.ring.gamma_multiply(if big { 0.9 } else { 0.5 }));
    }
    for a in [-58.0f32, -40.0, -22.0, 22.0, 40.0, 58.0] {
        let dir = a.signum();
        let pts = vec![at(a - dir * 2.6, g::RING_OUTER - 9.0), at(a, g::RING_OUTER), at(a - dir * 2.6, g::RING_OUTER + 9.0)];
        p.add(Shape::line(pts.clone(), haze(8.0)));
        p.add(Shape::line(pts, Stroke::new(2.0, pal.ring)));
    }
    ring_at(g::RING_MID, 2.5, pal.ring);
    ring_at(g::RING_HUB_OUT, 1.5, pal.ring_dim);
    ring_at(g::RING_INNER, 1.5, pal.ring_dim);
    if cats.len() > 1 {
        for a in &angles {
            let mid = a + 180.0 / cats.len() as f32;
            let line = [at(mid, g::RING_INNER), at(mid, g::RING_MID)];
            p.line_segment(line, haze(8.0));
            p.line_segment(line, Stroke::new(1.2, pal.ring_dim));
        }
    }
    // Tier 2 wedge and the links from the hub.
    if let Some((r0, r1, w0, w1)) = fan_wedge {
        let outline: Vec<Pos2> = g::wedge(r0, r1, w0, w1).into_iter().map(|v| c + v * s).collect();
        p.add(Shape::closed_line(outline.clone(), haze(8.0)));
        p.add(Shape::closed_line(outline, Stroke::new(1.0, pal.ring)));
        let hub_edge = at(center_deg, g::R_HUB + g::HUB_R);
        for slot in &fan {
            let link = [hub_edge, c + slot.offset() * s];
            p.line_segment(link, haze(5.0));
            p.extend(Shape::dashed_line(&link, Stroke::new(1.0, pal.ring), 3.0, 6.0));
        }
    }
    // Tier 3 spine.
    if let Some((f, fslot, _, _, _, _, sn, a0, a1)) = &spine
        && !sn.is_empty()
    {
        let pts: Vec<Pos2> = g::arc_points(g::R_SPINE, a0 - 4.0, a1 + 4.0).into_iter().map(|v| c + v * s).collect();
        p.add(Shape::line(pts.clone(), Stroke::new(12.0, pal.glow)));
        p.add(Shape::line(pts, Stroke::new(2.5, pal.bright)));
        for a in [a0 - 4.0, a1 + 4.0] {
            p.circle_filled(at(a, g::R_SPINE), 4.5, pal.bright);
        }
        let joint = at(fslot.deg.clamp(*a0, *a1), g::R_SPINE);
        p.line_segment([c + fslot.offset() * s, joint], Stroke::new(2.0, pal.bright));
        p.circle_filled(joint, 7.0, pal.bright.gamma_multiply(0.35));
        p.circle_filled(joint, 3.5, Color32::WHITE);
        // Header past the start of the spine, along the ring's tangent and away from the spine:
        // the variations' labels run radially, so it never crosses them.
        let header = crate::i18n::fmt(
            if f.members.len() == 1 { tl!("{family} · {count} VARIATION") } else { tl!("{family} · {count} VARIATIONS") },
            &[("family", &f.name.to_uppercase()), ("count", &f.members.len().to_string())],
        );
        let galley = p.layout_no_wrap(header, theme::semibold(10.5), pal.bright);
        let (w, h) = (galley.size().x, galley.size().y);
        let start = a0 - 6.0;
        let anchor = at(start, g::R_SPINE);
        let rad = start.to_radians();
        let away = vec2(-rad.cos(), -rad.sin());
        let (dir, from) = if away.x >= 0.0 { (away, anchor) } else { (-away, anchor + away * w) };
        let across = vec2(-dir.y, dir.x);
        p.add(egui::epaint::TextShape::new(from - across * (h / 2.0), galley, pal.bright).with_angle(dir.y.atan2(dir.x)));
    }

    // Nodes.
    let pulse = ((now % 1.5) / 1.5) as f32;
    let target_cats: Vec<&str> = if st.query.trim().is_empty() {
        Vec::new()
    } else {
        lib.search(&st.query, presets, &st.layout, 200).into_iter().filter_map(|i| lib.entries.get(i).map(|e| e.category)).collect()
    };
    let target_cat = st.target.as_ref().and_then(|n| lib.entry(n)).map(|e| e.category);
    for (i, n) in nodes.iter().enumerate() {
        let hot = hovered == Some(i) && drag_node.is_none();
        let is_drag = drag_node == Some(i);
        let is_drop = drop_target == Some(i);
        let fp = |name: &str| preset(name);
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
                p.circle_stroke(
                    n.center,
                    n.r + 9.0 * s,
                    Stroke::new(if active { 16.0 } else { 11.0 } * s.max(0.5), pal.glow.gamma_multiply(if active || matched || hot { 1.6 } else { 0.6 })),
                );
                p.circle(n.center, n.r, pal.disc, Stroke::new(if active { 4.0 } else { 2.5 }, if active || hot { pal.bright } else { pal.ring }));
                p.circle(n.center, n.r - 9.0 * s, pal.disc_hi, Stroke::new(1.0, pal.ring_dim));
                if is_drop {
                    p.circle_stroke(n.center, n.r + 4.0, Stroke::new(3.0, t.warning));
                }
                glyph(ui, n.center - vec2(0.0, n.r * 0.22), n.r * 0.62, &n.id, if active { pal.bright } else { pal.text });
                p.text(
                    n.center + vec2(0.0, n.r * 0.36),
                    Align2::CENTER_CENTER,
                    tl!(n.id.as_str()),
                    theme::semibold((n.r * 0.27).clamp(10.0, 18.0)),
                    pal.text,
                );
                let count = cats.iter().find(|(c2, _)| *c2 == n.id).map_or(0, |(_, k)| *k);
                let bc = n.center + vec2(n.r * 0.72, -n.r * 0.72);
                p.circle(bc, 10.0, pal.disc_hi, Stroke::new(1.0, pal.ring));
                p.text(bc, Align2::CENTER_CENTER, count.to_string(), FontId::proportional(9.5), pal.text);
            }
            Kind::Center => {
                p.circle_stroke(c, n.r + 20.0 * s, Stroke::new(1.5, pal.ring_dim));
                p.circle_stroke(c, n.r + 6.0 * s, Stroke::new(9.0 * s.max(0.6), pal.glow.gamma_multiply(1.4)));
                p.circle(c, n.r, pal.disc, Stroke::new(if hot { 4.0 } else { 3.0 }, if hot { pal.bright } else { pal.ring }));
                p.circle_filled(c, n.r - 12.0 * s, pal.disc_hi);
                let cur = current.and_then(|name| lib.entry(name));
                glyph(ui, c - vec2(0.0, n.r * 0.3), n.r * 0.42, cur.map_or("Paint", |e| e.category), pal.text);
                let label = current.unwrap_or("Brush");
                let font = theme::semibold((n.r * 0.2).clamp(10.0, 17.0));
                let galley = p.layout(label.to_string(), font, pal.text, n.r * 1.6);
                p.galley(c + vec2(-galley.size().x / 2.0, n.r * 0.02), galley, pal.text);
                p.text(c + vec2(0.0, n.r * 0.55), Align2::CENTER_CENTER, tl!("Master Tool"), FontId::proportional((n.r * 0.13).clamp(9.0, 12.0)), pal.dim);
            }
            Kind::Recent => {
                p.circle_stroke(n.center, n.r + 4.0 * s.max(0.6), Stroke::new(7.0 * s.max(0.6), pal.glow.gamma_multiply(if hot { 1.8 } else { 0.7 })));
                p.circle(n.center, n.r, pal.disc_hi, Stroke::new(if hot { 2.5 } else { 1.5 }, if hot { pal.bright } else { pal.ring_dim }));
                if let Some(pr) = fp(&n.id) {
                    tip(ui, pr, Rect::from_center_size(n.center, Vec2::splat(n.r * 1.5)), pal.text);
                }
            }
            Kind::Family | Kind::Var => {
                let open = n.kind == Kind::Family && st.family.as_deref() == Some(n.id.as_str());
                let selected = n.kind == Kind::Var && current == Some(n.id.as_str());
                let targeted = (n.kind == Kind::Var && st.target.as_deref() == Some(n.id.as_str()))
                    || (n.kind == Kind::Family && st.target.as_ref().and_then(|t2| lib.entry(t2)).is_some_and(|e| e.family_id == n.id));
                let lit = open || hot || selected || targeted;
                p.circle_stroke(
                    n.center,
                    n.r + 6.0 * s.max(0.6),
                    Stroke::new(
                        10.0 * s.max(0.6),
                        if targeted {
                            pal.target.gamma_multiply(0.45)
                        } else {
                            pal.glow.gamma_multiply(if lit { 1.8 } else { 0.7 })
                        },
                    ),
                );
                let edge = if selected || hot || open {
                    pal.bright
                } else if targeted {
                    pal.target
                } else {
                    pal.ring
                };
                p.circle(n.center, n.r, pal.disc, Stroke::new(if hot || selected || open { 3.0 } else { 2.0 }, edge));
                if is_drop {
                    p.circle_stroke(n.center, n.r + 4.0, Stroke::new(3.0, t.warning));
                }
                let rep = if n.kind == Kind::Var {
                    fp(&n.id)
                } else {
                    lib.family(&n.id, &st.layout).and_then(|f| f.members.first().and_then(|&m| lib.entries.get(m)).and_then(|e| presets.get(e.index)))
                };
                if let Some(pr) = rep {
                    tip(ui, pr, Rect::from_center_size(n.center, Vec2::splat(n.r * 1.45)), pal.text);
                }
                let font = FontId::proportional(if n.kind == Kind::Var { 11.5 } else { 11.0 } * (s / 0.62).clamp(0.85, 1.15));
                let name = if n.kind == Kind::Family { lib.family(&n.id, &st.layout).map(|f| (f.name, f.members.len())) } else { None };
                let text = name.as_ref().map_or(n.id.as_str(), |(nm, _)| nm.as_str()).to_string();
                let col = if hot || selected || open || targeted { pal.bright } else { pal.text };
                let gap = 5.0;
                let text = elide(&text, 22);
                match n.side {
                    Some(LabelSide::Left) => {
                        p.text(n.center - vec2(n.r + gap, 0.0), Align2::RIGHT_CENTER, text, font, col);
                    }
                    Some(LabelSide::Right) => {
                        p.text(n.center + vec2(n.r + gap, 0.0), Align2::LEFT_CENTER, text, font, col);
                    }
                    // Near 12 and 6 o'clock neighbours sit side by side: labels run radially
                    // outward there, so they never overlap.
                    Some(LabelSide::Above | LabelSide::Below) => radial_label(&p, c, n, text, font, col, gap),
                    None => {
                        p.text(n.center + vec2(0.0, n.r + gap - 1.0), Align2::CENTER_TOP, text, font, col);
                    }
                }
                if let Some((_, k)) = name.filter(|(_, k)| *k > 1) {
                    let bc = n.center + vec2(n.r * 0.78, n.r * 0.7);
                    p.circle(bc, 8.0, pal.disc_hi, Stroke::new(1.0, pal.ring));
                    p.text(bc, Align2::CENTER_CENTER, k.to_string(), FontId::proportional(8.5), pal.text);
                }
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
                p.extend(Shape::dashed_line(
                    &g::arc_points(n.r / s, 0.0, 360.0).into_iter().map(|v| n.center + v * s).collect::<Vec<_>>(),
                    Stroke::new(1.5, pal.ring),
                    4.0,
                    4.0,
                ));
                let (more, pg, pages) =
                    if n.kind == Kind::MoreFam { (fam_more, fam_page, fam_pages) } else { spine.as_ref().map_or((0, 0, 1), |sp| (sp.3, sp.5, sp.4)) };
                p.text(n.center, Align2::CENTER_CENTER, format!("+{more}"), theme::semibold(11.0), pal.bright);
                p.text(
                    n.center + vec2(0.0, n.r + 4.0),
                    Align2::CENTER_TOP,
                    crate::i18n::fmt(tl!("Page {page}/{pages}"), &[("page", &(pg + 1).to_string()), ("pages", &pages.to_string())]),
                    FontId::proportional(10.0),
                    pal.dim,
                );
            }
        }
    }
    // Recent / favourites label, which toggles between the two.
    let label_pos = at(0.0, g::R_RECENT - 40.0);
    let label = match (st.ring, ring.is_empty()) {
        (Ring::Recent, false) => tl!("Recent Brushes").to_string(),
        (Ring::Recent, true) => tl!("Recent Brushes · none yet").to_string(),
        (Ring::Favorites, false) => tl!("Favorite Brushes").to_string(),
        (Ring::Favorites, true) => tl!("Favorite Brushes · none yet").to_string(),
    };
    let lr = p.text(label_pos, Align2::CENTER_CENTER, label, FontId::proportional(11.5 * (s / 0.62).clamp(0.85, 1.1)), pal.text);
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

/// The wheel section at offset `v` from the centre (wheel units), as (inner radius, outer radius,
/// start angle, end angle): the open family's wedge, a hub's sector between the dividers, the
/// inner disc, or the outer band. `None` outside the wheel.
fn section_at(v: Vec2, wedge: Option<(f32, f32, f32, f32)>, hubs: &[f32]) -> Option<(f32, f32, f32, f32)> {
    let r = v.length();
    let a = g::angle_of(v);
    if let Some((r0, r1, a0, a1)) = wedge
        && (r0..=r1).contains(&r)
        && g::norm(a - a0) <= a1 - a0
    {
        return Some((r0, r1, a0, a1));
    }
    if r < g::RING_INNER {
        return Some((0.0, g::RING_INNER, 0.0, 360.0));
    }
    if r < g::RING_MID {
        if hubs.len() < 2 {
            return Some((g::RING_INNER, g::RING_MID, 0.0, 360.0));
        }
        let half = 180.0 / hubs.len() as f32;
        let hub = hubs.iter().copied().find(|h| g::delta(*h, a).abs() <= half)?;
        return Some((g::RING_INNER, g::RING_MID, hub - half, hub + half));
    }
    (r < g::RING_OUTER).then_some((g::RING_MID, g::RING_OUTER, 0.0, 360.0))
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

/// A label running outward along the radius through node `n` (wheel centre `c`), reading left to
/// right on either half of the wheel.
fn radial_label(p: &egui::Painter, c: Pos2, n: &Node, text: String, font: FontId, color: Color32, gap: f32) {
    let out = (n.center - c).normalized();
    let galley = p.layout_no_wrap(text, font, color);
    let (w, h) = (galley.size().x, galley.size().y);
    let near = n.center + out * (n.r + gap);
    // Text runs along `dir`; its top edge is offset by half its height across the line.
    let (dir, start) = if out.x >= 0.0 { (out, near) } else { (-out, near + out * w) };
    let across = vec2(-dir.y, dir.x);
    let pos = start - across * (h / 2.0);
    p.add(egui::epaint::TextShape::new(pos, galley, color).with_angle(dir.y.atan2(dir.x)));
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

fn apply(app: &mut PhotocraftApp, ctx: &egui::Context, lib: &Library, acts: Vec<Act>, now: f64) {
    for act in acts {
        let st = &mut app.ui.brush_hud;
        match act {
            Act::Category(c) => {
                super::set_category(st, &c);
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
                st.family = None;
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
            Act::Settings => {
                crate::brush_picker::open_settings(app, ctx);
            }
            // The HUD stays open: imported brushes appear on it once the file is read.
            Act::Import => {
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

#[cfg(test)]
mod section_tests {
    use super::*;

    #[test]
    fn the_section_under_the_pointer() {
        let hubs = g::hub_angles(6);
        let at = |deg: f32, r: f32| g::polar(deg, r);
        assert_eq!(section_at(at(10.0, 100.0), None, &hubs), Some((0.0, g::RING_INNER, 0.0, 360.0)), "inner disc");
        assert_eq!(section_at(at(62.0, 300.0), None, &hubs), Some((g::RING_INNER, g::RING_MID, 30.0, 90.0)), "the second hub's sector");
        assert_eq!(section_at(at(-20.0, 300.0), None, &hubs), Some((g::RING_INNER, g::RING_MID, -30.0, 30.0)), "across 12 o'clock");
        assert_eq!(section_at(at(200.0, 600.0), None, &hubs), Some((g::RING_MID, g::RING_OUTER, 0.0, 360.0)), "outer band");
        assert_eq!(section_at(at(0.0, 700.0), None, &hubs), None, "outside the wheel");
        let wedge = Some((376.0, 624.0, -40.0, 40.0));
        assert_eq!(section_at(at(-35.0, 600.0), wedge, &hubs), wedge, "the open family's wedge wins");
        assert_eq!(section_at(at(45.0, 600.0), wedge, &hubs).map(|s| s.0), Some(g::RING_MID), "beside the wedge");
    }
}
