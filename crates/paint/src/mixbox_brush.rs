//! The Mixer Brush (Mixbox): a brush made only of what Mixbox publishes. Sochorová and Jamriška,
//! *Practical Pigment Mixing for Digital Painting* (ACM TOG 40(6), 2021), and the Mixbox code
//! (<https://github.com/scrtwpns/mixbox>, CC BY-NC 4.0, by Secret Weapons): every colour is
//! encoded to a Mixbox latent, mixed as a weighted average of latents and decoded (§3.2, Eq. 14).
//! The paper's own painting test used two brushes, a soft round brush and an area-averaging
//! smudge brush; this brush is the two together, and the Mixer Brush Tool's rules aren't used.
//!
//! Per dab of PhotoCraft's brush engine (tip shape, size, dynamics and smoothing as any brush):
//!
//! 1. **Pickup** (the area-averaging smudge, Eq. 14): the latents of the canvas under the tip,
//!    as painted so far, averaged with weights mask × alpha and divided by the sum of the weights.
//!    A transparent pixel weighs nothing. Each dab averages the paint the dabs before it put down,
//!    which is what drags colour along the stroke.
//! 2. **The brush's colour**: the load (the foreground colour) and the pickup mixed by Mix, the
//!    pickup's share scaled by Wet ([`pickup_share`]) and by how much paint lay under the tip. The
//!    share the pickup can't fill comes from the load. The load runs dry by the distance painted
//!    (Load sets the half-life in brush widths, [`load_half_life`]); with it dry the brush only
//!    smears.
//! 3. **Deposit** (Mixbox's `splash.html` example: `pixel = mixbox.lerp(pixel, colour, alpha ×
//!    edge)`): each pixel is mixed towards the brush's colour by
//!    `t = 1 − (1 − Flow × mask × amount)^(d / ref)`, `d` the distance from the previous dab and
//!    `ref` = [`REFERENCE_SPACING`] of the diameter, so a stroke looks the same at any spacing,
//!    brush size and speed. Alpha goes "over": paint never makes a pixel more transparent.
//!
//! The canvas is held as latents (7 floats and alpha per pixel, 64 px tiles converted as the
//! stroke reaches them, freed with the brush) and only decoded, and clamped, when written to the
//! layer. Other colour models go through RGB ([`to_rgba`]) to Mixbox and back.

use std::collections::HashMap;

use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_geom::Rect;
use photocraft_raster::{Surface, from_rgba, from_rgba_into, to_rgba};
use rayon::prelude::*;

use crate::Stroke;
use crate::mixer::MixerSettings;
use crate::retouch::{Footprint, Region, alpha_index};

/// A Mixbox latent ([`mixbox::LATENT_SIZE`] floats).
pub type Latent = [f32; mixbox::LATENT_SIZE];

/// Distances are measured in this fraction of the tip diameter (see the deposit, module docs).
pub const REFERENCE_SPACING: f32 = 0.1;
/// Tip mask values below this are skipped.
const MASK_EPS: f32 = 1e-4;
/// Edge of the latent canvas tiles.
const TILE: i32 = 64;
/// Dabs are filled in so they are at most this fraction of the diameter (and at least 1 px) apart:
/// the brush's colour follows the canvas dab by dab, and wider steps would make it lag by the
/// spacing.
pub const SUBSTEP: f32 = 0.05;

/// How much of the canvas the brush takes into its colour at Wet `wet` (0..1): none dry, rising to
/// all of it at Wet 50 %, so from there Mix alone sets the canvas's share.
pub fn pickup_share(wet: f32) -> f32 {
    (2.0 * wet.clamp(0.0, 1.0)).min(1.0)
}

/// Brush widths over which the load halves at Load `load` (0..1); Load 100 % never runs dry.
pub fn load_half_life(load: f32) -> f32 {
    let l = load.clamp(0.0, 1.0);
    if l >= 0.999 { f32::INFINITY } else { 0.25 + 10.0 * l / (1.0 - l) }
}

