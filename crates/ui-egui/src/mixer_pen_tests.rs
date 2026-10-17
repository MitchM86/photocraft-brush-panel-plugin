//! The Mixer Brush through the real UI: settings changed with the options bar's own controls,
//! strokes painted with pointer events while pen samples at light pressure come through the
//! stylus feed (as the X11 tablet backend writes them), on a layer that is offset and larger than
//! the canvas. The live preview and the commit run as when painting by hand.

use egui::{Pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

use crate::PhotocraftApp;
use crate::canvas::ViewXform;
use crate::state::Tool;
use crate::stylus::PenSample;

const ORANGE: [f32; 4] = [240.0 / 255.0, 120.0 / 255.0, 20.0 / 255.0, 1.0];
const SKY: [f32; 4] = [235.0 / 255.0, 225.0 / 255.0, 190.0 / 255.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
const GREEN: [f32; 4] = [0.0, 0.6, 0.0, 1.0];
/// A pale tan load, like the patch ⌥-clicked off the painting.
const TAN: &str = "#bb8251";
/// The user's brush.
const SIZE: f32 = 173.0;

/// A 750 × 1500 document whose layer has Layer 33's geometry: 2245 px wide at X −747, blue past
/// the canvas on the left and green on the right; sky (cream) above y 450, orange below.
fn app() -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.run("file.new", json!({"width": 750, "height": 1500})).unwrap();
    {
        let st = app.session.active_mut().unwrap();
        let id = st.active_layer.unwrap();
        let doc = std::sync::Arc::make_mut(&mut st.doc);
        let s = doc.layer_mut(id).unwrap().surface_mut().unwrap();
        for (r, c) in [((-747, 0, 0, 1500), BLUE), ((0, 0, 750, 450), SKY), ((0, 450, 750, 1500), ORANGE), ((750, 0, 1498, 1500), GREEN)] {
            s.fill_rect(photocraft_geom::Rect::new(r.0, r.1, r.2, r.3), &c);
        }
    }
    app.run("tools.setColors", json!({"foreground": TAN})).unwrap();
    app.ui.tool = Tool::MixerBrush;
    // The brush in hand when painting by hand: Soft Round, Transfer › Opacity on Pen Pressure,
    // Pressure for Size off. (Its size is set on the brush tip, not a field of this bar.)
    app.run(
        "tools.setBrush",
        json!({"brush": {"size": SIZE, "hardness": 0.0, "pressureSize": false, "transfer": {"enabled": true, "opacity": {"control": "penPressure"}}}}),
    )
    .unwrap();
    app
}

fn harness(app: PhotocraftApp) -> Harness<'static, PhotocraftApp> {
    let mut h = Harness::builder().with_size(vec2(1500.0, 900.0)).build_ui_state(
        |ui, app: &mut PhotocraftApp| {
            let ctx = ui.ctx().clone();
            if !ctx.fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                return;
            }
            crate::shortcuts::handle(app, &ctx);
            crate::panels::options_bar(app, ui);
            egui::CentralPanel::default().show(ui, |ui| crate::canvas::document_area(app, ui));
        },
        app,
    );
    PhotocraftApp::setup_context(&h.ctx, crate::theme::ThemeKind::Studio);
    h.run_steps(4);
    let v = &mut h.state_mut().ui.views[0];
    v.zoom = 0.5;
    v.center = [375.0, 750.0];
    v.fit_pending = false;
    h.run_steps(2);
    h
}

/// A click at screen point `p`.
fn click_at(h: &mut Harness<'static, PhotocraftApp>, p: Pos2) {
    h.event(egui::Event::PointerMoved(p));
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.run_steps(1);
    }
}

/// Type `value` into the options bar's number field labelled `label`.
fn set_field(h: &mut Harness<'static, PhotocraftApp>, label: &str, value: &str) {
    let at = h.get_by_label(label).rect();
    let field = h
        .query_all_by_role(egui::accesskit::Role::SpinButton)
        .map(|n| n.rect())
        .filter(|r| r.left() >= at.right() - 1.0 && (r.center().y - at.center().y).abs() < 8.0)
        .min_by(|a, b| a.left().total_cmp(&b.left()))
        .unwrap_or_else(|| panic!("no field after {label}"));
    click_at(h, field.center());
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.run_steps(1);
    h.event(egui::Event::Text(value.into()));
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
}

