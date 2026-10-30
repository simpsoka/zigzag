//! The Brush group's clean-up tools, after VectorCraft's: Smooth (drag along
//! a selected path to take the wobble out of it), Path Eraser (drag across
//! one to cut that part away) and Join (scrub between two open ends to join
//! them). Plus Object › Path › Simplify… and Clean Up….

use crate::app::{Studio, Tool};
use crate::theme::space;
use eframe::egui::{self, Color32, Key, Pos2, Rect, Stroke, Vec2};
use zigzag::bezier::{self, BezPath};
use zigzag::document::{CleanUp, CleanUpReport, Document};
use zigzag::geom::P;
use zigzag::pathtools::{self, End};

/// Smooth's reach either side of the drag (screen points), as VectorCraft's.
const SMOOTH_REACH: f64 = 12.0;

/// How far (screen points) the Smooth tool's curves may stray from the path
/// at Smoothness `s` (0–100): 1 pt … 16 pt, 4 pt in the middle
/// (VectorCraft's fixed 2.5 pt is about 33).
fn smooth_fidelity(s: f64) -> f64 {
    16f64.powf(s.clamp(0.0, 100.0) / 100.0)
}
/// Half the Path Eraser's width (screen points).
const ERASE_REACH: f64 = 5.0;
/// How near the scrub an end must be for Join, and how near two ends must
/// be to merge into one point (screen points).
const JOIN_REACH: f64 = 16.0;
const JOIN_MERGE: f64 = 4.0;

/// A drag with one of the tools.
#[derive(Default)]
pub struct Gesture {
    /// Where the pointer went (design units).
    trail: Vec<P>,
    /// Join: the open ends it could join (object uid, end, where).
    ends: Vec<(usize, End, P)>,
}

