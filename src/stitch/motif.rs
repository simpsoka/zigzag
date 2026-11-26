//! Motifs: a small drawing, a few open lines, repeated over a shape on a
//! tile grid. It's sewn one of two ways, both as embroidery software does
//! them (there's no library: a motif is drawn by the user, or an agent):
//!
//! - **Patterned fill** (Wilcom's Program Split, Embrilliance's emboss
//!   patterns, Brother's programmable fill): the shape is an ordinary
//!   tatami, full coverage, and wherever a row crosses one of the motif's
//!   lines the needle goes down there. The motif shows as a texture of
//!   stitch ends. See [`Motif::crossings`] and `fill::tatami`.
//! - **Motif fill** (Wilcom's and Brother's motif fills): copies of the
//!   motif, a running-stitch line from its first point to its last, are
//!   chained into rows and sewn as open running stitch. See [`motif_fill`].
//!
//! The tile layout (grid, brick, hex) is VectorCraft's pattern tiling
//! (`crates/doc/src/pattern.rs`, `tile_offset`; MIT OR Apache-2.0).

use crate::geom::{FillRule, P, Polyline};
use crate::stitch::patterns::clip_inside;
use crate::stitch::stroke::running;

/// How the tiles are laid out: VectorCraft's (and Illustrator's) tile types.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    /// Rows and columns.
    Grid,
    /// Every other row shifted half a tile across.
    #[default]
    Brick,
    /// Every other column shifted half a tile down.
    BrickColumn,
    /// Hexagonal tiles in rows: rows ¾ of a tile apart, every other one
    /// shifted half a tile across.
    Hex,
    /// Hexagonal tiles in columns.
    HexColumn,
}

impl Layout {
    pub const ALL: [Layout; 5] = [
        Layout::Grid,
        Layout::Brick,
        Layout::BrickColumn,
        Layout::Hex,
        Layout::HexColumn,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Layout::Grid => "grid",
            Layout::Brick => "brick",
            Layout::BrickColumn => "brickcol",
            Layout::Hex => "hex",
            Layout::HexColumn => "hexcol",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Layout::Grid => "Grid",
            Layout::Brick => "Brick",
            Layout::BrickColumn => "Brick (columns)",
            Layout::Hex => "Hex",
            Layout::HexColumn => "Hex (columns)",
        }
    }
    pub fn parse(s: &str) -> Option<Layout> {
        Some(match s.replace(['-', ' '], "").as_str() {
            "grid" => Layout::Grid,
            "brick" | "brickrow" | "brickbyrow" => Layout::Brick,
            "brickcol" | "brickcolumn" | "brickbycolumn" => Layout::BrickColumn,
            "hex" | "hexrow" | "hexbyrow" => Layout::Hex,
            "hexcol" | "hexcolumn" | "hexbycolumn" => Layout::HexColumn,
            _ => return None,
        })
    }

    /// Where tile (column `i`, row `j`) sits, for a `w`×`h` tile; (0, 0)
    /// at the origin. VectorCraft's `tile_offset`, its bricks half a tile.
    pub fn offset(self, w: f64, h: f64, i: i64, j: i64) -> P {
        let (fi, fj) = (i as f64, j as f64);
        let odd = |n: i64| if n.rem_euclid(2) == 1 { 0.5 } else { 0.0 };
        match self {
            Layout::Grid => P::new(fi * w, fj * h),
            Layout::Brick => P::new((fi + odd(j)) * w, fj * h),
            Layout::BrickColumn => P::new(fi * w, (fj + odd(i)) * h),
            Layout::Hex => P::new((fi + odd(j)) * w, fj * h * 0.75),
            Layout::HexColumn => P::new(fi * w * 0.75, (fj + odd(i)) * h),
        }
    }

    /// Column and row steps (the hex layouts pack theirs at ¾).
    fn steps(self, w: f64, h: f64) -> (f64, f64) {
        match self {
            Layout::Hex => (w, h * 0.75),
            Layout::HexColumn => (w * 0.75, h),
            _ => (w, h),
        }
    }
}

/// A motif in design units: its lines, drawn in a tile `w`×`h` from (0, 0),
/// and how the tiles are laid out.
#[derive(Clone, Debug, PartialEq)]
pub struct Motif {
    pub lines: Vec<Polyline>,
    pub w: f64,
    pub h: f64,
    pub layout: Layout,
}