/// Pick `mode` ("RGB" or "Pigment") in the options bar's Mixing dropdown.
fn set_mixing(h: &mut Harness<'static, PhotocraftApp>, mode: &str) {
    let at = h.get_by_label("Mixing").rect();
    let button = h
        .query_all_by_role(egui::accesskit::Role::ComboBox)
        .chain(h.query_all_by_role(egui::accesskit::Role::Button))
        .map(|n| n.rect())
        .filter(|r| r.left() >= at.right() - 1.0 && (r.center().y - at.center().y).abs() < 8.0)
        .min_by(|a, b| a.left().total_cmp(&b.left()))
        .expect("the Mixing dropdown");
    click_at(h, button.center());
    h.run_steps(2);
    let option = h.query_all_by_label(mode).map(|n| n.rect()).filter(|r| r.top() > at.bottom()).min_by(|a, b| a.top().total_cmp(&b.top())).expect("the option");
    click_at(h, option.center());
    h.run_steps(2);
}

fn screen(h: &Harness<'static, PhotocraftApp>, x: f32, y: f32) -> Pos2 {
    let app = h.state();
    let v = &app.ui.views[0];
    let xf = ViewXform { rect: crate::rulers::content_rect(app, app.last_canvas_rect), zoom: v.zoom, center: v.center, flip: app.ui.view.flip_horizontal, rotation: v.rotation };
    xf.to_screen(x, y)
}

