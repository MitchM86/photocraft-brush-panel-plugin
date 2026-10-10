//! Response curves: how an input in 0..1 (pen pressure) maps to an output in 0..1, drawn as
//! control points. One implementation serves Preferences › Tools › Pressure Curve (the pen's
//! global response, re-exported by the engine as `prefs::PressureCurve`) and the brush's own
//! per-setting curves ([`crate::PressureResponse`]: size, opacity and flow each get their own,
//! as in Krita's brush options).

use serde::{Deserialize, Serialize};

/// A monotone cubic (Fritsch–Carlson) through control points, so more input never gives less
/// output. The points are sanitised here, not trusted: a command, a preset file or a hand-edited
/// preferences file can store anything. Non-finite points are dropped, the rest clamped to 0..1,
/// sorted by input (one point per input), capped at [`Self::MAX_POINTS`], and their outputs made
/// non-decreasing; before the first point and after the last the curve stays flat. Fewer than two
/// usable points is linear.
///
/// Serialises as its (sanitised) points, `[[x, y], …]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "Vec<[f32; 2]>", into = "Vec<[f32; 2]>")]
pub struct ResponseCurve {
    xs: Vec<f32>,
    ys: Vec<f32>,
    /// Tangents at the points.
    ms: Vec<f32>,
    /// Second derivatives of the natural cubic spline through the points ([`Self::eval_natural`]).
    cs: Vec<f32>,
}

impl Default for ResponseCurve {
    fn default() -> Self {
        Self::linear()
    }
}

impl From<Vec<[f32; 2]>> for ResponseCurve {
    fn from(points: Vec<[f32; 2]>) -> Self {
        Self::new(&points)
    }
}

impl From<ResponseCurve> for Vec<[f32; 2]> {
    fn from(c: ResponseCurve) -> Self {
        c.points()
    }
}

impl ResponseCurve {
    pub const MAX_POINTS: usize = 16;

    /// The identity: the input passes through unchanged.
    pub fn linear() -> Self {
        Self { xs: vec![0.0, 1.0], ys: vec![0.0, 1.0], ms: vec![1.0, 1.0], cs: vec![0.0, 0.0] }
    }

