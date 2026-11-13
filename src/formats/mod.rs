//! Output formats. PES/PEC/DST share a flat command stream (like pyembroidery's);
//! the Brother Skitch's PEN format is encoded straight from the [`Design`].

pub mod dst;
pub mod jef_data;
pub mod more;
pub mod pec_data;
pub mod pen;
pub mod pes;
pub mod preview;
pub mod read;
pub mod worksheet;

use crate::design::Design;
use crate::geom::{Bounds, P};

/// Machine formats ZigZag writes: (extension, description).
pub const MACHINE_FORMATS: [(&str, &str); 7] = [
    ("pes", "PES (Brother, Baby Lock: PE800, PE900, Skitch USB)"),
    ("dst", "DST (Tajima: most commercial machines)"),
    ("exp", "EXP (Melco, Bernina)"),
    ("jef", "JEF (Janome, Elna)"),
    ("vp3", "VP3 (Husqvarna Viking, Pfaff)"),
    ("pec", "PEC (Brother, stitches only)"),
    ("pen", "PEN (Brother Skitch over Bluetooth)"),
];

/// Machine files ZigZag can open.
pub const READABLE_FORMATS: [&str; 5] = ["pes", "pec", "dst", "exp", "jef"];

/// Encode a design in the machine format for `ext` (no preview formats),
/// with the usual locks and no shortest stitch.
pub fn write_format(d: &Design, ext: &str) -> anyhow::Result<Vec<u8>> {
    write_format_with(d, ext, &ExportOptions::default())
}

/// [`write_format`] with a document's export settings. (The Skitch's PEN
/// stream has locks of its own, matched to Brother's app: `opt` isn't for it.)
pub fn write_format_with(d: &Design, ext: &str, opt: &ExportOptions) -> anyhow::Result<Vec<u8>> {
    Ok(match ext.to_ascii_lowercase().as_str() {
        "pes" => pes::write_pes_with(d, opt),
        "pec" => pes::write_pec_file_with(d, opt),
        "dst" => dst::write_dst_with(d, opt),
        "exp" => more::write_exp_with(d, opt),
        "jef" => more::write_jef_with(d, opt),
        "vp3" => more::write_vp3_with(d, opt),
        "pen" => pen::encode(d).bytes,
        other => anyhow::bail!(
            "unknown machine format '.{other}' (use pes, dst, exp, jef, vp3, pec or pen)"
        ),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Stitch,
    Jump,
    Trim,
    ColorChange,
    End,
}

/// One machine command at an absolute position (integer embroidery units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cmd {
    pub x: f64,
    pub y: f64,
    pub op: Op,
}

/// How the thread is locked where sewing starts and before it's cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LockStyle {
    /// No lock stitches (the machine, or the design's own, hold the thread).
    None,
    /// Back and forth along the line of sewing.
    Back,
    /// Round a small triangle.
    Triangle,
}

impl LockStyle {
    pub fn name(self) -> &'static str {
        match self {
            LockStyle::None => "none",
            LockStyle::Back => "back",
            LockStyle::Triangle => "triangle",
        }
    }
    pub fn parse(s: &str) -> Option<LockStyle> {
        match s.trim().to_ascii_lowercase().as_str() {
            "none" | "off" => Some(LockStyle::None),
            "back" | "back-and-forth" | "backtack" => Some(LockStyle::Back),
            "triangle" => Some(LockStyle::Triangle),
            _ => None,
        }
    }
    /// Stitches in one lap: out and back, or round the triangle.
    fn lap(self) -> u32 {
        match self {
            LockStyle::Triangle => 3,
            _ => 2,
        }
    }
}

/// Lock (tie-in and tie-off) stitches: their shape, length and number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lock {
    pub style: LockStyle,
    /// How long each lock stitch is, mm.
    pub len_mm: f64,
    /// How many stitches: whole laps, so the lock ends where it began
    /// (twos back and forth, threes round a triangle).
    pub count: u32,
}

