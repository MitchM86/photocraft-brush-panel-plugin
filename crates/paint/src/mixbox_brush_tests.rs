use super::*;
use crate::{BrushSettings, StrokePoint};

const COBALT: [u8; 3] = [0, 33, 133];
const HANSA: [u8; 3] = [252, 211, 0];
const WHITE: [u8; 3] = [255, 255, 255];

fn rgba8(c: [u8; 3]) -> [f32; 4] {
    [f32::from(c[0]) / 255.0, f32::from(c[1]) / 255.0, f32::from(c[2]) / 255.0, 1.0]
}

fn u8s(c: [f32; 4]) -> [i32; 3] {
    [0, 1, 2].map(|i| (c[i] * 255.0).round() as i32)
}

fn image(w: i32, h: i32, c: [u8; 3]) -> Surface {
    let mut s = Surface::new(PixelFormat::RGBA8);
    s.fill_rect(Rect::new(0, 0, w, h), &rgba8(c));
    s
}

fn settings(wet: f32, load: f32, mix: f32) -> MixerSettings {
    MixerSettings { wet, load, mix, flow: 1.0, ..Default::default() }
}

/// A horizontal stroke, input points 1.7 px apart.
fn line(y: f64, x0: f64, x1: f64, size: f32, hardness: f32, spacing: f32) -> Stroke {
    let n = ((x1 - x0) / 1.7).floor() as usize;
    Stroke {
        brush: BrushSettings { size, hardness, spacing, pressure_size: false, ..Default::default() },
        points: (0..=n).map(|i| StrokePoint::new(x0 + i as f64 * 1.7, y, 1.0)).collect(),
    }
}

/// Largest channel difference (8-bit levels) from `c` to Mixbox's gradient between `a` and `b`,
/// and where on it the nearest point lies.
fn off_gradient(c: [i32; 3], a: [u8; 3], b: [u8; 3]) -> (i32, f32) {
    (0..=2000)
        .map(|i| {
            let t = i as f32 / 2000.0;
            let g = mixbox::lerp(&a, &b, t).map(i32::from);
            ((0..3).map(|k| (g[k] - c[k]).abs()).max().unwrap_or(0), t)
        })
        .min_by_key(|&(d, _)| d)
        .unwrap_or((i32::MAX, 0.0))
}

/// Largest channel difference from `c` to the straight RGB line between `a` and `b`.
fn off_rgb_line(c: [i32; 3], a: [u8; 3], b: [u8; 3]) -> f32 {
    (0..=2000)
        .map(|i| {
            let t = i as f32 / 2000.0;
            (0..3).map(|k| (f32::from(a[k]) + (f32::from(b[k]) - f32::from(a[k])) * t - c[k] as f32).abs()).fold(0.0, f32::max)
        })
        .fold(f32::MAX, f32::min)
}

/// Hue (degrees) and HSV saturation.
fn hue_sat(c: [i32; 3]) -> (f32, f32) {
    let [r, g, b] = c.map(|v| v as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let d = max - min;
    if d <= 1e-6 {
        return (0.0, 0.0);
    }
    let h = if max == r { 60.0 * ((g - b) / d).rem_euclid(6.0) } else if max == g { 60.0 * ((b - r) / d + 2.0) } else { 60.0 * ((r - g) / d + 4.0) };
    (h, d / max)
}

#[test]
fn the_pickup_is_the_weighted_latent_average_under_the_tip() {
    // Eq. 14: a hard 20 px tip centred on the edge between Cobalt Blue and Hansa Yellow, all
    // pickup (Mix 100 %, Wet 100 %): the first dab lays Mixbox's halfway mix, green.
    let mut img = image(40, 40, COBALT);
    img.fill_rect(Rect::new(20, 0, 40, 40), &rgba8(HANSA));
    let stroke = Stroke { brush: BrushSettings { size: 20.0, hardness: 1.0, pressure_size: false, ..Default::default() }, points: vec![StrokePoint::new(20.0, 20.0, 1.0)] };
    apply_mixbox_stroke(&mut img, &stroke, &settings(1.0, 0.0, 1.0), rgba8(WHITE), false, None, false);
    let want = mixbox::lerp(&COBALT, &HANSA, 0.5).map(i32::from);
    for x in [15, 20, 24] {
        let got = u8s(img.rgba(x, 20));
        assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() <= 3), "({x}, 20): {got:?}, Mixbox halfway {want:?}");
    }
}