/// Native pixels to Mixbox latents and back for one pixel format.
struct Conv {
    fmt: PixelFormat,
    /// `fmt` without alpha (the colour channels alone).
    color: PixelFormat,
    nc: usize,
    alpha: Option<usize>,
    /// 32-bit RGB and grey are linear light: Mixbox's linear functions.
    linear: bool,
}

impl Conv {
    fn new(fmt: PixelFormat) -> Self {
        let rgb_like = matches!(fmt.mode, ColorMode::Rgb | ColorMode::Indexed | ColorMode::Grayscale | ColorMode::Bitmap | ColorMode::Duotone);
        Self {
            fmt,
            color: PixelFormat { alpha: false, ..fmt },
            nc: fmt.mode.color_channels().min(fmt.channels()),
            alpha: alpha_index(&fmt),
            linear: fmt.sample == SampleType::F32 && rgb_like,
        }
    }

    fn rgb(&self, px: &[f32]) -> [f32; 3] {
        let c = &px[..self.nc];
        match (self.fmt.mode, self.nc) {
            (ColorMode::Rgb | ColorMode::Indexed, _) => [c[0], c[1], c[2]],
            // Multichannel has no RGB meaning: its first channels stand in (one is a grey).
            (ColorMode::Multichannel, 1 | 2) => [c[0]; 3],
            (ColorMode::Multichannel, _) => [c[0], c[1], c[2]],
            _ => {
                let v = to_rgba(&self.color, c);
                [v[0], v[1], v[2]]
            }
        }
    }

    fn encode(&self, px: &[f32]) -> (Latent, f32) {
        let a = self.alpha.map_or(1.0, |i| px[i]).clamp(0.0, 1.0);
        if a <= 0.0 {
            return ([0.0; mixbox::LATENT_SIZE], 0.0);
        }
        (self.latent(self.rgb(px)), a)
    }

    fn latent(&self, rgb: [f32; 3]) -> Latent {
        if self.linear { mixbox::linear_float_rgb_to_latent(&rgb) } else { mixbox::float_rgb_to_latent(&rgb) }
    }

    /// Writes the colour (when there is paint) and alpha. The decoded RGB is clamped to 0..1.
    fn decode(&self, z: &Latent, a: f32, px: &mut [f32]) {
        if a > 0.0 {
            let rgb = if self.linear { mixbox::latent_to_linear_float_rgb(z) } else { mixbox::latent_to_float_rgb(z) };
            let c = &mut px[..self.nc];
            match (self.fmt.mode, c.len()) {
                (ColorMode::Rgb | ColorMode::Indexed, _) | (ColorMode::Multichannel, 3..) => c[..3].copy_from_slice(&rgb),
                (ColorMode::Multichannel, _) => c[0] = photocraft_color::convert::rgb_to_gray(rgb),
                _ => {
                    from_rgba_into(&self.color, [rgb[0], rgb[1], rgb[2], 1.0], c);
                }
            }
        }
        if let Some(i) = self.alpha {
            px[i] = a.clamp(0.0, 1.0);
        }
    }
}

/// `a` and `b` (latent, amount) mixed with weights `wa`, `wb` times their amounts: the colour is the
/// weighted average of the latents, the amount the weighted sum.
fn weigh(a: &Latent, aa: f32, wa: f32, b: &Latent, ab: f32, wb: f32) -> (Latent, f32) {
    let (wa, wb) = (wa * aa, wb * ab);
    let total = wa + wb;
    if total <= 1e-8 {
        return ([0.0; mixbox::LATENT_SIZE], total);
    }
    (std::array::from_fn(|i| (wa * a[i] + wb * b[i]) / total), total)
}

/// A tile of the canvas in latents.
struct Tile {
    z: Vec<Latent>,
    a: Vec<f32>,
    /// The tile as the stroke began, kept only when the pickup reads it
    /// ([`MixboxBrush::from_stroke_start`]).
    start: Option<(Vec<Latent>, Vec<f32>)>,
    /// Painted since the last flush.
    touched: Vec<bool>,
}

