//! Reading machine files (PES, PEC, DST, EXP, JEF, VP3) into stitch blocks, and
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

/// Does the PEC stitch data at `i` start with the design box offset
/// (`0x9000 | -left`, `0x9000 | -top`)? Brother's own files always have it.
/// PES files ZigZag wrote before it was added go straight into the
/// stitches, and their first record (a jump) has the same form, so check
/// the geometry: with the offset, the stitches that follow reach exactly to
/// the box's top-left corner.
fn has_box_offset(d: &[u8], i: usize) -> bool {
    let Some(w) = d.get(i..i + 4) else {
        return false;
    };
    if w[0] & 0x80 == 0 || w[2] & 0x80 == 0 {
        return false;
    }
    let left = -signed12(u16::from_be_bytes([w[0], w[1]]));
    let top = -signed12(u16::from_be_bytes([w[2], w[3]]));
    let (mut x, mut y) = (0i32, 0i32);
    let (mut min_x, mut min_y) = (i32::MAX, i32::MAX);
    let mut k = i + 4;
    while let Some(&v1) = d.get(k) {
        if v1 == 0xFF {
            break;
        }
        if v1 == 0xFE && d.get(k + 1) == Some(&0xB0) {
            k += 3;
            continue;
        }
        let mut delta = [0i32; 2];
        for v in &mut delta {
            let Some(&b) = d.get(k) else { return false };
            if b & 0x80 != 0 {
                let Some(&b2) = d.get(k + 1) else {
                    return false;
                };
                *v = signed12(u16::from_be_bytes([b, b2]));
                k += 2;
            } else {
                *v = signed7(b);
                k += 1;
            }
        }
        x += delta[0];
        y += delta[1];
        min_x = min_x.min(x);
        min_y = min_y.min(y);
    }
    // A unit or two of slack for writers that round the box differently.
    min_x != i32::MAX && (min_x - left).abs() <= 2 && (min_y - top).abs() <= 2
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
    if has_box_offset(d, i) {
        i += 4;
    }
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

/// Husqvarna Viking / Pfaff VP3. Positions are in 1/1000 mm (stored
/// big-endian, y up); stitch deltas are in 0.1 mm (y down). Layout as in
/// pyembroidery's vp3reader.
fn read_vp3(d: &[u8]) -> Result<StitchFile> {
    struct Rd<'a> {
        d: &'a [u8],
        i: usize,
    }
    impl Rd<'_> {
        fn skip(&mut self, n: usize) -> Result<()> {
            if self.i + n > self.d.len() {
                bail!("VP3 file is truncated");
            }
            self.i += n;
            Ok(())
        }
        fn u8(&mut self) -> Result<u8> {
            let v = *self
                .d
                .get(self.i)
                .ok_or_else(|| anyhow::anyhow!("VP3 file is truncated"))?;
            self.i += 1;
            Ok(v)
        }
        fn u16(&mut self) -> Result<u16> {
            Ok(((self.u8()? as u16) << 8) | self.u8()? as u16)
        }
        fn i32(&mut self) -> Result<i32> {
            let b = self
                .d
                .get(self.i..self.i + 4)
                .ok_or_else(|| anyhow::anyhow!("VP3 file is truncated"))?;
            self.i += 4;
            Ok(i32::from_be_bytes(b.try_into().unwrap()))
        }
        /// A string with a 16-bit byte length in front.
        fn skip_str(&mut self) -> Result<()> {
            let n = self.u16()? as usize;
            self.skip(n)
        }
    }
    if !d.starts_with(b"%vsm%") {
        bail!("not a VP3 file");
    }
    let mut r = Rd { d, i: 6 };
    r.skip_str()?; // "Produced by …"
    r.skip(7)?;
    r.skip_str()?; // comments
    r.skip(32)?;
    let cx = r.i32()? as f64 / 100.0;
    let cy = -(r.i32()? as f64) / 100.0;
    r.skip(27)?;
    r.skip_str()?;
    r.skip(24)?;
    r.skip_str()?; // "Produced by …"
    let ncolors = r.u16()? as usize;
    let mut bld = Builder::new();
    let mut colors = Vec::new();
    for ci in 0..ncolors {
        if ci > 0 {
            bld.color_change();
        }
        r.skip(3)?; // 00 05 00
        let len = r.i32()?.max(0) as usize;
        let end = (r.i + len).min(d.len());
        let sx = r.i32()? as f64 / 100.0;
        let sy = -(r.i32()? as f64) / 100.0;
        let target = P::new(sx + cx, sy + cy);
        let here = bld.pos;
        bld.jump(target.x - here.x, target.y - here.y);
        // Thread: its colors (the first one counts), type, weight, three strings.
        let parts = r.u8()?;
        r.u8()?; // transition
        let mut color = None;
        for _ in 0..parts {
            let c = [r.u8()?, r.u8()?, r.u8()?];
            color.get_or_insert(c);
            r.skip(3)?; // parts, length
        }
        colors.push(color.unwrap_or(DEFAULT_COLORS[ci % DEFAULT_COLORS.len()]));
        r.skip(2)?; // type, weight
        for _ in 0..3 {
            r.skip_str()?;
        }
        r.skip(15)?;
        r.skip(3)?; // 0A F6 00
        let s = d.get(r.i..end).unwrap_or_default();
        let mut i = 0;
        let mut trimmed = false;
        let step = |bld: &mut Builder, x: f64, y: f64, trimmed: &mut bool| {
            // The move after a trim, or one too long to be a stitch, is a jump.
            if *trimmed || x.abs() > 127.0 || y.abs() > 127.0 {
                bld.jump(x, y);
            } else {
                bld.stitch(x, y);
            }
            *trimmed = false;
        };
        while i + 1 < s.len() {
            let (x, y) = (s[i], s[i + 1]);
            i += 2;
            if x != 0x80 {
                step(&mut bld, x as i8 as f64, y as i8 as f64, &mut trimmed);
                continue;
            }
            match y {
                0x01 => {
                    let Some(b) = s.get(i..i + 4) else { break };
                    let dx = i16::from_be_bytes([b[0], b[1]]) as f64;
                    let dy = i16::from_be_bytes([b[2], b[3]]) as f64;
                    i += 4;
                    step(&mut bld, dx, dy, &mut trimmed);
                }
                0x02 => {}
                0x03 => {
                    trimmed = true;
                    bld.jumped = true;
                }
                _ => {}
            }
        }
        r.i = end;
    }
    Ok(StitchFile {
        name: String::new(),
        blocks: bld.finish(&colors),
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
            let Ok(off) = usize::try_from(off) else {
                bail!("PES header points before the start of the file")
            };
            read_pec(data, off)
        }
        "pec" => read_pec(data, 8),
        "dst" => read_dst(data),
        "exp" => Ok(StitchFile {
            name: String::new(),
            blocks: read_exp_like(data, 0x04, &[]),
        }),
        "jef" => read_jef(data),
        "vp3" => read_vp3(data),
        other => bail!("can't read .{other} files (PES, PEC, DST, EXP, JEF and VP3 are supported)"),
    }
}

