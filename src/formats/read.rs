//! Reading machine files (PES, PEC, DST, EXP, JEF) into stitch blocks, and
//! turning them into an editable SVG of manual-stitch paths.
//! Decoding follows pyembroidery (MIT License, Copyright (c) 2018 Tatarize).

use super::jef_data::JEF_PALETTE;
use super::pec_data::PEC_PALETTE;
use crate::geom::{P, Polyline};
use anyhow::{Result, bail};
use std::fmt::Write;

/// One thread color's worth of stitching: runs separated by jumps/trims.
#[derive(Clone, Debug, Default)]
pub struct StitchBlock {
    pub color: [u8; 3],
    pub runs: Vec<Polyline>,
}

#[derive(Clone, Debug, Default)]
pub struct StitchFile {
    pub name: String,
    pub blocks: Vec<StitchBlock>,
}

/// Builder that tracks position and splits runs/blocks.
struct Builder {
    blocks: Vec<StitchBlock>,
    pos: P,
    jumped: bool,
}

impl Builder {
    fn new() -> Self {
        Builder {
            blocks: vec![StitchBlock::default()],
            pos: P::default(),
            jumped: true,
        }
    }
    fn stitch(&mut self, dx: f64, dy: f64) {
        let from = self.pos;
        self.pos = self.pos + P::new(dx, dy);
        let blk = self.blocks.last_mut().unwrap();
        if self.jumped || blk.runs.is_empty() {
            blk.runs.push(vec![from]);
            self.jumped = false;
        }
        blk.runs.last_mut().unwrap().push(self.pos);
    }
    fn jump(&mut self, dx: f64, dy: f64) {
        self.pos = self.pos + P::new(dx, dy);
        self.jumped = true;
    }
    fn color_change(&mut self) {
        self.blocks.push(StitchBlock::default());
        self.jumped = true;
    }
    fn finish(mut self, colors: &[[u8; 3]]) -> Vec<StitchBlock> {
        for (i, b) in self.blocks.iter_mut().enumerate() {
            b.color = colors
                .get(i)
                .copied()
                .unwrap_or(DEFAULT_COLORS[i % DEFAULT_COLORS.len()]);
            // Drop the single-point runs left by jumps.
            b.runs.retain(|r| r.len() >= 2);
        }
        self.blocks.retain(|b| !b.runs.is_empty());
        self.blocks
    }
}

/// Distinct colors for formats that don't store any (DST, EXP).
const DEFAULT_COLORS: [[u8; 3]; 8] = [
    [30, 30, 30],
    [200, 40, 40],
    [30, 90, 170],
    [240, 180, 30],
    [40, 140, 70],
    [130, 60, 150],
    [230, 120, 30],
    [90, 90, 90],
];

fn signed7(b: u8) -> i32 {
    if b > 63 { b as i32 - 128 } else { b as i32 }
}

fn signed12(v: u16) -> i32 {
    let v = (v & 0xFFF) as i32;
    if v > 0x7FF { v - 0x1000 } else { v }
}

