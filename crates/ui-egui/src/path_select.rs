//! Path Selection tool (A), as in Photoshop: it works on whole path components (subpaths) of the
//! paths Direct Selection edits: the targeted vector mask or the active shape layer's path, then
//! the work path.
//!
//! * Click a component (its outline, or inside a closed one) to select it, ⇧-click to add or
//!   remove it; click empty canvas to deselect.
//! * Drag a component to move the selected ones (⇧ keeps the move at 45° steps); ⌥-drag moves
//!   copies and leaves the originals.
//! * Drag on empty canvas to marquee-select the components it touches (⇧ adds).
//! * ⌫ / Delete removes the selected components.
//!
//! A drag previews with `photocraft_vector::edit` and commits once on release through the engine
//! (`path.moveSubpaths`, `path.deleteSubpaths`; a whole live shape moves with `shape.edit` and
//! stays live): one history step per drag.

use egui::{Color32, Modifiers, Pos2};
use photocraft_doc::vector::Path;
use photocraft_geom::Point;
use photocraft_vector::edit;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::PhotocraftApp;
use crate::canvas::ToolEvent;
use crate::direct_select::{PathRef, candidates};
use crate::state::Tool;

/// Path Selection state: the selected components and the drag in progress.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PathSelection {
    /// The path whose components are selected.
    pub target: Option<PathRef>,
    /// Selected subpaths, by index.
    pub subpaths: Vec<usize>,
    #[serde(skip)]
    pub drag: Option<Drag>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Drag {
    /// Moving (or with `duplicate`, copying) `subpaths` of `target`; `base` is its path at the press.
    Move { target: PathRef, base: Path, subpaths: Vec<usize>, duplicate: bool, start: [f64; 2], end: [f64; 2], constrain: bool },
    /// Marquee-selecting components (⇧ adds to the selection).
    Marquee { start: [f64; 2], end: [f64; 2], add: bool },
}

/// The drag offset; ⇧ snaps its direction to 45° steps.
fn offset(start: [f64; 2], end: [f64; 2], constrain: bool) -> [f64; 2] {
    let d = [end[0] - start[0], end[1] - start[1]];
    if !constrain {
        return d;
    }
    let len = d[0].hypot(d[1]);
    let a = (d[1].atan2(d[0]) / std::f64::consts::FRAC_PI_4).round() * std::f64::consts::FRAC_PI_4;
    [len * a.cos(), len * a.sin()]
}

/// `base` with the drag applied: the preview, the moved subpaths' indices included.
fn preview(base: &Path, subpaths: &[usize], duplicate: bool, d: [f64; 2]) -> Option<(Path, Vec<usize>)> {
    let mut p = base.clone();
    let moved = edit::move_subpaths(&mut p, subpaths, d, duplicate).ok()?;
    Some((p, moved))
}

/// The selection, without indices the path no longer has (after an undo, say).
fn selected(ps: &PathSelection, r: PathRef, path: &Path) -> Vec<usize> {
    if ps.target != Some(r) {
        return Vec::new();
    }
    ps.subpaths.iter().copied().filter(|&s| s < path.subpaths.len()).collect()
}

/// Canvas pointer events for Path Selection; false when the event is not its.
pub fn pointer(app: &mut PhotocraftApp, ev: ToolEvent, mods: Modifiers) -> bool {
    if app.ui.tool != Tool::PathSelection {
        return false;
    }
    match ev {
        ToolEvent::Down { x, y, .. } => press(app, [x, y], mods),
        ToolEvent::Move { x, y, .. } => match &mut app.ui.path_selection.drag {
            Some(Drag::Move { end, constrain, .. }) => {
                *end = [x, y];
                *constrain = mods.shift;
            }
            Some(Drag::Marquee { end, .. }) => *end = [x, y],
            None => {}
        },
        ToolEvent::Up { x, y } => match app.ui.path_selection.drag.take() {
            Some(Drag::Move { target, base, subpaths, duplicate, start, .. }) => {
                release(app, target, &base, &subpaths, duplicate, offset(start, [x, y], mods.shift))
            }
            Some(Drag::Marquee { start, add, .. }) => marquee(app, start, [x, y], add),
            None => {}
        },
    }
    true
}

/// A press: select the component under the pointer and start moving the selection, or start a
/// marquee.
fn press(app: &mut PhotocraftApp, p: [f64; 2], mods: Modifiers) {
    let tol = 6.0 / f64::from(app.point_zoom().max(0.01));
    let found = candidates(app).into_iter().find_map(|(r, path)| edit::subpath_at(&path, Point::new(p[0], p[1]), tol).map(|s| (r, path, s)));
    let ps = &mut app.ui.path_selection;
    let Some((target, base, s)) = found else {
        ps.drag = Some(Drag::Marquee { start: p, end: p, add: mods.shift });
        return;
    };
    let mut sel = selected(ps, target, &base);
    if mods.shift {
        // ⇧-click toggles the component; a click that deselects drags nothing.
        if let Some(i) = sel.iter().position(|&q| q == s) {
            sel.remove(i);
            *ps = PathSelection { target: Some(target), subpaths: sel, drag: None };
            return;
        }
        sel.push(s);
    } else if !sel.contains(&s) {
        sel = vec![s];
    }
    *ps = PathSelection {
        target: Some(target),
        subpaths: sel.clone(),
        drag: Some(Drag::Move { target, base, subpaths: sel, duplicate: mods.alt, start: p, end: p, constrain: mods.shift }),
    };
}

