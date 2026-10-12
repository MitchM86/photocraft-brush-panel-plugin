//! The Pen's siblings: Freeform Pen, Curvature Pen, Add / Delete Anchor Point and Convert Point.

use egui::Modifiers;
use photocraft_doc::vector::Path;
use photocraft_geom::Point;
use serde_json::json;

use super::*;
use crate::canvas::{ToolEvent, tool_event};

fn app(tool: Tool) -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.session.execute("file.new", json!({"width": 200, "height": 200})).unwrap();
    app.sync_views();
    app.ui.tool = tool;
    app
}

/// A square work path: (40, 40), (120, 40), (120, 120), (40, 120).
fn with_square(app: &mut PhotocraftApp) {
    app.run("path.set", json!({"name": "work", "path": {"subpaths": [{"closed": true, "knots": [[40, 40], [120, 40], [120, 120], [40, 120]]}]}})).unwrap();
}

fn work(app: &PhotocraftApp) -> Option<Path> {
    app.session.active().unwrap().doc.work_path.clone()
}

fn anchors(app: &PhotocraftApp) -> Vec<Point> {
    work(app).unwrap().subpaths[0].knots.iter().map(|k| k.anchor).collect()
}

fn steps(app: &PhotocraftApp) -> usize {
    app.session.active().unwrap().history.entries().len()
}

fn drag_through(app: &mut PhotocraftApp, pts: &[[f64; 2]], mods: Modifiers) {
    let (Some(first), Some(last)) = (pts.first(), pts.last()) else { return };
    tool_event(app, ToolEvent::Down { x: first[0], y: first[1], pressure: 1.0 }, mods);
    for p in &pts[1..] {
        tool_event(app, ToolEvent::Move { x: p[0], y: p[1], pressure: 1.0 }, mods);
    }
    tool_event(app, ToolEvent::Up { x: last[0], y: last[1] }, mods);
}

fn click(app: &mut PhotocraftApp, at: [f64; 2], mods: Modifiers) {
    drag_through(app, &[at], mods);
}

#[test]
fn add_anchor_point_splits_a_segment_and_alt_deletes() {
    let mut app = app(Tool::AddAnchorPoint);
    with_square(&mut app);
    let n = steps(&app);
    click(&mut app, [80.0, 40.5], Modifiers::NONE);
    let a = anchors(&app);
    assert_eq!(a.len(), 5);
    assert!((a[1].x - 80.0).abs() < 1.0 && a[1].y == 40.0, "a new anchor on the top edge: {:?}", a[1]);
    assert_eq!(steps(&app), n + 1, "a click is one step");
    assert_eq!(app.ui.direct_selection.anchors, vec![[0, 1]], "the new anchor is selected");
    // ⌥ is Delete Anchor Point.
    click(&mut app, [a[1].x, 40.0], Modifiers::ALT);
    assert_eq!(anchors(&app).len(), 4);
    // Clicking empty canvas does nothing, and no brush stroke starts.
    click(&mut app, [10.0, 180.0], Modifiers::NONE);
    assert_eq!((anchors(&app).len(), steps(&app)), (4, n + 2));
    assert!(app.drag.is_none() && app.live_stroke.is_none());
}

#[test]
fn dragging_from_a_segment_adds_an_anchor_and_moves_it() {
    let mut app = app(Tool::AddAnchorPoint);
    with_square(&mut app);
    drag_through(&mut app, &[[120.0, 80.0], [140.0, 80.0], [150.0, 80.0]], Modifiers::NONE);
    let a = anchors(&app);
    assert_eq!(a.len(), 5);
    assert_eq!(a[2], Point::new(150.0, 80.0), "the new anchor followed the drag");
    assert!(app.ui.direct_selection.drag.is_none());
}

#[test]
fn delete_anchor_point_removes_anchors_and_last_one_takes_the_work_path() {
    let mut app = app(Tool::DeleteAnchorPoint);
    with_square(&mut app);
    click(&mut app, [120.0, 120.0], Modifiers::NONE);
    assert_eq!(anchors(&app), vec![Point::new(40.0, 40.0), Point::new(120.0, 40.0), Point::new(40.0, 120.0)]);
    // A segment click does nothing; ⌥ adds.
    click(&mut app, [80.0, 40.0], Modifiers::NONE);
    assert_eq!(anchors(&app).len(), 3);
    click(&mut app, [80.0, 40.0], Modifiers::ALT);
    assert_eq!(anchors(&app).len(), 4);
    for p in anchors(&app) {
        click(&mut app, [p.x, p.y], Modifiers::NONE);
    }
    assert!(work(&app).is_none(), "deleting the last anchor deletes the work path");
}

