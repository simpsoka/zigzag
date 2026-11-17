//! Deterministic CPU mockup renderer. Geometry is in design units (0.1 mm).
use crate::design::Design;
use anyhow::{Context, Result, ensure};
use resvg::tiny_skia::Pixmap;
use std::f64::consts::TAU;

/// Background treatment for an embroidery mockup.
#[derive(Clone, Copy, Debug)]
pub enum MockupBackground {
    Fabric([u8; 3]),
    Solid([u8; 3]),
    Transparent,
}

/// Render actual stitches as lit thread on woven fabric, with 5 mm padding.
/// Jumps and the hoop are omitted. Width is clamped to 64–4096; height is
/// capped at 4096 while preserving aspect ratio. No GUI or GPU is required.
pub fn render_realistic_png(d: &Design, background: [u8; 3], width: u32) -> Result<Vec<u8>> {
    render_realistic_png_with_background(d, MockupBackground::Fabric(background), width)
}

/// Render shaded embroidery on fabric, a solid color, or transparency.
pub fn render_realistic_png_with_background(
    d: &Design,
    background: MockupBackground,
    width: u32,
) -> Result<Vec<u8>> {
    let b = d.bounds();
    let (x0, y0, span_x, span_y) = if b.is_empty() {
        (-50.0, -50.0, 100.0, 100.0)
    } else {
        (
            b.min.x - 50.0,
            b.min.y - 50.0,
            b.max.x - b.min.x + 100.0,
            b.max.y - b.min.y + 100.0,
        )
    };
    ensure!(
        [x0, y0, span_x, span_y].iter().all(|v| v.is_finite()),
        "non-finite design bounds"
    );
    let scale = (width.clamp(64, 4096) as f64 / span_x).min(4096.0 / span_y);
    let w = (span_x * scale).round().max(1.0) as u32;
    let h = (span_y * scale).round().max(1.0) as u32;
    let mut image = Pixmap::new(w, h).context("allocating mockup")?;
    for y in 0..h {
        for x in 0..w {
            let background = match background {
                MockupBackground::Transparent => continue,
                MockupBackground::Solid(rgb) => {
                    let i = ((y * w + x) * 4) as usize;
                    image.data_mut()[i..i + 3].copy_from_slice(&rgb);
                    image.data_mut()[i + 3] = 255;
                    continue;
                }
                MockupBackground::Fabric(rgb) => rgb,
            };
            // Alternating warp/weft yarns, fine fibers and seeded grain.
            let u = x as f64 / scale;
            let v = y as f64 / scale;
            let warp = (u * TAU / 2.4).cos();
            let weft = (v * TAU / 2.4).cos();
            let grain = noise(x, y) - 0.5;
            let weave = 0.018 * (warp + weft) + 0.012 * warp * weft;
            let fiber = 0.007 * (u * TAU / 0.6).sin();
            let vignette = ((x as f64 / w as f64 - 0.4).powi(2)
                + (y as f64 / h as f64 - 0.35).powi(2))
                * 0.065;
            let light = 1.0 + weave + fiber + grain * 0.025 - vignette;
            let i = ((y * w + x) * 4) as usize;
            for (c, value) in background.iter().enumerate() {
                image.data_mut()[i + c] = (*value as f64 * light).clamp(0.0, 255.0) as u8;
            }
            image.data_mut()[i + 3] = 255;
        }
    }
    let radius = 1.6 * scale; // 0.32 mm thread diameter
    for block in &d.blocks {
        for run in &block.runs {
            for pair in run.pts.windows(2) {
                let a = ((pair[0].x - x0) * scale, (pair[0].y - y0) * scale);
                let b = ((pair[1].x - x0) * scale, (pair[1].y - y0) * scale);
                if ![a.0, a.1, b.0, b.1].iter().all(|v| v.is_finite()) {
                    continue;
                }
                stitch(&mut image, a, b, radius, block.color, true);
                stitch(&mut image, a, b, radius, block.color, false);
            }
        }
    }
    image.encode_png().context("encoding mockup PNG")
}

fn noise(x: u32, y: u32) -> f64 {
    let mut n = x
        .wrapping_mul(374761393)
        .wrapping_add(y.wrapping_mul(668265263));
    n = (n ^ (n >> 13)).wrapping_mul(1274126177);
    (n ^ (n >> 16)) as f64 / u32::MAX as f64
}