#[test]
fn transparent_pixels_weigh_nothing_in_the_pickup() {
    // Half the tip over yellow, half over nothing, all pickup: the pickup is pure yellow, and
    // the empty half's share falls back to the (still full) blue load, so the first dab lays
    // Mixbox's halfway blue-yellow. Had the transparent pixels counted as black, it would be an
    // olive.
    let mut img = Surface::new(PixelFormat::RGBA8);
    img.fill_rect(Rect::new(0, 0, 20, 40), &rgba8(HANSA));
    let stroke = Stroke { brush: BrushSettings { size: 20.0, hardness: 1.0, pressure_size: false, ..Default::default() }, points: vec![StrokePoint::new(20.0, 20.0, 1.0)] };
    apply_mixbox_stroke(&mut img, &stroke, &settings(1.0, 0.0, 1.0), rgba8(COBALT), false, None, false);
    let want = mixbox::lerp(&COBALT, &HANSA, 0.5).map(i32::from);
    for x in [14, 18, 24] {
        let got = u8s(img.rgba(x, 20));
        assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() <= 3), "({x}, 20): {got:?}, want {want:?}");
    }
}

#[test]
fn a_stroke_across_blue_and_yellow_stays_on_the_mixbox_gradient() {
    // The paper's Fig. 5 colours. Loaded with the blue, every paint in play is a mix of the two,
    // so each pixel must lie on Mixbox's curve between them, away from RGB's straight blend.
    let mut img = image(160, 60, COBALT);
    img.fill_rect(Rect::new(80, 0, 160, 60), &rgba8(HANSA));
    apply_mixbox_stroke(&mut img, &line(30.0, 10.0, 150.0, 30.0, 0.7, 0.1), &settings(0.5, 0.5, 0.5), rgba8(COBALT), false, None, false);
    let (mut mixed, mut worst) = (0, 0);
    for y in 0..60 {
        for x in 0..160 {
            let c = u8s(img.rgba(x, y));
            let (d, t) = off_gradient(c, COBALT, HANSA);
            worst = worst.max(d);
            assert!(d <= 2, "({x}, {y}) {c:?} is {d} levels off the Mixbox gradient");
            if (0.2..=0.8).contains(&t) {
                mixed += 1;
                let r = off_rgb_line(c, COBALT, HANSA);
                assert!(r >= 20.0, "({x}, {y}) {c:?} is only {r} levels from the RGB blend");
            }
        }
    }
    assert!(mixed >= 200, "only {mixed} pixels mixed");
    println!("worst {worst} levels off the Mixbox gradient, {mixed} mixed pixels");
}

/// Cobalt Blue, solid load, across white and Hansa Yellow stripes: 80 px, hardness 70 %,
/// Wet 50 %, Load 80 %, Mix 50 %.
fn stripes(from_start: bool) -> Surface {
    let mut img = image(640, 160, WHITE);
    for i in 0..8 {
        img.fill_rect(Rect::new(i * 80 + 40, 0, i * 80 + 80, 160), &rgba8(HANSA));
    }
    apply_mixbox_stroke(&mut img, &line(80.0, 20.0, 620.0, 80.0, 0.7, 0.1), &settings(0.5, 0.8, 0.5), rgba8(COBALT), from_start, None, false);
    img
}

#[test]
fn blue_across_stripes_picking_up_the_live_canvas_stays_blue_without_grey() {
    // Picking up its own fresh paint, a full brush holds its load colour: a strong blue all the
    // way, a little greener over the yellow, never grey.
    let img = stripes(false);
    for x in 20..620 {
        let c = u8s(img.rgba(x, 80));
        let (h, sat) = hue_sat(c);
        assert!(sat >= 0.5 && (190.0..=230.0).contains(&h), "x = {x}: {c:?} hue {h:.0}° saturation {sat:.2}");
    }
}