impl Default for Lock {
    /// What ZigZag has always sewn: four 0.4 mm stitches back and forth.
    fn default() -> Self {
        Lock {
            style: LockStyle::Back,
            len_mm: 0.4,
            count: 4,
        }
    }
}

impl Lock {
    /// The lengths a lock stitch can have, mm.
    pub const LEN_MM: std::ops::RangeInclusive<f64> = 0.2..=2.0;
    /// The most stitches in a lock.
    pub const MAX_COUNT: u32 = 12;

    /// A lock with its length in range and its stitches in whole laps
    /// (rounded up).
    pub fn new(style: LockStyle, len_mm: f64, count: u32) -> Lock {
        let lap = style.lap();
        Lock {
            style,
            len_mm: if len_mm.is_finite() {
                len_mm.clamp(*Self::LEN_MM.start(), *Self::LEN_MM.end())
            } else {
                Lock::default().len_mm
            },
            count: (count.clamp(lap, Self::MAX_COUNT).div_ceil(lap) * lap).min(Self::MAX_COUNT),
        }
    }

    /// As files write it: `none`, or `back:0.4:4` (style, mm, stitches).
    pub fn format(&self) -> String {
        match self.style {
            LockStyle::None => "none".into(),
            s => format!(
                "{}:{}:{}",
                s.name(),
                crate::params::fmt_num(self.len_mm),
                self.count
            ),
        }
    }

    /// [`Lock::format`]'s inverse; a style alone takes the usual length and
    /// number.
    pub fn parse(v: &str) -> Option<Lock> {
        let mut parts = v.split(':');
        let style = LockStyle::parse(parts.next()?)?;
        let d = Lock::default();
        let len = match parts.next() {
            Some(l) => l.parse().ok()?,
            None => d.len_mm,
        };
        let count = match parts.next() {
            Some(c) => c.parse().ok()?,
            None => style.lap() * 2,
        };
        Some(Lock::new(style, len, count))
    }
}

/// What a document says about the machine file, whatever its format.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ExportOptions {
    pub lock: Lock,
    /// No stitch in the file is shorter than this, units (0: any length).
    /// The plan already keeps to it; rounding to the file's whole units can
    /// still shorten a stitch, and those are merged here.
    pub min_stitch: f64,
}

#[derive(Clone, Debug)]
pub struct StreamOptions {
    /// Lock stitches where sewing starts, and before trims and color changes.
    pub lock: Lock,
    /// No stitch shorter than this, units (see [`ExportOptions`]).
    pub min_stitch: f64,
    /// Longest single stitch or jump the format can encode, units.
    pub max_step: f64,
}

impl StreamOptions {
    pub fn new(opt: &ExportOptions, max_step: f64) -> StreamOptions {
        StreamOptions {
            lock: opt.lock,
            min_stitch: opt.min_stitch,
            max_step,
        }
    }
}

/// The needle points of a lock at `at` (whole units), lying along `dir`:
/// each lap leaves `at` and comes back to it. No lock stitch is shorter
/// than the shortest allowed: the lock is at least that long, and where
/// rounding to whole units shortens a side, a unit longer until none is.
fn lock_points(at: P, dir: P, opt: &StreamOptions) -> Vec<P> {
    let r = |p: P| P::new(p.x.round(), p.y.round());
    if opt.lock.style == LockStyle::None {
        return Vec::new();
    }
    // The triangle's second corner: `dir` turned a sixth of a turn.
    let (s, c) = std::f64::consts::FRAC_PI_3.sin_cos();
    let side = P::new(dir.x * c - dir.y * s, dir.x * s + dir.y * c);
    let mut len = (opt.lock.len_mm * 10.0).max(opt.min_stitch);
    let lap = loop {
        let lap = match opt.lock.style {
            LockStyle::Triangle => vec![r(at + dir * len), r(at + side * len), at],
            _ => vec![r(at + dir * len), at],
        };
        let shortest = (0..lap.len())
            .map(|i| lap[i].dist(lap[(i + lap.len() - 1) % lap.len()]))
            .fold(f64::INFINITY, f64::min);
        if shortest >= opt.min_stitch || len > 60.0 {
            break lap;
        }
        len += 1.0;
    };
    lap.iter()
        .cycle()
        .take(opt.lock.count as usize)
        .copied()
        .collect()
}

