//! Mixer Brush comparison sheet: the same strokes in RGB and Pigment mixing at several Wet/Load/Mix
//! settings, over stripes of paint. Writes a binary PPM (RGB mode in the left column, Pigment in
//! the right, one row per setting in [`ROWS`]).
//!
//! `cargo run -p photocraft-paint --release --example mixer_sheet -- sheet.ppm`

use photocraft_color::PixelFormat;
use photocraft_geom::Rect;
use photocraft_paint::mixer::{MixMode, MixerSettings, MixerState, Paint, apply_mixer_stroke};
use photocraft_paint::{BrushSettings, Stroke, StrokePoint};
use photocraft_raster::Surface;

/// (Wet, Load, Mix) in %.
const ROWS: [(f32, f32, f32); 7] = [(0.0, 50.0, 0.0), (20.0, 80.0, 50.0), (50.0, 80.0, 50.0), (80.0, 80.0, 50.0), (50.0, 80.0, 20.0), (50.0, 80.0, 80.0), (30.0, 80.0, 100.0)];
const W: i32 = 520;
const H: i32 = 84;
const GAP: i32 = 8;

fn rgb(r: u8, g: u8, b: u8) -> [f32; 4] {
    [f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0, 1.0]
}

fn cell(mode: MixMode, (wet, load, mix): (f32, f32, f32)) -> Surface {
    let mut s = Surface::new(PixelFormat::RGBA8);
    // Mixbox's Hansa Yellow, Cadmium Red and Permanent Green, with white between.
    let stripes = [rgb(252, 211, 0), rgb(255, 255, 255), rgb(255, 39, 2), rgb(252, 211, 0), rgb(7, 109, 22), rgb(255, 255, 255), rgb(252, 211, 0), rgb(128, 2, 46)];
    let sw = W / stripes.len() as i32 + 1;
    for (i, c) in stripes.iter().enumerate() {
        s.fill_rect(Rect::new(i as i32 * sw, 0, (i as i32 + 1) * sw, H), c);
    }
    let points = (0..=100).map(|i| {
        let t = f64::from(i) / 100.0;
        StrokePoint::new(24.0 + t * f64::from(W - 48), f64::from(H) / 2.0 + 14.0 * (t * std::f64::consts::TAU * 1.5).sin(), 1.0)
    });
    let stroke = Stroke { brush: BrushSettings { size: 40.0, hardness: 0.7, spacing: 0.1, pressure_size: false, ..Default::default() }, points: points.collect() };
    let m = MixerSettings { wet: wet / 100.0, load: load / 100.0, mix: mix / 100.0, flow: 1.0, sample_all_layers: false, mode };
    // Mixbox's Cobalt Blue.
    let mut state = MixerState::default();
    state.load(Paint::solid(rgb(0, 33, 133)));
    apply_mixer_stroke(&mut s, None, &stroke, &m, &mut state, None, false);
    s
}

fn main() -> std::io::Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "mixer_sheet.ppm".into());
    let (sw, sh) = (2 * W + 3 * GAP, ROWS.len() as i32 * (H + GAP) + GAP);
    let mut out = vec![40u8; (sw * sh * 3) as usize];
    for (r, row) in ROWS.iter().enumerate() {
        for (c, mode) in [MixMode::Rgb, MixMode::Pigment].into_iter().enumerate() {
            let s = cell(mode, *row);
            let (ox, oy) = (GAP + c as i32 * (W + GAP), GAP + r as i32 * (H + GAP));
            for y in 0..H {
                for x in 0..W {
                    let p = s.rgba(x, y);
                    let i = (((oy + y) * sw + ox + x) * 3) as usize;
                    for k in 0..3 {
                        out[i + k] = (p[k].clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                }
            }
        }
    }
    let mut file = format!("P6\n{sw} {sh}\n255\n").into_bytes();
    file.extend_from_slice(&out);
    std::fs::write(&path, file)?;
    println!("{path}: {sw}×{sh}, rows (Wet, Load, Mix) % = {ROWS:?}");
    Ok(())
}
