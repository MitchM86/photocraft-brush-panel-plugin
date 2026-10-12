//! Color Sampler Tool (I), as in Photoshop: click to place up to ten numbered samplers whose
//! colours the Info panel reads out; drag one to move it (off the image deletes it), ⌥-click to
//! delete it; the options bar's Sample Size is the Eyedropper's, and Clear All removes them all.
//! The samplers are the engine's (`sampler.*` commands), shown with any tool while View › Extras
//! is on.

use egui::{Align2, Color32, FontId, Modifiers, Pos2, Stroke, vec2};
use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::canvas::ToolEvent;
use crate::state::Tool;

/// A sampler being dragged: its index and where it is now (document px).
pub type SamplerDrag = (usize, [f64; 2]);

/// What the Info panel's readouts were read for (document, revision, samplers, Sample Size).
type InfoKey = (u64, u64, Vec<[f64; 2]>, u32);

/// The Info panel's sampler readouts (`#n`, colour) and what they were read for.
pub type InfoCache = (InfoKey, Vec<(usize, [f32; 4])>);

fn samplers(app: &PhotocraftApp) -> Vec<[f64; 2]> {
    app.session.active().map(|st| st.color_samplers.clone()).unwrap_or_default()
}

fn inside(app: &PhotocraftApp, p: [f64; 2]) -> bool {
    app.session.active().is_some_and(|st| p[0] >= 0.0 && p[1] >= 0.0 && p[0] < f64::from(st.doc.size.width) && p[1] < f64::from(st.doc.size.height))
}

fn report(app: &mut PhotocraftApp, r: Result<Value, String>) -> Option<Value> {
    r.map_err(|e| {
        app.ui.status = e;
        app.ui.status_error = true;
    })
    .ok()
}

/// Canvas pointer events while the Color Sampler is the tool (it takes them all, ⌘ aside).
pub fn pointer(app: &mut PhotocraftApp, ev: ToolEvent, mods: Modifiers) -> bool {
    if app.ui.tool != Tool::ColorSampler || (mods.command && app.ui.sampler_drag.is_none()) {
        return false;
    }
    let tol = 6.0 / f64::from(app.point_zoom().max(0.01));
    match ev {
        ToolEvent::Down { x, y, .. } => {
            let p = [x, y];
            let hit = samplers(app).iter().rposition(|q| (q[0] - x).hypot(q[1] - y) <= tol);
            match hit {
                Some(i) if mods.alt => {
                    let r = app.run("sampler.delete", json!({"index": i}));
                    report(app, r);
                }
                Some(i) => app.ui.sampler_drag = Some((i, p)),
                None if inside(app, p) => {
                    let r = app.run("sampler.add", json!({"x": x, "y": y}));
                    if let Some(i) = report(app, r).and_then(|r| r["index"].as_u64()) {
                        app.ui.sampler_drag = Some((i as usize, p));
                    }
                }
                None => {}
            }
        }
        ToolEvent::Move { x, y, .. } => {
            if let Some((_, at)) = app.ui.sampler_drag.as_mut() {
                *at = [x, y];
            }
        }
        ToolEvent::Up { x, y } => {
            if let Some((i, _)) = app.ui.sampler_drag.take() {
                let moved = samplers(app).get(i).is_some_and(|q| (q[0] - x).abs() + (q[1] - y).abs() > 1e-9);
                // Dragged off the image, a sampler goes, as in Photoshop.
                let r = if !inside(app, [x, y]) {
                    Some(app.run("sampler.delete", json!({"index": i})))
                } else if moved {
                    Some(app.run("sampler.move", json!({"index": i, "x": x, "y": y})))
                } else {
                    None
                };
                if let Some(r) = r {
                    report(app, r);
                }
            }
        }
    }
    true
}

/// The samplers as Photoshop draws them: a ring with a cross and its number.
pub fn draw(app: &PhotocraftApp, painter: &egui::Painter, to_scr: &dyn Fn([f64; 2]) -> Pos2) {
    if !app.ui.view.shows(true) {
        return;
    }
    for (i, mut p) in samplers(app).into_iter().enumerate() {
        if let Some((_, at)) = app.ui.sampler_drag.filter(|(k, _)| *k == i) {
            p = at;
        }
        let c = to_scr(p);
        for (w, col) in [(3.0, Color32::from_black_alpha(160)), (1.2, Color32::WHITE)] {
            let s = Stroke::new(w, col);
            painter.circle_stroke(c, 5.0, s);
            painter.line_segment([c - vec2(9.0, 0.0), c + vec2(9.0, 0.0)], s);
            painter.line_segment([c - vec2(0.0, 9.0), c + vec2(0.0, 9.0)], s);
        }
        let label = (i + 1).to_string();
        let at = c + vec2(8.0, 8.0);
        painter.text(at + vec2(1.0, 1.0), Align2::LEFT_TOP, &label, FontId::proportional(11.0), Color32::BLACK);
        painter.text(at, Align2::LEFT_TOP, &label, FontId::proportional(11.0), Color32::WHITE);
    }
}