fn run(app: &mut PhotocraftApp, id: &str, params: Value) -> Option<Value> {
    app.run(id, params)
        .inspect_err(|e| {
            app.ui.status = e.clone();
            app.ui.status_error = true;
        })
        .ok()
}

/// `params` with the engine's target params for `target` added.
fn targeted(target: PathRef, mut params: Value) -> Value {
    if let (Some(p), Some(t)) = (params.as_object_mut(), target.params().as_object()) {
        p.extend(t.clone());
    }
    params
}

/// End of a move: commit it as one command (whole pixels, as Photoshop nudges paths). A click
/// commits nothing.
fn release(app: &mut PhotocraftApp, target: PathRef, base: &Path, subpaths: &[usize], duplicate: bool, d: [f64; 2]) {
    if d[0].hypot(d[1]) * f64::from(app.point_zoom()) < 1.0 {
        return;
    }
    let d = [d[0].round(), d[1].round()];
    if d == [0.0, 0.0] {
        return;
    }
    // A live shape (rectangle, ellipse…) moved whole stays live.
    let whole = subpaths.len() == base.subpaths.len();
    if let PathRef::Layer(id) = target
        && whole
        && !duplicate
        && crate::vector_ui::active_shape_path(app).is_some_and(|(l, _)| l == id)
    {
        run(app, "shape.edit", json!({"layer": id, "move": d}));
        return;
    }
    if let Some(r) = run(app, "path.moveSubpaths", targeted(target, json!({"subpaths": subpaths, "move": d, "duplicate": duplicate}))) {
        let moved = r.get("subpaths").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_u64().map(|i| i as usize)).collect());
        app.ui.path_selection.subpaths = moved.unwrap_or_default();
    }
}

/// End of a marquee: select the components it touches on the frontmost path that has any (⇧
/// adds). A click on empty canvas deselects.
fn marquee(app: &mut PhotocraftApp, start: [f64; 2], end: [f64; 2], add: bool) {
    let rect = [start[0], start[1], end[0], end[1]];
    let hits = candidates(app).into_iter().map(|(r, path)| (r, edit::subpaths_touching(&path, rect))).find(|(_, s)| !s.is_empty());
    let ps = &mut app.ui.path_selection;
    match hits {
        Some((r, subs)) if add && ps.target == Some(r) => {
            for s in subs {
                if !ps.subpaths.contains(&s) {
                    ps.subpaths.push(s);
                }
            }
        }
        Some((r, subpaths)) => *ps = PathSelection { target: Some(r), subpaths, drag: None },
        None if add => {}
        None => ps.subpaths.clear(),
    }
}

/// ⌫ / Delete with Path Selection: remove the selected components. All of the work path's
/// deletes the work path; all of a layer's path is refused (delete the layer or its mask).
/// False when nothing is selected, so the key does what it otherwise would.
pub fn delete_selected(app: &mut PhotocraftApp) -> bool {
    if app.ui.tool != Tool::PathSelection {
        return false;
    }
    let ps = &app.ui.path_selection;
    let Some((target, path)) = ps.target.and_then(|t| candidates(app).into_iter().find(|(r, _)| *r == t)) else { return false };
    let subs = selected(ps, target, &path);
    if subs.is_empty() {
        return false;
    }
    let all = subs.len() == path.subpaths.len();
    match target {
        PathRef::Work if all => {
            run(app, "path.delete", json!({"name": "work"}));
        }
        PathRef::Layer(_) if all => {
            app.ui.status = tl!("Select less than the whole path, or delete the layer or its vector mask in the Layers panel.").into();
            app.ui.status_error = true;
            return true;
        }
        _ => {
            run(app, "path.deleteSubpaths", targeted(target, json!({"subpaths": subs})));
        }
    }
    app.ui.path_selection.subpaths.clear();
    true
}

/// The paths with their outlines, the selected components' anchors (filled, as in Photoshop),
/// the drag's preview in place of its path, and the marquee. `paths` false (View › Show ›
/// Target Path off) leaves only the marquee.
pub fn draw_overlay(app: &PhotocraftApp, painter: &egui::Painter, to_scr: &dyn Fn([f64; 2]) -> Pos2, accent: Color32, paths: bool) {
    let ps = &app.ui.path_selection;
    for (r, path) in if paths { candidates(app) } else { Vec::new() } {
        let (path, sel) = match &ps.drag {
            Some(Drag::Move { target, base, subpaths, duplicate, start, end, constrain }) if *target == r => {
                preview(base, subpaths, *duplicate, offset(*start, *end, *constrain)).unwrap_or_else(|| (path.clone(), selected(ps, r, &path)))
            }
            _ => {
                let sel = selected(ps, r, &path);
                (path, sel)
            }
        };
        crate::vector_ui::draw_outline(painter, &path, to_scr, accent);
        for s in sel {
            for kn in &path.subpaths[s].knots {
                crate::vector_ui::draw_anchor(painter, to_scr([kn.anchor.x, kn.anchor.y]), true, accent);
            }
        }
    }
    if let Some(Drag::Marquee { start, end, .. }) = &ps.drag {
        let pts = [*start, [end[0], start[1]], *end, [start[0], end[1]]].map(to_scr);
        crate::tool_feedback::draw_ants(painter, &pts, true);
    }
}

#[cfg(test)]
#[path = "path_select_tests.rs"]
mod tests;
