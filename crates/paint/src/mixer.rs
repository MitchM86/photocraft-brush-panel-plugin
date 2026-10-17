//! Mixer Brush: paint loaded on the brush, mixed with paint it picks up from the canvas.
//!
//! The rules come from `mixer_core` (PhotoCraftMixer, written for PhotoCraft), adapted so
//! PhotoCraft's brush engine places the dabs: tip shapes, dynamics, Transfer and smoothing all
//! apply. The smear itself follows the idea of Krita's Color Smudge brush (picked-up canvas laid
//! down further along the stroke; no Krita code). Per dab, for every pixel under the tip (bands of
//! tile rows in parallel):
//!
//! 1. **Pickup**: the brush carries a bitmap of picked-up paint on a brush-local grid, one texel
//!    per pixel offset from the dab centre. An empty texel fills straight away from the canvas
//!    under it; a filled one drifts towards it, so paint picked up earlier is carried forward
//!    and fades over [`drag`]`(wet)` tip diameters of travel (a third of a width at Wet 50 %,
//!    so the brush has caught up with the canvas within a width): the smears and streaks. The
//!    canvas is
//!    read **as it was when the stroke began** (with Sample All Layers, over the other layers):
//!    picking up its own fresh paint would wash the canvas colour out of the mix.
//! 2. **Brush paint**: the reservoir texel and the picked-up paint (its amount × [`pickup_amount`]
//!    `(wet)`, full from Wet 50 %) mixed by Mix ([`brush_paint`]): at Mix 50 % half the paint is
//!    the canvas's. Whatever share the pickup can't fill falls back to the reservoir, so
//!    an empty pickup paints the plain load at any Mix, and a dry reservoir at Mix 100 % is a
//!    pure smudge.
//! 3. **Deposit** onto the canvas as painted so far ([`deposit`]): colour mixed by amount, alpha
//!    by "over", laying `1 − (1 − mask × flow × amount)^(d / ref)` of the paint, `d` the distance
//!    from the previous dab and `ref` = [`REFERENCE_SPACING`] of the diameter, so a stroke looks
//!    the same at any spacing and brush size.
//! 4. **Reservoir**: a bitmap on the pickup's grid (a solid load fills it evenly; ⌥-click loads
//!    a patch of the image, texture and all). Each texel under the tip runs dry with the
//!    distance it paints, in brush widths (half-life by Load, [`reservoir_half_life`]; Load 100 %
//!    never runs dry), and is dirtied by what it picks up ([`CONTAMINATION`]), so Load feels the
//!    same at any brush size.
//!
//! Every mix weights colour by the amount of paint, so a transparent pixel contributes nothing,
//! whatever colour it stores. The stroke paints into a per-stroke cache of the target in mixing
//! space (64 px tiles, each pixel converted once per stroke), written back by [`Mixer::flush`].
//!
//! All mixing goes through [`mix`] in one [`PaintSpace`]: [`MixMode::Rgb`] mixes the channels as
//! they are; [`MixMode::Pigment`] uses Mixbox by Secret Weapons (CC BY-NC 4.0,
//! <https://scrtwpns.com/mixbox>), so blue and yellow make green. Another mixer can be swapped in
//! by adding a space.
//!
//! In Pigment every blend of colours is a weighted average of Mixbox latents, as Sochorová and
//! Jamriška's paper (Practical Pigment Mixing for Digital Painting, 2021, §3.2) asks: encode each
//! participating colour, take a weighted average of the latents, decode the result. Weights are
//! normalised by the amount of paint, so a transparent pixel weighs nothing. That covers the
//! deposit, pickup, smear, the sweep along a move, contamination, bilinear reads of the canvas and
//! of the brush, resampling a loaded patch to the tip ([`Codec::grid`]), Load Solid Colors Only's
//! average ([`average_paint`]) and a feathered selection's edge ([`Mixer::mix_back`]). The canvas
//! copy, reservoir and pickup are float latents for the whole stroke and are kept as latents
//! between strokes ([`MixerState::exact`]); only what is written to the layer is decoded and
//! clamped.

use std::collections::HashMap;

use photocraft_color::{ColorMode, PixelFormat, SampleType};
use photocraft_geom::Rect;
use photocraft_raster::{Surface, from_rgba, to_rgba};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::Stroke;
use crate::retouch::{Footprint, Region, alpha_index, mix_back};

/// Floats in a [`Latent`].
pub const LATENT: usize = mixbox::LATENT_SIZE;
/// A colour in a form where mixing paints is a weighted sum (see [`PaintSpace`]).
pub type Latent = [f32; LATENT];

/// Distances along the stroke are measured in this fraction of the tip diameter: deposits,
/// pickup, running dry and contamination per this distance don't depend on the brush spacing.
pub const REFERENCE_SPACING: f32 = 0.1;
/// Tip diameters over which carried paint fades to 1/e at Wet `wet` (0..1): wetter paint carries
/// further, the longer streaks Photoshop's Wet describes; at Wet 50 % a third of a width, so the
/// brush holds the canvas under it again within a width.
pub fn drag(wet: f32) -> f32 {
    0.1 + 0.5 * wet.clamp(0.0, 1.0)
}

/// How much of the paint it carries the brush puts into its mix at Wet `wet` (0..1): none dry,
/// rising to all of it at Wet 50 %, so Mix alone sets the canvas's share from there on.
pub fn pickup_amount(wet: f32) -> f32 {
    (2.0 * wet.clamp(0.0, 1.0)).min(1.0)
}
/// mixer_core's dab spacing (a fraction of the diameter), which its Load half-life and
/// contamination were tuned per: they are kept per this distance travelled, so a Load setting
/// feels the same at any brush size and spacing.
pub const LOAD_REFERENCE: f32 = 0.05;
/// How far a reservoir texel moves towards the paint it picks up per [`LOAD_REFERENCE`] of the
/// diameter travelled, at Wet 100 %.
pub const CONTAMINATION: f32 = 0.001;
/// Tip mask values below this are skipped.
pub const MASK_EPS: f32 = 1e-4;
/// The mixer fills in dabs between the brush engine's so they are at most this fraction of the
/// diameter (and at least 1 px) apart: a soft edge sampled at wider steps bands, column by column.
pub const SUBSTEP: f32 = 0.05;
/// A pixel takes paint at points this fraction of the diameter (at least 1 px) apart along the
/// way the brush moved between two dabs, at most [`MAX_SWEEP`] of them.
const SWEEP_STEP: f32 = 0.01;
const MAX_SWEEP: usize = 8;
/// Edge of the per-stroke cache tiles.
const TILE: i32 = 64;

/// How the Mixer Brush mixes colours.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MixMode {
    /// Average the channels (what layers and other brushes do).
    #[default]
    Rgb,
    /// Mix like pigments (Mixbox): blue and yellow make green. Every blend, resample and average
    /// of the brush's paint is a weighted sum of Mixbox latents; other colour models go through
    /// RGB (Multichannel mixes as [`MixMode::Rgb`]).
    Pigment,
}

/// Where colours are mixed: native colour channels become latents, which mix as weighted sums
/// ([`mix`]), and come back as native colour channels.
pub trait PaintSpace: Sync {
    /// Native colour channels (straight, no alpha) to a latent.
    fn encode(&self, native: &[f32]) -> Latent;
    /// A latent back to native colour channels (`native` holds the colour channels).
    fn decode(&self, z: &Latent, native: &mut [f32]);
}

/// [`MixMode::Rgb`]: the native channels themselves (any colour model, up to [`LATENT`] channels).
struct Channels {
    nc: usize,
}

impl PaintSpace for Channels {
    fn encode(&self, native: &[f32]) -> Latent {
        let mut z = [0.0; LATENT];
        let n = self.nc.min(LATENT).min(native.len());
        z[..n].copy_from_slice(&native[..n]);
        z
    }
    fn decode(&self, z: &Latent, native: &mut [f32]) {
        let n = self.nc.min(LATENT).min(native.len());
        native[..n].copy_from_slice(&z[..n]);
    }
}

/// [`MixMode::Pigment`]: Mixbox latents. 8/16-bit RGB samples are sRGB-encoded, 32-bit ones
/// linear light (values above 1 are clamped).
struct Mixbox {
    linear: bool,
}

impl PaintSpace for Mixbox {
    fn encode(&self, native: &[f32]) -> Latent {
        let rgb = [native[0], native[1], native[2]];
        if self.linear { mixbox::linear_float_rgb_to_latent(&rgb) } else { mixbox::float_rgb_to_latent(&rgb) }
    }
    fn decode(&self, z: &Latent, native: &mut [f32]) {
        let rgb = if self.linear { mixbox::latent_to_linear_float_rgb(z) } else { mixbox::latent_to_float_rgb(z) };
        native[..3].copy_from_slice(&rgb);
    }
}

/// [`MixMode::Pigment`] in a Grayscale, Bitmap, Duotone, CMYK or Lab document: the colour to RGB
/// ([`to_rgba`]), Mixbox latents, and back ([`from_rgba`]), as the paper's RGB-in / RGB-out mixing
/// asks. 32-bit grey is linear light like 32-bit RGB.
struct MixboxVia {
    /// The document's format without alpha (the codec hands over colour channels only).
    fmt: PixelFormat,
    space: Mixbox,
}

impl PaintSpace for MixboxVia {
    fn encode(&self, native: &[f32]) -> Latent {
        let c = to_rgba(&self.fmt, native);
        self.space.encode(&c[..3])
    }
    fn decode(&self, z: &Latent, native: &mut [f32]) {
        let mut rgb = [0.0f32; 3];
        self.space.decode(z, &mut rgb);
        photocraft_raster::from_rgba_into(&self.fmt, [rgb[0], rgb[1], rgb[2], 1.0], native);
    }
}

/// The space `mode` mixes in for pixels of `fmt`. Pigment mixes every colour model through
/// Mixbox but Multichannel (no RGB meaning), which mixes its channels.
pub fn space(mode: MixMode, fmt: &PixelFormat) -> Box<dyn PaintSpace> {
    let f32 = fmt.sample == SampleType::F32;
    match (mode, fmt.mode) {
        (MixMode::Pigment, ColorMode::Rgb | ColorMode::Indexed) => Box::new(Mixbox { linear: f32 }),
        (MixMode::Pigment, ColorMode::Grayscale | ColorMode::Bitmap | ColorMode::Duotone) => Box::new(MixboxVia { fmt: PixelFormat { alpha: false, ..*fmt }, space: Mixbox { linear: f32 } }),
        // Their conversions to RGB give sRGB-encoded values at any depth.
        (MixMode::Pigment, ColorMode::Cmyk | ColorMode::Lab) => Box::new(MixboxVia { fmt: PixelFormat { alpha: false, ..*fmt }, space: Mixbox { linear: false } }),
        _ => Box::new(Channels { nc: fmt.mode.color_channels() }),
    }
}

/// Paint in a [`PaintSpace`]: a colour and an amount (0 = no paint, the colour then means nothing).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Colorant {
    pub z: Latent,
    pub a: f32,
}

impl Colorant {
    pub const NONE: Colorant = Colorant { z: [0.0; LATENT], a: 0.0 };

    fn full(self) -> Self {
        Colorant { a: 1.0, ..self }
    }
}

/// The one mix: `t` = the share of `b`. Colour is weighted by each paint's amount, so paint that
/// isn't there never tints the result; the amount is the plain weighted sum.
pub fn mix(a: &Colorant, b: &Colorant, t: f32) -> Colorant {
    let t = t.clamp(0.0, 1.0);
    let (wa, wb) = ((1.0 - t) * a.a, t * b.a);
    let total = wa + wb;
    if total <= 1e-8 {
        return Colorant { z: [0.0; LATENT], a: total };
    }
    Colorant { z: std::array::from_fn(|i| (wa * a.z[i] + wb * b.z[i]) / total), a: total }
}