/// One stroke of the Mixer Brush (Mixbox): [`MixboxBrush::new`] at pointer down,
/// [`MixboxBrush::stroke_dab`] for each dab, [`MixboxBrush::flush`] to write what changed.
pub struct MixboxBrush {
    settings: MixerSettings,
    conv: Conv,
    /// The target as the stroke began (tiles are converted from it the first time the stroke
    /// reaches them; from then on the tile is the live canvas).
    pre: Surface,
    tiles: HashMap<(i32, i32), Tile>,
    /// The load colour and how much of it is left (1 = full).
    load: Latent,
    left: f32,
    /// Centre of the previous dab.
    last: Option<(f64, f64)>,
    /// Only the canvas is picked up from: a layer's pixels past its edge are out of sight.
    canvas: Option<Rect>,
    /// The pickup reads the canvas as the stroke began instead of as painted so far.
    from_start: bool,
    /// The brush engine's previous dab (what [`MixboxBrush::stroke_dab`] fills in from).
    last_dab: Option<crate::Dab>,
}

impl MixboxBrush {
    /// Start a stroke on a target holding `pre`, loaded with `color` (straight RGBA as the
    /// document's colours read through [`to_rgba`]; the foreground colour).
    pub fn new(settings: &MixerSettings, color: [f32; 4], pre: Surface) -> Self {
        let conv = Conv::new(pre.format());
        let native = from_rgba(&conv.fmt, [color[0], color[1], color[2], 1.0]);
        let load = conv.latent(conv.rgb(&native));
        Self { settings: settings.clone(), conv, pre, tiles: HashMap::new(), load, left: 1.0, last: None, canvas: None, from_start: false, last_dab: None }
    }

    /// Pick up from the canvas as it was when the stroke began, not as painted so far: the brush
    /// then never picks its own fresh paint back up, so a full load mixes with the canvas under it
    /// all along the stroke instead of drifting back to the load colour. Keeps a second copy of
    /// the tiles.
    pub fn from_stroke_start(mut self) -> Self {
        self.from_start = true;
        self
    }

    /// Pick up only inside `canvas` (the document's bounds).
    pub fn within(mut self, canvas: Rect) -> Self {
        self.canvas = Some(canvas);
        self
    }

    /// How much load is left (1 = full, 0 = dry).
    pub fn load_left(&self) -> f32 {
        self.left
    }

    /// Bytes the latent canvas holds now (it is freed with the brush, at the end of the stroke).
    pub fn cache_bytes(&self) -> usize {
        let px = std::mem::size_of::<Latent>() + std::mem::size_of::<f32>();
        self.tiles.len() * (TILE * TILE) as usize * (px * if self.from_start { 2 } else { 1 } + 1)
    }

    fn ensure_tiles(&mut self, rect: Rect) {
        let (t0x, t0y) = (rect.x0.div_euclid(TILE), rect.y0.div_euclid(TILE));
        let (t1x, t1y) = ((rect.x1 - 1).div_euclid(TILE), (rect.y1 - 1).div_euclid(TILE));
        let todo: Vec<(i32, i32)> = (t0y..=t1y).flat_map(|ty| (t0x..=t1x).map(move |tx| (tx, ty))).filter(|k| !self.tiles.contains_key(k)).collect();
        let (conv, pre, from_start) = (&self.conv, &self.pre, self.from_start);
        let n = conv.fmt.channels();
        let done: Vec<((i32, i32), Tile)> = todo
            .into_par_iter()
            .map(|(tx, ty)| {
                let r = Rect::new(tx * TILE, ty * TILE, (tx + 1) * TILE, (ty + 1) * TILE);
                let (z, a): (Vec<Latent>, Vec<f32>) = pre.read_region(r).chunks(n).map(|p| conv.encode(p)).unzip();
                let len = z.len();
                let start = from_start.then(|| (z.clone(), a.clone()));
                ((tx, ty), Tile { z, a, start, touched: vec![false; len] })
            })
            .collect();
        self.tiles.extend(done);
    }

