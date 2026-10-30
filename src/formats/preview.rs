//! Stitch previews: an SVG drawing of the plan (thread-colored stitches, dashed
//! jumps, hoop outline) and a PNG rendering of it. Units are millimetres.

use crate::design::Design;
use anyhow::{Context, Result};
use std::fmt::Write;

#[derive(Clone, Copy, Debug)]
pub struct PreviewOptions {
    /// Hoop (width, height) in mm to outline, centered on the origin.
    pub hoop_mm: Option<(f64, f64)>,
    /// Fabric color behind the stitches.
    pub background: [u8; 3],
    pub show_jumps: bool,
    /// Drawn thread thickness, mm.
    pub thread_mm: f64,
}

impl Default for PreviewOptions {
    fn default() -> Self {
        PreviewOptions {
            hoop_mm: Some((100.0, 100.0)),
            background: [244, 241, 234],
            show_jumps: true,
            thread_mm: 0.32,
        }
    }
}

pub fn write_svg(d: &Design, opt: &PreviewOptions) -> String {
    let b = d.bounds();
    let pad = 30.0;
    let (mut x0, mut y0, mut x1, mut y1) = if b.is_empty() {
        (-pad, -pad, pad, pad)
    } else {
        (b.min.x - pad, b.min.y - pad, b.max.x + pad, b.max.y + pad)
    };
    if let Some((hw, hh)) = opt.hoop_mm {
        x0 = x0.min(-hw * 5.0 - pad);
        y0 = y0.min(-hh * 5.0 - pad);
        x1 = x1.max(hw * 5.0 + pad);
        y1 = y1.max(hh * 5.0 + pad);
    }
    let (w, h) = ((x1 - x0) / 10.0, (y1 - y0) / 10.0);
    let mut s = String::with_capacity(d.stitch_count() * 14 + 1024);
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.1}mm" height="{h:.1}mm" viewBox="{:.1} {:.1} {:.1} {:.1}">"#,
        x0 / 10.0,
        y0 / 10.0,
        w,
        h
    );
    let [r, g, bl] = opt.background;
    let _ = write!(
        s,
        r##"<rect x="{:.1}" y="{:.1}" width="{w:.1}" height="{h:.1}" fill="#{r:02x}{g:02x}{bl:02x}"/>"##,
        x0 / 10.0,
        y0 / 10.0
    );
    if let Some((hw, hh)) = opt.hoop_mm {
        let _ = write!(
            s,
            r##"<rect x="{:.1}" y="{:.1}" width="{hw:.1}" height="{hh:.1}" rx="4" fill="none" stroke="#9a9488" stroke-width="0.3" stroke-dasharray="2 1"/>"##,
            -hw / 2.0,
            -hh / 2.0
        );
    }
    let mut last: Option<crate::geom::P> = None;
    for blk in &d.blocks {
        let color = format!(
            "#{:02x}{:02x}{:02x}",
            blk.color[0], blk.color[1], blk.color[2]
        );
        for run in &blk.runs {
            if opt.show_jumps
                && let (Some(a), Some(&b)) = (last, run.pts.first())
            {
                let _ = write!(
                    s,
                    r##"<line x1="{:.2}" y1="{:.2}" x2="{:.2}" y2="{:.2}" stroke="#888" stroke-width="0.08" stroke-dasharray="0.4 0.4"/>"##,
                    a.x / 10.0,
                    a.y / 10.0,
                    b.x / 10.0,
                    b.y / 10.0
                );
            }
            let _ = write!(
                s,
                r#"<polyline fill="none" stroke-linejoin="round" stroke-linecap="round" stroke-width="{:.2}" stroke="{color}" points=""#,
                opt.thread_mm
            );
            for p in &run.pts {
                let _ = write!(s, "{:.2},{:.2} ", p.x / 10.0, p.y / 10.0);
            }
            s.push_str(r#""/>"#);
            last = run.pts.last().copied();
        }
    }
    s.push_str("</svg>\n");
    s
}

/// Render the preview to a PNG `width_px` wide.
pub fn render_png(d: &Design, opt: &PreviewOptions, width_px: u32) -> Result<Vec<u8>> {
    let svg = write_svg(d, opt);
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).context("preview SVG")?;
    let size = tree.size();
    let scale = width_px.clamp(64, 4096) as f32 / size.width();
    let (w, h) = (
        (size.width() * scale).ceil() as u32,
        (size.height() * scale).ceil() as u32,
    );
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).context("allocating preview")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().context("encoding PNG")
}