/// An SVG (mm units, origin at the design's center) with one manual-stitch
/// path per run, so the design can be moved, scaled and recolored as objects.
pub fn to_svg(f: &StitchFile) -> String {
    let mut b = crate::geom::Bounds::empty();
    for p in f.blocks.iter().flat_map(|b| b.runs.iter().flatten()) {
        b.add(*p);
    }
    // Stitches sit on whole 0.1 mm units: so does the center, or every
    // coordinate would be written (rounded) half a unit off.
    let c = b.center();
    let c = P::new(c.x.round(), c.y.round());
    let (w, h) = ((b.width() / 10.0).max(1.0), (b.height() / 10.0).max(1.0));
    let mut s = String::new();
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.2}mm" height="{h:.2}mm" viewBox="{:.2} {:.2} {w:.2} {h:.2}">"#,
        (b.min.x - c.x) / 10.0,
        (b.min.y - c.y) / 10.0
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
            ("vp3", more::write_vp3(&d)),
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
            if ext == "pes" || ext == "jef" || ext == "vp3" {
                assert_ne!(f.blocks[0].color, f.blocks[1].color, "{ext}: colors kept");
            }
        }
    }

    #[test]
    fn reopening_and_exporting_a_file_keeps_every_stitch_in_place() {
        let mut d = sample();
        // An odd width: the artwork's center falls on a half unit.
        d.blocks[1].runs[0].pts.push(P::new(301.0, -297.0));
        let f = read_stitch_file(&dst::write_dst(&d), "dst").unwrap();
        let doc = crate::document::Document::from_svg(&to_svg(&f), "t", None).unwrap();
        let g = read_stitch_file(&dst::write_dst(&doc.plan()), "dst").unwrap();
        let pts = |f: &StitchFile| -> Vec<P> {
            f.blocks
                .iter()
                .flat_map(|b| b.runs.iter().flatten().copied())
                .collect()
        };
        let (a, b) = (pts(&f), pts(&g));
        let bounds = |v: &[P]| {
            let mut b = crate::geom::Bounds::empty();
            v.iter().for_each(|p| b.add(*p));
            b
        };
        let shift = bounds(&b).min - bounds(&a).min;
        for p in &a {
            assert!(
                b.iter().any(|q| (*q - shift).dist(*p) < 1e-6),
                "{p:?} moved (shift {shift:?})"
            );
        }
    }

    /// Where the PEC stitch block starts in a PES file.
    fn pec_stitch_block(pes: &[u8]) -> usize {
        let g = i32le(pes, 8).unwrap() as usize + 512;
        assert_eq!(&pes[g + 5..g + 8], &[0x31, 0xFF, 0xF0]);
        g
    }

    /// Absolute positions of every PEC move from `k`, starting at the origin.
    fn pec_positions(d: &[u8], mut k: usize) -> Vec<(i32, i32)> {
        let (mut x, mut y, mut out) = (0, 0, Vec::new());
        while d[k] != 0xFF {
            if d[k] == 0xFE && d[k + 1] == 0xB0 {
                k += 3;
                continue;
            }
            let mut delta = [0i32; 2];
            for v in &mut delta {
                if d[k] & 0x80 != 0 {
                    *v = signed12(u16::from_be_bytes([d[k], d[k + 1]]));
                    k += 2;
                } else {
                    *v = signed7(d[k]);
                    k += 1;
                }
            }
            x += delta[0];
            y += delta[1];
            out.push((x, y));
        }
        out
    }

    /// A Brother machine reads the design box offset from the first two
    /// words of the stitch block, decodes stitches after them and only sews
    /// what lies inside the box. Every stitch must land inside it.
    #[test]
    fn pes_stitches_lie_inside_the_design_box_a_brother_machine_reads() {
        let pes = pes::write_pes(&sample());
        let g = pec_stitch_block(&pes);
        let w = u16::from_le_bytes([pes[g + 8], pes[g + 9]]) as i32;
        let h = u16::from_le_bytes([pes[g + 10], pes[g + 11]]) as i32;
        let word = |at: usize| u16::from_be_bytes([pes[at], pes[at + 1]]);
        assert_eq!(word(g + 16) & 0xF000, 0x9000, "x offset word");
        assert_eq!(word(g + 18) & 0xF000, 0x9000, "y offset word");
        let (left, top) = (-signed12(word(g + 16)), -signed12(word(g + 18)));
        let pts = pec_positions(&pes, g + 20);
        assert!(!pts.is_empty());
        let min_x = pts.iter().map(|p| p.0).min().unwrap();
        let min_y = pts.iter().map(|p| p.1).min().unwrap();
        assert_eq!(
            (left, top),
            (min_x, min_y),
            "box corner is the top-left stitch"
        );
        for &(x, y) in &pts {
            assert!(
                (left..=left + w).contains(&x) && (top..=top + h).contains(&y),
                "({x}, {y}) is outside the box ({left}, {top}) {w}x{h}"
            );
        }
    }

    /// PES files written before the box offset was added still open with
    /// every stitch in the same place.
    #[test]
    fn reads_pes_written_without_the_design_box_offset() {
        let new = pes::write_pes(&sample());
        let g = pec_stitch_block(&new);
        let mut old = new.clone();
        old.drain(g + 16..g + 20);
        let len = u32::from_le_bytes([old[g + 2], old[g + 3], old[g + 4], 0]) - 4;
        old[g + 2..g + 5].copy_from_slice(&len.to_le_bytes()[..3]);
        let a = read_stitch_file(&new, "pes").unwrap();
        let b = read_stitch_file(&old, "pes").unwrap();
        assert_eq!(a.blocks.len(), b.blocks.len());
        for (x, y) in a.blocks.iter().zip(&b.blocks) {
            assert_eq!(x.runs, y.runs);
        }
    }

    #[test]
    fn a_negative_pec_offset_is_an_error_not_a_crash() {
        let mut f = b"#PES0001".to_vec();
        f.extend_from_slice(&(-8i32).to_le_bytes());
        f.extend_from_slice(&[0; 32]);
        assert!(read_stitch_file(&f, "pes").is_err());
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
