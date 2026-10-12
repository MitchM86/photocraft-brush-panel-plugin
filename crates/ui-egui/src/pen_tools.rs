//! The Pen tool's siblings in its toolbar slot, as in Photoshop:
//!
//! * **Freeform Pen**: drag to draw; on release the stroke becomes a path fitted within the
//!   options bar's Curve Fit (0.5–10 px), closed when released over its start. With Magnetic
//!   ticked it follows edges in the image instead, as the Magnetic Lasso does
//!   (`magnetic_lasso_ui`).
//! * **Curvature Pen**: each click adds a point and the path runs smoothly through all of them.
//!   Double-click (or ⌥-click) a point to make it a corner or smooth again, drag a point to move
//!   it, click a segment to add a point there, click the first point to close; ↩ / Esc finish,
//!   ⌫ removes the last point touched.
//! * **Add Anchor Point**: click a segment to add an anchor without changing the outline, drag to
//!   add one and move it; ⌥ switches to Delete Anchor Point.
//! * **Delete Anchor Point**: click an anchor to remove it; ⌥ switches to Add Anchor Point.
//! * **Convert Point** is Direct Selection's convert mode (`direct_select`), the Pen's ⌥.
//!
//! ⌘ / Ctrl is Direct Selection with all of them except the Curvature Pen. They edit the paths
//! Direct Selection edits (`direct_select::candidates`) and finish new paths like the Pen
//! (`vector_ui::commit_path`): one engine command per edit, so each is one History step.

use egui::{Color32, Modifiers, Pos2, Stroke};
use photocraft_doc::vector::Path;
use photocraft_doc::{Knot, Subpath};
use photocraft_geom::Point;
use photocraft_vector::edit::Hit;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::canvas::ToolEvent;
use crate::direct_select::{DirectSelection, Drag, Edit, PathRef};
use crate::state::Tool;

/// Add Anchor Point, Delete Anchor Point and Convert Point: they work on existing anchors.
pub fn is_anchor_tool(t: Tool) -> bool {
    matches!(t, Tool::AddAnchorPoint | Tool::DeleteAnchorPoint | Tool::ConvertPoint)
}

/// Pointer hit radius in document px (6 screen points).
fn tolerance(app: &PhotocraftApp) -> f64 {
    6.0 / f64::from(app.point_zoom().max(0.01))
}

fn report(app: &mut PhotocraftApp, r: Result<Value, String>) -> Option<Value> {
    r.map_err(|e| {
        app.ui.status = e;
        app.ui.status_error = true;
    })
    .ok()
}

/// Canvas pointer events for these tools; false when the event is not theirs (⌘ goes on to
/// Direct Selection, Convert Point to `direct_select`).
pub fn pointer(app: &mut PhotocraftApp, ev: ToolEvent, mods: Modifiers) -> bool {
    match app.ui.tool {
        Tool::AddAnchorPoint | Tool::DeleteAnchorPoint if !mods.command => anchor_pointer(app, ev, (app.ui.tool == Tool::AddAnchorPoint) != mods.alt),
        // Magnetic: the Magnetic Lasso's edge following (it leaves ⌘ to Direct Selection).
        Tool::FreeformPen if app.ui.tool_options.freeform_magnetic => crate::magnetic_lasso_ui::pointer(app, ev, mods),
        Tool::FreeformPen if !mods.command => freeform_pointer(app, ev),
        Tool::CurvaturePen => curvature_pointer(app, ev, mods),
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------------
// Add / Delete Anchor Point

fn anchor_pointer(app: &mut PhotocraftApp, ev: ToolEvent, add: bool) -> bool {
    let ToolEvent::Down { x, y, .. } = ev else {
        // A drag started here (a new anchor, a handle) belongs to Direct Selection from now on.
        return app.ui.direct_selection.drag.is_none();
    };
    let p = [x, y];
    match crate::direct_select::hit_at(app, p) {
        Some((target, _, Hit::Segment(k, t))) if add => add_anchor(app, target, k, t, p),
        Some((target, _, Hit::Anchor(k))) if !add => delete_anchor(app, target, k),
        // Over an anchor (adding) or a shown handle, they act as Direct Selection, as in Photoshop.
        Some((_, _, Hit::Anchor(_) | Hit::Handle(..))) => {
            crate::direct_select::press(app, p, Modifiers::NONE, false);
        }
        _ => {}
    }
    true
}

fn with_target(target: PathRef, mut params: Value) -> Value {
    if let (Some(p), Some(t)) = (params.as_object_mut(), target.params().as_object()) {
        p.extend(t.clone());
    }
    params
}

/// Adds an anchor on segment `k` at `t` and selects it, ready to be dragged.
fn add_anchor(app: &mut PhotocraftApp, target: PathRef, k: [usize; 2], t: f64, p: [f64; 2]) {
    let r = app.run("path.addAnchor", with_target(target, json!({"subpath": k[0], "knot": k[1], "t": t})));
    let Some(r) = report(app, r) else { return };
    let added = r["knot"].as_array().and_then(|a| Some([a.first()?.as_u64()? as usize, a.get(1)?.as_u64()? as usize]));
    let base = crate::direct_select::candidates(app).into_iter().find(|(r, _)| *r == target).map(|(_, path)| path);
    let (Some(new), Some(base)) = (added, base) else { return };
    app.ui.direct_selection = DirectSelection {
        target: Some(target),
        anchors: vec![new],
        segment: None,
        drag: Some(Drag::Edit { target, base, edit: Edit::Anchors(vec![new]), start: p, end: p, preview: None }),
    };
}

/// Removes anchor `k`. The work path's last anchor takes the work path with it, as in Photoshop.
fn delete_anchor(app: &mut PhotocraftApp, target: PathRef, k: [usize; 2]) {
    let only = crate::direct_select::candidates(app)
        .into_iter()
        .find(|(r, _)| *r == target)
        .is_some_and(|(_, path)| path.subpaths.iter().map(|s| s.knots.len()).sum::<usize>() <= 1);
    let r = if only && target == PathRef::Work {
        app.run("path.delete", json!({"name": "work"}))
    } else {
        app.run("path.deleteAnchor", with_target(target, json!({"subpath": k[0], "knot": k[1]})))
    };
    if report(app, r).is_some() {
        // Knot indices after it have shifted.
        app.ui.direct_selection = DirectSelection { target: Some(target), ..Default::default() };
    }
}

// ---------------------------------------------------------------------------------------------
// Freeform Pen

fn freeform_pointer(app: &mut PhotocraftApp, ev: ToolEvent) -> bool {
    match ev {
        ToolEvent::Down { x, y, .. } => app.ui.freeform = Some(vec![[x, y]]),
        ToolEvent::Move { x, y, .. } => {
            let step = 0.5 / f64::from(app.point_zoom().max(0.01));
            if let Some(pts) = app.ui.freeform.as_mut()
                && pts.last().is_none_or(|l| (l[0] - x).hypot(l[1] - y) >= step)
            {
                pts.push([x, y]);
            }
        }
        ToolEvent::Up { x, y } => {
            let Some(mut pts) = app.ui.freeform.take() else { return true };
            pts.push([x, y]);
            freeform_finish(app, &pts);
        }
    }
    true
}

/// Fits the drawn stroke and finishes it as the Pen would. Released over its start, it closes.
pub(crate) fn freeform_finish(app: &mut PhotocraftApp, pts: &[[f64; 2]]) {
    let closed = pts.len() > 2 && pts.first().zip(pts.last()).is_some_and(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1]) < tolerance(app));
    fit_and_commit(app, pts, closed);
}

