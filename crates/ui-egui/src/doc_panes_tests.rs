use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

use super::*;

fn app_with(names: &[&str]) -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    for name in names {
        app.run("file.new", json!({"width": 64, "height": 48, "name": name})).unwrap();
    }
    app.sync_views();
    sync(&mut app);
    app
}

fn id(app: &PhotocraftApp, name: &str) -> DocId {
    app.session.documents().iter().find(|d| d.doc.name == name).map(|d| d.doc.id).unwrap()
}

fn idx(app: &PhotocraftApp, name: &str) -> usize {
    app.session.documents().iter().position(|d| d.doc.name == name).unwrap()
}

/// Drop the document named `name` at `to`.
fn put(app: &mut PhotocraftApp, name: &str, to: Drop) {
    let i = idx(app, name);
    apply(app, i, to).unwrap();
}

fn pane_names(app: &PhotocraftApp) -> Vec<Vec<String>> {
    let (panes, _) = layout(&app.ui.panes.root, Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0)));
    panes
        .iter()
        .map(|(p, _)| (0..app.session.documents().len()).filter(|&i| in_pane(app, i, *p)).map(|i| app.session.documents()[i].doc.name.clone()).collect())
        .collect()
}

#[test]
fn splitting_right_then_below_the_right_pane() {
    let mut app = app_with(&["a", "b", "c"]);
    assert!(app.ui.panes.single());
    put(&mut app, "b", Drop::Split(None, Side::Right));
    assert_eq!(pane_names(&app), [vec!["a", "c"], vec!["b"]]);
    let right = app.ui.panes.pane_of(id(&app, "b")).unwrap();
    assert_eq!(app.ui.panes.focused, right);
    put(&mut app, "c", Drop::Split(Some(right), Side::Bottom));
    assert_eq!(pane_names(&app), [vec!["a"], vec!["b"], vec!["c"]]);
    // a on the left, the full height; b above c on the right.
    let area = Rect::from_min_size(Pos2::ZERO, vec2(900.0, 600.0));
    let (rects, dividers) = layout(&app.ui.panes.root, area);
    let r = |name: &str| rects.iter().find(|(p, _)| Some(*p) == app.ui.panes.pane_of(id(&app, name))).unwrap().1;
    assert_eq!(r("a").height(), 600.0);
    assert!((r("a").width() - (900.0 - GAP) * 0.65).abs() < 1.0, "the new right pane takes about a third: {:?}", r("a"));
    assert!(r("b").left() > r("a").right() && r("b").bottom() < r("c").top());
    assert_eq!(r("b").left(), r("c").left());
    assert_eq!(dividers.len(), 2);
    // Each document is active in turn without leaving its pane.
    app.session.set_active(idx(&app, "a"));
    sync(&mut app);
    assert_eq!(app.ui.panes.focused, app.ui.panes.pane_of(id(&app, "a")).unwrap());
    assert_eq!(pane_names(&app), [vec!["a"], vec!["b"], vec!["c"]]);
    // New documents open in the focused pane.
    app.run("file.new", json!({"width": 8, "height": 8, "name": "d"})).unwrap();
    app.sync_views();
    sync(&mut app);
    assert_eq!(pane_names(&app), [vec!["a", "d"], vec!["b"], vec!["c"]]);
    // A pane closes with its last document; its neighbour takes the space.
    app.run("file.close", json!({"document": idx(&app, "c")})).unwrap();
    app.sync_views();
    sync(&mut app);
    assert_eq!(pane_names(&app), [vec!["a", "d"], vec!["b"]]);
    // Joining the other pane's strip; then everything back in one.
    let left = app.ui.panes.pane_of(id(&app, "a")).unwrap();
    put(&mut app, "b", Drop::Join(left, 0));
    assert!(app.ui.panes.single(), "the right pane closed");
    assert_eq!(app.session.documents()[0].doc.name, "b", "joined before a");
    put(&mut app, "d", Drop::Split(None, Side::Left));
    assert!(!app.ui.panes.single());
    crate::doc_float::dock_all(&mut app);
    assert!(app.ui.panes.single());
    assert_eq!(pane_names(&app), [vec!["b", "a", "d"]]);
}

