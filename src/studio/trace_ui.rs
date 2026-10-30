//! The Trace Image window: open, drop or place a PNG/JPEG logo and it is
//! traced into stitchable shapes (VectorCraft's Image Trace), with a live
//! preview while you choose colours, size and how lines are sewn.

use crate::app::Studio;
use crate::theme::{self, space};
use eframe::egui::{self, Color32, Key, Vec2};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use zigzag::trace::{Image, TraceOptions, Traced};

/// Open the trace as a new design, or place it in this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Open,
    Place,
}

type Done = (u64, Result<(Traced, egui::ColorImage), String>);

pub struct TraceUi {
    pub path: PathBuf,
    pub name: String,
    pub target: Target,
    image: Arc<Image>,
    pub opts: TraceOptions,
    /// Use the active palette's threads.
    pub use_threads: bool,
    /// Lock the height to the width (aspect kept): height from width.
    aspect: f64,
    pub result: Option<Result<Traced, String>>,
    preview: Option<egui::TextureHandle>,
    original: Option<egui::TextureHandle>,
    show_original: bool,
    /// The options of the trace running now, and how far the request count is.
    running: Option<(u64, Receiver<Done>)>,
    asked: u64,
    shown: u64,
    traced: Option<(TraceOptions, bool)>,
}

const PREVIEW: Vec2 = Vec2::new(380.0, 300.0);

/// Draw SVG text into an image that fits `max` (for the preview).
fn rasterize(svg: &str, max: Vec2) -> Option<egui::ColorImage> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).ok()?;
    let size = tree.size();
    // Twice the window size, so it's sharp on a Retina screen.
    let s = ((max.x * 2.0) / size.width()).min((max.y * 2.0) / size.height());
    let (w, h) = (
        (size.width() * s).ceil().max(1.0) as u32,
        (size.height() * s).ceil().max(1.0) as u32,
    );
    let mut pm = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(s, s),
        &mut pm.as_mut(),
    );
    Some(egui::ColorImage::from_rgba_premultiplied(
        [w as usize, h as usize],
        pm.data(),
    ))
}

