//! Brother PES (version 1) and its embedded PEC stitch block.
//!
//! Byte layout ported from pyembroidery's PesWriter/PecWriter/PecGraphics
//! (MIT License, Copyright (c) 2018 Tatarize).

use super::pec_data::PEC_ICON_BLANK;
use super::{Cmd, Op, StreamOptions, commands, nearest_index, pec_palette, stream_bounds};
use crate::design::Design;
use crate::geom::Bounds;

pub const MAX_STEP: f64 = 2047.0;

pub fn stream(d: &Design) -> Vec<Cmd> {
    commands(
        d,
        &StreamOptions {
            ties: true,
            max_step: 120.0,
        },
    )
}

pub fn write_pes(d: &Design) -> Vec<u8> {
    let cmds = stream(d);
    let threads: Vec<[u8; 3]> = d.blocks.iter().map(|b| b.color).collect();
    pes_from_stream(&d.name, &cmds, &threads)
}

pub fn write_pec_file(d: &Design) -> Vec<u8> {
    let cmds = stream(d);
    let threads: Vec<[u8; 3]> = d.blocks.iter().map(|b| b.color).collect();
    let mut f = b"#PEC0001".to_vec();
    write_pec(&mut f, &d.name, &cmds, &threads);
    f
}

fn i16le(f: &mut Vec<u8>, v: i64) {
    f.extend_from_slice(&(v as i16 as u16).to_le_bytes());
}
fn u24le(f: &mut Vec<u8>, v: usize) {
    f.extend_from_slice(&(v as u32).to_le_bytes()[..3]);
}
fn f32le(f: &mut Vec<u8>, v: f64) {
    f.extend_from_slice(&(v as f32).to_le_bytes());
}

pub fn pes_from_stream(name: &str, cmds: &[Cmd], threads: &[[u8; 3]]) -> Vec<u8> {
    let mut f: Vec<u8> = Vec::with_capacity(cmds.len() * 4 + 4096);
    f.extend_from_slice(b"#PES0001");
    let b = stream_bounds(cmds);
    let (cx, cy) = ((b.max.x + b.min.x) / 2.0, (b.max.y + b.min.y) / 2.0);
    let (left, top, right, bottom) = (b.min.x - cx, b.min.y - cy, b.max.x - cx, b.max.y - cy);

    let pec_ptr = f.len();
    f.extend_from_slice(&[0; 4]);
    let has_stitches = !cmds.is_empty();
    // Header v1: scale-to-fit, hoop 130x180, number of distinct block objects.
    i16le(&mut f, 1);
    i16le(&mut f, 1);
    i16le(&mut f, has_stitches as i64);
    if has_stitches {
        i16le(&mut f, 0xFFFF);
        i16le(&mut f, 0);
        write_pes_blocks(&mut f, cmds, threads, left, top, right, bottom, cx, cy);
    } else {
        i16le(&mut f, 0);
        i16le(&mut f, 0);
    }
    let pos = f.len() as u32;
    f[pec_ptr..pec_ptr + 4].copy_from_slice(&pos.to_le_bytes());
    write_pec(&mut f, name, cmds, threads);
    f
}

fn write_pes_string_16(f: &mut Vec<u8>, s: &str) {
    i16le(f, s.len() as i64);
    f.extend_from_slice(s.as_bytes());
}

