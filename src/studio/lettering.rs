//! Lettering in the studio: the "Add text" window and the editor for a
//! selected lettering object.

use crate::app::Studio;
use eframe::egui;
use crate::fonts::{FontItem, Have};
use zigzag::document::{TextSpec, font_families};
use zigzag::geom::P;
use zigzag::params::Kind;

#[derive(Default)]
pub struct LetteringState {
    /// The "Add text" window, when open (Object › Add Text…).
    pub new: Option<TextSpec>,
    /// Edits to the selected lettering object: (uid, settings).
    pub edit: Option<(usize, TextSpec)>,
    pub fonts: Vec<String>,
    /// How many font faces there were when `fonts` was read.
    pub fonts_faces: usize,
    pub font_filter: String,
    /// The Type tool is typing into this object (uid), in the box over the
    /// canvas; `typed` is the text as typed so far.
    pub typing: Option<Typing>,
    /// The settings the next text gets (the last ones used).
    pub last: Option<TextSpec>,
}

/// Text being typed on the canvas with the Type tool.
#[derive(Clone, Debug)]
pub struct Typing {
    pub uid: usize,
    pub text: String,
    /// The caret: a character index into `text`, 0..=chars.
    pub cursor: usize,
    /// Where the caret goes when the text is empty: the left edge, top and
    /// bottom of where the letters were (design units), kept from the
    /// last frame that had letters, so the frame and caret stay put.
    pub empty_frame: Option<(f64, f64, f64)>,
    /// The text is still the placeholder "Text" it was placed with,
    /// selected (as Illustrator's is): typing replaces it, Delete takes it
    /// away, and ending the edit untouched takes the new text away again.
    pub placeholder: bool,
    /// Placed by this edit (so ending it with nothing typed takes it away
    /// again), rather than text that was already there.
    pub fresh: bool,
    /// The other end of the selection (the caret is one end); None when
    /// nothing is selected.
    pub anchor: Option<usize>,
    /// A drag that started on the text is selecting.
    pub dragging: bool,
    /// The text as laid out (caret lines and glyph boxes), for the
    /// document revision it was computed from.
    pub layout: Option<(u64, Layout)>,
}

/// The laid-out text: carets between the letters and the letters' boxes.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub carets: zigzag::document::Carets,
    pub boxes: Vec<zigzag::document::GlyphBox>,
}

impl Typing {
    fn new(uid: usize, text: String, cursor: usize, placeholder: bool) -> Self {
        let n = text.chars().count();
        Typing {
            uid,
            text,
            cursor: cursor.min(n),
            empty_frame: None,
            placeholder,
            fresh: placeholder,
            anchor: None,
            dragging: false,
            layout: None,
        }
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    /// The selected range of characters, if any.
    pub fn selection(&self) -> Option<std::ops::Range<usize>> {
        let a = self.anchor?;
        if a == self.cursor {
            return None;
        }
        Some(a.min(self.cursor)..a.max(self.cursor))
    }

    pub fn selected_text(&self) -> String {
        match self.selection() {
            Some(r) => self
                .text
                .chars()
                .skip(r.start)
                .take(r.end - r.start)
                .collect(),
            None => String::new(),
        }
    }

    /// Remove the selection (the caret goes to its start). False if there
    /// was none.
    fn delete_selection(&mut self) -> bool {
        let Some(r) = self.selection() else {
            self.anchor = None;
            return false;
        };
        let (a, b) = (self.byte(r.start), self.byte(r.end));
        self.text.replace_range(a..b, "");
        self.cursor = r.start;
        self.anchor = None;
        true
    }

    /// Put the caret at `k`, extending the selection from where it was
    /// (Shift) or dropping it.
    fn move_to(&mut self, k: usize, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = k.min(self.len());
    }

    fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.len();
    }

    /// Select the word around character `k` (whitespace splits words).
    fn select_word_at(&mut self, k: usize) {
        let cs: Vec<char> = self.text.chars().collect();
        if cs.is_empty() {
            return;
        }
        let k = k.min(cs.len().saturating_sub(1));
        let (mut a, mut b) = (k, k + 1);
        if cs[k].is_whitespace() {
            while a > 0 && cs[a - 1].is_whitespace() && cs[a - 1] != '\n' {
                a -= 1;
            }
            while b < cs.len() && cs[b].is_whitespace() && cs[b] != '\n' {
                b += 1;
            }
        } else {
            while a > 0 && !cs[a - 1].is_whitespace() {
                a -= 1;
            }
            while b < cs.len() && !cs[b].is_whitespace() {
                b += 1;
            }
        }
        self.anchor = Some(a);
        self.cursor = b;
    }

    /// Byte offset of character `k`.
    fn byte(&self, k: usize) -> usize {
        self.text
            .char_indices()
            .nth(k)
            .map(|(b, _)| b)
            .unwrap_or(self.text.len())
    }

    /// Type `s` at the caret, in place of the selection if there is one.
    fn insert(&mut self, s: &str) {
        self.delete_selection();
        let b = self.byte(self.cursor);
        self.text.insert_str(b, s);
        self.cursor += s.chars().count();
    }

    /// Delete the selection, else the character before the caret (or the
    /// word, back to the previous space).
    fn backspace(&mut self, word: bool) {
        if self.delete_selection() || self.cursor == 0 {
            return;
        }
        let to = if word {
            let before: Vec<char> = self.text.chars().take(self.cursor).collect();
            let mut k = before.len();
            while k > 0 && before[k - 1] == ' ' {
                k -= 1;
            }
            while k > 0 && before[k - 1] != ' ' && before[k - 1] != '\n' {
                k -= 1;
            }
            k
        } else {
            self.cursor - 1
        };
        let (a, b) = (self.byte(to), self.byte(self.cursor));
        self.text.replace_range(a..b, "");
        self.cursor = to;
    }

    /// Delete the selection, else the character after the caret.
    fn delete_forward(&mut self) {
        if self.delete_selection() || self.cursor >= self.len() {
            return;
        }
        let (a, b) = (self.byte(self.cursor), self.byte(self.cursor + 1));
        self.text.replace_range(a..b, "");
    }

    /// Start (or end) of the caret's line.
    fn line_edge(&self, end: bool) -> usize {
        let cs: Vec<char> = self.text.chars().collect();
        if end {
            let mut k = self.cursor;
            while k < cs.len() && cs[k] != '\n' {
                k += 1;
            }
            k
        } else {
            let mut k = self.cursor;
            while k > 0 && cs[k - 1] != '\n' {
                k -= 1;
            }
            k
        }
    }

    /// The text as laid out in `doc` (cached per document revision).
    fn layout(&mut self, doc: &zigzag::document::Document, i: usize) -> &Layout {
        let rev = doc.rev();
        if self.layout.as_ref().is_none_or(|(r, _)| *r != rev) {
            self.layout = Some((
                rev,
                Layout {
                    carets: doc.carets(i),
                    boxes: doc.glyph_boxes(i),
                },
            ));
        }
        &self.layout.as_ref().unwrap().1
    }

    /// The caret lines for the text as it is in `doc` (cached per revision).
    fn carets(&mut self, doc: &zigzag::document::Document, i: usize) -> &zigzag::document::Carets {
        &self.layout(doc, i).carets
    }
}

/// What a font menu lists: a search field (the keyboard's when the menu
/// opens, so typing narrows the list straight away) and the fonts: those
/// on this computer, and the rest of ZigZag's catalogue greyed out with a
/// download mark (picking one downloads it). Fonts made for embroidery
/// carry a stitch mark. ↑ and ↓ step through the fonts listed that are
/// here (not the catalogue's, which a click picks: each stepped past would
/// be downloaded), the text
/// taking each as it's reached and the list scrolling to it; Enter closes
/// the menu (as Escape and a click on a font do). True when the font
/// changed.
pub(crate) fn font_list(
    ui: &mut egui::Ui,
    font: &mut String,
    fonts: &[FontItem],
    filter: &mut String,
) -> bool {
    // (Before the search field sees them: there they'd move its caret.)
    let (up, down, enter) = ui.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
        )
    });
    let field = ui.add(egui::TextEdit::singleline(filter).hint_text("Search fonts"));
    // Just opened (it wasn't shown last frame)?
    let seen = ui.scope_id().with("font-list-seen");
    let frame = ui.ctx().cumulative_frame_nr();
    let opened = ui
        .data(|d| d.get_temp::<u64>(seen))
        .is_none_or(|last| last + 1 < frame);
    ui.data_mut(|d| d.insert_temp(seen, frame));
    if opened {
        field.request_focus();
    }
    let f = filter.to_lowercase();
    let listed: Vec<&FontItem> = fonts
        .iter()
        .filter(|n| f.is_empty() || n.name.to_lowercase().contains(&f))
        .collect();
    let mut changed = false;
    // The font to bring into view: the one in use as the menu opens, the
    // one stepped to.
    let mut reveal = opened;
    // (Through the fonts that are here: each one stepped to is set, and a
    // catalogue font stepped past would be downloaded on the way. Those are
    // picked with a click.)
    let steps: Vec<&FontItem> = listed
        .iter()
        .copied()
        .filter(|n| n.have == Have::Here || n.name == *font)
        .collect();
    if (up || down) && !steps.is_empty() {
        let last = steps.len() - 1;
        let k = match (steps.iter().position(|n| n.name == *font), down) {
            (Some(k), true) => (k + 1).min(last),
            (Some(k), false) => k.saturating_sub(1),
            (None, true) => 0,
            (None, false) => last,
        };
        if steps[k].name != *font {
            *font = steps[k].name.clone();
            changed = true;
        }
        reveal = true;
    }
    let p = crate::theme::pal_ui(ui);
    for item in listed {
        let here = item.have == Have::Here;
        let label = if here {
            egui::RichText::new(&item.name)
        } else {
            egui::RichText::new(&item.name).color(p.secondary)
        };
        let r = ui.selectable_value(font, item.name.clone(), label);
        // Marks at the row's right: made for embroidery, to download.
        let side = 12.0;
        let mut x = r.rect.right() - ui.spacing().button_padding.x - side;
        let mark = |x: f32| {
            egui::Rect::from_center_size(
                egui::pos2(x + side / 2.0, r.rect.center().y),
                egui::vec2(side, side),
            )
        };
        match item.have {
            Have::Here => {}
            Have::Download => {
                crate::icons::paint(ui.painter(), mark(x), crate::icons::Icon::Download, p.secondary);
                x -= side + 4.0;
            }
            Have::Getting => {
                ui.put(mark(x), egui::Spinner::new().size(side));
                x -= side + 4.0;
            }
        }
        if item.embroidery {
            crate::icons::paint(ui.painter(), mark(x), crate::icons::Icon::Stitch, p.secondary);
        }
        let r = match (here, item.embroidery) {
            (true, false) => r,
            (true, true) => r.on_hover_text("Made for embroidery"),
            (false, emb) => r.on_hover_text(format!(
                "{}Downloads from zigzag.diy when picked",
                if emb { "Made for embroidery. " } else { "" }
            )),
        };
        changed |= r.changed();
        if reveal && item.name == *font {
            // (At once: the menu opens on it, not scrolling to it.)
            r.scroll_to_me_animation(
                Some(egui::Align::Center),
                egui::style::ScrollAnimation::none(),
            );
        }
    }
    if enter {
        ui.close();
    }
    changed
}

/// A font was picked for `spec`: a single-line font made for embroidery
/// is sewn as a run (its letters are lines, with nothing to fill).
pub(crate) fn picked_font(spec: &mut TextSpec, fonts: &[FontItem]) {
    if fonts
        .iter()
        .any(|f| f.embroidery && f.name.eq_ignore_ascii_case(&spec.font))
        && matches!(spec.kind, Kind::Satin | Kind::Fill)
    {
        spec.kind = Kind::Run;
    }
}

/// Where the caret and the typing frame are drawn: on the canvas, over the
/// letters, and only there (not over the docks beside it when the letters
/// are zoomed in past its edges, nor over a panel or menu on top of it).
fn caret_painter(ctx: &egui::Context, canvas: egui::Rect) -> egui::Painter {
    ctx.layer_painter(egui::LayerId::background())
        .with_clip_rect(canvas)
}