/// Fits `pts` within the Freeform Pen's Curve Fit and finishes the path as the Pen would (the
/// magnetic option's border comes here too).
pub(crate) fn fit_and_commit(app: &mut PhotocraftApp, pts: &[[f64; 2]], closed: bool) {
    let fit = f64::from(app.ui.tool_options.freeform_curve_fit);
    let Some(sub) = photocraft_vector::trace::fit_points(pts, fit, closed) else { return };
    let closed = sub.closed;
    crate::vector_ui::commit_path(app, photocraft_engine::vector_cmds::path_json(&Path::new(vec![sub])), closed);
}

// ---------------------------------------------------------------------------------------------
// Curvature Pen

/// Curvature Pen path under construction: the points it runs through.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CurvaturePath {
    /// Points in order, each with whether it is a corner.
    pub points: Vec<([f64; 2], bool)>,
    /// The point last added or moved: drawn filled, and the one ⌫ removes.
    #[serde(skip)]
    pub focus: Option<usize>,
    /// The point being dragged.
    #[serde(skip)]
    pub drag: Option<usize>,
}

impl CurvaturePath {
    /// The Bézier subpath through the points: a smooth point's handles lie along the line through
    /// its neighbours, a third of the way to each; corners and open ends have none.
    pub fn subpath(&self, closed: bool) -> Subpath {
        let n = self.points.len();
        let closed = closed && n > 2;
        let pt = |p: [f64; 2]| Point::new(p[0], p[1]);
        let knots = (0..n)
            .map(|i| {
                let (p, corner) = self.points[i];
                let prev = if i > 0 { Some(self.points[i - 1].0) } else { closed.then(|| self.points[n - 1].0) };
                let next = if i + 1 < n { Some(self.points[i + 1].0) } else { closed.then(|| self.points[0].0) };
                let (Some(a), Some(b), false) = (prev, next, corner) else { return Knot::corner(p[0], p[1]) };
                let d = [b[0] - a[0], b[1] - a[1]];
                let len = d[0].hypot(d[1]);
                if len < 1e-9 {
                    return Knot::corner(p[0], p[1]);
                }
                let u = [d[0] / len, d[1] / len];
                let (back, ahead) = ((p[0] - a[0]).hypot(p[1] - a[1]) / 3.0, (b[0] - p[0]).hypot(b[1] - p[1]) / 3.0);
                Knot::smooth(pt(p), pt([p[0] - u[0] * back, p[1] - u[1] * back]), pt([p[0] + u[0] * ahead, p[1] + u[1] * ahead]))
            })
            .collect();
        Subpath { closed, knots, op: Default::default() }
    }
}

