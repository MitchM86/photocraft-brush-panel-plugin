//! Art History Brush, as in Photoshop: paints stylised strokes in the colours of a history state
//! or snapshot (the History Brush's source). Each "burst" (where the pointer is, again every few
//! pixels of travel and every few tenths of a second while it is held still) lays several short
//! strokes inside the options bar's Area around the pointer. A stroke starts at a random point
//! there, takes the source colour under it and follows the source image's contours (across its
//! luminance gradient), as long, loose and curly as the Style says. Tolerance keeps strokes to
//! where the image now differs from the source by at least that much.
//!
//! Strokes come from a seeded generator fed the stroke's points in order, so the live preview
//! (`LiveArtHistory`, any batching) and the committed `paint.artHistoryBrush` with the same seed
//! paint the same pixels.

use std::sync::Arc;

use photocraft_color::{BlendMode, PixelFormat};
use photocraft_doc::{Document, LayerId};
use photocraft_geom::Rect;
use photocraft_paint::retouch::{Region, alpha_index, apply_coverage, stroke_coverage};
use photocraft_paint::{BrushSettings, Stroke, StrokePoint};
use photocraft_raster::{Surface, to_rgba};
use serde_json::{Value, json};

use super::{bad, blend_param, damage_json, has_pixel_layer, history_source, num, stroke_brush};
use crate::{EngineError, Result, Session};

const CMD: &str = "paint.artHistoryBrush";

/// Photoshop's Style menu, in its order (the params' names).
pub const STYLES: [&str; 10] =
    ["tightShort", "tightMedium", "tightLong", "looseMedium", "looseLong", "dab", "tightCurl", "looseCurl", "tightCurlLong", "looseCurlLong"];

/// A held pointer bursts this often (ms).
const HOLD_INTERVAL_MS: f64 = 60.0;
/// At most this many strokes per burst.
const MAX_STROKES: usize = 24;

/// How a Style draws: length in brush diameters, how much each step wanders off the contour
/// (radians, either way) and how much it turns every step (radians).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Style {
    length: f32,
    wander: f32,
    curl: f32,
}

fn style(name: &str) -> Option<Style> {
    let (length, wander, curl) = match name {
        "tightShort" => (2.5, 0.06, 0.0),
        "tightMedium" => (5.0, 0.06, 0.0),
        "tightLong" => (10.0, 0.06, 0.0),
        "looseMedium" => (5.0, 0.45, 0.0),
        "looseLong" => (10.0, 0.45, 0.0),
        "dab" => (0.0, 0.0, 0.0),
        "tightCurl" => (5.0, 0.04, 0.55),
        "looseCurl" => (5.0, 0.3, 0.55),
        "tightCurlLong" => (11.0, 0.04, 0.32),
        "looseCurlLong" => (11.0, 0.3, 0.32),
        _ => return None,
    };
    Some(Style { length, wander, curl })
}

/// Seeded random numbers: draw `n` of the stroke is `rand01(seed, n)`.
struct Rng {
    seed: u64,
    n: u64,
}

impl Rng {
    fn next(&mut self) -> f64 {
        self.n += 1;
        f64::from(photocraft_paint::rng::rand01(self.seed, self.n, 0))
    }
}

/// Where bursts fall along the stroke's points.
struct Bursts {
    spacing: f64,
    last: Option<(f64, f64)>,
    burst_time: f64,
    travelled: f64,
}

impl Bursts {
    fn feed(&mut self, pts: &[StrokePoint], out: &mut Vec<[f64; 2]>) {
        for p in pts {
            let burst = match self.last {
                None => true,
                Some((x, y)) => {
                    self.travelled += (p.x - x).hypot(p.y - y);
                    self.travelled >= self.spacing || p.time - self.burst_time >= HOLD_INTERVAL_MS
                }
            };
            self.last = Some((p.x, p.y));
            if burst {
                out.push([p.x, p.y]);
                self.travelled = 0.0;
                self.burst_time = p.time;
            }
        }
    }
}