impl Studio {
    /// The font families on this computer (its own, the bundled ones, and
    /// those downloaded), read again whenever fonts were added.
    pub(crate) fn fonts_here(&mut self) -> Vec<String> {
        let faces = zigzag::svg::font_faces();
        if self.lettering.fonts.is_empty() || self.lettering.fonts_faces != faces {
            self.lettering.fonts = font_families();
            self.lettering.fonts_faces = faces;
        }
        self.lettering.fonts.clone()
    }

    /// The font menu: the fonts here, and the catalogue's others to
    /// download.
    pub(crate) fn fonts(&mut self) -> Vec<FontItem> {
        let here = self.fonts_here();
        self.font_store.want_catalogue();
        self.font_store.menu(&here)
    }

    pub fn open_text_window(&mut self) {
        if self.lettering.new.is_none() {
            self.lettering.new = Some(self.new_text_spec());
        }
    }

    /// What new text starts as: as the last text was set (its font kept
    /// from one session to the next, if it's still installed), else as it
    /// comes. Empty, and not on a path.
    pub fn new_text_spec(&mut self) -> TextSpec {
        let mut spec = match self.lettering.last.clone() {
            Some(last) => last,
            None => {
                let mut spec = TextSpec::default();
                if let Some(font) = self.prefs.last_font.clone()
                    && self.fonts_here().contains(&font)
                {
                    spec.font = font;
                }
                spec
            }
        };
        spec.text = String::new();
        spec.path = None;
        spec
    }

    /// Keep a text's settings for the next text (see [`Self::new_text_spec`]).
    pub fn remember_text(&mut self, spec: &TextSpec) {
        self.lettering.last = Some(TextSpec {
            text: String::new(),
            path: None,
            ..spec.clone()
        });
        if self.prefs.last_font.as_deref() != Some(spec.font.as_str()) {
            self.prefs.last_font = Some(spec.font.clone());
            self.prefs.save();
        }
    }

    pub fn add_lettering(
        &mut self,
        spec: &TextSpec,
        center_mm: (f64, f64),
    ) -> anyhow::Result<usize> {
        let mut idx = 0;
        self.structural(|d| {
            idx = d.add_lettering(spec, center_mm)?;
            Ok(())
        })?;
        self.remember_text(spec);
        self.lettering_font_used(&spec.font);
        self.set_selection(&[idx]);
        // (Planned on the next frame: at once for a small design, on the
        // worker for a big one, so the UI never stalls here.)
        self.dirty = true;
        Ok(idx)
    }

    pub fn update_lettering(&mut self, index: usize, spec: &TextSpec) -> anyhow::Result<()> {
        let uid = self.doc.shapes[index].uid;
        self.structural(|d| d.set_lettering(index, spec))?;
        self.remember_text(spec);
        self.lettering_font_used(&spec.font);
        if let Some(i) = self.doc.shapes.iter().position(|s| s.uid == uid) {
            self.set_selection(&[i]);
        }
        // Planned on the next frame, and on the worker: new letters are new
        // outlines to find satin columns in, which is quick for most fonts
        // and seconds for some (there's no telling which from how long the
        // last font took), so the window never waits on it.
        self.planner.expect_slow();
        self.dirty = true;
        Ok(())
    }