/// The most tiles a motif is repeated over one shape (a motif far smaller
/// than its shape would otherwise take forever to plan).
const MAX_TILES: i64 = 200_000;

impl Motif {
    /// Whether there's anything to sew: a line, and a tile with some size.
    pub fn usable(&self) -> bool {
        self.w > 1.0
            && self.h > 1.0
            && self.w.is_finite()
            && self.h.is_finite()
            && self.lines.iter().any(|l| l.len() >= 2)
    }

    fn line_bounds(&self) -> (P, P) {
        let mut lo = P::new(0.0, 0.0);
        let mut hi = P::new(self.w, self.h);
        for q in self.lines.iter().flatten() {
            lo = P::new(lo.x.min(q.x), lo.y.min(q.y));
            hi = P::new(hi.x.max(q.x), hi.y.max(q.y));
        }
        (lo, hi)
    }

    /// Tile indices `(i0..=i1, j0..=j1)` whose lines may reach into the
    /// box `lo`–`hi` (relative to the origin). None when there'd be too
    /// many.
    fn covering(&self, lo: P, hi: P) -> Option<(i64, i64, i64, i64)> {
        if !self.usable() {
            return None;
        }
        let (sx, sy) = self.layout.steps(self.w, self.h);
        let (alo, ahi) = self.line_bounds();
        let index = |v: f64| (v.is_finite() && v.abs() < 1e12).then_some(v as i64);
        // (Shifted rows and columns move less than a tile: one more each side.)
        let i0 = index(((lo.x - ahi.x) / sx).floor())? - 1;
        let i1 = index(((hi.x - alo.x) / sx).ceil())? + 1;
        let j0 = index(((lo.y - ahi.y) / sy).floor())? - 1;
        let j1 = index(((hi.y - alo.y) / sy).ceil())? + 1;
        ((i1 - i0 + 1).saturating_mul(j1 - j0 + 1) <= MAX_TILES).then_some((i0, i1, j0, j1))
    }

    /// Every segment of every tile over the shape, in design units, the
    /// tiles laid out from `origin`.
    pub fn segments(&self, rings: &[Polyline], origin: P) -> Vec<(P, P)> {
        let Some((lo, hi)) = bounds(rings) else {
            return Vec::new();
        };
        let Some((i0, i1, j0, j1)) = self.covering(lo - origin, hi - origin) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for j in j0..=j1 {
            for i in i0..=i1 {
                let o = origin + self.layout.offset(self.w, self.h, i, j);
                for l in &self.lines {
                    out.extend(l.windows(2).map(|w| (w[0] + o, w[1] + o)));
                }
            }
        }
        out
    }

    /// Patterned fill: where rows `spacing` apart, at `angle` (radians)
    /// and laid out from `origin` as `fill::tatami` lays its rows out,
    /// cross the motif's lines. For row `k` (at `(k + ½)·spacing` once
    /// turned so rows run along x), the x's it crosses at, in that turned
    /// frame, keyed by `k`.
    pub fn crossings(
        &self,
        rings: &[Polyline],
        origin: P,
        angle: f64,
        spacing: f64,
    ) -> std::collections::HashMap<i64, Vec<f64>> {
        let (c, s) = ((-angle).cos(), (-angle).sin());
        let mut out: std::collections::HashMap<i64, Vec<f64>> = Default::default();
        if spacing <= 0.0 {
            return out;
        }
        for (a, b) in self.segments(rings, origin) {
            let a = (a - origin).rotate(c, s);
            let b = (b - origin).rotate(c, s);
            let (top, bot) = if a.y <= b.y { (a, b) } else { (b, a) };
            if bot.y - top.y < 1e-9 {
                // Along a row: no row crosses it.
                continue;
            }
            // Rows in [top.y, bot.y): a vertex shared by two segments
            // counts once.
            let k0 = (top.y / spacing - 0.5).ceil() as i64;
            let k1 = (bot.y / spacing - 0.5).ceil() as i64 - 1;
            for k in k0..=k1 {
                let y = (k as f64 + 0.5) * spacing;
                let x = top.x + (y - top.y) * (bot.x - top.x) / (bot.y - top.y);
                out.entry(k).or_default().push(x);
            }
        }
        for xs in out.values_mut() {
            xs.sort_unstable_by(f64::total_cmp);
        }
        out
    }