/// A pen stroke between document points at `pressure`: each pointer event comes with the pen's
/// sample in the stylus feed, then the pen lifts (pressure 0).
fn pen_stroke(h: &mut Harness<'static, PhotocraftApp>, from: [f32; 2], to: [f32; 2], pressure: f32) {
    let pen = |h: &mut Harness<'static, PhotocraftApp>, p: f32| h.state().stylus.feed.set(Some(PenSample { pressure: p, ..Default::default() }));
    let p = screen(h, from[0], from[1]);
    pen(h, pressure);
    h.event(egui::Event::PointerMoved(p));
    h.run_steps(1);
    pen(h, pressure);
    h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(1);
    let n = 40;
    for i in 1..=n {
        let t = i as f32 / n as f32;
        pen(h, pressure);
        h.event(egui::Event::PointerMoved(screen(h, from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t)));
        h.run_steps(1);
    }
    pen(h, 0.0);
    h.event(egui::Event::PointerButton { pos: screen(h, to[0], to[1]), button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(2);
    h.state().stylus.feed.set(None);
    h.run_steps(1);
}

fn pixel(h: &Harness<'static, PhotocraftApp>, x: i32, y: i32) -> [f32; 4] {
    let st = h.state().session.active().unwrap();
    st.doc.layer(st.active_layer.unwrap()).unwrap().surface().unwrap().rgba(x, y)
}

fn dist(a: [f32; 4], b: [f32; 4]) -> f32 {
    (0..3).map(|c| (a[c] - b[c]).powi(2)).sum::<f32>().sqrt()
}

fn tan() -> [f32; 4] {
    let v = u32::from_str_radix(&TAN[1..], 16).unwrap();
    [((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0, 1.0]
}

/// How far `got` is from halfway between `a` and `b`, as a share of the largest channel gap.
fn off_halfway(got: [f32; 4], a: [f32; 4], b: [f32; 4]) -> f32 {
    let span = (0..3).map(|c| (a[c] - b[c]).abs()).fold(0.0, f32::max);
    (0..3).map(|c| (got[c] - (a[c] + b[c]) / 2.0).abs()).fold(0.0, f32::max) / span
}

/// The user's strokes: a 173 px Soft Round, Wet/Load/Mix 50 and Flow 100 typed into the options
/// bar, RGB or Pigment picked there, pen pressure 0.3, a pale tan load, on a layer with Layer 33's
/// offset. Over the sky and over the orange the stroke is (in RGB) close to halfway between the
/// load and what it crosses; a stroke from the sky into the orange is within 10 % of one over
/// orange alone a brush width past the edge; light pressure changes nothing; and nothing comes from
/// the layer's off-canvas blue or green.
#[test]
fn pen_strokes_through_the_options_bar_pick_up_what_they_cross() {
    for mode in ["RGB", "Pigment"] {
        let paint = |pressure: f32| {
            let mut h = harness(app());
            set_field(&mut h, "Wet", "50");
            set_field(&mut h, "Load", "50");
            set_field(&mut h, "Mix", "50");
            set_field(&mut h, "Flow", "100");
            set_mixing(&mut h, mode);
            let m = h.state().session.tools.brush.mixer.clone();
            assert_eq!((m.wet, m.load, m.mix, m.flow), (0.5, 0.5, 0.5, 1.0), "{mode}: the options bar set {m:?}");
            assert_eq!(m.mode, if mode == "RGB" { photocraft_paint::MixMode::Rgb } else { photocraft_paint::MixMode::Pigment });
            // Across the sky; across the orange; from the sky down into the orange; and down the
            // orange alone, as far as the last has come by the point compared.
            pen_stroke(&mut h, [40.0, 200.0], [700.0, 200.0], pressure);
            pen_stroke(&mut h, [40.0, 1250.0], [700.0, 1250.0], pressure);
            // From the canvas's left edge along the orange, the tip starting over the layer's
            // hidden off-canvas blue: what the brush picked up there would come along onto the
            // canvas.
            pen_stroke(&mut h, [20.0, 1400.0], [700.0, 1400.0], pressure);
            pen_stroke(&mut h, [200.0, 100.0], [200.0, 1000.0], pressure);
            pen_stroke(&mut h, [560.0, 500.0], [560.0, 1400.0], pressure);
            assert_eq!(h.state().session.journal.iter().filter(|(id, _)| id == "paint.mixerBrush").count(), 5, "{mode}: five strokes committed");
            // The pen's pressure reached the strokes.
            let (_, last) = h.state().session.journal.iter().rev().find(|(id, _)| id == "paint.mixerBrush").unwrap();
            let pressures: Vec<f64> = last["points"].as_array().unwrap().iter().filter_map(|q| q[2].as_f64()).collect();
            assert!(!pressures.is_empty() && pressures.iter().all(|p| (p - f64::from(pressure)).abs() < 0.01), "{mode}: pressures {pressures:?}");
            // Two widths into the sky and orange strokes; one width past the edge (y 450 + 173),
            // 523 px along, and as far along the orange-only stroke.
            (pixel(&h, 386, 200), pixel(&h, 386, 1250), pixel(&h, 200, 623), pixel(&h, 560, 1023), pixel(&h, 90, 1400))
        };
        let (sky, orange, crossed, alone, near_edge) = paint(0.3);
        let load = tan();
        if mode == "RGB" {
            // Mix 50 %: about halfway (Load 50 % has thinned the load a little in two widths).
            assert!(off_halfway(sky, load, SKY) < 0.08, "{mode}: over the sky {sky:?}, {:.1} % off halfway", off_halfway(sky, load, SKY) * 100.0);
            assert!(off_halfway(orange, load, ORANGE) < 0.08, "{mode}: over orange {orange:?}, {:.1} % off halfway", off_halfway(orange, load, ORANGE) * 100.0);
        }
        // In both modes the strokes clearly differ, each leaning to what it crossed.
        assert!(dist(orange, ORANGE) < dist(sky, ORANGE) && dist(sky, SKY) < dist(orange, SKY), "{mode}: sky {sky:?} orange {orange:?}");
        assert!(dist(orange, sky) > 0.15, "{mode}: strokes over the sky {sky:?} and orange {orange:?} look alike");
        let gap = (0..3).map(|c| (sky[c] - orange[c]).abs()).fold(0.0, f32::max);
        let lag = (0..3).map(|c| (crossed[c] - alone[c]).abs()).fold(0.0, f32::max) / gap;
        assert!(lag < 0.1, "{mode}: one width into the orange {crossed:?} vs orange alone {alone:?}: {:.1} % of the way back", lag * 100.0);
        // Starting at the canvas edge it brings nothing of the layer's off-canvas blue along: it is
        // the stroke over orange (picking the blue up tints it by about 0.1).
        assert!(dist(near_edge, orange) < 0.04, "{mode}: from the edge {near_edge:?} vs over orange {orange:?}");
        // Light pressure (opacity on pressure, which the Mixer Brush has none of) changes nothing.
        let (_, full, ..) = paint(1.0);
        assert!(dist(full, orange) < 0.02, "{mode}: pressure 1 {full:?} vs 0.3 {orange:?}");
    }
}

/// A document of `w × h` with one opaque layer painted by `fill(x, y)`.
fn app_with(w: i32, h: i32, fill: impl Fn(i32, i32) -> [f32; 4]) -> PhotocraftApp {
    let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
    app.run("file.new", json!({"width": w, "height": h})).unwrap();
    {
        let st = app.session.active_mut().unwrap();
        let id = st.active_layer.unwrap();
        let doc = std::sync::Arc::make_mut(&mut st.doc);
        let s = doc.layer_mut(id).unwrap().surface_mut().unwrap();
        for y in 0..h {
            for x in 0..w {
                s.fill_rect(photocraft_geom::Rect::new(x, y, x + 1, y + 1), &fill(x, y));
            }
        }
    }
    app.ui.tool = Tool::MixerBrush;
    app.run("tools.setBrush", json!({"brush": {"size": 60, "hardness": 0.0, "pressureSize": false}})).unwrap();
    app
}

/// Set Wet, Load, Mix (and Flow 100) and the mixing mode through the options bar.
fn bar(h: &mut Harness<'static, PhotocraftApp>, wet: &str, load: &str, mix: &str, mode: &str) {
    set_field(h, "Wet", wet);
    set_field(h, "Load", load);
    set_field(h, "Mix", mix);
    set_field(h, "Flow", "100");
    set_mixing(h, mode);
}

fn to8(c: [f32; 4]) -> [i32; 4] {
    c.map(|v| (v * 255.0).round() as i32)
}

fn layer_pixel(h: &Harness<'static, PhotocraftApp>, id: photocraft_doc::LayerId, x: i32, y: i32) -> [f32; 4] {
    h.state().session.active().unwrap().doc.layer(id).unwrap().surface().unwrap().rgba(x, y)
}

/// Step 1 check: no holes. An opaque image layer over a magenta one: a Wet/Load/Mix 50 pen stroke
/// leaves every stroked pixel of the image layer opaque (alpha 255), so no magenta shows through.
#[test]
fn no_holes_a_stroke_keeps_an_opaque_layer_opaque() {
    for mode in ["RGB", "Pigment"] {
        let mut app = app_with(400, 200, |_, _| [1.0, 0.0, 1.0, 1.0]);
        app.run("layer.new.layer", json!({})).unwrap();
        let image = app.session.active().unwrap().active_layer.unwrap();
        {
            let st = app.session.active_mut().unwrap();
            let doc = std::sync::Arc::make_mut(&mut st.doc);
            let s = doc.layer_mut(image).unwrap().surface_mut().unwrap();
            s.fill_rect(photocraft_geom::Rect::new(0, 0, 200, 200), &ORANGE);
            s.fill_rect(photocraft_geom::Rect::new(200, 0, 400, 200), &SKY);
        }
        app.run("tools.setColors", json!({"foreground": TAN})).unwrap();
        let mut h = harness(app);
        bar(&mut h, "50", "50", "50", mode);
        pen_stroke(&mut h, [30.0, 100.0], [370.0, 100.0], 0.3);
        let min = (0..400).flat_map(|x| (60..140).map(move |y| (x, y))).map(|(x, y)| to8(layer_pixel(&h, image, x, y))[3]).min().unwrap();
        eprintln!("STEP1 no holes {mode}: lowest alpha under the stroke {min}");
        assert_eq!(min, 255, "{mode}: the stroke made holes");
    }
}

/// Step 1 check: identity. A clean brush smudged across flat sky leaves the sky as it was (±1).
#[test]
fn identity_a_clean_smudge_over_flat_sky_changes_nothing() {
    for mode in ["RGB", "Pigment"] {
        let mut app = app_with(400, 200, |_, _| SKY);
        app.run("paint.mixerClean", json!({})).unwrap();
        app.ui.tool_options.mixer_load_after = false;
        let mut h = harness(app);
        bar(&mut h, "50", "50", "50", mode);
        let id = h.state().session.active().unwrap().active_layer.unwrap();
        let before: Vec<[i32; 4]> = (40..360).step_by(20).map(|x| to8(layer_pixel(&h, id, x, 100))).collect();
        pen_stroke(&mut h, [30.0, 100.0], [370.0, 100.0], 0.3);
        assert!(h.state().session.journal.iter().any(|(c, _)| c == "paint.mixerBrush"), "{mode}: no stroke");
        let after: Vec<[i32; 4]> = (40..360).step_by(20).map(|x| to8(layer_pixel(&h, id, x, 100))).collect();
        let worst = before.iter().zip(&after).flat_map(|(a, b)| (0..4).map(move |c| (a[c] - b[c]).abs())).max().unwrap();
        eprintln!("STEP1 identity {mode}: sky {:?} before, {:?} after, worst {worst} levels", before[8], after[8]);
        assert!(worst <= 1, "{mode}: the smudge changed flat sky by {worst} levels");
    }
}

/// Step 1 check: pure paint. Wet 0, Mix 0, Load 100 lays down exactly the foreground colour.
#[test]
fn pure_paint_dry_unmixed_full_load_is_the_foreground() {
    for mode in ["RGB", "Pigment"] {
        let mut app = app_with(400, 200, |x, y| if (x / 20 + y / 20) % 2 == 0 { ORANGE } else { SKY });
        app.run("tools.setColors", json!({"foreground": TAN})).unwrap();
        let mut h = harness(app);
        bar(&mut h, "0", "100", "0", mode);
        pen_stroke(&mut h, [30.0, 100.0], [370.0, 100.0], 0.3);
        let id = h.state().session.active().unwrap().active_layer.unwrap();
        let want = to8(tan());
        let worst = (60..340).step_by(10).map(|x| to8(layer_pixel(&h, id, x, 100))).flat_map(|p| (0..4).map(move |c| (p[c] - want[c]).abs())).max().unwrap();
        eprintln!("STEP1 pure paint {mode}: {:?} vs foreground {want:?}, worst {worst} levels", to8(layer_pixel(&h, id, 200, 100)));
        assert!(worst <= 1, "{mode}: dry unmixed paint is off the foreground by {worst} levels");
    }
}