/// Flatten a design into a command stream: jump to each run, tie in, stitch,
/// tie off, trim. Coordinates are rounded to whole units.
pub fn commands(d: &Design, opt: &StreamOptions) -> Vec<Cmd> {
    commands_owned(d, opt).0
}

/// [`commands`], and for each command the input shape it's for
/// ([`Design::stitch_shapes`]): a shape's stitches, the locks and the jump
/// that begin them and the lock and cut that end them. `None` for a change
/// of thread, the end, and a design that doesn't say.
pub fn commands_owned(d: &Design, opt: &StreamOptions) -> (Vec<Cmd>, Vec<Option<usize>>) {
    let mut owners: Vec<Option<usize>> = Vec::new();
    // The shape the last stitches were for.
    let mut owner: Option<usize> = None;
    let mut out: Vec<Cmd> = Vec::new();
    let r = |p: P| P::new(p.x.round(), p.y.round());
    let mut pos: Option<P> = None;
    let mut prev_dir = P::new(1.0, 0.0);

    // Split long moves so no single step exceeds `max_step` on either axis,
    // measured between rounded points (what the encoder actually sees).
    let push_line = |out: &mut Vec<Cmd>, from: P, to: P, op: Op| {
        let d = to - from;
        let mut n = (d.x.abs().max(d.y.abs()) / opt.max_step).ceil().max(1.0) as usize;
        loop {
            let pts: Vec<P> = (1..=n)
                .map(|i| r(from.lerp(to, i as f64 / n as f64)))
                .collect();
            let mut prev = from;
            let ok = pts.iter().all(|&q| {
                let fits =
                    (q.x - prev.x).abs() <= opt.max_step && (q.y - prev.y).abs() <= opt.max_step;
                prev = q;
                fits
            });
            if ok {
                out.extend(pts.into_iter().map(|q| Cmd { x: q.x, y: q.y, op }));
                return;
            }
            n += 1;
        }
    };
    let tie = |out: &mut Vec<Cmd>, at: P, dir: P| {
        for q in lock_points(at, dir, opt) {
            out.push(Cmd {
                x: q.x,
                y: q.y,
                op: Op::Stitch,
            });
        }
    };

    for (bi, blk) in d.blocks.iter().enumerate() {
        if bi > 0
            && let Some(p) = pos
        {
            tie(&mut out, p, prev_dir);
            out.push(Cmd {
                x: p.x,
                y: p.y,
                op: Op::Trim,
            });
            owners.resize(out.len(), owner);
            out.push(Cmd {
                x: p.x,
                y: p.y,
                op: Op::ColorChange,
            });
            owners.resize(out.len(), None);
        }
        for (ri, run) in blk.runs.iter().enumerate() {
            if run.pts.is_empty() {
                continue;
            }
            let start = r(run.pts[0]);
            if ri > 0
                && run.trim_before
                && let Some(p) = pos
            {
                tie(&mut out, p, prev_dir);
                out.push(Cmd {
                    x: p.x,
                    y: p.y,
                    op: Op::Trim,
                });
                owners.resize(out.len(), owner);
            }
            // Whose stitches these are, from each point of the run on.
            let spans: &[(usize, usize)] = d
                .stitch_shapes
                .get(bi)
                .and_then(|b| b.get(ri))
                .map_or(&[], |s| s.as_slice());
            let mut span = 0;
            owner = spans.first().map(|s| s.1);
            match pos {
                // Machines start at the origin; the first move is a jump from there.
                None => push_line(&mut out, P::default(), start, Op::Jump),
                Some(p) if p != start => push_line(&mut out, p, start, Op::Jump),
                _ => {}
            }
            out.push(Cmd {
                x: start.x,
                y: start.y,
                op: Op::Stitch,
            });
            owners.resize(out.len(), owner);
            let first_dir = run
                .pts
                .iter()
                .map(|&q| r(q) - start)
                .find(|v| v.len() > 0.5)
                .map(|v| v.norm())
                .unwrap_or(P::new(1.0, 0.0));
            tie(&mut out, start, first_dir);
            owners.resize(out.len(), owner);
            let mut cur = start;
            for (k, &q) in run.pts.iter().enumerate().skip(1) {
                while spans.get(span + 1).is_some_and(|s| s.0 <= k) {
                    span += 1;
                    owner = Some(spans[span].1);
                }
                let q = r(q);
                // (Rounded onto the last point, or too near it for a stitch.)
                if q == cur || q.dist(cur) < opt.min_stitch {
                    continue;
                }
                prev_dir = (cur - q).norm();
                push_line(&mut out, cur, q, Op::Stitch);
                owners.resize(out.len(), owner);
                cur = q;
            }
            pos = Some(cur);
        }
    }
    if let Some(p) = pos {
        tie(&mut out, p, prev_dir);
        out.push(Cmd {
            x: p.x,
            y: p.y,
            op: Op::Trim,
        });
        owners.resize(out.len(), owner);
        out.push(Cmd {
            x: p.x,
            y: p.y,
            op: Op::End,
        });
        owners.resize(out.len(), None);
    }
    (out, owners)
}