    /// The motif's lines one after another, as one line: the first's start
    /// to the last's end (what a motif fill chains copy to copy).
    fn chain(&self) -> Polyline {
        let mut out: Polyline = Vec::new();
        for l in self.lines.iter().filter(|l| l.len() >= 2) {
            for &q in l {
                if out.last().is_none_or(|p: &P| p.dist(q) > 1e-6) {
                    out.push(q);
                }
            }
        }
        out
    }
}

fn bounds(rings: &[Polyline]) -> Option<(P, P)> {
    let mut lo = P::new(f64::INFINITY, f64::INFINITY);
    let mut hi = P::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for q in rings.iter().flatten() {
        lo = P::new(lo.x.min(q.x), lo.y.min(q.y));
        hi = P::new(hi.x.max(q.x), hi.y.max(q.y));
    }
    lo.x.is_finite().then_some((lo, hi))
}

/// Motif fill: copies of the motif chained along each row of tiles (one
/// copy's end straight to the next one's start), cut to the shape, sewn as
/// running stitch at most `len` long. Rows go back and forth. Bits shorter
/// than `min_run` are left out.
pub fn motif_fill(
    rings: &[Polyline],
    rule: FillRule,
    motif: &Motif,
    origin: P,
    len: f64,
    min_run: f64,
) -> Vec<Polyline> {
    let chain = motif.chain();
    if chain.len() < 2 {
        return Vec::new();
    }
    let Some((lo, hi)) = bounds(rings) else {
        return Vec::new();
    };
    let Some((i0, i1, j0, j1)) = motif.covering(lo - origin, hi - origin) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for j in j0..=j1 {
        let mut row: Polyline = Vec::new();
        for i in i0..=i1 {
            let o = origin + motif.layout.offset(motif.w, motif.h, i, j);
            for &q in &chain {
                let q = q + o;
                if row.last().is_none_or(|p: &P| p.dist(q) > 1e-6) {
                    row.push(q);
                }
            }
        }
        if (j - j0) % 2 == 1 {
            row.reverse();
        }
        for run in clip_inside(&row, rings, rule) {
            let length: f64 = run.windows(2).map(|w| w[0].dist(w[1])).sum();
            if length >= min_run {
                out.push(running(&run, false, len, 1));
            }
        }
    }
    out
}