/// The brush's paint from a reservoir texel and the picked-up paint, `mix_ratio` = the share of
/// picked-up paint. The share the pickup can't fill (it holds little or no paint) falls back to
/// the reservoir; a dry reservoir paints only the pickup's share.
pub fn brush_paint(reservoir: &Colorant, picked: &Colorant, mix_ratio: f32) -> Colorant {
    let m = mix_ratio.clamp(0.0, 1.0);
    let wp = m * picked.a;
    let wr = ((1.0 - m) + m * (1.0 - picked.a)) * reservoir.a;
    if wr + wp <= 1e-8 {
        return Colorant::NONE;
    }
    Colorant { a: (wr + wp).min(1.0), ..mix(&reservoir.full(), &picked.full(), wp / (wr + wp)) }
}

/// Lay `brush` paint onto `canvas` at `coverage` (0..1): colour mixed by amount, alpha "over",
/// so an opaque canvas stays opaque.
pub fn deposit(canvas: &Colorant, brush: &Colorant, coverage: f32) -> Colorant {
    let e = (coverage.clamp(0.0, 1.0) * brush.a).clamp(0.0, 1.0);
    let wc = canvas.a * (1.0 - e);
    let total = wc + e;
    if total <= 1e-8 {
        return Colorant { z: [0.0; LATENT], a: total };
    }
    Colorant { z: std::array::from_fn(|i| (wc * canvas.z[i] + e * brush.z[i]) / total), a: total.min(1.0) }
}

/// Brush widths (diameters) of travel over which a reservoir texel painting at full mask loses
/// half its paint at Load `load` (0..1): mixer_core's `1 + 400·Load²` dabs at its 5 % spacing.
/// Load 100 % never runs dry.
pub fn reservoir_half_life(load: f32) -> f32 {
    if load >= 0.999 { f32::INFINITY } else { LOAD_REFERENCE * (1.0 + 400.0 * load.clamp(0.0, 1.0).powi(2)) }
}

/// Mixer Brush options (all 0..1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MixerSettings {
    pub wet: f32,
    pub load: f32,
    pub mix: f32,
    pub flow: f32,
    /// Pick up colour from all visible layers instead of the target layer only.
    pub sample_all_layers: bool,
    /// How colours mix.
    pub mode: MixMode,
}

impl Default for MixerSettings {
    fn default() -> Self {
        Self { wet: 0.5, load: 0.5, mix: 0.5, flow: 1.0, sample_all_layers: false, mode: MixMode::Rgb }
    }
}

/// Paint on the brush: a square of `side × side` straight-RGBA pixels centred on the tip (a solid
/// colour is one pixel). Tip sizes differ between strokes, so it is read scaled to the tip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Paint {
    pub side: usize,
    pub px: Vec<[f32; 4]>,
}

impl Default for Paint {
    fn default() -> Self {
        Self::solid([0.0, 0.0, 0.0, 1.0])
    }
}

impl Paint {
    pub fn solid(c: [f32; 4]) -> Self {
        Self { side: 1, px: vec![c] }
    }

    /// A grid holding paint: `side × side` cells (a solid colour is one).
    fn is_grid(&self) -> bool {
        self.side > 1 && self.px.len() >= self.side * self.side
    }

    /// The amount of paint (alpha) at cell (`qx`, `qy`) of a `side × side` grid, read bilinearly so
    /// a patch loaded at one brush size runs dry smoothly on a brush of another. Colours are only
    /// resampled in mixing space ([`Codec::grid`]): averaging them here would mix them in RGB.
    fn amount_at(&self, qx: usize, qy: usize, side: usize) -> f32 {
        if !self.is_grid() {
            return self.px.first().map_or(0.0, |p| p[3]);
        }
        if side == self.side {
            return self.px[qy * self.side + qx][3];
        }
        let last = (self.side - 1) as f32;
        let map = |q: usize| grid_map(q, side, self.side).clamp(0.0, last);
        let a: Vec<f32> = self.px.iter().map(|p| p[3]).collect();
        bilinear_f32(&a, self.side, self.side, map(qx), map(qy))
    }

    /// The average colour (alpha-weighted, straight RGB), for the options bar's swatch. Paint to
    /// paint with is averaged by [`average_paint`] in mixing space.
    pub fn average(&self) -> [f32; 4] {
        let (mut acc, mut asum) = ([0.0f32; 3], 0.0f32);
        for p in &self.px {
            for c in 0..3 {
                acc[c] += p[c] * p[3];
            }
            asum += p[3];
        }
        if asum <= 1e-6 {
            return [0.0; 4];
        }
        [acc[0] / asum, acc[1] / asum, acc[2] / asum, asum / self.px.len().max(1) as f32]
    }
}

/// Where cell `q` of a `to`-cell row lies on a `from`-cell row (cell centres at integers).
fn grid_map(q: usize, to: usize, from: usize) -> f32 {
    (q as f32 + 0.5) * from as f32 / to as f32 - 0.5
}

/// `buf` (`w × h`, row-major) at the continuous position (`fx`, `fy`), integer positions being
/// cell centres, interpolated bilinearly with colour weighted by amount; outside is no paint.
fn bilinear(buf: &[Colorant], w: usize, h: usize, fx: f32, fy: f32) -> Colorant {
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let (mut z, mut a) = ([0.0f32; LATENT], 0.0f32);
    for (dx, dy, wgt) in [(0, 0, (1.0 - tx) * (1.0 - ty)), (1, 0, tx * (1.0 - ty)), (0, 1, (1.0 - tx) * ty), (1, 1, tx * ty)] {
        let (x, y) = (x0 + dx, y0 + dy);
        if wgt <= 0.0 || x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            continue;
        }
        let c = &buf[y as usize * w + x as usize];
        let wa = wgt * c.a;
        if wa > 0.0 {
            for (zi, ci) in z.iter_mut().zip(&c.z) {
                *zi += wa * ci;
            }
            a += wa;
        }
    }
    if a <= 1e-8 {
        return Colorant { z: [0.0; LATENT], a };
    }
    Colorant { z: z.map(|v| v / a), a }
}

/// [`bilinear`] for plain values (a tip's coverage).
fn bilinear_f32(buf: &[f32], w: usize, h: usize, fx: f32, fy: f32) -> f32 {
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let at = |x: i64, y: i64| if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 { 0.0 } else { buf[y as usize * w + x as usize] };
    at(x0, y0) * (1.0 - tx) * (1.0 - ty) + at(x0 + 1, y0) * tx * (1.0 - ty) + at(x0, y0 + 1) * (1.0 - tx) * ty + at(x0 + 1, y0 + 1) * tx * ty
}

/// Paint carried between strokes (session tool state).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MixerState {
    /// The reservoir as it is now (dirtied by what it picked up; alpha = paint left), None =
    /// clean (nothing loaded).
    pub reservoir: Option<Paint>,
    /// The reservoir as loaded.
    pub loaded: Option<Paint>,
    /// Paint picked up from the canvas (alpha = how much), None = none.
    pub pickup: Option<Paint>,
    /// The foreground colour when the brush was last loaded: a different foreground colour loads
    /// the brush with it.
    pub loaded_with: Option<[f32; 4]>,
    /// The reservoir and pickup exactly as the last stroke left them, in its mixing space:
    /// `reservoir` and `pickup` are them decoded (and clamped) to straight RGBA, which a stroke in
    /// the same space and on the same grid doesn't go back through.
    #[serde(skip)]
    pub exact: Option<ExactPaint>,
}

/// [`MixerState::exact`]: the brush's paint as latents.
#[derive(Clone, Debug, PartialEq)]
pub struct ExactPaint {
    /// The mixing mode and the pixel format whose space the latents are in.
    pub space: (MixMode, PixelFormat),
    pub side: usize,
    pub reservoir: Vec<Colorant>,
    pub pickup: Vec<Colorant>,
}

impl MixerState {
    /// Load the reservoir (full) with `paint`. The picked-up paint is dropped.
    pub fn load(&mut self, paint: Paint) {
        self.reservoir = Some(paint.clone());
        self.loaded = Some(paint);
        self.pickup = None;
        self.exact = None;
    }
    /// Fill the reservoir up again, keeping its colours.
    pub fn refill(&mut self) {
        if let (Some(r), Some(l)) = (self.reservoir.as_mut(), self.loaded.as_ref()) {
            let side = r.side;
            for (q, p) in r.px.iter_mut().enumerate() {
                p[3] = l.amount_at(q % side.max(1), q / side.max(1), side);
            }
            if let Some(e) = self.exact.as_mut().filter(|e| e.side == side && e.reservoir.len() == r.px.len()) {
                for (c, p) in e.reservoir.iter_mut().zip(&r.px) {
                    c.a = p[3];
                }
            }
        }
    }
    /// Wash the brush after a stroke: no picked-up paint, and the reservoir back to the colours it
    /// was loaded with (as much of them as is left). The loaded colours are taken as they are, on
    /// their own grid: resampling them would mix them outside the mixing space.
    pub fn wash(&mut self) {
        self.pickup = None;
        self.exact = None;
        if let (Some(r), Some(l)) = (self.reservoir.as_mut(), self.loaded.as_ref()) {
            if !l.is_grid() || l.side == r.side {
                for (q, p) in r.px.iter_mut().enumerate() {
                    let c = if l.is_grid() { l.px[q] } else { l.px.first().copied().unwrap_or([0.0; 4]) };
                    *p = [c[0], c[1], c[2], p[3].min(c[3])];
                }
            } else {
                let px = l.px.iter().enumerate().map(|(q, c)| [c[0], c[1], c[2], r.amount_at(q % l.side, q / l.side, l.side).min(c[3])]).collect();
                *r = Paint { side: l.side, px };
            }
        }
    }
    /// Clean the brush: no paint loaded or picked up.
    pub fn clean(&mut self) {
        self.reservoir = None;
        self.loaded = None;
        self.pickup = None;
        self.exact = None;
    }
    /// The colour the options bar shows: the reservoir's average, None when clean.
    pub fn swatch(&self) -> Option<[f32; 4]> {
        let r = self.reservoir.as_ref()?;
        let avg = r.average();
        if avg[3] > 1e-6 { Some([avg[0], avg[1], avg[2], 1.0]) } else { self.loaded.as_ref().map(Paint::average) }
    }
}

/// Side of the brush-local square the brush's paint covers for a tip of `size` px (sampled tips
/// reach out to the corners of their square). ⌥-click loads a square this size.
pub fn paint_side(size: f32) -> usize {
    2 * ((size.max(1.0) * std::f32::consts::SQRT_2 / 2.0).ceil() as usize + 2) + 1
}

/// A tile's stretch of a pixel row a dab deposits into: the tile's column and its cells.
type Stretch<'a> = (i32, &'a mut [Colorant], &'a mut [bool]);

/// A band of tile rows a dab deposits into: its pixel rows and its tiles.
type Band = (i32, i32, Vec<((i32, i32), CacheTile)>);

/// A tile of the per-stroke cache, in mixing space.
struct CacheTile {
    /// The target as the stroke began.
    own: Vec<Colorant>,
    /// What the brush picks up with Sample All Layers: `own` over the other layers.
    seen: Option<Vec<Colorant>>,
    /// The target as painted so far.
    work: Vec<Colorant>,
    /// Painted since the last flush.
    touched: Vec<bool>,
}

/// Converts between native pixels and [`Colorant`]s.
struct Codec {
    fmt: PixelFormat,
    space: Box<dyn PaintSpace>,
    nc: usize,
    alpha: Option<usize>,
}