/// The longest step a machine format's records hold, units: what its
/// writer splits longer stitches and jumps to. `None` for a format that
/// isn't written from the command stream.
fn max_step(ext: &str) -> Option<f64> {
    match ext.to_ascii_lowercase().as_str() {
        "dst" => Some(121.0),
        "pes" | "pec" => Some(120.0),
        "exp" | "jef" => Some(127.0),
        "vp3" => Some(255.0),
        _ => None,
    }
}

/// A run of commands for one object in a written file.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OwnedRange {
    /// The first and last command, counting every command (stitch, jump,
    /// trim, change of thread, end) from 0 in the order written.
    pub first: usize,
    pub last: usize,
    /// The same counting only stitches (needle points), if it has any.
    pub stitches: Option<(usize, usize)>,
    /// The input shape, or `None` (a change of thread, the end).
    pub shape: Option<usize>,
    /// Where the first command is, units.
    pub at: P,
}

/// Which object each command of the `ext` file for `d` is for, as runs of
/// commands for one object: what a checker needs to name the shape behind
/// a stitch. `None` for a format that isn't written from the command
/// stream (the Skitch's PEN).
pub fn object_ranges(d: &Design, ext: &str, opt: &ExportOptions) -> Option<Vec<OwnedRange>> {
    let (cmds, owners) = commands_owned(d, &StreamOptions::new(opt, max_step(ext)?));
    let mut out: Vec<OwnedRange> = Vec::new();
    let mut stitch = 0;
    for (k, (c, &shape)) in cmds.iter().zip(&owners).enumerate() {
        let is_stitch = c.op == Op::Stitch;
        match out.last_mut() {
            Some(r) if r.shape == shape => {
                r.last = k;
                if is_stitch {
                    r.stitches = Some((r.stitches.map_or(stitch, |s| s.0), stitch));
                }
            }
            _ => out.push(OwnedRange {
                first: k,
                last: k,
                stitches: is_stitch.then_some((stitch, stitch)),
                shape,
                at: P::new(c.x, c.y),
            }),
        }
        stitch += is_stitch as usize;
    }
    Some(out)
}

/// Bounds over every command position (pyembroidery's `pattern.bounds()`).
pub fn stream_bounds(cmds: &[Cmd]) -> Bounds {
    let mut b = Bounds::empty();
    for c in cmds {
        b.add(P::new(c.x, c.y));
    }
    if b.is_empty() {
        b.add(P::default());
    }
    b
}

