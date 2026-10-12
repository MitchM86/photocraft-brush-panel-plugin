//! Brush HUD wheel geometry, in "wheel units": the wheel is `U` units across, centred on the
//! origin; the view scales units to points. Angles are degrees, 0° at 12 o'clock, clockwise.
//!
//! | radius            | holds                                   |
//! |-------------------|-----------------------------------------|
//! | `R_CENTER`        | the master hub (the brush in hand)      |
//! | `R_RECENT`        | the recent / favourite brushes arc      |
//! | `R_HUB`           | Tier 1 category hubs                    |
//! | `R_FAM_IN/OUT`    | Tier 2 families, staggered in a fan     |
//! | `R_SPINE`         | Tier 3 variations along the outer spine |

use egui::{Vec2, vec2};

pub const U: f32 = 1400.0;
pub const R_CENTER: f32 = 92.0;
pub const R_RECENT: f32 = 176.0;
pub const RECENT_R: f32 = 19.0;
pub const R_HUB: f32 = 286.0;
pub const HUB_R: f32 = 74.0;
pub const RING_INNER: f32 = 214.0;
pub const RING_HUB_OUT: f32 = 370.0;
pub const R_FAM_IN: f32 = 414.0;
pub const R_FAM_OUT: f32 = 478.0;
pub const FAM_R: f32 = 36.0;
pub const RING_MID: f32 = 530.0;
pub const R_SPINE: f32 = 590.0;
pub const VAR_R: f32 = 23.0;
pub const RING_OUTER: f32 = 664.0;
// The tiers never overlap.
const _: () = assert!(R_SPINE > RING_MID && RING_MID > R_FAM_OUT && R_FAM_IN > RING_HUB_OUT && RING_INNER > R_RECENT);

/// Most families shown at once; more page behind a `+N` node.
pub const FAM_MAX: usize = 16;
/// Most variations shown at once on the spine.
pub const VAR_MAX: usize = 22;
/// Most brushes on the recent / favourites arc.
pub const RECENT_MAX: usize = 7;

/// Offset of the point at `deg` and radius `r` from the wheel centre.
pub fn polar(deg: f32, r: f32) -> Vec2 {
    let a = deg.to_radians();
    vec2(r * a.sin(), -r * a.cos())
}

pub fn norm(deg: f32) -> f32 {
    deg.rem_euclid(360.0)
}

/// Angle of an offset from the centre (0° up, clockwise).
pub fn angle_of(v: Vec2) -> f32 {
    norm(v.x.atan2(-v.y).to_degrees())
}

/// Signed shortest turn from `a` to `b`, in (−180, 180].
pub fn delta(a: f32, b: f32) -> f32 {
    let d = norm(b - a);
    if d > 180.0 { d - 360.0 } else { d }
}

/// Hub angles for `n` categories, evenly spaced from 12 o'clock.
pub fn hub_angles(n: usize) -> Vec<f32> {
    (0..n).map(|i| i as f32 * 360.0 / n.max(1) as f32).collect()
}

/// A node on the wheel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    pub deg: f32,
    pub r: f32,
}

impl Slot {
    pub fn offset(self) -> Vec2 {
        polar(self.deg, self.r)
    }
}

/// Tier 2: a fan of `m` families centred on the active hub, on alternating radii.
pub fn family_fan(center: f32, m: usize) -> (Vec<Slot>, f32, f32) {
    if m == 0 {
        return (Vec::new(), center, center);
    }
    let step = if m > 1 { (168.0 / (m - 1) as f32).min(12.5) } else { 0.0 };
    let span = step * (m.saturating_sub(1)) as f32;
    let slots = (0..m)
        .map(|i| {
            let r = if m > 3 { if i % 2 == 1 { R_FAM_OUT } else { R_FAM_IN } } else { (R_FAM_IN + R_FAM_OUT) / 2.0 };
            Slot { deg: center - span / 2.0 + i as f32 * step, r }
        })
        .collect();
    (slots, center - span / 2.0 - 8.0, center + span / 2.0 + 8.0)
}

/// Where a variation's label goes relative to its node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelSide {
    Left,
    Right,
    Above,
    Below,
}

/// Tier 3: `n` variations along the outer spine, centred on the family's angle. Near 12 and 6
/// o'clock, side labels would collide with the neighbours, so labels go above/below there,
/// alternating between the outer and inner side of the spine.
pub fn spine(center: f32, n: usize) -> (Vec<(Slot, LabelSide)>, f32, f32) {
    if n == 0 {
        return (Vec::new(), center, center);
    }
    let step = if n > 1 { (200.0 / (n - 1) as f32).min(8.6) } else { 0.0 };
    let span = step * (n.saturating_sub(1)) as f32;
    let nodes = (0..n)
        .map(|i| {
            let deg = center - span / 2.0 + i as f32 * step;
            let c = deg.to_radians().cos();
            let (outward, inward) = if c > 0.0 { (LabelSide::Above, LabelSide::Below) } else { (LabelSide::Below, LabelSide::Above) };
            let side = if c.abs() > 0.6 {
                if i % 2 == 1 { inward } else { outward }
            } else if norm(deg) > 180.0 {
                LabelSide::Left
            } else {
                LabelSide::Right
            };
            (Slot { deg, r: R_SPINE }, side)
        })
        .collect();
    (nodes, center - span / 2.0, center + span / 2.0)
}

