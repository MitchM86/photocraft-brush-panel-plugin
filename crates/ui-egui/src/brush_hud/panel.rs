//! The Brush HUD's right-hand panel: a live stroke preview of the hovered brush (or the brush in
//! hand), its name and purpose, Size / Opacity / Flow, Advanced Settings and the dynamics
//! sections. Hovering previews read-only; otherwise the controls edit the brush in hand through
//! `tools.setBrush` gestures ([`crate::brush_panel::commit_gesture`]), with the same section
//! editors as the Brush Settings panel ([`crate::brush_sections::section_body`]).

use egui::{Align2, Color32, FontId, Rect, RichText, Stroke, Ui, UiBuilder, pos2, vec2};
use photocraft_engine::BrushSettings;
use photocraft_engine::paint::MAX_BRUSH_SIZE;

use super::model::Library;
use super::view::PanelPal;
use crate::PhotocraftApp;
use crate::theme::{self, Tokens};
use crate::widgets;

/// The dynamics sections the panel lists: Brush Settings section index, title and icon.
const SECTIONS: [(usize, &str, &str); 8] = [
    (1, "Shape Dynamics", "maximize-2"),
    (2, "Scattering", "sparkles"),
    (3, "Texture", "grid-3x3"),
    (4, "Dual Brush", "layers"),
    (5, "Color Dynamics", "palette"),
    (6, "Transfer", "gauge"),
    (8, "Noise", "circle-dashed"),
    (9, "Wet Edges", "droplet"),
];