/// What's wrong with a motif for how it's used: things that won't sew as
/// drawn. Plain sentences, for the Stitch panel and for agents.
/// `pattern`: a patterned fill (else a motif fill) with rows at `angle`
/// (radians, as the shape's fill angle).
pub fn problems(m: &Motif, pattern: bool, angle: f64) -> Vec<String> {
    let mut out = Vec::new();
    if !m.usable() {
        out.push("The motif has no lines, or no size.".to_string());
        return out;
    }
    let small = 15.0; // 1.5 mm
    if m.w < small || m.h < small {
        out.push(format!(
            "The tile is {:.1} × {:.1} mm: under 1.5 mm, the motif is lost in the stitches.",
            m.w / 10.0,
            m.h / 10.0
        ));
    }
    if pattern {
        // Lines along the rows put no needle down (Brother's manual: "some
        // lines will not sew"). Within 10° of the rows, few do.
        let (c, s) = ((-angle).cos(), (-angle).sin());
        let (mut along, mut all) = (0.0, 0.0);
        for l in &m.lines {
            for w in l.windows(2) {
                let d = (w[1] - w[0]).rotate(c, s);
                let n = d.x.hypot(d.y);
                all += n;
                if n > 1e-9 && (d.y / n).abs() < (10f64).to_radians().sin() {
                    along += n;
                }
            }
        }
        if all > 0.0 && along / all > 0.25 {
            out.push(format!(
                "{:.0}% of the motif runs along the fill's rows, and won't show: \
                 turn the fill's angle, or the motif.",
                100.0 * along / all
            ));
        }
    } else {
        let chain = m.chain();
        let (a, b) = (chain[0], *chain.last().unwrap());
        // Chained copy to copy: the end should meet the next copy's start,
        // a tile across, or there's a straight stitch between.
        let gap = (b - (a + P::new(m.w, 0.0))).dist(P::new(0.0, 0.0));
        if gap > m.w.max(m.h) * 0.25 {
            out.push(format!(
                "The motif ends {:.1} mm from where the next copy starts \
                 (its start, one tile to the right): a straight stitch joins them.",
                gap / 10.0
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(w: f64) -> Vec<Polyline> {
        vec![vec![
            P::new(0.0, 0.0),
            P::new(w, 0.0),
            P::new(w, w),
            P::new(0.0, w),
        ]]
    }

    fn zig(w: f64, h: f64) -> Motif {
        Motif {
            lines: vec![vec![P::new(0.0, 0.0), P::new(w / 2.0, h), P::new(w, 0.0)]],
            w,
            h,
            layout: Layout::Grid,
        }
    }

    #[test]
    fn layouts_shift_alternate_rows() {
        assert_eq!(Layout::Grid.offset(10.0, 5.0, 1, 1), P::new(10.0, 5.0));
        assert_eq!(Layout::Brick.offset(10.0, 5.0, 1, 1), P::new(15.0, 5.0));
        assert_eq!(Layout::Brick.offset(10.0, 5.0, 1, -1), P::new(15.0, -5.0));
        assert_eq!(
            Layout::BrickColumn.offset(10.0, 5.0, 1, 0),
            P::new(10.0, 2.5)
        );
        assert_eq!(Layout::Hex.offset(10.0, 8.0, 0, 1), P::new(5.0, 6.0));
        for l in Layout::ALL {
            assert_eq!(Layout::parse(l.name()), Some(l));
        }
    }

    #[test]
    fn rows_cross_a_vertical_line_once_each() {
        // A vertical line in a 50-unit tile: every row crosses each copy once.
        let m = Motif {
            lines: vec![vec![P::new(25.0, 0.0), P::new(25.0, 50.0)]],
            w: 50.0,
            h: 50.0,
            layout: Layout::Grid,
        };
        let x = m.crossings(&square(200.0), P::new(0.0, 0.0), 0.0, 4.0);
        let row = &x[&10]; // y = 42
        for want in [25.0, 75.0, 125.0, 175.0] {
            assert!(row.iter().any(|&v| (v - want).abs() < 1e-9), "{row:?}");
        }
        // Copies join end to start: no doubled crossing where they meet.
        assert_eq!(
            x[&12].iter().filter(|&&v| (v - 25.0).abs() < 1e-9).count(),
            1
        );
    }

    #[test]
    fn lines_along_the_rows_put_no_needle_down() {
        let m = Motif {
            lines: vec![vec![P::new(0.0, 25.0), P::new(50.0, 25.0)]],
            w: 50.0,
            h: 50.0,
            layout: Layout::Grid,
        };
        assert!(
            m.crossings(&square(200.0), P::new(0.0, 0.0), 0.0, 4.0)
                .is_empty()
        );
        assert!(!problems(&m, true, 0.0).is_empty());
        // Turned 90°, the rows cross it.
        assert!(
            !m.crossings(&square(200.0), P::new(0.0, 0.0), 1.2, 4.0)
                .is_empty()
        );
        assert!(problems(&m, true, std::f64::consts::FRAC_PI_2).is_empty());
    }

    #[test]
    fn motif_fill_rows_stay_inside_and_stitch_short() {
        let m = zig(60.0, 30.0);
        let rings = square(300.0);
        let rows = motif_fill(&rings, FillRule::NonZero, &m, P::new(0.0, 0.0), 25.0, 10.0);
        assert!(rows.len() >= 8, "{} rows", rows.len());
        for r in &rows {
            for w in r.windows(2) {
                assert!(w[0].dist(w[1]) <= 25.0 + 1e-6);
            }
            for q in r {
                assert!(
                    q.x >= -2.0 && q.x <= 302.0 && q.y >= -2.0 && q.y <= 302.0,
                    "{q:?}"
                );
            }
        }
        assert!(problems(&m, false, 0.0).is_empty());
    }

    #[test]
    fn a_motif_that_ends_far_from_its_start_is_flagged() {
        let m = Motif {
            lines: vec![vec![P::new(0.0, 0.0), P::new(30.0, 30.0)]],
            w: 60.0,
            h: 30.0,
            layout: Layout::Grid,
        };
        assert!(!problems(&m, false, 0.0).is_empty());
    }

    #[test]
    fn too_many_tiles_gives_nothing() {
        let m = zig(2.0, 2.0);
        assert!(m.segments(&square(100_000.0), P::new(0.0, 0.0)).is_empty());
    }
}
