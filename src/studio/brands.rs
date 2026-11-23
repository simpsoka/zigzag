//! The marks of the AI services the Agent panel can run, for telling them
//! apart at a glance: drawn in one color (the text's, or any), from the
//! single-color SVGs in `brands/` (Simple Icons, CC0; the marks themselves
//! belong to their companies and only name their services here).

use eframe::egui::{self, Color32, Rect, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Brand {
    OpenAi,
    Anthropic,
    Google,
}

impl Brand {
    fn svg(self) -> &'static str {
        match self {
            Brand::OpenAi => include_str!("brands/openai.svg"),
            Brand::Anthropic => include_str!("brands/anthropic.svg"),
            Brand::Google => include_str!("brands/google.svg"),
        }
    }
}

/// Pixels the marks are drawn at: sharp up to about 32 points on a Retina screen.
const PX: u32 = 96;

fn texture(ctx: &egui::Context, brand: Brand) -> Option<egui::TextureHandle> {
    let id = egui::Id::unique(("brand-mark", brand));
    if let Some(t) = ctx.data(|d| d.get_temp::<Option<egui::TextureHandle>>(id)) {
        return t;
    }
    let t = rasterize(brand.svg()).map(|img| {
        ctx.load_texture(
            format!("brand-{brand:?}"),
            img,
            egui::TextureOptions::LINEAR,
        )
    });
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// The mark, white on clear, `PX` square.
fn rasterize(svg: &str) -> Option<egui::ColorImage> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).ok()?;
    let size = tree.size();
    let s = PX as f32 / size.width().max(size.height());
    let mut pm = tiny_skia::Pixmap::new(PX, PX)?;
    let dx = (PX as f32 - size.width() * s) / 2.0;
    let dy = (PX as f32 - size.height() * s) / 2.0;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_row(s, 0.0, 0.0, s, dx, dy),
        &mut pm.as_mut(),
    );
    Some(egui::ColorImage::from_rgba_premultiplied(
        [PX as usize, PX as usize],
        pm.data(),
    ))
}

/// Paint `brand`'s mark in `rect`, in `color`.
pub fn paint(ui: &egui::Ui, rect: Rect, brand: Brand, color: Color32) {
    if let Some(t) = texture(ui.ctx(), brand) {
        ui.painter().image(
            t.id(),
            rect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            color,
        );
    }
}

/// The mark as a widget of `size` points.
pub fn show(ui: &mut egui::Ui, brand: Brand, size: f32, color: Color32) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    paint(ui, r, brand, color);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mark_draws() {
        for b in [Brand::OpenAi, Brand::Anthropic, Brand::Google] {
            let img = rasterize(b.svg()).expect("parses");
            let inked = img.pixels.iter().filter(|p| p.a() > 128).count();
            assert!(inked > 400, "{b:?}: {inked} pixels");
        }
    }
}