/// The recent / favourites arc above the master hub.
pub fn recent_arc(n: usize) -> Vec<Slot> {
    let step = 17.0;
    let span = step * n.saturating_sub(1) as f32;
    (0..n).map(|i| Slot { deg: -span / 2.0 + i as f32 * step, r: R_RECENT }).collect()
}

/// Points along an arc (for painting), every ~3°.
pub fn arc_points(r: f32, a0: f32, a1: f32) -> Vec<Vec2> {
    let segs = (((a1 - a0).abs() / 3.0).ceil() as usize).clamp(2, 240);
    (0..=segs).map(|i| polar(a0 + (a1 - a0) * i as f32 / segs as f32, r)).collect()
}

/// An annular wedge between radii `r0..r1` and angles `a0..a1`, as a closed polygon.
pub fn wedge(r0: f32, r1: f32, a0: f32, a1: f32) -> Vec<Vec2> {
    let mut pts = arc_points(r1, a0, a1);
    pts.extend(arc_points(r0, a1, a0));
    pts
}

/// The search tracer: from `start` (outside the wheel) to the ring of radius `r`, around the
/// ring the short way to the target's angle, then straight in to `target`, stopping `stop`
/// short of it. All offsets from the wheel centre.
pub fn tracer(start: Vec2, target: Vec2, r: f32, stop: f32) -> Vec<Vec2> {
    let a0 = angle_of(start);
    let a1 = angle_of(target);
    let d = delta(a0, a1);
    let mut pts = vec![start];
    if start.length() > r {
        pts.push(polar(a0, r));
    }
    let segs = ((d.abs() / 3.0).ceil() as usize).max(2);
    pts.extend((1..=segs).map(|i| polar(a0 + d * i as f32 / segs as f32, r)));
    let last = pts.last().copied().unwrap_or(start);
    let to = target - last;
    let len = to.length();
    if len > 1e-3 {
        pts.push(last + to * ((len - stop).max(0.0) / len));
    }
    pts
}

/// The prefix of a polyline covering `frac` (0..1) of its length (the tracer's draw-in).
pub fn polyline_prefix(pts: &[Vec2], frac: f32) -> Vec<Vec2> {
    let total: f32 = pts.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    let mut left = total * frac.clamp(0.0, 1.0);
    let mut out = Vec::with_capacity(pts.len());
    for w in pts.windows(2) {
        if out.is_empty() {
            out.push(w[0]);
        }
        let seg = (w[1] - w[0]).length();
        if seg >= left {
            if seg > 0.0 {
                out.push(w[0] + (w[1] - w[0]) * (left / seg));
            }
            return out;
        }
        left -= seg;
        out.push(w[1]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polar_and_angle_agree() {
        for deg in [0.0, 45.0, 135.0, 200.0, 359.0] {
            let v = polar(deg, 300.0);
            assert!((angle_of(v) - deg).abs() < 1e-3, "{deg}");
        }
        assert!((polar(0.0, 10.0).y + 10.0).abs() < 1e-5, "0° points up");
        assert!((polar(90.0, 10.0).x - 10.0).abs() < 1e-5, "90° points right");
        assert_eq!(delta(350.0, 10.0), 20.0);
        assert_eq!(delta(10.0, 350.0), -20.0);
    }

    #[test]
    fn fans_and_spines_centre_on_their_anchor() {
        let (f, a0, a1) = family_fan(90.0, 5);
        assert_eq!(f.len(), 5);
        assert!((f[2].deg - 90.0).abs() < 1e-4);
        assert!(a0 < 90.0 && a1 > 90.0);
        let (s, b0, b1) = spine(270.0, 11);
        assert_eq!(s.len(), 11);
        assert!(((b0 + b1) / 2.0 - 270.0).abs() < 1e-3);
        let (side, _, _) = spine(270.0, 3);
        assert!(side.iter().all(|(_, l)| *l == LabelSide::Left), "the left side reads leftwards");
        let (bottom, _, _) = spine(180.0, 6);
        assert_eq!(bottom[0].1, LabelSide::Below);
        assert_eq!(bottom[1].1, LabelSide::Above, "alternates to the inner side");
        assert!(family_fan(0.0, 0).0.is_empty() && spine(0.0, 0).0.is_empty());
    }

    #[test]
    fn tracer_runs_around_the_ring_into_the_target() {
        let start = vec2(500.0, -900.0);
        let target = polar(200.0, R_SPINE);
        let pts = tracer(start, target, RING_OUTER, 10.0);
        assert_eq!(pts[0], start);
        assert!((pts[1].length() - RING_OUTER).abs() < 1e-2);
        let end = *pts.last().unwrap();
        assert!(((end - target).length() - 10.0).abs() < 0.5);
        let half = polyline_prefix(&pts, 0.5);
        assert!(half.len() < pts.len());
        assert_eq!(polyline_prefix(&pts, 1.0).len(), pts.len());
    }
}
