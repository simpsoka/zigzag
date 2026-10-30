//! More machine formats: Melco/Bernina EXP, Janome JEF and Husqvarna/Pfaff VP3.
//! Byte layouts ported from pyembroidery (MIT License, Copyright (c) 2018 Tatarize).

use super::jef_data::JEF_PALETTE;
use super::{Cmd, Op, StreamOptions, color_distance, commands, stream_bounds};
use crate::design::Design;

fn stream(d: &Design, max_step: f64) -> Vec<Cmd> {
    commands(
        d,
        &StreamOptions {
            ties: true,
            max_step,
        },
    )
}

fn deltas(cmds: &[Cmd]) -> impl Iterator<Item = (i64, i64, Op)> + '_ {
    let (mut xx, mut yy) = (0i64, 0i64);
    cmds.iter().map(move |c| {
        let (x, y) = (c.x.round() as i64, c.y.round() as i64);
        let (dx, dy) = (x - xx, y - yy);
        xx = x;
        yy = y;
        (dx, dy, c.op)
    })
}

/// Melco / Bernina EXP.
pub fn write_exp(d: &Design) -> Vec<u8> {
    let cmds = stream(d, 127.0);
    let mut f = Vec::with_capacity(cmds.len() * 2 + 16);
    for (dx, dy, op) in deltas(&cmds) {
        match op {
            Op::Stitch => f.extend_from_slice(&[dx as u8, (-dy) as u8]),
            Op::Jump => f.extend_from_slice(&[0x80, 0x04, dx as u8, (-dy) as u8]),
            Op::Trim => f.extend_from_slice(&[0x80, 0x80, 0x07, 0x00]),
            Op::ColorChange => f.extend_from_slice(&[0x80, 0x01, 0x00, 0x00]),
            Op::End => break,
        }
    }
    f
}

/// Nearest Janome thread for each color block, never giving two consecutive
/// different colors the same thread (so the machine still stops between them).
fn jef_palette(threads: &[[u8; 3]]) -> Vec<i32> {
    let mut out = Vec::new();
    let mut last: Option<(usize, [u8; 3])> = None;
    for &t in threads {
        let nearest = |skip: Option<usize>| {
            (1..JEF_PALETTE.len())
                .filter(|&i| Some(i) != skip)
                .min_by_key(|&i| {
                    let (r, g, b, _, _) = JEF_PALETTE[i];
                    color_distance(t, [r, g, b])
                })
                .unwrap_or(1)
        };
        let mut i = nearest(None);
        if let Some((li, lt)) = last
            && li == i
            && lt != t
        {
            i = nearest(Some(li));
        }
        out.push(i as i32);
        last = Some((i, t));
    }
    out
}

fn jef_hoop(w: i64, h: i64) -> i32 {
    match (w, h) {
        (w, h) if w < 500 && h < 500 => 1,
        (w, h) if w < 1260 && h < 1100 => 3,
        (w, h) if w < 1400 && h < 2000 => 2,
        (w, h) if w < 2000 && h < 2000 => 4,
        _ => 0,
    }
}

/// Janome JEF.
pub fn write_jef(d: &Design) -> Vec<u8> {
    let cmds = stream(d, 127.0);
    let threads: Vec<[u8; 3]> = d.blocks.iter().map(|b| b.color).collect();
    let palette = jef_palette(&threads);
    let colors = palette.len().max(1);
    let mut f = Vec::new();
    let i32le = |f: &mut Vec<u8>, v: i64| f.extend_from_slice(&(v as i32).to_le_bytes());
    i32le(&mut f, 0x74 + colors as i64 * 8);
    i32le(&mut f, 0x14);
    f.extend_from_slice(b"20260101000000");
    f.extend_from_slice(&[0, 0]);
    i32le(&mut f, colors as i64);
    let points: i64 = 1 + cmds
        .iter()
        .map(|c| match c.op {
            Op::Stitch => 1,
            Op::Jump | Op::ColorChange => 2,
            _ => 0,
        })
        .sum::<i64>();
    i32le(&mut f, points);
    let b = stream_bounds(&cmds);
    let (w, h) = (b.width().round() as i64, b.height().round() as i64);
    i32le(&mut f, jef_hoop(w, h) as i64);
    let (hw, hh) = (
        (w as f64 / 2.0).round() as i64,
        (h as f64 / 2.0).round() as i64,
    );
    for v in [hw, hh, hw, hh] {
        i32le(&mut f, v);
    }
    for (ex, ey) in [(550, 550), (250, 250), (700, 1000), (700, 1000)] {
        let (x, y) = (ex - hw, ey - hh);
        let vals = if x.min(y) >= 0 {
            [x, y, x, y]
        } else {
            [-1, -1, -1, -1]
        };
        for v in vals {
            i32le(&mut f, v);
        }
    }
    for p in &palette {
        i32le(&mut f, *p as i64);
    }
    for _ in 0..palette.len() {
        i32le(&mut f, 0x0D);
    }
    for (dx, dy, op) in deltas(&cmds) {
        match op {
            Op::Stitch => f.extend_from_slice(&[dx as u8, (-dy) as u8]),
            Op::ColorChange => f.extend_from_slice(&[0x80, 0x01, dx as u8, (-dy) as u8]),
            Op::Jump => f.extend_from_slice(&[0x80, 0x02, dx as u8, (-dy) as u8]),
            Op::Trim => {}
            Op::End => break,
        }
    }
    f.extend_from_slice(&[0x80, 0x10]);
    f
}