#[allow(clippy::too_many_arguments)]
fn write_pes_blocks(
    f: &mut Vec<u8>,
    cmds: &[Cmd],
    threads: &[[u8; 3]],
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
    cx: f64,
    cy: f64,
) {
    write_pes_string_16(f, "CEmbOne");
    // CSewSeg header.
    let (width, height) = (right - left, bottom - top);
    let (hoop_w, hoop_h) = (1300.0, 1800.0);
    for _ in 0..8 {
        i16le(f, 0);
    }
    let trans_x = 350.0 + hoop_w / 2.0 - width / 2.0;
    let trans_y = 100.0 + height + hoop_h / 2.0 - height / 2.0;
    for v in [1.0, 0.0, 0.0, 1.0, trans_x, trans_y] {
        f32le(f, v);
    }
    i16le(f, 1);
    i16le(f, 0);
    i16le(f, 0);
    i16le(f, width as i64);
    i16le(f, height as i64);
    f.extend_from_slice(&[0; 8]);
    let sections_ptr = f.len();
    i16le(f, 0);
    i16le(f, 0xFFFF);
    i16le(f, 0); // more blocks follow

    write_pes_string_16(f, "CSewSeg");
    let chart = pec_palette();
    let (adjust_x, adjust_y) = (left + cx, bottom + cy);
    let mut color_index = 0usize;
    let thread = |i: usize| threads.get(i).copied().unwrap_or([0, 0, 0]);
    let mut color_code = nearest_index(thread(color_index), &chart).unwrap_or(1);
    color_index += 1;
    let (mut sx, mut sy) = (0.0, 0.0);
    let mut section = 0i64;
    let mut colorlog: Vec<(i64, usize)> = Vec::new();
    let mut prev_code: Option<usize> = None;
    let mut first = true;
    for block in command_blocks(cmds) {
        let (pts, flag): (Vec<(f64, f64)>, i64) = match block[0].op {
            Op::Jump => {
                let last = block.last().unwrap();
                (
                    vec![
                        (sx - adjust_x, sy - adjust_y),
                        (last.x - adjust_x, last.y - adjust_y),
                    ],
                    1,
                )
            }
            Op::ColorChange => {
                color_code = nearest_index(thread(color_index), &chart).unwrap_or(1);
                color_index += 1;
                continue;
            }
            Op::Stitch => {
                let pts = block
                    .iter()
                    .map(|c| {
                        (sx, sy) = (c.x, c.y);
                        (c.x - adjust_x, c.y - adjust_y)
                    })
                    .collect();
                (pts, 0)
            }
            _ => continue,
        };
        if !first {
            i16le(f, 0x8003); // section end
        }
        first = false;
        if prev_code != Some(color_code) {
            colorlog.push((section, color_code));
            prev_code = Some(color_code);
        }
        i16le(f, flag);
        i16le(f, color_code as i64);
        i16le(f, pts.len() as i64);
        for (x, y) in pts {
            i16le(f, x as i64);
            i16le(f, y as i64);
        }
        section += 1;
    }
    i16le(f, colorlog.len() as i64);
    for (s, c) in &colorlog {
        i16le(f, *s);
        i16le(f, *c as i64);
    }
    f[sections_ptr..sections_ptr + 2].copy_from_slice(&(section as u16).to_le_bytes());
    i16le(f, 0);
    i16le(f, 0); // no more blocks
}

/// Consecutive runs of the same command (pyembroidery's `get_as_command_blocks`).
fn command_blocks(cmds: &[Cmd]) -> Vec<&[Cmd]> {
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..cmds.len() {
        if cmds[i].op != cmds[i - 1].op {
            out.push(&cmds[start..i]);
            start = i;
        }
    }
    if start < cmds.len() {
        out.push(&cmds[start..]);
    }
    out
}

pub fn write_pec(f: &mut Vec<u8>, name: &str, cmds: &[Cmd], threads: &[[u8; 3]]) {
    let b = stream_bounds(cmds);
    // Header.
    let short: String = name.chars().filter(|c| c.is_ascii()).take(8).collect();
    f.extend_from_slice(format!("LA:{short:<16}\r").as_bytes());
    f.extend_from_slice(&[0x20; 12]);
    f.extend_from_slice(&[0xFF, 0x00, 6, 38]);
    let indices = unique_palette(threads);
    if !indices.is_empty() {
        f.extend_from_slice(&[0x20; 12]);
        f.push((indices.len() - 1) as u8);
        f.extend(indices.iter().map(|&i| i as u8));
    } else {
        f.extend_from_slice(b"\x20\x20\x20\x20\x64\x20\x00\x20\x00\x20\x20\x20\xFF");
    }
    for _ in indices.len()..463 {
        f.push(0x20);
    }

    // Stitch block.
    let start = f.len();
    f.extend_from_slice(&[0, 0]);
    u24le(f, 0);
    f.extend_from_slice(&[0x31, 0xFF, 0xF0]);
    i16le(f, b.width().round() as i64);
    i16le(f, b.height().round() as i64);
    i16le(f, 0x1E0);
    i16le(f, 0x1B0);
    pec_encode(f, cmds);
    let len = f.len() - start;
    f[start + 2..start + 5].copy_from_slice(&(len as u32).to_le_bytes()[..3]);

    // Thumbnails: the whole design, then one per color.
    let mut icon = PEC_ICON_BLANK;
    draw_scaled(
        &b,
        cmds.iter().filter(|c| c.op == Op::Stitch),
        &mut icon,
        4.0,
    );
    f.extend_from_slice(&icon);
    for block in color_blocks(cmds) {
        let mut icon = PEC_ICON_BLANK;
        draw_scaled(
            &b,
            block.iter().filter(|c| c.op == Op::Stitch),
            &mut icon,
            5.0,
        );
        f.extend_from_slice(&icon);
    }
}