    pub fn new(points: &[[f32; 2]]) -> Self {
        let mut pts: Vec<[f32; 2]> =
            points.iter().filter(|p| p[0].is_finite() && p[1].is_finite()).map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)]).collect();
        pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
        pts.dedup_by(|b, a| {
            // Same input: keep the later point's output.
            let same = (a[0] - b[0]).abs() < 1e-6;
            if same {
                a[1] = b[1];
            }
            same
        });
        pts.truncate(Self::MAX_POINTS);
        if pts.len() < 2 {
            return Self::linear();
        }
        let xs: Vec<f32> = pts.iter().map(|p| p[0]).collect();
        let mut ys: Vec<f32> = pts.iter().map(|p| p[1]).collect();
        for i in 1..ys.len() {
            ys[i] = ys[i].max(ys[i - 1]);
        }
        // Fritsch–Carlson tangents: secant averages, zero at flat steps, limited so the cubic
        // never overshoots (which keeps it monotone).
        let n = xs.len();
        let d: Vec<f32> = (0..n - 1).map(|k| (ys[k + 1] - ys[k]) / (xs[k + 1] - xs[k])).collect();
        let mut ms = vec![0.0f32; n];
        ms[0] = d[0];
        ms[n - 1] = d[n - 2];
        for k in 1..n - 1 {
            ms[k] = if d[k - 1] * d[k] <= 0.0 { 0.0 } else { (d[k - 1] + d[k]) / 2.0 };
        }
        for k in 0..n - 1 {
            if d[k] == 0.0 {
                ms[k] = 0.0;
                ms[k + 1] = 0.0;
                continue;
            }
            let (a, b) = (ms[k] / d[k], ms[k + 1] / d[k]);
            let s = a * a + b * b;
            if s > 9.0 {
                let t = 3.0 / s.sqrt();
                ms[k] = t * a * d[k];
                ms[k + 1] = t * b * d[k];
            }
        }
        let cs = natural_second_derivatives(&xs, &ys);
        Self { xs, ys, ms, cs }
    }

    /// The curve's output for `x` (0..1; anything else is clamped, NaN reads as 0).
    pub fn eval(&self, x: f32) -> f32 {
        let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
        let n = self.xs.len();
        let (Some(&x0), Some(&xn)) = (self.xs.first(), self.xs.last()) else { return x };
        if x <= x0 {
            return self.ys.first().copied().unwrap_or(x);
        }
        if x >= xn {
            return self.ys.last().copied().unwrap_or(x);
        }
        let k = self.xs.partition_point(|&v| v <= x).saturating_sub(1).min(n.saturating_sub(2));
        let (Some(&xa), Some(&xb), Some(&ya), Some(&yb), Some(&ma), Some(&mb)) =
            (self.xs.get(k), self.xs.get(k + 1), self.ys.get(k), self.ys.get(k + 1), self.ms.get(k), self.ms.get(k + 1))
        else {
            return x;
        };
        let h = xb - xa;
        let t = (x - xa) / h;
        let (t2, t3) = (t * t, t * t * t);
        let y = (2.0 * t3 - 3.0 * t2 + 1.0) * ya + (t3 - 2.0 * t2 + t) * h * ma + (-2.0 * t3 + 3.0 * t2) * yb + (t3 - t2) * h * mb;
        y.clamp(ya.min(yb), ya.max(yb))
    }

    /// Krita's curve shape (`KisCubicSpline`): the natural cubic spline through the points (no
    /// curvature at the ends), clamped to 0..1. Unlike [`Self::eval`] it can bend below a straight
    /// line between points, as Krita's brush curves do.
    pub fn eval_natural(&self, x: f32) -> f32 {
        let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
        let n = self.xs.len();
        let (Some(&x0), Some(&xn)) = (self.xs.first(), self.xs.last()) else { return x };
        let x = x.clamp(x0, xn);
        let k = self.xs.partition_point(|&v| v <= x).saturating_sub(1).min(n.saturating_sub(2));
        let (Some(&xa), Some(&xb), Some(&ya), Some(&yb), Some(&ca), Some(&cb)) =
            (self.xs.get(k), self.xs.get(k + 1), self.ys.get(k), self.ys.get(k + 1), self.cs.get(k), self.cs.get(k + 1))
        else {
            return x;
        };
        let h = xb - xa;
        if h.is_nan() || h <= 0.0 {
            return ya;
        }
        let (u, v) = (xb - x, x - xa);
        let y = ca * u * u * u / (6.0 * h) + cb * v * v * v / (6.0 * h) + (ya / h - ca * h / 6.0) * u + (yb / h - cb * h / 6.0) * v;
        if y.is_finite() { y.clamp(0.0, 1.0) } else { x }
    }

    /// The identity (the default curve): the input passes through unchanged.
    pub fn is_linear(&self) -> bool {
        self.xs == [0.0, 1.0] && self.ys == [0.0, 1.0]
    }

    /// The sanitised control points.
    pub fn points(&self) -> Vec<[f32; 2]> {
        self.xs.iter().zip(&self.ys).map(|(x, y)| [*x, *y]).collect()
    }
}

