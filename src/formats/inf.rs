//! The thread colors file Bernina (and bernette) machines read beside an
//! EXP, and Melco's EXP+: `design.inf`. Without it the machine shows the
//! design in its own standard colors.
//!
//! Big-endian throughout: a header (1, 8, the length of what follows it,
//! the number of colors), then a record per color stop, in sewing order:
//! its length, its index, red, green, blue, its needle (from 1), and the
//! thread's description and chart, each ending in a 0 byte. (As
//! pyembroidery writes and reads it.)

use crate::design::Design;

/// The `.inf` for `d`: one record per color block, in sewing order.
pub fn write_inf(d: &Design) -> Vec<u8> {
    let mut records = Vec::new();
    for (i, b) in d.blocks.iter().enumerate() {
        let [r, g, bl] = b.color;
        // No thread chart is known for the colors, so each is named by its
        // own value (the machine shows the color itself either way).
        let description = format!("#{r:02X}{g:02X}{bl:02X}");
        let chart = "RGB";
        let len = 11 + description.len() + chart.len();
        records.extend_from_slice(&(len as u16).to_be_bytes());
        records.extend_from_slice(&(i as u16).to_be_bytes());
        records.extend_from_slice(&[r, g, bl]);
        records.extend_from_slice(&((i + 1) as u16).to_be_bytes());
        records.extend_from_slice(description.as_bytes());
        records.push(0);
        records.extend_from_slice(chart.as_bytes());
        records.push(0);
    }
    let mut out = Vec::with_capacity(16 + records.len());
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(&8u32.to_be_bytes());
    // (What follows this field: the count and the records.)
    out.extend_from_slice(&((4 + records.len()) as u32).to_be_bytes());
    out.extend_from_slice(&(d.blocks.len() as u32).to_be_bytes());
    out.extend_from_slice(&records);
    out
}

/// The colors in an `.inf`, in order (a reader for checking what's written).
pub fn read_inf_colors(data: &[u8]) -> Option<Vec<[u8; 3]>> {
    let n = u32::from_be_bytes(data.get(12..16)?.try_into().ok()?) as usize;
    let mut at = 16;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let len = u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?) as usize;
        let rec = data.get(at..at + len)?;
        out.push([*rec.get(4)?, *rec.get(5)?, *rec.get(6)?]);
        at += len;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::{Block, Run};
    use crate::geom::P;

    fn two_colors() -> Design {
        let run = Run {
            pts: vec![P::new(0.0, 0.0), P::new(10.0, 0.0)],
            trim_before: true,
        };
        Design {
            blocks: vec![
                Block {
                    color: [245, 236, 30],
                    runs: vec![run.clone()],
                },
                Block {
                    color: [31, 111, 209],
                    runs: vec![run],
                },
            ],
            ..Design::default()
        }
    }

    #[test]
    fn colors_are_written_in_sewing_order_as_pyembroidery_lays_them_out() {
        let b = write_inf(&two_colors());
        // Header: 1, 8, the length of what follows, 2 colors.
        assert_eq!(&b[0..8], &[0, 0, 0, 1, 0, 0, 0, 8]);
        assert_eq!(
            u32::from_be_bytes(b[8..12].try_into().unwrap()) as usize,
            b.len() - 12
        );
        assert_eq!(u32::from_be_bytes(b[12..16].try_into().unwrap()), 2);
        // The first record: its length, index 0, the color, needle 1, then
        // "#F5EC1E\0RGB\0".
        let len = u16::from_be_bytes([b[16], b[17]]) as usize;
        assert_eq!(len, 11 + 7 + 3);
        assert_eq!(&b[18..20], &[0, 0]);
        assert_eq!(&b[20..23], &[245, 236, 30]);
        assert_eq!(&b[23..25], &[0, 1]);
        assert_eq!(&b[25..16 + len], b"#F5EC1E\0RGB\0");
        assert_eq!(
            read_inf_colors(&b).unwrap(),
            [[245, 236, 30], [31, 111, 209]]
        );
    }

    #[test]
    fn a_design_with_no_colors_is_just_the_header() {
        let b = write_inf(&Design::default());
        assert_eq!(b, [0, 0, 0, 1, 0, 0, 0, 8, 0, 0, 0, 4, 0, 0, 0, 0]);
        assert_eq!(read_inf_colors(&b).unwrap(), Vec::<[u8; 3]>::new());
    }
}