    /// Paint the brush engine's dab `d` (number `index` in the stroke). Returns the rectangle
    /// it touched; [`MixboxBrush::flush`] writes it.
    pub fn stroke_dab(&mut self, ctx: &crate::BrushContext, d: &crate::Dab, index: usize) -> Rect {
        let prev = self.last_dab.replace(*d);
        let Some(p) = prev else { return self.dab(&ctx.footprint(d, index)) };
        let dist = (d.center.x - p.center.x).hypot(d.center.y - p.center.y) as f32;
        let step = (SUBSTEP * 2.0 * d.radius.min(p.radius)).max(1.0);
        let n = ((dist / step).ceil() as usize).clamp(1, 256);
        let mut dirty = Rect::EMPTY;
        for k in 1..=n {
            let t = k as f32 / n as f32;
            let lerp = |a: f32, b: f32| a + (b - a) * t;
            let sub = if k == n {
                *d
            } else {
                crate::Dab {
                    center: photocraft_geom::Point::new(p.center.x + (d.center.x - p.center.x) * f64::from(t), p.center.y + (d.center.y - p.center.y) * f64::from(t)),
                    radius: lerp(p.radius, d.radius),
                    alpha: lerp(p.alpha, d.alpha),
                    wet: lerp(p.wet, d.wet),
                    mix: lerp(p.mix, d.mix),
                    ..*d
                }
            };
            dirty = dirty.union(&self.dab(&ctx.footprint(&sub, index)));
        }
        dirty
    }

