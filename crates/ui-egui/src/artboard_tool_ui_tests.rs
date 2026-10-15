use egui::Modifiers;
use serde_json::json;

use super::*;

fn app() -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.run("file.new", json!({"width": 800, "height": 600, "background": "white"})).unwrap();
    app.sync_views();
    app.ui.tool = Tool::Artboard;
    app
}

fn drag(app: &mut PhotocraftApp, a: [f64; 2], b: [f64; 2]) {
    let n = Modifiers::NONE;
    crate::canvas::tool_event(app, ToolEvent::Down { x: a[0], y: a[1], pressure: 1.0 }, n);
    crate::canvas::tool_event(app, ToolEvent::Move { x: (a[0] + b[0]) / 2.0, y: (a[1] + b[1]) / 2.0, pressure: 1.0 }, n);
    crate::canvas::tool_event(app, ToolEvent::Move { x: b[0], y: b[1], pressure: 1.0 }, n);
    crate::canvas::tool_event(app, ToolEvent::Up { x: b[0], y: b[1] }, n);
    assert!(!app.ui.status_error, "{}", app.ui.status);
}

fn boards(app: &PhotocraftApp) -> Vec<[i32; 4]> {
    app.session.active().unwrap().doc.artboards().iter().map(|b| [b.2.rect.x0, b.2.rect.y0, b.2.rect.x1, b.2.rect.y1]).collect()
}

#[test]
fn draw_move_and_resize_a_board() {
    let mut app = app();
    drag(&mut app, [100.0, 80.0], [300.2, 249.6]);
    assert_eq!(boards(&app), vec![[100, 80, 300, 250]]);
    assert!(selected(&app).is_some(), "the new board is selected");
    // A pixel layer inside it moves with it.
    let st = app.session.active().unwrap();
    let board = st.active_layer.unwrap();
    app.run("layer.new.layer", json!({})).unwrap();
    app.run("select.rect", json!({"x": 150, "y": 100, "width": 20, "height": 20})).unwrap();
    app.run("edit.fill", json!({"color": "#ff0000"})).unwrap();
    app.run("select.deselect", json!({})).unwrap();
    let inside = app.session.active().unwrap().active_layer.unwrap();
    app.run("layer.moveTo", json!({"layer": inside.0, "target": board.0, "position": "into"})).unwrap();
    app.session.select_layer(board).unwrap();
    assert_eq!(app.session.active().unwrap().doc.artboard_of(inside), Some(board), "drawn into the board");
    drag(&mut app, [200.0, 200.0], [250.0, 220.0]);
    assert_eq!(boards(&app), vec![[150, 100, 350, 270]]);
    let b = app.session.active().unwrap().doc.layer(inside).unwrap().surface().unwrap().content_bounds();
    assert_eq!([b.x0, b.y0], [200, 120], "the contents moved with the board");
    // The right edge's handle resizes; the contents stay.
    drag(&mut app, [350.0, 185.0], [400.0, 185.0]);
    assert_eq!(boards(&app), vec![[150, 100, 400, 270]]);
    let b = app.session.active().unwrap().doc.layer(inside).unwrap().surface().unwrap().content_bounds();
    assert_eq!([b.x0, b.y0], [200, 120]);
}

#[test]
fn plus_buttons_add_boards_beside_and_presets_resize() {
    let mut app = app();
    drag(&mut app, [0.0, 0.0], [200.0, 100.0]);
    let (_, r) = selected(&app).unwrap();
    let z = f64::from(app.point_zoom());
    let at = plus_at(r, Side::Right, z);
    crate::canvas::tool_event(&mut app, ToolEvent::Down { x: at[0], y: at[1], pressure: 1.0 }, Modifiers::NONE);
    crate::canvas::tool_event(&mut app, ToolEvent::Up { x: at[0], y: at[1] }, Modifiers::NONE);
    assert_eq!(boards(&app), vec![[0, 0, 200, 100], [300, 0, 500, 100]]);
    // Nothing fits left of a board at x = 0.
    assert_eq!(beside([0.0, 0.0, 200.0, 100.0], Side::Left), None);
    assert_eq!(beside([0.0, 0.0, 200.0, 100.0], Side::Bottom), Some([0.0, 200.0, 200.0, 300.0]));
    let (id, _) = selected(&app).unwrap();
    app.run("layer.artboard.set", json!({"layer": id.0, "preset": "Instagram Post", "moveContents": false})).unwrap();
    assert_eq!(boards(&app)[1], [300, 0, 1380, 1080]);
    assert_eq!(Tool::Artboard.key(), 'V');
    assert!(crate::icons::exists(crate::icons::tool_icon(Tool::Artboard)));
}

#[test]
fn a_click_on_the_pasteboard_draws_nothing() {
    let mut app = app();
    drag(&mut app, [100.0, 100.0], [101.0, 100.5]);
    assert!(boards(&app).is_empty());
}
