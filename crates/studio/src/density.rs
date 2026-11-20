//! View › Density Map: where stitches pile up. Needle penetrations are
//! counted in 1 mm cells (smoothed over their neighbours) and the cells
//! with too many are tinted, yellow then red, over the design, as
//! Ink/Stitch's density map does. Heavy spots stiffen the fabric, break
//! needles and pucker thin cotton.

use crate::canvas::View;
use eframe::egui::{self, Color32, Rect};
use zigzag::design::Design;
use zigzag::geom::P;

/// Cell size, units (1 mm).
const CELL: f64 = 10.0;
/// Needle penetrations per mm² that count as heavy, and as too heavy.
/// A single tatami fill runs about 1 per mm², a satin about 2; fills
/// stacked three deep pass 2.5, satin over two fills 4.
pub const HEAVY: f64 = 2.5;
pub const TOO_HEAVY: f64 = 4.0;

pub struct DensityMap {
    origin: P,
    cols: usize,
    rows: usize,
    /// Penetrations per mm² at each cell, smoothed.
    per_mm2: Vec<f32>,
}

impl DensityMap {
    pub fn of(d: &Design) -> DensityMap {
        let b = d.bounds();
        let origin = P::new(
            (b.min.x / CELL).floor() * CELL,
            (b.min.y / CELL).floor() * CELL,
        );
        let cols = ((b.max.x - origin.x) / CELL).ceil().max(1.0) as usize + 1;
        let rows = ((b.max.y - origin.y) / CELL).ceil().max(1.0) as usize + 1;
        let mut counts = vec![0u32; cols * rows];
        for p in d.blocks.iter().flat_map(|b| &b.runs).flat_map(|r| &r.pts) {
            let cx = ((p.x - origin.x) / CELL) as usize;
            let cy = ((p.y - origin.y) / CELL) as usize;
            if cx < cols && cy < rows {
                counts[cy * cols + cx] += 1;
            }
        }
        // Smooth over the eight neighbours, so a cell on a stitch boundary
        // reads like its surroundings.
        let mut per_mm2 = vec![0.0f32; cols * rows];
        for y in 0..rows {
            for x in 0..cols {
                let (mut sum, mut n) = (0u32, 0u32);
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                        if xx >= 0 && yy >= 0 && (xx as usize) < cols && (yy as usize) < rows {
                            sum += counts[yy as usize * cols + xx as usize];
                            n += 1;
                        }
                    }
                }
                per_mm2[y * cols + x] = sum as f32 / n.max(1) as f32;
            }
        }
        DensityMap {
            origin,
            cols,
            rows,
            per_mm2,
        }
    }

    /// The heaviest cell, per mm².
    pub fn peak(&self) -> f32 {
        self.per_mm2.iter().copied().fold(0.0, f32::max)
    }

    /// Cells at or above `HEAVY`.
    pub fn heavy_cells(&self) -> usize {
        self.per_mm2.iter().filter(|&&v| v >= HEAVY as f32).count()
    }

    pub fn draw(&self, painter: &egui::Painter, rect: Rect, view: &View) {
        for y in 0..self.rows {
            for x in 0..self.cols {
                let v = self.per_mm2[y * self.cols + x] as f64;
                if v < HEAVY {
                    continue;
                }
                let t = ((v - HEAVY) / (TOO_HEAVY - HEAVY)).clamp(0.0, 1.0) as f32;
                let c = Color32::from_rgba_unmultiplied(
                    255,
                    (210.0 * (1.0 - t)) as u8 + 20,
                    20,
                    (90.0 + 90.0 * t) as u8,
                );
                let a = view.to_screen(
                    rect,
                    P::new(
                        self.origin.x + x as f64 * CELL,
                        self.origin.y + y as f64 * CELL,
                    ),
                );
                let b = view.to_screen(
                    rect,
                    P::new(
                        self.origin.x + (x + 1) as f64 * CELL,
                        self.origin.y + (y + 1) as f64 * CELL,
                    ),
                );
                let r = Rect::from_two_pos(a, b);
                if r.intersects(rect) {
                    painter.rect_filled(r, 0.0, c);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zigzag::design::{Block, Run};

    fn grid(spacing: f64, passes: usize) -> Design {
        // A 10 × 10 mm area stitched in rows `spacing` apart, `passes` times over.
        let mut pts = Vec::new();
        for _ in 0..passes {
            let mut y = 0.0;
            while y <= 100.0 {
                let mut x = 0.0;
                while x <= 100.0 {
                    pts.push(P::new(x, y));
                    x += 30.0;
                }
                y += spacing;
            }
        }
        Design {
            blocks: vec![Block {
                color: [0; 3],
                runs: vec![Run {
                    pts,
                    trim_before: false,
                }],
            }],
            ..Default::default()
        }
    }

    #[test]
    fn one_fill_is_fine_and_three_on_top_of_each_other_are_heavy() {
        let one = DensityMap::of(&grid(3.0, 1));
        assert!(one.peak() < HEAVY as f32, "{}", one.peak());
        assert_eq!(one.heavy_cells(), 0);
        let three = DensityMap::of(&grid(3.0, 3));
        assert!(three.peak() >= HEAVY as f32, "{}", three.peak());
        assert!(three.heavy_cells() > 20);
    }
}
