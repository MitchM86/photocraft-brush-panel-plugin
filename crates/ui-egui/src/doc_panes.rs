//! Split document panes: the document area as a tree of panes side by side or one above the
//! other, each with its own tab strip and its own shown document (like a code editor's editor
//! groups).
//!
//! Drag a document tab (or a floating document window by its title bar) and drop it:
//! - on a pane's tab strip: it joins that pane, where the insertion line shows;
//! - near the document area's right or left edge (beside the icon rail, say): a new pane the
//!   whole height of the area on that side;
//! - in a pane's outer quarter (left, right, top or bottom): that pane splits, the document taking
//!   that half (a third document below the right one, for instance);
//! - anywhere else: it floats in a window of its own (`doc_float`).
//!
//! A blue preview shows where it will go before the button is released. The dividers between
//! panes drag to resize. A pane closes when its last document leaves, and its space goes to its
//! neighbour. The document that's active (whichever pane it's in) takes the tools; the pane
//! clicked last is where new documents open. Window › Arrange › Consolidate All to Tabs (and the
//! n-up layouts) go back to one pane. The layout lasts for the session.
//!
//! Tab order within a pane follows the documents' order, so reordering tabs is `document.move`
//! as before.

use egui::{Pos2, Rect, Sense, Stroke, pos2, vec2};
use photocraft_doc::DocId;
use serde::{Deserialize, Serialize};

use crate::PhotocraftApp;
use crate::canvas::TabStrip;
use crate::theme::Tokens;

pub type PaneId = u32;

/// The layout tree: a pane, or two parts split across (side by side: `a` left, `b` right) or
/// down (`a` above `b`), `ratio` of the space going to `a`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Node {
    Pane(PaneId),
    Split { across: bool, ratio: f32, a: Box<Node>, b: Box<Node> },
}

/// A pane's documents (in the documents' order on its strip) and the one it shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pane {
    pub id: PaneId,
    pub docs: Vec<DocId>,
    pub shown: Option<DocId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaneLayout {
    pub root: Node,
    pub panes: Vec<Pane>,
    /// The pane clicked last: new documents open here, and its strip is the one files drop on.
    pub focused: PaneId,
    next: PaneId,
}

impl Default for PaneLayout {
    fn default() -> Self {
        PaneLayout { root: Node::Pane(0), panes: vec![Pane::default()], focused: 0, next: 1 }
    }
}

/// Which side of a pane (or of the whole area) a document goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

/// Where a dragged document lands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drop {
    /// Onto a pane's strip, before the document at this index (the end when past it).
    Join(PaneId, usize),
    /// Split a pane (or, with `None`, the whole area) and take the `Side` half.
    Split(Option<PaneId>, Side),
    /// Float in a window with its title bar here.
    Float(Pos2),
}

/// Divider width between panes.
pub const GAP: f32 = 4.0;

/// Within this of the document area's edge a drop makes a pane across the whole area.
pub const AREA_EDGE: f32 = 44.0;

/// The outer part of a pane (a fraction of its size) that splits it.
pub const PANE_EDGE: f32 = 0.25;

/// The share of the area a pane made at its edge takes.
const AREA_SHARE: f32 = 0.35;