#[test]
fn convert_point_tool_toggles_corners_and_smooth_points() {
    let mut app = app(Tool::ConvertPoint);
    with_square(&mut app);
    drag_through(&mut app, &[[120.0, 40.0], [130.0, 40.0], [140.0, 40.0]], Modifiers::NONE);
    let k = work(&app).unwrap().subpaths[0].knots[1];
    assert_eq!((k.smooth, k.out_ctrl, k.in_ctrl), (true, Point::new(140.0, 40.0), Point::new(100.0, 40.0)));
    click(&mut app, [120.0, 40.0], Modifiers::NONE);
    let k = work(&app).unwrap().subpaths[0].knots[1];
    assert_eq!((k.smooth, k.out_ctrl), (false, k.anchor), "a click makes a corner");
    let n = steps(&app);
    click(&mut app, [10.0, 180.0], Modifiers::NONE);
    assert_eq!(steps(&app), n);
    assert!(app.drag.is_none() && app.live_stroke.is_none(), "a miss paints nothing");
}

#[test]
fn freeform_pen_fits_the_stroke_and_closes_over_its_start() {
    let mut app = app(Tool::FreeformPen);
    let arc: Vec<[f64; 2]> = (0..=90).map(|i| f64::from(i).to_radians()).map(|a| [100.0 + 60.0 * a.cos(), 100.0 + 60.0 * a.sin()]).collect();
    drag_through(&mut app, &arc, Modifiers::NONE);
    let sp = work(&app).unwrap().subpaths[0].clone();
    assert!(!sp.closed && (2..=4).contains(&sp.knots.len()), "{:?}", sp.knots);
    assert!(app.ui.freeform.is_none());
    // A loop back to the start closes, and adds to the selected work path.
    let mut lp: Vec<[f64; 2]> = (0..=360).step_by(4).map(|i| f64::from(i).to_radians()).map(|a| [100.0 + 30.0 * a.cos(), 100.0 + 30.0 * a.sin()]).collect();
    lp.push([130.5, 100.5]);
    drag_through(&mut app, &lp, Modifiers::NONE);
    let w = work(&app).unwrap();
    assert_eq!(w.subpaths.len(), 2);
    assert!(w.subpaths[1].closed);
    // A click draws nothing; Curve Fit decides how many anchors a stroke gets.
    click(&mut app, [10.0, 10.0], Modifiers::NONE);
    assert_eq!(work(&app).unwrap().subpaths.len(), 2);
    let wiggle: Vec<[f64; 2]> = (0..=200).map(|i| [f64::from(i) * 0.75, 50.0 + 3.0 * (f64::from(i) * 0.2).sin()]).collect();
    let knots = |app: &mut PhotocraftApp, fit: f32| {
        app.ui.tool_options.freeform_curve_fit = fit;
        app.ui.selected_path = None;
        drag_through(app, &wiggle, Modifiers::NONE);
        work(app).unwrap().subpaths[0].knots.len()
    };
    assert!(knots(&mut app, 0.5) > knots(&mut app, 10.0));
}

#[test]
fn curvature_pen_runs_smoothly_through_its_points() {
    let mut app = app(Tool::CurvaturePen);
    for p in [[20.0, 100.0], [100.0, 20.0], [180.0, 100.0]] {
        click(&mut app, p, Modifiers::NONE);
    }
    let cp = app.ui.curvature.clone().unwrap();
    assert_eq!(cp.points.len(), 3);
    let k = cp.subpath(false).knots[1];
    assert!(k.smooth && k.in_ctrl.y == 20.0 && k.out_ctrl.y == 20.0, "the middle point's handles are level: {k:?}");
    // Drag a point to move it; ⌥-click makes it a corner; a click on a segment adds a point.
    drag_through(&mut app, &[[100.0, 20.0], [100.0, 30.0]], Modifiers::NONE);
    click(&mut app, [100.0, 30.0], Modifiers::ALT);
    let cp = app.ui.curvature.clone().unwrap();
    assert_eq!(cp.points[1], ([100.0, 30.0], true));
    assert!(!cp.subpath(false).knots[1].smooth);
    click(&mut app, [60.0, 65.0], Modifiers::NONE);
    assert_eq!(app.ui.curvature.as_ref().unwrap().points.len(), 4);
    assert_eq!(app.ui.curvature.as_ref().unwrap().points[1].0, [60.0, 65.0], "inserted between the first two");
    // ⌫ / Undo removes the point last touched; a double-click toggles a corner.
    assert!(curvature_remove_point(&mut app));
    assert_eq!(app.ui.curvature.as_ref().unwrap().points.len(), 3);
    curvature_double_click(&mut app, [100.0, 30.0]);
    assert!(!app.ui.curvature.as_ref().unwrap().points[1].1);
    // Clicking the first point closes it into the work path.
    let n = steps(&app);
    click(&mut app, [20.5, 100.5], Modifiers::NONE);
    assert!(app.ui.curvature.is_none());
    let sp = work(&app).unwrap().subpaths[0].clone();
    assert!(sp.closed && sp.knots.len() == 3 && sp.knots.iter().all(|k| k.smooth));
    assert_eq!(steps(&app), n + 1);
}