/// The stroke's settings and state, shared by the live preview and the commit.
struct Art {
    brush: BrushSettings,
    style: Style,
    area: f64,
    tolerance: f32,
    mode: BlendMode,
    /// The source, in the target surface's format.
    src: Surface,
    bounds: Rect,
    rng: Rng,
    bursts: Bursts,
}

impl Art {
    fn new(s: &Session, id: Option<LayerId>, p: &Value, target: &Surface, bounds: Rect) -> Result<Self> {
        let brush = stroke_brush(&s.tools.brush, p);
        crate::brush_cmds::validate_brush(&brush, CMD)?;
        let name = p.get("style").and_then(Value::as_str).unwrap_or("tightShort");
        let style = style(name).ok_or_else(|| bad(CMD, format!("unknown style `{name}` (one of {})", STYLES.join(", "))))?;
        let area = f64::from(num(p, "area", 50.0));
        if !(0.0..=500.0).contains(&area) {
            return Err(bad(CMD, "`area` must be 0..500 px"));
        }
        let tolerance = num(p, "tolerance", 0.0);
        if !(0.0..=100.0).contains(&tolerance) {
            return Err(bad(CMD, "`tolerance` must be 0..100 %"));
        }
        let src = history_source(s, id, p, CMD)?;
        let src = if src.format() == target.format() { src } else { src.convert(target.format()) };
        let seed = p.get("seed").and_then(Value::as_u64).unwrap_or(1);
        let spacing = (area * 0.25).max(f64::from(brush.size) * 0.5).max(2.0);
        Ok(Self {
            brush,
            style,
            area,
            tolerance,
            mode: blend_param(p, CMD)?,
            src,
            bounds,
            rng: Rng { seed, n: 0 },
            bursts: Bursts { spacing, last: None, burst_time: 0.0, travelled: 0.0 },
        })
    }

    /// Paint the bursts `pts` bring onto `surf`; returns what changed.
    fn feed(&mut self, pts: &[StrokePoint], surf: &mut Surface, sel: Option<&Surface>, lock: bool) -> Rect {
        let mut at = Vec::new();
        self.bursts.feed(pts, &mut at);
        at.iter().fold(Rect::EMPTY, |dmg, &c| dmg.union(&self.burst(c, surf, sel, lock)))
    }

    fn burst(&mut self, c: [f64; 2], surf: &mut Surface, sel: Option<&Surface>, lock: bool) -> Rect {
        let size = f64::from(self.brush.size);
        let n = ((1.5 * self.area / size).round() as usize).clamp(1, MAX_STROKES);
        let mut dmg = Rect::EMPTY;
        for _ in 0..n {
            let (a, r) = (self.rng.next() * std::f64::consts::TAU, self.rng.next().sqrt() * self.area);
            let start = [c[0] + a.cos() * r, c[1] + a.sin() * r];
            let flip = self.rng.next() < 0.5;
            let length = f64::from(self.style.length) * size * (0.7 + 0.6 * self.rng.next());
            let wander: Vec<f64> = (0..64).map(|_| self.rng.next() * 2.0 - 1.0).collect();
            if let Some(points) = self.path(start, flip, length, &wander, surf) {
                dmg = dmg.union(&self.paint(&points, surf, sel, lock));
            }
        }
        dmg
    }