fn near(a: [f64; 2], b: [f64; 2], tol: f64) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) < tol
}

fn curvature_pointer(app: &mut PhotocraftApp, ev: ToolEvent, mods: Modifiers) -> bool {
    let tol = tolerance(app);
    match ev {
        ToolEvent::Down { x, y, .. } => {
            let p = [x, y];
            let cp = app.ui.curvature.get_or_insert_with(CurvaturePath::default);
            let n = cp.points.len();
            if n >= 3 && !mods.alt && near(cp.points[0].0, p, tol) {
                curvature_commit(app, true);
                return true;
            }
            if let Some(i) = cp.points.iter().rposition(|(q, _)| near(*q, p, tol)) {
                if mods.alt {
                    cp.points[i].1 = !cp.points[i].1;
                } else {
                    cp.drag = Some(i);
                }
                cp.focus = Some(i);
                return true;
            }
            // A click on the path in progress adds a point there.
            let at = match photocraft_vector::edit::hit(&Path::new(vec![cp.subpath(false)]), Point::new(x, y), tol, &[]) {
                Some(Hit::Segment([_, k], _)) => k + 1,
                _ => n,
            };
            cp.points.insert(at, (p, false));
            cp.drag = Some(at);
            cp.focus = Some(at);
        }
        ToolEvent::Move { x, y, .. } => {
            if let Some(cp) = app.ui.curvature.as_mut()
                && let Some(pt) = cp.drag.and_then(|i| cp.points.get_mut(i))
            {
                pt.0 = [x, y];
            }
        }
        ToolEvent::Up { .. } => {
            if let Some(cp) = app.ui.curvature.as_mut() {
                cp.drag = None;
            }
        }
    }
    true
}

/// Double-click on a point: corner ↔ smooth.
pub fn curvature_double_click(app: &mut PhotocraftApp, p: [f64; 2]) {
    let tol = tolerance(app);
    if let Some(cp) = app.ui.curvature.as_mut()
        && let Some(i) = cp.points.iter().rposition(|(q, _)| near(*q, p, tol))
    {
        cp.points[i].1 = !cp.points[i].1;
    }
}

/// ⌫ / Undo while drawing: remove the point last touched. False when there's no path in progress.
pub fn curvature_remove_point(app: &mut PhotocraftApp) -> bool {
    let Some(cp) = app.ui.curvature.as_mut() else { return false };
    let i = cp.focus.filter(|&i| i < cp.points.len()).or(cp.points.len().checked_sub(1));
    let Some(i) = i else { return false };
    cp.points.remove(i);
    cp.drag = None;
    cp.focus = cp.points.len().checked_sub(1);
    if cp.points.is_empty() {
        app.ui.curvature = None;
    }
    true
}

/// Finish the Curvature Pen's path as the Pen would (a single point is dropped).
pub fn curvature_commit(app: &mut PhotocraftApp, closed: bool) {
    let Some(cp) = app.ui.curvature.take() else { return };
    if cp.points.len() < 2 {
        return;
    }
    let sub = cp.subpath(closed);
    let closed = sub.closed;
    crate::vector_ui::commit_path(app, photocraft_engine::vector_cmds::path_json(&Path::new(vec![sub])), closed);
}

// ---------------------------------------------------------------------------------------------
// Overlay

/// The Freeform stroke being drawn and the Curvature Pen's path, with a preview of the next
/// point at the pointer.
pub fn draw_overlay(app: &PhotocraftApp, painter: &egui::Painter, to_scr: &dyn Fn([f64; 2]) -> Pos2, accent: Color32) {
    if let Some(pts) = &app.ui.freeform {
        painter.add(egui::Shape::line(pts.iter().map(|p| to_scr(*p)).collect(), Stroke::new(1.5, accent)));
    }
    let Some(cp) = &app.ui.curvature else { return };
    let curve = |cp: &CurvaturePath, width: f32| {
        let path = Path::new(vec![cp.subpath(false)]);
        for (line, _) in crate::vector_ui::path_lines(&path, to_scr) {
            painter.add(egui::Shape::line(line, Stroke::new(width, accent)));
        }
    };
    // Where the path would go with a click at the pointer.
    let over_point = app.hover_doc.is_some_and(|h| cp.points.iter().any(|(q, _)| near(*q, h, 6.0 / f64::from(app.point_zoom().max(0.01)))));
    if let Some(h) = app.hover_doc
        && cp.drag.is_none()
        && !over_point
    {
        let mut next = cp.clone();
        next.points.push((h, false));
        curve(&next, 1.0);
    }
    curve(cp, 1.5);
    let focus = cp.focus.or(cp.points.len().checked_sub(1));
    for (i, (p, corner)) in cp.points.iter().enumerate() {
        let c = to_scr(*p);
        if *corner {
            crate::vector_ui::draw_anchor(painter, c, Some(i) == focus, accent);
        } else if Some(i) == focus {
            painter.circle_filled(c, 3.5, accent);
        } else {
            painter.circle(c, 3.5, Color32::WHITE, Stroke::new(1.0, accent));
        }
    }
}

#[cfg(test)]
#[path = "pen_tools_tests.rs"]
mod tests;