/// Red-mean color distance, identical to pyembroidery's `color_distance_red_mean`.
/// How different two colors look (CIEDE2000 ΔE), for naming and matching
/// threads. CIE76 sent saturated blues to purple. Machine palettes keep
/// `color_distance`, which other software matches.
pub fn perceptual_distance(a: [u8; 3], b: [u8; 3]) -> f64 {
    fn lab(c: [u8; 3]) -> [f64; 3] {
        let lin = |v: u8| {
            let v = v as f64 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let (r, g, b) = (lin(c[0]), lin(c[1]), lin(c[2]));
        let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
        let f = |t: f64| {
            if t > 0.008856 {
                t.cbrt()
            } else {
                7.787 * t + 16.0 / 116.0
            }
        };
        let (fx, fy, fz) = (f(x), f(y), f(z));
        [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
    }
    delta_e2000(lab(a), lab(b))
}

/// CIEDE2000 colour difference (kL = kC = kH = 1), after VectorCraft's
/// `cms::lab::delta_e2000` (MIT OR Apache-2.0).
fn delta_e2000(x: [f64; 3], y: [f64; 3]) -> f64 {
    let ([l1, a1, b1], [l2, a2, b2]) = (x, y);
    let (c1, c2) = (a1.hypot(b1), a2.hypot(b2));
    let cm = (c1 + c2) / 2.0;
    let g = 0.5 * (1.0 - (cm.powi(7) / (cm.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = (a1 * (1.0 + g), a2 * (1.0 + g));
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hp = |b: f64, a: f64| {
        if b == 0.0 && a == 0.0 {
            0.0
        } else {
            b.atan2(a).to_degrees().rem_euclid(360.0)
        }
    };
    let (h1p, h2p) = (hp(b1, a1p), hp(b2, a2p));
    let (dlp, dcp) = (l2 - l1, c2p - c1p);
    let dhp = if c1p * c2p == 0.0 {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p - h1p > 180.0 {
        h2p - h1p - 360.0
    } else {
        h2p - h1p + 360.0
    };
    let dhp_big = 2.0 * (c1p * c2p).sqrt() * (dhp.to_radians() / 2.0).sin();
    let lpm = (l1 + l2) / 2.0;
    let cpm = (c1p + c2p) / 2.0;
    let hpm = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hpm - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hpm).to_radians().cos()
        + 0.32 * (3.0 * hpm + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hpm - 63.0).to_radians().cos();
    let dtheta = 30.0 * (-((hpm - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (cpm.powi(7) / (cpm.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (lpm - 50.0).powi(2) / (20.0 + (lpm - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cpm;
    let sh = 1.0 + 0.015 * cpm * t;
    let rt = -(2.0 * dtheta).to_radians().sin() * rc;
    let v = (dlp / sl).powi(2)
        + (dcp / sc).powi(2)
        + (dhp_big / sh).powi(2)
        + rt * (dcp / sc) * (dhp_big / sh);
    v.max(0.0).sqrt()
}

pub fn color_distance(a: [u8; 3], b: [u8; 3]) -> i64 {
    let (r1, g1, b1) = (a[0] as i64, a[1] as i64, a[2] as i64);
    let (r2, g2, b2) = (b[0] as i64, b[1] as i64, b[2] as i64);
    let red_mean = ((r1 + r2) as f64 / 2.0).round_ties_even() as i64;
    let (r, g, bl) = (r1 - r2, g1 - g2, b1 - b2);
    (((512 + red_mean) * r * r) >> 8) + 4 * g * g + (((767 - red_mean) * bl * bl) >> 8)
}

/// Index of the nearest color in `palette` (skipping `None`); ties go to the later index.
pub fn nearest_index(color: [u8; 3], palette: &[Option<[u8; 3]>]) -> Option<usize> {
    let mut best = None;
    let mut best_d = i64::MAX;
    for (i, c) in palette.iter().enumerate() {
        if let Some(c) = c {
            let d = color_distance(color, *c);
            if d <= best_d {
                best_d = d;
                best = Some(i);
            }
        }
    }
    best
}

pub fn pec_palette() -> Vec<Option<[u8; 3]>> {
    pec_data::PEC_PALETTE
        .iter()
        .enumerate()
        .map(|(i, &(r, g, b, _))| if i == 0 { None } else { Some([r, g, b]) })
        .collect()
}

#[cfg(test)]
mod lock_tests {
    use super::*;
    use crate::design::{Block, Run};

    /// Two runs with a cut between them, the first sewn in direction `dir`.
    fn two_runs(dir: P) -> Design {
        let a: Vec<P> = (0..5).map(|k| dir * (k as f64 * 25.0)).collect();
        let b: Vec<P> = (0..5).map(|k| P::new(300.0, k as f64 * 25.0)).collect();
        Design {
            blocks: vec![Block {
                color: [0, 0, 0],
                runs: vec![
                    Run {
                        pts: a,
                        trim_before: false,
                    },
                    Run {
                        pts: b,
                        trim_before: true,
                    },
                ],
            }],
            ..Default::default()
        }
    }

    fn stitch_lengths(cmds: &[Cmd]) -> Vec<f64> {
        cmds.windows(2)
            .filter(|w| w[0].op == Op::Stitch && w[1].op == Op::Stitch)
            .map(|w| P::new(w[0].x, w[0].y).dist(P::new(w[1].x, w[1].y)))
            .collect()
    }

    fn stream(d: &Design, lock: Lock, min_stitch: f64) -> Vec<Cmd> {
        commands(
            d,
            &StreamOptions {
                lock,
                min_stitch,
                max_step: 121.0,
            },
        )
    }

    #[test]
    fn a_lock_is_written_and_read_back_in_whole_laps() {
        assert_eq!(Lock::default().format(), "back:0.4:4");
        for l in [
            Lock::default(),
            Lock::new(LockStyle::Triangle, 0.55, 6),
            Lock::new(LockStyle::None, 0.4, 4),
        ] {
            let back = Lock::parse(&l.format()).unwrap();
            assert_eq!(back.style, l.style);
            if l.style != LockStyle::None {
                assert_eq!(back, l);
            }
        }
        // Stitches come in whole laps (rounded up), within limits.
        assert_eq!(Lock::new(LockStyle::Back, 0.4, 3).count, 4);
        assert_eq!(Lock::new(LockStyle::Triangle, 0.4, 4).count, 6);
        assert_eq!(Lock::new(LockStyle::Triangle, 0.4, 1).count, 3);
        assert_eq!(Lock::new(LockStyle::Back, 0.4, 99).count, 12);
        assert_eq!(Lock::new(LockStyle::Back, 9.0, 2).len_mm, 2.0);
        // A style alone takes the usual length and two laps.
        assert_eq!(Lock::parse("back"), Some(Lock::default()));
        assert_eq!(
            Lock::parse("triangle"),
            Some(Lock::new(LockStyle::Triangle, 0.4, 6))
        );
        assert_eq!(Lock::parse("zigzag"), None);
        assert_eq!(Lock::parse("back:long:4"), None);
    }

    #[test]
    fn the_usual_lock_is_what_it_always_was() {
        let d = two_runs(P::new(1.0, 0.0));
        let cmds = stream(&d, Lock::default(), 0.0);
        // Tie-in at the start: out 0.4 mm along the run and back, twice.
        let at: Vec<(f64, f64)> = cmds[1..6].iter().map(|c| (c.x, c.y)).collect();
        assert_eq!(
            at,
            [(0.0, 0.0), (4.0, 0.0), (0.0, 0.0), (4.0, 0.0), (0.0, 0.0)]
        );
        // 4 lock stitches at each of: start, before the cut, after it, end.
        let n = |cmds: &[Cmd]| cmds.iter().filter(|c| c.op == Op::Stitch).count();
        assert_eq!(n(&cmds), 10 + 4 * 4);
        // No locks: only the runs' own stitches.
        let none = stream(&d, Lock::new(LockStyle::None, 0.4, 4), 0.0);
        assert_eq!(n(&none), 10);
        assert_eq!(
            none.iter().filter(|c| c.op == Op::Trim).count(),
            cmds.iter().filter(|c| c.op == Op::Trim).count()
        );
    }

    #[test]
    fn a_triangle_lock_goes_round_and_comes_back() {
        let d = two_runs(P::new(1.0, 0.0));
        let cmds = stream(&d, Lock::new(LockStyle::Triangle, 0.6, 3), 0.0);
        let at: Vec<(f64, f64)> = cmds[1..5].iter().map(|c| (c.x, c.y)).collect();
        assert_eq!(at, [(0.0, 0.0), (6.0, 0.0), (3.0, 5.0), (0.0, 0.0)]);
        assert_eq!(
            cmds.iter().filter(|c| c.op == Op::Stitch).count(),
            10 + 4 * 3
        );
    }

    #[test]
    fn no_lock_stitch_is_shorter_than_the_shortest_stitch() {
        // Whichever way the run goes: rounding a lock's corner to whole
        // units can shorten its sides (0.4 mm along (3, 4) rounds to 0.36).
        for deg in (0..360).step_by(7) {
            let a = (deg as f64).to_radians();
            let d = two_runs(P::new(a.cos(), a.sin()));
            for lock in [
                Lock::default(),
                Lock::new(LockStyle::Triangle, 0.4, 3),
                Lock::new(LockStyle::Back, 0.2, 2),
            ] {
                let cmds = stream(&d, lock, 4.0);
                let shortest = stitch_lengths(&cmds)
                    .into_iter()
                    .fold(f64::INFINITY, f64::min);
                assert!(shortest >= 4.0, "{deg}° {lock:?}: a {shortest} stitch");
            }
        }
        // (Without a shortest stitch, that one is as it rounds.)
        let d = two_runs(P::new(0.6, 0.8));
        let cmds = stream(&d, Lock::default(), 0.0);
        assert!(stitch_lengths(&cmds).iter().any(|l| *l < 4.0));
    }

    #[test]
    fn stitches_rounded_too_short_are_merged_in_the_file() {
        // 0.44 mm apart, but 0.4 and then 0.3 once on whole units.
        let pts = vec![
            P::new(0.0, 0.0),
            P::new(30.0, 0.0),
            P::new(34.4, 0.0),
            P::new(37.6, 0.0),
            P::new(60.0, 0.0),
        ];
        let d = Design {
            blocks: vec![Block {
                color: [0, 0, 0],
                runs: vec![Run {
                    pts,
                    trim_before: false,
                }],
            }],
            ..Default::default()
        };
        let none = Lock::new(LockStyle::None, 0.4, 4);
        let xs = |cmds: &[Cmd]| -> Vec<f64> {
            cmds.iter()
                .filter(|c| c.op == Op::Stitch)
                .map(|c| c.x)
                .collect()
        };
        assert_eq!(xs(&stream(&d, none, 0.0)), [0.0, 30.0, 34.0, 38.0, 60.0]);
        assert_eq!(xs(&stream(&d, none, 4.0)), [0.0, 30.0, 34.0, 38.0, 60.0]);
        assert_eq!(xs(&stream(&d, none, 5.0)), [0.0, 30.0, 38.0, 60.0]);
    }
}

#[cfg(test)]
mod ciede2000_tests {
    use super::*;

    #[test]
    fn reference_pairs() {
        // Sharma, Wu & Dalal (2005) test data, pairs 1 and 7.
        let d = delta_e2000([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485]);
        assert!((d - 2.0425).abs() < 1e-3, "{d}");
        let d = delta_e2000([50.0, 0.0, 0.0], [50.0, -1.0, 2.0]);
        assert!((d - 2.3669).abs() < 1e-3, "{d}");
    }

    #[test]
    fn a_saturated_blue_matches_a_blue_thread() {
        let pal: Vec<[u8; 3]> = pec_data::PEC_PALETTE
            .iter()
            .skip(1)
            .map(|&(r, g, b, _)| [r, g, b])
            .collect();
        let best = pal
            .iter()
            .min_by(|x, y| {
                perceptual_distance([20, 64, 200], **x)
                    .total_cmp(&perceptual_distance([20, 64, 200], **y))
            })
            .unwrap();
        assert_eq!(*best, [11, 61, 145], "Ultramarine, not Purple");
    }
}