fn i32le(d: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// Decode a PEC block starting at `start` (just after any "#PEC0001").
fn read_pec(d: &[u8], start: usize) -> Result<StitchFile> {
    let label_at = start + 3;
    let name = String::from_utf8_lossy(d.get(label_at..label_at + 16).unwrap_or_default())
        .trim()
        .to_string();
    let cc_at = label_at + 16 + 0xF + 2 + 0xC;
    let Some(&cc) = d.get(cc_at) else {
        bail!("PEC header is truncated")
    };
    let ncolors = cc as usize + 1;
    let palette: Vec<[u8; 3]> = (0..ncolors)
        .map(|i| {
            let idx = d.get(cc_at + 1 + i).copied().unwrap_or(0) as usize % PEC_PALETTE.len();
            let (r, g, b, _) = PEC_PALETTE[idx];
            [r, g, b]
        })
        .collect();
    let mut i = cc_at + 1 + ncolors + (0x1D0 - cc as usize);
    i += 3; // stitch block length (u24)
    i += 0x0B;
    let mut bld = Builder::new();
    while let (Some(&v1), Some(&v2)) = (d.get(i), d.get(i + 1)) {
        if v1 == 0xFF && v2 == 0x00 {
            break;
        }
        if v1 == 0xFE && v2 == 0xB0 {
            i += 3;
            bld.color_change();
            continue;
        }
        let (mut jump, mut trim) = (false, false);
        let x;
        let mut v2 = v2;
        if v1 & 0x80 != 0 {
            trim |= v1 & 0x20 != 0;
            jump |= v1 & 0x10 != 0;
            x = signed12(((v1 as u16) << 8) | v2 as u16);
            i += 2;
            let Some(&n) = d.get(i) else { break };
            v2 = n;
        } else {
            x = signed7(v1);
            i += 1;
        }
        let y;
        if v2 & 0x80 != 0 {
            trim |= v2 & 0x20 != 0;
            jump |= v2 & 0x10 != 0;
            let Some(&v3) = d.get(i + 1) else { break };
            y = signed12(((v2 as u16) << 8) | v3 as u16);
            i += 2;
        } else {
            y = signed7(v2);
            i += 1;
        }
        if jump || trim {
            bld.jump(x as f64, y as f64);
        } else {
            bld.stitch(x as f64, y as f64);
        }
    }
    Ok(StitchFile {
        name,
        blocks: bld.finish(&palette),
    })
}

fn read_dst(d: &[u8]) -> Result<StitchFile> {
    if d.len() < 512 {
        bail!("DST file is too short");
    }
    let header = String::from_utf8_lossy(&d[..512]);
    let mut name = String::new();
    let mut colors = Vec::new();
    for line in header.split(['\r', '\n']) {
        if let Some(v) = line.strip_prefix("LA:") {
            name = v.trim().to_string();
        }
        if let Some(v) = line.strip_prefix("TC:")
            && let Some(c) = crate::params::parse_hex(v.split(',').next().unwrap_or("").trim())
        {
            colors.push(c);
        }
    }
    let bit = |b: u8, p: u8| ((b >> p) & 1) as i32;
    let mut bld = Builder::new();
    for r in d[512..].chunks_exact(3) {
        let (b0, b1, b2) = (r[0], r[1], r[2]);
        let dx = bit(b2, 2) * 81 - bit(b2, 3) * 81 + bit(b1, 2) * 27 - bit(b1, 3) * 27
            + bit(b0, 2) * 9
            - bit(b0, 3) * 9
            + bit(b1, 0) * 3
            - bit(b1, 1) * 3
            + bit(b0, 0)
            - bit(b0, 1);
        let dy = -(bit(b2, 5) * 81 - bit(b2, 4) * 81 + bit(b1, 5) * 27 - bit(b1, 4) * 27
            + bit(b0, 5) * 9
            - bit(b0, 4) * 9
            + bit(b1, 7) * 3
            - bit(b1, 6) * 3
            + bit(b0, 7)
            - bit(b0, 6));
        if b2 & 0b1111_0011 == 0b1111_0011 {
            break;
        } else if b2 & 0b1100_0011 == 0b1100_0011 {
            bld.color_change();
            bld.jump(dx as f64, dy as f64);
        } else if b2 & 0b1000_0011 == 0b1000_0011 {
            bld.jump(dx as f64, dy as f64);
        } else {
            bld.stitch(dx as f64, dy as f64);
        }
    }
    Ok(StitchFile {
        name,
        blocks: bld.finish(&colors),
    })
}

/// EXP and JEF share the stitch encoding; `jump_code` differs (EXP 0x04, JEF 0x02).
fn read_exp_like(d: &[u8], jump_code: u8, colors: &[[u8; 3]]) -> Vec<StitchBlock> {
    let s8 = |b: u8| b as i8 as f64;
    let mut bld = Builder::new();
    let mut i = 0;
    while i + 1 < d.len() {
        let (b0, b1) = (d[i], d[i + 1]);
        i += 2;
        if b0 != 0x80 {
            bld.stitch(s8(b0), -s8(b1));
            continue;
        }
        let ctrl = b1;
        if ctrl == 0x10 {
            break;
        }
        if i + 1 >= d.len() {
            break;
        }
        let (x, y) = (s8(d[i]), -s8(d[i + 1]));
        i += 2;
        match ctrl {
            0x80 => bld.jumped = true,
            0x01 => {
                bld.color_change();
                if x != 0.0 || y != 0.0 {
                    bld.jump(x, y);
                }
            }
            0x02 if jump_code == 0x02 => bld.jump(x, y),
            0x02 => bld.stitch(x, y),
            0x04 => bld.jump(x, y),
            _ => break,
        }
    }
    bld.finish(colors)
}

fn read_jef(d: &[u8]) -> Result<StitchFile> {
    let Some(offset) = i32le(d, 0) else {
        bail!("JEF file is too short")
    };
    let ncolors = i32le(d, 24).unwrap_or(0).max(0) as usize;
    let colors: Vec<[u8; 3]> = (0..ncolors.min(256))
        .filter_map(|k| i32le(d, 116 + k * 4))
        .map(|idx| {
            let (r, g, b, _, _) = JEF_PALETTE[idx.unsigned_abs() as usize % JEF_PALETTE.len()];
            [r, g, b]
        })
        .collect();
    let body = d.get(offset as usize..).unwrap_or_default();
    Ok(StitchFile {
        name: String::new(),
        blocks: read_exp_like(body, 0x02, &colors),
    })
}

/// Read a machine file by extension.
pub fn read_stitch_file(data: &[u8], ext: &str) -> Result<StitchFile> {
    match ext.to_ascii_lowercase().as_str() {
        "pes" => {
            if !data.starts_with(b"#PES") {
                bail!("not a PES file");
            }
            let Some(off) = i32le(data, 8) else {
                bail!("PES header is truncated")
            };
            read_pec(data, off as usize)
        }
        "pec" => read_pec(data, 8),
        "dst" => read_dst(data),
        "exp" => Ok(StitchFile {
            name: String::new(),
            blocks: read_exp_like(data, 0x04, &[]),
        }),
        "jef" => read_jef(data),
        other => bail!("can't read .{other} files (PES, PEC, DST, EXP and JEF are supported)"),
    }
}

/// An SVG (mm units, origin at the design's center) with one manual-stitch
/// path per run, so the design can be moved, scaled and recolored as objects.
pub fn to_svg(f: &StitchFile) -> String {
    let mut b = crate::geom::Bounds::empty();
    for p in f.blocks.iter().flat_map(|b| b.runs.iter().flatten()) {
        b.add(*p);
    }
    let c = b.center();
    let (w, h) = ((b.width() / 10.0).max(1.0), (b.height() / 10.0).max(1.0));
    let mut s = String::new();
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.2}mm" height="{h:.2}mm" viewBox="{:.2} {:.2} {w:.2} {h:.2}">"#,
        -w / 2.0,
        -h / 2.0
    );
    for (bi, blk) in f.blocks.iter().enumerate() {
        let col = format!(
            "#{:02x}{:02x}{:02x}",
            blk.color[0], blk.color[1], blk.color[2]
        );
        let _ = writeln!(s, r#"  <g id="color_{}">"#, bi + 1);
        for (ri, run) in blk.runs.iter().enumerate() {
            let mut d = String::new();
            for (k, p) in run.iter().enumerate() {
                let q = *p - c;
                let _ = write!(
                    d,
                    "{}{:.1},{:.1} ",
                    if k == 0 { "M" } else { "L" },
                    q.x / 10.0,
                    q.y / 10.0
                );
            }
            let _ = writeln!(
                s,
                r#"    <path id="stitches_{}_{}_manual" d="{}" fill="none" stroke="{col}" stroke-width="0.4"/>"#,
                bi + 1,
                ri + 1,
                d.trim_end()
            );
        }
        s.push_str("  </g>\n");
    }
    s.push_str("</svg>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::{Block, Design, Run};
    use crate::formats::{dst, more, pes};

    fn sample() -> Design {
        let run = |pts: &[(f64, f64)]| Run {
            pts: pts.iter().map(|&(x, y)| P::new(x, y)).collect(),
            trim_before: true,
        };
        Design {
            name: "t".into(),
            blocks: vec![
                Block {
                    color: [237, 23, 31],
                    runs: vec![
                        run(&[(-200.0, 0.0), (-150.0, 30.0), (-100.0, 0.0)]),
                        run(&[(50.0, 50.0), (80.0, 50.0)]),
                    ],
                },
                Block {
                    color: [10, 85, 163],
                    runs: vec![run(&[(0.0, -300.0), (40.0, -260.0), (300.0, -300.0)])],
                },
            ],
            ..Design::default()
        }
    }

    /// Every writer's output reads back to the same stitch positions.
    #[test]
    fn writers_round_trip_through_readers() {
        let d = sample();
        for (ext, bytes) in [
            ("pes", pes::write_pes(&d)),
            ("dst", dst::write_dst(&d)),
            ("exp", more::write_exp(&d)),
            ("jef", more::write_jef(&d)),
        ] {
            let f = read_stitch_file(&bytes, ext).unwrap_or_else(|e| panic!("{ext}: {e}"));
            assert_eq!(f.blocks.len(), 2, "{ext}: two colors");
            let pts: Vec<P> = f
                .blocks
                .iter()
                .flat_map(|b| b.runs.iter().flatten().copied())
                .collect();
            for want in [
                P::new(-150.0, 30.0),
                P::new(80.0, 50.0),
                P::new(40.0, -260.0),
                P::new(300.0, -300.0),
            ] {
                assert!(
                    pts.iter().any(|p| p.dist(want) < 0.6),
                    "{ext}: missing {want:?}"
                );
            }
            if ext == "pes" || ext == "jef" {
                assert_ne!(f.blocks[0].color, f.blocks[1].color, "{ext}: colors kept");
            }
        }
    }

    #[test]
    fn imported_file_becomes_editable_document() {
        let bytes = pes::write_pes(&sample());
        let f = read_stitch_file(&bytes, "pes").unwrap();
        let svg = to_svg(&f);
        let doc = crate::document::Document::from_svg(&svg, "t", None).unwrap();
        assert!(
            doc.shapes
                .iter()
                .all(|s| s.params.kind == Some(crate::params::Kind::Manual))
        );
        let d = doc.plan();
        assert!(d.stitch_count() >= 8);
        assert_eq!(d.blocks.len(), 2);
    }
}
