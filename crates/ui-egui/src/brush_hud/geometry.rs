//! Brush HUD wheel geometry, in "wheel units": the wheel is `U` units across, centred on the
//! origin; the view scales units to points. Angles are degrees, 0° at 12 o'clock, clockwise.
//!
//! The wheel is three orbits around the master hub:
//!
//! | radius       | holds                                                              |
//! |--------------|--------------------------------------------------------------------|
//! | `R_CENTER`   | the master hub (the brush in hand)                                 |
//! | `R_RECENT`   | the recent / favourite brushes arc over the master hub, its label  |
//! |              | under the hub at `RECENT_LABEL_DEG`                                |
//! | `R_HUB`      | Tier 1 category hubs, evenly spaced on one ring                    |
//! | `R_FAM`      | Tier 2 families of the active hub, its name in the gap at 12       |
//! | `R_VAR`      | Tier 3 variations of the open family, its header in the gap at 12  |
//! | `RING_OUTER` | the outer frame ring (and the search tracer's path)                |

use egui::{Vec2, vec2};

pub const U: f32 = 1400.0;
pub const R_CENTER: f32 = 100.0;
pub const R_RECENT: f32 = 146.0;
pub const RECENT_R: f32 = 32.0;
/// The recent arc's label sits on the arc's radius, under the master hub (the arc fills the
/// room above it).
pub const R_RECENT_LABEL: f32 = R_RECENT;
pub const RECENT_LABEL_DEG: f32 = 180.0;
/// The step between recent brushes along their arc.
pub const RECENT_STEP: f32 = 28.0;
pub const R_HUB: f32 = 272.0;
pub const HUB_R: f32 = 70.0;
pub const R_FAM: f32 = 414.0;
pub const R_VAR: f32 = 570.0;
/// Family and variation nodes.
pub const NODE_R: f32 = 41.0;
pub const RING_OUTER: f32 = 676.0;
/// The glowing divider between the centre (master hub, recent arc) and the hub orbit.
pub const RING_INNER: f32 = 192.0;
/// The glowing divider between the hub orbit and the family orbit.
pub const RING_MID: f32 = 356.0;
/// The glowing divider between the family orbit and the variation orbit.
pub const RING_FAM: f32 = 506.0;
/// The arc left free at 12 o'clock on the outer orbits, for their headers.
pub const ORBIT_GAP: f32 = 44.0;
/// The widest step between orbit nodes, so a few nodes gather at the bottom instead of
/// scattering around the ring.
pub const ORBIT_STEP_MAX: f32 = 34.0;
// The tiers never overlap: the recent label clears the hubs, the hubs clear the families, the
// families clear the variations, and the variations' labels stay inside the frame.
const _: () = assert!(R_HUB - HUB_R > R_RECENT + RECENT_R + 20.0 && R_RECENT > R_CENTER + RECENT_R + 12.0);
const _: () = assert!(R_FAM - NODE_R > R_HUB + HUB_R + 28.0 && R_VAR - NODE_R > R_FAM + NODE_R + 70.0 && R_VAR + NODE_R + 60.0 < RING_OUTER);
// The dividers run in the gaps between the tiers, touching no node.
const _: () = assert!(RING_INNER > R_RECENT + RECENT_R + 12.0 && RING_INNER < R_HUB - HUB_R - 4.0);
const _: () = assert!(RING_MID > R_HUB + HUB_R + 12.0 && RING_MID < R_FAM - NODE_R - 12.0);
const _: () = assert!(RING_FAM > R_FAM + NODE_R + 40.0 && RING_FAM < R_VAR - NODE_R - 12.0);

/// Most nodes on an orbit at once; more page behind a `+N` node.
pub const ORBIT_MAX: usize = 16;
/// Most families shown at once (fewer than variations: their orbit is shorter).
pub const FAM_MAX: usize = 12;
/// Most variations shown at once.
pub const VAR_MAX: usize = ORBIT_MAX;
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

/// An orbit of radius `r`: `n` nodes evenly spaced around the ring and centred on `center`
/// degrees (the hub or family they belong to), leaving a gap of at least `ORBIT_GAP` for the
/// header, which sits just before the first node (see [`header_deg`]).
pub fn orbit(n: usize, r: f32, center: f32) -> Vec<Slot> {
    if n == 0 {
        return Vec::new();
    }
    let step = if n > 1 { ((360.0 - ORBIT_GAP) / (n - 1) as f32).min(ORBIT_STEP_MAX) } else { 0.0 };
    let span = step * (n - 1) as f32;
    (0..n).map(|i| Slot { deg: norm(center - span / 2.0 + i as f32 * step), r }).collect()
}

