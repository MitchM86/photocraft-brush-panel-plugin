//! Path Selection (A) works on whole path components, as in Photoshop: select one by clicking it
//! or inside it, ⇧ adds, a marquee selects what it touches, ⌥-drag copies, ⌫ deletes.

use egui::Modifiers;
use photocraft_doc::vector::Path;
use photocraft_geom::Point;
use serde_json::json;

use super::PathSelection;
use crate::PhotocraftApp;
use crate::canvas::{ToolEvent, tool_event};
use crate::direct_select::PathRef;
use crate::state::Tool;

fn app() -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.session.execute("file.new", json!({"width": 200, "height": 200})).unwrap();
    app.sync_views();
    app.ui.tool = Tool::PathSelection;
    app
}

fn square(x: f64, y: f64) -> serde_json::Value {
    json!({"closed": true, "knots": [[x, y], [x + 40.0, y], [x + 40.0, y + 40.0], [x, y + 40.0]]})
}

/// A work path of two squares: A at (10, 10) and B at (120, 120), 40 px each.
fn with_two(app: &mut PhotocraftApp) {
    app.run("path.set", json!({"name": "work", "path": {"subpaths": [square(10.0, 10.0), square(120.0, 120.0)]}})).unwrap();
}

fn work(app: &PhotocraftApp) -> Path {
    app.session.active().unwrap().doc.work_path.clone().unwrap()
}

/// The first anchor of each subpath.
fn corners(app: &PhotocraftApp) -> Vec<Point> {
    work(app).subpaths.iter().map(|s| s.knots[0].anchor).collect()
}

fn steps(app: &PhotocraftApp) -> usize {
    app.session.active().unwrap().history.entries().len()
}

fn drag(app: &mut PhotocraftApp, from: [f64; 2], to: [f64; 2], mods: Modifiers) {
    tool_event(app, ToolEvent::Down { x: from[0], y: from[1], pressure: 1.0 }, mods);
    let mid = [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0];
    tool_event(app, ToolEvent::Move { x: mid[0], y: mid[1], pressure: 1.0 }, mods);
    tool_event(app, ToolEvent::Move { x: to[0], y: to[1], pressure: 1.0 }, mods);
    tool_event(app, ToolEvent::Up { x: to[0], y: to[1] }, mods);
}

fn click(app: &mut PhotocraftApp, at: [f64; 2], mods: Modifiers) {
    drag(app, at, at, mods);
}

fn sel(app: &PhotocraftApp) -> Vec<usize> {
    app.ui.path_selection.subpaths.clone()
}

#[test]
fn dragging_a_component_moves_only_it_in_one_step() {
    let mut app = app();
    with_two(&mut app);
    let before = steps(&app);
    drag(&mut app, [140.0, 140.0], [150.0, 130.0], Modifiers::NONE);
    assert_eq!(corners(&app), vec![Point::new(10.0, 10.0), Point::new(130.0, 110.0)], "only B moved (dragged from inside it)");
    assert_eq!(steps(&app), before + 1);
    assert_eq!(app.ui.path_selection, PathSelection { target: Some(PathRef::Work), subpaths: vec![1], drag: None });
    // Dragging on empty canvas never moves the path.
    drag(&mut app, [90.0, 190.0], [100.0, 195.0], Modifiers::NONE);
    assert_eq!(corners(&app), vec![Point::new(10.0, 10.0), Point::new(130.0, 110.0)]);
}

#[test]
fn shift_adds_and_removes_and_the_selection_moves_together() {
    let mut app = app();
    with_two(&mut app);
    click(&mut app, [30.0, 10.0], Modifiers::NONE);
    assert_eq!(sel(&app), vec![0], "a click on the outline selects");
    click(&mut app, [130.0, 130.0], Modifiers::SHIFT);
    assert_eq!(sel(&app), vec![0, 1], "⇧ adds");
    drag(&mut app, [30.0, 30.0], [35.0, 30.0], Modifiers::NONE);
    assert_eq!(corners(&app), vec![Point::new(15.0, 10.0), Point::new(125.0, 120.0)], "both move");
    click(&mut app, [30.0, 30.0], Modifiers::SHIFT);
    assert_eq!(sel(&app), vec![1], "⇧ on a selected one removes it");
    click(&mut app, [90.0, 90.0], Modifiers::NONE);
    assert!(sel(&app).is_empty(), "a click on empty canvas deselects");
}

#[test]
fn shift_drag_keeps_the_move_at_45_degree_steps() {
    let mut app = app();
    with_two(&mut app);
    drag(&mut app, [30.0, 30.0], [70.0, 33.0], Modifiers::SHIFT);
    assert_eq!(corners(&app)[0], Point::new(50.0, 10.0), "horizontal");
}

#[test]
fn a_marquee_selects_the_components_it_touches() {
    let mut app = app();
    with_two(&mut app);
    drag(&mut app, [100.0, 100.0], [125.0, 125.0], Modifiers::NONE);
    assert_eq!(sel(&app), vec![1]);
    drag(&mut app, [0.0, 0.0], [12.0, 12.0], Modifiers::SHIFT);
    assert_eq!(sel(&app), vec![1, 0], "⇧ adds");
    assert_eq!(corners(&app), vec![Point::new(10.0, 10.0), Point::new(120.0, 120.0)], "nothing moved");
}

#[test]
fn alt_drag_moves_a_copy_and_selects_it() {
    let mut app = app();
    with_two(&mut app);
    let before = steps(&app);
    drag(&mut app, [30.0, 30.0], [30.0, 80.0], Modifiers::ALT);
    assert_eq!(corners(&app), vec![Point::new(10.0, 10.0), Point::new(120.0, 120.0), Point::new(10.0, 60.0)]);
    assert_eq!((sel(&app), steps(&app)), (vec![2], before + 1));
}

#[test]
fn delete_removes_the_selected_components() {
    let mut app = app();
    with_two(&mut app);
    click(&mut app, [130.0, 130.0], Modifiers::NONE);
    assert!(super::delete_selected(&mut app));
    assert_eq!(corners(&app), vec![Point::new(10.0, 10.0)]);
    assert!(!super::delete_selected(&mut app), "nothing selected: the key does what it otherwise would");
    click(&mut app, [30.0, 30.0], Modifiers::NONE);
    assert!(super::delete_selected(&mut app));
    assert!(app.session.active().unwrap().doc.work_path.is_none(), "all of the work path deletes it");
}

#[test]
fn a_live_shape_moved_whole_stays_live() {
    let mut app = app();
    app.run("shape.create", json!({"kind": "rect", "rect": [20, 20, 40, 30], "fill": "#ff0000"})).unwrap();
    drag(&mut app, [40.0, 35.0], [50.0, 45.0], Modifiers::NONE);
    let info = app.run("shape.info", json!({})).unwrap();
    assert_eq!(app.session.journal.last().map(|(id, _)| id.as_str()), Some("shape.edit"));
    assert!(info["live"].is_object(), "still a live rectangle: {info}");
}