/// A smaller copy of the image for showing it (box-filtered).
fn thumbnail(img: &Image, max: Vec2) -> egui::ColorImage {
    let (w, h) = img.size();
    let s = ((max.x * 2.0) / w as f32)
        .min((max.y * 2.0) / h as f32)
        .min(1.0);
    let (tw, th) = (
        ((w as f32 * s).round() as usize).max(1),
        ((h as f32 * s).round() as usize).max(1),
    );
    let rgba = img.rgba();
    let mut out = Vec::with_capacity(tw * th * 4);
    for ty in 0..th {
        let (y0, y1) = (
            ty * h as usize / th,
            ((ty + 1) * h as usize / th).max(ty * h as usize / th + 1),
        );
        for tx in 0..tw {
            let (x0, x1) = (
                tx * w as usize / tw,
                ((tx + 1) * w as usize / tw).max(tx * w as usize / tw + 1),
            );
            let mut acc = [0u32; 4];
            let mut n = 0;
            for y in y0..y1.min(h as usize) {
                for x in x0..x1.min(w as usize) {
                    let i = (y * w as usize + x) * 4;
                    for k in 0..4 {
                        acc[k] += rgba[i + k] as u32;
                    }
                    n += 1;
                }
            }
            let n = n.max(1);
            out.extend(acc.iter().map(|v| (v / n) as u8));
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([tw, th], &out)
}

impl Studio {
    /// Start tracing an image file (from Open, a drop or Place).
    pub fn open_image(&mut self, path: &Path, target: Target) {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                self.status = format!("Couldn't read {}: {e}", path.display());
                return;
            }
        };
        let image = match Image::decode(&bytes) {
            Ok(i) => i,
            Err(e) => {
                self.status = format!("Couldn't open {}: {e:#}", path.display());
                return;
            }
        };
        let (w, h) = image.size();
        let aspect = h as f64 / w.max(1) as f64;
        // As big as fits 80% of the hoop.
        let (hw, hh) = self.hoop_dims();
        let width_mm = (hw * 0.8).min(hh * 0.8 / aspect).max(5.0);
        let opts = TraceOptions {
            width_mm: (width_mm * 2.0).round() / 2.0,
            ..TraceOptions::default()
        };
        self.trace = Some(TraceUi {
            path: path.to_path_buf(),
            name: path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            target,
            image: Arc::new(image),
            opts,
            use_threads: false,
            aspect,
            result: None,
            preview: None,
            original: None,
            show_original: false,
            running: None,
            asked: 0,
            shown: 0,
            traced: None,
        });
        self.status = format!("Tracing {}…", path.display());
    }

    fn trace_threads(&self) -> Vec<[u8; 3]> {
        self.palettes
            .colors(self.palettes.source, crate::threads::Brand::Brother)
            .into_iter()
            .map(|(c, _)| c)
            .collect()
    }

    /// Keep the trace up to date with the options (one at a time, in the background).
    fn trace_work(&mut self, ctx: &egui::Context) {
        let threads = self.trace_threads();
        let Some(t) = self.trace.as_mut() else {
            return;
        };
        if let Some((n, rx)) = &t.running {
            match rx.try_recv() {
                Ok((n, done)) => {
                    t.running = None;
                    t.shown = n;
                    match done {
                        Ok((traced, img)) => {
                            t.preview = Some(ctx.load_texture(
                                "trace-preview",
                                img,
                                egui::TextureOptions::LINEAR,
                            ));
                            t.result = Some(Ok(traced));
                        }
                        Err(e) => {
                            t.preview = None;
                            t.result = Some(Err(e));
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    let _ = n;
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                    return;
                }
                Err(_) => t.running = None,
            }
        }
        let want = (t.opts.clone(), t.use_threads);
        if t.traced.as_ref() == Some(&want) {
            return;
        }
        t.traced = Some(want);
        t.asked += 1;
        let n = t.asked;
        let mut opts = t.opts.clone();
        opts.threads = if t.use_threads { threads } else { Vec::new() };
        let image = t.image.clone();
        let (tx, rx) = channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let r = zigzag::trace::trace(&image, &opts)
                .map_err(|e| format!("{e:#}"))
                .and_then(|t| {
                    let img = rasterize(&t.svg, PREVIEW).ok_or("couldn't draw the preview")?;
                    Ok((t, img))
                });
            let _ = tx.send((n, r));
            repaint.request_repaint();
        });
        t.running = Some((n, rx));
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }

    /// The Trace Image window.
    pub fn trace_window(&mut self, ctx: &egui::Context) {
        if self.trace.is_none() {
            return;
        }
        self.trace_work(ctx);
        let palette_name = self.palettes.name(self.palettes.source);
        let cloth = self.doc.settings.cloth;
        let Some(t) = self.trace.as_mut() else {
            return;
        };
        if t.original.is_none() {
            t.original = Some(ctx.load_texture(
                "trace-original",
                thumbnail(&t.image, PREVIEW),
                egui::TextureOptions::LINEAR,
            ));
        }
        let mut open = true;
        let (mut ok, mut cancel) = (false, false);
        let busy = t.running.is_some();
        egui::Window::new("Trace Image")
            .title_frame(crate::theme::title_frame(ctx))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, 80.0))
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = space::M;
                    // Preview.
                    ui.vertical(|ui| {
                        theme::segmented(
                            ui,
                            "trace-view",
                            &mut t.show_original,
                            &[(false, "Traced"), (true, "Original")],
                        );
                        ui.add_space(space::S);
                        let (r, _) = ui.allocate_exact_size(PREVIEW, egui::Sense::hover());
                        let p = theme::pal_ui(ui);
                        // On the hat's color (Document › Preview on), as it will sew.
                        ui.painter().rect_filled(r, 6.0, Color32::from_rgb(cloth[0], cloth[1], cloth[2]));
                        let tex = if t.show_original { t.original.as_ref() } else { t.preview.as_ref() };
                        if let Some(tex) = tex {
                            let size = tex.size_vec2();
                            let s = (PREVIEW.x / size.x).min(PREVIEW.y / size.y);
                            let img = egui::Rect::from_center_size(r.center(), size * s);
                            ui.painter().image(
                                tex.id(),
                                img,
                                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                                Color32::WHITE,
                            );
                        }
                        if busy && !t.show_original {
                            ui.put(
                                egui::Rect::from_center_size(r.center(), Vec2::splat(24.0)),
                                egui::Spinner::new(),
                            );
                        }
                        ui.add_space(space::S);
                        let line = match &t.result {
                            Some(Ok(r)) => format!(
                                "{} {} · {} shapes{} · {}",
                                r.colors.len(),
                                if r.colors.len() == 1 { "color" } else { "colors" },
                                r.shapes,
                                if r.lines > 0 { format!(" · {} lines", r.lines) } else { String::new() },
                                r.method
                            ),
                            Some(Err(e)) => e.clone(),
                            None => "Tracing…".into(),
                        };
                        theme::caption(ui, line);
                        // The thread colors (a row kept even while tracing, so nothing moves).
                        ui.horizontal(|ui| {
                            ui.set_min_height(theme::LIST_SWATCH);
                            ui.spacing_mut().item_spacing.x = space::XS;
                            if let Some(Ok(r)) = &t.result {
                                for c in &r.colors {
                                    let (sw, _) = ui.allocate_exact_size(Vec2::splat(theme::LIST_SWATCH), egui::Sense::hover());
                                    ui.painter().rect_filled(sw, 3.0, Color32::from_rgb(c[0], c[1], c[2]));
                                    ui.painter().rect_stroke(sw, 3.0, egui::Stroke::new(1.0, p.separator), egui::StrokeKind::Inside);
                                }
                            }
                        });
                    });
                    // Options.
                    ui.vertical(|ui| {
                        ui.set_width(280.0);
                        let o = &mut t.opts;
                        theme::form(ui, "trace-form", |f| {
                            f.row("Colors", |ui, l| {
                                let label = |n: u32| if n == 0 { "Auto".to_string() } else { n.to_string() };
                                egui::ComboBox::from_id_salt("trace-colors")
                                    .width(80.0)
                                    .selected_text(label(o.colors))
                                    .show_ui(ui, |ui| {
                                        for n in [0, 2, 3, 4, 5, 6, 7, 8, 10] {
                                            ui.selectable_value(&mut o.colors, n, label(n));
                                        }
                                    })
                                    .response
                                    .labelled_by(l);
                            });
                            f.row("Width", |ui, l| {
                                ui.add(crate::units::drag_mm(&mut o.width_mm, 5.0..=400.0, 0.5))
                                    .labelled_by(l);
                                theme::caption(ui, format!("× {}", crate::units::fmt_mm(o.width_mm * t.aspect)));
                            });
                            f.row("Detail", |ui, l| {
                                ui.add(egui::Slider::new(&mut o.detail, 0.0..=100.0).show_value(false)).labelled_by(l);
                            });
                            f.row("Ignore specks", |ui, l| {
                                ui.add(crate::units::drag_mm(&mut o.speckle_mm, 0.0..=10.0, 0.05))
                                    .labelled_by(l);
                            });
                        });
                        ui.add_space(space::S);
                        ui.spacing_mut().item_spacing.y = space::S;
                        ui.checkbox(&mut o.remove_background, "Remove white background");
                        ui.checkbox(&mut t.use_threads, format!("Use {palette_name} thread colors"));
                        ui.checkbox(&mut o.lines, "Sew thin lines as lines");
                        theme::form(ui, "trace-lines", |f| {
                            if o.lines {
                                f.row("Up to", |ui, l| {
                                    ui.add(egui::DragValue::new(&mut o.line_width_mm).range(0.3..=5.0).speed(0.05).fixed_decimals(1).suffix(" mm wide"))
                                        .labelled_by(l);
                                });
                            }
                        });
                        ui.add_space(space::S);
                        theme::caption(
                            ui,
                            "Flat logos trace best: a few solid colors, crisp edges. Photos are posterized.",
                        );
                    });
                });
                ui.add_space(space::M);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 24.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let accent = theme::pal_ui(ui).accent;
                        let ready = matches!(t.result, Some(Ok(_))) && !busy;
                        let label = match t.target {
                            Target::Open => "Open",
                            Target::Place => "Place",
                        };
                        ok = ui
                            .add_enabled(
                                ready,
                                egui::Button::new(egui::RichText::new(label).color(Color32::WHITE)).fill(accent),
                            )
                            .clicked()
                            || (ready && ui.input(|i| i.key_pressed(Key::Enter)));
                        cancel = ui.button("Cancel").clicked();
                    },
                );
            });
        if ok {
            let t = self.trace.take();
            if let Some(TraceUi {
                result: Some(Ok(r)),
                path,
                name,
                target,
                ..
            }) = t
            {
                self.finish_trace(&r, &path, &name, target);
            }
        } else if cancel || !open {
            self.trace = None;
            self.status = "Trace canceled.".into();
        }
    }

    fn finish_trace(&mut self, r: &Traced, path: &Path, name: &str, target: Target) {
        let what = format!(
            "{} ({} colors, {} objects)",
            path.display(),
            r.colors.len(),
            r.shapes + r.lines
        );
        match target {
            Target::Open => match zigzag::document::Document::from_svg(&r.svg, name, None) {
                Ok(doc) => {
                    self.open_document(doc);
                    self.status = format!("Traced {what}. Save to keep it as SVG.");
                }
                Err(e) => self.status = format!("Couldn't open the trace: {e:#}"),
            },
            Target::Place => {
                let mut added = Vec::new();
                match self.structural(|d| {
                    added = d.add_svg(&r.svg, zigzag::document::Placement::default())?;
                    Ok(())
                }) {
                    Ok(()) => {
                        self.set_selection(&added);
                        self.status = format!("Placed traced {what}.");
                    }
                    Err(e) => self.status = format!("Couldn't place the trace: {e:#}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    fn logo_png(dir: &Path) -> PathBuf {
        let png = vectorcraft_trace::Raster::from_fn(160, 80, |x, y| {
            let (fx, fy) = (x as f64 - 56.0, y as f64 - 40.0);
            if fx * fx + fy * fy < 24.0 * 24.0 {
                [220, 30, 40, 255]
            } else if (110..140).contains(&x) && (16..64).contains(&y) {
                [20, 60, 200, 255]
            } else {
                [255, 255, 255, 255]
            }
        })
        .encode_png();
        let path = dir.join("logo.png");
        std::fs::write(&path, png).unwrap();
        path
    }

    fn wait_for_trace(h: &mut Harness<'_, Studio>) {
        for _ in 0..600 {
            h.run_steps(1);
            let t = h.state().trace.as_ref().unwrap();
            let current = t.traced.as_ref() == Some(&(t.opts.clone(), t.use_threads));
            if current && t.running.is_none() && t.shown == t.asked && t.result.is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the trace never finished");
    }

    #[test]
    fn opening_a_png_traces_it_into_a_design() {
        let dir = std::env::temp_dir().join(format!("tl-trace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = logo_png(&dir);
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.state_mut().open_image(&png, Target::Open);
        wait_for_trace(&mut h);
        let r = h
            .state()
            .trace
            .as_ref()
            .unwrap()
            .result
            .clone()
            .unwrap()
            .unwrap();
        assert_eq!(r.colors.len(), 2);
        // Snap to the Brother threads, then open it.
        h.get_by_label("Use Brother thread colors").click();
        h.run_steps(2);
        wait_for_trace(&mut h);
        let r = h
            .state()
            .trace
            .as_ref()
            .unwrap()
            .result
            .clone()
            .unwrap()
            .unwrap();
        let brother: Vec<[u8; 3]> = crate::threads::chart(crate::threads::Brand::Brother)
            .into_iter()
            .map(|c| c.0)
            .collect();
        assert!(h.state().trace.as_ref().unwrap().use_threads, "checkbox");
        assert!(
            r.colors.iter().all(|c| brother.contains(c)),
            "{:?}",
            r.colors
        );
        h.get_by_label("Open").click();
        h.run_steps(3);
        assert!(h.state().trace.is_none());
        assert_eq!(h.state().doc.shapes.len(), 2);
        assert_eq!(h.state().doc.name, "logo");
        // Placing it again adds two more, selected.
        h.state_mut().open_image(&png, Target::Place);
        wait_for_trace(&mut h);
        h.get_by_label("Place").click();
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), 4);
        assert_eq!(h.state().sel_indices().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
