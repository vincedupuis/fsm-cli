//! Diagram geometry shared with the editors (`center`, `polylineMid` and `n1` of fsm-editor-vscode/src/xmi.ts),
//! computed with JavaScript's arithmetic so that the same file gives the same numbers.

use crate::model::Point;

/// `Math.round(n * 10) / 10`: rounds to 0.1, halves towards +∞ as JavaScript does.
pub fn n1(n: f64) -> f64 {
    js_round(n * 10.0) / 10.0
}

/// `Math.round`: the nearest integer, halves towards +∞; NaN, infinities and -0 stay as they are.
pub fn js_round(x: f64) -> f64 {
    if !x.is_finite() || x == 0.0 {
        return x;
    }
    let f = x.floor();
    let r = if x - f >= 0.5 { f + 1.0 } else { f };
    // -0.5 <= x < 0 rounds to -0.
    if r == 0.0 && x < 0.0 { -0.0 } else { r }
}

/// `Math.hypot(a, b)` as V8 computes it (scaled by the largest value, Kahan summation), not the C library's.
pub fn hypot(a: f64, b: f64) -> f64 {
    if a.is_infinite() || b.is_infinite() {
        return f64::INFINITY;
    }
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    let values = [a.abs(), b.abs()];
    let max = values[0].max(values[1]);
    if max == 0.0 {
        return 0.0;
    }
    let mut sum = 0.0;
    let mut compensation = 0.0;
    for v in values {
        let n = v / max;
        let summand = n * n - compensation;
        let preliminary = sum + summand;
        compensation = (preliminary - sum) - summand;
        sum = preliminary;
    }
    sum.sqrt() * max
}

/// The center of a box.
pub fn center(x: f64, y: f64, w: f64, h: f64) -> Point {
    Point { x: x + w / 2.0, y: y + h / 2.0 }
}

/// The point halfway along a polyline: where a transition's label goes by default.
pub fn polyline_mid(pts: &[Point]) -> Point {
    let mut total = 0.0;
    for i in 1..pts.len() {
        total += hypot(pts[i].x - pts[i - 1].x, pts[i].y - pts[i - 1].y);
    }
    let mut half = total / 2.0;
    for i in 1..pts.len() {
        let (a, b) = (pts[i - 1], pts[i]);
        let len = hypot(b.x - a.x, b.y - a.y);
        if len >= half && len > 0.0 {
            return Point { x: a.x + ((b.x - a.x) * half) / len, y: a.y + ((b.y - a.y) * half) / len };
        }
        half -= len;
    }
    pts[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_as_javascript() {
        assert_eq!(n1(12.25), 12.3);
        assert_eq!(n1(-12.25), -12.2);
        assert_eq!(js_round(-0.5).to_bits(), (-0.0f64).to_bits());
        assert_eq!(js_round(2.5), 3.0);
        assert!(n1(f64::NAN).is_nan());
    }

    #[test]
    fn middle_of_a_polyline() {
        let p = |x, y| Point { x, y };
        assert_eq!(polyline_mid(&[p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0)]), p(10.0, 0.0));
        assert_eq!(polyline_mid(&[p(0.0, 0.0), p(0.0, 30.0), p(10.0, 30.0)]), p(0.0, 20.0));
        assert_eq!(polyline_mid(&[p(5.0, 5.0), p(5.0, 5.0)]), p(5.0, 5.0));
        assert_eq!(hypot(3.0, 4.0), 5.0);
    }
}