impl Codec {
    fn new(mode: MixMode, fmt: PixelFormat) -> Self {
        Self { space: space(mode, &fmt), nc: fmt.mode.color_channels().min(fmt.channels()), alpha: alpha_index(&fmt), fmt }
    }
    fn encode_native(&self, px: &[f32]) -> Colorant {
        let a = self.alpha.map_or(1.0, |i| px[i]);
        if a <= 0.0 {
            return Colorant::NONE;
        }
        Colorant { z: self.space.encode(&px[..self.nc]), a: a.min(1.0) }
    }
    fn decode_native(&self, c: &Colorant, px: &mut [f32]) {
        if c.a > 0.0 {
            self.space.decode(&c.z, &mut px[..self.nc]);
        }
        if let Some(i) = self.alpha {
            px[i] = c.a.clamp(0.0, 1.0);
        }
    }
    fn encode_rgba(&self, c: [f32; 4]) -> Colorant {
        if c[3] <= 0.0 {
            return Colorant::NONE;
        }
        let native = from_rgba(&self.fmt, [c[0], c[1], c[2], 1.0]);
        Colorant { z: self.space.encode(&native[..self.nc]), a: c[3].min(1.0) }
    }
    fn decode_rgba(&self, c: &Colorant) -> [f32; 4] {
        if c.a <= 0.0 {
            return [0.0; 4];
        }
        let mut native = vec![0.0f32; self.fmt.channels()];
        self.decode_native(&Colorant { a: 1.0, ..*c }, &mut native);
        let mut out = to_rgba(&self.fmt, &native);
        out[3] = c.a.min(1.0);
        out
    }

    /// `p` on a `side × side` grid in mixing space: its pixels encoded, then resampled bilinearly
    /// as latents (weighted by amount), so a patch loaded at one brush size mixes on a brush of
    /// another exactly as its colours would on the canvas.
    fn grid(&self, p: &Paint, side: usize) -> Vec<Colorant> {
        if !p.is_grid() {
            return vec![self.encode_rgba(p.px.first().copied().unwrap_or([0.0; 4])); side * side];
        }
        let src: Vec<Colorant> = p.px.iter().map(|&c| self.encode_rgba(c)).collect();
        if side == p.side {
            return src;
        }
        let last = (p.side - 1) as f32;
        let map = |q: usize| grid_map(q, side, p.side).clamp(0.0, last);
        (0..side * side).map(|q| bilinear(&src, p.side, p.side, map(q % side), map(q / side))).collect()
    }
}

/// The average of `p` (weighted by amount; alpha = the mean amount) mixed as `mode` mixes paint in
/// a `fmt` document: what Load Solid Colors Only loads.
pub fn average_paint(p: &Paint, mode: MixMode, fmt: PixelFormat) -> [f32; 4] {
    let codec = Codec::new(mode, fmt);
    let (mut z, mut a) = ([0.0f32; LATENT], 0.0f32);
    for c in p.px.iter().map(|&c| codec.encode_rgba(c)) {
        for (zi, ci) in z.iter_mut().zip(&c.z) {
            *zi += c.a * ci;
        }
        a += c.a;
    }
    if a <= 1e-6 {
        return [0.0; 4];
    }
    let mut out = codec.decode_rgba(&Colorant { z: z.map(|v| v / a), a: 1.0 });
    out[3] = a / p.px.len().max(1) as f32;
    out
}

/// One Mixer Brush stroke: [`Mixer::new`] at pointer down, [`Mixer::dab`] for each dab
/// PhotoCraft's brush engine places, [`Mixer::flush`] to write what changed, [`Mixer::state`]
/// for the brush's paint afterwards.
pub struct Mixer {
    settings: MixerSettings,
    codec: Codec,
    /// The target as the stroke began.
    pre: Surface,
    /// Half-size of the brush-local grid (`side = 2r + 1`).
    r: i32,
    side: usize,
    reservoir: Vec<Colorant>,
    pickup: Vec<Colorant>,
    tiles: HashMap<(i32, i32), CacheTile>,
    /// Centre of the previous dab.
    last: Option<(f64, f64)>,
    /// The brush engine's previous dab (what [`Mixer::stroke_dab`] fills in from).
    last_dab: Option<crate::Dab>,
    /// The canvas: what lies outside it isn't picked up (a layer can reach past it, out of sight).
    canvas: Option<Rect>,
    /// The brush's paint under the current dab, per texel (kept to reuse the allocation), and the
    /// texel rows and columns the last dab wrote.
    brush: Vec<Colorant>,
    brush_span: (usize, usize, usize, usize),
    state: MixerState,
}

impl Mixer {
    /// Start a stroke on a target that holds `pre` as the stroke begins, with `state`'s paint, for
    /// tips up to `max_brush_size` px.
    pub fn new(settings: &MixerSettings, state: &MixerState, pre: Surface, max_brush_size: f32) -> Self {
        let codec = Codec::new(settings.mode, pre.format());
        let side = paint_side(max_brush_size);
        // The last stroke's latents when this one mixes in the same space on the same grid.
        let exact = state.exact.as_ref().filter(|e| e.space == (settings.mode, pre.format()) && e.side == side && e.reservoir.len() == side * side && e.pickup.len() == side * side);
        let empty = || vec![Colorant::NONE; side * side];
        let reservoir = match (&state.reservoir, exact) {
            (None, _) => empty(),
            (Some(_), Some(e)) => e.reservoir.clone(),
            (Some(p), None) => codec.grid(p, side),
        };
        let pickup = match (&state.pickup, exact) {
            (None, _) => empty(),
            (Some(_), Some(e)) => e.pickup.clone(),
            (Some(p), None) => codec.grid(p, side),
        };
        Self { settings: settings.clone(), codec, pre, r: (side / 2) as i32, side, reservoir, pickup, tiles: HashMap::new(), last: None, last_dab: None, brush: Vec::new(), brush_span: (0, 0, 0, 0), canvas: None, state: state.clone() }
    }

    /// Pick up only inside `canvas` (the document's bounds): a layer's pixels past its edge are out
    /// of sight, so the brush mustn't drag their colours in.
    pub fn within(mut self, canvas: Rect) -> Self {
        self.canvas = Some(canvas);
        self
    }

    /// The brush's paint after the dabs so far.
    pub fn state(&self) -> MixerState {
        let mut st = self.state.clone();
        let paint = |cells: &[Colorant]| Paint { side: self.side, px: cells.iter().map(|c| self.codec.decode_rgba(c)).collect() };
        if st.reservoir.is_some() {
            st.reservoir = Some(paint(&self.reservoir));
        }
        st.pickup = self.pickup.iter().any(|c| c.a > 1e-4).then(|| paint(&self.pickup));
        st.exact = Some(ExactPaint { space: (self.settings.mode, self.codec.fmt), side: self.side, reservoir: self.reservoir.clone(), pickup: self.pickup.clone() });
        st
    }