    /// The Character and Paragraph settings (after VectorCraft's panels,
    /// the parts that matter for stitching), the baseline, and the stitch
    /// row. Shared by the Add Text window and the Lettering panel. Returns
    /// true when a value was committed (not mid-drag).
    fn text_fields(
        ui: &mut egui::Ui,
        spec: &mut TextSpec,
        fonts: &[FontItem],
        filter: &mut String,
        id: &str,
    ) -> bool {
        use crate::theme::{form, icon_button, segmented};
        use zigzag::document::{Align, Case};
        let mut committed = false;
        let done = |r: &egui::Response| {
            r.drag_stopped() || r.lost_focus() || (r.changed() && !r.dragged() && !r.has_focus())
        };
        let num = |ui: &mut egui::Ui,
                   v: &mut f64,
                   range: std::ops::RangeInclusive<f64>,
                   step: f64,
                   label: &str| {
            ui.spacing_mut().slider_width = 96.0;
            let r = ui.add(
                egui::Slider::new(v, range.clone())
                    .show_value(false)
                    .step_by(step)
                    // (A value past the range, as text scaled by its
                    // handles has, stays until this is what's changed.)
                    .clamping(egui::SliderClamping::Edits),
            );
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, label));
            let d = ui.add(crate::units::drag_mm(v, range, step).clamp_existing_to_range(false));
            done(&r) || done(&d)
        };
        let pct = |ui: &mut egui::Ui, v: &mut f64, label: &str| {
            let d = ui.add(
                egui::DragValue::new(v)
                    .range(20.0..=400.0)
                    .speed(1.0)
                    .fixed_decimals(0)
                    .suffix(" %"),
            );
            d.widget_info(|| egui::WidgetInfo::labeled(egui::Role::SpinButton, true, label));
            done(&d)
        };
        form(ui, &format!("{id}-form"), |fm| {
            fm.row("Text", |ui, lid| {
                let r = ui
                    .add(
                        egui::TextEdit::multiline(&mut spec.text)
                            .hint_text("Your text")
                            .desired_rows(1)
                            .desired_width(f32::INFINITY),
                    )
                    .labelled_by(lid);
                committed |= done(&r);
            });
            fm.row("Font", |ui, lid| {
                let r = egui::ComboBox::from_id_salt(format!("{id}-font"))
                    .selected_text(&spec.font)
                    .width(170.0)
                    .height(320.0)
                    .show_ui(ui, |ui| font_list(ui, &mut spec.font, fonts, filter));
                if r.inner == Some(true) {
                    picked_font(spec, fonts);
                    committed = true;
                }
                let r = r.response.labelled_by(lid);
                committed |= r.changed();
            });
            fm.row("Style", |ui, _| {
                committed |= ui.toggle_value(&mut spec.bold, "Bold").changed();
                committed |= ui.toggle_value(&mut spec.italic, "Italic").changed();
                committed |= ui
                    .color_edit_button_srgb(&mut spec.color)
                    .on_hover_text("Thread color")
                    .changed();
            });
            fm.row("Height", |ui, _| {
                committed |= num(ui, &mut spec.size_mm, 3.0..=40.0, 0.5, "Letter height")
            });
            fm.row("Tracking", |ui, _| {
                committed |= num(ui, &mut spec.spacing_mm, -1.0..=5.0, 0.1, "Letter spacing")
            });
            // Pair kerning: pick the pair, set its gap.
            let chars: Vec<char> = spec.case.apply(&spec.text).chars().collect();
            if chars.len() >= 2 {
                fm.row("Kern", |ui, _| {
                    let pair_id = ui.make_persistent_id((id, "kern-pair"));
                    let mut at: usize = ui
                        .data(|d| d.get_temp(pair_id))
                        .unwrap_or(1)
                        .clamp(1, chars.len() - 1);
                    let label = |k: usize| {
                        let show = |c: char| if c == '\n' { '↵' } else { c };
                        format!("{}{}", show(chars[k - 1]), show(chars[k]))
                    };
                    let kerned: Vec<usize> = spec.kern.iter().map(|(k, _)| *k).collect();
                    egui::ComboBox::from_id_salt((id, "kern-combo"))
                        .selected_text(label(at))
                        .width(64.0)
                        .show_ui(ui, |ui| {
                            for k in 1..chars.len() {
                                let text = if kerned.contains(&k) {
                                    egui::RichText::new(label(k)).strong()
                                } else {
                                    egui::RichText::new(label(k))
                                };
                                ui.selectable_value(&mut at, k, text);
                            }
                        })
                        .response
                        .on_hover_text(
                            "The pair of letters to adjust (pairs already kerned are bold)",
                        );
                    ui.data_mut(|d| d.insert_temp(pair_id, at));
                    let mut v = spec.kern_at(at);
                    let d = ui.add(crate::units::drag_mm(&mut v, -3.0..=3.0, 0.05));
                    if d.changed() {
                        spec.set_kern(at, v);
                    }
                    committed |= done(&d);
                });
            }
            if spec.text.contains('\n') {
                fm.row("Leading", |ui, lid| {
                    let d = ui
                        .add(
                            egui::DragValue::new(&mut spec.leading)
                                .range(0.5..=5.0)
                                .speed(0.02)
                                .fixed_decimals(2)
                                .suffix(" ×"),
                        )
                        .on_hover_text("Line spacing, as a multiple of the letter height")
                        .labelled_by(lid);
                    committed |= done(&d);
                });
            }
            fm.row("Scale", |ui, _| {
                ui.label(egui::RichText::new("W").size(11.0));
                committed |= pct(ui, &mut spec.h_scale, "Horizontal scale");
                ui.label(egui::RichText::new("H").size(11.0));
                committed |= pct(ui, &mut spec.v_scale, "Vertical scale");
            });
            fm.row("Case", |ui, _| {
                if segmented(
                    ui,
                    &format!("{id}-case"),
                    &mut spec.case,
                    &[
                        (Case::AsTyped, "Aa"),
                        (Case::Upper, "AA"),
                        (Case::Lower, "aa"),
                        (Case::Title, "Ab"),
                    ],
                ) {
                    committed = true;
                }
            });
            fm.row("Align", |ui, _| {
                for (a, icon, name) in [
                    (Align::Left, crate::icons::Icon::AlignLeft, "Align Left"),
                    (
                        Align::Center,
                        crate::icons::Icon::AlignCenter,
                        "Align Center",
                    ),
                    (Align::Right, crate::icons::Icon::AlignRight, "Align Right"),
                ] {
                    if icon_button(ui, icon, name, spec.align == a, crate::theme::PANEL_ICON)
                        .clicked()
                        && spec.align != a
                    {
                        spec.align = a;
                        committed = true;
                    }
                }
            });
            fm.row("Baseline", |ui, _| {
                if spec.path.is_some() {
                    ui.label("On a path");
                    if ui
                        .add(egui::Button::new("Straighten").small())
                        .on_hover_text("Take the text off its path")
                        .clicked()
                    {
                        spec.path = None;
                        committed = true;
                    }
                    return;
                }
                let mut mode = if spec.arc_mm.abs() < 0.5 {
                    0
                } else if spec.arc_mm > 0.0 {
                    1
                } else {
                    -1
                };
                if segmented(
                    ui,
                    &format!("{id}-baseline"),
                    &mut mode,
                    &[(0, "Straight"), (1, "Arc Over"), (-1, "Arc Under")],
                ) {
                    let r = if spec.arc_mm.abs() < 0.5 {
                        25.0
                    } else {
                        spec.arc_mm.abs()
                    };
                    spec.arc_mm = r * mode as f64;
                    committed = true;
                }
            });
            if spec.path.is_some() {
                fm.row("Offset", |ui, lid| {
                    let mut v = spec.path_offset * 100.0;
                    let d = ui
                        .add(
                            egui::DragValue::new(&mut v)
                                .range(0.0..=100.0)
                                .speed(1.0)
                                .fixed_decimals(0)
                                .suffix(" %"),
                        )
                        .on_hover_text("Where along the path the text starts")
                        .labelled_by(lid);
                    spec.path_offset = v / 100.0;
                    committed |= done(&d);
                    let mut flip = spec.path_flip;
                    if ui
                        .checkbox(&mut flip, "Flip")
                        .on_hover_text("Letters on the other side of the path")
                        .changed()
                    {
                        spec.path_flip = flip;
                        committed = true;
                    }
                });
            } else if spec.arc_mm.abs() >= 0.5 {
                fm.row("Radius", |ui, _| {
                    let sign = spec.arc_mm.signum();
                    let mut r = spec.arc_mm.abs();
                    committed |= num(ui, &mut r, 8.0..=80.0, 0.5, "Arc radius");
                    spec.arc_mm = r * sign;
                });
            }
            fm.row("Stitch", |ui, _| {
                if segmented(
                    ui,
                    &format!("{id}-kind"),
                    &mut spec.kind,
                    &[
                        (Kind::Satin, "Satin"),
                        (Kind::Fill, "Fill"),
                        (Kind::Run, "Run"),
                    ],
                ) {
                    committed = true;
                }
            });
        });
        // Slider steps leave float dust (0.4000000001); keep values clean.
        let r2 = |v: f64| (v * 100.0).round() / 100.0;
        spec.size_mm = r2(spec.size_mm);
        spec.spacing_mm = r2(spec.spacing_mm);
        spec.arc_mm = r2(spec.arc_mm);
        spec.leading = r2(spec.leading);
        spec.h_scale = spec.h_scale.round();
        spec.v_scale = spec.v_scale.round();
        committed
    }

    /// Type › Create Outlines: the selected lettering becomes plain paths.
    /// Type › Convert to Text: the reverse of Create Outlines, for outlines
    /// that record their text (made here, or in Illustrator from text made
    /// here).
    pub fn convert_to_text(&mut self) {
        let uids: Vec<usize> = self
            .sel_indices()
            .into_iter()
            .filter(|&i| self.doc.records_text(i))
            .map(|i| self.doc.shapes[i].uid)
            .collect();
        if uids.is_empty() {
            return;
        }
        let mut new = Vec::new();
        let r = self.structural(|d| {
            for uid in &uids {
                // (Letters of one text all go at once: later ones may be gone.)
                let Some(i) = d.shapes.iter().position(|s| s.uid == *uid) else {
                    continue;
                };
                let j = d.restore_lettering(i)?;
                new.push(d.shapes[j].uid);
            }
            Ok(())
        });
        match r {
            Ok(()) => {
                let sel: Vec<usize> = self
                    .doc
                    .shapes
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| new.contains(&s.uid))
                    .map(|(i, _)| i)
                    .collect();
                self.set_selection(&sel);
                self.status = format!(
                    "Back to text: {} object(s) can be edited as text again (changes made to the outlines' points don't carry over).",
                    sel.len()
                );
            }
            Err(e) => self.status = format!("Couldn't convert to text: {e:#}"),
        }
        self.dirty = true;
    }

    pub fn create_outlines(&mut self) {
        let idx: Vec<usize> = self
            .sel_indices()
            .into_iter()
            .filter(|&i| self.doc.lettering(i).is_some())
            .collect();
        if idx.is_empty() {
            return;
        }
        let uids: Vec<usize> = idx.iter().map(|&i| self.doc.shapes[i].uid).collect();
        let mut new = Vec::new();
        let r = self.structural(|d| {
            for uid in &uids {
                let Some(i) = d.shapes.iter().position(|s| s.uid == *uid) else {
                    continue;
                };
                let j = d.expand_lettering(i)?;
                new.push(d.shapes[j].uid);
            }
            Ok(())
        });
        match r {
            Ok(()) => {
                let sel: Vec<usize> = self
                    .doc
                    .shapes
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| new.contains(&s.uid))
                    .map(|(i, _)| i)
                    .collect();
                self.set_selection(&sel);
                self.status = format!(
                    "Outlines made: {} object(s) can be edited point by point now, not as text.",
                    sel.len()
                );
            }
            Err(e) => self.status = format!("Couldn't create outlines: {e:#}"),
        }
        // (Planned on the next frame: at once for a small design, on the
        // worker for a big one, so the UI never stalls here.)
        self.dirty = true;
    }

    /// Change one setting on every selected lettering object (one undo step).
    pub fn edit_selected_text(&mut self, f: impl Fn(&mut TextSpec)) {
        let idx: Vec<usize> = self
            .sel_indices()
            .into_iter()
            .filter(|&i| self.doc.lettering(i).is_some())
            .collect();
        if idx.is_empty() {
            return;
        }
        let uids: Vec<usize> = idx.iter().map(|&i| self.doc.shapes[i].uid).collect();
        let mut edited: Option<TextSpec> = None;
        let r = self.structural(|d| {
            for uid in &uids {
                let Some(i) = d.shapes.iter().position(|s| s.uid == *uid) else {
                    continue;
                };
                let Some(mut spec) = d.lettering(i) else {
                    continue;
                };
                f(&mut spec);
                d.set_lettering(i, &spec)?;
                edited = Some(spec);
            }
            Ok(())
        });
        if let Some(spec) = edited {
            self.remember_text(&spec);
        }
        if let Err(e) = r {
            self.status = format!("Couldn't change the text: {e:#}");
        }
        // (Planned on the next frame: at once for a small design, on the
        // worker for a big one, so the UI never stalls here.)
        self.dirty = true;
    }

    /// Type › Type on a Path: with one lettering object and one path
    /// selected, the text follows the path (which is then not sewn).
    pub fn type_on_selected_path(&mut self) {
        let sel = self.sel_indices();
        let Some(&t) = sel.iter().find(|&&i| self.doc.lettering(i).is_some()) else {
            self.status = "Select a text object and a path.".into();
            return;
        };
        let Some(&p) = sel.iter().find(|&&i| self.doc.lettering(i).is_none()) else {
            self.status = "Select the path the text should follow too.".into();
            return;
        };
        let Ok(path) = self.doc.edit_path(p) else {
            self.status = format!("{} can't be a baseline.", self.names[p]);
            return;
        };
        let c = zigzag::bezier::bounds(&path).center();
        let rel = zigzag::bezier::transform(&path, zigzag::geom::translation(P::default() - c));
        let Some(mut spec) = self.doc.lettering(t) else {
            return;
        };
        spec.path = Some(rel);
        spec.arc_mm = 0.0;
        spec.align = zigzag::document::Align::Left;
        let (t_uid, p_uid) = (self.doc.shapes[t].uid, self.doc.shapes[p].uid);
        // The text is re-made at the path's center (its baseline is relative
        // to that), in the path's place in the order; the old one goes.
        let r = self.structural(|d| {
            let Some(ti) = d.shapes.iter().position(|s| s.uid == t_uid) else {
                anyhow::bail!("text vanished")
            };
            d.delete(ti)?;
            let ni = d.add_lettering(&spec, (c.x / 10.0, c.y / 10.0))?;
            let _ = ni;
            if let Some(pi) = d.shapes.iter().position(|s| s.uid == p_uid) {
                d.shapes[pi].params.kind = Some(Kind::Skip);
            }
            Ok(())
        });
        match r {
            Ok(()) => {
                self.status =
                    "The text follows the path now. Offset and Flip are in the Lettering panel."
                        .into();
                let sel: Vec<usize> = self
                    .doc
                    .shapes
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| self.doc.lettering(*i).is_some())
                    .map(|(i, _)| i)
                    .collect();
                self.set_selection(&sel);
            }
            Err(e) => self.status = format!("Couldn't put the text on the path: {e:#}"),
        }
        // (Planned on the next frame: at once for a small design, on the
        // worker for a big one, so the UI never stalls here.)
        self.dirty = true;
    }

    /// The Type tool (T) on the canvas, as in VectorCraft and Illustrator:
    /// click where the text should go and type; click lettering to edit
    /// it; click a path (while it's selected) to put the text on it. Enter
    /// makes a new line, Escape or a click elsewhere finishes.
    pub fn type_canvas(&mut self, ui: &egui::Ui, resp: &egui::Response, rect: egui::Rect) {
        let ctx = ui.ctx().clone();
        ctx.set_cursor_icon(egui::CursorIcon::Text);
        let tol = self.snap_tol();
        let pressed = resp.clicked() || resp.drag_started_by(egui::PointerButton::Primary);
        // Dragging across the text selects from where the press was.
        if !pressed
            && resp.dragged_by(egui::PointerButton::Primary)
            && let Some(mut t) = self.lettering.typing.take()
        {
            if t.dragging
                && let Some(i) = self.doc.shapes.iter().position(|s| s.uid == t.uid)
                && let Some(pos) = resp.interact_pointer_pos()
            {
                let p = self.view.to_design(rect, pos);
                if let Some(k) = t.carets(&self.doc, i).nearest(p) {
                    t.move_to(k, true);
                }
            }
            self.lettering.typing = Some(t);
            return;
        }
        if (resp.drag_stopped() || ctx.input(|i| i.pointer.any_released()))
            && let Some(t) = &mut self.lettering.typing
        {
            t.dragging = false;
            if t.anchor == Some(t.cursor) {
                t.anchor = None;
            }
        }
        if pressed {
            let Some(pos) = ctx
                .input(|i| i.pointer.press_origin())
                .or(resp.interact_pointer_pos())
            else {
                return;
            };
            let p = self.view.to_design(rect, pos);
            let shift = ctx.input(|i| i.modifiers.shift);
            // Clicking the text being typed moves the caret there (Shift
            // extends the selection, double-click takes the word, triple the
            // lot); a click anywhere else just finishes it (a second click
            // starts new text, so ending an edit never leaves a stray box).
            if let Some(mut t) = self.lettering.typing.take() {
                if let Some(i) = self.doc.shapes.iter().position(|s| s.uid == t.uid)
                    && self.text_box_hit(i, p, tol)
                {
                    if let Some(k) = t.carets(&self.doc, i).nearest(p) {
                        if resp.triple_clicked() {
                            t.select_all();
                        } else if resp.double_clicked() {
                            t.select_word_at(k.min(t.len().saturating_sub(1)));
                        } else {
                            t.move_to(k, shift);
                            t.dragging = !resp.clicked();
                        }
                        // (Its letters are anyone's to edit now.)
                        t.placeholder = false;
                    }
                    self.lettering.typing = Some(t);
                    return;
                }
                self.lettering.typing = Some(t);
                self.finish_typing();
                return;
            }
            // Clicking on (or between the letters of) existing text edits it,
            // with the caret where the click was: one undo step for the
            // whole edit.
            if let Some(i) = self.text_at(p, tol) {
                self.edit_text_at(i, p);
                return;
            }
            let hit = self.pick(p, tol);
            // Text on a path: the clicked object was selected and is a path.
            let on_path = hit
                .filter(|&i| self.sel_indices().contains(&i) && self.doc.lettering(i).is_none())
                .and_then(|i| self.doc.edit_path(i).ok().map(|p| (i, p)));
            let mut spec = self.new_text_spec();
            let center = match &on_path {
                Some((_, path)) => {
                    // The path, relative to its own center: the text sits on it.
                    let b = zigzag::bezier::bounds(path);
                    let c = b.center();
                    let rel = zigzag::bezier::transform(
                        path,
                        zigzag::geom::translation(P::default() - c),
                    );
                    spec.path = Some(rel);
                    spec.align = zigzag::document::Align::Left;
                    (c.x / 10.0, c.y / 10.0)
                }
                None => (p.x / 10.0, p.y / 10.0),
            };
            // Placed with a placeholder so there's something to see; the
            // placeholder is replaced by what's typed.
            spec.text = "Text".into();
            match self.add_lettering(&spec, center) {
                Ok(i) => {
                    if let Some((src, _)) = on_path {
                        // The path object itself is no longer sewn: it's the
                        // baseline now.
                        let src_uid = self.doc.shapes[src].uid;
                        if let Some(j) = self.doc.shapes.iter().position(|s| s.uid == src_uid) {
                            self.doc.shapes[j].params.kind = Some(Kind::Skip);
                        }
                    }
                    let uid = self.doc.shapes[i].uid;
                    let mut t = Typing::new(uid, spec.text.clone(), 0, true);
                    t.select_all();
                    self.lettering.typing = Some(t);
                    self.status = "Type your text. Enter for a new line, Esc when done.".into();
                }
                Err(e) => self.status = format!("Couldn't add text: {e:#}"),
            }
        }
    }

    /// Start editing text object `i` with the Type tool, the caret nearest
    /// `p` (design units): the whole edit is one undo step. Also what a
    /// double-click on text with the Selection tool does, as in Illustrator.
    pub fn edit_text_at(&mut self, i: usize, p: P) {
        let Some(spec) = self.doc.lettering(i) else {
            return;
        };
        if let Some(t) = &self.lettering.typing
            && t.uid == self.doc.shapes[i].uid
        {
            return;
        }
        self.finish_typing();
        self.set_tool(crate::app::Tool::Text);
        self.set_selection(&[i]);
        self.push_undo(self.stable.clone());
        self.redo.clear();
        let uid = self.doc.shapes[i].uid;
        let n = spec.text.chars().count();
        let mut t = Typing::new(uid, spec.text, n, false);
        if let Some(k) = t.carets(&self.doc, i).nearest(p) {
            t.cursor = k.min(n);
        }
        self.lettering.typing = Some(t);
    }

    /// The topmost text whose box (gaps between the letters included)
    /// `p` is in: text is picked as a whole, not letter by letter.
    pub fn text_at(&self, p: P, tol: f64) -> Option<usize> {
        (0..self.doc.shapes.len()).rev().find(|&i| {
            self.free(i) && self.doc.lettering(i).is_some() && self.text_box_hit(i, p, tol)
        })
    }

    /// The text object being typed into, while every letter is deleted:
    /// its stitches and outline are hidden (the frame and caret show
    /// instead), the text staying in the document until something's typed
    /// or the edit ends.
    pub fn hidden_text(&self) -> Option<usize> {
        let t = self.lettering.typing.as_ref()?;
        if !t.text.is_empty() || t.placeholder {
            return None;
        }
        self.doc.shapes.iter().position(|s| s.uid == t.uid)
    }

    /// A plan just made still has the hidden text's stitches (its letters
    /// stay in the document): out of the plan on show they go, wherever
    /// they are in it (in a block of their own, or sewn in one thread
    /// with their neighbours).
    pub(crate) fn hide_planned_text(&mut self) {
        if let Some(i) = self.hidden_text()
            && self.doc.shapes.len() == self.design.per_shape.len()
        {
            self.drop_planned(&[i]);
            // (Planned again before it's sewn, or shown once there are
            // letters: see `plan_now`, and typing re-plans.)
        }
    }

    /// Is `p` on text object `i`: within its box (with a little margin),
    /// gaps between the letters included.
    fn text_box_hit(&self, i: usize, p: P, tol: f64) -> bool {
        let b = crate::app::shape_bounds(&self.doc.shapes[i]);
        if b.is_empty() {
            return false;
        }
        let m = tol.max(10.0);
        p.x >= b.min.x - m && p.x <= b.max.x + m && p.y >= b.min.y - m && p.y <= b.max.y + m
    }

    /// Typing with the Type tool goes straight into the lettering on the
    /// canvas, as in Illustrator: each key edits the text at the caret
    /// (Backspace and Delete remove, Enter starts a line, the arrow keys
    /// and a click move the caret, Escape ends), and the caret blinks
    /// between the letters.
    pub fn typing_box(&mut self, ctx: &egui::Context, rect: egui::Rect) {
        let Some(mut t) = self.lettering.typing.take() else {
            return;
        };
        let Some(i) = self.doc.shapes.iter().position(|s| s.uid == t.uid) else {
            return;
        };
        if self.tool != crate::app::Tool::Text {
            self.lettering.typing = Some(t);
            self.finish_typing();
            return;
        }
        // Take the keys: text, Backspace, Enter, Escape (and paste).
        let mut done = false;
        let mut changed = false;
        let mut kern_step: Option<f64> = None;
        // Up/down: +1 / -1 lines, moved by geometry after the keys.
        let mut line_step: i32 = 0;
        let mut shift_lines = false;
        let mut copied: Option<String> = None;
        ctx.input_mut(|input| {
            input.events.retain(|ev| {
                match ev {
                    egui::Event::Text(s) => {
                        t.insert(s);
                        changed = true;
                        false
                    }
                    egui::Event::Paste(s) => {
                        t.insert(s);
                        changed = true;
                        false
                    }
                    egui::Event::Copy => {
                        if t.selection().is_some() {
                            copied = Some(t.selected_text());
                        }
                        false
                    }
                    egui::Event::Cut => {
                        if t.selection().is_some() {
                            copied = Some(t.selected_text());
                            t.delete_selection();
                            changed = true;
                        }
                        false
                    }
                    egui::Event::Key {
                        key: egui::Key::A,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.command => {
                        t.select_all();
                        false
                    }
                    egui::Event::Key {
                        key: egui::Key::Backspace,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        t.backspace(modifiers.alt || modifiers.command);
                        changed = true;
                        false
                    }
                    egui::Event::Key {
                        key: egui::Key::Delete,
                        pressed: true,
                        ..
                    } => {
                        t.delete_forward();
                        changed = true;
                        false
                    }
                    egui::Event::Key {
                        key: egui::Key::Enter,
                        pressed: true,
                        ..
                    } => {
                        t.insert("\n");
                        changed = true;
                        false
                    }
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        ..
                    } => {
                        done = true;
                        false
                    }
                    // Option+←/→: kern the pair at the caret (as Illustrator
                    // does), by 0.1 mm.
                    egui::Event::Key {
                        key: key @ (egui::Key::ArrowLeft | egui::Key::ArrowRight),
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.alt => {
                        kern_step = Some(if *key == egui::Key::ArrowLeft {
                            -0.1
                        } else {
                            0.1
                        });
                        false
                    }
                    // ←/→ move the caret; ⌘←/→ (or Home/End) to the ends of
                    // the line; Shift extends the selection. Without Shift,
                    // ←/→ with a selection collapse it to that end.
                    egui::Event::Key {
                        key: egui::Key::ArrowLeft,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        let k = if modifiers.command {
                            t.line_edge(false)
                        } else if let Some(r) = t.selection().filter(|_| !modifiers.shift) {
                            r.start
                        } else {
                            t.cursor.saturating_sub(1)
                        };
                        t.move_to(k, modifiers.shift);
                        false
                    }
                    egui::Event::Key {
                        key: egui::Key::ArrowRight,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        let k = if modifiers.command {
                            t.line_edge(true)
                        } else if let Some(r) = t.selection().filter(|_| !modifiers.shift) {
                            r.end
                        } else {
                            t.cursor + 1
                        };
                        t.move_to(k, modifiers.shift);
                        false
                    }
                    egui::Event::Key {
                        key: egui::Key::Home,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        t.move_to(t.line_edge(false), modifiers.shift);
                        false
                    }
                    egui::Event::Key {
                        key: egui::Key::End,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        t.move_to(t.line_edge(true), modifiers.shift);
                        false
                    }
                    egui::Event::Key {
                        key: egui::Key::ArrowUp,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        line_step -= 1;
                        shift_lines = modifiers.shift;
                        false
                    }
                    egui::Event::Key {
                        key: egui::Key::ArrowDown,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        line_step += 1;
                        shift_lines = modifiers.shift;
                        false
                    }
                    egui::Event::Key { .. } => false,
                    _ => true,
                }
            });
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        if let Some(step) = kern_step
            && let Some(mut spec) = self.doc.lettering(i)
        {
            // At the end of the text (while typing along), the last pair.
            let at = t.cursor.min(t.len().saturating_sub(1));
            if at > 0 {
                let v = spec.kern_at(at) + step;
                spec.set_kern(at, v);
                let _ = self.doc.set_lettering(i, &spec);
                self.dirty = true;
                self.modified = true;
                let (a, b) = {
                    let cs: Vec<char> = t.text.chars().collect();
                    (cs[at - 1], cs[at])
                };
                self.status = format!("Kerning {a}{b}: {}", crate::units::fmt_mm(spec.kern_at(at)));
            }
        }
        if line_step != 0 && !t.text.is_empty() {
            // The caret one line up or down: the position nearest a point a
            // line's height above or below the caret.
            let cursor = t.cursor;
            let carets = t.carets(&self.doc, i);
            if let Some((top, bottom)) = carets.at.get(cursor).copied() {
                let h = (bottom - top) * 1.2 * line_step as f64;
                let target = (top + bottom) * 0.5 + h;
                if let Some(k) = carets.nearest(target) {
                    t.move_to(k, shift_lines);
                }
            }
        }
        if let Some(s) = copied {
            ctx.copy_text(s);
            self.status = "Copied.".into();
        }
        if changed && let Some(spec) = self.doc.lettering(i) {
            t.placeholder = false;
            // Everything deleted: the letters stay in the document (a text
            // element with no letters would have no outline, and vanish)
            // but are hidden; just the frame and caret show.
            if !t.text.is_empty() {
                let new = TextSpec {
                    text: t.text.clone(),
                    ..spec
                };
                // Live: no undo step per keystroke (one at the end).
                let _ = self.doc.set_lettering(i, &new);
                self.dirty = true;
                self.modified = true;
            } else {
                self.drop_planned(&[i]);
            }
        }
        // The caret: after the last glyph of the last line, as tall as the
        // letters, blinking.
        let b = crate::app::shape_bounds(&self.doc.shapes[i]);
        if !b.is_empty() && (!t.text.is_empty() || t.placeholder) {
            t.empty_frame = Some((b.min.x, b.min.y, b.max.y));
        }
        let blink = (ctx.input(|i| i.time) * 2.0) as i64 % 2 == 0;
        if t.text.is_empty() && !t.placeholder {
            // Just the caret and a thin frame, where the letters were.
            if let Some((x0, y0, y1)) = t.empty_frame {
                let p = crate::theme::pal(ctx);
                let painter = caret_painter(ctx, rect);
                let (top, bottom) = (
                    self.view.to_screen(rect, P::new(x0, y0)),
                    self.view.to_screen(rect, P::new(x0, y1)),
                );
                if blink {
                    painter.line_segment([top, bottom], egui::Stroke::new(2.0, p.accent));
                }
                painter.rect_stroke(
                    egui::Rect::from_two_pos(top, bottom).expand2(egui::Vec2::new(6.0, 0.0)),
                    0.0,
                    egui::Stroke::new(1.0, p.accent.gamma_multiply(0.6)),
                    egui::StrokeKind::Outside,
                );
            }
        } else if !b.is_empty() {
            let p = crate::theme::pal(ctx);
            let painter = caret_painter(ctx, rect);
            // The caret between the letters where the cursor is (following
            // the baseline on arcs); after the last letter while the
            // placeholder shows.
            let cursor = t.cursor;
            let selection = t.selection();
            let at = {
                let layout = t.layout(&self.doc, i);
                // The selected letters, highlighted glyph by glyph (so the
                // highlight follows arcs and paths).
                if let Some(r) = &selection {
                    let fill = p.accent.gamma_multiply(0.25);
                    for g in layout
                        .boxes
                        .iter()
                        .filter(|g| g.chars.start < r.end && g.chars.end > r.start)
                    {
                        let quad = vec![
                            self.view.to_screen(rect, g.left.0),
                            self.view.to_screen(rect, g.right.0),
                            self.view.to_screen(rect, g.right.1),
                            self.view.to_screen(rect, g.left.1),
                        ];
                        painter.add(egui::Shape::convex_polygon(quad, fill, egui::Stroke::NONE));
                    }
                }
                layout.carets.at.get(cursor).copied()
            };
            let (top, bottom) = match at {
                Some((a, c)) => (self.view.to_screen(rect, a), self.view.to_screen(rect, c)),
                None => {
                    let scale = self.view.scale as f64 / 10.0;
                    let x = (2.0 * scale) as f32 + 2.0;
                    (
                        self.view.to_screen(rect, P::new(b.max.x, b.min.y)) + egui::vec2(x, 0.0),
                        self.view.to_screen(rect, P::new(b.max.x, b.max.y)) + egui::vec2(x, 0.0),
                    )
                }
            };
            if blink || selection.is_some() {
                painter.line_segment([top, bottom], egui::Stroke::new(2.0, p.accent));
            }
            let (top, bottom) = (
                self.view.to_screen(rect, P::new(b.max.x, b.min.y)),
                self.view.to_screen(rect, P::new(b.max.x, b.max.y)),
            );
            // A thin box round the text being edited.
            painter.rect_stroke(
                egui::Rect::from_two_pos(top, bottom).union(egui::Rect::from_two_pos(
                    self.view.to_screen(rect, b.min),
                    self.view.to_screen(rect, b.max),
                )),
                0.0,
                egui::Stroke::new(1.0, p.accent.gamma_multiply(0.6)),
                egui::StrokeKind::Outside,
            );
        }
        self.lettering.typing = Some(t);
        if done {
            self.finish_typing();
        }
    }

    /// End typing: an empty text is removed, otherwise the whole edit is
    /// one undo step.
    pub fn finish_typing(&mut self) {
        let Some(t) = self.lettering.typing.take() else {
            return;
        };
        let Some(i) = self.doc.shapes.iter().position(|s| s.uid == t.uid) else {
            return;
        };
        if t.text.trim().is_empty() || (t.placeholder && t.fresh) {
            // Gone from the canvas now, not when the next plan is made
            // (till then the old plan would show its letters again).
            self.drop_planned(&[i]);
            if t.fresh {
                // Back to before it was placed.
                if let Some(doc) = self.undo.pop() {
                    self.doc = doc;
                    self.stable = self.doc.clone();
                }
                self.status = "Nothing typed.".into();
            } else {
                // Every letter deleted: the text goes (the edit's undo step
                // brings it back).
                match self.doc.delete(i) {
                    Ok(()) => self.status = "Deleted the text.".into(),
                    Err(e) => self.status = format!("Couldn't delete the text: {e:#}"),
                }
                self.stable = self.doc.clone();
                self.modified = true;
            }
            self.set_selection(&[]);
            self.dirty = true;
            return;
        }
        if let Some(spec) = self.doc.lettering(i) {
            self.remember_text(&spec);
        }
        // Placing the text (or starting to edit it) recorded the undo step;
        // the typing since then joins it.
        self.stable = self.doc.clone();
        self.modified = true;
        // (Planned on the next frame: at once for a small design, on the
        // worker for a big one, so the UI never stalls here.)
        self.dirty = true;
        self.set_selection(&[i]);
        self.status = "Done. Press T and click to add more text; V to go back to selecting.".into();
    }

    /// The floating "Add text" window.
    pub fn text_window(&mut self, ctx: &egui::Context) {
        let Some(mut spec) = self.lettering.new.take() else {
            return;
        };
        let fonts = self.fonts();
        let mut filter = std::mem::take(&mut self.lettering.font_filter);
        let mut open = true;
        let mut add = false;
        let mut cancel = false;
        let mut picked = false;
        egui::Window::new("Add Text")
            .title_frame(crate::theme::title_frame(ctx))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_TOP, egui::Vec2::new(0.0, 90.0))
            .default_width(360.0)
            .show(ctx, |ui| {
                picked |= Self::text_fields(ui, &mut spec, &fonts, &mut filter, "new-text");
                ui.add_space(crate::theme::space::XS);
                ui.add_space(crate::theme::space::S);
                // A button row (right-aligned), only as tall as its buttons.
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 24.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let ok = !spec.text.trim().is_empty();
                        let add_btn = egui::Button::new(
                            egui::RichText::new("Add to Design").color(egui::Color32::WHITE),
                        )
                        .fill(crate::theme::pal_ui(ui).accent);
                        add = ui.add_enabled(ok, add_btn).clicked()
                            || (ok && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                        cancel = ui.button("Cancel").clicked()
                            || ui.input(|i| i.key_pressed(egui::Key::Escape));
                    },
                );
            });
        if cancel {
            open = false;
        }
        self.lettering.font_filter = filter;
        // A font from the catalogue starts downloading as it's picked, not
        // once the text is added.
        if picked {
            self.fetch_font_if_missing(&spec.font);
        }
        if add {
            match self.add_lettering(&spec, (0.0, 0.0)) {
                Ok(_) => {
                    self.status = format!("Added \"{}\".", spec.text);
                    return;
                }
                Err(e) => self.status = format!("Couldn't add text: {e:#}"),
            }
        }
        if open {
            self.lettering.new = Some(spec);
        }
    }

    /// Lettering settings for a selected lettering object.
    pub fn lettering_panel(&mut self, ui: &mut egui::Ui, index: usize) {
        let uid = self.doc.shapes[index].uid;
        let current = match self.doc.lettering(index) {
            Some(s) => s,
            None => return,
        };
        let mut spec = match self.lettering.edit.take() {
            Some((u, s)) if u == uid => s,
            _ => current.clone(),
        };
        let fonts = self.fonts();
        let mut filter = std::mem::take(&mut self.lettering.font_filter);
        let commit = Self::text_fields(ui, &mut spec, &fonts, &mut filter, "edit-text");
        self.lettering.font_filter = filter;
        if commit && spec != current && !spec.text.trim().is_empty() {
            match self.update_lettering(index, &spec) {
                Ok(()) => self.status = "Lettering updated.".into(),
                Err(e) => self.status = format!("Couldn't update lettering: {e:#}"),
            }
        } else if spec != current {
            self.lettering.edit = Some((uid, spec));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Vec2;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    #[test]
    fn the_typing_frame_stays_on_the_canvas() {
        // Text placed at the canvas's right edge runs on past it, under the
        // dock: the caret and the frame round it are cut off at the edge,
        // not drawn over the dock.
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(2);
        h.key_press(eframe::egui::Key::T);
        h.run_steps(1);
        let canvas = h.state().canvas_rect;
        let at = eframe::egui::Pos2::new(canvas.right() - 40.0, canvas.center().y);
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(3);
        for ch in ["C", "A", "M", "P", "S"] {
            h.event(eframe::egui::Event::Text(ch.into()));
            h.run_steps(1);
        }
        h.run_steps(2);
        assert!(h.state().lettering.typing.is_some());
        let b = crate::app::shape_bounds(&h.state().doc.shapes[0]);
        let right = h.state().view.to_screen(canvas, P::new(b.max.x, b.min.y)).x;
        assert!(right > canvas.right() + 20.0, "the text runs past the edge");
        // Whatever reaches from the canvas over the dock beside it.
        let over: Vec<_> = h
            .output()
            .shapes
            .iter()
            .filter(|c| {
                let r = c.shape.visual_bounding_rect();
                r.min.x > canvas.left()
                    && r.min.x < canvas.right() - 1.0
                    && r.max.x > canvas.right() + 1.0
                    && r.min.y < canvas.bottom()
                    && r.max.y > canvas.top()
                    && c.clip_rect.right() > canvas.right() + 0.5
            })
            .map(|c| c.shape.visual_bounding_rect())
            .collect();
        assert!(over.is_empty(), "drawn over the dock: {over:?}");
    }

    #[test]
    fn the_type_tool_places_text_where_you_click_and_types_into_it() {
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(2);
        h.key_press(eframe::egui::Key::T);
        h.run_steps(1);
        assert_eq!(h.state().tool, crate::app::Tool::Text);
        let rect = h.state().canvas_rect;
        let at = h.state().view.to_screen(rect, P::new(-200.0, 100.0));
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), 1, "{}", h.state().status);
        assert!(h.state().lettering.typing.is_some());
        for ch in ["C", "A", "M", "P"] {
            h.event(eframe::egui::Event::Text(ch.into()));
            h.run_steps(1);
        }
        h.run_steps(2);
        assert_eq!(h.state().doc.lettering(0).unwrap().text, "CAMP");
        let b = crate::app::shape_bounds(&h.state().doc.shapes[0]);
        assert!(
            b.center().dist(P::new(-200.0, 100.0)) < 60.0,
            "near the click: {:?}",
            b.center()
        );
        // Escape ends it: one undo step, the text selected, the tool kept.
        h.key_press(eframe::egui::Key::Escape);
        h.run_steps(3);
        assert!(h.state().lettering.typing.is_none());
        assert_eq!(h.state().undo.len(), 1);
        assert_eq!(h.state().sel_indices(), vec![0]);
        // Clicking it again edits it.
        let c = h.state().view.to_screen(rect, b.center());
        h.hover_at(c);
        h.run_steps(1);
        h.drag_at(c);
        h.run_steps(1);
        h.drop_at(c);
        h.run_steps(3);
        assert!(h.state().lettering.typing.is_some());
        assert_eq!(h.state().lettering.typing.as_ref().unwrap().text, "CAMP");
        // Typing nothing into a new one removes it.
        h.key_press(eframe::egui::Key::Escape);
        h.run_steps(2);
        let far = h.state().view.to_screen(rect, P::new(300.0, 300.0));
        h.hover_at(far);
        h.run_steps(1);
        h.drag_at(far);
        h.run_steps(1);
        h.drop_at(far);
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), 2);
        h.key_press(eframe::egui::Key::Escape);
        h.run_steps(3);
        assert_eq!(h.state().doc.shapes.len(), 1, "empty text removed");
    }

    #[test]
    fn the_caret_moves_with_clicks_and_arrows_and_types_where_it_is() {
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(2);
        h.key_press(eframe::egui::Key::T);
        h.run_steps(1);
        let rect = h.state().canvas_rect;
        let click = |h: &mut Harness<Studio>, p: P| {
            let at = h.state().view.to_screen(rect, p);
            h.hover_at(at);
            h.run_steps(1);
            h.drag_at(at);
            h.run_steps(1);
            h.drop_at(at);
            h.run_steps(3);
        };
        click(&mut h, P::new(0.0, 0.0));
        for ch in ["C", "A", "M", "P"] {
            h.event(eframe::egui::Event::Text(ch.into()));
            h.run_steps(1);
        }
        h.run_steps(2);
        assert_eq!(h.state().lettering.typing.as_ref().unwrap().cursor, 4);
        // Click in the gap between A and M: the caret goes there, and the
        // click doesn't start a new text.
        let carets = h.state().doc.carets(0);
        assert_eq!(carets.at.len(), 5);
        let gap = (carets.at[2].0 + carets.at[2].1) * 0.5;
        click(&mut h, gap);
        assert_eq!(h.state().doc.shapes.len(), 1);
        let t = h.state().lettering.typing.as_ref().unwrap();
        assert_eq!(t.cursor, 2, "{}", h.state().status);
        h.event(eframe::egui::Event::Text("R".into()));
        h.run_steps(2);
        assert_eq!(h.state().doc.lettering(0).unwrap().text, "CARMP");
        // Left twice, Backspace removes the A; End then types at the end.
        h.key_press(eframe::egui::Key::ArrowLeft);
        h.key_press(eframe::egui::Key::ArrowLeft);
        h.run_steps(1);
        assert_eq!(h.state().lettering.typing.as_ref().unwrap().cursor, 1);
        h.key_press(eframe::egui::Key::Delete);
        h.run_steps(2);
        assert_eq!(h.state().doc.lettering(0).unwrap().text, "CRMP");
        h.key_press(eframe::egui::Key::End);
        h.event(eframe::egui::Event::Text("S".into()));
        h.run_steps(2);
        assert_eq!(h.state().doc.lettering(0).unwrap().text, "CRMPS");
        // Clicking away ends typing without placing another text.
        click(&mut h, P::new(400.0, 400.0));
        assert!(h.state().lettering.typing.is_none());
        assert_eq!(h.state().doc.shapes.len(), 1);
        // Clicking between the letters of existing text edits it (no new box).
        let carets = h.state().doc.carets(0);
        let gap = (carets.at[1].0 + carets.at[1].1) * 0.5;
        click(&mut h, gap);
        assert_eq!(h.state().doc.shapes.len(), 1);
        let t = h.state().lettering.typing.as_ref().unwrap();
        assert_eq!((t.text.as_str(), t.cursor), ("CRMPS", 1));
    }

    #[test]
    fn text_is_selected_with_shift_arrows_double_clicks_drags_and_typed_over() {
        // Short steps: the harness takes one pointer event per step, and a
        // double-click needs its presses within 0.3 s.
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .with_step_dt(0.05)
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(2);
        h.key_press(eframe::egui::Key::T);
        h.run_steps(1);
        let rect = h.state().canvas_rect;
        let click = |h: &mut Harness<Studio>, p: P| {
            let at = h.state().view.to_screen(rect, p);
            h.hover_at(at);
            h.run_steps(1);
            h.drag_at(at);
            h.run_steps(1);
            h.drop_at(at);
            h.run_steps(3);
        };
        let key = |h: &mut Harness<Studio>, k: eframe::egui::Key, m: eframe::egui::Modifiers| {
            h.event(eframe::egui::Event::Key {
                key: k,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: m,
            });
            h.run_steps(1);
        };
        use eframe::egui::{Key, Modifiers};
        click(&mut h, P::new(0.0, 0.0));
        for ch in "HAT CAMP".chars() {
            h.event(eframe::egui::Event::Text(ch.to_string()));
            h.run_steps(1);
        }
        h.run_steps(2);
        // Shift+← ×4 selects "CAMP"; typing replaces it.
        for _ in 0..4 {
            key(&mut h, Key::ArrowLeft, Modifiers::SHIFT);
        }
        let t = h.state().lettering.typing.as_ref().unwrap();
        assert_eq!(t.selection(), Some(4..8));
        assert_eq!(t.selected_text(), "CAMP");
        h.event(eframe::egui::Event::Text("SHOP".into()));
        h.run_steps(2);
        assert_eq!(h.state().doc.lettering(0).unwrap().text, "HAT SHOP");
        assert!(
            h.state()
                .lettering
                .typing
                .as_ref()
                .unwrap()
                .selection()
                .is_none()
        );
        // ⌘A then Backspace clears the lot (frame and caret stay).
        key(&mut h, Key::A, Modifiers::COMMAND);
        assert_eq!(
            h.state().lettering.typing.as_ref().unwrap().selection(),
            Some(0..8)
        );
        key(&mut h, Key::Backspace, Modifiers::NONE);
        h.run_steps(2);
        assert_eq!(h.state().lettering.typing.as_ref().unwrap().text, "");
        assert_eq!(h.state().doc.shapes.len(), 1);
        for ch in "HAT CAMP".chars() {
            h.event(eframe::egui::Event::Text(ch.to_string()));
            h.run_steps(1);
        }
        h.run_steps(2);
        // Double-click on the A of HAT selects the word.
        let boxes = h.state().doc.glyph_boxes(0);
        let a_mid = (boxes[1].left_mid() + boxes[1].right_mid()) * 0.5;
        let at = h.state().view.to_screen(rect, a_mid);
        h.hover_at(at);
        h.run_steps(1);
        for _ in 0..2 {
            for pressed in [true, false] {
                h.event(eframe::egui::Event::PointerButton {
                    pos: at,
                    button: eframe::egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                });
            }
            h.run_steps(1);
        }
        h.run_steps(2);
        let t = h.state().lettering.typing.as_ref().unwrap();
        assert_eq!(t.selected_text(), "HAT", "{:?}", t.selection());
        // Shift+End extends to the end; Delete removes it all.
        key(&mut h, Key::End, Modifiers::SHIFT);
        assert_eq!(
            h.state().lettering.typing.as_ref().unwrap().selected_text(),
            "HAT CAMP"
        );
        key(&mut h, Key::ArrowLeft, Modifiers::NONE);
        let t = h.state().lettering.typing.as_ref().unwrap();
        assert_eq!((t.cursor, t.selection()), (0, None));
        // Dragging from the H to the middle of CAMP selects across.
        let boxes = h.state().doc.glyph_boxes(0);
        let from = h.state().view.to_screen(rect, boxes[0].left_mid());
        let to = h.state().view.to_screen(rect, boxes[6].left_mid());
        h.hover_at(from);
        h.run_steps(1);
        h.drag_at(from);
        h.run_steps(1);
        h.hover_at(from + (to - from) * 0.5);
        h.run_steps(1);
        h.hover_at(to);
        h.run_steps(1);
        h.drop_at(to);
        h.run_steps(2);
        let t = h.state().lettering.typing.as_ref().unwrap();
        assert_eq!(t.selected_text(), "HAT CA", "{:?}", t.selection());
        assert!(!t.dragging);
        // Copy, then paste at the end doubles it.
        h.event(eframe::egui::Event::Copy);
        h.run_steps(1);
        key(&mut h, Key::End, Modifiers::NONE);
        h.event(eframe::egui::Event::Paste("HAT CA".into()));
        h.run_steps(2);
        assert_eq!(h.state().doc.lettering(0).unwrap().text, "HAT CAMPHAT CA");
    }

    #[test]
    fn type_menu_changes_case_alignment_puts_text_on_a_path_and_makes_outlines() {
        use crate::commands::Action;
        use zigzag::document::{Align, Case};
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(2);
        let spec = TextSpec {
            text: "camp fire".into(),
            ..TextSpec::default()
        };
        let t = h.state_mut().add_lettering(&spec, (0.0, 0.0)).unwrap();
        h.run_steps(2);
        h.state_mut().run_action(Action::ChangeCase(Case::Upper));
        h.run_steps(2);
        assert_eq!(h.state().doc.lettering(t).unwrap().text, "CAMP FIRE");
        assert_eq!(
            h.state().action_checked(Action::ChangeCase(Case::Upper)),
            Some(true)
        );
        h.state_mut().run_action(Action::AlignText(Align::Left));
        h.run_steps(2);
        assert_eq!(h.state().doc.lettering(t).unwrap().align, Align::Left);
        // A path to follow: an arc drawn below the text.
        let arc: Vec<P> = (0..=20)
            .map(|k| {
                let a = std::f64::consts::PI * (1.0 + k as f64 / 20.0);
                P::new(300.0 * a.cos(), 200.0 + 150.0 * a.sin())
            })
            .collect();
        let path = h
            .state_mut()
            .structural(|d| d.add_drawing(&arc, false, [0, 0, 0], Kind::Run).map(|_| ()))
            .map(|_| h.state().doc.shapes.len() - 1)
            .unwrap();
        h.run_steps(2);
        h.state_mut().set_selection(&[t, path]);
        h.run_steps(1);
        assert!(h.state().action_enabled(Action::TypeOnPath));
        h.state_mut().run_action(Action::TypeOnPath);
        h.run_steps(3);
        let ti = (0..h.state().doc.shapes.len())
            .find(|&i| h.state().doc.lettering(i).is_some())
            .unwrap_or_else(|| panic!("{}", h.state().status));
        let on = h.state().doc.lettering(ti).unwrap();
        assert!(on.path.is_some(), "{}", h.state().status);
        let b = crate::app::shape_bounds(&h.state().doc.shapes[ti]);
        assert!(
            b.center().y > 40.0,
            "moved down onto the arc: {:?}",
            b.center()
        );
        let pi = h
            .state()
            .doc
            .shapes
            .iter()
            .position(|s| s.id.starts_with("drawing_"))
            .unwrap();
        assert_eq!(
            h.state().doc.shapes[pi].params.kind,
            Some(Kind::Skip),
            "the path isn't sewn"
        );
        // Outlines: no longer text, same stitch type.
        h.state_mut().set_selection(&[ti]);
        h.run_steps(1);
        h.state_mut().run_action(Action::CreateOutlines);
        h.run_steps(3);
        assert!(
            h.state().doc.shapes.iter().enumerate().all(|(i, _)| h
                .state()
                .doc
                .lettering(i)
                .is_none())
        );
        let o = h.state().sel_index().unwrap();
        assert_eq!(h.state().doc.shapes[o].params.kind, Some(Kind::Satin));
        assert!(h.state().status.starts_with("Outlines made"));
    }

    #[test]
    fn kerning_from_the_type_tool_and_the_panel() {
        use egui_kittest::kittest::Queryable;
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(2);
        h.key_press(eframe::egui::Key::T);
        h.run_steps(1);
        let rect = h.state().canvas_rect;
        let at = h.state().view.to_screen(rect, P::new(0.0, 0.0));
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(2);
        for ch in ["A", "V"] {
            h.event(eframe::egui::Event::Text(ch.into()));
            h.run_steps(1);
        }
        let wide = crate::app::shape_bounds(&h.state().doc.shapes[0]).width();
        // Option+← twice: the AV pair 0.2 mm tighter.
        for _ in 0..2 {
            h.event(eframe::egui::Event::Key {
                key: eframe::egui::Key::ArrowLeft,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: eframe::egui::Modifiers::ALT,
            });
            h.run_steps(2);
        }
        let spec = h.state().doc.lettering(0).unwrap();
        assert!((spec.kern_at(1) + 0.2).abs() < 1e-9, "{:?}", spec.kern);
        let narrow = crate::app::shape_bounds(&h.state().doc.shapes[0]).width();
        assert!(wide - narrow > 1.5, "{wide} -> {narrow}");
        assert!(
            h.state().status.starts_with("Kerning AV"),
            "{}",
            h.state().status
        );
        h.key_press(eframe::egui::Key::Escape);
        h.run_steps(2);
        // The panel shows the Kern row for the selected text.
        h.get_by_label("Kern");
    }

    #[test]
    #[ignore]
    fn bench_typing_frames() {
        let path =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/badge.svg"));
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(Some(path), None),
            );
        h.run_steps(4);
        h.key_press(eframe::egui::Key::T);
        h.run_steps(1);
        let rect = h.state().canvas_rect;
        let at = h.state().view.to_screen(rect, P::new(0.0, 300.0));
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(2);
        let t = std::time::Instant::now();
        for ch in "CAMPFIRE".chars() {
            h.event(eframe::egui::Event::Text(ch.to_string()));
            h.run_steps(1);
        }
        eprintln!(
            "typing frame: {:.1} ms",
            t.elapsed().as_secs_f64() * 1000.0 / 8.0
        );
        let t = std::time::Instant::now();
        h.run_steps(10);
        eprintln!(
            "idle frame while typing: {:.1} ms",
            t.elapsed().as_secs_f64() * 1000.0 / 10.0
        );
        h.key_press(eframe::egui::Key::Escape);
        h.run_steps(2);
        // Drag the text.
        h.key_press(eframe::egui::Key::V);
        h.run_steps(1);
        let i = h.state().doc.shapes.len() - 1;
        let c = h.state().view.to_screen(
            rect,
            crate::app::shape_bounds(&h.state().doc.shapes[i]).center(),
        );
        h.hover_at(c);
        h.run_steps(1);
        h.drag_at(c);
        h.run_steps(1);
        let t = std::time::Instant::now();
        for k in 1..=20 {
            h.hover_at(c + eframe::egui::Vec2::new(k as f32 * 3.0, 0.0));
            h.run_steps(1);
        }
        eprintln!(
            "drag frame: {:.1} ms",
            t.elapsed().as_secs_f64() * 1000.0 / 20.0
        );
        h.drop_at(c + eframe::egui::Vec2::new(60.0, 0.0));
        h.run_steps(2);
    }

    #[test]
    fn new_text_starts_in_the_font_last_used() {
        let mut s = Studio::with_file(None, None);
        let _ = s.run_tool("add_text", &serde_json::json!({ "text": "CAMP" }));
        let fonts = s.fonts_here();
        let was = s.doc.lettering(0).unwrap().font;
        let other = fonts
            .iter()
            .find(|f| **f != was)
            .expect("another font")
            .clone();
        let mut spec = s.doc.lettering(0).unwrap();
        spec.font = other.clone();
        s.update_lettering(0, &spec).unwrap();
        assert_eq!(s.new_text_spec().font, other, "in this session");
        assert_eq!(s.prefs.last_font.as_deref(), Some(other.as_str()));
        // And the next time ZigZag opens, by the font alone.
        let mut fresh = Studio::with_file(None, None);
        fresh.prefs.last_font = Some(other.clone());
        assert_eq!(fresh.new_text_spec().font, other, "next session");
        assert!(fresh.new_text_spec().text.is_empty());
        // (Not one that's no longer installed.)
        fresh.prefs.last_font = Some("No Such Font Anywhere".into());
        assert_ne!(fresh.new_text_spec().font, "No Such Font Anywhere");
    }

    #[test]
    fn a_letter_height_typed_into_either_box_is_taken() {
        // The lettering panel's box, then the task bar's under the text.
        for which in [0usize, 1] {
            let mut h = Harness::builder()
                .with_size(Vec2::new(1400.0, 860.0))
                .build_ui_state(
                    |ui, s: &mut Studio| s.frame(ui),
                    Studio::with_file(None, None),
                );
            h.run_steps(2);
            let _ = h
                .state_mut()
                .run_tool("add_text", &serde_json::json!({ "text": "CAMP" }));
            h.run_steps(3);
            h.state_mut().set_selection(&[0]);
            h.run_steps(3);
            let boxes: Vec<egui::Rect> = h
                .query_all_by_role(egui::accesskit::Role::SpinButton)
                .filter(|n| n.value().as_deref() == Some("8.0 mm"))
                .map(|n| n.rect())
                .collect();
            assert_eq!(boxes.len(), 2, "the panel's and the task bar's");
            let at = boxes[which].center();
            h.hover_at(at);
            h.run_steps(1);
            h.drag_at(at);
            h.run_steps(1);
            h.drop_at(at);
            h.run_steps(3);
            // As a keyboard types: each key down, its text, key up.
            let keys = [
                (egui::Key::Backspace, None),
                (egui::Key::Backspace, None),
                (egui::Key::Backspace, None),
                (egui::Key::Num1, Some("1")),
                (egui::Key::Num2, Some("2")),
            ];
            for (key, text) in keys {
                h.key_down(key);
                if let Some(t) = text {
                    h.event(egui::Event::Text(t.into()));
                }
                h.run_steps(1);
                h.key_up(key);
                h.run_steps(1);
            }
            h.key_press(egui::Key::Enter);
            h.run_steps(4);
            let size = h.state().doc.lettering(0).unwrap().size_mm;
            assert!((size - 12.0).abs() < 1e-9, "box {which}: {size}");
        }
    }

    #[test]
    fn add_text_window_creates_editable_lettering() {
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 860.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(2);
        h.state_mut().run_action(crate::commands::Action::AddText);
        h.run_steps(2);
        h.get_by_label("Text").click();
        h.run_steps(1);
        h.get_by_label("Text").type_text("CAMP");
        h.run_steps(2);
        h.get_by_label("Add to Design").click();
        h.run_steps(4);
        assert_eq!(h.state().doc.shapes.len(), 1);
        assert_eq!(h.state().doc.lettering(0).unwrap().text, "CAMP");
        assert_eq!(h.state().doc.shapes[0].params.kind, Some(Kind::Satin));
        assert!(h.state().design.stitch_count() > 100);

        // Edit it through the API the panel uses; position stays.
        let before = zigzag::svg::shapes_bounds(&h.state().doc.shapes);
        let spec = TextSpec {
            text: "CAMPFIRE".into(),
            ..h.state().doc.lettering(0).unwrap()
        };
        h.state_mut().update_lettering(0, &spec).unwrap();
        h.run_steps(2);
        let after = zigzag::svg::shapes_bounds(&h.state().doc.shapes);
        assert!(after.width() > before.width() * 1.5);
        assert!((after.center().y - before.center().y).abs() < 3.0);
        assert_eq!(h.state().undo.len(), 2);
    }
}

