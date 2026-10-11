use egui::{Modifiers, PointerButton, pos2, vec2};
use egui_kittest::Harness;

use super::*;

#[test]
fn the_ryb_wheel_maps_hues_both_ways_and_puts_yellow_opposite_violet() {
    for a in (0..360).map(|a| a as f32) {
        let h = wheel_to_hue(WheelKind::Ryb, a);
        assert!((hue_to_wheel(WheelKind::Ryb, h) - a).abs() < 1e-3, "{a}");
        assert_eq!(wheel_to_hue(WheelKind::Rgb, a), a);
    }
    // Painter's complements: red–green, yellow–violet, blue–orange.
    assert!((wheel_to_hue(WheelKind::Ryb, 180.0) - 120.0).abs() < 1e-3, "red's opposite is green");
    let yellow = hue_to_wheel(WheelKind::Ryb, 60.0);
    assert!((wheel_to_hue(WheelKind::Ryb, yellow + 180.0) - 275.0).abs() < 1e-3, "yellow's opposite is violet");
    for bad in [f32::NAN, f32::INFINITY, -720.0] {
        assert!(wheel_to_hue(WheelKind::Ryb, bad).is_finite() && hue_to_wheel(WheelKind::Ryb, bad).is_finite());
    }
}

#[test]
fn the_triangle_maps_saturation_and_value_both_ways() {
    let tri = triangle(pos2(100.0, 100.0), 80.0, 37.0);
    for (s, v) in [(1.0, 1.0), (0.0, 1.0), (0.5, 0.5), (0.25, 0.8), (0.9, 0.1)] {
        let p = triangle_point(tri, s, v);
        let (s2, v2) = triangle_sv(tri, p);
        assert!((v2 - v).abs() < 1e-4 && (v < 1e-3 || (s2 - s).abs() < 1e-3), "{s},{v} -> {s2},{v2}");
    }
    assert_eq!(triangle_sv(tri, tri[0]), (1.0, 1.0), "the pure hue corner");
    let (_, v) = triangle_sv(tri, tri[2]);
    assert!(v < 1e-4, "the black corner");
    // Outside it clamps; a degenerate triangle never panics.
    let (s, v) = triangle_sv(tri, pos2(-500.0, 900.0));
    assert!((0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&v));
    assert_eq!(triangle_sv([pos2(1.0, 1.0); 3], pos2(2.0, 2.0)), (0.0, 0.0));
}

#[test]
fn harmonies_and_recent_colors() {
    assert_eq!(Harmony::Complementary.offsets(), &[180.0]);
    assert_eq!(Harmony::Tetrad.offsets().len(), 3);
    let mut recent = Vec::new();
    for i in 0..20 {
        remember(&mut recent, [i as f32 / 20.0, 0.0, 0.0, 1.0]);
    }
    assert_eq!(recent.len(), RECENT);
    let again = recent[5];
    remember(&mut recent, again);
    assert_eq!(recent.len(), RECENT, "no repeats");
    let state = ColorWheelState { shape: WheelShape::Triangle, kind: WheelKind::Ryb, harmony: Harmony::Triad, recent: recent.clone() };
    let back: ColorWheelState = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    assert_eq!(back, state);
}

fn harness(app: PhotocraftApp) -> Harness<'static, PhotocraftApp> {
    let mut h = Harness::builder().with_size(vec2(320.0, 420.0)).build_ui_state(
        |ui, app: &mut PhotocraftApp| {
            let ctx = ui.ctx().clone();
            if !ctx.fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                return;
            }
            panel(app, ui);
        },
        app,
    );
    PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::ProMedium);
    h.run_steps(3);
    h
}

fn click(h: &mut Harness<'static, PhotocraftApp>, p: egui::Pos2) {
    h.event(egui::Event::PointerMoved(p));
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.event(egui::Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(2);
}

#[test]
fn clicks_on_the_ring_and_the_square_set_the_foreground() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.tools.foreground = [0.8, 0.2, 0.2, 1.0];
    let mut h = harness(app);
    let r = last_rects(&h.ctx).expect("drawn");
    // The ring's right side (90°) is green-yellow on the RGB wheel: hue 90°.
    click(&mut h, ring_point(r.center, (r.inner + r.outer) / 2.0, 90.0));
    let [hue, s, _] = rgb_to_hsv([h.state().session.tools.foreground[0], h.state().session.tools.foreground[1], h.state().session.tools.foreground[2]]);
    assert!((hue - 90.0).abs() < 3.0 && s > 0.7, "{hue} {s}");
    // The square's top-left corner is white, its bottom black.
    let sq = last_rects(&h.ctx).unwrap().square;
    click(&mut h, sq.left_top() + vec2(1.0, 1.0));
    assert!(h.state().session.tools.foreground[..3].iter().all(|v| *v > 0.95), "{:?}", h.state().session.tools.foreground);
    click(&mut h, pos2(sq.center().x, sq.bottom() - 1.0));
    assert!(h.state().session.tools.foreground[..3].iter().all(|v| *v < 0.05));
    assert!(h.state().ui.color_wheel.recent.len() >= 2, "picked colours are remembered");
    // Editing the background leaves the foreground alone.
    h.state_mut().ui.color_panel.background = true;
    h.run_steps(2);
    let fg = h.state().session.tools.foreground;
    click(&mut h, ring_point(r.center, (r.inner + r.outer) / 2.0, 240.0));
    assert_eq!(h.state().session.tools.foreground, fg);
    let [hue, _, _] = rgb_to_hsv([h.state().session.tools.background[0], h.state().session.tools.background[1], h.state().session.tools.background[2]]);
    assert!((hue - 240.0).abs() < 3.0, "{hue}");
}