    /// Lay the stroke (`work`) over the target as it began (`orig`) at `opacity` inside
    /// `selection`, as [`mix_back`] does, mixing a part-selected pixel in this stroke's mixing
    /// space: a feathered selection edge blends the paint into the canvas as pigment, not RGB.
    #[allow(clippy::too_many_arguments)]
    pub fn mix_back(&self, target: &mut Surface, orig: &Region, work: &Region, rect: Rect, opacity: f32, selection: Option<&Surface>, lock_transparency: bool) {
        if self.settings.mode == MixMode::Rgb {
            return mix_back(target, orig, work, rect, opacity, selection, lock_transparency);
        }
        let codec = &self.codec;
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
                let keep = codec.alpha.map(|a| o[a]);
                if k >= 1.0 {
                    o.copy_from_slice(wv);
                } else {
                    let (was, now) = (codec.encode_native(o), codec.encode_native(wv));
                    let m = if lock_transparency {
                        // Alpha stays: mix the colours alone (a transparent side has none to give).
                        match (was.a > 0.0, now.a > 0.0) {
                            (true, true) => Colorant { a: was.a, ..mix(&was.full(), &now.full(), k) },
                            (false, true) => Colorant { a: 1.0, ..now },
                            _ => continue,
                        }
                    } else {
                        mix(&was, &now, k)
                    };
                    codec.decode_native(&m, o);
                }
                if let (true, Some(a), Some(v)) = (lock_transparency, codec.alpha, keep) {
                    o[a] = v;
                }
            }
        }
        out.write(target);
    }

    /// Convert the cache tiles over `rect` not converted yet (in parallel).
    fn ensure_tiles(&mut self, rect: Rect, sample: Option<&Surface>) {
        let (t0x, t0y) = (rect.x0.div_euclid(TILE), rect.y0.div_euclid(TILE));
        let (t1x, t1y) = ((rect.x1 - 1).div_euclid(TILE), (rect.y1 - 1).div_euclid(TILE));
        let todo: Vec<(i32, i32)> = (t0y..=t1y).flat_map(|ty| (t0x..=t1x).map(move |tx| (tx, ty))).filter(|k| !self.tiles.contains_key(k)).collect();
        let (codec, pre) = (&self.codec, &self.pre);
        let n = codec.fmt.channels();
        let done: Vec<((i32, i32), CacheTile)> = todo
            .into_par_iter()
            .map(|(tx, ty)| {
                let r = Rect::new(tx * TILE, ty * TILE, (tx + 1) * TILE, (ty + 1) * TILE);
                let own: Vec<Colorant> = pre.read_region(r).chunks(n).map(|p| codec.encode_native(p)).collect();
                let seen = sample.map(|sm| {
                    own.iter()
                        .enumerate()
                        .map(|(i, o)| {
                            let under = codec.encode_rgba(sm.rgba(r.x0 + i as i32 % TILE, r.y0 + i as i32 / TILE));
                            deposit(&under, o, 1.0)
                        })
                        .collect()
                });
                let len = own.len();
                ((tx, ty), CacheTile { work: own.clone(), own, seen, touched: vec![false; len] })
            })
            .collect();
        self.tiles.extend(done);
    }

    /// Paint the brush engine's next dab `d` (number `index` in the stroke), filling in dabs from
    /// the previous one so they are at most [`SUBSTEP`] of the diameter apart. Returns the
    /// rectangle touched.
    pub fn stroke_dab(&mut self, ctx: &crate::BrushContext, d: &crate::Dab, index: usize, sample: Option<&Surface>) -> Rect {
        let prev = self.last_dab.replace(*d);
        let Some(p) = prev else { return self.dab(&ctx.footprint(d, index), sample) };
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
            dirty = dirty.union(&self.dab(&ctx.footprint(&sub, index), sample));
        }
        dirty
    }

    /// Paint one dab (its footprint: the tip shape with size, rotation and dynamics applied, and
    /// the dab's flow) into the stroke cache. `sample` is the composite of the other layers (Sample
    /// All Layers), covering the dab. Returns the rectangle it touched; [`Mixer::flush`] writes it.
    ///
    /// The brush's grid sits at the dab's sub-pixel centre: texels pick up the canvas at their exact
    /// position and pixels read the brush's paint at theirs, both interpolated (bilinear, weighted
    /// by amount). Rounding either to whole pixels would draw a stroke at a slant as stair-steps.
    pub fn dab(&mut self, fp: &Footprint, sample: Option<&Surface>) -> Rect {
        let (cx, cy) = (fp.dab.center.x, fp.dab.center.y);
        let diameter = (2.0 * fp.dab.radius).max(1.0);
        let reference = REFERENCE_SPACING * diameter;
        // The first dab counts as one reference step.
        let dist = self.last.map_or(reference, |(lx, ly)| (cx - lx).hypot(cy - ly) as f32);
        // How far the brush moved since the previous dab.
        let (mx, my) = self.last.map_or((0.0, 0.0), |(lx, ly)| ((cx - lx) as f32, (cy - ly) as f32));
        self.last = Some((cx, cy));
        let flow = fp.dab.alpha;
        let rect = fp.rect;
        if flow <= 0.0 || rect.is_empty() {
            return Rect::EMPTY;
        }
        let steps = dist / reference;
        // Pixels the tip swept since the previous dab (it moved by (mx, my)).
        let back = Rect::new(rect.x0 - mx.ceil().max(0.0) as i32, rect.y0 - my.ceil().max(0.0) as i32, rect.x1 + (-mx).ceil().max(0.0) as i32, rect.y1 + (-my).ceil().max(0.0) as i32);
        let swept = rect.union(&back);
        let halo = swept.inflate(1);
        self.ensure_tiles(halo, sample);
        let wet = (self.settings.wet * fp.dab.wet).clamp(0.0, 1.0);
        let mix_ratio = (self.settings.mix * fp.dab.mix).clamp(0.0, 1.0);
        let pickup_rate = 1.0 - (-dist / (drag(wet) * diameter)).exp();
        let amount = pickup_amount(wet);
        // Running dry and contamination go by brush widths travelled.
        let widths = dist / diameter;
        let half_life = reservoir_half_life(self.settings.load);
        let dry = if half_life.is_finite() { widths / half_life } else { 0.0 };
        let dirt_steps = widths / LOAD_REFERENCE;
        let inv_flow = 1.0 / flow;
        let (side, r) = (self.side, self.r as f64);
        // Texel (tx, ty)'s centre in pixel-index space (pixel x's centre is x + 0.5).
        let texel_at = |t: usize, c: f64| (c + t as f64 - r - 0.5) as f32;

        // What the brush picks up, densely over the dab (plus a pixel for interpolation).
        let (hw, hh) = (halo.width() as usize, halo.height() as usize);
        let tiles = &self.tiles;
        let seen = self.canvas.map_or(halo, |c| halo.intersect(&c));
        let mut src = vec![Colorant::NONE; hw * hh];
        src.par_chunks_mut(hw).enumerate().for_each(|(j, row)| {
            let y = halo.y0 + j as i32;
            if y < seen.y0 || y >= seen.y1 {
                return;
            }
            let ty = y.div_euclid(TILE);
            // A tile's stretch of the row at a time.
            let mut x = seen.x0;
            while x < seen.x1 {
                let tx = x.div_euclid(TILE);
                let end = seen.x1.min((tx + 1) * TILE);
                if let Some(t) = tiles.get(&(tx, ty)) {
                    let li = ((y - ty * TILE) * TILE + (x - tx * TILE)) as usize;
                    let cells = t.seen.as_ref().unwrap_or(&t.own);
                    row[(x - halo.x0) as usize..(end - halo.x0) as usize].copy_from_slice(&cells[li..li + (end - x) as usize]);
                }
                x = end;
            }
        });
        let (fw, fh) = (rect.width() as usize, rect.height() as usize);

        // 1–2 and 4, per texel: pick up, the brush's paint, the reservoir running dry. Only the
        // texel rows over the dab.
        // Texels within a pixel of the dab: rows t0..t1, columns c0..c1.
        let span = |lo: i32, hi: i32, c: f64| {
            let first = ((f64::from(lo) - 1.0 - c + r + 0.5).floor().max(0.0) as usize).min(side);
            let last = ((f64::from(hi) + 1.0 - c + r + 0.5).floor().max(-1.0) + 1.0).min(side as f64) as usize;
            (first, last.max(first))
        };
        let (t0, t1) = span(rect.y0, rect.y1, cy);
        let (c0, c1) = span(rect.x0, rect.x1, cx);
        // Texels outside this dab's span must read as no paint: clear what the last dab wrote.
        let mut brush = std::mem::take(&mut self.brush);
        if brush.len() != side * side {
            brush = vec![Colorant::NONE; side * side];
        } else {
            let (p0, p1, q0, q1) = self.brush_span;
            for row in brush[p0 * side..p1 * side].chunks_mut(side) {
                row[q0..q1].fill(Colorant::NONE);
            }
        }
        self.brush_span = (t0, t1, c0, c1);
        let (pickup, reservoir) = (&mut self.pickup, &mut self.reservoir);
        pickup[t0 * side..t1 * side]
            .par_chunks_mut(side)
            .zip(reservoir[t0 * side..t1 * side].par_chunks_mut(side))
            .zip(brush[t0 * side..t1 * side].par_chunks_mut(side))
            .enumerate()
            .for_each(|(k, ((prow, rrow), brow))| {
                let sy = texel_at(t0 + k, cy);
                for tx in c0..c1 {
                    let sx = texel_at(tx, cx);
                    let mask = bilinear_f32(&fp.cov, fw, fh, sx - rect.x0 as f32, sy - rect.y0 as f32) * inv_flow;
                    if mask < MASK_EPS {
                        brow[tx] = Colorant::NONE;
                        continue;
                    }
                    // 1. Pickup: an empty texel fills at once, a filled one drifts.
                    if wet > 0.0 {
                        let at = bilinear(&src, hw, hh, sx - halo.x0 as f32, sy - halo.y0 as f32);
                        // As fast under the soft rim as under the core: a rim drifting slower would
                        // hold the colours left behind and, painting last, tint the stroke with them.
                        let rate = if prow[tx].a <= 0.0 { 1.0 } else { pickup_rate };
                        prow[tx] = mix(&prow[tx], &at, rate);
                    }
                    let picked = Colorant { a: prow[tx].a * amount, ..prow[tx] };
                    // 2. The brush's paint.
                    let mut held = rrow[tx];
                    brow[tx] = brush_paint(&held, &picked, mix_ratio);
                    // 4. The reservoir texel runs dry and is dirtied by what it picked up.
                    if held.a > 0.0 {
                        if dry > 0.0 {
                            held.a *= 0.5f32.powf(mask * dry);
                        }
                        if picked.a > 0.0 {
                            let c = 1.0 - (1.0 - (CONTAMINATION * wet * mask).min(1.0)).powf(dirt_steps);
                            held = Colorant { a: held.a, ..mix(&held.full(), &picked.full(), c) };
                        }
                        rrow[tx] = held;
                    }
                }
            });

        // 3. Deposit, per pixel, by bands of tile rows in parallel.
        let tiles = &mut self.tiles;
        let (t0x, t1x) = (swept.x0.div_euclid(TILE), (swept.x1 - 1).div_euclid(TILE));
        let mut bands: Vec<Band> = (swept.y0.div_euclid(TILE)..=(swept.y1 - 1).div_euclid(TILE))
            .map(|ty| (swept.y0.max(ty * TILE), swept.y1.min((ty + 1) * TILE), (t0x..=t1x).filter_map(|tx| tiles.remove_entry(&(tx, ty))).collect()))
            .collect();
        let texels = &brush;
        // The brush slid from the previous dab to this one: each pixel takes paint from the tip at
        // `sweep` points along the way, oldest first, each with the tip's coverage and the brush's
        // paint there and its share of the distance. Taking only this dab's, a patterned load
        // would print in a pattern repeating every dab.
        let sweep = ((mx.hypot(my) / (SWEEP_STEP * diameter).max(1.0)).ceil() as usize).clamp(1, MAX_SWEEP);
        let share = steps / sweep as f32;
        let (rw, rh) = (rect.width() as usize, rect.height() as usize);
        // Every pixel row in parallel: each gets its stretch of every tile it crosses.
        let mut rows: Vec<(i32, Vec<Stretch>)> = Vec::new();
        for (y0, y1, band_tiles) in bands.iter_mut() {
            let start = rows.len();
            rows.extend((*y0..*y1).map(|y| (y, Vec::new())));
            for ((tx, ty), tile) in band_tiles.iter_mut() {
                let skip = (*y0 - *ty * TILE) as usize;
                let cells = tile.work.chunks_mut(TILE as usize).skip(skip).zip(tile.touched.chunks_mut(TILE as usize).skip(skip));
                for (row, (work, touched)) in rows[start..].iter_mut().zip(cells) {
                    row.1.push((*tx, work, touched));
                }
            }
        }
        rows.par_iter_mut().for_each(|(y, stretches)| {
            let y = *y;
            for (tx, work, touched) in stretches.iter_mut() {
                let tx = *tx;
                {
                    let fy = (f64::from(y) + 0.5 - cy + r) as f32;
                    for x in swept.x0.max(tx * TILE)..swept.x1.min((tx + 1) * TILE) {
                        let fx = (f64::from(x) + 0.5 - cx + r) as f32;
                        // The points' paint laid one over the other, newest on top, combined
                        // into one deposit: `laid` of colour `z`.
                        let (mut z, mut keep) = ([0.0f32; LATENT], 1.0f32);
                        for k in 0..sweep {
                            // Newest first: the tip was (t·mx, t·my) behind where it is now.
                            let t = if sweep == 1 { 0.0 } else { (k as f32 + 0.5) / sweep as f32 };
                            let (ox, oy) = (t * mx, t * my);
                            let mask = bilinear_f32(&fp.cov, rw, rh, (x - rect.x0) as f32 + ox, (y - rect.y0) as f32 + oy) * inv_flow;
                            if mask < MASK_EPS {
                                continue;
                            }
                            let paint = bilinear(texels, side, side, fx + ox, fy + oy);
                            if paint.a <= 0.0 {
                                continue;
                            }
                            // The same over a distance at any spacing: the paint's amount is in the
                            // exponent too, or thin paint would build up dab by dab where the mask is 1.
                            let v = (mask * flow * paint.a).min(1.0);
                            let e = if v >= 1.0 { 1.0 } else { 1.0 - ((1.0 - v).ln() * share).exp() };
                            let w = e * keep;
                            for (zi, ci) in z.iter_mut().zip(&paint.z) {
                                *zi += w * ci;
                            }
                            keep *= 1.0 - e;
                        }
                        let laid = 1.0 - keep;
                        if laid <= 0.0 {
                            continue;
                        }
                        let li = (x - tx * TILE) as usize;
                        work[li] = deposit(&work[li], &Colorant { z: z.map(|v| v / laid), a: 1.0 }, laid);
                        touched[li] = true;
                    }
                }
            }
        });
        drop(rows);
        for (_, _, band_tiles) in bands {
            tiles.extend(band_tiles);
        }
        self.brush = brush;
        swept
    }

    /// Write the pixels painted since the last flush inside `rect` from the stroke cache to
    /// `work` (native pixels).
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
                            self.codec.decode_native(&tile.work[li], work.px_mut(x, y));
                        }
                    }
                }
            }
        }
    }
}

/// Run a whole Mixer Brush stroke with `mixer` on `target`, PhotoCraft's brush engine placing the
/// dabs. `sample` gives the composite of the other layers over the stroke's bounds (Sample All
/// Layers), or None. The result is mixed back at the brush opacity inside the selection. Returns
/// the damaged rectangle.
pub fn run_stroke<'a>(
    target: &mut Surface,
    stroke: &Stroke,
    mixer: &mut Mixer,
    sample: impl FnOnce(Rect) -> Option<&'a Surface>,
    selection: Option<&Surface>,
    lock_transparency: bool,
) -> Rect {
    let ds = crate::dabs(stroke);
    if ds.is_empty() {
        return Rect::EMPTY;
    }
    let ctx = crate::BrushContext::new(&stroke.brush);
    let bounds = ds.iter().fold(Rect::EMPTY, |r, d| r.union(&ctx.dab_rect(d, false)));
    let sample = sample(bounds);
    let mut dirty = Rect::EMPTY;
    for (i, d) in ds.iter().enumerate() {
        dirty = dirty.union(&mixer.stroke_dab(&ctx, d, i, sample));
    }
    let orig = Region::read(target, bounds);
    let mut work = orig.clone();
    mixer.flush(&mut work, dirty);
    mixer.mix_back(target, &orig, &work, bounds, stroke.brush.opacity, selection, lock_transparency);
    bounds
}