fn color_blocks(cmds: &[Cmd]) -> Vec<&[Cmd]> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in cmds.iter().enumerate() {
        if c.op == Op::ColorChange {
            out.push(&cmds[start..i]);
            start = i + 1;
        }
    }
    out.push(&cmds[start..]);
    out
}

/// Assign each distinct thread color its own PEC palette slot (no reuse),
/// then map every thread to its slot. Mirrors pyembroidery's `build_unique_palette`.
fn unique_palette(threads: &[[u8; 3]]) -> Vec<usize> {
    let mut palette = pec_palette();
    let mut chart: Vec<Option<[u8; 3]>> = vec![None; palette.len()];
    let mut seen: Vec<[u8; 3]> = Vec::new();
    for &t in threads {
        if seen.contains(&t) {
            continue;
        }
        seen.push(t);
        let Some(i) = nearest_index(t, &palette) else {
            break;
        };
        palette[i] = None;
        chart[i] = Some(t);
    }
    threads
        .iter()
        .map(|&t| nearest_index(t, &chart).unwrap_or(0))
        .collect()
}

fn draw_scaled<'a>(
    b: &Bounds,
    pts: impl Iterator<Item = &'a Cmd>,
    icon: &mut [u8; 228],
    buffer: f64,
) {
    let (w, h) = (b.width().max(1.0), b.height().max(1.0));
    let scale = ((48.0 - buffer) / w).min((38.0 - buffer) / h);
    let c = b.center();
    let (tx, ty) = (-c.x * scale + 24.0, -c.y * scale + 19.0);
    for p in pts {
        let x = (p.x * scale + tx).floor() as i64;
        let y = (p.y * scale + ty).floor() as i64;
        if (0..48).contains(&x) && (0..38).contains(&y) {
            icon[(y * 6 + x / 8) as usize] |= 1 << (x % 8);
        }
    }
}

fn pec_value(f: &mut Vec<u8>, v: i64, long: bool, flag: u16) {
    if !long && -64 < v && v < 63 {
        f.push((v & 0x7F) as u8);
    } else {
        let mut x = (v as u16) & 0x0FFF;
        x |= 0x8000;
        x |= flag << 8;
        f.extend_from_slice(&x.to_be_bytes());
    }
}

/// PEC stitch encoding (pyembroidery `pec_encode`).
fn pec_encode(f: &mut Vec<u8>, cmds: &[Cmd]) {
    const JUMP_CODE: u16 = 0x10;
    const TRIM_CODE: u16 = 0x20;
    let mut color_two = true;
    let mut jumping = true;
    let mut init = true;
    let (mut xx, mut yy) = (0i64, 0i64);
    for c in cmds {
        let dx = c.x.round() as i64 - xx;
        let dy = c.y.round() as i64 - yy;
        xx += dx;
        yy += dy;
        match c.op {
            Op::Stitch => {
                if jumping {
                    if dx != 0 && dy != 0 {
                        pec_value(f, 0, false, 0);
                        pec_value(f, 0, false, 0);
                    }
                    jumping = false;
                }
                pec_value(f, dx, false, 0);
                pec_value(f, dy, false, 0);
            }
            Op::Jump => {
                jumping = true;
                let flag = if init { JUMP_CODE } else { TRIM_CODE };
                pec_value(f, dx, true, flag);
                pec_value(f, dy, true, flag);
            }
            Op::ColorChange => {
                if jumping {
                    pec_value(f, 0, false, 0);
                    pec_value(f, 0, false, 0);
                    jumping = false;
                }
                f.extend_from_slice(&[0xFE, 0xB0, if color_two { 2 } else { 1 }]);
                color_two = !color_two;
            }
            Op::Trim => {}
            Op::End => {
                f.push(0xFF);
                break;
            }
        }
        init = false;
    }
}
