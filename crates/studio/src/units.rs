//! Units (View › Units): the unit positions, sizes, distances and the grid
//! are shown and typed in. The document is always in design units (0.1 mm);
//! this is display only. Stitch settings (spacing, density, pull
//! compensation) stay in millimetres, as embroidery software keeps them.
//!
//! Typed values can carry their own unit and simple arithmetic, as
//! VectorCraft's fields do (https://github.com/storytold/vectorcraft,
//! `crates/doc/src/units.rs`): `5 mm`, `1/4 in`, `10+5`, `3cm*2`.

use eframe::egui;
use std::cell::Cell;
use std::ops::RangeInclusive;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Unit {
    #[default]
    Mm,
    Cm,
    In,
    Pt,
    Px,
}

impl Unit {
    pub const ALL: [Unit; 5] = [Unit::Mm, Unit::Cm, Unit::In, Unit::Pt, Unit::Px];

    /// Millimetres in one of this unit. Pixels are CSS/SVG pixels (1/96
    /// in), as SVG files are read; points are 1/72 in.
    pub fn mm(self) -> f64 {
        match self {
            Unit::Mm => 1.0,
            Unit::Cm => 10.0,
            Unit::In => 25.4,
            Unit::Pt => 25.4 / 72.0,
            Unit::Px => 25.4 / 96.0,
        }
    }

    pub fn suffix(self) -> &'static str {
        match self {
            Unit::Mm => "mm",
            Unit::Cm => "cm",
            Unit::In => "in",
            Unit::Pt => "pt",
            Unit::Px => "px",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Unit::Mm => "Millimeters",
            Unit::Cm => "Centimeters",
            Unit::In => "Inches",
            Unit::Pt => "Points",
            Unit::Px => "Pixels",
        }
    }

    /// The unit `name` names: its suffix or label, in any case.
    pub fn named(name: &str) -> Option<Unit> {
        let n = name.trim().to_ascii_lowercase();
        let n = n.trim_end_matches('.');
        Unit::ALL.into_iter().find(|u| {
            n == u.suffix()
                || n == u.label().to_ascii_lowercase()
                || u.label().to_ascii_lowercase().strip_suffix('s') == Some(n)
                || (*u == Unit::In && (n == "inch" || n == "\""))
        })
    }

    /// Decimals a field shows (trailing zeros dropped in readouts).
    pub fn decimals(self) -> usize {
        match self {
            Unit::Mm | Unit::Pt | Unit::Px => 1,
            Unit::Cm => 2,
            Unit::In => 3,
        }
    }

    /// Design units (0.1 mm) → this unit.
    pub fn amount(self, v: f64) -> f64 {
        v / 10.0 / self.mm()
    }

    /// This unit → design units.
    pub fn to_units(self, v: f64) -> f64 {
        v * self.mm() * 10.0
    }

    /// `v` (in this unit) as a short number: `12.5`, `0.25`, `3`.
    pub fn number(self, v: f64) -> String {
        let s = format!("{:.*}", self.decimals(), v);
        let s = if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.')
        } else {
            &s
        };
        if s == "-0" { "0".into() } else { s.into() }
    }

    /// Design units `v` as a readout: `12.5 mm`, `0.492 in`.
    pub fn fmt(self, v: f64) -> String {
        format!("{} {}", self.number(self.amount(v)), self.suffix())
    }

    /// Millimetres `mm` as a readout in this unit.
    pub fn fmt_mm(self, mm: f64) -> String {
        self.fmt(mm * 10.0)
    }

    /// Steps the grid and its labels use, in this unit, smallest first:
    /// halves of an inch down to a thirty-second (an inch isn't read in
    /// tenths); 1, 2, 5 × 10ⁿ otherwise.
    fn ladder(self) -> &'static [f64] {
        match self {
            Unit::In => &[
                1.0 / 32.0,
                1.0 / 16.0,
                0.125,
                0.25,
                0.5,
                1.0,
                2.0,
                5.0,
                10.0,
            ],
            _ => &[
                0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0,
            ],
        }
    }

    /// The grid at `scale` (screen points per mm): its minor step and its
    /// labelled major step, in this unit. Minor lines are at least 8 points
    /// apart, labels at least 56; the major step is a whole number of
    /// minor ones.
    pub fn grid(self, scale: f64) -> (f64, f64) {
        let px = |s: f64| s * self.mm() * scale;
        let steps = self.ladder();
        let minor = steps
            .iter()
            .copied()
            .find(|&s| px(s) >= 8.0)
            .unwrap_or(*steps.last().unwrap());
        let whole = |s: f64| {
            let k = s / minor;
            (k - k.round()).abs() < 1e-9
        };
        let major = steps
            .iter()
            .copied()
            .find(|&s| s > minor && whole(s) && px(s) >= 56.0)
            .unwrap_or(minor * 10.0);
        (minor, major)
    }

    /// Read a typed length (`12`, `5 mm`, `1/4 in`, `10+5`, `3cm*2`): bare
    /// numbers are in this unit. Returns the value in this unit.
    pub fn parse(self, s: &str) -> Option<f64> {
        parse(s, self)
            .map(|mm| mm / self.mm())
            .filter(|v| v.is_finite())
    }
}

