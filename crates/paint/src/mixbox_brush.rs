//! The Pigment Mixer Brush: a brush made only of what Mixbox publishes. Sochorová and Jamriška,
//! *Practical Pigment Mixing for Digital Painting* (ACM TOG 40(6), 2021), and the Mixbox code
//! (<https://github.com/scrtwpns/mixbox>, CC BY-NC 4.0, by Secret Weapons): every colour is
//! encoded to a Mixbox latent, mixed as a weighted average of latents and decoded (§3.2, Eq. 14).
//! The Mixer Brush Tool's rules aren't used.
//!
//! The brush carries a pickup image the size of the tip, as latents, on a grid that moves with
//! it. Per dab of PhotoCraft's brush engine (tip shape, size, dynamics and smoothing as any brush),
//! for each texel `p` under the tip and each pixel under it:
//!
//! 1. `pickup[p] = lerp(pickup[p], canvas[p], rate)`: the texel follows the canvas under it,
//!    `rate` = [`pickup_rate`]`(Wet, distance)`, so picked-up paint is dragged further the wetter
//!    the brush. The canvas is read as the stroke began (the default) or as painted so far
//!    ([`MixboxBrush::from_stroke_start`]); an empty texel fills at once.
//! 2. `brush[p] = lerp(load, pickup[p], Mix)`: the load is the foreground colour; the pickup's
//!    share is Mix × [`pickup_share`]`(Wet)` × how much paint the texel holds, the rest is the
//!    load's, as much as is left. The load runs dry by the distance painted (Load sets the
//!    half-life in brush widths, [`load_half_life`]); dry, the brush only smears.
//! 3. `canvas[p] = lerp(canvas[p], brush[p], t[p])`, as Mixbox's `splash.html` stamps paint, with
//!    `t = 1 − (1 − Flow × mask × amount)^(d / ref)`, `d` the distance from the previous dab and
//!    `ref` = [`REFERENCE_SPACING`] of the diameter, so a stroke looks the same at any spacing,
//!    brush size and speed. Alpha goes "over": paint never makes a pixel more transparent.
//!
//! Every blend is one of Eq. 14's weighted averages of latents (bilinear reads included, weights ×
//! amount over their sum, so transparent pixels weigh nothing). Each pixel mixes with its own
//! texel, so the colour varies across the tip: no dab-wide colour drawing the tip's outline as
//! arcs. Dabs are filled in at most [`SUBSTEP`] of the diameter apart. The canvas is held as
//! latents (64 px tiles converted as the stroke reaches them, freed with the brush) and only
//! decoded, and clamped, when written to the layer. Other colour models go through RGB
//! ([`to_rgba`]) to Mixbox and back.

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
/// Between two dabs the slide is followed at points this many pixels apart, at most
/// [`MAX_SWEEP`] of them.
const SWEEP_STEP: f32 = 1.0;
const MAX_SWEEP: usize = 8;

/// How much of the canvas the brush takes into its colour at Wet `wet` (0..1): none dry, rising to
/// all of it at Wet 50 %, so from there Mix alone sets the canvas's share.
pub fn pickup_share(wet: f32) -> f32 {
    (2.0 * wet.clamp(0.0, 1.0)).min(1.0)
}

/// Brush widths over which picked-up paint fades to 1/e at Wet `wet` (0..1): wetter paint is
/// dragged further.
pub fn drag(wet: f32) -> f32 {
    0.1 + 0.5 * wet.clamp(0.0, 1.0)
}

/// How far the pickup moves towards the canvas under it after `widths` brush widths of travel.
pub fn pickup_rate(wet: f32, widths: f32) -> f32 {
    1.0 - (-widths.max(0.0) / drag(wet)).exp()
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

/// Paint on the brush-local grid: a latent and an amount (0 = none, the latent then means nothing).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Texel {
    z: Latent,
    a: f32,
}

impl Texel {
    const NONE: Texel = Texel { z: [0.0; mixbox::LATENT_SIZE], a: 0.0 };

    /// `self` and `other` mixed, `t` = the share of `other` (weighted by the amounts).
    fn lerp(&self, other: &Texel, t: f32) -> Texel {
        let (z, a) = weigh(&self.z, self.a, 1.0 - t, &other.z, other.a, t);
        Texel { z, a }
    }
}