#[test]
fn floating_documents_leave_their_pane_and_dock_into_one() {
    let mut app = app_with(&["a", "b", "c"]);
    put(&mut app, "c", Drop::Split(None, Side::Right));
    put(&mut app, "c", Drop::Float(pos2(300.0, 200.0)));
    assert!(app.ui.panes.single(), "its pane closed when it floated");
    assert!(crate::doc_float::is_floating(&app, idx(&app, "c")));
    let focused = app.ui.panes.focused;
    put(&mut app, "c", Drop::Split(Some(focused), Side::Bottom));
    assert!(!crate::doc_float::is_floating(&app, idx(&app, "c")));
    assert_eq!(pane_names(&app), [vec!["a", "b"], vec!["c"]]);
}

#[test]
fn drop_targets_from_the_drawn_panes() {
    let ctx = egui::Context::default();
    let mut app = app_with(&["a", "b", "c"]);
    let area = Rect::from_min_max(pos2(0.0, 0.0), pos2(900.0, 600.0));
    let strip = |r: Rect, tabs: Vec<(usize, Rect)>| TabStrip { rect: Rect::from_min_size(r.min, vec2(r.width(), 26.0)), tabs };
    let tab = |x: f32| Rect::from_min_size(pos2(x, 0.0), vec2(100.0, 26.0));
    record(&ctx, area, vec![(0, area, strip(area, vec![(0, tab(0.0)), (1, tab(100.0)), (2, tab(200.0))]))]);
    let b = idx(&app, "b");
    assert_eq!(target(&app, &ctx, b, pos2(880.0, 300.0)), Drop::Split(None, Side::Right), "beside the icon rail");
    assert_eq!(target(&app, &ctx, b, pos2(450.0, 590.0)), Drop::Split(Some(0), Side::Bottom), "the bottom splits the pane");
    assert_eq!(target(&app, &ctx, b, pos2(800.0, 300.0)), Drop::Split(Some(0), Side::Right), "the pane's right quarter");
    assert_eq!(target(&app, &ctx, b, pos2(450.0, 520.0)), Drop::Split(Some(0), Side::Bottom));
    assert_eq!(target(&app, &ctx, b, pos2(450.0, 300.0)), Drop::Float(pos2(450.0, 300.0)), "the middle floats");
    assert_eq!(target(&app, &ctx, b, pos2(210.0, 10.0)), Drop::Join(0, 2));
    // A window moved around only splits a pane right at its edge.
    assert_eq!(window_target(&app, &ctx, b, pos2(800.0, 300.0)), Drop::Float(pos2(800.0, 300.0)));
    assert_eq!(window_target(&app, &ctx, b, pos2(450.0, 570.0)), Drop::Split(Some(0), Side::Bottom));
    assert_eq!(window_target(&app, &ctx, b, pos2(890.0, 300.0)), Drop::Split(None, Side::Right));
    // The only document of the only pane can't split it.
    let mut one = app_with(&["solo"]);
    sync(&mut one);
    assert!(matches!(target(&one, &ctx, 0, pos2(880.0, 300.0)), Drop::Float(_)));
    // Nonsense pointers and empty frames never panic.
    record(&ctx, Rect::NOTHING, Vec::new());
    for p in [pos2(f32::NAN, 1.0), pos2(-1e9, 1e9), pos2(0.0, 0.0)] {
        let _ = target(&app, &ctx, b, p);
    }
    sync(&mut app);
}