/// Paint a Mixer Brush stroke. `sample` is the composite of all visible layers but the target
/// (Sample All Layers), None to pick up from the target only. `state` is the brush's paint, updated
/// by the stroke. With `lock_transparency`, alpha is preserved. Returns the damaged rectangle.
pub fn apply_mixer_stroke(
    target: &mut Surface,
    sample: Option<&Surface>,
    stroke: &Stroke,
    m: &MixerSettings,
    state: &mut MixerState,
    selection: Option<&Surface>,
    lock_transparency: bool,
) -> Rect {
    let st = mixer_stroke(stroke, m);
    let mut mixer = Mixer::new(m, state, target.clone(), stroke.brush.size);
    let dmg = run_stroke(target, &st, &mut mixer, |_| sample, selection, lock_transparency);
    *state = mixer.state();
    dmg
}

/// The stroke the Mixer Brush runs: its own Flow per dab, no stroke opacity (it has none).
pub fn mixer_stroke(stroke: &Stroke, m: &MixerSettings) -> Stroke {
    let mut st = stroke.clone();
    st.brush.opacity = 1.0;
    st.brush.flow = m.flow.clamp(0.0, 1.0);
    st
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BrushSettings, StrokePoint};

    // The colours of mixer_core's tests.
    const BLUE: [f32; 4] = [0.0, 33.0 / 255.0, 133.0 / 255.0, 1.0];
    const YELLOW: [f32; 4] = [252.0 / 255.0, 211.0 / 255.0, 0.0, 1.0];
    const RED: [f32; 4] = [0.85, 0.1, 0.1, 1.0];
    const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    const BROWN: [f32; 4] = [0.71, 0.35, 0.12, 1.0];

    fn close(a: [f32; 4], b: [f32; 4], tol: f32) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() <= tol)
    }

    fn settings(mode: MixMode, wet: f32, load: f32, mix: f32) -> MixerSettings {
        MixerSettings { wet, load, mix, flow: 1.0, sample_all_layers: false, mode }
    }

    fn loaded(c: [f32; 4]) -> MixerState {
        let mut st = MixerState::default();
        st.load(Paint::solid(c));
        st
    }

    fn image(w: i32, h: i32, c: [f32; 4]) -> Surface {
        let mut s = Surface::new(PixelFormat::RGBA8);
        s.fill_rect(Rect::new(0, 0, w, h), &c);
        s
    }

    /// A 20 px tip at hardness 0.8, input points 1.7 px apart (a real pointer's are irregular).
    fn line(y: f64, x0: f64, x1: f64, spacing: f32) -> Stroke {
        let n = ((x1 - x0) / 1.7).floor() as usize;
        Stroke {
            brush: BrushSettings { size: 20.0, hardness: 0.8, spacing, pressure_size: false, ..Default::default() },
            points: (0..=n).map(|i| StrokePoint::new(x0 + i as f64 * 1.7, y, 1.0)).collect(),
        }
    }

    fn paint(img: &mut Surface, sample: Option<&Surface>, st: &mut MixerState, m: &MixerSettings, stroke: &Stroke) {
        apply_mixer_stroke(img, sample, stroke, m, st, None, false);
    }

    // --- The mixing maths ---------------------------------------------------------------------

    #[test]
    fn transparent_paint_never_tints() {
        for mode in [MixMode::Rgb, MixMode::Pigment] {
            let codec = Codec::new(mode, PixelFormat::RGBA8);
            // A transparent pixel that happens to store bright magenta.
            let ghost = codec.encode_native(&[1.0, 0.0, 1.0, 0.0]);
            let m = mix(&codec.encode_rgba(BROWN), &ghost, 0.5);
            assert!(close([codec.decode_rgba(&m)[0], codec.decode_rgba(&m)[1], codec.decode_rgba(&m)[2], 1.0], BROWN, 2.0 / 255.0), "{mode:?}");
            assert!((m.a - 0.5).abs() < 1e-6);
            // A part-transparent canvas pixel weighs in by its alpha only.
            let d = deposit(&Colorant { a: 0.0, ..codec.encode_rgba([1.0, 0.0, 1.0, 1.0]) }, &codec.encode_rgba(BROWN), 1.0);
            assert!(close(codec.decode_rgba(&d), BROWN, 2.0 / 255.0), "{mode:?}");
        }
    }

    #[test]
    fn pigment_blue_and_yellow_make_green_and_rgb_does_not() {
        let mid = |mode: MixMode| {
            let codec = Codec::new(mode, PixelFormat::RGBA8);
            codec.decode_rgba(&mix(&codec.encode_rgba(BLUE), &codec.encode_rgba(YELLOW), 0.5)).map(|v| (v * 255.0).round() as i32)
        };
        let g = mid(MixMode::Pigment);
        assert!(g.iter().zip([41, 130, 57]).all(|(a, b)| (a - b).abs() <= 3), "{g:?}, want about (41, 130, 57)");
        assert_eq!(mid(MixMode::Rgb)[..3], [126, 122, 67]);
    }

    /// Mixbox's pigment table (from the crate's header).
    const PIGMENTS: [[u8; 3]; 13] = [
        [254, 236, 0],
        [252, 211, 0],
        [255, 105, 0],
        [255, 39, 2],
        [128, 2, 46],
        [78, 0, 66],
        [25, 0, 89],
        [0, 33, 133],
        [13, 27, 68],
        [0, 60, 50],
        [7, 109, 22],
        [107, 148, 4],
        [123, 72, 0],
    ];

    fn rgba8(c: [u8; 3]) -> [f32; 4] {
        [f32::from(c[0]) / 255.0, f32::from(c[1]) / 255.0, f32::from(c[2]) / 255.0, 1.0]
    }

    #[test]
    fn pigment_mix_is_mixbox_lerp_for_every_pigment_pair() {
        // PhotoCraft's mix (8-bit pixels through the Pigment space) against the crate's own lerp.
        let codec = Codec::new(MixMode::Pigment, PixelFormat::RGBA8);
        let mut worst = 0;
        for (i, a) in PIGMENTS.iter().enumerate() {
            for b in &PIGMENTS[i + 1..] {
                for t in [0.25, 0.5, 0.75] {
                    let got = u8s3(codec.decode_rgba(&mix(&codec.encode_rgba(rgba8(*a)), &codec.encode_rgba(rgba8(*b)), t)));
                    let want = mixbox::lerp(a, b, t).map(i32::from);
                    let d = got.iter().zip(want).map(|(g, w)| (g - w).abs()).max().unwrap_or(0);
                    worst = worst.max(d);
                    assert!(d <= 1, "{a:?} + {b:?} at {t}: {got:?}, Mixbox {want:?}");
                }
            }
        }
        assert!(worst <= 1);
        // 32-bit (linear-light) documents through the linear functions, against lerp_linear_float.
        let codec = Codec::new(MixMode::Pigment, PixelFormat::RGBA32F);
        let lin = |c: [u8; 3]| c.map(|v| photocraft_color::convert::srgb_to_linear(f32::from(v) / 255.0));
        for (a, b) in [(PIGMENTS[7], PIGMENTS[1]), (PIGMENTS[3], PIGMENTS[10])] {
            let (la, lb) = (lin(a), lin(b));
            let got = codec.decode_rgba(&mix(&codec.encode_rgba([la[0], la[1], la[2], 1.0]), &codec.encode_rgba([lb[0], lb[1], lb[2], 1.0]), 0.5));
            let want = mixbox::lerp_linear_float(&la, &lb, 0.5);
            assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-4), "{got:?} vs {want:?}");
        }
    }

    fn u8s3(c: [f32; 4]) -> [i32; 3] {
        [0, 1, 2].map(|i| (c[i] * 255.0).round() as i32)
    }

    #[test]
    fn a_colour_mixed_with_itself_is_itself() {
        // 1,000 random colours, each mixed with itself, in both modes: within ±1.
        for mode in [MixMode::Rgb, MixMode::Pigment] {
            let codec = Codec::new(mode, PixelFormat::RGBA8);
            for i in 0..1000 {
                let c = [0, 1, 2].map(|k| (crate::rng::hash2(i, k, 77) * 255.0).round() / 255.0);
                let z = codec.encode_rgba([c[0], c[1], c[2], 1.0]);
                let got = u8s3(codec.decode_rgba(&mix(&z, &z, 0.37)));
                let want = c.map(|v| (v * 255.0).round() as i32);
                assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() <= 1), "{mode:?}: {want:?} → {got:?}");
            }
        }
    }

    #[test]
    fn a_clean_brush_from_yellow_into_blue_leaves_green() {
        // No load, Mix 100 %: the brush carries Hansa Yellow into Cobalt Blue. In Pigment mode the
        // meeting is green; averaging RGB makes it grey.
        let (yellow, blue) = (rgba8([252, 211, 0]), rgba8([0, 33, 133]));
        let run = |mode: MixMode| {
            let mut img = image(400, 80, blue);
            img.fill_rect(Rect::new(0, 0, 200, 80), &yellow);
            let mut st = MixerState::default();
            let mut k = line(40.0, 40.0, 380.0, 0.1);
            k.brush.size = 40.0;
            paint(&mut img, None, &mut st, &settings(mode, 0.6, 1.0, 1.0), &k);
            img.rgba(212, 40)
        };
        let (pig, rgb) = (run(MixMode::Pigment), run(MixMode::Rgb));
        assert!(pig[1] > pig[0] + 0.1 && pig[1] > pig[2] + 0.05, "Pigment: {pig:?} isn't green");
        assert!(rgb[1] <= rgb[0].max(rgb[2]) + 0.05, "RGB: {rgb:?}");
    }

    #[test]
    fn a_half_and_half_stroke_leaves_an_opaque_image_opaque() {
        // Wet/Load/Mix 50 over an opaque image, through the reservoir running low: alpha stays 255
        // everywhere, in both modes.
        for mode in [MixMode::Rgb, MixMode::Pigment] {
            let mut img = image(400, 80, YELLOW);
            img.fill_rect(Rect::new(200, 0, 400, 80), &RED);
            let mut st = loaded(BLUE);
            let mut k = line(40.0, 20.0, 380.0, 0.1);
            k.brush.size = 40.0;
            paint(&mut img, None, &mut st, &settings(mode, 0.5, 0.5, 0.5), &k);
            for y in 0..80 {
                for x in 0..400 {
                    assert_eq!((img.rgba(x, y)[3] * 255.0).round(), 255.0, "{mode:?} ({x},{y})");
                }
            }
        }
    }

    #[test]
    fn mixing_at_zero_and_one_returns_the_inputs() {
        for mode in [MixMode::Rgb, MixMode::Pigment] {
            let codec = Codec::new(mode, PixelFormat::RGBA8);
            let (a, b) = (codec.encode_rgba(BLUE), codec.encode_rgba(YELLOW));
            assert!(close(codec.decode_rgba(&mix(&a, &b, 0.0)), BLUE, 1e-4), "{mode:?}");
            assert!(close(codec.decode_rgba(&mix(&a, &b, 1.0)), YELLOW, 1e-4), "{mode:?}");
        }
        // 32-bit (linear) documents round-trip too.
        let codec = Codec::new(MixMode::Pigment, PixelFormat::RGBA32F);
        let mut out = [0.0f32; 4];
        codec.decode_native(&codec.encode_native(&[0.2, 0.5, 0.05, 1.0]), &mut out);
        assert!(close(out, [0.2, 0.5, 0.05, 1.0], 1e-4), "{out:?}");
    }

    // --- The brush (mixer_core's tests, on PhotoCraft's dabs) ---------------------------------

    #[test]
    fn empty_layer_gets_clean_reservoir_colour() {
        // The striped pink strokes: painting on an empty layer whose transparent pixels hold
        // garbage colour must give the plain brush colour.
        for mode in [MixMode::Rgb, MixMode::Pigment] {
            let mut layer = image(120, 40, [1.0, 0.0, 1.0, 0.0]);
            let mut st = loaded(BROWN);
            paint(&mut layer, None, &mut st, &settings(mode, 0.5, 1.0, 0.5), &line(20.0, 10.0, 110.0, 0.1));
            for x in 20..100 {
                let c = layer.rgba(x, 20);
                assert!(c[3] > 0.99 && close(c, BROWN, 3.0 / 255.0), "{mode:?} x={x} {c:?}");
            }
        }
    }

    #[test]
    fn dry_brush_paints_reservoir_colour() {
        let mut img = image(120, 40, YELLOW);
        let mut st = loaded(BLUE);
        paint(&mut img, None, &mut st, &settings(MixMode::Pigment, 0.0, 1.0, 1.0), &line(20.0, 10.0, 110.0, 0.1));
        assert!(close(img.rgba(60, 20), BLUE, 3.0 / 255.0), "{:?}", img.rgba(60, 20));
    }

    #[test]
    fn opaque_canvas_stays_opaque() {
        let mut img = image(120, 40, RED);
        let mut st = loaded(BLUE);
        paint(&mut img, None, &mut st, &settings(MixMode::Pigment, 1.0, 0.0, 1.0), &line(20.0, 10.0, 110.0, 0.1));
        for y in 0..40 {
            for x in 0..120 {
                assert_eq!(img.rgba(x, y)[3], 1.0);
            }
        }
    }

    #[test]
    fn smear_drags_paint_along_the_stroke() {
        // Red on the left, white on the right: a wet brush loaded with white at Mix 100 % carries
        // red into the white, fading with distance.
        let mut img = image(200, 40, WHITE);
        img.fill_rect(Rect::new(0, 0, 80, 40), &RED);
        let mut st = loaded(WHITE);
        let m = MixerSettings { flow: 0.5, ..settings(MixMode::Rgb, 1.0, 0.0, 1.0) };
        paint(&mut img, None, &mut st, &m, &line(20.0, 40.0, 180.0, 0.1));
        let redness = |x: i32| 1.0 - img.rgba(x, 20)[1];
        assert!(redness(90) > 0.3, "{}", redness(90));
        assert!(redness(90) > redness(120) && redness(120) > redness(170), "{} {} {}", redness(90), redness(120), redness(170));
        assert!(redness(195) < 1e-3);
    }

    #[test]
    fn pigment_mixes_like_paint() {
        let run = |mode: MixMode| {
            let mut img = image(140, 40, YELLOW);
            let mut st = loaded(BLUE);
            paint(&mut img, None, &mut st, &settings(mode, 1.0, 1.0, 0.5), &line(20.0, 10.0, 130.0, 0.1));
            img.rgba(70, 20)
        };
        let p = run(MixMode::Pigment);
        assert!(p[1] > p[0] && p[1] > p[2], "pigment blue over yellow should be green: {p:?}");
        let q = run(MixMode::Rgb);
        assert!(q[0] > q[1] - 0.1, "RGB mixing should not turn green: {q:?}");
    }

    #[test]
    fn low_load_runs_dry() {
        let mut img = image(300, 40, WHITE);
        let mut st = loaded(BLUE);
        paint(&mut img, None, &mut st, &settings(MixMode::Rgb, 0.5, 0.1, 0.5), &line(20.0, 10.0, 290.0, 0.1));
        let centre = |st: &MixerState| {
            let r = st.reservoir.as_ref().unwrap();
            r.px[r.side / 2 * r.side + r.side / 2][3]
        };
        assert!(centre(&st) < 0.2, "{}", centre(&st));
        assert!(img.rgba(30, 20)[0] < img.rgba(270, 20)[0], "early bluer than late");
        let mut full = loaded(BLUE);
        paint(&mut img, None, &mut full, &settings(MixMode::Rgb, 0.5, 1.0, 0.5), &line(30.0, 10.0, 290.0, 0.1));
        assert_eq!(centre(&full), 1.0);
    }

    #[test]
    fn stroke_has_no_banding() {
        // The saw-tooth hatching: along a steady stroke over a flat canvas neighbouring columns
        // match, at any sub-pixel position.
        for mode in [MixMode::Rgb, MixMode::Pigment] {
            for y in [20.0, 20.3, 20.5, 20.8] {
                let mut img = image(200, 40, YELLOW);
                let mut st = loaded(BLUE);
                paint(&mut img, None, &mut st, &settings(mode, 0.5, 1.0, 0.5), &line(y, 10.0, 190.0, 0.1));
                for row in 14..27 {
                    let worst = (70..150).map(|x| (0..3).map(|c| (img.rgba(x, row)[c] - img.rgba(x + 1, row)[c]).abs()).fold(0.0f32, f32::max)).fold(0.0f32, f32::max);
                    assert!(worst < 3.0 / 255.0, "{mode:?} y={y} row={row}: column-to-column jump {worst}");
                }
            }
        }
    }

    #[test]
    fn sample_all_layers_picks_up_from_the_merged_image() {
        let merged = image(120, 40, RED);
        let mut layer = Surface::new(PixelFormat::RGBA8);
        let mut st = loaded(WHITE);
        paint(&mut layer, Some(&merged), &mut st, &settings(MixMode::Rgb, 1.0, 0.0, 1.0), &line(20.0, 10.0, 110.0, 0.1));
        let c = layer.rgba(80, 20);
        assert!(c[3] > 0.5 && c[0] > 0.6 && c[1] < 0.4, "{c:?}");
        // Without Sample All Layers the empty layer has nothing to pick up.
        let mut layer = Surface::new(PixelFormat::RGBA8);
        let mut st = loaded(WHITE);
        paint(&mut layer, None, &mut st, &settings(MixMode::Rgb, 1.0, 0.0, 1.0), &line(20.0, 10.0, 110.0, 0.1));
        assert!(layer.rgba(80, 20)[3] < 0.05, "{:?}", layer.rgba(80, 20));
    }

    #[test]
    fn clean_after_stroke_controls_carry_over() {
        let run = |wash: bool| {
            let mut img = image(120, 40, RED);
            let mut st = loaded(WHITE);
            let m = settings(MixMode::Rgb, 1.0, 1.0, 1.0);
            paint(&mut img, None, &mut st, &m, &line(20.0, 10.0, 110.0, 0.1));
            if wash {
                st.wash();
            }
            let mut white = image(120, 40, WHITE);
            paint(&mut white, None, &mut st, &m, &line(20.0, 60.0, 60.0, 0.1));
            white.rgba(60, 20)
        };
        assert!(run(false)[1] < 0.9, "carried red: {:?}", run(false));
        assert!(close(run(true), WHITE, 1.0 / 255.0), "{:?}", run(true));
    }

    #[test]
    fn dirty_rect_covers_every_change() {
        let before = image(160, 80, YELLOW);
        let stroke = mixer_stroke(&line(20.0, 20.0, 140.0, 0.1), &settings(MixMode::Pigment, 0.5, 0.5, 0.5));
        let mut stroke = stroke;
        for (i, p) in stroke.points.iter_mut().enumerate() {
            p.y += i as f64 * 0.5;
        }
        let mut mixer = Mixer::new(&settings(MixMode::Pigment, 0.5, 0.5, 0.5), &loaded(BLUE), before.clone(), 20.0);
        let ctx = crate::BrushContext::new(&stroke.brush);
        let mut dirty = Rect::EMPTY;
        for (i, d) in crate::dabs(&stroke).iter().enumerate() {
            dirty = dirty.union(&mixer.stroke_dab(&ctx, d, i, None));
        }
        let all = Rect::new(0, 0, 160, 80);
        let mut work = Region::read(&before, all);
        mixer.flush(&mut work, all);
        let orig = Region::read(&before, all);
        for y in 0..80 {
            for x in 0..160 {
                if work.px(x, y) != orig.px(x, y) {
                    assert!(dirty.contains(x, y), "change at ({x},{y}) outside {dirty:?}");
                }
            }
        }
    }

    // --- PhotoCraft's additions -------------------------------------------------------------

    #[test]
    fn strokes_look_the_same_at_any_spacing() {
        for mode in [MixMode::Rgb, MixMode::Pigment] {
            let run = |spacing: f32| {
                let mut img = image(200, 40, YELLOW);
                img.fill_rect(Rect::new(100, 0, 200, 40), &RED);
                let mut st = loaded(BLUE);
                let m = MixerSettings { flow: 0.6, ..settings(mode, 0.5, 0.6, 0.5) };
                paint(&mut img, None, &mut st, &m, &line(20.0, 10.0, 190.0, spacing));
                img
            };
            let (a, b) = (run(0.05), run(0.15));
            let mut worst = 0.0f32;
            for y in 12..29 {
                for x in 30..170 {
                    let (p, q) = (a.rgba(x, y), b.rgba(x, y));
                    worst = worst.max((0..4).map(|c| (p[c] - q[c]).abs()).fold(0.0, f32::max));
                }
            }
            assert!(worst <= 4.0 / 255.0, "{mode:?}: spacing 5 % vs 15 % differ by {} levels", worst * 255.0);
        }
    }

    #[test]
    fn a_loaded_patch_paints_its_own_texture() {
        // ⌥-click: a patch, top half red, bottom half green; a dry stroke lays down both.
        let side = 25;
        let px = (0..side * side).map(|i| if i / side < side / 2 { RED } else { [0.0, 1.0, 0.0, 1.0] }).collect();
        let mut st = MixerState::default();
        st.load(Paint { side, px });
        let mut img = Surface::new(PixelFormat::RGBA8);
        paint(&mut img, None, &mut st, &settings(MixMode::Pigment, 0.0, 1.0, 0.0), &line(20.0, 10.0, 110.0, 0.1));
        let (top, bottom) = (img.rgba(60, 15), img.rgba(60, 25));
        assert!(top[0] > 0.8 && top[1] < 0.2, "{top:?}");
        assert!(bottom[1] > 0.8 && bottom[0] < 0.2, "{bottom:?}");
    }

    #[test]
    fn load_runs_dry_after_the_same_brush_widths_at_any_size() {
        // A dry stroke on an empty layer, 12 widths long: the paint left at each width along it
        // matches between a 20 px and a 200 px brush. An 8 px brush (its dabs filled in 1 px
        // apart, 12 % of its width, against 5 % for the others) stays close; its tip, 8 pixels
        // across, is a coarse copy of the round shape.
        let alpha_at = |size: f32| {
            let n = 12.0 * f64::from(size);
            let k = Stroke {
                brush: BrushSettings { size, hardness: 0.8, spacing: 0.1, pressure_size: false, ..Default::default() },
                points: vec![StrokePoint::new(f64::from(size), f64::from(size), 1.0), StrokePoint::new(f64::from(size) + n, f64::from(size), 1.0)],
            };
            let mut img = Surface::new(PixelFormat::RGBA8);
            let mut st = loaded(BLUE);
            paint(&mut img, None, &mut st, &settings(MixMode::Rgb, 0.0, 0.25, 0.0), &k);
            [1.0, 2.0, 3.0, 4.0, 6.0].map(|w| img.rgba((f64::from(size) * (1.0 + w)) as i32, size as i32)[3])
        };
        let (tiny, small, big) = (alpha_at(8.0), alpha_at(20.0), alpha_at(200.0));
        assert!(big[0] > 0.9 && big[4] < 0.5, "it runs dry: {big:?}");
        for ((a, b), c) in small.iter().zip(&big).zip(&tiny) {
            assert!((a - b).abs() < 0.05, "20 px {small:?} vs 200 px {big:?}");
            assert!((c - b).abs() < 0.12, "8 px {tiny:?} vs 200 px {big:?}");
        }
    }

    #[test]
    fn a_slanted_stroke_has_no_stair_steps() {
        // The saw-tooth hatching: a patch of alternating red and green rows, a pixel each, dragged
        // dry at a slant. Each streak drifts a row every few pixels; read on whole pixels it jumps
        // from red to green at each step, interpolated it shifts by the slant's share a pixel.
        for mode in [MixMode::Rgb, MixMode::Pigment] {
            let side = paint_side(30.0);
            let px = (0..side * side).map(|i| if (i / side).is_multiple_of(2) { RED } else { [0.0, 0.8, 0.0, 1.0] });
            let mut st = MixerState::default();
            st.load(Paint { side, px: px.collect() });
            let mut img = Surface::new(PixelFormat::RGBA8);
            let mut k = line(20.0, 10.0, 290.0, 0.1);
            k.brush.size = 30.0;
            for p in &mut k.points {
                p.y = 20.0 + (p.x - 10.0) * 0.18;
            }
            paint(&mut img, None, &mut st, &settings(mode, 0.0, 1.0, 0.0), &k);
            let mut worst = 0.0f32;
            for x in 60..240 {
                let yc = 20.0 + (f64::from(x) - 10.0) * 0.18;
                for dy in -8..=8 {
                    let y = (yc + f64::from(dy)).floor() as i32;
                    let (p, q) = (img.rgba(x, y), img.rgba(x + 1, y));
                    worst = worst.max((p[0] - q[0]).abs()).max((p[1] - q[1]).abs());
                }
            }
            // Rows differ by 0.8; drifting evenly, 0.18 of that a pixel.
            assert!(worst < 0.3, "{mode:?}: column-to-column jump {worst}");
        }
    }

    #[test]
    fn mix_alone_sets_the_canvas_share() {
        // Wet 50 / Mix 50 over a flat colour, one width into the stroke: within a few percent of
        // halfway between the load and the canvas (RGB, where halfway is the plain average). At
        // Load 100 % the load stays as it is, so Mix alone decides; at Load 50 % it has thinned a
        // little in a width (half-life 5 widths), leaving the canvas a few percent more.
        for (wet, load, mix, tolerance) in [(0.5, 1.0, 0.5, 0.03), (1.0, 1.0, 0.5, 0.03), (0.5, 1.0, 0.25, 0.03), (0.75, 1.0, 0.75, 0.03), (0.5, 0.5, 0.5, 0.06)] {
            let mut img = image(400, 80, YELLOW);
            let mut st = loaded(BLUE);
            let mut k = line(40.0, 40.0, 360.0, 0.1);
            k.brush.size = 40.0;
            paint(&mut img, None, &mut st, &settings(MixMode::Rgb, wet, load, mix), &k);
            let got = img.rgba(80, 40);
            let want: Vec<f32> = (0..3).map(|c| BLUE[c] + (YELLOW[c] - BLUE[c]) * mix).collect();
            let span = (0..3).map(|c| (YELLOW[c] - BLUE[c]).abs()).fold(0.0, f32::max);
            let off = (0..3).map(|c| (got[c] - want[c]).abs()).fold(0.0, f32::max) / span;
            assert!(off < tolerance, "Wet {wet} Load {load} Mix {mix}: {got:?} is {:.1} % off {want:?}", off * 100.0);
        }
    }

    #[test]
    fn crossing_into_a_colour_catches_up_within_a_width() {
        // Wet 50: a stroke from cream into orange is within 10 % of a stroke over orange alone,
        // one width past the edge.
        let cream = [235.0 / 255.0, 225.0 / 255.0, 190.0 / 255.0, 1.0];
        let orange = [240.0 / 255.0, 120.0 / 255.0, 20.0 / 255.0, 1.0];
        for mode in [MixMode::Rgb, MixMode::Pigment] {
            let run = |from_cream: bool| {
                let mut img = image(500, 80, orange);
                if from_cream {
                    img.fill_rect(Rect::new(0, 0, 200, 80), &cream);
                }
                let mut st = loaded(BROWN);
                let mut k = line(40.0, 20.0, 480.0, 0.1);
                k.brush.size = 40.0;
                paint(&mut img, None, &mut st, &settings(mode, 0.5, 0.5, 0.5), &k);
                (img.rgba(240, 40), img.rgba(100, 40))
            };
            let ((crossed, over_cream), (alone, _)) = (run(true), run(false));
            let gap = (0..3).map(|c| (over_cream[c] - alone[c]).abs()).fold(0.0, f32::max);
            let off = (0..3).map(|c| (crossed[c] - alone[c]).abs()).fold(0.0, f32::max) / gap;
            assert!(off < 0.1, "{mode:?}: one width in, {:.1} % of the way back to the cream stroke", off * 100.0);
        }
    }

    #[test]
    fn a_dry_reservoir_smudges_and_an_empty_pickup_paints_the_load() {
        let codec = Codec::new(MixMode::Rgb, PixelFormat::RGBA8);
        let (load, canvas) = (codec.encode_rgba(BLUE), codec.encode_rgba(YELLOW));
        // A dry reservoir at Mix 100 %: the picked-up paint only.
        let dry = Colorant { a: 0.0, ..load };
        assert_eq!(brush_paint(&dry, &canvas, 1.0), canvas);
        // Nothing picked up: the plain load at any Mix.
        for m in [0.0, 0.5, 1.0] {
            assert_eq!(brush_paint(&load, &Colorant::NONE, m), load, "Mix {m}");
        }
    }

    #[test]
    fn a_textured_load_leaves_streaks_not_a_stamp_pattern() {
        // A patch of noise dragged dry along a row: each of its rows streaks along the stroke.
        // Taking only the texel under a pixel at the last dab over it would print the patch
        // again every dab, a pattern repeating across the streaks.
        let side = paint_side(40.0);
        let px = (0..side * side).map(|i| {
            let v = crate::rng::hash2(i as i32, 7, 3);
            [0.3 + 0.6 * v, 0.5, 0.2, 1.0]
        });
        let mut st = MixerState::default();
        st.load(Paint { side, px: px.collect() });
        let mut img = Surface::new(PixelFormat::RGBA8);
        let mut k = line(30.3, 10.0, 290.0, 0.1);
        k.brush.size = 40.0;
        paint(&mut img, None, &mut st, &settings(MixMode::Rgb, 0.0, 1.0, 0.0), &k);
        let mut worst = 0.0f32;
        for y in 24..37 {
            for x in 80..220 {
                worst = worst.max((img.rgba(x, y)[0] - img.rgba(x + 1, y)[0]).abs());
            }
        }
        assert!(worst < 0.02, "column-to-column jump along the streaks {worst}");
    }

    #[test]
    fn reservoir_texels_run_dry_and_are_dirtied_where_they_paint() {
        let mut img = image(300, 40, YELLOW);
        let mut st = loaded(BLUE);
        paint(&mut img, None, &mut st, &settings(MixMode::Rgb, 1.0, 0.3, 0.5), &line(20.0, 10.0, 290.0, 0.1));
        let r = st.reservoir.clone().unwrap();
        let (centre, corner) = (r.px[r.side / 2 * r.side + r.side / 2], r.px[0]);
        assert!(centre[3] < 0.5 && corner[3] == 1.0, "centre {centre:?}, corner {corner:?}");
        assert!(centre[0] > BLUE[0] + 0.01 && corner == BLUE, "the centre picked up yellow: {centre:?}");
        // Load after each stroke refills it, keeping the colour; washing restores the colour.
        st.refill();
        assert_eq!(st.reservoir.as_ref().unwrap().px[r.side / 2 * r.side + r.side / 2][3], 1.0);
        st.wash();
        assert_eq!(st.reservoir.as_ref().unwrap().px[r.side / 2 * r.side + r.side / 2], BLUE);
    }

    #[test]
    fn transfer_wetness_control_scales_pickup_per_dab() {
        let run = |pressure: f32| {
            let mut img = image(200, 40, BLUE);
            let mut st = loaded(RED);
            let mut k = line(20.0, 10.0, 190.0, 0.1);
            k.brush.transfer = crate::Transfer { enabled: true, wetness: crate::Dynamic::controlled(crate::Control::PenPressure), ..Default::default() };
            for p in &mut k.points {
                p.pressure = pressure;
            }
            paint(&mut img, None, &mut st, &settings(MixMode::Rgb, 1.0, 1.0, 1.0), &k);
            img.rgba(100, 20)
        };
        assert!(run(0.0)[2] < 0.15, "{:?}", run(0.0));
        assert!(run(1.0)[2] > 0.5, "{:?}", run(1.0));
    }

    #[test]
    fn works_at_all_depths_and_modes() {
        for fmt in [PixelFormat::RGBA8, PixelFormat::RGBA16, PixelFormat::RGBA32F, PixelFormat::GRAY8, PixelFormat::GRAYA8, PixelFormat::CMYKA8] {
            for mode in [MixMode::Rgb, MixMode::Pigment] {
                let mut s = Surface::new(fmt);
                s.fill_rect(Rect::new(0, 0, 100, 40), &vec![1.0; fmt.channels()]);
                let mut st = loaded(RED);
                paint(&mut s, None, &mut st, &settings(mode, 0.5, 0.5, 0.5), &line(20.0, 4.0, 96.0, 0.1));
                assert!(s.pixel(50, 20).iter().all(|v| v.is_finite()), "{fmt:?} {mode:?}");
                assert!(st.pickup.is_some(), "{fmt:?} {mode:?}");
            }
        }
    }

    // --- Every blend through Mixbox (Sochorová & Jamriška 2021) ---------------------------------

    /// How far (largest channel, 8-bit levels) `c` lies from Mixbox's gradient between `a` and `b`,
    /// and where on it: the gradient sampled at 1/2000 steps.
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

    /// How far (largest channel) `c` lies from the straight RGB line between `a` and `b`.
    fn off_rgb_line(c: [i32; 3], a: [u8; 3], b: [u8; 3]) -> f32 {
        (0..=2000)
            .map(|i| {
                let t = i as f32 / 2000.0;
                (0..3).map(|k| (f32::from(a[k]) + (f32::from(b[k]) - f32::from(a[k])) * t - c[k] as f32).abs()).fold(0.0, f32::max)
            })
            .fold(f32::MAX, f32::min)
    }

    #[test]
    fn a_stroke_across_blue_and_yellow_follows_the_mixbox_gradient_not_rgb() {
        // The paper's Fig. 5: Cobalt Blue and Hansa Yellow. A brush loaded with the blue strokes
        // from the blue half into the yellow one, so every paint in play is a mix of the two: each
        // pixel must lie on Mixbox's curved gradient between them, away from RGB's straight
        // grey-blue blend.
        let (blue, yellow) = ([0u8, 33, 133], [252u8, 211, 0]);
        let mut img = image(120, 40, rgba8(blue));
        img.fill_rect(Rect::new(60, 0, 120, 40), &rgba8(yellow));
        let mut st = loaded(rgba8(blue));
        paint(&mut img, None, &mut st, &settings(MixMode::Pigment, 0.5, 0.5, 0.5), &line(20.0, 10.0, 110.0, 0.1));
        let (mut mixed, mut worst) = (0, 0);
        for y in 0..40 {
            for x in 0..120 {
                let c = u8s3(img.rgba(x, y));
                let (d, t) = off_gradient(c, blue, yellow);
                worst = worst.max(d);
                assert!(d <= 2, "({x}, {y}) {c:?} is {d} levels off the Mixbox gradient");
                if (0.2..=0.8).contains(&t) {
                    mixed += 1;
                    let r = off_rgb_line(c, blue, yellow);
                    assert!(r >= 20.0, "({x}, {y}) {c:?} is only {r} levels from the RGB blend");
                }
            }
        }
        assert!(mixed >= 100, "the stroke mixed only {mixed} pixels");
        // RGB mode, for contrast, makes the grey-blue blend.
        let mut img = image(120, 40, rgba8(blue));
        img.fill_rect(Rect::new(60, 0, 120, 40), &rgba8(yellow));
        let mut st = loaded(rgba8(blue));
        paint(&mut img, None, &mut st, &settings(MixMode::Rgb, 0.5, 0.5, 0.5), &line(20.0, 10.0, 110.0, 0.1));
        let greyish = (0..120).filter(|&x| off_rgb_line(u8s3(img.rgba(x, 20)), blue, yellow) <= 2.0 && off_gradient(u8s3(img.rgba(x, 20)), blue, yellow).0 > 10).count();
        assert!(greyish > 10, "RGB mode should blend in a straight line ({greyish} pixels)");
        println!("worst {worst} levels off the gradient over {mixed} mixed pixels");
    }

    #[test]
    fn bilinear_halfway_between_two_pixels_is_mixbox_lerp() {
        let codec = Codec::new(MixMode::Pigment, PixelFormat::RGBA8);
        for (i, a) in PIGMENTS.iter().enumerate() {
            for b in PIGMENTS[i + 1..].iter().chain([[255u8, 255, 255]].iter()) {
                let want = mixbox::lerp(a, b, 0.5).map(i32::from);
                let (za, zb) = (codec.encode_rgba(rgba8(*a)), codec.encode_rgba(rgba8(*b)));
                // Picking up between two canvas pixels.
                let got = u8s3(codec.decode_rgba(&bilinear(&[za, zb], 2, 1, 0.5, 0.0)));
                assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() <= 1), "{a:?} | {b:?}: {got:?}, Mixbox {want:?}");
                // A loaded 2 × 2 patch on a 3 × 3 tip: the middle column lies halfway.
                let patch = Paint { side: 2, px: vec![rgba8(*a), rgba8(*b), rgba8(*a), rgba8(*b)] };
                let grid = codec.grid(&patch, 3);
                for row in 0..3 {
                    let got = u8s3(codec.decode_rgba(&grid[row * 3 + 1]));
                    assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() <= 1), "patch {a:?} | {b:?}: {got:?}, Mixbox {want:?}");
                }
            }
        }
    }

    /// Hue (degrees) and HSV saturation of an 8-bit colour.
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
    fn phthalo_blue_with_white_turns_turquoise_not_grey() {
        // The paper's Fig. 4: Phthalo Blue gains white towards turquoise (hue falling from its
        // ~225° towards 180°) and stays saturated; RGB fades it towards lavender-grey.
        let phthalo = [13u8, 27, 68];
        let (h0, _) = hue_sat(phthalo.map(i32::from));
        let codec = Codec::new(MixMode::Pigment, PixelFormat::RGBA8);
        let (p, w) = (codec.encode_rgba(rgba8(phthalo)), codec.encode_rgba(WHITE));
        let mut last = h0;
        for t in [0.2, 0.4, 0.6, 0.8] {
            let c = u8s3(codec.decode_rgba(&mix(&p, &w, t)));
            let (h, sat) = hue_sat(c);
            assert!(h < last && h < h0 - 10.0, "{t}: {c:?} hue {h:.0}° doesn't move from {h0:.0}° towards turquoise");
            assert!(sat > 0.25, "{t}: {c:?} turned grey (saturation {sat:.2})");
            if t == 0.2 {
                // A little white first raises the saturation, revealing the pigment.
                assert!(sat > hue_sat(phthalo.map(i32::from)).1, "{c:?}: saturation {sat:.2}");
            }
            last = h;
        }
        // A stroke of white over Phthalo Blue at Mix 50 %.
        let mut img = image(100, 40, rgba8(phthalo));
        let mut st = loaded(WHITE);
        paint(&mut img, None, &mut st, &settings(MixMode::Pigment, 0.5, 1.0, 0.5), &line(20.0, 4.0, 96.0, 0.1));
        let c = u8s3(img.rgba(60, 20));
        let (h, sat) = hue_sat(c);
        assert!(h < h0 - 10.0 && sat > 0.25, "{c:?}: hue {h:.0}°, saturation {sat:.2}");
        // RGB keeps the hue and loses saturation.
        let mut img = image(100, 40, rgba8(phthalo));
        let mut st = loaded(WHITE);
        paint(&mut img, None, &mut st, &settings(MixMode::Rgb, 0.5, 1.0, 0.5), &line(20.0, 4.0, 96.0, 0.1));
        let (hr, sr) = hue_sat(u8s3(img.rgba(60, 20)));
        assert!((hr - h0).abs() < 5.0 && sr < sat, "RGB: hue {hr:.0}°, saturation {sr:.2}");
        println!("phthalo {h0:.0}°; stroke Pigment {c:?} {h:.0}° sat {sat:.2}; RGB {:?} {hr:.0}° sat {sr:.2}", u8s3(img.rgba(60, 20)));
    }

    #[test]
    fn a_feathered_selection_edge_mixes_as_pigment() {
        // Half selected: the blue laid on the yellow comes out as Mixbox's halfway green.
        let mut img = image(60, 40, YELLOW);
        let mut sel = Surface::new(PixelFormat::GRAY8);
        sel.fill_rect(Rect::new(0, 0, 60, 40), &[0.5]);
        let mut st = loaded(BLUE);
        let stroke = mixer_stroke(&line(20.0, 4.0, 56.0, 0.1), &settings(MixMode::Pigment, 0.0, 1.0, 0.0));
        let mut mixer = Mixer::new(&settings(MixMode::Pigment, 0.0, 1.0, 0.0), &st, img.clone(), stroke.brush.size);
        run_stroke(&mut img, &stroke, &mut mixer, |_| None, Some(&sel), false);
        st = mixer.state();
        assert!(st.exact.is_some());
        let half = (0.5f32 * 255.0).round() / 255.0;
        let want = mixbox::lerp(&[252, 211, 0], &[0, 33, 133], half).map(i32::from);
        let got = u8s3(img.rgba(30, 20));
        assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() <= 2), "{got:?}, Mixbox {want:?}");
    }

    #[test]
    fn the_brush_keeps_its_latents_between_strokes() {
        let m = MixerSettings { ..settings(MixMode::Pigment, 0.8, 0.3, 0.6) };
        let mut img = image(100, 40, YELLOW);
        img.fill_rect(Rect::new(50, 0, 100, 40), &BLUE);
        let mut st = loaded(RED);
        paint(&mut img, None, &mut st, &m, &line(20.0, 4.0, 96.0, 0.1));
        let e = st.exact.clone().expect("latents kept");
        let next = Mixer::new(&m, &st, img.clone(), 20.0);
        assert_eq!(next.reservoir, e.reservoir);
        assert_eq!(next.pickup, e.pickup);
        // Another space (RGB) or a wash goes back to the paint as RGBA.
        let rgb = Mixer::new(&settings(MixMode::Rgb, 0.8, 0.3, 0.6), &st, img.clone(), 20.0);
        assert_ne!(rgb.reservoir, e.reservoir);
        st.wash();
        assert!(st.exact.is_none());
    }

    #[test]
    fn pigment_mixes_through_mixbox_in_other_colour_models() {
        use photocraft_color::ColorMode;
        for mode in [ColorMode::Lab, ColorMode::Cmyk, ColorMode::Grayscale] {
            let fmt = PixelFormat { mode, sample: SampleType::U8, alpha: true };
            let codec = Codec::new(MixMode::Pigment, fmt);
            let c = codec.decode_rgba(&mix(&codec.encode_rgba(BLUE), &codec.encode_rgba(YELLOW), 0.5));
            if mode == ColorMode::Grayscale {
                // Grey has no hue; mixing a grey with itself stays that grey.
                let g = codec.encode_rgba([0.4, 0.4, 0.4, 1.0]);
                assert!((codec.decode_rgba(&mix(&g, &g, 0.5))[0] - 0.4).abs() < 0.01);
            } else {
                assert!(c[1] > c[0] && c[1] > c[2], "{mode:?}: {c:?} should be green");
            }
        }
    }

    /// Time one dab, RGB against Pigment, per pixel under the tip, on one core and on all of them
    /// (stroke cache already converted). The tip is `MIXER_DAB_SIZE` px (default 300, the largest
    /// built-in brush).
    /// `cargo test -p photocraft-paint --release --lib mixer::tests::dab_timing -- --ignored --nocapture`
    #[test]
    #[ignore = "timing"]
    fn dab_timing() {
        let size: f32 = std::env::var("MIXER_DAB_SIZE").ok().and_then(|v| v.parse().ok()).unwrap_or(300.0);
        for mode in [MixMode::Rgb, MixMode::Pigment] {
            dab_timing_in(mode, size);
        }
    }

    fn dab_timing_in(mode: MixMode, size: f32) {
        let mut s = image(1400, 400, WHITE);
        for i in 0..70 {
            s.fill_rect(Rect::new(i * 20, 0, i * 20 + 10, 400), &[0.9, 0.7, 0.1, 1.0]);
        }
        let m = settings(mode, 0.6, 0.5, 0.5);
        let mut st = line(200.0, 160.0, 1240.0, 0.05);
        st.brush.size = size;
        let st = mixer_stroke(&st, &m);
        let ctx = crate::BrushContext::new(&st.brush);
        let fps: Vec<Footprint> = crate::dabs(&st).iter().enumerate().map(|(i, d)| ctx.footprint(d, i)).collect();
        let pixels: usize = fps.iter().map(|fp| fp.cov.iter().filter(|&&c| c / fp.dab.alpha >= MASK_EPS).count()).sum();
        let all = Rect::new(0, 0, 1400, 400);
        let run = || {
            let mut mixer = Mixer::new(&m, &loaded(BLUE), s.clone(), size);
            let t = std::time::Instant::now();
            mixer.ensure_tiles(all, None);
            let convert = t.elapsed().as_nanos() as f64 / (1400.0 * 400.0);
            let t = std::time::Instant::now();
            for fp in &fps {
                mixer.dab(fp, None);
            }
            let dab = t.elapsed().as_nanos() as f64 / pixels as f64;
            let mut work = Region::read(&s, all);
            let t = std::time::Instant::now();
            mixer.flush(&mut work, all);
            (convert, dab, t.elapsed().as_nanos() as f64 / (1400.0 * 400.0))
        };
        let best = |f: &dyn Fn() -> (f64, f64, f64)| (0..3).map(|_| f()).fold((f64::MAX, f64::MAX, f64::MAX), |a, b| (a.0.min(b.0), a.1.min(b.1), a.2.min(b.2)));
        let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().map(|p| p.install(|| best(&run))).unwrap_or((f64::NAN, f64::NAN, f64::NAN));
        let many = best(&run);
        println!(
            "{mode:?}, {size} px tip, {} dabs of {:.0} px: dab {:.1} ns/px on 1 core, {:.1} on {} threads ({:.2} ms per dab); conversion {:.1} ns/px; flush {:.1} ns/px",
            fps.len(),
            pixels as f64 / fps.len() as f64,
            one.1,
            many.1,
            rayon::current_num_threads(),
            many.1 * pixels as f64 / fps.len() as f64 / 1e6,
            one.0,
            one.2
        );
    }
}

