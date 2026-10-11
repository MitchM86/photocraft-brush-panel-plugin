//! Window › Brush Settings (F5) and Window › Brushes: Photoshop's floating brush panel.
//!
//! Brush Settings: the section list with enable boxes on the left (clicking a name shows and turns
//! on that section), the selected section's controls on the right (see [`crate::brush_sections`]),
//! and a live stroke preview strip below, rendered with the real brush engine and cached on the
//! settings (it re-renders only when they change). Brushes: the presets in collapsible groups with
//! tip thumbnails and stroke previews, a size slider and a search field.
//!
//! The panel never writes the session brush itself: each frame's edits become one
//! `tools.setBrush` call carrying only the changed fields (Rule 1), so the journal, the control
//! channel and the MCP server see exactly what the panel did and can do the same. A drag's calls
//! share a `coalesce` key, so a whole gesture is one journal entry ([`commit_gesture`]).
//!
//! The lock beside a section name keeps that section's settings when another preset is picked
//! (`locks` in the brush, applied by the engine). The Brushes tab lives in [`crate::brushes_tab`].

use egui::{Color32, RichText, Sense, Stroke, vec2};
use photocraft_engine::BrushSettings;
use photocraft_engine::paint;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::brush_preview;
use crate::theme::{self, Tokens};
use crate::{PhotocraftApp, icons, widgets};

pub use crate::brush_preview::{preview_pixels, preview_sig};
pub use crate::brush_sections::section_body;
pub use photocraft_engine::brush_cmds::brush_patch;

/// Sections in Photoshop's order. The bool says whether the section has an enable box.
pub const SECTIONS: [(&str, bool); 13] = [
    ("Brush Tip Shape", false),
    ("Shape Dynamics", true),
    ("Scattering", true),
    ("Texture", true),
    ("Dual Brush", true),
    ("Color Dynamics", true),
    ("Transfer", true),
    ("Brush Pose", true),
    ("Noise", true),
    ("Wet Edges", true),
    ("Build-up", true),
    ("Smoothing", false),
    ("Protect Texture", true),
];

/// Panel width at its default size, and the stroke strip's height.
pub(crate) const WIDTH: f32 = 540.0;
const STRIP_H: u32 = 76;
/// The panel's default content height (the section body's 450 pt scroll area plus the rest) and
/// its smallest size: resizing the window makes the section body and the strip follow.
pub(crate) const DEFAULT_H: f32 = 662.0;
pub(crate) const MIN_SIZE: (f32, f32) = (460.0, 420.0);
/// The section body's smallest height.
const MIN_BODY_H: f32 = 150.0;

/// How the Brushes tab lists presets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BrushesView {
    /// Rows: tip, size, stroke preview, name.
    #[default]
    List,
    /// Tip thumbnails with their sizes.
    Grid,
}

/// A rename in progress in the Brushes tab (a preset, or a group when `group` is set).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Renaming {
    pub group: bool,
    /// The preset (or group) being renamed.
    pub name: String,
    /// With `group`: the nested folder of group `name` being renamed (empty = the group itself).
    pub folder: Vec<String>,
    /// The text typed so far.
    pub text: String,
}

/// Brushes panel view state (serde, so the control channel can read and drive it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BrushesPanelState {
    /// Collapsed groups and folders: a group's label, or for a nested folder the group's label and
    /// the folder path joined with `/` ([`crate::brushes_tab::folder_view_key`]).
    pub collapsed: Vec<String>,
    /// Case-insensitive name filter.
    pub filter: String,
    pub view: BrushesView,
    pub renaming: Option<Renaming>,
    /// The Brush Preset picker's cards: show the preset's name. At least one of the three parts
    /// is always on, and the picker's cards never go empty (see [`crate::brush_picker`]).
    pub show_name: bool,
    /// The Brush Preset picker's cards: show the stroke preview.
    pub show_stroke: bool,
    /// The Brush Preset picker's cards: show the tip thumbnail.
    pub show_tip: bool,
    /// The Brush Preset picker's cards: their size scale, from the picker's footer slider
    /// (`crate::brush_picker::body`). 1 is the standard size; the cards' width, height and
    /// insides multiply by it (`crate::brushes_tab`) — the height down to a 0.50 floor, the
    /// width all the way — the text holds its 1.0 size, and at or below the compact threshold
    /// the tips drop the size number under them and take their cell whole.
    pub scale: f32,
}

impl Default for BrushesPanelState {
    fn default() -> Self {
        Self {
            collapsed: Vec::new(),
            filter: String::new(),
            view: BrushesView::default(),
            renaming: None,
            show_name: true,
            show_stroke: true,
            show_tip: true,
            scale: 1.0,
        }
    }
}

