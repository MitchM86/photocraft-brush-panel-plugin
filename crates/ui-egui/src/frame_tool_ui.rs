//! Frame Tool (K), as in Photoshop: drag a rectangular or elliptical frame (options bar; ⇧ keeps
//! it square or round, ⌥ draws it from the centre). Drawn over a selected image layer, that
//! layer goes into the frame and is clipped to it; an empty frame shows a grey placeholder with a
//! cross until an image is placed in it (File › Place Embedded or a dropped file, while the frame
//! is selected, fills it: `file_open::place_params`). The frame is `layer.new.frame`.

use egui::{Color32, CursorIcon, Modifiers, Pos2, Stroke};
use photocraft_doc::{Layer, LayerContent};
use serde_json::json;

use crate::PhotocraftApp;
use crate::canvas::{ToolEvent, ViewXform};
use crate::state::Tool;

#[derive(Clone, Debug, Default)]
pub struct State {
    /// The frame being drawn: where the drag started, where it is, and its modifiers.
    drag: Option<([f64; 2], [f64; 2], Modifiers)>,
}

/// The frame a drag from `a` to `b` draws: ⇧ square / round, ⌥ from the centre.
pub fn drawn(a: [f64; 2], b: [f64; 2], mods: Modifiers) -> [f64; 4] {
    let (mut dx, mut dy) = (b[0] - a[0], b[1] - a[1]);
    if mods.shift {
        let s = dx.abs().max(dy.abs());
        (dx, dy) = (s * dx.signum(), s * dy.signum());
    }
    let r = if mods.alt { [a[0] - dx, a[1] - dy, a[0] + dx, a[1] + dy] } else { [a[0], a[1], a[0] + dx, a[1] + dy] };
    [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]
}

fn ellipse(app: &PhotocraftApp) -> bool {
    app.ui.tool_options.frame_shape == "ellipse"
}

/// The active layer, when it can go into a frame drawn now (an image layer, not the Background).
fn content(app: &PhotocraftApp) -> Option<u64> {
    let st = app.session.active()?;
    let l = st.doc.layer(st.active_layer?)?;
    let image = matches!(l.content, LayerContent::Raster(_) | LayerContent::Smart(_));
    (image && !(l.name == "Background" && l.locks.position && l.locks.transparency)).then_some(l.id.0)
}

/// Pointer input for the tool. Returns true when the event was its.
pub fn pointer(app: &mut PhotocraftApp, ev: ToolEvent, mods: Modifiers) -> bool {
    if app.ui.tool != Tool::Frame {
        app.frame_tool.drag = None;
        return false;
    }
    let p = match ev {
        ToolEvent::Down { x, y, .. } | ToolEvent::Move { x, y, .. } | ToolEvent::Up { x, y } => [x, y],
    };
    if app.session.active().is_none() || !p[0].is_finite() || !p[1].is_finite() {
        return true;
    }
    match ev {
        ToolEvent::Down { .. } => app.frame_tool.drag = Some((p, p, mods)),
        ToolEvent::Move { .. } => {
            if let Some(d) = app.frame_tool.drag.as_mut() {
                (d.1, d.2) = (p, mods);
            }
        }
        ToolEvent::Up { .. } => {
            let Some((a, _, _)) = app.frame_tool.drag.take() else { return true };
            let r = drawn(a, p, mods);
            let min = 2.0 / f64::from(app.point_zoom()).max(0.01);
            if r[2] - r[0] < min || r[3] - r[1] < min {
                return true;
            }
            let rect = [r[0].round(), r[1].round(), (r[2] - r[0]).round().max(1.0), (r[3] - r[1]).round().max(1.0)];
            let mut params = json!({"rect": rect, "shape": if ellipse(app) { "ellipse" } else { "rectangle" }});
            if let Some(c) = content(app) {
                params["content"] = json!(c);
            }
            if let Err(e) = app.run("layer.new.frame", params) {
                app.ui.status = e;
                app.ui.status_error = true;
            }
        }
    }
    true
}

pub fn cursor() -> CursorIcon {
    CursorIcon::Crosshair
}

/// The outline of frame box `r` on screen: a rectangle or an ellipse.
fn outline(r: [f64; 4], ellipse: bool, scr: &impl Fn([f64; 2]) -> Pos2) -> Vec<Pos2> {
    if !ellipse {
        return vec![scr([r[0], r[1]]), scr([r[2], r[1]]), scr([r[2], r[3]]), scr([r[0], r[3]])];
    }
    let (cx, cy, rx, ry) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0, (r[2] - r[0]) / 2.0, (r[3] - r[1]) / 2.0);
    (0..64).map(|i| f64::from(i) / 64.0 * std::f64::consts::TAU).map(|a| scr([cx + rx * a.cos(), cy + ry * a.sin()])).collect()
}

fn is_ellipse(l: &Layer) -> bool {
    matches!(&l.content, LayerContent::Group(g) if g.frame == Some(photocraft_doc::FrameShape::Ellipse))
}