/// `s` in millimetres, bare numbers in `unit`.
fn parse(s: &str, unit: Unit) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // `+ -` then `* /`, each split at its last operator (left to right).
    for ops in [&['+', '-'][..], &['*', '/'][..]] {
        let skip = s.chars().next().map_or(0, char::len_utf8);
        let is_sign = |i: usize| {
            matches!(s[i..].chars().next(), Some('+' | '-'))
                && s[..i]
                    .trim_end()
                    .chars()
                    .last()
                    .is_some_and(|c| "+-*/".contains(c))
        };
        let at = s
            .char_indices()
            .rev()
            .find(|&(i, c)| i >= skip && ops.contains(&c) && !is_sign(i))
            .map(|(i, _)| i);
        let Some(i) = at else { continue };
        let (l, r) = (&s[..i], &s[i + 1..]);
        if l.trim().is_empty() {
            continue;
        }
        return match &s[i..i + 1] {
            "+" => Some(parse(l, unit)? + parse(r, unit)?),
            "-" => Some(parse(l, unit)? - parse(r, unit)?),
            op => {
                // A factor is a plain number; a unit after it is the
                // whole expression's (`1/4 in` is a quarter inch).
                let (k, u) = number_unit(r)?;
                let a = parse(l, u.unwrap_or(unit))?;
                if op == "*" {
                    Some(a * k)
                } else if k == 0.0 {
                    None
                } else {
                    Some(a / k)
                }
            }
        };
    }
    let (v, u) = number_unit(s)?;
    Some(v * u.unwrap_or(unit).mm())
}

/// `12`, `12mm`, `1.5 in`, `3"`: the number and its unit, if it names one.
fn number_unit(s: &str) -> Option<(f64, Option<Unit>)> {
    let s = s.trim();
    let end = s
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+'))))
        .map_or(s.len(), |(i, _)| i);
    let (n, rest) = s.split_at(end);
    let v: f64 = n.parse().ok()?;
    let rest = rest.trim();
    if rest.is_empty() {
        return Some((v, None));
    }
    Some((v, Some(Unit::named(rest)?)))
}

thread_local! {
    /// The unit this thread's UI is showing (set each frame from the
    /// saved preference; each test thread has its own).
    static CURRENT: Cell<Unit> = const { Cell::new(Unit::Mm) };
}

/// The unit lengths are shown in.
pub fn current() -> Unit {
    CURRENT.with(Cell::get)
}

pub fn set_current(u: Unit) {
    CURRENT.with(|c| c.set(u));
}

/// Design units `v` as a readout in the current unit.
pub fn fmt(v: f64) -> String {
    current().fmt(v)
}

/// Millimetres as a readout in the current unit.
pub fn fmt_mm(mm: f64) -> String {
    current().fmt_mm(mm)
}

/// A width × height readout: `12.5 × 8 mm`.
pub fn fmt_size(w: f64, h: f64) -> String {
    let u = current();
    format!(
        "{} × {} {}",
        u.number(u.amount(w)),
        u.number(u.amount(h)),
        u.suffix()
    )
}