pub(super) fn show(app: &mut PhotocraftApp, ui: &mut Ui, rect: Rect, lib: &Library, pal: &PanelPal) {
    let t = Tokens::get(ui.ctx());
    ui.painter().rect_filled(rect, t.radius_lg, pal.card);
    ui.painter().rect_stroke(rect, t.radius_lg, Stroke::new(1.0, pal.ring), egui::StrokeKind::Inside);
    let inner = rect.shrink(14.0);
    let hover = app.ui.brush_hud.hover.clone().filter(|h| app.session.tools.current_preset.as_deref() != Some(h.as_str()));
    let shown_name = hover.clone().or_else(|| app.session.tools.current_preset.clone());
    let entry = shown_name.as_ref().and_then(|n| lib.entry(n)).cloned();

    ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
        // Breadcrumb: category › family.
        let head = ui.allocate_exact_size(vec2(inner.width(), 30.0), egui::Sense::hover()).0;
        ui.painter().line_segment([head.left_top(), head.left_bottom()], Stroke::new(3.0, pal.bright));
        let font = FontId::proportional(14.5);
        match &entry {
            Some(e) => {
                let cat = ui.painter().text(head.left_center() + vec2(14.0, 0.0), Align2::LEFT_CENTER, tl!(e.category), font.clone(), t.text);
                let sep = ui.painter().text(cat.right_center() + vec2(10.0, 0.0), Align2::LEFT_CENTER, "›", font.clone(), pal.dim);
                ui.painter().text(sep.right_center() + vec2(10.0, 0.0), Align2::LEFT_CENTER, &e.family, font, pal.bright);
            }
            None => {
                ui.painter().text(head.left_center() + vec2(14.0, 0.0), Align2::LEFT_CENTER, tl!("Brush in hand"), font, t.text);
            }
        }
        ui.add_space(6.0);

        // The live stroke preview, rendered by the brush engine.
        let preview_b: BrushSettings = match &hover {
            Some(h) => lib.entry(h).and_then(|e| app.session.tools.presets.get(e.index)).map_or_else(|| app.session.tools.brush.clone(), |p| p.brush.clone()),
            None => app.session.tools.brush.clone(),
        };
        let pw = inner.width().round().max(80.0);
        let ph = 104.0;
        let (pr, _) = ui.allocate_exact_size(vec2(pw, ph), egui::Sense::hover());
        ui.painter().rect_filled(pr, t.radius, theme::canvas_bg(&t));
        ui.painter().rect_stroke(pr, t.radius, Stroke::new(1.0, if hover.is_some() { pal.bright } else { pal.ring }), egui::StrokeKind::Inside);
        let slot = format!("brush-hud-stroke:{}", if hover.is_some() { "hover" } else { "current" });
        let tex = crate::brush_preview::stroke_texture(ui.ctx(), &slot, &preview_b, pw as u32, ph as u32, t.text);
        ui.painter().image(tex.id(), pr, crate::brush_panel::full_uv(), Color32::WHITE);
        if hover.is_some() {
            ui.painter().text(
                pr.left_bottom() + vec2(10.0, -8.0),
                Align2::LEFT_BOTTOM,
                tl!("PREVIEW · CLICK TO SELECT"),
                FontId::proportional(9.5),
                pal.bright,
            );
        }
        ui.add_space(10.0);
        let title = shown_name.clone().unwrap_or_else(|| tl!("Current brush").to_string());
        ui.label(RichText::new(title).font(theme::semibold(19.0)).color(t.text));
        if let Some(e) = &entry {
            ui.label(RichText::new(tl!(e.description.as_str())).color(t.text_dim));
        }
        ui.add_space(8.0);

        if let Some(h) = &hover {
            // Read-only summary while previewing.
            if let Some(p) = lib.entry(h).and_then(|e| app.session.tools.presets.get(e.index)) {
                let b = &p.brush;
                for (k, v) in [
                    (tl!("Size"), format!("{:.0} px", b.size)),
                    (tl!("Opacity"), format!("{:.0} %", b.opacity * 100.0)),
                    (tl!("Flow"), format!("{:.0} %", b.flow * 100.0)),
                    (tl!("Hardness"), format!("{:.0} %", b.hardness * 100.0)),
                    (tl!("Spacing"), format!("{:.0} %", b.spacing * 100.0)),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(k).color(pal.dim));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(RichText::new(v).color(t.text)));
                    });
                }
                let on: Vec<&str> = SECTIONS
                    .iter()
                    .filter(|(i, _, _)| {
                        let mut c = b.clone();
                        crate::brush_panel::section_flag(&mut c, *i).is_some_and(|f| *f)
                    })
                    .map(|(_, n, _)| *n)
                    .collect();
                if !on.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(on.iter().map(|n| tl!(n)).collect::<Vec<_>>().join(" · ")).color(pal.bright).font(FontId::proportional(11.5)));
                }
            }
            return;
        }

        let before = app.session.tools.brush.clone();
        let mut b = before.clone();
        let presets = &app.session.tools.presets;
        let current = app.session.tools.current_preset.as_deref();
        egui::ScrollArea::vertical()
            .id_salt("brush-hud-panel")
            .auto_shrink([false, false])
            .max_height((inner.bottom() - ui.cursor().top() - 52.0).max(60.0))
            .show(ui, |ui| {
                ui.set_width(inner.width() - 6.0);
                widgets::slider_row(ui, "Size", &mut b.size, 1.0..=MAX_BRUSH_SIZE, " px", None);
                let mut opacity = b.opacity * 100.0;
                if widgets::slider_row(ui, "Opacity", &mut opacity, 0.0..=100.0, " %", None).changed() {
                    b.opacity = opacity / 100.0;
                }
                let mut flow = b.flow * 100.0;
                if widgets::slider_row(ui, "Flow", &mut flow, 0.0..=100.0, " %", None).changed() {
                    b.flow = flow / 100.0;
                }
                egui::CollapsingHeader::new(RichText::new(tl!("Advanced Settings")).color(pal.bright)).id_salt("brush-hud-adv").default_open(true).show(
                    ui,
                    |ui| {
                        widgets::checkbox(ui, &mut b.spacing_enabled, tl!("Auto Spacing"));
                        let mut spacing = b.spacing * 100.0;
                        if widgets::slider_row(ui, "Spacing", &mut spacing, 1.0..=1000.0, " %", None).changed() {
                            b.spacing = spacing / 100.0;
                        }
                        widgets::slider_row(ui, "Angle", &mut b.angle, -180.0..=180.0, "°", None);
                        let mut round = b.roundness * 100.0;
                        if widgets::slider_row(ui, "Roundness", &mut round, 1.0..=100.0, " %", None).changed() {
                            b.roundness = round / 100.0;
                        }
                        let mut hard = b.hardness * 100.0;
                        if widgets::slider_row(ui, "Hardness", &mut hard, 0.0..=100.0, " %", None).changed() {
                            b.hardness = hard / 100.0;
                        }
                    },
                );
                // The dynamics sections: one quiet row each (icon, title, its on/off switch and a
                // chevron); a click opens the section's controls under its row.
                ui.add_space(6.0);
                for (i, title, icon) in SECTIONS {
                    let id = egui::Id::new(("brush-hud-section", i));
                    let open = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
                    let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), egui::Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::CollapsingHeader, true, open, tl!(title)));
                    let p = ui.painter();
                    if resp.hovered() {
                        p.rect_filled(row, t.radius_sm, t.hover);
                    }
                    p.line_segment([row.left_bottom(), row.right_bottom()], Stroke::new(1.0, pal.ring.gamma_multiply(0.5)));
                    crate::icons::paint(ui, Rect::from_center_size(pos2(row.left() + 12.0, row.center().y), vec2(16.0, 16.0)), icon, 15.0, pal.dim);
                    p.text(pos2(row.left() + 30.0, row.center().y), Align2::LEFT_CENTER, tl!(title), FontId::proportional(13.0), t.text);
                    let chevron = if open { "chevron-down" } else { "chevron-right" };
                    crate::icons::paint(ui, Rect::from_center_size(pos2(row.right() - 12.0, row.center().y), vec2(16.0, 16.0)), chevron, 15.0, pal.dim);
                    if let Some(flag) = crate::brush_panel::section_flag(&mut b, i) {
                        let at = Rect::from_center_size(pos2(row.right() - 38.0, row.center().y), vec2(18.0, 18.0));
                        ui.put(at, |ui: &mut Ui| widgets::checkbox(ui, flag, ""));
                    }
                    if resp.clicked() {
                        ui.ctx().data_mut(|d| d.insert_temp(id, !open));
                    }
                    if open {
                        ui.add_space(4.0);
                        if let Some(name) = crate::brush_sections::section_body(ui, &mut b, i, presets, current) {
                            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("brush-hud-set-current"), name));
                        }
                        ui.add_space(6.0);
                    }
                }
            });
        crate::brush_panel::commit_gesture(app, ui.ctx(), &before, &b);
        if let Some(name) = ui.ctx().data_mut(|d| d.remove_temp::<String>(egui::Id::new("brush-hud-set-current"))) {
            crate::brush_panel::run_or_status(app, "brush.presets.setCurrent", serde_json::json!({ "name": name }));
        }

        // Footer: favourite.
        let foot = Rect::from_min_max(pos2(inner.left(), inner.bottom() - 40.0), inner.max);
        ui.scope_builder(UiBuilder::new().max_rect(foot), |ui| {
            ui.horizontal(|ui| {
                if let Some(name) = app.session.tools.current_preset.clone() {
                    let fav = app.ui.brush_hud.favorites.contains(&name);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if fav { tl!("★ Favorited") } else { tl!("☆ Add to Favorites") };
                        if widgets::secondary_button(ui, label, 0.0).clicked() {
                            app.ui.brush_hud.toggle_favorite(&name);
                        }
                    });
                }
            });
        });
    });
}