    /// The stroke from `start` along the source's contours, or `None` where it paints nothing
    /// (off the image, transparent source, or within Tolerance of the image now).
    fn path(&self, start: [f64; 2], flip: bool, length: f64, wander: &[f64], surf: &Surface) -> Option<Vec<[f64; 2]>> {
        let (x, y) = (start[0].floor() as i32, start[1].floor() as i32);
        if !self.bounds.contains(x, y) {
            return None;
        }
        let fmt = self.src.format();
        let here = to_rgba(&fmt, &self.src.pixel(x, y));
        if here[3] <= 0.0 {
            return None;
        }
        if self.tolerance > 0.0 {
            let now = to_rgba(&fmt, &surf.pixel(x, y));
            let diff = (0..4).map(|i| (here[i] - now[i]).abs()).fold(0.0f32, f32::max);
            if diff * 100.0 < self.tolerance {
                return None;
            }
        }
        let step = (f64::from(self.brush.size) * 0.35).max(1.0);
        let steps = (length / step).round() as usize;
        let mut points = vec![start];
        let mut angle = self.contour(start).unwrap_or(wander[0] * std::f64::consts::PI) + if flip { std::f64::consts::PI } else { 0.0 };
        let (wobble, curl) = (f64::from(self.style.wander), f64::from(self.style.curl) * if flip { -1.0 } else { 1.0 });
        let mut p = start;
        for i in 0..steps {
            // Follow the contour the way the stroke already goes (contours have no direction).
            if let Some(c) = self.contour(p) {
                let c = if angle_between(c, angle).abs() > std::f64::consts::FRAC_PI_2 { c + std::f64::consts::PI } else { c };
                angle += angle_between(c, angle) * if wobble > 0.2 { 0.35 } else { 0.8 };
            }
            angle += curl + wobble * wander[(i + 1) % wander.len()];
            p = [p[0] + angle.cos() * step, p[1] + angle.sin() * step];
            if !self.bounds.contains(p[0].floor() as i32, p[1].floor() as i32) {
                break;
            }
            points.push(p);
        }
        Some(points)
    }

    /// Direction of the source's contour at `p` (radians), `None` where it is flat.
    fn contour(&self, p: [f64; 2]) -> Option<f64> {
        let h = (f64::from(self.brush.size) * 0.5).max(2.0);
        let fmt = self.src.format();
        let lum = |dx: f64, dy: f64| luminance(&fmt, &self.src, p[0] + dx, p[1] + dy);
        let (gx, gy) = (lum(h, 0.0) - lum(-h, 0.0), lum(0.0, h) - lum(0.0, -h));
        (gx.hypot(gy) > 1e-3).then(|| gy.atan2(gx) + std::f64::consts::FRAC_PI_2)
    }

    /// One stroke of the brush in the source colour at its start.
    fn paint(&self, points: &[[f64; 2]], surf: &mut Surface, sel: Option<&Surface>, lock: bool) -> Rect {
        let fmt = surf.format();
        let color = self.src.pixel(points[0][0].floor() as i32, points[0][1].floor() as i32);
        let stroke = Stroke { brush: self.brush.clone(), points: points.iter().map(|q| StrokePoint::new(q[0], q[1], 1.0)).collect() };
        let (bounds, cov) = stroke_coverage(&stroke);
        if bounds.intersect(&self.bounds).is_empty() {
            return Rect::EMPTY;
        }
        let mut paint = Region::new(bounds, fmt.channels());
        for px in paint.data.chunks_exact_mut(fmt.channels()) {
            px.copy_from_slice(&color);
        }
        // Painted colour is opaque where the source is: a stroke shows its colour, not its alpha.
        if let Some(a) = alpha_index(&fmt) {
            for px in paint.data.chunks_exact_mut(fmt.channels()) {
                px[a] = 1.0;
            }
        }
        apply_coverage(surf, bounds, &cov, self.brush.opacity, sel, lock, &paint, self.mode)
    }
}

/// Signed difference `a - b` wrapped into -π..π.
fn angle_between(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(std::f64::consts::TAU);
    if d > std::f64::consts::PI { d - std::f64::consts::TAU } else { d }
}

fn luminance(fmt: &PixelFormat, s: &Surface, x: f64, y: f64) -> f64 {
    let c = to_rgba(fmt, &s.pixel(x.floor() as i32, y.floor() as i32));
    f64::from(0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2])
}

fn target_id(s: &Session, p: &Value) -> Result<Option<LayerId>> {
    if crate::channel_cmds::is_channel_target(p) {
        return Ok(None);
    }
    match p.get("layer").and_then(Value::as_u64) {
        Some(id) => Ok(Some(LayerId(id))),
        None => s.active().and_then(|d| d.active_layer).map(Some).ok_or(EngineError::Other("no active layer".into())),
    }
}