    /// Paint one dab (its footprint: tip coverage × flow).
    pub fn dab(&mut self, fp: &Footprint) -> Rect {
        let (cx, cy) = (fp.dab.center.x, fp.dab.center.y);
        let diameter = (2.0 * fp.dab.radius).max(1.0);
        // The first dab counts as one reference step.
        let dist = self.last.map_or(REFERENCE_SPACING * diameter, |(lx, ly)| (cx - lx).hypot(cy - ly) as f32);
        self.last = Some((cx, cy));
        let flow = fp.dab.alpha;
        let rect = fp.rect;
        if flow <= 0.0 || rect.is_empty() {
            return Rect::EMPTY;
        }
        self.ensure_tiles(rect);
        let inv_flow = 1.0 / flow;
        let w = rect.width() as usize;
        let seen = self.canvas.map_or(rect, |c| rect.intersect(&c));

        // 1. Pickup: Σ mask·alpha·z / Σ mask·alpha over the tip (Eq. 14), and how much of the tip
        //    held paint (Σ mask·alpha / Σ mask).
        let mask_at = |x: i32, y: i32| fp.cov[(y - rect.y0) as usize * w + (x - rect.x0) as usize] * inv_flow;
        let (tiles, from_start) = (&self.tiles, self.from_start);
        let (sz, swa, sw) = (seen.y0..seen.y1)
            .into_par_iter()
            .map(|y| {
                let (mut z, mut wa, mut wsum) = ([0.0f64; mixbox::LATENT_SIZE], 0.0f64, 0.0f64);
                let ty = y.div_euclid(TILE);
                for x in seen.x0..seen.x1 {
                    let m = mask_at(x, y);
                    if m < MASK_EPS {
                        continue;
                    }
                    wsum += f64::from(m);
                    let Some(t) = tiles.get(&(x.div_euclid(TILE), ty)) else { continue };
                    let li = ((y - ty * TILE) * TILE + (x - x.div_euclid(TILE) * TILE)) as usize;
                    let (tz, ta) = match (&t.start, from_start) {
                        (Some((z0, a0)), true) => (&z0[li], a0[li]),
                        _ => (&t.z[li], t.a[li]),
                    };
                    let k = f64::from(m * ta);
                    if k > 0.0 {
                        for (zi, ci) in z.iter_mut().zip(tz) {
                            *zi += k * f64::from(*ci);
                        }
                        wa += k;
                    }
                }
                (z, wa, wsum)
            })
            .reduce(
                || ([0.0f64; mixbox::LATENT_SIZE], 0.0, 0.0),
                |a, b| (std::array::from_fn(|i| a.0[i] + b.0[i]), a.1 + b.1, a.2 + b.2),
            );
        let (picked, held) = if swa > 1e-9 { (sz.map(|v| (v / swa) as f32), (swa / sw.max(1e-9)) as f32) } else { ([0.0; mixbox::LATENT_SIZE], 0.0) };

        // 2. The brush's colour: load and pickup by Mix, the share the pickup can't fill from the
        //    load. Then the load runs dry by the distance painted.
        let wet = (self.settings.wet * fp.dab.wet).clamp(0.0, 1.0);
        let mix = (self.settings.mix * fp.dab.mix).clamp(0.0, 1.0);
        let wp = mix * pickup_share(wet) * held;
        // Whatever the pickup doesn't fill (all but `wp`) is the load's, as much as is left. With
        // the load full the brush always lays full paint; dry, it smears the pickup alone.
        let wl = (1.0 - wp) * self.left;
        let (paint, amount) = weigh(&self.load, 1.0, wl, &picked, 1.0, wp);
        let amount = amount.min(1.0);
        let half_life = load_half_life(self.settings.load);
        if half_life.is_finite() {
            self.left *= 0.5f32.powf(dist / diameter / half_life);
        }
        if amount <= 0.0 {
            return Rect::EMPTY;
        }

        // 3. Deposit: each pixel towards the brush's colour by t = 1 − (1 − Flow·mask·amount)^(d/ref).
        let steps = dist / (REFERENCE_SPACING * diameter);
        let (t0x, t1x) = (rect.x0.div_euclid(TILE), (rect.x1 - 1).div_euclid(TILE));
        let (t0y, t1y) = (rect.y0.div_euclid(TILE), (rect.y1 - 1).div_euclid(TILE));
        let mut work: Vec<((i32, i32), Tile)> = (t0y..=t1y).flat_map(|ty| (t0x..=t1x).map(move |tx| (tx, ty))).filter_map(|k| self.tiles.remove_entry(&k)).collect();
        work.par_iter_mut().for_each(|((tx, ty), tile)| {
            let r = Rect::new(*tx * TILE, *ty * TILE, (*tx + 1) * TILE, (*ty + 1) * TILE).intersect(&rect);
            for y in r.y0..r.y1 {
                for x in r.x0..r.x1 {
                    let v = (fp.cov[(y - rect.y0) as usize * w + (x - rect.x0) as usize] * amount).min(1.0);
                    if v * inv_flow < MASK_EPS {
                        continue;
                    }
                    let e = if v >= 1.0 { 1.0 } else { 1.0 - ((1.0 - v).ln() * steps).exp() };
                    let li = ((y - *ty * TILE) * TILE + (x - *tx * TILE)) as usize;
                    let a = tile.a[li];
                    // "Over": the canvas keeps (1 − e) of its paint, the brush adds e.
                    let (z, _) = weigh(&tile.z[li], a, 1.0 - e, &paint, 1.0, e);
                    tile.z[li] = z;
                    tile.a[li] = (a + (1.0 - a) * e).min(1.0);
                    tile.touched[li] = true;
                }
            }
        });
        self.tiles.extend(work);
        rect
    }

    /// Write the pixels painted since the last flush inside `rect` to `work` (native pixels).
    pub fn flush(&mut self, work: &mut Region, rect: Rect) {
        let r = rect.intersect(&work.rect);
        if r.is_empty() {
            return;
        }
        for ty in r.y0.div_euclid(TILE)..=(r.y1 - 1).div_euclid(TILE) {
            for tx in r.x0.div_euclid(TILE)..=(r.x1 - 1).div_euclid(TILE) {
                let Some(tile) = self.tiles.get_mut(&(tx, ty)) else { continue };
                for y in r.y0.max(ty * TILE)..r.y1.min((ty + 1) * TILE) {
                    for x in r.x0.max(tx * TILE)..r.x1.min((tx + 1) * TILE) {
                        let li = ((y - ty * TILE) * TILE + (x - tx * TILE)) as usize;
                        if tile.touched[li] {
                            tile.touched[li] = false;
                            self.conv.decode(&tile.z[li], tile.a[li], work.px_mut(x, y));
                        }
                    }
                }
            }
        }
    }