/// `buf` (`w × h` texels) at the continuous position (`fx`, `fy`), integer positions being texel
/// centres: the four neighbours' latents averaged with bilinear weights × amount over their sum
/// (Eq. 14); outside is no paint.
fn bilinear(buf: &[Texel], w: usize, h: usize, fx: f32, fy: f32) -> Texel {
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let (mut z, mut a) = ([0.0f32; mixbox::LATENT_SIZE], 0.0f32);
    for (dx, dy, wgt) in [(0, 0, (1.0 - tx) * (1.0 - ty)), (1, 0, tx * (1.0 - ty)), (0, 1, (1.0 - tx) * ty), (1, 1, tx * ty)] {
        let (x, y) = (x0 + dx, y0 + dy);
        if wgt <= 0.0 || x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            continue;
        }
        let c = &buf[y as usize * w + x as usize];
        let k = wgt * c.a;
        if k > 0.0 {
            for (zi, ci) in z.iter_mut().zip(&c.z) {
                *zi += k * ci;
            }
            a += k;
        }
    }
    if a <= 1e-8 {
        return Texel { z: [0.0; mixbox::LATENT_SIZE], a };
    }
    Texel { z: z.map(|v| v / a), a }
}

/// [`bilinear`] for plain values (the tip's coverage).
fn bilinear_f32(buf: &[f32], w: usize, h: usize, fx: f32, fy: f32) -> f32 {
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let at = |x: i64, y: i64| if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 { 0.0 } else { buf[y as usize * w + x as usize] };
    at(x0, y0) * (1.0 - tx) * (1.0 - ty) + at(x0 + 1, y0) * tx * (1.0 - ty) + at(x0, y0 + 1) * (1.0 - tx) * ty + at(x0 + 1, y0 + 1) * tx * ty
}