/// `paint.artHistoryBrush`: the whole stroke as one History step.
pub(super) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let pts = crate::brush_cmds::parse_points(p, CMD)?;
    crate::brush_cmds::check_coords(&pts, CMD)?;
    let id = target_id(s, p)?;
    let mut doc = (*s.active().ok_or(EngineError::NoDocument)?.doc).clone();
    let bounds = doc.bounds();
    let mut art = {
        let (surf, _) = crate::channel_cmds::target_surface(&mut doc, id, p)?;
        Art::new(s, id, p, surf, bounds)?
    };
    let dmg = super::run_stroke(s, "Art History Brush", id, p, |_, surf, sel, lock| Ok(art.feed(&pts, surf, sel, lock)))?;
    Ok(json!({ "damage": damage_json(dmg) }))
}

/// The Art History Brush drawn while the stroke is made: each step paints the bursts its points
/// bring onto a copy of the document.
pub struct LiveArtHistory {
    pub doc: Arc<Document>,
    pub seed: u64,
    id: Option<LayerId>,
    params: Value,
    art: Art,
    sel: Option<Surface>,
    lock: bool,
    bounds: Rect,
}

impl LiveArtHistory {
    /// Start from `paint.artHistoryBrush` params (a `seed` is required, so the commit can repeat it);
    /// their `points` are painted.
    pub fn begin(s: &Session, p: &Value) -> Result<Self> {
        has_pixel_layer(s).map_err(EngineError::Other)?;
        let pts = crate::brush_cmds::parse_points(p, CMD)?;
        let seed = p.get("seed").and_then(Value::as_u64).ok_or_else(|| bad(CMD, "a live stroke needs a `seed`"))?;
        let id = target_id(s, p)?;
        let mut doc = (*s.active().ok_or(EngineError::NoDocument)?.doc).clone();
        let bounds = doc.bounds();
        let sel = doc.selection.clone();
        let (surf, lock) = crate::channel_cmds::target_surface(&mut doc, id, p)?;
        let art = Art::new(s, id, p, surf, bounds)?;
        let mut live = Self { doc: Arc::new(doc), seed, id, params: p.clone(), art, sel, lock, bounds: Rect::EMPTY };
        live.push(&pts)?;
        Ok(live)
    }

    /// Everything painted so far.
    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Paint the bursts of more points; returns what changed.
    pub fn push(&mut self, pts: &[StrokePoint]) -> Result<Rect> {
        crate::brush_cmds::check_coords(pts, CMD)?;
        let (surf, _) = crate::channel_cmds::target_surface(Arc::make_mut(&mut self.doc), self.id, &self.params)?;
        let dmg = self.art.feed(pts, surf, self.sel.as_ref(), self.lock);
        self.bounds = self.bounds.union(&dmg);
        Ok(dmg)
    }
}