/// Options bar: the Eyedropper's Sample Size, and Clear All.
pub fn options_bar(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(tl!("Sample Size:")).color(t.text_dim).size(12.0));
    let sizes: Vec<(u32, &str)> = crate::eyedropper_ui::size_options().iter().map(|(v, l)| (*v, tl!(l))).collect();
    crate::widgets::dropdown(ui, "sampler-size", &mut app.ui.tool_options.eyedropper_size, &sizes, 130.0);
    crate::widgets::vline(ui, 22.0);
    let any = app.session.is_enabled("sampler.clearAll");
    if ui.add_enabled(any, egui::Button::new(tl!("Clear All"))).clicked() {
        let r = app.run("sampler.clearAll", json!({}));
        report(app, r);
    }
}

/// The samplers' colours for the Info panel: `#n` and R G B each, read again only when the
/// document, a sampler or the Sample Size changed.
pub fn info_rows(app: &mut PhotocraftApp) -> Vec<(usize, [f32; 4])> {
    let Some(st) = app.session.active() else { return Vec::new() };
    if st.color_samplers.is_empty() {
        return Vec::new();
    }
    let size = app.ui.tool_options.eyedropper_size;
    let key = (st.doc.id.0, st.revision, st.color_samplers.clone(), size);
    if let Some((k, v)) = &app.sampler_info
        && *k == key
    {
        return v.clone();
    }
    let rows: Vec<(usize, [f32; 4])> = app
        .session
        .execute("sampler.list", json!({"size": size}))
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let c: Vec<f32> = serde_json::from_value(s["color"].clone()).ok()?;
            Some((i + 1, [*c.first()?, *c.get(1)?, *c.get(2)?, *c.get(3)?]))
        })
        .collect();
    app.sampler_info = Some((key, rows.clone()));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::tool_event;

    fn app() -> PhotocraftApp {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 100, "height": 80, "background": "white"})).unwrap();
        app.sync_views();
        app.ui.tool = Tool::ColorSampler;
        app
    }

    fn drag(app: &mut PhotocraftApp, from: [f64; 2], to: [f64; 2], m: Modifiers) {
        tool_event(app, ToolEvent::Down { x: from[0], y: from[1], pressure: 1.0 }, m);
        tool_event(app, ToolEvent::Move { x: to[0], y: to[1], pressure: 1.0 }, m);
        tool_event(app, ToolEvent::Up { x: to[0], y: to[1] }, m);
    }

    #[test]
    fn clicks_place_drags_move_alt_and_dragging_off_delete() {
        let mut app = app();
        drag(&mut app, [10.0, 10.0], [10.0, 10.0], Modifiers::NONE);
        drag(&mut app, [50.0, 40.0], [60.0, 45.0], Modifiers::NONE);
        assert_eq!(samplers(&app), vec![[10.0, 10.0], [60.0, 45.0]], "a press places one, dragging places it where released");
        drag(&mut app, [60.0, 45.0], [70.0, 20.0], Modifiers::NONE);
        assert_eq!(samplers(&app)[1], [70.0, 20.0], "dragging a sampler moves it");
        assert_eq!(info_rows(&mut app).iter().map(|r| r.0).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(info_rows(&mut app)[0].1, [1.0; 4], "white under #1");
        drag(&mut app, [10.0, 10.0], [10.0, 10.0], Modifiers::ALT);
        assert_eq!(samplers(&app).len(), 1, "⌥-click deletes");
        drag(&mut app, [70.0, 20.0], [150.0, 20.0], Modifiers::NONE);
        assert!(samplers(&app).is_empty(), "dragged off the image");
        assert!(app.drag.is_none() && app.live_stroke.is_none());
        assert_eq!(app.session.active().unwrap().history.entries().len(), 1, "no history steps");
    }
}