impl PaneLayout {
    pub fn single(&self) -> bool {
        matches!(self.root, Node::Pane(_))
    }

    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.iter().find(|p| p.id == id)
    }

    fn pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.panes.iter_mut().find(|p| p.id == id)
    }

    pub fn pane_of(&self, doc: DocId) -> Option<PaneId> {
        self.panes.iter().find(|p| p.docs.contains(&doc)).map(|p| p.id)
    }

    fn add(&mut self, pane: PaneId, doc: DocId) {
        self.remove(doc);
        if let Some(p) = self.pane_mut(pane) {
            p.docs.push(doc);
            p.shown = Some(doc);
        }
    }

    /// Take `doc` out of its pane; a pane left empty closes (unless it's the only one).
    fn remove(&mut self, doc: DocId) {
        let Some(id) = self.pane_of(doc) else { return };
        if let Some(p) = self.pane_mut(id) {
            p.docs.retain(|d| *d != doc);
            if p.shown == Some(doc) {
                p.shown = p.docs.first().copied();
            }
        }
        if self.pane(id).is_some_and(|p| p.docs.is_empty()) {
            self.close(id);
        }
    }

    /// Close pane `id` (its space goes to its neighbour); the last pane stays.
    fn close(&mut self, id: PaneId) {
        if self.single() {
            return;
        }
        fn without(n: Node, id: PaneId) -> Option<Node> {
            match n {
                Node::Pane(p) if p == id => None,
                Node::Pane(p) => Some(Node::Pane(p)),
                Node::Split { across, ratio, a, b } => match (without(*a, id), without(*b, id)) {
                    (Some(a), Some(b)) => Some(Node::Split { across, ratio, a: Box::new(a), b: Box::new(b) }),
                    (Some(x), None) | (None, Some(x)) => Some(x),
                    (None, None) => None,
                },
            }
        }
        let root = std::mem::replace(&mut self.root, Node::Pane(0));
        self.root = without(root, id).unwrap_or(Node::Pane(id));
        if !matches!(self.root, Node::Pane(p) if p == id) {
            self.panes.retain(|p| p.id != id);
        }
        if self.pane(self.focused).is_none() {
            self.focused = self.panes.first().map_or(0, |p| p.id);
        }
    }

    /// A new pane holding `doc` on `side` of pane `target` (the whole area with `None`).
    fn split(&mut self, target: Option<PaneId>, side: Side, doc: DocId) -> PaneId {
        self.remove(doc);
        let id = self.next;
        self.next += 1;
        self.panes.push(Pane { id, docs: vec![doc], shown: Some(doc) });
        let across = matches!(side, Side::Left | Side::Right);
        let first = matches!(side, Side::Left | Side::Top);
        let wrap = |old: Node, ratio: f32| {
            let new = Node::Pane(id);
            let (a, b, ratio) = if first { (new, old, 1.0 - ratio) } else { (old, new, ratio) };
            Node::Split { across, ratio, a: Box::new(a), b: Box::new(b) }
        };
        fn replace(n: Node, target: PaneId, f: &dyn Fn(Node) -> Node) -> Node {
            match n {
                Node::Pane(p) if p == target => f(Node::Pane(p)),
                Node::Split { across, ratio, a, b } => {
                    Node::Split { across, ratio, a: Box::new(replace(*a, target, f)), b: Box::new(replace(*b, target, f)) }
                }
                n => n,
            }
        }
        let root = std::mem::replace(&mut self.root, Node::Pane(0));
        self.root = match target.filter(|t| self.pane(*t).is_some()) {
            Some(t) => replace(root, t, &|n| wrap(n, 0.5)),
            None => wrap(root, 1.0 - AREA_SHARE),
        };
        self.focused = id;
        id
    }
}

/// Is document `idx` on pane `pane`'s strip? Documents not placed yet (opened this frame) belong
/// to the focused pane; floating ones to none.
pub fn in_pane(app: &PhotocraftApp, idx: usize, pane: PaneId) -> bool {
    let Some(id) = app.session.documents().get(idx).map(|st| st.doc.id) else { return false };
    if crate::doc_float::is_floating(app, idx) {
        return false;
    }
    match app.ui.panes.pane_of(id) {
        Some(p) => p == pane,
        None => pane == app.ui.panes.focused,
    }
}

/// The document pane `pane` shows: the active one when it's there, else the one it showed last,
/// else its first.
pub fn shown(app: &PhotocraftApp, pane: PaneId) -> Option<usize> {
    if let Some(a) = app.session.active_index()
        && in_pane(app, a, pane)
    {
        return Some(a);
    }
    let docs = app.session.documents();
    if let Some(i) = app.ui.panes.pane(pane).and_then(|p| p.shown).and_then(|d| docs.iter().position(|st| st.doc.id == d))
        && in_pane(app, i, pane)
    {
        return Some(i);
    }
    (0..docs.len()).find(|&i| in_pane(app, i, pane))
}

/// Once a frame, before the document area draws: closed and floating documents leave their panes,
/// new ones join the focused pane, empty panes close, and the active document's pane becomes the
/// focused one and shows it.
pub fn sync(app: &mut PhotocraftApp) {
    crate::doc_float::track(app);
    let docs: Vec<(DocId, bool)> = (0..app.session.documents().len())
        .filter_map(|i| app.session.documents().get(i).map(|st| (st.doc.id, crate::doc_float::is_floating(app, i))))
        .collect();
    let layout = &mut app.ui.panes;
    for p in layout.panes.clone() {
        for d in p.docs {
            if !docs.iter().any(|(id, floating)| *id == d && !floating) {
                layout.remove(d);
            }
        }
    }
    if layout.pane(layout.focused).is_none() {
        layout.focused = layout.panes.first().map_or(0, |p| p.id);
    }
    let focused = layout.focused;
    for (id, floating) in &docs {
        if !floating && layout.pane_of(*id).is_none() {
            layout.add(focused, *id);
        }
    }
    if let Some(a) = app.session.active_index()
        && let Some(id) = app.session.documents().get(a).map(|st| st.doc.id)
        && let Some(p) = app.ui.panes.pane_of(id)
    {
        app.ui.panes.focused = p;
        if let Some(pane) = app.ui.panes.pane_mut(p) {
            pane.shown = Some(id);
        }
    }
}

