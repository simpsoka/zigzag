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

/// Encode a design in the machine format for `ext` (no preview formats).
pub fn write_format(d: &Design, ext: &str) -> anyhow::Result<Vec<u8>> {
    Ok(match ext.to_ascii_lowercase().as_str() {
        "pes" => pes::write_pes(d),
        "pec" => pes::write_pec_file(d),
        "dst" => dst::write_dst(d),
        "exp" => more::write_exp(d),
        "jef" => more::write_jef(d),
        "vp3" => more::write_vp3(d),
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

#[derive(Clone, Debug)]
pub struct StreamOptions {
    /// Add tie-in/tie-off stitches around trims and color changes.
    pub ties: bool,
    /// Longest single stitch or jump the format can encode, units.
    pub max_step: f64,
}

/// Flatten a design into a command stream: jump to each run, tie in, stitch,
/// tie off, trim. Coordinates are rounded to whole units.
pub fn commands(d: &Design, opt: &StreamOptions) -> Vec<Cmd> {
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
        if opt.ties {
            let d = at + dir * 4.0;
            for q in [r(d), at, r(d), at] {
                out.push(Cmd {
                    x: q.x,
                    y: q.y,
                    op: Op::Stitch,
                });
            }
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
            out.push(Cmd {
                x: p.x,
                y: p.y,
                op: Op::ColorChange,
            });
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
            }
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
            let first_dir = run
                .pts
                .iter()
                .map(|&q| r(q) - start)
                .find(|v| v.len() > 0.5)
                .map(|v| v.norm())
                .unwrap_or(P::new(1.0, 0.0));
            tie(&mut out, start, first_dir);
            let mut cur = start;
            for &q in &run.pts[1..] {
                let q = r(q);
                if q == cur {
                    continue;
                }
                prev_dir = (cur - q).norm();
                push_line(&mut out, cur, q, Op::Stitch);
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
        out.push(Cmd {
            x: p.x,
            y: p.y,
            op: Op::End,
        });
    }
    out
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