/// Distance from `q` to the polyline `trail`.
fn trail_dist(trail: &[P], q: P) -> f64 {
    match trail {
        [] => f64::INFINITY,
        [a] => a.dist(q),
        _ => trail
            .windows(2)
            .map(|w| {
                let d = w[1] - w[0];
                let l2 = d.dot(d);
                let t = if l2 > 1e-12 {
                    ((q - w[0]).dot(d) / l2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (w[0] + d * t).dist(q)
            })
            .fold(f64::INFINITY, f64::min),
    }
}

/// A test for "within `r` of the trail", quick to say no far from it.
fn near_trail(trail: &[P], r: f64) -> impl Fn(P) -> bool + '_ {
    let mut b = zigzag::geom::Bounds::empty();
    trail.iter().for_each(|&p| b.add(p));
    move |q: P| {
        q.x >= b.min.x - r
            && q.x <= b.max.x + r
            && q.y >= b.min.y - r
            && q.y <= b.max.y + r
            && trail_dist(trail, q) <= r
    }
}

/// The Simplify window: VectorCraft's slider strip (0: fewest points … 100:
/// closest to the original), previewing on the canvas.
#[derive(Clone)]
pub struct SimplifyDialog {
    pub level: f64,
    pub auto: bool,
    /// Auto-Simplify's level for the selection's size.
    auto_level: f64,
    base: Document,
    indices: Vec<usize>,
    shown: Option<f64>,
    before: usize,
    after: usize,
}

/// The Clean Up window: what to take out, and how much of each there is.
#[derive(Clone)]
pub struct CleanUpDialog {
    pub opt: CleanUp,
    found: Option<(CleanUp, CleanUpReport)>,
}

fn points(doc: &Document, indices: &[usize]) -> usize {
    let mut seen = std::collections::HashSet::new();
    indices
        .iter()
        .filter(|&&i| doc.shapes.get(i).is_some_and(|s| seen.insert(s.id.clone())))
        .filter_map(|&i| doc.edit_path(i).ok())
        .map(|p| p.iter().map(|sp| sp.anchors.len()).sum::<usize>())
        .sum()
}

impl Studio {
    /// A dialog is previewing into the document: nothing else may edit it.
    pub fn previewing(&self) -> bool {
        self.layout.corner_dialog.is_some() || self.layout.simplify_dialog.is_some()
    }

    /// Canvas input and drawing for Smooth, Path Eraser and Join.
    pub fn cleanup_canvas(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        painter: &egui::Painter,
        rect: Rect,
    ) {
        let px = self.view.scale as f64 / 10.0; // screen points per design unit
        let tool = self.tool;
        let reach = match tool {
            Tool::Smooth => SMOOTH_REACH,
            Tool::PathEraser => ERASE_REACH,
            _ => JOIN_REACH,
        };
        match tool {
            Tool::Join => {
                self.want_cursor(crate::cursors::Glyph::Cross(Some(crate::icons::Icon::Join)))
            }
            _ => self.want_cursor(crate::cursors::Glyph::Brush(reach as f32)),
        }
        let accent = Color32::from_rgb(40, 140, 255);
        // The selected paths' outlines, to see what the tool works on.
        let sel = self.sel_indices();
        for &i in sel.iter().take(64) {
            if let Some(path) = self.outline_of(i) {
                draw_path(painter, &path, |q| self.view.to_screen(rect, q), accent);
            }
        }
        if resp.drag_started_by(egui::PointerButton::Primary)
            && let Some(at) = ui.input(|i| i.pointer.press_origin())
        {
            let q = self.view.to_design(rect, at);
            // Nothing selected: Smooth and the Eraser take the path pressed on.
            if sel.is_empty()
                && tool != Tool::Join
                && let Some(i) = self.pick(q, reach / px)
            {
                self.set_selection(&[i]);
            }
            self.gesture = Gesture {
                trail: vec![q],
                ends: if tool == Tool::Join {
                    self.joinable_ends()
                } else {
                    Vec::new()
                },
            };
        }
        if !self.gesture.trail.is_empty()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let q = self.view.to_design(rect, pos);
            if self
                .gesture
                .trail
                .last()
                .is_none_or(|l| l.dist(q) * px >= 1.5)
            {
                self.gesture.trail.push(q);
            }
            ui.ctx().request_repaint();
        }
        // The drag so far: the brush's swath, or Join's scrub and the ends it reaches.
        if self.gesture.trail.len() > 1 {
            let pts: Vec<Pos2> = self
                .gesture
                .trail
                .iter()
                .map(|&p| self.view.to_screen(rect, p))
                .collect();
            if tool == Tool::Join {
                painter.add(egui::Shape::line(pts, Stroke::new(1.5, accent)));
                for (_, _, at) in self.join_pick(reach / px) {
                    painter.circle(
                        self.view.to_screen(rect, at),
                        6.0,
                        accent.gamma_multiply(0.35),
                        Stroke::new(1.5, accent),
                    );
                }
            } else {
                let tint = if tool == Tool::PathEraser {
                    Color32::from_rgb(230, 80, 70)
                } else {
                    accent
                };
                painter.add(egui::Shape::line(
                    pts,
                    Stroke::new(reach as f32 * 2.0, tint.gamma_multiply(0.22)),
                ));
            }
        }
        if resp.drag_stopped() {
            let g = std::mem::take(&mut self.gesture);
            match tool {
                Tool::Join => self.finish_scrub_join(&g, px),
                _ => self.finish_brushing(tool, &g.trail, px),
            }
        }
        let msg = match tool {
            Tool::Smooth => {
                "Drag along a selected path to smooth it: fewer points, smoother curves · drag again for more"
            }
            Tool::PathEraser => "Drag across a selected path to erase that part of it",
            _ => {
                "Scrub between two open ends to join them (selected paths, or any) · an open path's own ends close it"
            }
        };
        self.hud(painter, rect, msg);
        if tool == Tool::Smooth {
            self.smooth_options(ui.ctx(), ui.make_persistent_id("smooth-options"), rect);
        }
    }

    /// The bar at the top of the canvas for Smooth: how smooth.
    fn smooth_options(&mut self, ctx: &egui::Context, id: egui::Id, rect: Rect) {
        egui::Area::new(id)
            .pivot(egui::Align2::CENTER_TOP)
            .fixed_pos(rect.center_top() + Vec2::new(0.0, space::M))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .corner_radius(9)
                    .inner_margin(egui::Margin::symmetric(space::S as i8, space::XS as i8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = space::S;
                            let p = crate::theme::pal_ui(ui);
                            ui.label(
                                egui::RichText::new("Smoothness").size(12.0).color(p.secondary),
                            );
                            ui.add(
                                egui::Slider::new(&mut self.brush.smooth_tool, 0.0..=100.0)
                                    .show_value(false),
                            )
                            .on_hover_text(
                                "How far a stroke may move the path to smooth it: right for smoother, left to stay closer",
                            );
                        });
                    });
            });
    }

    /// Smooth or erase the selected paths along `trail`.
    fn finish_brushing(&mut self, tool: Tool, trail: &[P], px: f64) {
        let idx = self.sel_indices();
        if idx.is_empty() {
            self.status = "Select a path first (or start the drag on one).".into();
            return;
        }
        let uids: Vec<usize> = idx.iter().map(|&i| self.doc.shapes[i].uid).collect();
        let mut result = (0, 0);
        let r = match tool {
            Tool::Smooth => {
                let near = near_trail(trail, SMOOTH_REACH / px);
                let tol = smooth_fidelity(self.brush.smooth_tool) / px;
                self.structural(|d| {
                    result = d.edit_paths_with(&idx, |p| pathtools::smooth_near(p, &near, tol))?;
                    Ok(())
                })
            }
            _ => {
                let near = near_trail(trail, ERASE_REACH / px);
                let min_len = 2.0 / px;
                self.structural(|d| {
                    result =
                        d.edit_paths_with(&idx, |p| pathtools::erase_near(p, &near, min_len))?;
                    Ok(())
                })
            }
        };
        match r {
            Ok(()) if result == (0, 0) => {
                self.undo.pop();
                self.status = if tool == Tool::Smooth {
                    "Already smooth there.".into()
                } else {
                    "Nothing of the selected paths under the eraser.".into()
                };
            }
            Ok(()) => {
                self.restore_selection(&uids);
                let (changed, gone) = result;
                self.status = match (tool, gone) {
                    (Tool::Smooth, _) => format!("Smoothed {changed} path(s)."),
                    (_, 0) => format!("Erased part of {changed} path(s)."),
                    _ => format!("Erased {gone} path(s) entirely."),
                };
            }
            Err(e) => {
                self.undo.pop();
                self.status = format!("Couldn't change the path: {e:#}");
            }
        }
    }

    /// Open ends Join could take: of the selected paths, or (none selected) any.
    fn joinable_ends(&mut self) -> Vec<(usize, End, P)> {
        let mut idx = self.sel_indices();
        if idx.is_empty() {
            idx = (0..self.doc.shapes.len()).collect();
        }
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for i in idx {
            let s = &self.doc.shapes[i];
            let (uid, id) = (s.uid, s.id.clone());
            if !seen.insert(id) {
                continue;
            }
            if let Some(path) = self.outline_of(i) {
                out.extend(
                    pathtools::open_ends(&path)
                        .into_iter()
                        .map(|(e, at)| (uid, e, at)),
                );
            }
        }
        out
    }

    /// The two ends the scrub joins: the one nearest where it began and the
    /// one nearest where it is now, each within `reach` of the scrub.
    fn join_pick(&self, reach: f64) -> Vec<(usize, End, P)> {
        let t = &self.gesture.trail;
        let (Some(&first), Some(&last)) = (t.first(), t.last()) else {
            return Vec::new();
        };
        let near: Vec<&(usize, End, P)> = self
            .gesture
            .ends
            .iter()
            .filter(|e| trail_dist(t, e.2) <= reach)
            .collect();
        let best = |to: P, not: Option<&(usize, End, P)>| {
            near.iter()
                .filter(|e| not.is_none_or(|n| (n.0, n.1) != (e.0, e.1)))
                .min_by(|a, b| a.2.dist(to).total_cmp(&b.2.dist(to)))
                .copied()
        };
        let Some(a) = best(first, None) else {
            return Vec::new();
        };
        match best(last, Some(a)) {
            Some(b) => vec![*a, *b],
            None => vec![*a],
        }
    }

    fn finish_scrub_join(&mut self, g: &Gesture, px: f64) {
        self.gesture.ends = g.ends.clone();
        self.gesture.trail = g.trail.clone();
        let pick = self.join_pick(JOIN_REACH / px);
        self.gesture = Gesture::default();
        let [(ua, ea, _), (ub, eb, _)] = pick[..] else {
            self.status = "Scrub from one open end to another to join them.".into();
            return;
        };
        let find = |s: &Studio, uid| s.doc.shapes.iter().position(|x| x.uid == uid);
        let (Some(a), Some(b)) = (find(self, ua), find(self, ub)) else {
            return;
        };
        let mut joined = 0;
        match self.structural(|d| {
            joined = d.join_ends(a, ea, b, eb, JOIN_MERGE / px)?;
            Ok(())
        }) {
            Ok(()) => {
                self.set_selection(&[joined]);
                self.status = if ua == ub && ea.0 == eb.0 {
                    "Closed the path.".into()
                } else {
                    "Joined.".into()
                };
            }
            Err(e) => {
                self.undo.pop();
                self.status = format!("Couldn't join: {e:#}");
            }
        }
    }

    /// Open Object › Path › Simplify… for the selection.
    pub fn open_simplify_dialog(&mut self) {
        let indices = self.sel_indices();
        if indices.is_empty() {
            return;
        }
        let mut b = zigzag::geom::Bounds::empty();
        for &i in &indices {
            if let Ok(p) = self.doc.edit_path(i) {
                let pb = bezier::bounds(&p);
                if !pb.is_empty() {
                    b.add(pb.min);
                    b.add(pb.max);
                }
            }
        }
        let size = if b.is_empty() {
            0.0
        } else {
            b.width().max(b.height()) / pathtools::PT
        };
        let auto_level = pathtools::auto_simplify_level(size);
        let before = points(&self.doc, &indices);
        self.layout.simplify_dialog = Some(SimplifyDialog {
            level: auto_level,
            auto: true,
            auto_level,
            base: self.doc.clone(),
            indices,
            shown: None,
            before,
            after: before,
        });
    }

    /// The Simplify window, previewing on the canvas as the slider moves.
    pub fn simplify_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.layout.simplify_dialog.take() else {
            return;
        };
        let (mut ok, mut cancel) = (false, false);
        use crate::dialog;
        let id = egui::Id::unique("simplify");
        dialog::show(ctx, id, dialog::Place::Top, |ui| {
            dialog::heading(ui, "Simplify");
            ui.horizontal(|ui| {
                let p = crate::theme::pal_ui(ui);
                ui.label(
                    egui::RichText::new("Fewer points")
                        .size(12.0)
                        .color(p.secondary),
                );
                let r = ui
                    .add(
                        egui::Slider::new(&mut d.level, 0.0..=100.0)
                            .show_value(false)
                            .step_by(1.0),
                    )
                    .on_hover_text("Left for fewer points, right to keep closer to the original");
                if r.changed() {
                    d.auto = false;
                }
                ui.label(
                    egui::RichText::new("More detail")
                        .size(12.0)
                        .color(p.secondary),
                );
            });
            if ui
                .checkbox(&mut d.auto, "Auto-Simplify")
                .on_hover_text("The level for the selection's size")
                .changed()
                && d.auto
            {
                d.level = d.auto_level;
            }
            crate::theme::caption(
                ui,
                &if d.after < d.before {
                    format!("{} points, {} after simplifying.", d.before, d.after)
                } else {
                    format!("{} points: already as simple as this level gets.", d.before)
                },
            );
            dialog::buttons(ui, |ui| {
                ok = dialog::primary(ui, "OK", true).clicked()
                    || ui.input(|i| i.key_pressed(Key::Enter));
                cancel = dialog::secondary(ui, "Cancel").clicked()
                    || ui.input(|i| i.key_pressed(Key::Escape));
            });
        });
        if d.shown != Some(d.level) {
            let mut doc = d.base.clone();
            let tol = pathtools::simplify_level_tolerance(d.level) * pathtools::PT;
            let removed = doc.simplify_paths(&d.indices, tol).unwrap_or(0);
            d.after = d.before.saturating_sub(removed);
            self.doc = doc;
            self.dirty = true;
            d.shown = Some(d.level);
        }
        let uids: Vec<usize> = d
            .indices
            .iter()
            .filter_map(|&i| d.base.shapes.get(i))
            .map(|s| s.uid)
            .collect();
        if ok {
            let after = std::mem::replace(&mut self.doc, d.base.clone());
            if d.after < d.before {
                let _ = self.structural(|doc| {
                    *doc = after;
                    Ok(())
                });
                self.status = format!("Simplified: {} points, {} before.", d.after, d.before);
            }
            self.restore_selection(&uids);
            self.dirty = true;
        } else if cancel {
            self.doc = d.base;
            self.restore_selection(&uids);
            self.dirty = true;
        } else {
            self.layout.simplify_dialog = Some(d);
        }
    }

    /// Cancel a previewing dialog (Escape), putting the document back.
    pub fn cancel_previews(&mut self) {
        if let Some(d) = self.layout.corner_dialog.take() {
            self.doc = d.base;
            self.dirty = true;
        }
        if let Some(d) = self.layout.simplify_dialog.take() {
            self.doc = d.base;
            self.dirty = true;
        }
    }

    /// Open Object › Path › Clean Up….
    pub fn open_cleanup_dialog(&mut self) {
        self.layout.cleanup_dialog = Some(CleanUpDialog {
            opt: CleanUp::default(),
            found: None,
        });
    }

    /// The Clean Up window: what to take out, with how much of each is there.
    pub fn cleanup_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.layout.cleanup_dialog.take() else {
            return;
        };
        if d.found.is_none_or(|(o, _)| o != d.opt) {
            let mut doc = self.doc.clone();
            let r = doc.clean_up(&d.opt, true).unwrap_or_default();
            d.found = Some((d.opt, r));
        }
        let found = d.found.map(|f| f.1).unwrap_or_default();
        let (mut ok, mut cancel) = (false, false);
        let mut open = true;
        egui::Window::new("Clean Up")
            .title_frame(crate::theme::title_frame(ctx))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, 90.0))
            .show(ctx, |ui| {
                let count = |n: usize| {
                    if n == 0 {
                        " (none)".to_string()
                    } else {
                        format!(" ({n})")
                    }
                };
                ui.checkbox(
                    &mut d.opt.stray,
                    format!("Stray points{}", count(found.stray)),
                )
                .on_hover_text("Objects with only one point, or no size at all");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut d.opt.specks, "Objects smaller than")
                        .on_hover_text("Too small to sew: they stitch as a knot");
                    let mut mm = d.opt.speck / 10.0;
                    if ui
                        .add_enabled(
                            d.opt.specks,
                            crate::units::drag_mm(&mut mm, 0.1..=10.0, 0.05),
                        )
                        .changed()
                    {
                        d.opt.speck = mm * 10.0;
                    }
                    ui.label(count(found.specks).trim());
                });
                ui.checkbox(
                    &mut d.opt.points,
                    format!("Extra points{}", count(found.points)),
                )
                .on_hover_text(
                    "Points on top of each other, and points in the middle of straight lines",
                );
                ui.add_space(space::S);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width().max(240.0), 24.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let accent = crate::theme::pal_ui(ui).accent;
                        ok = ui
                            .add_enabled(
                                found.total() > 0,
                                egui::Button::new(egui::RichText::new("OK").color(Color32::WHITE))
                                    .fill(accent),
                            )
                            .clicked()
                            || (found.total() > 0 && ui.input(|i| i.key_pressed(Key::Enter)));
                        cancel = ui.button("Cancel").clicked()
                            || ui.input(|i| i.key_pressed(Key::Escape));
                    },
                );
            });
        if ok {
            let opt = d.opt;
            let mut done = CleanUpReport::default();
            match self.structural(|doc| {
                done = doc.clean_up(&opt, false)?;
                Ok(())
            }) {
                Ok(()) => {
                    self.set_selection(&[]);
                    self.status = cleanup_summary(&done);
                }
                Err(e) => {
                    self.undo.pop();
                    self.status = format!("Couldn't clean up: {e:#}");
                }
            }
        } else if !(cancel || !open) {
            self.layout.cleanup_dialog = Some(d);
        }
    }
}