/// The enable flag behind section `i` (None for Brush Tip Shape and Smoothing).
pub fn section_flag(b: &mut BrushSettings, i: usize) -> Option<&mut bool> {
    Some(match i {
        1 => &mut b.shape_dynamics.enabled,
        2 => &mut b.scattering.enabled,
        3 => &mut b.texture.enabled,
        4 => &mut b.dual_brush.enabled,
        5 => &mut b.color_dynamics.enabled,
        6 => &mut b.transfer.enabled,
        7 => &mut b.pose.enabled,
        8 => &mut b.noise,
        9 => &mut b.wet_edges,
        10 => &mut b.build_up,
        12 => &mut b.protect_texture,
        _ => return None,
    })
}

/// The lock behind section `i` (every section but Brush Tip Shape has one, as in Photoshop).
pub fn section_lock(b: &mut BrushSettings, i: usize) -> Option<&mut bool> {
    let l = &mut b.locks;
    Some(match i {
        1 => &mut l.shape_dynamics,
        2 => &mut l.scattering,
        3 => &mut l.texture,
        4 => &mut l.dual_brush,
        5 => &mut l.color_dynamics,
        6 => &mut l.transfer,
        7 => &mut l.pose,
        8 => &mut l.noise,
        9 => &mut l.wet_edges,
        10 => &mut l.build_up,
        11 => &mut l.smoothing,
        12 => &mut l.protect_texture,
        _ => return None,
    })
}

/// Send the panel's edits (`before` → `after`) through `tools.setBrush`.
pub fn commit(app: &mut PhotocraftApp, before: &BrushSettings, after: &BrushSettings) {
    send(app, before, after, None);
}

fn send(app: &mut PhotocraftApp, before: &BrushSettings, after: &BrushSettings, key: Option<String>) -> bool {
    if before == after {
        return false;
    }
    let patch = brush_patch(before, after);
    if patch.as_object().is_some_and(Map::is_empty) {
        return false;
    }
    let mut p = json!({ "brush": patch });
    if let Some(k) = key {
        p["coalesce"] = json!(k);
    }
    if let Err(e) = app.run("tools.setBrush", p) {
        app.ui.status = e;
    }
    true
}

const GESTURE: &str = "brush-set-gesture";

/// The current brush-edit gesture number. A pointer press starts a new gesture (once per frame);
/// [`commit_gesture`] ends one after an edit made with the pointer up (a click, a typed value).
fn gesture(ctx: &egui::Context) -> u64 {
    let frame = ctx.cumulative_pass_nr();
    let pressed = ctx.input(|i| i.pointer.any_pressed());
    ctx.data_mut(|d| {
        let g = d.get_temp_mut_or_default::<(u64, u64)>(egui::Id::new(GESTURE));
        if pressed && g.1 != frame {
            g.0 += 1;
            g.1 = frame;
        }
        g.0
    })
}

/// [`commit`] as part of a gesture: every call of one drag carries the same `coalesce` key, so
/// the engine journals the drag as a single `tools.setBrush` (Rule 1: one command per gesture).
/// Call it every frame (it tracks presses even when nothing changed).
pub fn commit_gesture(app: &mut PhotocraftApp, ctx: &egui::Context, before: &BrushSettings, after: &BrushSettings) {
    let g = gesture(ctx);
    if send(app, before, after, Some(format!("brush-ui:{g}"))) && !ctx.input(|i| i.pointer.any_down()) {
        ctx.data_mut(|d| d.get_temp_mut_or_default::<(u64, u64)>(egui::Id::new(GESTURE)).0 += 1);
    }
}

/// Group label for presets saved without a group.
pub const UNGROUPED: &str = "My Brushes";

/// Preset indices in panel order: groups in order of first appearance.
pub fn grouped_presets(presets: &[paint::BrushPreset]) -> Vec<(String, Vec<usize>)> {
    let mut out: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, p) in presets.iter().enumerate() {
        let g = if p.group.is_empty() { UNGROUPED } else { p.group.as_str() };
        match out.iter_mut().find(|(n, _)| n == g) {
            Some((_, v)) => v.push(i),
            None => out.push((g.to_string(), vec![i])),
        }
    }
    out
}

/// A fresh "Brush N" name.
pub(crate) fn new_preset_name(presets: &[paint::BrushPreset]) -> String {
    (1..).map(|n| format!("Brush {n}")).find(|n| !presets.iter().any(|p| &p.name == n)).unwrap_or_else(|| "Brush".into())
}

pub(crate) fn run_or_status(app: &mut PhotocraftApp, id: &str, p: Value) {
    if let Err(e) = app.run(id, p) {
        app.ui.status = e;
    }
}

pub(crate) fn full_uv() -> egui::Rect {
    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0))
}