fn stitch(
    image: &mut Pixmap,
    mut a: (f64, f64),
    mut b: (f64, f64),
    r: f64,
    color: [u8; 3],
    shadow: bool,
) {
    if shadow {
        a.0 += r * 0.45;
        a.1 += r * 0.65;
        b.0 += r * 0.45;
        b.1 += r * 0.65;
    }
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len = dx.hypot(dy);
    if len < 0.001 {
        return;
    }
    let extent = if shadow { r * 2.2 + 1.0 } else { r + 1.0 };
    let min_x = (a.0.min(b.0) - extent).floor().max(0.0) as u32;
    let max_x = (a.0.max(b.0) + extent).ceil().min(image.width() as f64) as u32;
    let min_y = (a.1.min(b.1) - extent).floor().max(0.0) as u32;
    let max_y = (a.1.max(b.1) + extent).ceil().min(image.height() as f64) as u32;
    let w = image.width();
    for y in min_y..max_y {
        for x in min_x..max_x {
            let px = x as f64 + 0.5 - a.0;
            let py = y as f64 + 0.5 - a.1;
            let t = ((px * dx + py * dy) / (len * len)).clamp(0.0, 1.0);
            let ex = px - t * dx;
            let ey = py - t * dy;
            let distance = ex.hypot(ey);
            let (alpha, rgb) = if shadow {
                (
                    0.16 * (-2.5 * (distance / r.max(0.5)).powi(2)).exp(),
                    [35.0, 29.0, 24.0],
                )
            } else {
                let alpha = (r + 0.5 - distance).clamp(0.0, 1.0);
                if alpha == 0.0 {
                    continue;
                }
                let across = ((px * -dy + py * dx) / len / r).clamp(-1.0, 1.0);
                let z = (1.0 - across * across).sqrt();
                let diffuse = (z * 0.82 + across * (dy - dx) / len * 0.4).max(0.0);
                let sheen = diffuse.powi(14) * 0.24;
                let twist = (t * len / r * 3.5 + across * 8.0).sin() * 0.035;
                // Thread sinks into the fabric at each needle penetration.
                let tip = (t.min(1.0 - t) * len / r).min(1.0);
                let light = (0.57 + diffuse * 0.48 + twist) * (0.72 + tip * 0.28);
                (
                    alpha,
                    color.map(|c| (c as f64 * light + 255.0 * sheen).min(255.0)),
                )
            };
            let i = ((y * w + x) * 4) as usize;
            // tiny-skia stores premultiplied RGBA; composite both color and alpha.
            let old_alpha = image.data()[i + 3] as f64 / 255.0;
            image.data_mut()[i + 3] = ((alpha + old_alpha * (1.0 - alpha)) * 255.0).round() as u8;
            for (c, value) in rgb.iter().enumerate() {
                let old = image.data()[i + c] as f64;
                image.data_mut()[i + c] = (old + (value - old) * alpha).round() as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        design::{Block, Run},
        geom::P,
    };

    #[test]
    fn blank_backgrounds_have_exact_color_and_alpha() {
        for (background, expected) in [
            (MockupBackground::Solid([255; 3]), [255; 4]),
            (MockupBackground::Transparent, [0; 4]),
        ] {
            let png =
                render_realistic_png_with_background(&Design::default(), background, 64).unwrap();
            let image = Pixmap::decode_png(&png).unwrap();
            assert!(image.data().chunks_exact(4).all(|pixel| pixel == expected));
        }
        let mut image = Pixmap::new(32, 32).unwrap();
        stitch(
            &mut image,
            (8.0, 16.0),
            (24.0, 16.0),
            2.0,
            [180, 40, 30],
            false,
        );
        assert!(image.data().chunks_exact(4).any(|p| p[3] > 0));
        assert!(
            image
                .data()
                .chunks_exact(4)
                .all(|p| p[..3].iter().all(|c| *c <= p[3]))
        );
        assert_eq!(&image.data()[..4], &[0; 4]);
    }

    #[test]
    fn mockup_is_deterministic_and_handles_empty_and_tall_designs() {
        let empty = render_realistic_png(&Design::default(), [240; 3], 64).unwrap();
        assert_eq!(
            empty,
            render_realistic_png(&Design::default(), [240; 3], 64).unwrap()
        );
        let design = Design {
            blocks: vec![Block {
                color: [180, 40, 30],
                runs: vec![Run {
                    pts: vec![P::new(0.0, 0.0), P::new(0.0, 10000.0)],
                    trim_before: false,
                }],
            }],
            ..Default::default()
        };
        let png = render_realistic_png(&design, [240; 3], 64).unwrap();
        let decoded = Pixmap::decode_png(&png).unwrap();
        assert_eq!(decoded.height(), 4096);
        assert!(decoded.width() <= 64);
        assert_ne!(png, empty);
    }
}