/// Back to one pane holding every docked document (Consolidate All to Tabs, the n-up layouts).
pub fn consolidate(app: &mut PhotocraftApp) {
    app.ui.panes = PaneLayout::default();
    sync(app);
}

// ----------------------------------------------------------------------------- layout

/// A divider between the two parts of a split: the split's place in the tree (`false` = `a`,
/// `true` = `b` at each level), the split's whole rectangle, and the divider's.
#[derive(Clone, Debug, PartialEq)]
pub struct Divider {
    pub path: Vec<bool>,
    pub across: bool,
    pub whole: Rect,
    pub rect: Rect,
}

/// Each pane's rectangle in `rect`, and the dividers.
pub fn layout(root: &Node, rect: Rect) -> (Vec<(PaneId, Rect)>, Vec<Divider>) {
    fn walk(n: &Node, r: Rect, path: &mut Vec<bool>, panes: &mut Vec<(PaneId, Rect)>, dividers: &mut Vec<Divider>) {
        match n {
            Node::Pane(p) => panes.push((*p, r)),
            Node::Split { across, ratio, a, b } => {
                let ratio = if ratio.is_finite() { ratio.clamp(0.1, 0.9) } else { 0.5 };
                let (ra, rd, rb) = if *across {
                    let x = r.left() + (r.width() - GAP).max(0.0) * ratio;
                    (
                        Rect::from_min_max(r.min, pos2(x, r.bottom())),
                        Rect::from_min_max(pos2(x, r.top()), pos2(x + GAP, r.bottom())),
                        Rect::from_min_max(pos2(x + GAP, r.top()), r.max),
                    )
                } else {
                    let y = r.top() + (r.height() - GAP).max(0.0) * ratio;
                    (
                        Rect::from_min_max(r.min, pos2(r.right(), y)),
                        Rect::from_min_max(pos2(r.left(), y), pos2(r.right(), y + GAP)),
                        Rect::from_min_max(pos2(r.left(), y + GAP), r.max),
                    )
                };
                dividers.push(Divider { path: path.clone(), across: *across, whole: r, rect: rd });
                path.push(false);
                walk(a, ra, path, panes, dividers);
                path.pop();
                path.push(true);
                walk(b, rb, path, panes, dividers);
                path.pop();
            }
        }
    }
    let (mut panes, mut dividers) = (Vec::new(), Vec::new());
    walk(root, rect, &mut Vec::new(), &mut panes, &mut dividers);
    (panes, dividers)
}

fn ratio_at<'a>(n: &'a mut Node, path: &[bool]) -> Option<&'a mut f32> {
    match (n, path.split_first()) {
        (Node::Split { ratio, .. }, None) => Some(ratio),
        (Node::Split { a, b, .. }, Some((&right, rest))) => ratio_at(if right { b } else { a }, rest),
        _ => None,
    }
}

/// The panes as drawn last frame: the area, and each pane's rectangle and tab strip.
#[derive(Clone, Debug)]
pub struct Frames {
    pub frame: u64,
    pub area: Rect,
    pub panes: Vec<(PaneId, Rect, TabStrip)>,
}

fn frames_id() -> egui::Id {
    egui::Id::new("doc-pane-frames")
}

/// Record where the panes were drawn (the single-pane path calls it too).
pub fn record(ctx: &egui::Context, area: Rect, panes: Vec<(PaneId, Rect, TabStrip)>) {
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|d| d.insert_temp(frames_id(), Frames { frame, area, panes }));
}

pub fn frames(ctx: &egui::Context) -> Frames {
    ctx.data(|d| d.get_temp::<Frames>(frames_id())).unwrap_or(Frames { frame: 0, area: Rect::NOTHING, panes: Vec::new() })
}

// ----------------------------------------------------------------------------- drops

/// Where document `idx`'s tab, dragged to `p`, would land.
pub fn target(app: &PhotocraftApp, ctx: &egui::Context, idx: usize, p: Pos2) -> Drop {
    target_in(app, ctx, idx, p, false)
}