/// The section list: enable boxes, names, locks, the selection. Clicking a name shows the section
/// and turns it on (Photoshop); clicking a box only toggles it; the lock on the right keeps the
/// section when another preset is picked.
fn section_list(app: &mut PhotocraftApp, ui: &mut egui::Ui, b: &mut BrushSettings) {
    let t = Tokens::get(ui.ctx());
    ui.vertical(|ui| {
        ui.set_width(150.0);
        ui.spacing_mut().item_spacing.y = 1.0;
        for (i, (name, has_box)) in SECTIONS.iter().enumerate() {
            let sel = app.ui.brush_section == i;
            let (r, resp) = ui.allocate_exact_size(vec2(150.0, 23.0), Sense::click());
            // Named for accessibility and for tests and agents that look sections up by name.
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, sel, tl!(*name)));
            if sel {
                ui.painter().rect_filled(r, t.radius_sm, t.accent_soft);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, t.radius_sm, t.hover);
            }
            let mut x = r.left() + 6.0;
            if *has_box && let Some(flag) = section_flag(b, i) {
                let br = egui::Rect::from_center_size(egui::pos2(x + 6.0, r.center().y), vec2(13.0, 13.0));
                let box_resp = ui.interact(br, ui.id().with(("brush-sec-box", i)), Sense::click());
                let checked = *flag;
                box_resp.widget_info(|| {
                    egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, checked, crate::i18n::fmt(tl!("Enable {name}"), &[("name", tl!(*name))]))
                });
                if *flag {
                    ui.painter().rect_filled(br, 2.0, t.accent);
                    let (a, m, c) = (br.left_center() + vec2(3.0, 0.5), br.center_bottom() + vec2(-1.0, -3.5), br.right_top() + vec2(-3.0, 3.5));
                    ui.painter().line_segment([a, m], Stroke::new(1.6, Color32::WHITE));
                    ui.painter().line_segment([m, c], Stroke::new(1.6, Color32::WHITE));
                } else {
                    ui.painter().rect_stroke(br, 2.0, Stroke::new(1.2, t.text_faint), egui::StrokeKind::Inside);
                }
                let on_lock = resp.interact_pointer_pos().is_some_and(|p| p.x > r.right() - 22.0);
                if box_resp.clicked() {
                    *flag = !*flag;
                } else if resp.clicked() && !on_lock {
                    *flag = true;
                }
                x += 20.0;
            } else if i > 0 {
                x += 20.0;
            }
            let color = if sel { t.text } else { t.text_dim };
            let font = if i == 0 { theme::semibold(12.0) } else { theme::medium(12.0) };
            let mut job = egui::text::LayoutJob::simple_singleline(tl!(name).to_string(), font, color);
            job.wrap = egui::text::TextWrapping::truncate_at_width((r.right() - 22.0 - x).max(0.0));
            let galley = ui.painter().layout_job(job);
            let elided = galley.elided;
            ui.painter().galley(egui::pos2(x, r.center().y - galley.size().y / 2.0), galley, color);
            let resp = if elided { resp.on_hover_text(tl!(name)) } else { resp };
            let mut lock_clicked = false;
            if let Some(lock) = section_lock(b, i) {
                let lr = egui::Rect::from_center_size(egui::pos2(r.right() - 11.0, r.center().y), vec2(18.0, 18.0));
                let lresp = ui.interact(lr, ui.id().with(("brush-sec-lock", i)), Sense::click());
                // Like Photoshop: the lock shows only when set or hovered.
                if *lock || resp.hovered() || lresp.hovered() {
                    let tint = if *lock { t.text } else { t.text_faint };
                    icons::paint(ui, lr, if *lock { "lock" } else { "lock-open" }, 12.0, tint);
                }
                let tip =
                    if *lock { tl!("Unlock: picking a preset replaces these settings") } else { tl!("Lock: keep these settings when picking another preset") };
                if lresp.on_hover_text(tip).clicked() {
                    *lock = !*lock;
                    lock_clicked = true;
                }
            }
            if resp.clicked() && !lock_clicked {
                app.ui.brush_section = i;
            }
            if i == 0 {
                ui.add_space(4.0);
            }
        }
    });
}