#[test]
fn blue_across_yellow_stripes_goes_green_without_grey_seams() {
    // Picking up the canvas as the stroke began: Load 80 % blue at Mix 50 % mixes with each stripe.
    let img = stripes(true);
    let row: Vec<[i32; 3]> = (20..620).map(|x| u8s(img.rgba(x, 80))).collect();
    // No grey anywhere along the stroke, seams included.
    for (i, c) in row.iter().enumerate() {
        let (_, sat) = hue_sat(*c);
        assert!(sat >= 0.3, "x = {}: {c:?} is greyish (saturation {sat:.2})", i + 20);
    }
    // Blue and yellow make green: every pixel past the first stripe is green to teal (the tip
    // spans a white and a yellow stripe, so the mix swings with them), never the load's blue.
    let mut greenest = f32::MAX;
    for x in 100..600 {
        let (h, _) = hue_sat(u8s(img.rgba(x, 80)));
        assert!((90.0..=192.0).contains(&h), "x = {x}: hue {h:.0}° isn't green");
        greenest = greenest.min(h);
    }
    assert!(greenest < 145.0, "the greenest pixel is at {greenest:.0}°");
    // The live pickup stays blue on the same stripes: the two pickups differ.
    let live = stripes(false);
    assert!(hue_sat(u8s(live.rgba(300, 80))).0 > 200.0);
}

#[test]
fn full_load_lays_full_paint_and_never_thins_alpha() {
    // An empty layer: the load colour itself, fully opaque under the core (no pale paint).
    let mut img = Surface::new(PixelFormat::RGBA8);
    apply_mixbox_stroke(&mut img, &line(20.0, 10.0, 90.0, 20.0, 0.8, 0.1), &settings(0.5, 1.0, 0.5), rgba8(COBALT), false, None, false);
    let c = img.rgba(50, 20);
    assert!(u8s(c).iter().zip(COBALT.map(i32::from)).all(|(g, w)| (g - w).abs() <= 1) && c[3] > 0.99, "{c:?}");
    // An opaque canvas stays opaque wherever the brush goes, even dry.
    let mut img = image(100, 40, HANSA);
    apply_mixbox_stroke(&mut img, &line(20.0, 4.0, 96.0, 20.0, 0.5, 0.1), &settings(1.0, 0.0, 1.0), rgba8(COBALT), false, None, false);
    for x in 0..100 {
        for y in 0..40 {
            assert!(img.rgba(x, y)[3] >= 1.0 - 1e-6, "({x}, {y}) lost alpha");
        }
    }
    // Part-transparent paint only gains alpha.
    let mut img = Surface::new(PixelFormat::RGBA8);
    img.fill_rect(Rect::new(0, 0, 100, 40), &[0.9, 0.7, 0.1, 0.5]);
    apply_mixbox_stroke(&mut img, &line(20.0, 4.0, 96.0, 20.0, 0.5, 0.1), &settings(1.0, 0.0, 1.0), rgba8(COBALT), false, None, false);
    for x in 0..100 {
        assert!(img.rgba(x, 20)[3] >= 0.5 - 1.0 / 255.0, "({x}, 20) alpha {}", img.rgba(x, 20)[3]);
    }
}

#[test]
fn strokes_look_the_same_at_any_spacing() {
    let run = |spacing: f32| {
        let mut img = image(200, 60, HANSA);
        img.fill_rect(Rect::new(100, 0, 200, 60), &rgba8(WHITE));
        apply_mixbox_stroke(&mut img, &line(30.0, 10.0, 190.0, 30.0, 0.5, spacing), &settings(0.5, 0.6, 0.5), rgba8(COBALT), false, None, false);
        img
    };
    let (fine, coarse) = (run(0.03), run(0.15));
    let mut worst = 0;
    for x in (20..180).step_by(4) {
        for y in [22, 30, 38] {
            let (a, b) = (u8s(fine.rgba(x, y)), u8s(coarse.rgba(x, y)));
            worst = worst.max((0..3).map(|k| (a[k] - b[k]).abs()).max().unwrap_or(0));
        }
    }
    assert!(worst <= 12, "spacing 3 % vs 15 % differ by up to {worst} levels");
}

#[test]
fn load_runs_dry_by_distance_at_any_brush_size() {
    // Load 30 %: after the same number of brush widths on an empty layer, the same paint is left.
    let left_after = |size: f32, widths: f64| {
        let mut img = Surface::new(PixelFormat::RGBA8);
        let mut brush = MixboxBrush::new(&settings(0.5, 0.3, 0.5), rgba8(COBALT), img.clone());
        let stroke = crate::mixer::mixer_stroke(&line(100.0, 20.0, 20.0 + widths * f64::from(size), size, 0.8, 0.1), &settings(0.5, 0.3, 0.5));
        run_stroke(&mut img, &stroke, &mut brush, None, false);
        brush.load_left()
    };
    let (small, big) = (left_after(20.0, 6.0), left_after(60.0, 6.0));
    assert!(small < 0.9 && (small - big).abs() < 0.05, "left {small:.3} at 20 px, {big:.3} at 60 px");
    let want = 0.5f32.powf(6.0 / load_half_life(0.3));
    assert!((small - want).abs() < 0.08, "left {small:.3}, half-life says {want:.3}");
}