/// Husqvarna Viking / Pfaff VP3.
pub fn write_vp3(d: &Design) -> Vec<u8> {
    let cmds = stream(d, 255.0);
    let mut f: Vec<u8> = Vec::new();
    let be16 = |f: &mut Vec<u8>, v: i64| f.extend_from_slice(&(v as i16).to_be_bytes());
    let be32 = |f: &mut Vec<u8>, v: i64| f.extend_from_slice(&(v as i32).to_be_bytes());
    fn str16(f: &mut Vec<u8>, s: &str) {
        let units: Vec<u16> = s.encode_utf16().collect();
        f.extend_from_slice(&((units.len() * 2) as u16).to_be_bytes());
        for u in units {
            f.extend_from_slice(&u.to_be_bytes());
        }
    }
    fn str8(f: &mut Vec<u8>, s: &str) {
        f.extend_from_slice(&(s.len() as u16).to_be_bytes());
        f.extend_from_slice(s.as_bytes());
    }
    fn patch(f: &mut [u8], at: usize) {
        let dist = (f.len() - at - 4) as u32;
        f[at..at + 4].copy_from_slice(&dist.to_be_bytes());
    }

    f.extend_from_slice(b"%vsm%\0");
    str16(&mut f, "Produced by     Software Ltd");
    f.extend_from_slice(&[0x00, 0x02, 0x00]);
    let file_block = f.len();
    be32(&mut f, 0);
    str16(&mut f, "");
    let b = stream_bounds(&cmds);
    be32(&mut f, (b.max.x * 100.0) as i64);
    be32(&mut f, (b.min.y * -100.0) as i64);
    be32(&mut f, (b.min.x * 100.0) as i64);
    be32(&mut f, (b.max.y * -100.0) as i64);
    // Color blocks: split at color changes.
    let mut blocks: Vec<&[Cmd]> = Vec::new();
    let mut start = 0;
    for (i, c) in cmds.iter().enumerate() {
        if c.op == Op::ColorChange {
            blocks.push(&cmds[start..i]);
            start = i;
        }
    }
    blocks.push(&cmds[start..]);
    let ends = cmds.iter().filter(|c| c.op == Op::End).count();
    be32(&mut f, (cmds.len() - ends) as i64);
    f.extend_from_slice(&[0, blocks.len() as u8, 12, 0, 1]);

    // Design block.
    f.extend_from_slice(&[0x00, 0x03, 0x00]);
    let design_block = f.len();
    be32(&mut f, 0);
    let (w, h) = (b.width(), b.height());
    let (cx, cy) = (b.max.x - w / 2.0, b.max.y - h / 2.0);
    be32(&mut f, (cx as i64) * 100);
    be32(&mut f, (cy as i64) * -100);
    f.extend_from_slice(&[0, 0, 0]);
    be32(&mut f, ((w / 2.0) as i64) * -100);
    be32(&mut f, ((w / 2.0) as i64) * 100);
    be32(&mut f, ((h / 2.0) as i64) * -100);
    be32(&mut f, ((h / 2.0) as i64) * 100);
    be32(&mut f, (w as i64) * 100);
    be32(&mut f, (h as i64) * 100);
    str16(&mut f, "");
    f.extend_from_slice(b"\x64\x64");
    for v in [4096, 0, 0, 4096] {
        be32(&mut f, v);
    }
    f.extend_from_slice(b"xxPP\x01\x00");
    str16(&mut f, "Produced by     Software Ltd");
    be16(&mut f, blocks.len() as i64);
    for (bi, block) in blocks.iter().enumerate() {
        f.extend_from_slice(&[0x00, 0x05, 0x00]);
        let color_block = f.len();
        be32(&mut f, 0);
        let (fx, fy) = match block.first() {
            Some(c) if bi > 0 => (c.x, c.y),
            _ => (0.0, 0.0),
        };
        let (lx, ly) = block.last().map(|c| (c.x, c.y)).unwrap_or((0.0, 0.0));
        be32(&mut f, ((fx - cx) as i64) * 100);
        be32(&mut f, (-(fy - cy) as i64) * 100);
        let color = d.blocks.get(bi).map(|b| b.color).unwrap_or([0, 0, 0]);
        f.extend_from_slice(&[
            0x01, 0x00, color[0], color[1], color[2], 0, 0, 0, 0x05, 0x28,
        ]);
        str8(&mut f, "");
        str8(
            &mut f,
            &format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2]),
        );
        str8(&mut f, "");
        be32(&mut f, ((lx - fx) as i64) * 100);
        be32(&mut f, (-(ly - fy) as i64) * 100);
        // Stitches.
        f.extend_from_slice(&[0x00, 0x01, 0x00]);
        let stitch_block = f.len();
        be32(&mut f, 0);
        f.extend_from_slice(&[0x0A, 0xF6, 0x00]);
        let (mut px, mut py) = (fx, fy);
        for c in block.iter() {
            match c.op {
                Op::End => {
                    f.extend_from_slice(&[0x80, 0x03]);
                    break;
                }
                Op::Trim => f.extend_from_slice(&[0x80, 0x03]),
                Op::ColorChange | Op::Jump => {}
                Op::Stitch => {
                    let dx = (c.x - px) as i64;
                    let dy = (c.y - py) as i64;
                    px += dx as f64;
                    py += dy as f64;
                    if (-127..=127).contains(&dx) && (-127..=127).contains(&dy) {
                        f.extend_from_slice(&[dx as u8, dy as u8]);
                    } else {
                        f.extend_from_slice(&[0x80, 0x01]);
                        be16(&mut f, dx);
                        be16(&mut f, dy);
                        f.extend_from_slice(&[0x80, 0x02]);
                    }
                }
            }
        }
        patch(&mut f, stitch_block);
        f.push(0);
        patch(&mut f, color_block);
    }
    patch(&mut f, design_block);
    patch(&mut f, file_block);
    f
}