pub(crate) fn settings_tab(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let before = app.session.tools.brush.clone();
    let mut b = before.clone();
    let section = app.ui.brush_section.min(SECTIONS.len() - 1);
    // The window is resizable: the strip and its buttons are a footer panel, and the section list
    // and body take exactly what it leaves (egui's panel layout, so nothing measures the window
    // and the size stays put when the tab changes).
    let width = ui.available_width().max(MIN_SIZE.0 - 22.0);
    egui::Panel::bottom(ui.id().with("brush-settings-footer")).resizable(false).show_separator_line(false).frame(egui::Frame::NONE).show(ui, |ui| {
        ui.add_space(6.0);
        widgets::hairline(ui);
        ui.add_space(6.0);
        settings_footer(app, ui, &mut b, width);
    });
    egui::CentralPanel::no_frame().show(ui, |ui| {
        ui.horizontal_top(|ui| {
            // Scrolls when the panel is shorter than the list (docked in a short group).
            egui::ScrollArea::vertical()
                .id_salt("brush-section-list")
                .max_height(ui.available_height().max(MIN_BODY_H))
                .auto_shrink([true, false])
                .show(ui, |ui| section_list(app, ui, &mut b));
            widgets::vline(ui, ui.available_height().max(MIN_BODY_H));
            ui.vertical(|ui| {
                ui.set_width(width - 190.0);
                let name = SECTIONS.get(section).map_or("Brush Tip Shape", |entry| entry.0);
                ui.label(RichText::new(tl!(name)).font(theme::semibold(12.5)).color(t.text));
                ui.add_space(6.0);
                let on = section_flag(&mut b, section).is_none_or(|f| *f);
                let body_h = ui.available_height().max(MIN_BODY_H);
                egui::ScrollArea::vertical().id_salt(("brush-section", section)).max_height(body_h).auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_width(width - 204.0);
                    // A section that's off shows its options greyed out (Photoshop). A tip picked
                    // in the grid copies the tip fields only — the dynamics sections stay
                    // (Photoshop). The picked preset is recorded by name
                    // (`brush.presets.setCurrent`, identity, so look-alike duplicates stay
                    // distinct), since the engine can't infer it from the brush afterwards.
                    ui.add_enabled_ui(on, |ui| section_body(ui, &mut b, section, &app.session.tools.presets, app.session.tools.current_preset.as_deref()))
                        .inner
                        .inspect(|name| run_or_status(app, "brush.presets.setCurrent", json!({ "name": name })));
                    crate::brush_sections::response_curves(ui, &mut b, section);
                });
            });
        });
    });
    commit_gesture(app, ui.ctx(), &before, &b);
}

/// The settings tab's footer: the stroke strip and the new / reset / update buttons.
fn settings_footer(app: &mut PhotocraftApp, ui: &mut egui::Ui, b: &mut BrushSettings, width: f32) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        // In steps of 8 pt (488 at the default size), so dragging the window's edge doesn't
        // re-render the stroke every frame.
        let strip_w = (((width - 52.0) / 8.0).floor() * 8.0).max(200.0) as u32;
        let (r, _) = ui.allocate_exact_size(vec2(strip_w as f32, STRIP_H as f32), Sense::hover());
        ui.painter().rect_filled(r, t.radius_sm, t.field);
        let tex = brush_preview::stroke_texture(ui.ctx(), "settings-strip", b, strip_w, STRIP_H, t.text);
        ui.painter().image(tex.id(), r, full_uv(), Color32::WHITE);
        ui.vertical(|ui| {
            if icons::button(ui, "square-plus", 24.0, false, tl!("Create new brush from these settings")).clicked() {
                let name = new_preset_name(&app.session.tools.presets);
                run_or_status(app, "brush.presets.save", json!({ "name": name, "brush": serde_json::to_value(&*b).unwrap_or(Value::Null) }));
            }
            if icons::button(ui, "undo-2", 24.0, false, tl!("Reset the brush to the defaults")).clicked() {
                *b = BrushSettings { color: b.color, background: b.background, smoothing: b.smoothing.clone(), locks: b.locks.clone(), ..Default::default() };
            }
            // Overwrite the preset the current brush was picked from (Photoshop's "update the
            // selected brush"): no new preset, the brush keeps its place in the list.
            let current = app.session.tools.current_preset.clone().filter(|n| app.session.tools.presets.iter().any(|p| p.name.eq_ignore_ascii_case(n)));
            let update =
                ui.add_enabled_ui(current.is_some(), |ui| icons::button(ui, "check", 24.0, false, tl!("Update the current brush with these settings"))).inner;
            update.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, current.is_some(), "Update the current brush with these settings"));
            if update.clicked() {
                run_or_status(app, "brush.presets.update", json!({ "brush": serde_json::to_value(&*b).unwrap_or(Value::Null) }));
            }
        });
    });
}

/// Window › Brush Settings (F5): the Brush Settings | Brushes group, floating unless docked,
/// drawn with the other floating panel groups (`dock::floating_windows`), which also drop panels
/// dragged by their tabs.
pub fn window(app: &mut PhotocraftApp, ctx: &egui::Context) {
    crate::dock::floating_windows(app, ctx);
    crate::dock::end_drag(app, ctx);
}

#[cfg(test)]
#[path = "brush_panel_tests.rs"]
mod tests;