/// One stroke of the Pigment Mixer Brush: [`MixboxBrush::new`] at pointer down,
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
    /// The pickup image: `side × side` texels on a grid centred on the tip, one per pixel offset
    /// (`side = 2r + 1`), moving with the brush.
    side: usize,
    r: i32,
    pickup: Vec<Texel>,
    /// The brush's paint per texel under the current dab (kept to reuse the allocation).
    brush: Vec<Texel>,
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
    /// document's colours read through [`to_rgba`]; the foreground colour), for tips up to
    /// `max_brush_size` px. The pickup reads the live canvas unless
    /// [`MixboxBrush::from_stroke_start`].
    pub fn new(settings: &MixerSettings, color: [f32; 4], pre: Surface, max_brush_size: f32) -> Self {
        let conv = Conv::new(pre.format());
        let native = from_rgba(&conv.fmt, [color[0], color[1], color[2], 1.0]);
        let load = conv.latent(conv.rgb(&native));
        // Room for the tip's corners (sampled tips fill their square) and a texel each side.
        let side = 2 * ((max_brush_size.max(1.0) * std::f32::consts::SQRT_2 / 2.0).ceil() as usize + 2) + 1;
        Self {
            settings: settings.clone(),
            conv,
            pre,
            tiles: HashMap::new(),
            load,
            left: 1.0,
            side,
            r: (side / 2) as i32,
            pickup: vec![Texel::NONE; side * side],
            brush: vec![Texel::NONE; side * side],
            last: None,
            canvas: None,
            from_start: false,
            last_dab: None,
        }
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

    /// Bytes the latent canvas and the pickup image hold now (freed with the brush, at the end of
    /// the stroke).
    pub fn cache_bytes(&self) -> usize {
        let px = std::mem::size_of::<Latent>() + std::mem::size_of::<f32>();
        self.tiles.len() * (TILE * TILE) as usize * (px * if self.from_start { 2 } else { 1 } + 1) + 2 * self.side * self.side * std::mem::size_of::<Texel>()
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

    /// Paint the brush engine's dab `d` (number `index` in the stroke), filling in dabs from the
    /// previous one so they are at most [`SUBSTEP`] of the diameter apart. Returns the rectangle
    /// touched; [`MixboxBrush::flush`] writes it.
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

    /// The canvas the pickup reads at pixel (`x`, `y`): as the stroke began or as painted so far;
    /// no paint off the canvas or where no tile was converted.
    fn source(&self, x: i32, y: i32) -> Texel {
        if self.canvas.is_some_and(|c| !c.contains(x, y)) {
            return Texel::NONE;
        }
        let (tx, ty) = (x.div_euclid(TILE), y.div_euclid(TILE));
        let Some(t) = self.tiles.get(&(tx, ty)) else { return Texel::NONE };
        let li = ((y - ty * TILE) * TILE + (x - tx * TILE)) as usize;
        match (&t.start, self.from_start) {
            (Some((z, a)), true) => Texel { z: z[li], a: a[li] },
            _ => Texel { z: t.z[li], a: t.a[li] },
        }
    }

    /// Paint one dab (its footprint: tip coverage × flow). The pickup image's grid sits at the
    /// dab's sub-pixel centre; texels read the canvas at their exact position and pixels read the
    /// brush at theirs, both bilinear (Eq. 14 weights), so slanted strokes don't step.
    pub fn dab(&mut self, fp: &Footprint) -> Rect {
        let (cx, cy) = (fp.dab.center.x, fp.dab.center.y);
        let diameter = (2.0 * fp.dab.radius).max(1.0);
        // The first dab counts as one reference step.
        let dist = self.last.map_or(REFERENCE_SPACING * diameter, |(lx, ly)| (cx - lx).hypot(cy - ly) as f32);
        // How far the brush slid since the previous dab.
        let (mx, my) = self.last.map_or((0.0, 0.0), |(lx, ly)| ((cx - lx) as f32, (cy - ly) as f32));
        self.last = Some((cx, cy));
        let flow = fp.dab.alpha;
        let rect = fp.rect;
        if flow <= 0.0 || rect.is_empty() {
            return Rect::EMPTY;
        }
        // The slide is followed at points SWEEP_STEP px apart (at most MAX_SWEEP): without them
        // a pixel's colour would come from wherever the last dab happened to leave the tip, in
        // steps of the dab spacing, and show as stripes.
        let sweep = ((mx.hypot(my) / SWEEP_STEP).ceil() as usize).clamp(1, MAX_SWEEP);
        // Pixels the tip covered while it slid here, and the canvas around them.
        let back = Rect::new(rect.x0 - mx.ceil().max(0.0) as i32, rect.y0 - my.ceil().max(0.0) as i32, rect.x1 + (-mx).ceil().max(0.0) as i32, rect.y1 + (-my).ceil().max(0.0) as i32);
        let swept = rect.union(&back);
        let halo = swept.inflate(1);
        self.ensure_tiles(halo);
        let inv_flow = 1.0 / flow;
        let (fw, fh) = (rect.width() as usize, rect.height() as usize);
        let wet = (self.settings.wet * fp.dab.wet).clamp(0.0, 1.0);
        let mix = (self.settings.mix * fp.dab.mix).clamp(0.0, 1.0);
        let share = mix * pickup_share(wet);
        let rate = pickup_rate(wet, dist / diameter);
        let left = self.left;
        let load = Texel { z: self.load, a: 1.0 };

        // The canvas under the slide (plus a pixel for interpolation), dense, for the texels to read.
        let (hw, hh) = (halo.width() as usize, halo.height() as usize);
        let src: Vec<Texel> = (0..hw * hh).into_par_iter().map(|i| self.source(halo.x0 + (i % hw) as i32, halo.y0 + (i / hw) as i32)).collect();

        // Per texel under the dab: the pickup follows the canvas, and the brush's paint is the
        // load and the pickup by Mix.
        let (side, r) = (self.side, f64::from(self.r));
        // Texel `t`'s centre in pixel-index space (pixel x's centre is x + 0.5).
        let texel_at = |t: usize, c: f64| (c + t as f64 - r - 0.5) as f32;
        // Texels within a pixel of the dab: `first..last`.
        let span = |lo: i32, hi: i32, c: f64| {
            let first = ((f64::from(lo) - 1.0 - c + r + 0.5).floor().max(0.0) as usize).min(side);
            let last = ((f64::from(hi) + 1.0 - c + r + 0.5).floor().max(-1.0) + 1.0).min(side as f64) as usize;
            (first, last.max(first))
        };
        let (t0, t1) = span(rect.y0, rect.y1, cy);
        let (c0, c1) = span(rect.x0, rect.x1, cx);
        self.brush.fill(Texel::NONE);
        self.pickup[t0 * side..t1 * side].par_chunks_mut(side).zip(self.brush[t0 * side..t1 * side].par_chunks_mut(side)).enumerate().for_each(|(k, (prow, brow))| {
            let sy = texel_at(t0 + k, cy);
            for tx in c0..c1 {
                let sx = texel_at(tx, cx);
                let mask = bilinear_f32(&fp.cov, fw, fh, sx - rect.x0 as f32, sy - rect.y0 as f32) * inv_flow;
                if mask < MASK_EPS {
                    continue;
                }
                // The canvas the texel slid over since the previous dab: Eq. 14's average of the
                // points along its way (bilinear reads, weights × amount).
                let (mut z, mut a) = ([0.0f32; mixbox::LATENT_SIZE], 0.0f32);
                for j in 0..sweep {
                    let t = if sweep == 1 { 0.0 } else { (j as f32 + 0.5) / sweep as f32 };
                    let p = bilinear(&src, hw, hh, sx - t * mx - halo.x0 as f32, sy - t * my - halo.y0 as f32);
                    for (zi, ci) in z.iter_mut().zip(&p.z) {
                        *zi += p.a * ci;
                    }
                    a += p.a;
                }
                let under = if a <= 1e-8 { Texel::NONE } else { Texel { z: z.map(|v| v / a), a: a / sweep as f32 } };
                // pickup[p] = lerp(pickup[p], canvas[p], rate); an empty texel fills at once.
                prow[tx] = if prow[tx].a <= 0.0 { under } else { prow[tx].lerp(&under, rate) };
                // brush[p] = lerp(load, pickup[p], Mix): the share the pickup can't fill (it holds
                // little paint) comes from the load, as much of it as is left.
                let wp = share * prow[tx].a;
                let (z, a) = weigh(&load.z, 1.0, (1.0 - wp) * left, &prow[tx].z, 1.0, wp);
                brow[tx] = Texel { z, a: a.min(1.0) };
            }
        });
        let half_life = load_half_life(self.settings.load);
        if half_life.is_finite() {
            self.left *= 0.5f32.powf(dist / diameter / half_life);
        }

        // canvas[p] = lerp(canvas[p], brush[p], t[p]), t = 1 − (1 − Flow·mask·amount)^(d/ref),
        // alpha "over". The slide's points are laid one over the other, newest on top, each with
        // its share of the distance, and combined into one lerp.
        let steps = dist / (REFERENCE_SPACING * diameter);
        let part = steps / sweep as f32;
        let (t0x, t1x) = (swept.x0.div_euclid(TILE), (swept.x1 - 1).div_euclid(TILE));
        let (t0y, t1y) = (swept.y0.div_euclid(TILE), (swept.y1 - 1).div_euclid(TILE));
        let mut work: Vec<((i32, i32), Tile)> = (t0y..=t1y).flat_map(|ty| (t0x..=t1x).map(move |tx| (tx, ty))).filter_map(|k| self.tiles.remove_entry(&k)).collect();
        let brush = &self.brush;
        work.par_iter_mut().for_each(|((tx, ty), tile)| {
            let tr = Rect::new(*tx * TILE, *ty * TILE, (*tx + 1) * TILE, (*ty + 1) * TILE).intersect(&swept);
            for y in tr.y0..tr.y1 {
                let fy = (f64::from(y) + 0.5 - cy + r) as f32;
                for x in tr.x0..tr.x1 {
                    let fx = (f64::from(x) + 0.5 - cx + r) as f32;
                    let (mut z, mut keep) = ([0.0f32; mixbox::LATENT_SIZE], 1.0f32);
                    for j in 0..sweep {
                        // Newest first: the tip was (t·mx, t·my) behind where it is now.
                        let t = if sweep == 1 { 0.0 } else { (j as f32 + 0.5) / sweep as f32 };
                        let (ox, oy) = (t * mx, t * my);
                        let cov = bilinear_f32(&fp.cov, fw, fh, (x - rect.x0) as f32 + ox, (y - rect.y0) as f32 + oy);
                        if cov * inv_flow < MASK_EPS {
                            continue;
                        }
                        let paint = bilinear(brush, side, side, fx + ox, fy + oy);
                        let v = (cov * paint.a).min(1.0);
                        if v <= 0.0 {
                            continue;
                        }
                        let e = if v >= 1.0 { 1.0 } else { 1.0 - ((1.0 - v).ln() * part).exp() };
                        let w = e * keep;
                        for (zi, ci) in z.iter_mut().zip(&paint.z) {
                            *zi += w * ci;
                        }
                        keep *= 1.0 - e;
                        if keep <= 0.0 {
                            break;
                        }
                    }
                    let e = 1.0 - keep;
                    if e <= 0.0 {
                        continue;
                    }
                    let li = ((y - *ty * TILE) * TILE + (x - *tx * TILE)) as usize;
                    let a = tile.a[li];
                    // "Over": the canvas keeps (1 − e) of its paint, the brush adds e.
                    let (zc, _) = weigh(&tile.z[li], a, 1.0 - e, &z.map(|v| v / e), 1.0, e);
                    tile.z[li] = zc;
                    tile.a[li] = (a + (1.0 - a) * e).min(1.0);
                    tile.touched[li] = true;
                }
            }
        });
        self.tiles.extend(work);
        swept
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

/// Paint a Pigment Mixer Brush stroke loaded with `color`, picking up from the canvas as painted
/// so far or, with `from_start`, as the stroke began. Returns the damaged rectangle.
#[allow(clippy::too_many_arguments)]
pub fn apply_mixbox_stroke(target: &mut Surface, stroke: &Stroke, m: &MixerSettings, color: [f32; 4], from_start: bool, selection: Option<&Surface>, lock_transparency: bool) -> Rect {
    let st = crate::mixer::mixer_stroke(stroke, m);
    let mut brush = MixboxBrush::new(m, color, target.clone(), st.brush.size);
    if from_start {
        brush = brush.from_stroke_start();
    }
    run_stroke(target, &st, &mut brush, selection, lock_transparency)
}

#[cfg(test)]
#[path = "mixbox_brush_tests.rs"]
mod tests;