/// Where an orbit's header goes: in the gap just before its first node (or at `center` when
/// the orbit is empty). On a full orbit that is the middle of the gap.
pub fn header_deg(slots: &[Slot], center: f32) -> f32 {
    slots.first().map_or(center, |s| norm(s.deg - ORBIT_GAP / 2.0))
}

/// The recent / favourites arc above the master hub.
pub fn recent_arc(n: usize) -> Vec<Slot> {
    let step = RECENT_STEP;
    let span = step * n.saturating_sub(1) as f32;
    (0..n).map(|i| Slot { deg: -span / 2.0 + i as f32 * step, r: R_RECENT }).collect()
}

/// Points along an arc (for painting), every ~3°.
pub fn arc_points(r: f32, a0: f32, a1: f32) -> Vec<Vec2> {
    let segs = (((a1 - a0).abs() / 3.0).ceil() as usize).clamp(2, 240);
    (0..=segs).map(|i| polar(a0 + (a1 - a0) * i as f32 / segs as f32, r)).collect()
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
    fn the_orbit_spreads_evenly_and_keeps_its_header_gap_free() {
        assert!(orbit(0, R_VAR, 0.0).is_empty());
        assert_eq!(orbit(1, R_VAR, 180.0)[0].deg, 180.0);
        let full = orbit(VAR_MAX, R_VAR, 180.0);
        assert_eq!(full.len(), VAR_MAX);
        assert!(full.iter().all(|s| s.deg >= ORBIT_GAP / 2.0 - 1e-3 && s.deg <= 360.0 - ORBIT_GAP / 2.0 + 1e-3), "{full:?}");
        let steps: Vec<f32> = full.windows(2).map(|w| w[1].deg - w[0].deg).collect();
        assert!(steps.iter().all(|d| (d - steps[0]).abs() < 1e-3), "evenly spaced");
        // Neighbours never touch, with room for a label between them.
        let gap = (full[1].offset() - full[0].offset()).length();
        assert!(gap > NODE_R * 4.5, "{gap}");
        let fams = orbit(FAM_MAX, R_FAM, 180.0);
        let gap = (fams[1].offset() - fams[0].offset()).length();
        assert!(gap > NODE_R * 4.5, "{gap}");
        let few = orbit(3, R_VAR, 180.0);
        assert!(((few[0].deg + few[2].deg) / 2.0 - 180.0).abs() < 1e-3 && few[1].deg - few[0].deg <= ORBIT_STEP_MAX);
    }

    #[test]
    fn an_orbit_gathers_by_its_hub_and_its_header_sits_in_the_free_gap() {
        // A few families of the hub at 12 o'clock gather at the top, around 0°.
        let few = orbit(3, R_FAM, 0.0);
        assert!(few.iter().all(|s| delta(0.0, s.deg).abs() <= ORBIT_STEP_MAX + 1e-3), "{few:?}");
        assert!((delta(0.0, few[1].deg)).abs() < 1e-3);
        // The header is next to the group, clear of every node.
        let h = header_deg(&few, 0.0);
        assert!(few.iter().all(|s| delta(h, s.deg).abs() >= ORBIT_GAP / 2.0 - 1e-3), "{h}");
        // A full orbit around the hub at 3 o'clock leaves its gap opposite, at 9 o'clock.
        let full = orbit(FAM_MAX, R_FAM, 90.0);
        let h = header_deg(&full, 90.0);
        assert!(delta(h, 270.0).abs() < 1e-2, "{h}");
        assert!(full.iter().all(|s| delta(h, s.deg).abs() >= ORBIT_GAP / 2.0 - 1e-3));
    }

    #[test]
    fn recent_brushes_never_touch_each_other_or_their_label() {
        let arc = recent_arc(RECENT_MAX);
        let gap = (arc[1].offset() - arc[0].offset()).length();
        assert!(gap > RECENT_R * 2.0 + 4.0, "{gap}");
        let label = polar(RECENT_LABEL_DEG, R_RECENT_LABEL);
        assert!(arc.iter().all(|s| (s.offset() - label).length() > RECENT_R + 40.0));
    }

    #[test]
    fn tracer_runs_around_the_ring_into_the_target() {
        let start = vec2(500.0, -900.0);
        let target = polar(200.0, R_VAR);
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
