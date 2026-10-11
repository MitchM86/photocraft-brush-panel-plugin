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
    app
}

fn names(app: &PhotocraftApp) -> Vec<String> {
    app.session.documents().iter().map(|d| d.doc.name.clone()).collect()
}

/// The tab area and the floating windows, as a frame draws them.
fn harness(app: PhotocraftApp, kind: crate::theme::ThemeKind) -> Harness<'static, (PhotocraftApp, bool)> {
    let mut h = Harness::builder().with_size(egui::vec2(900.0, 600.0)).build_ui_state(
        |ui, (app, ready): &mut (PhotocraftApp, bool)| {
            if *ready {
                let ctx = ui.ctx().clone();
                egui::CentralPanel::default().show(ui, |ui| crate::canvas::document_area(app, ui));
                windows(app, &ctx);
            }
        },
        (app, false),
    );
    PhotocraftApp::setup_context(&h.ctx, kind);
    h.state_mut().1 = true;
    h.run_steps(3);
    h
}

fn drag(h: &mut Harness<'static, (PhotocraftApp, bool)>, from: egui::Pos2, to: egui::Pos2) {
    h.drag_at(from);
    h.run_steps(1);
    let mid = from + (to - from) / 2.0;
    h.hover_at(mid);
    h.run_steps(1);
    h.hover_at(to);
    h.run_steps(1);
    h.drop_at(to);
    h.run_steps(3);
}

#[test]
fn which_documents_float_and_which_the_tab_area_shows() {
    let mut app = app_with(&["a", "b", "c"]);
    assert_eq!(docked(&app), [0, 1, 2]);
    assert_eq!(shown_docked(&app), Some(2));
    float(&mut app, 2, None).unwrap();
    assert!(is_floating(&app, 2));
    assert_eq!(app.session.active_index(), Some(2), "the floated document stays active");
    assert_eq!(docked(&app), [0, 1]);
    // The tab area falls back to a docked document while the floating one is active.
    assert_eq!(shown_docked(&app), Some(0));
    app.session.set_active(1);
    track(&mut app);
    app.session.set_active(2);
    assert_eq!(shown_docked(&app), Some(1), "the docked one last active");
    // Reordering keeps floating windows with their documents.
    app.run("document.move", json!({"document": 2, "to": 0})).unwrap();
    assert!(is_floating(&app, 0) && !is_floating(&app, 2));
    // Docking puts it before the tab at the slot.
    dock(&mut app, 0, Some(2)).unwrap();
    assert_eq!(names(&app), ["a", "c", "b"]);
    assert!(app.ui.floating_docs.is_empty());
    assert_eq!(app.session.active().unwrap().doc.name, "c");
    // Closing a floating document forgets its window.
    float(&mut app, 1, None).unwrap();
    app.run("file.close", json!({"document": 1})).unwrap();
    track(&mut app);
    assert!(app.ui.floating_docs.is_empty());
    float_all(&mut app);
    assert_eq!(app.ui.floating_docs.len(), 2);
    assert_eq!(shown_docked(&app), None);
    dock_all(&mut app);
    assert!(app.ui.floating_docs.is_empty());
}

#[test]
fn window_rects_cascade_and_follow_the_pointer() {
    let canvas = Rect::from_min_size(pos2(50.0, 80.0), vec2(1000.0, 700.0));
    let a = window_rect(canvas, None, 0);
    let b = window_rect(canvas, None, 1);
    assert!(b[0] > a[0] && b[1] > a[1], "cascaded");
    assert!((320.0..=1400.0).contains(&a[2]) && (240.0..=1000.0).contains(&a[3]));
    let at = window_rect(canvas, Some(pos2(400.0, 300.0)), 0);
    assert!(Rect::from_min_size(pos2(at[0], at[1]), vec2(at[2], HEADER_H)).contains(pos2(400.0, 300.0)), "the pointer holds its title bar");
    // A degenerate workspace still gives a usable window.
    assert!(window_rect(Rect::NOTHING, None, 0).iter().all(|v| v.is_finite()));
    let strip = Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 26.0));
    assert!(!torn_off(strip, pos2(100.0, 40.0)), "just below the strip still reorders");
    assert!(torn_off(strip, pos2(100.0, 120.0)));
}

/// A tab dragged down off the strip floats; its window dragged back onto the strip docks it where
/// the line shows, in both tab strip styles.
#[test]
fn drag_a_tab_out_and_its_window_back_in() {
    for kind in [crate::theme::ThemeKind::Pro, crate::theme::ThemeKind::Studio] {
        let mut h = harness(app_with(&["a", "b", "c"]), kind);
        let b = h.get_by_label_contains("b").rect();
        drag(&mut h, b.center(), pos2(450.0, 320.0));
        let app = &h.state().0;
        assert_eq!(app.ui.floating_docs.len(), 1, "{kind:?}: the tab floated");
        assert!(is_floating(app, 1));
        assert_eq!(app.session.active().unwrap().doc.name, "b");
        let strip = app.tab_strip.clone().expect("the strip");
        assert_eq!(strip.tabs.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [0, 2], "{kind:?}: b left the strip");
        let win = app.ui.floating_docs[0].rect;
        if let Some(dir) = std::env::var_os("PHOTOCRAFT_UI_SHOT")
            && let Ok(img) = h.render()
        {
            let _ = img.save(std::path::Path::new(&dir).join(format!("floating-doc-{kind:?}.png")));
        }
        // Under the pointer (egui keeps the window on screen, so only across).
        assert!((win[0] - 390.0).abs() < 2.0, "{kind:?}: dropped under the pointer: {win:?}");
        // Its title bar dragged before tab a's middle: docked first.
        let head = pos2(win[0] + 60.0, win[1] + HEADER_H / 2.0);
        let a = strip.tabs.iter().find(|(i, _)| *i == 0).map(|(_, r)| *r).unwrap();
        drag(&mut h, head, pos2(a.left() + 4.0, a.center().y));
        let app = &h.state().0;
        assert!(app.ui.floating_docs.is_empty(), "{kind:?}: docked again");
        assert_eq!(names(app), ["b", "a", "c"], "{kind:?}");
        assert_eq!(app.session.active().unwrap().doc.name, "b");
    }
}

/// Dragging a floating window around the workspace (not onto the strip) only moves it.
#[test]
fn moving_a_floating_window_keeps_it_floating() {
    let mut app = app_with(&["a", "b"]);
    app.last_canvas_rect = Rect::from_min_size(pos2(0.0, 30.0), vec2(900.0, 570.0));
    float(&mut app, 1, Some(pos2(300.0, 200.0))).unwrap();
    let mut h = harness(app, crate::theme::ThemeKind::Pro);
    let r = h.state().0.ui.floating_docs[0].rect;
    let head = pos2(r[0] + 40.0, r[1] + HEADER_H / 2.0);
    drag(&mut h, head, head + vec2(120.0, 30.0));
    let app = &h.state().0;
    let moved = app.ui.floating_docs[0].rect;
    assert!((moved[0] - r[0] - 120.0).abs() < 2.0 && (moved[1] - r[1] - 30.0).abs() < 2.0, "{r:?} -> {moved:?}");
    assert_eq!(names(app), ["a", "b"]);
}