#[cfg(test)]
mod editing_tests {
    use super::*;
    use crate::app::{Studio, Tool};
    use eframe::egui::{Key, Vec2};
    use egui_kittest::Harness;
    use zigzag::geom::P;

    fn texts(h: &Harness<'_, Studio>) -> Vec<String> {
        (0..h.state().doc.shapes.len())
            .filter_map(|i| h.state().doc.lettering(i).map(|s| s.text))
            .collect()
    }

    fn click(h: &mut Harness<'static, Studio>, at: egui::Pos2) {
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(2);
    }

    fn type_text(h: &mut Harness<'static, Studio>, text: &str) {
        for ch in text.chars() {
            h.event(egui::Event::Text(ch.to_string()));
            h.run_steps(1);
        }
    }

    /// "Hello" typed on an empty canvas and finished; returns its center.
    fn with_hello() -> (Harness<'static, Studio>, egui::Pos2) {
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            // (Frames 1/60 s apart, so two quick clicks make a double-click.)
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(3);
        let rect = h.state().canvas_rect;
        let at = h.state().view.to_screen(rect, P::new(0.0, 0.0));
        h.key_press(Key::T);
        h.run_steps(1);
        click(&mut h, at);
        type_text(&mut h, "Hello");
        h.key_press(Key::Escape);
        h.run_steps(2);
        assert_eq!(texts(&h), ["Hello"]);
        let c = h.state().view.to_screen(
            rect,
            crate::app::shape_bounds(&h.state().doc.shapes[0]).center(),
        );
        (h, c)
    }