pub(super) fn spec() -> crate::commands::CommandSpec {
    crate::commands::CommandSpec {
        id: CMD,
        label: "Art History Brush",
        menu: &[],
        shortcut: None,
        params: brush_params!(
            r#","points" may carry [x,y,pressure,tiltX,tiltY,rotation,timeMs] (a held point bursts again every 60 ms),"style":"tightShort|tightMedium|tightLong|looseMedium|looseLong|dab|tightCurl|looseCurl|tightCurlLong|looseCurlLong"="tightShort","area":0..500 px=50,"tolerance":0..100 %=0,"mode":blend="normal","seed":u64=1,"state":index?,"snapshot":index? (the source, as paint.historyBrush)"#
        ),
        enabled: has_pixel_layer,
        run,
        journal: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let mut s = Session::new();
        s.execute("file.new", json!({"width": 120, "height": 80, "background": "white"})).unwrap();
        // The opening state: left half red, right half blue.
        s.execute("select.rect", json!({"x": 0, "y": 0, "width": 60, "height": 80})).unwrap();
        s.execute("edit.fill", json!({"color": "#ff0000"})).unwrap();
        s.execute("select.inverse", json!({})).unwrap();
        s.execute("edit.fill", json!({"color": "#0000ff"})).unwrap();
        s.execute("select.deselect", json!({})).unwrap();
        s.execute("history.newSnapshot", json!({})).unwrap();
        // Now all white.
        s.execute("edit.fill", json!({"color": "#ffffff"})).unwrap();
        s
    }

    fn pixel(s: &Session, x: i32, y: i32) -> Vec<f32> {
        let st = s.active().unwrap();
        st.doc.layer(st.active_layer.unwrap()).unwrap().surface().unwrap().pixel(x, y)
    }

    fn painted(s: &Session) -> (usize, usize) {
        let (mut red, mut blue) = (0, 0);
        for y in 0..80 {
            for x in 0..120 {
                let p = pixel(s, x, y);
                red += usize::from(p[0] > 0.6 && p[2] < 0.4 && x < 60);
                blue += usize::from(p[2] > 0.6 && p[0] < 0.4 && x >= 60);
            }
        }
        (red, blue)
    }

    #[test]
    fn strokes_take_the_source_colours_and_are_one_step() {
        let mut s = session();
        let snapshot = s.active().unwrap().snapshots.len() - 1;
        let steps = s.active().unwrap().history.entries().len();
        let r = s.execute(CMD, json!({"points": [[30, 40], [90, 40]], "size": 6, "area": 30, "snapshot": snapshot, "seed": 7})).unwrap();
        assert!(r["damage"][2].as_i64().unwrap() > 0);
        let (red, blue) = painted(&s);
        assert!(red > 50 && blue > 50, "red strokes on the left, blue on the right: {red} {blue}");
        // No stroke carries the wrong side's colour.
        for y in 0..80 {
            for x in 0..120 {
                let p = pixel(&s, x, y);
                assert!(!(x < 35 && p[2] > 0.6 && p[0] < 0.4), "blue at {x},{y}");
            }
        }
        let entries = s.active().unwrap().history.entries();
        assert_eq!((entries.len(), entries.last().map(String::as_str)), (steps + 1, Some("Art History Brush")));
    }

    #[test]
    fn the_same_seed_repeats_and_live_matches_the_commit_in_any_batches() {
        let pts: Vec<Value> = (0..40).map(|i| json!([10.0 + f64::from(i) * 2.5, 40.0 + (f64::from(i) * 0.3).sin() * 20.0, 1.0, 0, 0, 0, f64::from(i) * 16.0])).collect();
        let p = json!({"points": pts, "size": 5, "area": 20, "style": "looseCurl", "seed": 42, "snapshot": 1});
        let mut a = session();
        a.execute(CMD, p.clone()).unwrap();
        let mut b = session();
        let mut first = p.clone();
        first["points"] = json!([pts[0]]);
        let mut live = LiveArtHistory::begin(&b, &first).unwrap();
        let rest = crate::brush_cmds::parse_points(&json!({"points": pts[1..]}), CMD).unwrap();
        for chunk in rest.chunks(7) {
            live.push(chunk).unwrap();
        }
        b.execute(CMD, p).unwrap();
        let st = b.active().unwrap();
        let id = st.active_layer.unwrap();
        let shown = live.doc.layer(id).unwrap().surface().unwrap();
        for y in 0..80 {
            for x in 0..120 {
                assert_eq!(shown.pixel(x, y), pixel(&b, x, y), "live and commit differ at {x},{y}");
                assert_eq!(pixel(&a, x, y), pixel(&b, x, y), "the same seed repeats");
            }
        }
    }

    #[test]
    fn holding_still_keeps_painting_and_tolerance_spares_matching_areas() {
        let held: Vec<Value> = (0..10).map(|i| json!([30, 40, 1.0, 0, 0, 0, f64::from(i) * 16.0])).collect();
        let once = json!({"points": [[30, 40]], "size": 4, "area": 25, "style": "dab", "snapshot": 1});
        let mut a = session();
        a.execute(CMD, once).unwrap();
        let mut b = session();
        b.execute(CMD, json!({"points": held, "size": 4, "area": 25, "style": "dab", "snapshot": 1})).unwrap();
        assert!(painted(&b).0 > painted(&a).0, "a held pointer bursts again");
        // Painting from the current state at full tolerance: nothing differs, nothing is painted.
        let mut c = session();
        let now = c.active().unwrap().history.past_len();
        c.execute(CMD, json!({"points": [[30, 40], [90, 40]], "size": 6, "area": 30, "tolerance": 100, "state": now})).unwrap();
        assert_eq!(painted(&c), (0, 0));
        assert!(c.execute(CMD, json!({"points": [[1, 1]], "style": "wavy"})).is_err());
    }
}