/// [`target`] for a floating window dragged by its title bar: a pane splits only within
/// [`AREA_EDGE`] of its edge, so moving a window around over the documents doesn't dock it.
pub fn window_target(app: &PhotocraftApp, ctx: &egui::Context, idx: usize, p: Pos2) -> Drop {
    target_in(app, ctx, idx, p, true)
}

fn target_in(app: &PhotocraftApp, ctx: &egui::Context, idx: usize, p: Pos2, window: bool) -> Drop {
    let f = frames(ctx);
    let doc = app.session.documents().get(idx).map(|st| st.doc.id);
    let from = doc.and_then(|d| app.ui.panes.pane_of(d)).filter(|_| !crate::doc_float::is_floating(app, idx));
    // Alone in its pane: splitting that pane (or joining it) would change nothing.
    let alone = from.is_some_and(|p| app.ui.panes.pane(p).is_some_and(|x| x.docs.len() <= 1));
    if let Some((pane, _, strip)) = f.panes.iter().find(|(_, _, s)| s.rect.expand(4.0).contains(p)) {
        if !(alone && from == Some(*pane)) {
            return Drop::Join(*pane, strip.slot(p.x));
        }
        return Drop::Float(p);
    }
    let area = f.area;
    if area.contains(p) {
        let only_pane = alone && app.ui.panes.single();
        if !only_pane {
            if p.x > area.right() - AREA_EDGE {
                return Drop::Split(None, Side::Right);
            }
            if p.x < area.left() + AREA_EDGE {
                return Drop::Split(None, Side::Left);
            }
            // No whole-width zone at the bottom: the bottom of a pane splits that pane (a third
            // document below the right one).
        }
        if let Some((pane, r, strip)) = f.panes.iter().find(|(_, r, _)| r.contains(p)) {
            if alone && from == Some(*pane) {
                return Drop::Float(p);
            }
            let body = Rect::from_min_max(pos2(r.left(), strip.rect.bottom().max(r.top())), r.max);
            if body.width() > 1.0 && body.height() > 1.0 {
                let u = (p.x - body.left()) / body.width();
                let v = (p.y - body.top()) / body.height();
                // The nearest edge, if the pointer is in its outer quarter.
                let (w, h) = (body.width(), body.height());
                let edges = [(u, w, Side::Left), (1.0 - u, w, Side::Right), (v, h, Side::Top), (1.0 - v, h, Side::Bottom)];
                if let Some(&(d, size, side)) = edges.iter().min_by(|a, b| a.0.total_cmp(&b.0))
                    && (if window { d * size < AREA_EDGE } else { d < PANE_EDGE })
                {
                    return Drop::Split(Some(*pane), side);
                }
            }
        }
    }
    Drop::Float(p)
}

/// The half of `r` on `side` (`share` of it).
fn side_rect(r: Rect, side: Side, share: f32) -> Rect {
    match side {
        Side::Left => Rect::from_min_size(r.min, vec2(r.width() * share, r.height())),
        Side::Right => Rect::from_min_max(pos2(r.right() - r.width() * share, r.top()), r.max),
        Side::Top => Rect::from_min_size(r.min, vec2(r.width(), r.height() * share)),
        Side::Bottom => Rect::from_min_max(pos2(r.left(), r.bottom() - r.height() * share), r.max),
    }
}

