//! Tajima DST. Record encoding and header ported from pyembroidery's DstWriter
//! (MIT License, Copyright (c) 2018 Tatarize).

use super::{Cmd, Op, StreamOptions, commands, stream_bounds};
use crate::design::Design;

pub fn stream(d: &Design) -> Vec<Cmd> {
    commands(
        d,
        &StreamOptions {
            ties: true,
            max_step: 121.0,
        },
    )
}

pub fn write_dst(d: &Design) -> Vec<u8> {
    dst_from_stream(&d.name, &stream(d))
}

/// Largest per-axis step a DST record can encode.
pub const MAX_STEP: i64 = 121;

fn encode_record(x: i64, y: i64, op: Op) -> [u8; 3] {
    assert!(
        x.abs() <= MAX_STEP && y.abs() <= MAX_STEP,
        "DST step ({x}, {y}) out of range"
    );
    let (mut x, mut y) = (x, -y);
    let (mut b0, mut b1, mut b2) = (0u8, 0u8, 0u8);
    match op {
        Op::Stitch | Op::Jump => {
            if op == Op::Jump {
                b2 |= 0x80;
            }
            b2 |= 0x03;
            if x > 40 {
                b2 |= 0x04;
                x -= 81;
            }
            if x < -40 {
                b2 |= 0x08;
                x += 81;
            }
            if x > 13 {
                b1 |= 0x04;
                x -= 27;
            }
            if x < -13 {
                b1 |= 0x08;
                x += 27;
            }
            if x > 4 {
                b0 |= 0x04;
                x -= 9;
            }
            if x < -4 {
                b0 |= 0x08;
                x += 9;
            }
            if x > 1 {
                b1 |= 0x01;
                x -= 3;
            }
            if x < -1 {
                b1 |= 0x02;
                x += 3;
            }
            if x > 0 {
                b0 |= 0x01;
                x -= 1;
            }
            if x < 0 {
                b0 |= 0x02;
                x += 1;
            }
            debug_assert_eq!(x, 0);
            if y > 40 {
                b2 |= 0x20;
                y -= 81;
            }
            if y < -40 {
                b2 |= 0x10;
                y += 81;
            }
            if y > 13 {
                b1 |= 0x20;
                y -= 27;
            }
            if y < -13 {
                b1 |= 0x10;
                y += 27;
            }
            if y > 4 {
                b0 |= 0x20;
                y -= 9;
            }
            if y < -4 {
                b0 |= 0x10;
                y += 9;
            }
            if y > 1 {
                b1 |= 0x80;
                y -= 3;
            }
            if y < -1 {
                b1 |= 0x40;
                y += 3;
            }
            if y > 0 {
                b0 |= 0x80;
                y -= 1;
            }
            if y < 0 {
                b0 |= 0x40;
                y += 1;
            }
            debug_assert_eq!(y, 0);
        }
        Op::ColorChange => b2 = 0b1100_0011,
        Op::End => b2 = 0b1111_0011,
        Op::Trim => unreachable!("trims are written as jump sequences"),
    }
    [b0, b1, b2]
}

pub fn dst_from_stream(name: &str, cmds: &[Cmd]) -> Vec<u8> {
    let mut f: Vec<u8> = Vec::with_capacity(512 + cmds.len() * 3 + 16);
    let b = stream_bounds(cmds);
    let colors = cmds.iter().filter(|c| c.op == Op::ColorChange).count();
    let name: String = name.chars().filter(|c| c.is_ascii()).collect();
    let mut h = String::new();
    h += &format!("LA:{name:<16}\r");
    h += &format!("ST:{:7}\r", cmds.len());
    h += &format!("CO:{colors:3}\r");
    h += &format!("+X:{:5}\r", b.max.x.abs() as i64);
    h += &format!("-X:{:5}\r", b.min.x.abs() as i64);
    h += &format!("+Y:{:5}\r", b.max.y.abs() as i64);
    h += &format!("-Y:{:5}\r", b.min.y.abs() as i64);
    let (ax, ay) = cmds
        .last()
        .map(|c| (c.x as i64, -(c.y as i64)))
        .unwrap_or((0, 0));
    h += &if ax >= 0 {
        format!("AX:+{ax:5}\r")
    } else {
        format!("AX:-{:5}\r", ax.abs())
    };
    h += &if ay >= 0 {
        format!("AY:+{ay:5}\r")
    } else {
        format!("AY:-{:5}\r", ay.abs())
    };
    h += &format!("MX:+{:5}\r", 0);
    h += &format!("MY:+{:5}\r", 0);
    h += &format!("PD:{:>6}\r", "******");
    f.extend_from_slice(h.as_bytes());
    f.push(0x1A);
    f.resize(512, 0x20);

    let (mut xx, mut yy) = (0i64, 0i64);
    for c in cmds {
        let dx = c.x.round() as i64 - xx;
        let dy = c.y.round() as i64 - yy;
        xx += dx;
        yy += dy;
        if c.op == Op::Trim {
            // Three jumps that return to the same spot read as a trim.
            f.extend_from_slice(&encode_record(2, 2, Op::Jump));
            f.extend_from_slice(&encode_record(-4, -4, Op::Jump));
            f.extend_from_slice(&encode_record(2, 2, Op::Jump));
        } else {
            f.extend_from_slice(&encode_record(dx, dy, c.op));
        }
    }
    f
}