    /// Lay the stroke (`work`) over the target as it began (`orig`) at `opacity` inside
    /// `selection`: a part-selected pixel is mixed in latents too (Mixbox lerp, weighted by alpha).
    #[allow(clippy::too_many_arguments)]
    pub fn mix_back(&self, target: &mut Surface, orig: &Region, work: &Region, rect: Rect, opacity: f32, selection: Option<&Surface>, lock_transparency: bool) {
        let conv = &self.conv;
        let mut out = orig.crop(rect);
        let n = out.ch;
        let opacity = opacity.clamp(0.0, 1.0);
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let k = (opacity * selection.map_or(1.0, |s| s.sample_channel(x, y, 0))).min(1.0);
                if k <= 0.0 {
                    continue;
                }
                let wv = work.px(x, y);
                let i = out.index(x, y) * n;
                let o = &mut out.data[i..i + n];
                let keep = conv.alpha.map(|a| o[a]);
                if k >= 1.0 {
                    o.copy_from_slice(wv);
                } else {
                    let ((zo, ao), (zw, aw)) = (conv.encode(o), conv.encode(wv));
                    if lock_transparency {
                        // Alpha stays: the colours alone mix.
                        let z = match (ao > 0.0, aw > 0.0) {
                            (true, true) => weigh(&zo, 1.0, 1.0 - k, &zw, 1.0, k).0,
                            (false, true) => zw,
                            _ => continue,
                        };
                        conv.decode(&z, 1.0, o);
                    } else {
                        let (z, a) = weigh(&zo, ao, 1.0 - k, &zw, aw, k);
                        conv.decode(&z, a, o);
                    }
                }
                if let (true, Some(a), Some(v)) = (lock_transparency, conv.alpha, keep) {
                    o[a] = v;
                }
            }
        }
        out.write(target);
    }
}

/// Run a whole stroke with `brush` on `target`, PhotoCraft's brush engine placing the dabs, mixed
/// back at the brush opacity inside the selection. Returns the damaged rectangle.
pub fn run_stroke(target: &mut Surface, stroke: &Stroke, brush: &mut MixboxBrush, selection: Option<&Surface>, lock_transparency: bool) -> Rect {
    let ds = crate::dabs(stroke);
    if ds.is_empty() {
        return Rect::EMPTY;
    }
    let ctx = crate::BrushContext::new(&stroke.brush);
    let bounds = ds.iter().fold(Rect::EMPTY, |r, d| r.union(&ctx.dab_rect(d, false)));
    let mut dirty = Rect::EMPTY;
    for (i, d) in ds.iter().enumerate() {
        dirty = dirty.union(&brush.stroke_dab(&ctx, d, i));
    }
    let orig = Region::read(target, bounds);
    let mut work = orig.clone();
    brush.flush(&mut work, dirty);
    brush.mix_back(target, &orig, &work, bounds, stroke.brush.opacity, selection, lock_transparency);
    bounds
}

/// Paint a Mixer Brush (Mixbox) stroke loaded with `color`, picking up from the canvas as painted
/// so far or, with `from_start`, as the stroke began. Returns the damaged rectangle.
#[allow(clippy::too_many_arguments)]
pub fn apply_mixbox_stroke(target: &mut Surface, stroke: &Stroke, m: &MixerSettings, color: [f32; 4], from_start: bool, selection: Option<&Surface>, lock_transparency: bool) -> Rect {
    let st = crate::mixer::mixer_stroke(stroke, m);
    let mut brush = MixboxBrush::new(m, color, target.clone());
    if from_start {
        brush = brush.from_stroke_start();
    }
    run_stroke(target, &st, &mut brush, selection, lock_transparency)
}

#[cfg(test)]
#[path = "mixbox_brush_tests.rs"]
mod tests;