#[test]
fn a_dry_brush_smears_what_it_picks_up() {
    // Load 0 %: the load is gone within a width, then the brush drags the yellow into the blue.
    let mut img = image(160, 40, HANSA);
    img.fill_rect(Rect::new(80, 0, 160, 40), &rgba8(COBALT));
    apply_mixbox_stroke(&mut img, &line(20.0, 20.0, 150.0, 20.0, 0.6, 0.1), &settings(1.0, 0.0, 1.0), rgba8(WHITE), false, None, false);
    // Just past the edge the blue is mixed with dragged yellow: green, on the Mixbox gradient.
    let c = u8s(img.rgba(88, 20));
    let (d, t) = off_gradient(c, COBALT, HANSA);
    assert!(d <= 3 && t > 0.05, "{c:?}: {d} levels off the gradient at t = {t:.2}");
}

#[test]
fn a_feathered_selection_edge_mixes_as_pigment() {
    let mut img = image(60, 40, HANSA);
    let mut sel = Surface::new(PixelFormat::GRAY8);
    sel.fill_rect(Rect::new(0, 0, 60, 40), &[0.5]);
    apply_mixbox_stroke(&mut img, &line(20.0, 4.0, 56.0, 20.0, 0.8, 0.1), &settings(0.0, 1.0, 0.0), rgba8(COBALT), false, Some(&sel), false);
    let half = (0.5f32 * 255.0).round() / 255.0;
    let want = mixbox::lerp(&HANSA, &COBALT, half).map(i32::from);
    let got = u8s(img.rgba(30, 20));
    assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() <= 2), "{got:?}, Mixbox {want:?}");
}

#[test]
fn works_at_all_depths_and_colour_models() {
    for fmt in [PixelFormat::RGBA8, PixelFormat::RGBA16, PixelFormat::RGBA32F, PixelFormat::GRAY8, PixelFormat::GRAYA8, PixelFormat::CMYKA8] {
        let mut s = Surface::new(fmt);
        s.fill_rect(Rect::new(0, 0, 100, 40), &vec![1.0; fmt.channels()]);
        apply_mixbox_stroke(&mut s, &line(20.0, 4.0, 96.0, 20.0, 0.8, 0.1), &settings(0.5, 0.5, 0.5), rgba8(COBALT), false, None, false);
        assert!(s.pixel(50, 20).iter().all(|v| v.is_finite()), "{fmt:?}");
        assert_ne!(s.pixel(50, 20), vec![1.0; fmt.channels()], "{fmt:?} wasn't painted");
    }
}

/// Time a 241 px dab and report the latent canvas's peak size over a long stroke.
/// `cargo test -p photocraft-paint --release --lib mixbox_brush::tests::timing_and_memory -- --ignored --nocapture`
#[test]
#[ignore = "timing"]
fn timing_and_memory() {
    let size = 241.0;
    let (w, h) = (4000, 600);
    let mut img = image(w, h, WHITE);
    for i in 0..w / 40 {
        img.fill_rect(Rect::new(i * 40, 0, i * 40 + 20, h), &rgba8(HANSA));
    }
    let m = settings(0.5, 0.8, 0.5);
    let stroke = crate::mixer::mixer_stroke(&line(300.0, 150.0, f64::from(w) - 150.0, size, 0.7, 0.05), &m);
    let ctx = crate::BrushContext::new(&stroke.brush);
    let ds = crate::dabs(&stroke);
    let mut brush = MixboxBrush::new(&m, rgba8(COBALT), img.clone());
    let t = std::time::Instant::now();
    for (i, d) in ds.iter().enumerate() {
        brush.stroke_dab(&ctx, d, i);
    }
    let per_dab = t.elapsed().as_secs_f64() * 1e3 / ds.len() as f64;
    println!(
        "{} dabs of {size} px across {w} px: {per_dab:.2} ms per dab (tile conversion included) on {} threads; latent canvas peak {:.1} MB, freed with the stroke",
        ds.len(),
        rayon::current_num_threads(),
        brush.cache_bytes() as f64 / 1e6
    );
}