    /// The Type tool on an empty canvas: the text placed, "Text" in it.
    fn placed() -> (Harness<'static, Studio>, egui::Pos2) {
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(|ui, s: &mut Studio| s.frame(ui), Studio::with_file(None, None));
        h.run_steps(3);
        let rect = h.state().canvas_rect;
        let at = h.state().view.to_screen(rect, P::new(0.0, 0.0));
        h.key_press(Key::T);
        h.run_steps(1);
        click(&mut h, at);
        assert_eq!(texts(&h), ["Text"]);
        (h, at)
    }

    #[test]
    fn picking_a_single_line_font_sews_the_letters_as_a_run() {
        let (mut h, _) = with_hello();
        let s = h.state_mut();
        assert_eq!(s.doc.shapes[0].params.kind, Some(Kind::Satin));
        let fonts = s.fonts();
        assert!(fonts.iter().any(|f| f.name == "Hershey Sans" && f.embroidery));
        let mut spec = s.doc.lettering(0).unwrap();
        spec.font = "Hershey Sans".into();
        picked_font(&mut spec, &fonts);
        s.update_lettering(0, &spec).unwrap();
        h.run_steps(2);
        let s = h.state();
        assert!(s.doc.shapes[0].is_single_line());
        assert_eq!(s.doc.lettering(0).unwrap().kind, Kind::Run);
        assert_eq!(s.doc.shapes[0].params.kind, Some(Kind::Run));
    }