#[test]
fn curvature_pen_finishes_open_on_tool_change_and_drops_a_lone_point() {
    let mut app = app(Tool::CurvaturePen);
    click(&mut app, [20.0, 20.0], Modifiers::NONE);
    assert!(curvature_remove_point(&mut app));
    assert!(app.ui.curvature.is_none() && !curvature_remove_point(&mut app));
    click(&mut app, [20.0, 20.0], Modifiers::NONE);
    curvature_commit(&mut app, false);
    assert!(work(&app).is_none(), "one point makes no path");
    click(&mut app, [20.0, 20.0], Modifiers::NONE);
    click(&mut app, [80.0, 50.0], Modifiers::NONE);
    curvature_commit(&mut app, true);
    let sp = work(&app).unwrap().subpaths[0].clone();
    assert!(!sp.closed, "two points can't close");
    assert_eq!(sp.knots.len(), 2);
}

#[test]
fn command_is_direct_selection_with_the_anchor_tools() {
    for tool in [Tool::AddAnchorPoint, Tool::DeleteAnchorPoint, Tool::ConvertPoint, Tool::FreeformPen] {
        let mut app = app(tool);
        with_square(&mut app);
        drag_through(&mut app, &[[40.0, 40.0], [50.0, 45.0]], Modifiers::COMMAND);
        assert_eq!(anchors(&app)[0], Point::new(50.0, 45.0), "{tool:?}");
        assert_eq!(anchors(&app).len(), 4, "{tool:?}");
    }
}

#[test]
fn the_new_tools_sit_in_the_pen_slot_and_cycle_with_p() {
    for t in [Tool::FreeformPen, Tool::CurvaturePen] {
        assert_eq!(t.key(), 'P');
    }
    for t in [Tool::AddAnchorPoint, Tool::DeleteAnchorPoint, Tool::ConvertPoint] {
        assert_eq!(t.key(), '\0');
        assert!(is_anchor_tool(t));
    }
    assert_eq!(Tool::from_name("Curvature Pen Tool"), Some(Tool::CurvaturePen));
    assert_eq!(Tool::from_name("convertPoint"), Some(Tool::ConvertPoint));
    for t in [Tool::FreeformPen, Tool::CurvaturePen, Tool::AddAnchorPoint, Tool::DeleteAnchorPoint, Tool::ConvertPoint] {
        assert!(crate::icons::exists(crate::icons::tool_icon(t)), "{t:?}");
    }
}

#[test]
fn single_row_and_column_marquees_select_one_pixel_lines() {
    let mut app = app(Tool::SingleRowMarquee);
    click(&mut app, [37.6, 50.4], Modifiers::NONE);
    let b = app.session.active().unwrap().doc.selection.as_ref().unwrap().content_bounds();
    assert_eq!([b.x0, b.y0, b.x1, b.y1], [0, 50, 200, 51], "the whole row through the click");
    // ⇧ adds a column.
    app.ui.tool = Tool::SingleColumnMarquee;
    click(&mut app, [120.2, 10.0], Modifiers::SHIFT);
    let b = app.session.active().unwrap().doc.selection.as_ref().unwrap().content_bounds();
    assert_eq!([b.x0, b.y0, b.x1, b.y1], [0, 0, 200, 200]);
    assert!(app.drag.is_none(), "a click, not a drag");
    for t in [Tool::SingleRowMarquee, Tool::SingleColumnMarquee] {
        assert_eq!(t.key(), '\0');
        assert!(crate::icons::exists(crate::icons::tool_icon(t)));
    }
}