/// Every empty frame as a grey placeholder with a cross (View › Extras on); the selected frame's
/// outline; and, with the Frame tool, the frame being drawn.
pub fn draw(app: &PhotocraftApp, painter: &egui::Painter, xf: &ViewXform) {
    let Some(st) = app.session.active() else { return };
    let t = crate::theme::Tokens::get(painter.ctx());
    let scr = |p: [f64; 2]| xf.to_screen(p[0] as f32, p[1] as f32);
    let selected = st.active_layer.and_then(|id| photocraft_engine::frame_cmds::frame_of(&st.doc, id));
    if app.ui.view.extras {
        for (_, _, l) in st.doc.walk() {
            let Some(r) = photocraft_engine::frame_cmds::frame_rect(l) else { continue };
            let empty = matches!(&l.content, LayerContent::Group(g) if g.children.is_empty());
            if !l.visible || (!empty && selected != Some(l.id)) {
                continue;
            }
            let pts = outline(r, is_ellipse(l), &scr);
            if empty {
                painter.add(egui::Shape::convex_polygon(pts.clone(), Color32::from_rgba_unmultiplied(200, 200, 200, 110), Stroke::NONE));
                let cross = Stroke::new(1.0, Color32::from_gray(150));
                painter.line_segment([scr([r[0], r[1]]), scr([r[2], r[3]])], cross);
                painter.line_segment([scr([r[2], r[1]]), scr([r[0], r[3]])], cross);
            }
            let line = if selected == Some(l.id) { Stroke::new(1.5, t.accent) } else { Stroke::new(1.0, Color32::from_gray(150)) };
            painter.add(egui::Shape::closed_line(pts, line));
        }
    }
    if app.ui.tool == Tool::Frame
        && let Some((a, b, mods)) = app.frame_tool.drag
    {
        let pts = outline(drawn(a, b, mods), ellipse(app), &scr);
        painter.add(egui::Shape::closed_line(pts, Stroke::new(1.5, t.accent)));
    }
}

/// The options bar: rectangular or elliptical frame, and what to do.
pub fn options_bar(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let o = &mut app.ui.tool_options;
    for (shape, icon, tip) in [("rectangle", "square", tl!("Create a new rectangular frame")), ("ellipse", "circle", tl!("Create a new elliptical frame"))] {
        if crate::icons::button(ui, icon, 24.0, o.frame_shape == shape, tip).clicked() {
            o.frame_shape = shape.into();
        }
    }
    crate::widgets::vline(ui, 22.0);
    let dim = crate::theme::Tokens::get(ui.ctx()).text_dim;
    ui.label(egui::RichText::new(tl!("Drag a frame · over a selected image it clips the image · Place Embedded or drop a file to fill the selected frame")).color(dim).size(12.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> PhotocraftApp {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 300, "height": 200, "background": "white"})).unwrap();
        app.sync_views();
        app.ui.tool = Tool::Frame;
        app
    }

    fn drag(app: &mut PhotocraftApp, a: [f64; 2], b: [f64; 2], m: Modifiers) {
        crate::canvas::tool_event(app, ToolEvent::Down { x: a[0], y: a[1], pressure: 1.0 }, m);
        crate::canvas::tool_event(app, ToolEvent::Move { x: b[0], y: b[1], pressure: 1.0 }, m);
        crate::canvas::tool_event(app, ToolEvent::Up { x: b[0], y: b[1] }, m);
        assert!(!app.ui.status_error, "{}", app.ui.status);
    }

    #[test]
    fn drag_draws_frames_and_takes_in_the_selected_image() {
        let mut app = app();
        // Over the Background: an empty frame.
        drag(&mut app, [20.0, 20.0], [80.0, 60.0], Modifiers::NONE);
        let st = app.session.active().unwrap();
        let f = st.active_layer.unwrap();
        assert_eq!(photocraft_engine::frame_cmds::frame_rect(st.doc.layer(f).unwrap()), Some([20.0, 20.0, 80.0, 60.0]));
        assert!(matches!(&st.doc.layer(f).unwrap().content, LayerContent::Group(g) if g.children.is_empty()));
        // Over a selected image layer: it goes in. ⇧ makes it a circle, ⌥ from the centre.
        app.run("layer.new.layer", json!({})).unwrap();
        app.run("edit.fill", json!({"color": "#ff0000"})).unwrap();
        let img = app.session.active().unwrap().active_layer.unwrap();
        app.ui.tool_options.frame_shape = "ellipse".into();
        drag(&mut app, [150.0, 100.0], [190.0, 120.0], Modifiers { shift: true, alt: true, ..Modifiers::NONE });
        let st = app.session.active().unwrap();
        let f = st.active_layer.unwrap();
        assert_eq!(photocraft_engine::frame_cmds::frame_of(&st.doc, img), Some(f));
        assert_eq!(photocraft_engine::frame_cmds::frame_rect(st.doc.layer(f).unwrap()), Some([110.0, 60.0, 190.0, 140.0]));
        assert!(is_ellipse(st.doc.layer(f).unwrap()));
        // A click draws nothing.
        let n = app.session.active().unwrap().doc.walk().len();
        drag(&mut app, [10.0, 10.0], [10.5, 10.0], Modifiers::NONE);
        assert_eq!(app.session.active().unwrap().doc.walk().len(), n);
        assert_eq!(Tool::Frame.key(), 'K');
        assert!(crate::icons::exists(crate::icons::tool_icon(Tool::Frame)));
    }

    #[test]
    fn placing_with_a_frame_selected_fills_it() {
        let mut app = app();
        drag(&mut app, [20.0, 20.0], [80.0, 60.0], Modifiers::NONE);
        let f = app.session.active().unwrap().active_layer.unwrap();
        assert_eq!(app.place_params(), json!({"frame": f.0}));
        app.ui.tool = Tool::Move;
        let bg = app.session.active().unwrap().doc.layers[0].id;
        app.session.select_layer(bg).unwrap();
        assert_eq!(app.place_params(), json!({}), "no frame selected");
    }
}