    #[test]
    fn new_text_starts_with_its_placeholder_selected() {
        // Selected, as Illustrator's: Delete takes the word away...
        let (mut h, _) = placed();
        let t = h.state().lettering.typing.as_ref().unwrap();
        assert_eq!(t.selected_text(), "Text");
        h.key_press(Key::Backspace);
        h.run_steps(2);
        assert_eq!(h.state().lettering.typing.as_ref().unwrap().text, "");
        type_text(&mut h, "CAMP");
        h.run_steps(2);
        assert_eq!(texts(&h), ["CAMP"]);
        // ...typing replaces it...
        let (mut h, _) = placed();
        type_text(&mut h, "Hi");
        h.run_steps(2);
        assert_eq!(texts(&h), ["Hi"]);
        // ...a click puts the caret in it, to edit it like any text...
        let (mut h, at) = placed();
        let c = h.state().view.to_screen(
            h.state().canvas_rect,
            crate::app::shape_bounds(&h.state().doc.shapes[0]).max,
        );
        click(&mut h, egui::pos2(c.x - 1.0, at.y));
        type_text(&mut h, "s");
        h.run_steps(2);
        assert_eq!(texts(&h), ["Texts"]);
        // ...and left untouched, the new text goes again.
        let (mut h, _) = placed();
        h.key_press(Key::Escape);
        h.run_steps(2);
        assert!(texts(&h).is_empty());
    }