/// Second derivatives at the knots of the natural cubic spline through `xs`, `ys` (zero at both
/// ends): the tridiagonal system Krita's `KisCubicSpline` solves, by the Thomas algorithm.
fn natural_second_derivatives(xs: &[f32], ys: &[f32]) -> Vec<f32> {
    let n = xs.len();
    let mut c = vec![0.0f32; n];
    if n < 3 {
        return c;
    }
    let h: Vec<f32> = xs.windows(2).map(|w| w[1] - w[0]).collect();
    let m = n - 2;
    let (mut diag, mut rhs) = (vec![0.0f32; m], vec![0.0f32; m]);
    for i in 0..m {
        diag[i] = 2.0 * (h[i] + h[i + 1]);
        rhs[i] = 6.0 * ((ys[i + 2] - ys[i + 1]) / h[i + 1] - (ys[i + 1] - ys[i]) / h[i]);
    }
    // Forward sweep (off-diagonals h[i + 1]), then back substitution.
    for i in 1..m {
        let w = h[i] / diag[i - 1];
        diag[i] -= w * h[i];
        rhs[i] -= w * rhs[i - 1];
    }
    let mut sol = vec![0.0f32; m];
    for i in (0..m).rev() {
        let next = if i + 1 < m { h[i + 1] * sol[i + 1] } else { 0.0 };
        sol[i] = (rhs[i] - next) / diag[i];
    }
    for (i, v) in sol.into_iter().enumerate() {
        c[i + 1] = if v.is_finite() { v } else { 0.0 };
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialises_as_its_sanitised_points() {
        let c = ResponseCurve::new(&[[1.0, 1.0], [0.35, 0.1], [0.0, 0.0], [f32::NAN, 0.5]]);
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v, serde_json::json!([[0.0, 0.0], [0.35f32, 0.1f32], [1.0, 1.0]]));
        let back: ResponseCurve = serde_json::from_value(v).unwrap();
        assert_eq!(back, c);
        // Hostile input deserialises to something valid, never an error or a panic.
        let bad: ResponseCurve = serde_json::from_value(serde_json::json!([[2.0, -1.0]])).unwrap();
        assert!(bad.is_linear());
        assert!(ResponseCurve::default().is_linear());
        assert_eq!(ResponseCurve::linear(), ResponseCurve::new(&[[0.0, 0.0], [1.0, 1.0]]));
    }

    #[test]
    fn a_krita_style_size_curve_is_monotone_and_hits_its_points() {
        let c = ResponseCurve::new(&[[0.0, 0.0], [0.35, 0.1], [1.0, 1.0]]);
        assert!((c.eval(0.35) - 0.1).abs() < 1e-6);
        assert_eq!((c.eval(0.0), c.eval(1.0)), (0.0, 1.0));
        let mut last = 0.0;
        for i in 0..=1000 {
            let y = c.eval(i as f32 / 1000.0);
            assert!(y >= last - 1e-6 && (0.0..=1.0).contains(&y), "{i}: {y}");
            last = y;
        }
        // Light pressure stays thin, then the curve climbs fast: twice the pressure in the
        // pen's light range gives well over twice the output.
        assert!(c.eval(0.4) > 2.5 * c.eval(0.2), "{} vs {}", c.eval(0.4), c.eval(0.2));
    }

    #[test]
    fn the_natural_spline_matches_kritas_curve() {
        // Krita's Basic-5 Size curve, solved by hand: S(x) = 1.5699 x³ + 0.0934 x on 0..0.35.
        let c = ResponseCurve::new(&[[0.0, 0.0], [0.35, 0.1], [1.0, 1.0]]);
        for x in [0.05f32, 0.1, 0.2, 0.3] {
            let want = 1.5699 * x * x * x + 0.09341 * x;
            assert!((c.eval_natural(x) - want).abs() < 1e-4, "{x}: {} vs {want}", c.eval_natural(x));
        }
        assert!((c.eval_natural(0.35) - 0.1).abs() < 1e-6 && (c.eval_natural(1.0) - 1.0).abs() < 1e-6);
        // Krita's shape stays lower under light pressure than the monotone one.
        assert!(c.eval_natural(0.1) < 0.7 * c.eval(0.1), "{} vs {}", c.eval_natural(0.1), c.eval(0.1));
        // Two points: a straight line; out-of-range and NaN input never panic.
        assert!((ResponseCurve::linear().eval_natural(0.3) - 0.3).abs() < 1e-6);
        assert!(c.eval_natural(f32::NAN) == 0.0 && c.eval_natural(5.0) == 1.0);
    }
}
