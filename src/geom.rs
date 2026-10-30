//! Small 2D geometry toolkit. All coordinates are in embroidery units (0.1 mm),
//! with +y pointing down (the same convention as SVG and PES).

use std::ops::{Add, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct P {
    pub x: f64,
    pub y: f64,
}

impl P {
    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    #[inline]
    pub fn dot(self, o: P) -> f64 {
        self.x * o.x + self.y * o.y
    }
    #[inline]
    pub fn len(self) -> f64 {
        self.x.hypot(self.y)
    }
    #[inline]
    pub fn dist(self, o: P) -> f64 {
        (self - o).len()
    }
    #[inline]
    pub fn norm(self) -> P {
        let l = self.len();
        if l > 1e-12 {
            self * (1.0 / l)
        } else {
            P::new(0.0, 0.0)
        }
    }
    /// Perpendicular (rotated 90° counter-clockwise in y-down space).
    #[inline]
    pub fn perp(self) -> P {
        P::new(-self.y, self.x)
    }
    #[inline]
    pub fn rotate(self, cos: f64, sin: f64) -> P {
        P::new(self.x * cos - self.y * sin, self.x * sin + self.y * cos)
    }
    #[inline]
    pub fn lerp(self, o: P, t: f64) -> P {
        self + (o - self) * t
    }
}

impl Add for P {
    type Output = P;
    #[inline]
    fn add(self, o: P) -> P {
        P::new(self.x + o.x, self.y + o.y)
    }
}
impl Sub for P {
    type Output = P;
    #[inline]
    fn sub(self, o: P) -> P {
        P::new(self.x - o.x, self.y - o.y)
    }
}
impl Mul<f64> for P {
    type Output = P;
    #[inline]
    fn mul(self, s: f64) -> P {
        P::new(self.x * s, self.y * s)
    }
}
impl Neg for P {
    type Output = P;
    #[inline]
    fn neg(self) -> P {
        P::new(-self.x, -self.y)
    }
}

pub type Polyline = Vec<P>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillRule {
    NonZero,
    EvenOdd,
}

impl FillRule {
    #[inline]
    pub fn inside(self, winding: i32) -> bool {
        match self {
            FillRule::NonZero => winding != 0,
            FillRule::EvenOdd => winding & 1 == 1,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub min: P,
    pub max: P,
}

impl Bounds {
    pub fn empty() -> Self {
        Bounds {
            min: P::new(f64::INFINITY, f64::INFINITY),
            max: P::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
        }
    }
    #[inline]
    pub fn add(&mut self, p: P) {
        self.min.x = self.min.x.min(p.x);
        self.min.y = self.min.y.min(p.y);
        self.max.x = self.max.x.max(p.x);
        self.max.y = self.max.y.max(p.y);
    }
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x
    }
    pub fn width(&self) -> f64 {
        if self.is_empty() {
            0.0
        } else {
            self.max.x - self.min.x
        }
    }
    pub fn height(&self) -> f64 {
        if self.is_empty() {
            0.0
        } else {
            self.max.y - self.min.y
        }
    }
    pub fn center(&self) -> P {
        if self.is_empty() {
            P::default()
        } else {
            self.min.lerp(self.max, 0.5)
        }
    }
}

/// Winding number of `p` with respect to closed `rings`.
pub fn winding(rings: &[Polyline], p: P) -> i32 {
    let mut w = 0;
    for ring in rings {
        let n = ring.len();
        if n < 3 {
            continue;
        }
        let mut a = ring[n - 1];
        for &b in ring {
            if a.y <= p.y {
                if b.y > p.y && cross(b - a, p - a) > 0.0 {
                    w += 1;
                }
            } else if b.y <= p.y && cross(b - a, p - a) < 0.0 {
                w -= 1;
            }
            a = b;
        }
    }
    w
}

#[inline]
pub fn cross(a: P, b: P) -> f64 {
    a.x * b.y - a.y * b.x
}

pub fn point_inside(rings: &[Polyline], rule: FillRule, p: P) -> bool {
    rule.inside(winding(rings, p))
}

/// True if the straight segment a→b stays inside the shape (sampled every `step`).
pub fn segment_inside(rings: &[Polyline], rule: FillRule, a: P, b: P, step: f64) -> bool {
    let d = a.dist(b);
    let n = (d / step).ceil().max(1.0) as usize;
    (1..n).all(|i| point_inside(rings, rule, a.lerp(b, i as f64 / n as f64)))
}

pub fn polyline_length(pts: &[P]) -> f64 {
    pts.windows(2).map(|w| w[0].dist(w[1])).sum()
}

/// Flatten a quadratic Bézier into `out` (excluding the start point).
pub fn flatten_quad(p0: P, p1: P, p2: P, tol: f64, out: &mut Polyline) {
    // Wang's formula for degree 2.
    let dd = (p0 - p1 * 2.0 + p2).len();
    let n = ((0.25 * dd / tol).sqrt().ceil() as usize).clamp(1, 512);
    for i in 1..=n {
        let t = i as f64 / n as f64;
        let mt = 1.0 - t;
        out.push(p0 * (mt * mt) + p1 * (2.0 * mt * t) + p2 * (t * t));
    }
}

/// Flatten a cubic Bézier into `out` (excluding the start point).
pub fn flatten_cubic(p0: P, p1: P, p2: P, p3: P, tol: f64, out: &mut Polyline) {
    // Wang's formula for degree 3.
    let dd = (p0 - p1 * 2.0 + p2).len().max((p1 - p2 * 2.0 + p3).len());
    let n = ((0.75 * dd / tol).sqrt().ceil() as usize).clamp(1, 1024);
    for i in 1..=n {
        let t = i as f64 / n as f64;
        let mt = 1.0 - t;
        out.push(
            p0 * (mt * mt * mt)
                + p1 * (3.0 * mt * mt * t)
                + p2 * (3.0 * mt * t * t)
                + p3 * (t * t * t),
        );
    }
}

/// Remove consecutive points closer than `min` (keeps first and last).
pub fn dedupe(pts: &mut Polyline, min: f64) {
    if pts.len() < 3 {
        return;
    }
    let last = *pts.last().unwrap();
    let mut out: Polyline = Vec::with_capacity(pts.len());
    out.push(pts[0]);
    for &p in &pts[1..pts.len() - 1] {
        if out.last().unwrap().dist(p) >= min {
            out.push(p);
        }
    }
    if out.len() > 1 && out.last().unwrap().dist(last) < min {
        out.pop();
    }
    if out.last().unwrap().dist(last) > 1e-9 {
        out.push(last);
    }
    *pts = out;
}

/// Resample a polyline into stitches no longer than `len`, keeping every vertex
/// (corners stay sharp). Segments are split evenly.
pub fn resample_keep_corners(pts: &[P], len: f64) -> Polyline {
    let mut out = Vec::with_capacity(pts.len() * 2);
    if let Some(&first) = pts.first() {
        out.push(first);
    }
    for w in pts.windows(2) {
        let d = w[0].dist(w[1]);
        if d < 1e-9 {
            continue;
        }
        let n = (d / len).ceil().max(1.0) as usize;
        for i in 1..=n {
            out.push(w[0].lerp(w[1], i as f64 / n as f64));
        }
    }
    out
}

/// Points along a polyline at a fixed arc-length spacing, with unit tangents.
pub fn sample_by_arclength(pts: &[P], spacing: f64) -> Vec<(P, P)> {
    let total = polyline_length(pts);
    if total < 1e-9 || pts.len() < 2 {
        return Vec::new();
    }
    let n = (total / spacing).round().max(1.0) as usize;
    let step = total / n as f64;
    let mut out = Vec::with_capacity(n + 1);
    let mut seg = 0usize;
    let mut seg_start = 0.0;
    let mut seg_len = pts[0].dist(pts[1]);
    for i in 0..=n {
        let s = (i as f64 * step).min(total);
        while seg + 2 < pts.len() && s > seg_start + seg_len {
            seg_start += seg_len;
            seg += 1;
            seg_len = pts[seg].dist(pts[seg + 1]);
        }
        let t = if seg_len > 1e-12 {
            ((s - seg_start) / seg_len).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let p = pts[seg].lerp(pts[seg + 1], t);
        out.push((p, (pts[seg + 1] - pts[seg]).norm()));
    }
    out
}

/// 2D affine transform `[a b c d e f]` in SVG order: x' = a·x + c·y + e, y' = b·x + d·y + f.
pub type Affine = [f64; 6];

pub const IDENTITY: Affine = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `a · b`: apply `b` first, then `a`.
pub fn affine_mul(a: Affine, b: Affine) -> Affine {
    [
        a[0] * b[0] + a[2] * b[1],
        a[1] * b[0] + a[3] * b[1],
        a[0] * b[2] + a[2] * b[3],
        a[1] * b[2] + a[3] * b[3],
        a[0] * b[4] + a[2] * b[5] + a[4],
        a[1] * b[4] + a[3] * b[5] + a[5],
    ]
}

pub fn affine_inv(m: Affine) -> Option<Affine> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 {
        return None;
    }
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    Some([a, b, c, d, -(a * m[4] + c * m[5]), -(b * m[4] + d * m[5])])
}

pub fn affine_apply(m: Affine, p: P) -> P {
    P::new(
        m[0] * p.x + m[2] * p.y + m[4],
        m[1] * p.x + m[3] * p.y + m[5],
    )
}

pub fn translation(d: P) -> Affine {
    [1.0, 0.0, 0.0, 1.0, d.x, d.y]
}

/// Scale by (sx, sy) around `c`.
pub fn scale_about(c: P, sx: f64, sy: f64) -> Affine {
    [sx, 0.0, 0.0, sy, c.x - sx * c.x, c.y - sy * c.y]
}

/// Rotate by `deg` (clockwise on screen, since +y is down) around `c`.
pub fn rotate_about(c: P, deg: f64) -> Affine {
    let (s, co) = deg.to_radians().sin_cos();
    [
        co,
        s,
        -s,
        co,
        c.x - co * c.x + s * c.y,
        c.y - s * c.x - co * c.y,
    ]
}