    #[test]
    fn the_panels_text_field_keeps_its_keys_while_the_canvas_is_typing() {
        use egui_kittest::kittest::Queryable;
        let (mut h, _) = placed();
        assert!(h.state().lettering.typing.is_some());
        let field = h.get_by_label("Text");
        field.click();
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
        h.run_steps(1);
        h.get_by_label("Text").type_text("Patch");
        h.run_steps(2);
        // (Into the field, not the letters on the canvas.)
        assert_eq!(h.get_by_label("Text").value().as_deref(), Some("Patch"));
        // Set as the field is left.
        h.key_press(Key::Tab);
        h.run_steps(3);
        assert_eq!(texts(&h), ["Patch"]);
    }

    #[test]
    fn deleting_every_letter_hides_the_text_and_typing_replaces_it() {
        let (mut h, c) = with_hello();
        click(&mut h, c);
        assert!(h.state().lettering.typing.is_some(), "editing the text");
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
        h.run_steps(1);
        h.key_press(Key::Backspace);
        h.run_steps(2);
        // The letters stay in the document, hidden, until something's typed.
        assert_eq!(texts(&h), ["Hello"]);
        assert_eq!(h.state().hidden_text(), Some(0));
        type_text(&mut h, "New");
        h.run_steps(1);
        assert_eq!(h.state().hidden_text(), None);
        h.key_press(Key::Escape);
        h.run_steps(2);
        assert_eq!(texts(&h), ["New"]);
        assert_eq!(h.state().doc.shapes.len(), 1);
    }

    #[test]
    fn deleting_every_letter_and_finishing_removes_the_text() {
        let (mut h, c) = with_hello();
        click(&mut h, c);
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
        h.run_steps(1);
        h.key_press(Key::Backspace);
        h.run_steps(2);
        h.key_press(Key::Escape);
        h.run_steps(2);
        assert!(h.state().doc.shapes.is_empty(), "the text is gone");
        assert!(h.state().lettering.typing.is_none());
        // One undo step brings it back.
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
        h.run_steps(2);
        assert_eq!(texts(&h), ["Hello"]);
    }