#[test]
fn a_harmony_swatch_click_picks_that_colour() {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.tools.foreground = [1.0, 0.0, 0.0, 1.0];
    app.ui.color_wheel.harmony = Harmony::Complementary;
    let mut h = harness(app);
    let r = last_rects(&h.ctx).unwrap();
    let swatch = r.harmony[1].expect("the complement's swatch");
    click(&mut h, swatch.center());
    let fg = h.state().session.tools.foreground;
    let [hue, _, _] = rgb_to_hsv([fg[0], fg[1], fg[2]]);
    assert!((hue - 180.0).abs() < 2.0, "red's RGB complement is cyan: {hue}");
    // On the painter's wheel red's complement is green.
    h.state_mut().session.tools.foreground = [1.0, 0.0, 0.0, 1.0];
    h.state_mut().ui.color_wheel.kind = WheelKind::Ryb;
    h.run_steps(2);
    let swatch = last_rects(&h.ctx).unwrap().harmony[1].unwrap();
    click(&mut h, swatch.center());
    let fg = h.state().session.tools.foreground;
    let [hue, _, _] = rgb_to_hsv([fg[0], fg[1], fg[2]]);
    assert!((hue - 120.0).abs() < 2.0, "{hue}");
}

/// `PHOTOCRAFT_UI_SHOT=<dir>` saves the panel in both shapes.
#[test]
fn the_panel_renders_in_both_shapes() {
    for (shape, name) in [(WheelShape::Square, "square"), (WheelShape::Triangle, "triangle")] {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
        app.session.tools.foreground = [0.85, 0.45, 0.15, 1.0];
        app.ui.color_wheel = ColorWheelState { shape, kind: WheelKind::Ryb, harmony: Harmony::Triad, recent: vec![[0.2, 0.4, 0.8, 1.0], [0.9, 0.9, 0.2, 1.0]] };
        let mut h = harness(app);
        h.run_steps(2);
        assert!(last_rects(&h.ctx).is_some());
        if let Some(dir) = std::env::var_os("PHOTOCRAFT_UI_SHOT")
            && let Ok(img) = h.render()
        {
            let _ = img.save(std::path::Path::new(&dir).join(format!("color-wheel-{name}.png")));
        }
    }
}

#[test]
fn the_cmyk_wheel_shows_and_picks_printable_colours() {
    use crate::color_picker_ui::out_of_gamut;
    // Saturated RGB blue and green don't print; the CMYK wheel shows (and picks) what does.
    for rgb in [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 1.0]] {
        assert!(out_of_gamut(rgb).is_some(), "{rgb:?} is outside CMYK");
        let p = shown(WheelKind::Cmyk, rgb);
        assert!(out_of_gamut(p).is_none(), "{rgb:?} shown as {p:?}");
        assert_eq!(shown(WheelKind::Rgb, rgb), rgb, "the RGB wheel shows it as it is");
    }
    // Printable colours stay (nearly) as they are.
    let tan = [0.7, 0.55, 0.4];
    assert!(shown(WheelKind::Cmyk, tan).iter().zip(tan).all(|(a, b)| (a - b).abs() < 0.03));
    // The hue ring is the RGB one: cyan opposite red.
    assert_eq!(wheel_to_hue(WheelKind::Cmyk, 180.0), 180.0);
    assert_eq!(serde_json::to_value(WheelKind::Cmyk).unwrap(), "cmyk");

    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.tools.foreground = [0.8, 0.2, 0.2, 1.0];
    app.ui.color_wheel.kind = WheelKind::Cmyk;
    let mut h = harness(app);
    let r = last_rects(&h.ctx).expect("drawn");
    // Blue on the ring: the foreground becomes the printable blue.
    click(&mut h, ring_point(r.center, (r.inner + r.outer) / 2.0, 240.0));
    let fg = h.state().session.tools.foreground;
    assert!(out_of_gamut([fg[0], fg[1], fg[2]]).is_none(), "{fg:?}");
    assert!(fg[2] > fg[0] && fg[2] > fg[1], "still blue: {fg:?}");
    // An unprintable colour set elsewhere shows ⚠; the panel still draws.
    h.state_mut().session.tools.foreground = [0.0, 1.0, 0.0, 1.0];
    h.run_steps(2);
    if let Some(dir) = std::env::var_os("PHOTOCRAFT_UI_SHOT")
        && let Ok(img) = h.render()
    {
        let _ = img.save(std::path::Path::new(&dir).join("color-wheel-cmyk.png"));
    }
}