/// "Removed 2 stray points and 14 extra points."
pub fn cleanup_summary(r: &CleanUpReport) -> String {
    let mut parts = Vec::new();
    if r.stray > 0 {
        parts.push(format!("{} stray point(s)", r.stray));
    }
    if r.specks > 0 {
        parts.push(format!("{} tiny object(s)", r.specks));
    }
    if r.points > 0 {
        parts.push(format!("{} extra point(s)", r.points));
    }
    if parts.is_empty() {
        "Nothing to clean up.".into()
    } else {
        format!("Removed {}.", parts.join(", "))
    }
}

/// A path's outline on screen.
fn draw_path(painter: &egui::Painter, path: &BezPath, to: impl Fn(P) -> Pos2, c: Color32) {
    for sp in path {
        let mut pts = Vec::new();
        for (a, b) in sp.segments() {
            let s = bezier::sample(&sp.cubic(a, b), 12);
            let skip = usize::from(!pts.is_empty());
            pts.extend(s.into_iter().skip(skip).map(&to));
        }
        if pts.len() > 1 {
            painter.add(egui::Shape::line(pts, Stroke::new(1.0, c)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::Action;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    /// A wobbly hand-drawn line, two lines end to end, a box and a speck.
    fn art() -> String {
        let mut d = String::from("M10 50");
        for k in 1..=40 {
            let x = 10.0 + 2.0 * k as f64;
            let y = 50.0 + 0.4 * (x / 6.0).sin() + if k % 2 == 0 { 0.06 } else { -0.06 };
            d.push_str(&format!(" L{x:.3} {y:.3}"));
        }
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100mm" height="100mm" viewBox="0 0 100 100">
  <path id="wob" d="{d}" stroke="#000" fill="none" stroke-width="0.4"/>
  <path id="a" d="M20 80 L45 80" stroke="#00c" fill="none" stroke-width="0.4"/>
  <path id="b" d="M50 80 L80 80" stroke="#00c" fill="none" stroke-width="0.4"/>
  <rect id="box" x="20" y="15" width="20" height="20" fill="#c00"/>
  <rect id="speck" x="70" y="20" width="0.3" height="0.3" fill="#0a0"/>
</svg>"##
        )
    }

    fn studio() -> Harness<'static, Studio> {
        // (A file of its own per test: they run at once, and a reader of a
        // file another test is rewriting would get half of it.)
        static NTH: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let nth = NTH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tl-pathtools-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("art-{nth}.svg"));
        std::fs::write(&file, art()).unwrap();
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(Some(file), None),
            );
        h.run_steps(4);
        h
    }

    fn find(h: &Harness<'_, Studio>, id: &str) -> Option<usize> {
        let tagged = format!("{id}_");
        h.state()
            .doc
            .shapes
            .iter()
            .position(|s| s.id == id || s.id.starts_with(&tagged))
    }

    fn path(h: &Harness<'_, Studio>, id: &str) -> BezPath {
        h.state().doc.edit_path(find(h, id).unwrap()).unwrap()
    }

    fn screen(h: &Harness<'_, Studio>, p: P) -> Pos2 {
        h.state().view.to_screen(h.state().canvas_rect, p)
    }

    /// Drag through `pts` (screen), a few steps between each.
    fn drag(h: &mut Harness<'_, Studio>, pts: &[Pos2]) {
        h.hover_at(pts[0]);
        h.run_steps(1);
        h.drag_at(pts[0]);
        h.run_steps(1);
        for w in pts.windows(2) {
            for k in 1..=4 {
                h.hover_at(w[0] + (w[1] - w[0]) * (k as f32 / 4.0));
                h.run_steps(1);
            }
        }
        h.drop_at(pts[pts.len() - 1]);
        h.run_steps(3);
    }

    fn count(p: &BezPath) -> usize {
        p.iter().map(|sp| sp.anchors.len()).sum()
    }

    #[test]
    fn the_brush_slot_holds_the_clean_up_tools() {
        let mut h = studio();
        // Hold the Brush down: its group's flyout.
        let brush = h.get_by_label("Brush").rect().center();
        h.hover_at(brush);
        h.run_steps(1);
        h.drag_at(brush);
        h.run_steps(40);
        for name in ["Brush Tool", "Smooth Tool", "Path Eraser Tool", "Join Tool"] {
            h.get_by_label(name);
        }
        let item = h.get_by_label("Path Eraser Tool").rect().center();
        h.hover_at(item);
        h.run_steps(1);
        h.drop_at(item);
        h.run_steps(3);
        assert_eq!(h.state().tool, Tool::PathEraser);
        assert_eq!(h.state().view_tools.brush_slot, Tool::PathEraser);
    }

    #[test]
    fn smoothing_along_a_wobbly_line_takes_points_out() {
        let mut h = studio();
        let before = path(&h, "wob");
        assert_eq!(count(&before), 41);
        h.state_mut().set_tool(Tool::Smooth);
        // Nothing selected: the drag starts on the line and takes it.
        let a = &before[0].anchors;
        let pts: Vec<Pos2> = [2, 12, 22, 32, 40]
            .iter()
            .map(|&k| screen(&h, a[k].p))
            .collect();
        drag(&mut h, &pts);
        let after = path(&h, "wob");
        assert!(count(&after) < 20, "{} points", count(&after));
        assert_eq!(h.state().sel_indices(), vec![find(&h, "wob").unwrap()]);
        // One undo step puts it back.
        h.state_mut().apply_history(crate::app::History::Undo);
        h.run_steps(2);
        assert_eq!(count(&path(&h, "wob")), 41);
    }

    #[test]
    fn the_path_eraser_cuts_a_line_in_two() {
        let mut h = studio();
        let i = find(&h, "a").unwrap();
        h.state_mut().set_selection(&[i]);
        h.state_mut().set_tool(Tool::PathEraser);
        let line = path(&h, "a");
        let mid = (line[0].anchors[0].p + line[0].anchors[1].p) * 0.5;
        let c = screen(&h, mid);
        drag(
            &mut h,
            &[c + Vec2::new(0.0, -30.0), c + Vec2::new(0.0, 30.0)],
        );
        let cut = path(&h, "a");
        assert_eq!(cut.len(), 2, "{cut:?}");
    }

    #[test]
    fn join_scrubs_two_ends_together() {
        let mut h = studio();
        let n = h.state().doc.shapes.len();
        h.state_mut().set_tool(Tool::Join);
        h.run_steps(1);
        let (ea, sb) = (path(&h, "a")[0].anchors[1].p, path(&h, "b")[0].anchors[0].p);
        let pts = [screen(&h, ea), screen(&h, sb)];
        drag(&mut h, &pts);
        assert_eq!(h.state().doc.shapes.len(), n - 1, "{}", h.state().status);
        let joined = path(&h, "a");
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].anchors.len(), 4);
        assert!(find(&h, "b").is_none());
    }

    #[test]
    fn simplify_previews_and_cancels_or_keeps() {
        let mut h = studio();
        let i = find(&h, "wob").unwrap();
        h.state_mut().set_selection(&[i]);
        h.state_mut().run_action(Action::SimplifyPath);
        h.run_steps(3);
        h.get_by_label("Auto-Simplify");
        let shown = count(&path(&h, "wob"));
        assert!(shown < 41, "the preview is on the canvas ({shown})");
        // Escape: as it was, no undo step.
        let undo = h.state().undo.len();
        h.key_press(Key::Escape);
        h.run_steps(3);
        assert!(h.state().layout.simplify_dialog.is_none());
        assert_eq!(count(&path(&h, "wob")), 41);
        assert_eq!(h.state().undo.len(), undo);
        // Again, all the way to the fewest points, and OK.
        h.state_mut().run_action(Action::SimplifyPath);
        h.run_steps(2);
        if let Some(d) = &mut h.state_mut().layout.simplify_dialog {
            d.level = 0.0;
            d.auto = false;
        }
        h.run_steps(2);
        h.get_by_label("OK").click();
        h.run_steps(3);
        assert!(h.state().layout.simplify_dialog.is_none());
        let kept = count(&path(&h, "wob"));
        assert!(kept <= shown, "{kept} vs {shown}");
        assert_eq!(h.state().undo.len(), undo + 1);
    }

    #[test]
    fn clean_up_takes_out_the_speck() {
        let mut h = studio();
        assert!(find(&h, "speck").is_some());
        h.state_mut().run_action(Action::CleanUp);
        h.run_steps(3);
        h.get_by_label("(1)");
        h.get_by_label("OK").click();
        h.run_steps(3);
        assert!(find(&h, "speck").is_none(), "{}", h.state().status);
        assert!(h.state().layout.cleanup_dialog.is_none());
        assert!(
            h.state().status.contains("1 tiny object"),
            "{}",
            h.state().status
        );
    }

    #[test]
    fn agents_can_simplify_and_clean_up() {
        let mut h = studio();
        let s = h.state_mut();
        let r = s.run_tool("clean_up", &serde_json::json!({ "dry_run": true }));
        assert!(!r.is_error);
        let n = s.doc.shapes.len();
        let r = s.run_tool("clean_up", &serde_json::json!({}));
        assert!(!r.is_error);
        assert_eq!(s.doc.shapes.len(), n - 1);
        let name = s.names[s
            .doc
            .shapes
            .iter()
            .position(|x| x.id.starts_with("wob"))
            .unwrap()]
        .clone();
        let r = s.run_tool("simplify", &serde_json::json!({ "objects": [name] }));
        assert!(!r.is_error);
        let i = s
            .doc
            .shapes
            .iter()
            .position(|x| x.id.starts_with("wob"))
            .unwrap();
        assert!(count(&s.doc.edit_path(i).unwrap()) < 41);
    }
}