/// A field for a length kept in millimetres, shown and typed in the
/// current unit (a typed unit or sum works too).
pub fn drag_mm(value: &mut f64, range: RangeInclusive<f64>, speed: f64) -> egui::DragValue<'_> {
    let u = current();
    let k = 1.0 / u.mm();
    egui::DragValue::from_get_set(move |set| {
        if let Some(x) = set {
            *value = x / k;
        }
        *value * k
    })
    .range(range.start() * k..=range.end() * k)
    .speed(speed * k)
    .fixed_decimals(u.decimals())
    .suffix(format!(" {}", u.suffix()))
    .custom_parser(move |s| u.parse(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn conversions_and_readouts() {
        assert_eq!(Unit::Mm.fmt(125.0), "12.5 mm");
        assert_eq!(Unit::Cm.fmt(125.0), "1.25 cm");
        assert_eq!(Unit::In.fmt(254.0), "1 in");
        assert_eq!(Unit::In.fmt(127.0), "0.5 in");
        assert_eq!(Unit::Px.fmt(254.0), "96 px");
        assert_eq!(Unit::Pt.fmt(254.0), "72 pt");
        assert_eq!(Unit::Mm.fmt(-0.01), "0 mm");
        assert!(close(Unit::In.to_units(1.0), 254.0));
        // A px is an SVG pixel: the same size the import reads it at.
        assert!(close(Unit::Px.to_units(1.0), zigzag::svg::PX_TO_UNITS));
    }

    #[test]
    fn typed_values() {
        assert!(close(Unit::Mm.parse("12").unwrap(), 12.0));
        assert!(close(Unit::Mm.parse("1 in").unwrap(), 25.4));
        assert!(close(Unit::In.parse("1/4").unwrap(), 0.25));
        assert!(close(Unit::Mm.parse("1/4 in").unwrap(), 6.35));
        assert!(close(Unit::In.parse("25.4mm").unwrap(), 1.0));
        assert!(close(Unit::Mm.parse("10+5").unwrap(), 15.0));
        assert!(close(Unit::Mm.parse("3cm*2").unwrap(), 60.0));
        assert!(close(Unit::Mm.parse("2 in - 1 cm").unwrap(), 40.8));
        assert!(close(Unit::Mm.parse("-5").unwrap(), -5.0));
        assert!(close(Unit::Mm.parse("10*-2").unwrap(), -20.0));
        assert!(close(Unit::Cm.parse("2 Inches").unwrap(), 5.08));
        assert!(close(Unit::Mm.parse("1\"").unwrap(), 25.4));
        assert_eq!(Unit::Mm.parse("abc"), None);
        assert_eq!(Unit::Mm.parse("5 furlongs"), None);
        assert_eq!(Unit::Mm.parse("1/0"), None);
    }

    #[test]
    fn grid_steps_suit_the_unit() {
        // 8 screen points a mm: 1 mm lines, labels every 10 mm.
        assert_eq!(Unit::Mm.grid(8.0), (1.0, 10.0));
        // Zoomed out: 5 mm lines, labels every 50.
        assert_eq!(Unit::Mm.grid(2.0), (5.0, 50.0));
        // Inches go in halves: an eighth at 8 pt/mm, labels each half inch.
        let (minor, major) = Unit::In.grid(8.0);
        assert_eq!((minor, major), (0.0625, 0.5));
        for u in Unit::ALL {
            for scale in [0.5, 2.0, 8.0, 30.0, 120.0] {
                let (m, big) = u.grid(scale);
                let k = big / m;
                assert!((k - k.round()).abs() < 1e-9, "{u:?} {scale}: {m} {big}");
            }
        }
    }

    #[test]
    fn names() {
        assert_eq!(Unit::named("MM"), Some(Unit::Mm));
        assert_eq!(Unit::named("inches"), Some(Unit::In));
        assert_eq!(Unit::named("inch"), Some(Unit::In));
        assert_eq!(Unit::named("Pixels"), Some(Unit::Px));
        assert_eq!(Unit::named("pixel"), Some(Unit::Px));
        assert_eq!(Unit::named("ft"), None);
    }
}
