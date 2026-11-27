//! A Windows bitmap (24-bit, uncompressed): the preview picture Bernina
//! (and bernette) machines show for an EXP, `design.bmp`.
//!
//! Tried on a Bernina: a 120 × 120 24-bit bitmap, bottom row first, shows
//! in its design's tile; at 240 × 240 (24- or 32-bit) the tile is black, an
//! 8-bit one that size overflows the tile, and rows stored top first are
//! "missing image". So the preview is fitted into [`PREVIEW_PX`] square.

/// The side of the square a Bernina's preview is fitted into, px.
pub const PREVIEW_PX: u32 = 120;

/// `rgba` (`w` × `h`, premultiplied, top row first) scaled to fit a `side`
/// × `side` square, centered on `background`: RGBA, opaque.
pub fn fit_square(w: u32, h: u32, rgba: &[u8], side: u32, background: [u8; 3]) -> Vec<u8> {
    let s = side as usize;
    let mut out: Vec<u8> =
        std::iter::repeat_n([background[0], background[1], background[2], 255], s * s)
            .flatten()
            .collect();
    if w == 0 || h == 0 {
        return out;
    }
    let k = (side as f64 / w as f64).min(side as f64 / h as f64);
    let (fw, fh) = (
        ((w as f64 * k).round() as usize).max(1),
        ((h as f64 * k).round() as usize).max(1),
    );
    let (x0, y0) = ((s - fw.min(s)) / 2, (s - fh.min(s)) / 2);
    for y in 0..fh.min(s) {
        for x in 0..fw.min(s) {
            // The source pixel at this one's middle.
            let sx = (((x as f64 + 0.5) / k) as usize).min(w as usize - 1);
            let sy = (((y as f64 + 0.5) / k) as usize).min(h as usize - 1);
            let p = &rgba[(sy * w as usize + sx) * 4..][..4];
            let over =
                |c: u8, bg: u8| (c as u32 + (255 - p[3] as u32) * bg as u32 / 255).min(255) as u8;
            let at = ((y0 + y) * s + x0 + x) * 4;
            out[at..at + 4].copy_from_slice(&[
                over(p[0], background[0]),
                over(p[1], background[1]),
                over(p[2], background[2]),
                255,
            ]);
        }
    }
    out
}

/// `rgba` (`w` × `h`, premultiplied alpha, top row first) as a BMP, laid
/// over `background` where it isn't opaque.
pub fn write_bmp(w: u32, h: u32, rgba: &[u8], background: [u8; 3]) -> Vec<u8> {
    let row = (w as usize * 3).div_ceil(4) * 4;
    let pixels = row * h as usize;
    let mut out = Vec::with_capacity(54 + pixels);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + pixels) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(pixels as u32).to_le_bytes());
    // 72 dpi.
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    // Bottom row first, blue-green-red, each row padded to 4 bytes.
    for y in (0..h as usize).rev() {
        let start = out.len();
        for x in 0..w as usize {
            let p = &rgba[(y * w as usize + x) * 4..][..4];
            let over =
                |c: u8, bg: u8| (c as u32 + (255 - p[3] as u32) * bg as u32 / 255).min(255) as u8;
            out.extend_from_slice(&[
                over(p[2], background[2]),
                over(p[1], background[1]),
                over(p[0], background[0]),
            ]);
        }
        out.resize(start + row, 0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_fitted_into_the_square_and_centered() {
        // 4 × 2, red, fitted into 2 × 2: one row of red (the top, the
        // centered row rounding up) over a row of the background.
        let red = [255u8, 0, 0, 255].repeat(8);
        let sq = fit_square(4, 2, &red, 2, [0, 0, 255]);
        assert_eq!(sq.len(), 2 * 2 * 4);
        assert_eq!(&sq[0..8], &[255, 0, 0, 255, 255, 0, 0, 255]);
        assert_eq!(&sq[8..16], &[0, 0, 255, 255, 0, 0, 255, 255]);
        // A tall one is as tall as the square, centered across it.
        let tall = [0u8, 255, 0, 255].repeat(1 * 4);
        let sq = fit_square(1, 4, &tall, 4, [0, 0, 0]);
        let green_cols: Vec<usize> = (0..4).filter(|&x| sq[x * 4 + 1] == 255).collect();
        assert_eq!(green_cols.len(), 1, "one column wide");
        assert!(
            (1..=2).contains(&green_cols[0]),
            "in the middle: {green_cols:?}"
        );
        assert!(
            (0..4).all(|y| sq[(y * 4 + green_cols[0]) * 4 + 1] == 255),
            "all the way down"
        );
    }

    #[test]
    fn a_bitmap_is_bottom_row_first_bgr_with_padded_rows() {
        // 2 × 2: red, green on top; blue, transparent below.
        let rgba = [
            255, 0, 0, 255, 0, 255, 0, 255, //
            0, 0, 255, 255, 0, 0, 0, 0,
        ];
        let b = write_bmp(2, 2, &rgba, [10, 20, 30]);
        assert_eq!(&b[0..2], b"BM");
        assert_eq!(
            u32::from_le_bytes(b[2..6].try_into().unwrap()) as usize,
            b.len()
        );
        assert_eq!(u32::from_le_bytes(b[10..14].try_into().unwrap()), 54);
        assert_eq!(i32::from_le_bytes(b[18..22].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes([b[28], b[29]]), 24);
        // Rows of 6 bytes padded to 8: the bottom row (blue, background) first.
        assert_eq!(b.len(), 54 + 8 * 2);
        assert_eq!(&b[54..62], &[255, 0, 0, 30, 20, 10, 0, 0]);
        assert_eq!(&b[62..70], &[0, 0, 255, 0, 255, 0, 0, 0]);
    }
}
