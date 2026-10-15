use egui::Modifiers;
use serde_json::json;

use super::*;

fn app() -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.run("file.new", json!({"width": 300, "height": 200, "background": "white"})).unwrap();
    app.sync_views();
    app.ui.tool = Tool::PerspectiveCrop;
    app
}

fn ev(app: &mut PhotocraftApp, kind: &str, p: [f64; 2]) {
    let (x, y) = (p[0], p[1]);
    let e = match kind {
        "down" => ToolEvent::Down { x, y, pressure: 1.0 },
        "up" => ToolEvent::Up { x, y },
        _ => ToolEvent::Move { x, y, pressure: 1.0 },
    };
    crate::canvas::tool_event(app, e, Modifiers::NONE);
}

fn drag(app: &mut PhotocraftApp, a: [f64; 2], b: [f64; 2]) {
    ev(app, "down", a);
    ev(app, "move", [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
    ev(app, "move", b);
    ev(app, "up", b);
}

fn click(app: &mut PhotocraftApp, p: [f64; 2]) {
    ev(app, "down", p);
    ev(app, "up", p);
}

#[test]
fn drag_a_frame_move_a_corner_and_straighten_it() {
    let mut app = app();
    drag(&mut app, [50.0, 40.0], [250.0, 160.0]);
    assert_eq!(app.persp.quad(), Some([[50.0, 40.0], [250.0, 40.0], [250.0, 160.0], [50.0, 160.0]]));
    // The top-right corner moves alone; the bottom side's handle moves both its corners.
    drag(&mut app, [250.0, 40.0], [230.0, 60.0]);
    drag(&mut app, [150.0, 160.0], [150.0, 150.0]);
    assert_eq!(app.persp.quad(), Some([[50.0, 40.0], [230.0, 60.0], [250.0, 150.0], [50.0, 150.0]]));
    let steps = app.session.active().unwrap().history.entries().len();
    commit(&mut app);
    assert!(!app.ui.status_error, "{}", app.ui.status);
    assert!(app.persp.quad().is_none(), "the frame goes once committed");
    let st = app.session.active().unwrap();
    let (w, h) = natural_size(&[[50.0, 40.0], [230.0, 60.0], [250.0, 150.0], [50.0, 150.0]]);
    assert_eq!((st.doc.size.width, st.doc.size.height), (w.round() as u32, h.round() as u32));
    assert_eq!(st.history.entries().last().map(String::as_str), Some("Perspective Crop"));
    assert_eq!(st.history.entries().len(), steps + 1);
}

#[test]
fn four_clicks_in_any_order_make_a_clockwise_frame() {
    let mut app = app();
    for p in [[240.0, 170.0], [60.0, 30.0], [40.0, 160.0], [260.0, 50.0]] {
        assert!(app.persp.quad().is_none());
        click(&mut app, p);
    }
    assert_eq!(app.persp.quad(), Some([[60.0, 30.0], [260.0, 50.0], [240.0, 170.0], [40.0, 160.0]]));
    // Inside, a press moves the whole frame.
    drag(&mut app, [150.0, 100.0], [160.0, 105.0]);
    assert_eq!(app.persp.quad().unwrap()[0], [70.0, 35.0]);
}

#[test]
fn a_twisted_frame_is_refused_and_leaving_the_tool_drops_it() {
    let mut app = app();
    drag(&mut app, [50.0, 40.0], [250.0, 160.0]);
    // Drag the top-right corner past the bottom-left one: the sides cross.
    drag(&mut app, [250.0, 40.0], [20.0, 190.0]);
    let size = app.session.active().unwrap().doc.size;
    commit(&mut app);
    assert!(app.ui.status_error);
    assert_eq!(app.session.active().unwrap().doc.size, size, "nothing changed");
    assert!(app.persp.quad().is_some(), "the frame stays to be fixed");
    app.ui.tool = Tool::Crop;
    end_if_left(&mut app);
    assert!(app.persp.quad().is_none());
}

#[test]
fn width_height_and_resolution_fields_set_the_result() {
    let mut app = app();
    front_image(&mut app);
    assert_eq!((app.ui.tool_options.persp_width.as_str(), app.ui.tool_options.persp_height.as_str()), ("300 px", "200 px"));
    app.ui.tool_options.persp_width = "120 px".into();
    app.ui.tool_options.persp_height = "80 px".into();
    app.ui.tool_options.persp_resolution = "150".into();
    let p = commit_params(&app, [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]);
    assert_eq!((p["width"].clone(), p["height"].clone(), p["resolution"].clone()), (json!(120.0), json!(80.0), json!(150.0)));
    drag(&mut app, [50.0, 40.0], [250.0, 160.0]);
    commit(&mut app);
    let d = &app.session.active().unwrap().doc;
    assert_eq!((d.size.width, d.size.height, d.resolution_dpi), (120, 80, 150.0));
    assert_eq!(Tool::PerspectiveCrop.key(), 'C');
    assert!(crate::icons::exists(crate::icons::tool_icon(Tool::PerspectiveCrop)));
}