fn harness(app: PhotocraftApp) -> Harness<'static, (PhotocraftApp, bool)> {
    let mut h = Harness::builder().with_size(egui::vec2(900.0, 600.0)).build_ui_state(
        |ui, (app, ready): &mut (PhotocraftApp, bool)| {
            if *ready {
                let ctx = ui.ctx().clone();
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| crate::canvas::document_area(app, ui));
                crate::doc_float::windows(app, &ctx);
            }
        },
        (app, false),
    );
    PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::Pro);
    h.state_mut().1 = true;
    h.run_steps(3);
    h
}

fn drag(h: &mut Harness<'static, (PhotocraftApp, bool)>, from: Pos2, to: Pos2) {
    h.drag_at(from);
    h.run_steps(1);
    h.hover_at(from + (to - from) / 2.0);
    h.run_steps(1);
    h.hover_at(to);
    h.run_steps(1);
    h.drop_at(to);
    h.run_steps(3);
}

/// The user's case: a tab dragged beside the icon rail makes a right pane; a third tab dragged to
/// the bottom of that pane goes below it. Then the divider resizes the panes.
#[test]
fn dragging_tabs_into_a_right_pane_and_below_it() {
    let mut h = harness(app_with(&["a", "b", "c"]));
    let area = frames(&h.ctx).area;
    let b = h.get_by_label_contains("b @").rect();
    drag(&mut h, b.center(), pos2(area.right() - 10.0, area.center().y));
    assert_eq!(pane_names(&h.state().0), [vec!["a", "c"], vec!["b"]]);
    if let Some(dir) = std::env::var_os("PHOTOCRAFT_UI_SHOT")
        && let Ok(img) = h.render()
    {
        let _ = img.save(std::path::Path::new(&dir).join("panes-2.png"));
    }
    let right = frames(&h.ctx).panes.iter().find(|(p, _, _)| Some(*p) == h.state().0.ui.panes.pane_of(id(&h.state().0, "b"))).map(|(_, r, _)| *r).unwrap();
    let c = h.get_by_label_contains("c @").rect();
    drag(&mut h, c.center(), pos2(right.center().x, right.bottom() - 20.0));
    assert_eq!(pane_names(&h.state().0), [vec!["a"], vec!["b"], vec!["c"]]);
    let f = frames(&h.ctx);
    assert_eq!(f.panes.len(), 3);
    // c is below b only: same column, a keeps the full height.
    let rect_of = |name: &str| {
        let p = h.state().0.ui.panes.pane_of(id(&h.state().0, name)).unwrap();
        f.panes.iter().find(|(x, _, _)| *x == p).map(|(_, r, _)| *r).unwrap()
    };
    assert!((rect_of("c").left() - rect_of("b").left()).abs() < 1.0 && rect_of("c").top() > rect_of("b").bottom(), "{:?} {:?}", rect_of("b"), rect_of("c"));
    assert!((rect_of("a").height() - f.area.height()).abs() < 1.0);
    if let Some(dir) = std::env::var_os("PHOTOCRAFT_UI_SHOT")
        && let Ok(img) = h.render()
    {
        let _ = img.save(std::path::Path::new(&dir).join("panes-3.png"));
    }
    // The divider between the left pane and the right column, dragged left.
    let (_, dividers) = layout(&h.state().0.ui.panes.root, f.area);
    let d = dividers.iter().find(|d| d.across).unwrap().rect;
    let before = layout(&h.state().0.ui.panes.root, f.area).0[0].1.width();
    drag(&mut h, d.center(), d.center() - vec2(150.0, 0.0));
    let after = layout(&h.state().0.ui.panes.root, frames(&h.ctx).area).0[0].1.width();
    assert!((before - after - 150.0).abs() < 4.0, "{before} -> {after}");
    // Each pane shows its document; a click in one makes it active.
    let left = frames(&h.ctx).panes[0].1;
    h.drag_at(left.center());
    h.drop_at(left.center());
    h.run_steps(2);
    assert_eq!(h.state().0.session.active().unwrap().doc.name, "a");
}