    #[test]
    fn text_is_sewn_the_same_wherever_its_moved() {
        let (mut h, _) = with_hello();
        // A filled shape too: what holds for text holds for any fill.
        let square = zigzag::bezier::rect(P::new(-100.0, 300.0), P::new(100.0, 450.0), 0.0);
        h.state_mut()
            .structural(|d| {
                d.add_drawing_path(&square, [200, 30, 30], Kind::Fill)
                    .map(|_| ())
            })
            .unwrap();
        h.state_mut().doc.settings.connect_mm = 3.6;
        // Filled letters: rows of stitches, which is where a grid that
        // stays put shows. (Satin letters are columns, and a column can
        // still begin on its other side after a move, the two being as
        // good as each other.)
        let spec = TextSpec {
            kind: Kind::Fill,
            ..h.state().doc.lettering(0).unwrap()
        };
        h.state_mut().update_lettering(0, &spec).unwrap();
        h.run_steps(2);
        // Each object's stitches, from its own corner.
        let sewn = |h: &Harness<'_, Studio>, i: usize| -> Vec<Vec<(i64, i64)>> {
            let s = h.state();
            let o = crate::app::shape_bounds(&s.doc.shapes[i]).min;
            let mut runs = Vec::new();
            for (b, blk) in s.design.blocks.iter().enumerate() {
                for (r, run) in blk.runs.iter().enumerate() {
                    let spans = &s.design.stitch_shapes[b][r];
                    for (k, &(from, of)) in spans.iter().enumerate() {
                        let to = spans.get(k + 1).map_or(run.pts.len(), |x| x.0);
                        if of == i {
                            runs.push(
                                run.pts[from..to]
                                    .iter()
                                    .map(|p| {
                                        (
                                            ((p.x - o.x) * 100.0).round() as i64,
                                            ((p.y - o.y) * 100.0).round() as i64,
                                        )
                                    })
                                    .collect(),
                            );
                        }
                    }
                }
            }
            runs
        };
        // (To a hundredth of a millimetre: where it's put is kept to a
        // few decimals.)
        let alike = |a: &[Vec<(i64, i64)>], b: &[Vec<(i64, i64)>]| -> Result<(), String> {
            if a.len() != b.len() {
                return Err(format!("{} runs for {}", a.len(), b.len()));
            }
            for (k, (x, y)) in a.iter().zip(b).enumerate() {
                if x.len() != y.len() {
                    return Err(format!("run {k}: {} stitches for {}", x.len(), y.len()));
                }
                let off: Vec<usize> = (0..x.len())
                    .filter(|&i| (x[i].0 - y[i].0).abs() > 10 || (x[i].1 - y[i].1).abs() > 10)
                    .collect();
                if let Some(&i) = off.first() {
                    return Err(format!(
                        "run {k}: {} of {} stitches differ, {:?}; the first: {:?} for {:?}",
                        off.len(),
                        x.len(),
                        &off[..off.len().min(12)],
                        x[i],
                        y[i]
                    ));
                }
            }
            Ok(())
        };
        let (text, shape) = (sewn(&h, 0), sewn(&h, 1));
        assert!(text.iter().map(Vec::len).sum::<usize>() > 100);
        let joins = h.state().design.bridges.len();
        // Dragged about, by amounts that are no multiple of anything.
        for d in [
            P::new(3.3, 0.0),
            P::new(0.0, 1.7),
            P::new(-71.3, 13.9),
            P::new(128.6, -44.1),
        ] {
            h.state_mut().set_selection(&[0, 1]);
            h.state_mut()
                .transform_selected(zigzag::geom::translation(d), "Moved");
            h.run_steps(2);
            assert_eq!(
                alike(&sewn(&h, 0), &text),
                Ok(()),
                "the text, moved by {d:?}"
            );
            assert_eq!(
                alike(&sewn(&h, 1), &shape),
                Ok(()),
                "the square, moved by {d:?}"
            );
            assert_eq!(h.state().design.bridges.len(), joins, "joined as it was");
        }
        // The text alone, away from the square: its letters as they were.
        h.state_mut().set_selection(&[0]);
        h.state_mut()
            .transform_selected(zigzag::geom::translation(P::new(40.7, -160.3)), "Moved");
        h.run_steps(2);
        assert_eq!(
            alike(&sewn(&h, 0)[1..], &text[1..]),
            Ok(()),
            "all but how it's reached"
        );
        // And after a save and reopening.
        let svg = h.state().doc.to_svg().unwrap();
        let again = zigzag::document::Document::from_svg(&svg, "t", None).unwrap();
        h.state_mut().set_document(again);
        h.run_steps(3);
        assert_eq!(alike(&sewn(&h, 0)[1..], &text[1..]), Ok(()));
        assert_eq!(alike(&sewn(&h, 1), &shape), Ok(()));
    }

    #[test]
    fn a_change_of_font_never_waits_on_the_satin_columns() {
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        // (As the app does it: by how long plans take.)
        h.state_mut().planner.timed = true;
        h.run_steps(2);
        let spec = TextSpec {
            text: "Hi".into(),
            kind: Kind::Satin,
            ..TextSpec::default()
        };
        let i = h.state_mut().add_lettering(&spec, (0.0, 0.0)).unwrap();
        let wait = |h: &mut Harness<'_, Studio>| {
            for _ in 0..4000 {
                h.run_steps(1);
                if !h.state().planner.pending && !h.state().dirty {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            panic!("the plan never arrived");
        };
        wait(&mut h);
        assert!(h.state().satin_info_shown(i).is_some());
        // The last plan was quick: the next would be made in the frame,
        // were it not that new letters may take any time at all.
        h.state_mut().planner.slow = false;
        let bold = TextSpec {
            bold: !spec.bold,
            ..spec.clone()
        };
        h.state_mut().update_lettering(i, &bold).unwrap();
        // Nothing found on this thread meanwhile: not for the Stitch
        // panel's width, nor for the rails on the canvas.
        assert!(h.state().satin_info_shown(i).is_none());
        h.run_steps(1);
        assert!(h.state().planner.pending, "planned by the worker");
        assert!(h.state().satin_info_shown(i).is_none());
        wait(&mut h);
        let i = h.state().sel_index().unwrap();
        let info = h.state().satin_info_shown(i).expect("once it's planned");
        assert!(info.mean_width > 0.0);
    }

    #[test]
    fn arrow_keys_step_past_fonts_that_would_have_to_be_downloaded() {
        let item = |name: &str, have| FontItem {
            name: name.to_string(),
            have,
            embroidery: false,
        };
        let fonts = vec![
            item("Alpha", Have::Here),
            item("Cloud One", Have::Download),
            item("Cloud Two", Have::Getting),
            item("Gamma", Have::Here),
        ];
        let mut h = Harness::builder()
            .with_size(Vec2::new(300.0, 400.0))
            .build_ui_state(
                move |ui, s: &mut (String, String, Vec<String>)| {
                    if font_list(ui, &mut s.0, &fonts, &mut s.1) {
                        s.2.push(s.0.clone());
                    }
                },
                ("Alpha".to_string(), String::new(), Vec::new()),
            );
        h.run_steps(2);
        h.key_press(Key::ArrowDown);
        h.run_steps(2);
        h.key_press(Key::ArrowUp);
        h.run_steps(2);
        // (Never set in a catalogue font on the way: that downloads it.)
        assert_eq!(h.state().2, ["Gamma", "Alpha"]);
    }

    #[test]
    fn arrow_keys_step_through_a_font_list() {
        let fonts: Vec<FontItem> = ["Alpha", "Beta", "Bravo", "Gamma"]
            .iter()
            .map(|s| FontItem {
                name: s.to_string(),
                have: Have::Here,
                embroidery: false,
            })
            .collect();
        // (font, what's typed in the search field, times it changed)
        let mut h = Harness::builder()
            .with_size(Vec2::new(300.0, 400.0))
            .build_ui_state(
                {
                    let fonts = fonts.clone();
                    move |ui, s: &mut (String, String, usize)| {
                        if font_list(ui, &mut s.0, &fonts, &mut s.1) {
                            s.2 += 1;
                        }
                    }
                },
                ("Beta".to_string(), String::new(), 0),
            );
        h.run_steps(2);
        let press = |h: &mut Harness<'_, (String, String, usize)>, k: Key| {
            h.key_press(k);
            h.run_steps(2);
        };
        press(&mut h, Key::ArrowDown);
        assert_eq!(h.state().0, "Bravo");
        press(&mut h, Key::ArrowDown);
        press(&mut h, Key::ArrowDown);
        assert_eq!(h.state().0, "Gamma", "stops at the last");
        assert_eq!(h.state().2, 2, "each step is a change, the stop isn't");
        press(&mut h, Key::ArrowUp);
        press(&mut h, Key::ArrowUp);
        press(&mut h, Key::ArrowUp);
        press(&mut h, Key::ArrowUp);
        assert_eq!(h.state().0, "Alpha", "and at the first");
        // The search field has the keyboard from the start: typing
        // narrows the list, and the arrows step through what's listed.
        // (No click needed: it took the keyboard as the list opened.)
        h.event(egui::Event::Text("b".into()));
        h.run_steps(2);
        assert_eq!(h.state().1, "b");
        press(&mut h, Key::ArrowDown);
        assert_eq!(h.state().0, "Beta", "into the list from a font not in it");
        press(&mut h, Key::ArrowDown);
        assert_eq!(h.state().0, "Bravo");
        press(&mut h, Key::ArrowDown);
        assert_eq!(h.state().0, "Bravo");
        assert_eq!(h.state().1, "b", "the arrows aren't the field's");
    }

    /// How many stitches of the plan on show are inside `b`.
    fn stitches_in(h: &Harness<'_, Studio>, b: zigzag::geom::Bounds) -> usize {
        h.state()
            .design
            .blocks
            .iter()
            .flat_map(|blk| &blk.runs)
            .flat_map(|r| &r.pts)
            .filter(|p| p.x >= b.min.x && p.x <= b.max.x && p.y >= b.min.y && p.y <= b.max.y)
            .count()
    }

    #[test]
    fn text_typed_away_or_deleted_is_off_the_canvas_at_once() {
        let (mut h, c) = with_hello();
        // A square in the text's thread, sewn right after it: one block
        // of stitches for the two.
        let color = h.state().doc.shapes[0].color;
        let square = zigzag::bezier::rect(P::new(-100.0, 300.0), P::new(100.0, 450.0), 0.0);
        h.state_mut()
            .structural(|d| d.add_drawing_path(&square, color, Kind::Fill).map(|_| ()))
            .unwrap();
        h.run_steps(2);
        assert_eq!(h.state().design.blocks.len(), 1, "one thread, one block");
        let text = crate::app::shape_bounds(&h.state().doc.shapes[0]);
        let below = crate::app::shape_bounds(&h.state().doc.shapes[1]);
        assert!(text.max.y < below.min.y);
        let (letters, rest) = (stitches_in(&h, text), stitches_in(&h, below));
        assert!(letters > 50 && rest > 50);

        // Every letter typed away: the text's stitches are hidden, those
        // sharing its block aren't.
        click(&mut h, c);
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
        h.run_steps(1);
        h.key_press(Key::Backspace);
        h.run_steps(2);
        assert_eq!(h.state().hidden_text(), Some(0));
        assert_eq!(stitches_in(&h, text), 0, "hidden, shared block or not");
        assert_eq!(stitches_in(&h, below), rest);
        // A plan made meanwhile doesn't bring them back.
        h.state_mut().dirty = true;
        h.run_steps(2);
        assert_eq!(stitches_in(&h, text), 0);
        assert_eq!(texts(&h), ["Hello"], "the letters are still the document's");
        // What's sewn or exported is planned afresh, as what's shown.
        h.state_mut().plan_now();
        assert_eq!(stitches_in(&h, text), 0);
        // Something typed: there are stitches again.
        type_text(&mut h, "Hi");
        h.run_steps(2);
        assert!(stitches_in(&h, text) > 20);
        // Replacing Hello with Hi can change the route into the square.
        // Deletion must preserve this new plan, not the one before typing.
        let rest = stitches_in(&h, below);
        assert!(rest > 50);

        // Typed away again and the edit ended, while planning is the
        // worker's (a big design, or a busy computer): the old plan stays
        // on show until the new one arrives, without the letters.
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
        h.run_steps(1);
        h.key_press(Key::Backspace);
        h.run_steps(2);
        h.state_mut().planner.slow = true;
        h.state_mut().finish_typing();
        assert_eq!(texts(&h), [] as [&str; 0], "the text is deleted");
        assert_eq!(stitches_in(&h, text), 0, "and doesn't flash back");
        assert_eq!(stitches_in(&h, below), rest);
        h.step();
        assert_eq!(stitches_in(&h, text), 0);
        for _ in 0..200 {
            if !h.state().planner.pending && !h.state().dirty {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            h.step();
        }
        // (The square alone now: begun and ended where suits it.)
        assert!(stitches_in(&h, below) > 50, "the new plan");
        assert_eq!(stitches_in(&h, text), 0);
        assert!(!h.state().plan_cut);

        // Undo brings the text back; deleted with the Selection tool (the
        // worker planning again), it's off the canvas in that frame too.
        h.key_press_modifiers(egui::Modifiers::COMMAND, Key::Z);
        h.run_steps(3);
        assert_eq!(texts(&h).len(), 1);
        assert!(stitches_in(&h, text) > 20);
        h.state_mut().set_tool(crate::app::Tool::Select);
        let t = (0..h.state().doc.shapes.len())
            .find(|&i| h.state().doc.lettering(i).is_some())
            .unwrap();
        h.state_mut().set_selection(&[t]);
        h.state_mut().planner.slow = true;
        h.state_mut().delete_selection();
        assert!(h.state().dirty, "not planned yet");
        assert_eq!(stitches_in(&h, text), 0);
        assert!(stitches_in(&h, below) > 50);
    }

    #[test]
    fn double_click_on_text_with_the_selection_tool_edits_it() {
        let (mut h, c) = with_hello();
        h.key_press(Key::V);
        h.run_steps(1);
        assert_eq!(h.state().tool, Tool::Select);
        h.hover_at(c);
        // (egui counts clicks by its clock, a frame step at a time here:
        // long enough after the earlier click.)
        h.run_steps(30);
        for _ in 0..2 {
            h.drag_at(c);
            h.run_steps(1);
            h.drop_at(c);
            h.run_steps(1);
        }
        h.run_steps(2);
        assert_eq!(h.state().tool, Tool::Text, "double-click edits the text");
        let t = h.state().lettering.typing.as_ref().expect("typing");
        assert_eq!(t.text, "Hello");
        assert!(!t.fresh);
        type_text(&mut h, "!");
        h.key_press(Key::Escape);
        h.run_steps(2);
        assert!(texts(&h)[0].contains('!'), "{:?}", texts(&h));
    }

    #[test]
    fn text_scaled_past_the_height_box_keeps_its_size_when_edited() {
        // 20 mm text scaled to three times the size by its frame is 60 mm,
        // more than the height boxes go to: changing something else (bold,
        // in the task bar) leaves it 60 mm.
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 860.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(2);
        let _ = h.state_mut().run_tool(
            "add_text",
            &serde_json::json!({ "text": "CAMP", "size_mm": 20.0 }),
        );
        h.run_steps(3);
        let c = zigzag::svg::shapes_bounds(&h.state().doc.shapes[0..=0]).center();
        let m = zigzag::geom::affine_mul(
            zigzag::geom::translation(c),
            zigzag::geom::affine_mul(
                [3.0, 0.0, 0.0, 3.0, 0.0, 0.0],
                zigzag::geom::translation(zigzag::geom::P::new(-c.x, -c.y)),
            ),
        );
        h.state_mut().structural(|d| d.transform(&[0], m)).unwrap();
        h.state_mut().set_selection(&[0]);
        h.run_steps(3);
        let size = h.state().doc.lettering(0).unwrap().size_mm;
        assert!((size - 60.0).abs() < 0.1, "{size}");
        use egui_kittest::kittest::Queryable;
        // (The task bar's buttons and the panel's: each sets the text
        // again from the settings shown.)
        let mut taken = 0;
        for label in ["B", "Bold", "Italic", "I"] {
            let was = h.state().doc.lettering(0).unwrap();
            let found: Vec<egui::Rect> = h.query_all_by_label(label).map(|n| n.rect()).collect();
            for at in &found {
                click(&mut h, at.center());
                h.run_steps(4);
            }
            let now = h.state().doc.lettering(0).unwrap();
            taken += ((now.bold, now.italic) != (was.bold, was.italic)) as usize;
            assert!((now.size_mm - 60.0).abs() < 0.1, "{label}: {}", now.size_mm);
        }
        assert!(taken >= 2, "the buttons were pressed");
    }
}