/// Show where a drop would land: the strip outlined with an insertion line, the part of a pane or
/// of the area a new pane takes, or the window a floating document makes.
pub fn preview(app: &PhotocraftApp, ctx: &egui::Context, to: Drop) {
    let t = Tokens::get(ctx);
    let f = frames(ctx);
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    let fg = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("doc-pane-drop")));
    let fill = |r: Rect| {
        fg.rect_filled(r, t.radius_sm, t.accent.gamma_multiply(0.18));
        fg.rect_stroke(r, t.radius_sm, Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
    };
    match to {
        Drop::Join(pane, slot) => {
            if let Some((_, _, strip)) = f.panes.iter().find(|(p, _, _)| *p == pane) {
                fg.rect_filled(strip.rect, 0.0, t.accent.gamma_multiply(0.12));
                fg.rect_stroke(strip.rect, 0.0, Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
                let x = match strip.tabs.iter().find(|(i, _)| *i >= slot) {
                    Some((_, r)) => r.left(),
                    None => strip.tabs.last().map_or(strip.rect.left() + 6.0, |(_, r)| r.right()),
                };
                fg.line_segment([pos2(x, strip.rect.top() + 3.0), pos2(x, strip.rect.bottom() - 3.0)], Stroke::new(3.0, t.accent));
            }
        }
        Drop::Split(None, side) => fill(side_rect(f.area, side, AREA_SHARE)),
        Drop::Split(Some(pane), side) => {
            if let Some((_, r, _)) = f.panes.iter().find(|(p, _, _)| *p == pane) {
                fill(side_rect(*r, side, 0.5));
            }
        }
        Drop::Float(p) => crate::doc_float::paint_float_ghost(ctx, app.last_canvas_rect, p),
    }
}

/// Put document `idx` where `to` says, and make it active.
pub fn apply(app: &mut PhotocraftApp, idx: usize, to: Drop) -> Result<(), String> {
    let id = app.session.documents().get(idx).map(|st| st.doc.id).ok_or_else(|| format!("no document {idx}"))?;
    match to {
        Drop::Float(p) => {
            app.ui.panes.remove(id);
            crate::doc_float::float(app, idx, Some(p))?;
        }
        Drop::Join(pane, slot) => {
            app.ui.floating_docs.retain(|f| f.doc != id);
            if app.ui.panes.pane_of(id) != Some(pane) {
                app.ui.panes.add(pane, id);
            }
            let n = app.session.documents().len();
            let to = if slot > idx { slot - 1 } else { slot }.min(n.saturating_sub(1));
            if to != idx {
                app.run("document.move", serde_json::json!({"document": idx, "to": to}))?;
            }
            app.ui.panes.focused = pane;
        }
        Drop::Split(target, side) => {
            app.ui.floating_docs.retain(|f| f.doc != id);
            app.ui.panes.split(target, side, id);
        }
    }
    if let Some(i) = app.session.documents().iter().position(|st| st.doc.id == id) {
        if let Some(v) = app.ui.views.get_mut(i) {
            v.fit_pending = true;
        }
        app.session.set_active(i);
    }
    sync(app);
    Ok(())
}

// ----------------------------------------------------------------------------- drawing

/// The document area split into panes: each pane's tab strip and shown document, and the
/// dividers between them.
pub fn show(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let area = ui.available_rect_before_wrap();
    ui.allocate_rect(area, Sense::hover());
    let (panes, dividers) = layout(&app.ui.panes.root, area);
    let active = app.session.active_index();
    let focused = app.ui.panes.focused;
    let mut drawn = Vec::with_capacity(panes.len());
    for (pane, r) in panes {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)));
        child.set_clip_rect(r);
        if t.pro {
            child.spacing_mut().item_spacing.y = 0.0;
        }
        let strip = crate::canvas::pane_tabs(app, &mut child, pane);
        let body = Rect::from_min_max(pos2(r.left(), strip.rect.bottom().max(r.top())), r.max);
        if pane == focused {
            app.last_canvas_rect = body;
            app.drop_canvas_rect = Some(body);
            app.tab_strip = Some(strip.clone());
        }
        match shown(app, pane) {
            Some(d) => {
                let primary = Some(d) == active;
                let view = app.ui.views.get(d).cloned().unwrap_or_default();
                let v = crate::canvas::canvas_view(app, &mut child, d, body, view, primary);
                if !primary {
                    if let Some(slot) = app.ui.views.get_mut(d) {
                        *slot = v;
                    }
                    if child.input(|i| i.pointer.primary_pressed()) && child.rect_contains_pointer(body) {
                        app.session.set_active(d);
                    }
                }
            }
            None => crate::canvas::paint_dots(&child, body),
        }
        // The pane with the active document: a thin accent line under its strip.
        if active.is_some_and(|a| in_pane(app, a, pane)) {
            ui.painter().line_segment([pos2(r.left(), body.top()), pos2(r.right(), body.top())], Stroke::new(2.0, t.accent));
        }
        drawn.push((pane, r, strip));
    }
    for d in dividers {
        let resp = ui.interact(d.rect, ui.id().with(("pane-divider", d.path.clone())), Sense::drag());
        let hot = resp.hovered() || resp.dragged();
        ui.painter().rect_filled(d.rect, 0.0, if hot { t.accent.gamma_multiply(0.6) } else { t.separator });
        if hot {
            ui.ctx().set_cursor_icon(if d.across { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::ResizeVertical });
        }
        if resp.dragged()
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(ratio) = ratio_at(&mut app.ui.panes.root, &d.path)
        {
            let r = if d.across { (p.x - d.whole.left()) / d.whole.width().max(1.0) } else { (p.y - d.whole.top()) / d.whole.height().max(1.0) };
            if r.is_finite() {
                *ratio = r.clamp(0.1, 0.9);
            }
        }
    }
    record(ui.ctx(), area, drawn);
}

#[cfg(test)]
#[path = "doc_panes_tests.rs"]
mod tests;
