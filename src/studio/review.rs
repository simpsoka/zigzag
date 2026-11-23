//! Reviewing an agent's work. Notes pinned on the canvas show it where you
//! mean (a spot, or a box dragged out), go with the next prompt, and stay
//! until the agent (or you) resolves them. The design as it was before each
//! prompt is kept: flip back to it, set it beside what the agent made, or
//! go back to it. And the agent can try several versions, each shown side
//! by side with the original, to choose one.

use crate::app::{Studio, shape_bounds};
use crate::canvas::{self, DrawOptions, StitchMesh, View, color32};
use crate::icons::{self, Icon};
use crate::mcp::ToolResult;
use crate::theme::{pal_ui, space};
use eframe::egui::{self, Color32, Key, Pos2, Rect, Sense, Stroke, Vec2};
use serde_json::{Value, json};
use zigzag::design::Design;
use zigzag::document::Document;
use zigzag::formats::preview::Mark;
use zigzag::geom::P;

/// The notes' color: the interface's blue.
fn note_color(ctx: &egui::Context) -> Color32 {
    crate::theme::pal(ctx).accent
}

/// How big a note's marker is, and the buttons and icons beside notes and
/// the prompt.
const BUBBLE: f32 = 22.0;
pub const BUTTON: f32 = 24.0;
pub const ICON: f32 = 14.0;
/// The Agent panel's one text size (color, not size, tells what matters
/// more), and the prompt's and the comment box's.
pub const PANEL_PT: f32 = 13.5;
pub const FIELD_PT: f32 = PANEL_PT;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteState {
    /// Not sent yet: it goes with the next prompt.
    Open,
    /// Sent, and not resolved yet.
    Sent,
    /// Dealt with: off the canvas.
    Resolved,
}

/// A note pinned on the canvas for the agent.
#[derive(Clone, Debug)]
pub struct Note {
    pub id: u32,
    /// What it's called on the canvas, to the agent and in the panel.
    pub number: u32,
    /// The document it's on (its tab's recovery number).
    pub tab: u32,
    /// Where (design units): a pin, or with `to`, a box.
    pub at: P,
    pub to: Option<P>,
    /// The object a pin went on, and that object's center then: the pin
    /// goes where the object goes.
    pub on: Option<(usize, P)>,
    pub text: String,
    pub state: NoteState,
    /// What the agent said it did about it.
    pub reply: Option<String>,
}

impl Note {
    /// How far the object it's on has moved since it was pinned.
    fn shift(&self, doc: &Document) -> P {
        match self.on {
            Some((uid, was)) => doc
                .shapes
                .iter()
                .find(|s| s.uid == uid)
                .map(shape_bounds)
                .filter(|b| !b.is_empty())
                .map_or(P::default(), |b| b.center() - was),
            None => P::default(),
        }
    }

    /// Where it is now: (point, box corner).
    pub fn place(&self, doc: &Document) -> (P, Option<P>) {
        let d = self.shift(doc);
        (self.at + d, self.to.map(|t| t + d))
    }

    /// What it says, or that it says nothing.
    pub fn said(&self) -> String {
        let t = self.text.trim();
        if t.is_empty() {
            "(a pin, no comment)".into()
        } else {
            t.to_string()
        }
    }
}

/// A version of the design: what it was, and its stitches.
pub struct Version {
    pub label: String,
    pub doc: Document,
    pub design: Design,
    /// The stitches' bounds (walking them every frame adds up).
    pub bounds: zigzag::geom::Bounds,
    /// Tells its stitches apart when drawing (see `paint_stitches_in`).
    pub generation: u64,
    /// The document's fingerprint (see [`doc_key`]).
    pub key: u64,
}

/// One prompt to the agent (or one round of versions an outside agent
/// made): the design before, and what came of it.
pub struct Turn {
    pub id: u64,
    pub tab: u32,
    pub prompt: String,
    /// Its first line in the transcript, and its last once it's over.
    pub start: usize,
    pub end: Option<usize>,
    pub before: Version,
    /// The design after it, if it changed.
    pub after: Option<Version>,
    /// The versions it kept (see `save_variant`).
    pub variants: Vec<Version>,
    /// Still making versions.
    pub collecting: bool,
    /// Started by an agent connected from outside (`start_variants`), not
    /// a prompt in the Agent panel.
    pub outside: bool,
    /// The design's problems when the prompt went (what's new after is
    /// the turn's doing).
    pub problems: Vec<(Option<String>, String)>,
}

/// What covers the canvas instead of the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Showing {
    /// The design as it was before a turn (by its id).
    Before(u64),
    /// A turn's versions side by side.
    Compare(u64),
}

/// One of the panes of a comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Before,
    After,
    Variant(usize),
    /// The document as it is (a version being made).
    Live,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnAction {
    Before(u64),
    After,
    Compare(u64),
    Revert(u64),
    Use { turn: u64, pane: Pane },
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotesAction {
    Edit(u32),
    Remove(u32),
    Resolve(u32),
}

#[derive(Default)]
pub struct Review {
    pub notes: Vec<Note>,
    /// Clicks on the canvas pin notes (N).
    pub annotating: bool,
    /// The note whose text is being written.
    pub editing: Option<u32>,
    /// Give that note's field the keyboard next frame.
    pub focus: bool,
    /// Where a box being dragged out began.
    pub press: Option<P>,
    pub turns: Vec<Turn>,
    pub showing: Option<Showing>,
    /// The comparison's view: every pane shows the same part of the hoop
    /// at the same size, so moving or zooming one moves them all.
    pub view: View,
    /// The pane size that view was fitted to.
    pub fitted: Option<Vec2>,
    /// Where each pane of the comparison was last drawn.
    pub panes: Vec<(Pane, Rect)>,
    /// Whether the document differs from a turn's original, by (turn,
    /// plan number): worked out once per plan, not every frame.
    changed: std::cell::Cell<Option<(u64, u64, bool)>>,
    /// The comments' chip in the prompt box is opened out into a list.
    pub notes_open: bool,
    /// The comment whose marker was clicked (Delete takes it away).
    pub selected: Option<u32>,
    /// The markers are hidden (the annotation bar's eye).
    pub hidden: bool,
    /// Where the annotation bar was dragged to, from its usual place.
    pub bar_offset: Vec2,
    /// While pinning comments: the tool, and the design's fingerprint, as
    /// they were (kept up to date with the agent's changes). Another tool,
    /// or a change of the person's own, ends pinning.
    /// (With the document's revision the fingerprint was taken at: it's
    /// only taken again when that moves.)
    watch: Option<(crate::app::Tool, u64, u64)>,
    /// A note was sent while the agent was busy: it goes when it's done.
    /// (For the document with this recovery number.)
    pub queued: Option<u32>,
    /// What the person chose in a comparison since the last prompt, for
    /// the agent to be told with the next.
    /// (With the document it was chosen in.)
    pub chose: Option<(u32, String)>,
    next_id: u32,
    next_turn: u64,
    next_gen: u64,
    /// The panes' stitch meshes, when drawn by the painter (tests).
    pub meshes: std::cell::RefCell<Vec<Option<StitchMesh>>>,
}

impl Review {
    /// What's watched while comments are pinned, to put back after a look
    /// at another document (see `Studio::visit`).
    pub(crate) fn watch_kept(&self) -> Option<(crate::app::Tool, u64, u64)> {
        self.watch
    }

    pub(crate) fn watch_put(&mut self, w: Option<(crate::app::Tool, u64, u64)>) {
        self.watch = w;
    }

    /// How many notes on document `tab` go with the next prompt.
    pub fn pending(&self, tab: u32) -> usize {
        self.notes
            .iter()
            .filter(|n| n.tab == tab && n.state == NoteState::Open)
            .count()
    }

    /// The notes on `tab` still to be dealt with.
    pub fn live(&self, tab: u32) -> impl Iterator<Item = &Note> {
        self.notes
            .iter()
            .filter(move |n| n.tab == tab && n.state != NoteState::Resolved)
    }

    /// The Agent panel's turn that ended at line `k` of the transcript.
    /// The panel's turn on document `tab` that ended at line `k` of its chat.
    pub fn turn_ending_at(&self, k: usize, tab: u32) -> Option<&Turn> {
        self.turns
            .iter()
            .find(|t| !t.outside && t.tab == tab && t.end == Some(k))
    }

    pub fn turn(&self, id: u64) -> Option<&Turn> {
        self.turns.iter().find(|t| t.id == id)
    }

    fn turn_mut(&mut self, id: u64) -> Option<&mut Turn> {
        self.turns.iter_mut().find(|t| t.id == id)
    }

    /// The next note's number on `tab`: after the ones still there.
    fn next_number(&self, tab: u32) -> u32 {
        self.live(tab).map(|n| n.number).max().unwrap_or(0) + 1
    }

    /// Let go of the oldest turns past [`KEPT_TURNS`] (each holds the
    /// design before and after, stitches and all): not one still going,
    /// nor the one on show.
    fn prune(&mut self) {
        let showing = self
            .showing
            .map(|(Showing::Before(id) | Showing::Compare(id))| id);
        let mut extra = self.turns.len().saturating_sub(KEPT_TURNS);
        self.turns.retain(|t| {
            let going = t.collecting || (!t.outside && t.end.is_none());
            if extra == 0 || going || showing == Some(t.id) {
                return true;
            }
            extra -= 1;
            false
        });
    }

    fn open(&mut self, s: Showing) {
        self.showing = Some(s);
        self.fitted = None;
        self.annotating = false;
        self.editing = None;
    }
}

/// How many turns are kept to flip back to or compare.
const KEPT_TURNS: usize = 12;

/// A fingerprint of a document's objects, to tell whether it changed.
pub fn doc_key(d: &Document) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    d.shapes.len().hash(&mut h);
    for s in &d.shapes {
        crate::agent::fingerprint(s).hash(&mut h);
    }
    h.finish()
}

/// A version's letter: A, B, C…
pub fn letter(k: usize) -> char {
    (b'A' + (k % 26) as u8) as char
}

/// What `start_variants` says.
const START_VARIANTS: &str = "The design as it is now is the original; the person sees the versions beside it, side by side, as you make them. \
Make a version, look at it with render_preview, then call save_variant with a short label saying how it differs: it keeps that version and puts the design back to the original for the next. \
Make them genuinely different takes, not small tweaks of one another.";

/// The brief on notes, for the agent.
const NOTES_BRIEF: &str = "The person pinned numbered notes on the canvas to show where they mean \
(positions in mm from the hoop center, +x right, +y down; render_preview draws each as a blue pin at its position, \
or a dashed blue box with the pin at its top-left corner; the pins carry no numbers, so tell them apart by position). When you've dealt with a note, call resolve_note with its number and a few words on what \
you did.";

/// A note's marker: a speech bubble whose corner is on `tip` (the spot
/// meant), in the notes' color with a white border.
pub fn bubble(tip: Pos2, size: f32) -> Rect {
    Rect::from_min_max(tip - Vec2::new(0.0, size), tip + Vec2::new(size, 0.0))
}

pub fn paint_bubble(p: &egui::Painter, tip: Pos2, size: f32, alpha: f32) -> Rect {
    let r = bubble(tip, size);
    let round = (size / 2.0) as u8;
    let corners = egui::CornerRadius {
        nw: round,
        ne: round,
        sw: 0,
        se: round,
    };
    let border = (size / 11.0).max(1.0);
    if size > 16.0 {
        p.rect_filled(
            r.translate(Vec2::new(0.0, 1.5)),
            corners,
            Color32::from_black_alpha((50.0 * alpha) as u8),
        );
    }
    p.rect_filled(r, corners, note_color(p.ctx()).gamma_multiply(alpha));
    p.rect_stroke(
        r,
        corners,
        Stroke::new(border, Color32::WHITE.gamma_multiply(alpha)),
        egui::StrokeKind::Middle,
    );
    r
}

/// Annotate: the canvas marker's shape as a white (on light, dark)
/// outline beside Send; while on, white on a blue disc the size of Send's.
pub fn annotate_button(ui: &mut egui::Ui, on: bool) -> egui::Response {
    let p = pal_ui(ui);
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(BUTTON), Sense::click());
    // The same disc as Send: blue while on; a wash under the pointer.
    let fill = if on {
        if resp.hovered() {
            p.accent.gamma_multiply(0.85)
        } else {
            p.accent
        }
    } else if resp.hovered() {
        p.text.gamma_multiply(0.14)
    } else {
        Color32::TRANSPARENT
    };
    round_button(ui.painter(), r, fill);
    // (A filled-in shape looks bigger than a line: a little smaller than
    // Send's arrow, so the two weigh the same.)
    let icon = Rect::from_center_size(r.center(), Vec2::splat(ICON - 2.0));
    icons::paint(
        ui.painter(),
        icon,
        Icon::Comment,
        if on { Color32::WHITE } else { p.text },
    );
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, "Annotate"));
    resp
}

/// The disc behind the prompt's buttons (Annotate, Send): one size for both.
fn round_button(p: &egui::Painter, r: Rect, fill: Color32) {
    if fill != Color32::TRANSPARENT {
        p.circle_filled(r.center(), BUTTON / 2.0, fill);
    }
}

/// Placeholder text in the prompt and the comment box: dimmer than the
/// interface's usual.
pub fn hint_color(p: crate::theme::Pal) -> Color32 {
    p.tertiary.gamma_multiply(0.7)
}

/// One comment in the prompt box's list: what it says (a click opens it
/// to write), the agent's reply once sent, and a button to take it away
/// (or, once sent, to resolve it) in the column of Annotate and Send.
fn note_row(ui: &mut egui::Ui, n: &Note, line: f32, out: &mut Option<NotesAction>) {
    let p = pal_ui(ui);
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
        ui.spacing_mut().item_spacing.x = space::S;
        let (icon, tip, act) = match n.state {
            NoteState::Sent => (Icon::Check, "Resolve", NotesAction::Resolve(n.id)),
            _ => (Icon::Close, "Remove Note", NotesAction::Remove(n.id)),
        };
        let w = ui.available_width() - BUTTON - space::S;
        ui.allocate_ui_with_layout(
            Vec2::new(w, line),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.set_min_width(w);
                ui.set_min_height(line);
                let empty = n.text.trim().is_empty();
                let text = if empty {
                    "Add a comment…".to_string()
                } else {
                    n.text.trim().to_string()
                };
                let color = if empty || n.state == NoteState::Sent {
                    p.tertiary
                } else {
                    p.text
                };
                let r = ui.add(
                    egui::Label::new(egui::RichText::new(text).size(PANEL_PT).color(color))
                        .wrap()
                        .sense(Sense::click()),
                );
                if n.state == NoteState::Open && r.on_hover_cursor(egui::CursorIcon::Text).clicked()
                {
                    *out = Some(NotesAction::Edit(n.id));
                }
                if let (NoteState::Sent, Some(reply)) = (n.state, &n.reply) {
                    ui.label(egui::RichText::new(reply).size(PANEL_PT).color(p.tertiary));
                }
            },
        );
        let (r, b) = ui.allocate_exact_size(Vec2::new(BUTTON, line), Sense::click());
        if b.hovered() {
            ui.painter().circle_filled(r.center(), line / 2.0, p.hover);
        }
        icons::paint(
            ui.painter(),
            Rect::from_center_size(r.center(), Vec2::splat(10.0)),
            icon,
            p.secondary,
        );
        b.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tip));
        if b.on_hover_text(tip).clicked() {
            *out = Some(act);
        }
    });
}

/// A button on the annotation bar: an icon, greyed out when there's
/// nothing for it to do.
fn bar_button(ui: &mut egui::Ui, icon: Icon, label: &str, enabled: bool) -> egui::Response {
    let p = pal_ui(ui);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(28.0), sense);
    if enabled && resp.hovered() {
        ui.painter().rect_filled(r, 6.0, p.hover);
    }
    icons::paint(
        ui.painter(),
        Rect::from_center_size(r.center(), Vec2::splat(16.0)),
        icon,
        if enabled {
            p.text
        } else {
            p.tertiary.gamma_multiply(0.6)
        },
    );
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    resp.on_hover_text(label)
}

/// Send: an arrow on a disc in the text's color (white on dark, dark on
/// light), no border; while the agent works, Stop.
pub fn send_button(ui: &mut egui::Ui, enabled: bool, running: bool) -> egui::Response {
    let p = pal_ui(ui);
    let sense = if enabled || running {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(BUTTON), sense);
    let fill = if !enabled && !running {
        p.text.gamma_multiply(0.3)
    } else if resp.hovered() {
        p.text.gamma_multiply(0.85)
    } else {
        p.text
    };
    round_button(ui.painter(), r, fill);
    let color = p.dock;
    if running {
        icons::paint(
            ui.painter(),
            Rect::from_center_size(r.center(), Vec2::splat(ICON - 4.0)),
            Icon::Stop,
            color,
        );
    } else {
        // The arrow, as heavy as the comment icon beside it.
        let c = r.center();
        let (h, w) = (ICON * 0.42, ICON * 0.32);
        let stroke = Stroke::new(icons::COMMENT_STROKE, color);
        let painter = ui.painter();
        painter.line_segment([c + Vec2::new(0.0, h), c - Vec2::new(0.0, h - 0.6)], stroke);
        painter.add(egui::Shape::line(
            vec![
                c + Vec2::new(-w, -h + w),
                c - Vec2::new(0.0, h),
                c + Vec2::new(w, -h + w),
            ],
            stroke,
        ));
    }
    let label = if running { "Stop" } else { "Send" };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled || running, label));
    resp.on_hover_text(label)
}

/// Which of a turn's view buttons is lit: (Before, After, Side by Side).
/// After is the design as it is, on the canvas: lit only when neither
/// of the others is showing that turn.
pub fn lit(showing: Option<Showing>, id: u64) -> (bool, bool, bool) {
    let before = showing == Some(Showing::Before(id));
    let side = showing == Some(Showing::Compare(id));
    (before, !before && !side, side)
}

/// How wide a turn's view buttons are (see [`turn_controls`]), to set
/// them at the right; none for a turn that changed nothing.
pub fn turn_controls_width(t: &Turn) -> f32 {
    let n = if !t.variants.is_empty() {
        1.0
    } else if t.after.is_some() {
        3.0
    } else {
        0.0
    };
    n * BUTTON + (n - 1.0f32).max(0.0) * 2.0
}

/// A small icon button under a turn, lit while `on`.
fn turn_button(ui: &mut egui::Ui, icon: Icon, label: &str, on: bool) -> egui::Response {
    let p = pal_ui(ui);
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(BUTTON), Sense::click());
    let fill = if on {
        p.accent.gamma_multiply(0.2)
    } else if resp.hovered() {
        p.hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(r, 6.0, fill);
    // (Dim like the Done line they sit on; the one showing in blue.)
    icons::paint(
        ui.painter(),
        Rect::from_center_size(r.center(), Vec2::splat(ICON)),
        icon,
        if on {
            p.accent
        } else if resp.hovered() {
            p.secondary
        } else {
            hint_color(p)
        },
    );
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, label));
    resp.on_hover_text(label)
}

/// Under a turn that changed the design, as icons: the design before it,
/// after it (as it is), and both side by side; or, for versions, comparing
/// them.
pub fn turn_controls(
    ui: &mut egui::Ui,
    review: &Review,
    t: &Turn,
    here: u32,
) -> Option<TurnAction> {
    if t.variants.is_empty() && t.after.is_none() {
        return None;
    }
    let mut out = None;
    ui.add_enabled_ui(t.tab == here, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let (before, after, side) = lit(review.showing, t.id);
            if !t.variants.is_empty() {
                let n = t.variants.len();
                let label = format!("Compare {n} Version{}", if n == 1 { "" } else { "s" });
                if turn_button(ui, Icon::Compare, &label, side).clicked() {
                    out = Some(if side {
                        TurnAction::Close
                    } else {
                        TurnAction::Compare(t.id)
                    });
                }
                return;
            }
            if turn_button(ui, Icon::BeforeHalf, "Before", before).clicked() && !before {
                out = Some(TurnAction::Before(t.id));
            }
            if turn_button(ui, Icon::AfterHalf, "After", after).clicked() && !after {
                out = Some(TurnAction::After);
            }
            if turn_button(ui, Icon::Compare, "Side by Side", side).clicked() {
                out = Some(if side {
                    TurnAction::After
                } else {
                    TurnAction::Compare(t.id)
                });
            }
        });
    })
    .response
    .on_disabled_hover_text("This was in another document");
    out
}

/// `n` panes in `rect`, in the grid that shows a design `aspect` wide (for
/// 1 tall) biggest; a last row that isn't full is centered.
pub fn grid(rect: Rect, n: usize, aspect: f32, gap: f32, header: f32) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let mut best = (0.0f32, 1usize);
    for cols in 1..=n {
        let rows = n.div_ceil(cols);
        let pw = (rect.width() - gap * (cols + 1) as f32) / cols as f32;
        let ph = (rect.height() - gap * (rows + 1) as f32) / rows as f32 - header;
        let s = (pw / aspect.max(0.05)).min(ph);
        if s > best.0 {
            best = (s, cols);
        }
    }
    let cols = best.1;
    let rows = n.div_ceil(cols);
    let pw = (rect.width() - gap * (cols + 1) as f32) / cols as f32;
    let ph = (rect.height() - gap * (rows + 1) as f32) / rows as f32;
    (0..n)
        .map(|k| {
            let (row, col) = (k / cols, k % cols);
            let in_row = if row == rows - 1 {
                n - row * cols
            } else {
                cols
            };
            let slack = (cols - in_row) as f32 * (pw + gap) / 2.0;
            Rect::from_min_size(
                egui::pos2(
                    rect.left() + gap + slack + col as f32 * (pw + gap),
                    rect.top() + gap + row as f32 * (ph + gap),
                ),
                Vec2::new(pw.max(1.0), ph.max(1.0)),
            )
        })
        .collect()
}

/// "12,345".
fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (k, c) in s.chars().enumerate() {
        if k > 0 && (s.len() - k).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// What a pane says under its name: stitches and threads, and against the
/// original's, how many more or fewer.
fn pane_facts(d: &Design, original: Option<&Design>) -> String {
    let n = d.stitch_count();
    let mut s = format!("{} stitches · {} colors", thousands(n), d.blocks.len());
    if let Some(o) = original {
        let delta = n as i64 - o.stitch_count() as i64;
        if delta != 0 {
            s.push_str(&format!(
                " · {}{} stitches",
                if delta > 0 { "+" } else { "-" },
                thousands(delta.unsigned_abs() as usize)
            ));
        }
    }
    s
}

impl Studio {
    /// The document as it is now, with its stitches.
    fn version(&mut self, label: impl Into<String>) -> Version {
        // (Here and now: an edit still with the planner's worker counts.)
        self.plan_now();
        if self.dirty_needs_plan() {
            self.replan();
        }
        self.review.next_gen += 1;
        Version {
            label: label.into(),
            doc: self.doc.clone(),
            design: self.design.clone(),
            bounds: self.design.bounds(),
            generation: (1 << 48) + self.review.next_gen,
            key: doc_key(&self.doc),
        }
    }

    /// Put `doc` in place of the document, as one undo step.
    fn restore(&mut self, doc: Document) {
        // (The design, not where it's saved or what it's called: that may
        // have changed since the version was kept.)
        let _ = self.structural(|d| {
            let (path, name) = (d.path.take(), std::mem::take(&mut d.name));
            *d = doc;
            d.path = path;
            d.name = name;
            Ok(())
        });
        self.selected = self
            .selected
            .filter(|&u| self.doc.shapes.iter().any(|s| s.uid == u));
        self.replan_soon();
    }

    /// A prompt went to the agent: keep the design as it is, and send the
    /// open notes with it.
    pub(crate) fn begin_turn(&mut self, prompt: String, start: usize) {
        let tab = self.session.rec;
        for n in &mut self.review.notes {
            if n.tab == tab && n.state == NoteState::Open {
                n.state = NoteState::Sent;
            }
        }
        self.review.queued = None;
        self.begin_turn_without_notes(prompt, start);
    }

    /// A prompt went to the agent on its own (the issues popover's Fix):
    /// the design is kept as it is, and the open notes stay open.
    pub(crate) fn begin_turn_without_notes(&mut self, prompt: String, start: usize) {
        let tab = self.session.rec;
        let before = self.version("Original");
        self.review.next_turn += 1;
        let id = self.review.next_turn;
        self.review.turns.push(Turn {
            id,
            tab,
            prompt,
            start,
            end: None,
            before,
            after: None,
            variants: Vec::new(),
            collecting: false,
            outside: false,
            problems: self.problems(),
        });
        self.review.prune();
    }

    /// The agent finished (or stopped): what it made of the design.
    pub(crate) fn end_turn(&mut self) {
        // A new chat: its turns go with it (an outside agent's stay, and
        // so do other documents', whose chats are their own).
        let lines = self.agent.log.len();
        let here = self.session.rec;
        let before = self.review.turns.len();
        self.review
            .turns
            .retain(|t| t.outside || t.tab != here || t.start < lines);
        if self.review.turns.len() != before {
            let showing = self.review.showing;
            if let Some(Showing::Before(id) | Showing::Compare(id)) = showing
                && self.review.turn(id).is_none()
            {
                self.review.showing = None;
            }
        }
        let Some(k) = self
            .review
            .turns
            .iter()
            .position(|t| !t.outside && t.tab == here && t.end.is_none())
        else {
            return;
        };
        self.review.turns[k].end = Some(lines.saturating_sub(1));
        let changed = doc_key(&self.doc) != self.review.turns[k].before.key;
        if self.review.turns[k].collecting {
            self.review.turns[k].collecting = false;
            // A last version it didn't save: kept anyway.
            if changed {
                let n = self.review.turns[k].variants.len();
                let v = self.version(format!("Version {}", letter(n)));
                let original = self.review.turns[k].before.doc.clone();
                self.review.turns[k].variants.push(v);
                self.restore(original);
                self.agent_changed();
            }
        } else if changed {
            self.review.turns[k].after = Some(self.version("After"));
            // What it left wrong that wasn't before: said, with the way
            // to have it put right.
            // (Problems: not the notes on how something will be sewn.)
            let new: Vec<Value> = self
                .new_problems(&self.review.turns[k].problems)
                .into_iter()
                .filter(|p| p["level"] != "note")
                .collect();
            if let Some(first) = new.first() {
                let what = first["message"]
                    .as_str()
                    .unwrap_or("a problem")
                    .trim_end_matches('.');
                let more = match new.len() - 1 {
                    0 => String::new(),
                    n => format!(" (and {n} more)"),
                };
                self.agent.log.push(crate::agent::Entry::Error(format!(
                    "The design has problems it didn't have before: {what}{more}. The Issues list has them all, and Fix sends them to the agent."
                )));
            }
        }
    }

    /// A document was closed: its notes and versions go with it.
    pub(crate) fn forget_review(&mut self, tab: u32) {
        self.leave_review();
        self.review.notes.retain(|n| n.tab != tab);
        self.review.turns.retain(|t| t.tab != tab);
        self.review.queued = None;
    }

    /// Another document came forward: what covered the canvas was the
    /// last one's.
    pub(crate) fn leave_review(&mut self) {
        self.close_compare();
        self.review.annotating = false;
        self.review.editing = None;
        self.review.press = None;
    }

    /// The names of the objects a note is on.
    fn note_objects(&self, n: &Note) -> Vec<String> {
        let (a, b) = n.place(&self.doc);
        let hit: Vec<usize> = match b {
            Some(b) => self.in_box(a, b),
            None => self.pick(a, 15.0).into_iter().collect(),
        };
        hit.into_iter()
            .filter_map(|i| self.names.get(i).cloned())
            .collect()
    }

    /// A note, as the agent is told it.
    fn describe_note(&self, n: &Note) -> String {
        let (a, b) = n.place(&self.doc);
        let mm = |p: P| format!("({:.1}, {:.1})", p.x / 10.0, p.y / 10.0);
        let place = match b {
            Some(b) => format!("The box from {} to {} mm", mm(a), mm(b)),
            None => format!("At {} mm", mm(a)),
        };
        let objects = self.note_objects(n);
        let on = match (objects.len(), b.is_some()) {
            (0, _) => String::new(),
            (_, true) => format!(", over {}", objects.join(", ")),
            (_, false) => format!(", on {}", objects.join(", ")),
        };
        format!("{place}{on}: {}", n.said())
    }

    /// The open notes, as the transcript shows them, and what the agent
    /// is told about them (the ones still open from before too).
    pub(crate) fn notes_for_prompt(&mut self) -> (Vec<String>, String) {
        let here = self.session.rec;
        // (Here and now: an edit still with the planner's worker counts.)
        self.plan_now();
        if self.dirty_needs_plan() {
            self.replan();
        }
        let live: Vec<&Note> = self.review.live(here).collect();
        if live.is_empty() {
            return (Vec::new(), String::new());
        }
        let shown = live
            .iter()
            .filter(|n| n.state == NoteState::Open)
            .map(|n| n.said())
            .collect();
        let mut told = NOTES_BRIEF.to_string();
        let mut earlier = false;
        for n in &live {
            if n.state == NoteState::Sent {
                earlier = true;
                continue;
            }
            told.push_str(&format!("\n{}. {}", n.number, self.describe_note(n)));
        }
        if earlier {
            told.push_str("\nStill open from before:");
            for n in live.iter().filter(|n| n.state == NoteState::Sent) {
                told.push_str(&format!("\n{}. {}", n.number, self.describe_note(n)));
            }
        }
        (shown, told)
    }

    /// The notes to mark on a preview: the ones still open here.
    pub(crate) fn preview_marks(&self) -> Vec<Mark> {
        self.review
            .live(self.session.rec)
            .map(|n| {
                let (at, to) = n.place(&self.doc);
                Mark {
                    number: n.number,
                    at,
                    to,
                }
            })
            .collect()
    }

    /// Turn pinning notes on or off.
    pub(crate) fn set_annotating(&mut self, on: bool) {
        self.review.annotating = on;
        self.review.press = None;
        self.review.selected = None;
        self.review.watch = on.then(|| (self.tool, doc_key(&self.doc), self.doc.rev()));
        if on {
            self.close_compare();
        } else {
            self.finish_note();
            // (The eye that hides the markers goes with the bar.)
            self.review.hidden = false;
        }
    }

    /// Close the note being written (one with nothing in it stays a pin).
    fn finish_note(&mut self) {
        self.review.editing = None;
    }

    /// Pin a note at `at` (a box to `to`) and start writing it.
    pub(crate) fn add_note(&mut self, at: P, to: Option<P>) {
        let tab = self.session.rec;
        // (A pin goes with the object under it.)
        let on = if to.is_none() {
            self.pick(at, 10.0 * 6.0 / self.view.scale as f64)
                .and_then(|i| self.doc.shapes.get(i))
                .map(|s| (s.uid, shape_bounds(s).center()))
        } else {
            None
        };
        self.review.next_id += 1;
        let id = self.review.next_id;
        let number = self.review.next_number(tab);
        self.review.notes.push(Note {
            id,
            number,
            tab,
            at,
            to,
            on,
            text: String::new(),
            state: NoteState::Open,
            reply: None,
        });
        self.review.editing = Some(id);
        self.review.focus = true;
        // (A new one is to be seen: so are the rest.)
        self.review.hidden = false;
    }

    /// Where a note's marker points, on screen: the pin, or a box's corner.
    fn note_tip(&self, rect: Rect, n: &Note) -> Pos2 {
        let (a, b) = n.place(&self.doc);
        self.view
            .to_screen(rect, b.map_or(a, |b| P::new(a.x.min(b.x), a.y.min(b.y))))
    }

    /// The note whose marker is under screen point `s`.
    fn note_at(&self, rect: Rect, s: Pos2) -> Option<u32> {
        self.review
            .live(self.session.rec)
            .filter(|n| {
                // (Hidden markers aren't there to be clicked.)
                !self.review.hidden
                    && bubble(self.note_tip(rect, n), BUBBLE)
                        .expand(2.0)
                        .contains(s)
            })
            .map(|n| n.id)
            .last()
    }

    /// The canvas while pinning notes: a click pins one (on an existing
    /// pin, opens it), a drag boxes an area.
    pub(crate) fn annotate_canvas(
        &mut self,
        ctx: &egui::Context,
        resp: &egui::Response,
        painter: &egui::Painter,
        rect: Rect,
    ) {
        // Moving the view, as with any tool: a middle or right drag, or
        // Space and a drag (which pins nothing).
        let space = !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_down(Key::Space));
        let pan = resp.dragged_by(egui::PointerButton::Middle)
            || resp.dragged_by(egui::PointerButton::Secondary)
            || (space
                && self.review.press.is_none()
                && resp.dragged_by(egui::PointerButton::Primary));
        if pan {
            let d = resp.drag_delta();
            self.view.center =
                self.view.center - P::new(d.x as f64, d.y as f64) * (10.0 / self.view.scale as f64);
            if space {
                // (Space was for moving: letting go of it plays nothing.)
                self.space_pan = Some(true);
            }
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            return;
        }
        ctx.set_cursor_icon(if space {
            egui::CursorIcon::Grab
        } else {
            egui::CursorIcon::Crosshair
        });
        let tol = 10.0 * 6.0 / self.view.scale as f64;
        // What a pin here would go on.
        if self.review.press.is_none()
            && let Some(h) = resp.hover_pos()
            && self.note_at(rect, h).is_none()
            && let Some(i) = self.pick(self.view.to_design(rect, h), tol)
        {
            canvas::draw_outline(
                painter,
                rect,
                &self.view,
                &self.doc.shapes[i],
                Stroke::new(1.2, note_color(ctx).gamma_multiply(0.8)),
            );
        }
        if resp.drag_started_by(egui::PointerButton::Primary) && !space {
            let start = ctx
                .input(|i| i.pointer.press_origin())
                .or(resp.interact_pointer_pos())
                .unwrap_or(rect.center());
            self.review.press = Some(self.view.to_design(rect, start));
        }
        if let (Some(a), Some(pos)) = (self.review.press, resp.interact_pointer_pos())
            && resp.dragged()
        {
            let r = Rect::from_two_pos(self.view.to_screen(rect, a), pos);
            painter.rect_filled(r, 2.0, note_color(ctx).gamma_multiply(0.08));
            box_outline(painter, r, 1.0);
        }
        if resp.drag_stopped()
            && let Some(a) = self.review.press.take()
        {
            let pos = resp.interact_pointer_pos().unwrap_or(rect.center());
            let b = self.view.to_design(rect, pos);
            if self.view.to_screen(rect, a).distance(pos) > 10.0 {
                let lo = P::new(a.x.min(b.x), a.y.min(b.y));
                let hi = P::new(a.x.max(b.x), a.y.max(b.y));
                self.add_note(lo, Some(hi));
            } else {
                self.add_note(a, None);
            }
        } else if resp.clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            match self.note_at(rect, pos) {
                // A marker: selected; clicked again, its comment opens.
                Some(id) if self.review.selected == Some(id) => {
                    self.review.editing = Some(id);
                    self.review.focus = true;
                }
                Some(id) => {
                    self.review.selected = Some(id);
                    self.review.editing = None;
                }
                None => {
                    self.review.selected = None;
                    self.add_note(self.view.to_design(rect, pos), None);
                }
            }
        }
    }

    /// The notes on the canvas: a marker each (a box round an area has its
    /// marker at the corner); what one says, while the pointer is on it.
    pub(crate) fn draw_notes(&self, painter: &egui::Painter, rect: Rect, hover: Option<Pos2>) {
        if self.review.hidden {
            return;
        }
        let here = self.session.rec;
        let running = self.agent.running();
        let time = painter.ctx().input(|i| i.time);
        let color = note_color(painter.ctx());
        let mut drawn = Vec::new();
        for n in self.review.live(here) {
            let (a, b) = n.place(&self.doc);
            // Sent and being worked on: a gentle pulse.
            let alpha = match n.state {
                NoteState::Sent if running => 0.7 + 0.3 * (time * 3.0).sin().abs() as f32,
                NoteState::Sent => 0.8,
                _ => 1.0,
            };
            if let Some(b) = b {
                let r =
                    Rect::from_two_pos(self.view.to_screen(rect, a), self.view.to_screen(rect, b));
                painter.rect_filled(r, 2.0, color.gamma_multiply(0.07 * alpha));
                box_outline(painter, r, alpha);
            }
            let tip = self.note_tip(rect, n);
            if self.review.selected == Some(n.id) {
                // Selected: a ring round its marker.
                painter.rect_stroke(
                    bubble(tip, BUBBLE).expand(3.0),
                    egui::CornerRadius {
                        nw: 14,
                        ne: 14,
                        sw: 3,
                        se: 14,
                    },
                    Stroke::new(2.0, color),
                    egui::StrokeKind::Outside,
                );
            }
            let head = paint_bubble(painter, tip, BUBBLE, alpha);
            drawn.push((n, head));
        }
        if running && drawn.iter().any(|(n, _)| n.state == NoteState::Sent) {
            painter
                .ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        // What the note under the pointer says (not the one being written).
        let Some(h) = hover else { return };
        let Some((n, head)) = drawn
            .iter()
            .rev()
            .find(|(n, r)| r.expand(2.0).contains(h) && self.review.editing != Some(n.id))
        else {
            return;
        };
        let p = crate::theme::pal(painter.ctx());
        let mut text = n.said();
        if let Some(r) = &n.reply {
            text.push_str(&format!("\n{r}"));
        }
        let g = painter.layout(
            text,
            egui::FontId::proportional(12.0),
            Color32::WHITE,
            260.0,
        );
        let r = Rect::from_min_size(
            head.right_top() + Vec2::new(8.0, 0.0),
            g.size() + Vec2::new(16.0, 10.0),
        );
        painter.rect_filled(r, 6.0, p.hud);
        painter.galley(r.min + Vec2::new(8.0, 5.0), g, Color32::WHITE);
    }

    /// The annotation bar, over the canvas while pinning comments: its grip,
    /// how many there are, frame them, hide them, see the last change side
    /// by side, delete one (or all), send them, and stop.
    pub(crate) fn annotation_bar(&mut self, ctx: &egui::Context) {
        if !self.review.annotating || self.review.showing.is_some() {
            return;
        }
        let here = self.session.rec;
        let canvas = self.canvas_rect;
        let pending = self.review.pending(here);
        let live = self.review.live(here).count();
        let compare = self.latest_comparison();
        let selected = self.review.selected;
        let hidden = self.review.hidden;
        let width = 330.0;
        let at = egui::pos2(canvas.center().x - width / 2.0, canvas.top() + 12.0)
            + self.review.bar_offset;
        enum Do {
            Frame,
            Hide,
            Compare(u64),
            Delete,
            Send,
            Close,
        }
        let mut act = None;
        let mut moved = Vec2::ZERO;
        egui::Area::new(egui::Id::unique("annotation-bar"))
            .order(egui::Order::Middle)
            .fixed_pos(at)
            .constrain_to(canvas)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .corner_radius(9)
                    .inner_margin(egui::Margin::same(space::XS as i8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let p = pal_ui(ui);
                            ui.spacing_mut().item_spacing.x = space::XS;
                            moved = crate::taskbar::grip_for(ui, "Move Annotation Bar");
                            ui.add_space(space::XS);
                            ui.label(
                                egui::RichText::new(format!("Annotating · {pending}"))
                                    .size(13.0)
                                    .color(p.text),
                            );
                            ui.add_space(space::S);
                            if bar_button(ui, Icon::ZoomFit, "Frame Comments", live > 0).clicked() {
                                act = Some(Do::Frame);
                            }
                            let (icon, label) = if hidden {
                                (Icon::Eye, "Show Comments")
                            } else {
                                (Icon::EyeOff, "Hide Comments")
                            };
                            if bar_button(ui, icon, label, live > 0 || hidden).clicked() {
                                act = Some(Do::Hide);
                            }
                            if bar_button(ui, Icon::Compare, "Side by Side", compare.is_some())
                                .clicked()
                                && let Some(id) = compare
                            {
                                act = Some(Do::Compare(id));
                            }
                            let delete = if selected.is_some() {
                                "Delete Comment"
                            } else {
                                "Delete All Comments"
                            };
                            if bar_button(
                                ui,
                                Icon::Trash,
                                delete,
                                pending > 0 || selected.is_some(),
                            )
                            .clicked()
                            {
                                act = Some(Do::Delete);
                            }
                            ui.add_space(space::XS);
                            if send_button(ui, pending > 0, false).clicked() {
                                act = Some(Do::Send);
                            }
                            if bar_button(ui, Icon::Close, "Stop Annotating", true).clicked() {
                                act = Some(Do::Close);
                            }
                        });
                    });
            });
        self.review.bar_offset += moved;
        match act {
            Some(Do::Frame) => self.frame_notes(),
            Some(Do::Hide) => self.review.hidden = !hidden,
            Some(Do::Compare(id)) => self.review.open(Showing::Compare(id)),
            Some(Do::Delete) => match selected {
                Some(id) => self.notes_action(NotesAction::Remove(id)),
                None => self
                    .review
                    .notes
                    .retain(|n| !(n.tab == here && n.state == NoteState::Open)),
            },
            Some(Do::Send) => self.send_notes(ctx),
            Some(Do::Close) => self.set_annotating(false),
            None => {}
        }
    }

    /// The agent changed the design: that doesn't end pinning comments.
    pub(crate) fn agent_changed(&mut self) {
        if let Some((_, key, rev)) = &mut self.review.watch {
            *key = doc_key(&self.doc);
            *rev = self.doc.rev();
        }
    }

    /// At the end of a frame: another tool chosen, or the design changed by
    /// the person (not the agent), and pinning comments is over.
    /// `edited`: a setting was changed in place this frame (see
    /// `Studio::ui`); anything else that changes the design gives the
    /// document a new revision. Only then is the design looked over.
    pub(crate) fn watch_annotating(&mut self, edited: bool) {
        let Some((tool, key, rev)) = self.review.watch else {
            return;
        };
        if !self.review.annotating {
            self.review.watch = None;
        } else if self.tool != tool {
            self.set_annotating(false);
        } else if edited || self.doc.rev() != rev {
            if doc_key(&self.doc) != key {
                self.set_annotating(false);
            } else if let Some(w) = &mut self.review.watch {
                w.2 = self.doc.rev();
            }
        }
    }

    /// Fit the view round the comments still to be dealt with.
    fn frame_notes(&mut self) {
        let mut b = zigzag::geom::Bounds::empty();
        for n in self.review.live(self.session.rec) {
            let (a, to) = n.place(&self.doc);
            b.add(a);
            if let Some(t) = to {
                b.add(t);
            }
        }
        if !b.is_empty() {
            let canvas = self.canvas_rect;
            self.view.fit(
                canvas,
                b.center(),
                (b.width() / 10.0).max(20.0),
                (b.height() / 10.0).max(20.0),
            );
        }
    }

    /// The latest change on this document to set side by side: its
    /// versions, or else its before and after.
    fn latest_comparison(&self) -> Option<u64> {
        let here = self.session.rec;
        let mine = |t: &&Turn| t.tab == here;
        self.review
            .turns
            .iter()
            .rev()
            .filter(mine)
            .find(|t| !t.variants.is_empty() || t.collecting)
            .or_else(|| {
                self.review
                    .turns
                    .iter()
                    .rev()
                    .filter(mine)
                    .find(|t| t.after.is_some())
            })
            .map(|t| t.id)
    }

    /// The note being written: a comment box beside its marker, with a
    /// send button. Sending gives it to the agent there and then (or, while
    /// it's busy, when it's done).
    pub(crate) fn note_editor(&mut self, ctx: &egui::Context) {
        let Some(id) = self.review.editing else {
            return;
        };
        if self.review.showing.is_some() {
            return;
        }
        let rect = self.canvas_rect;
        let Some(k) = self.review.notes.iter().position(|n| n.id == id) else {
            self.review.editing = None;
            return;
        };
        let head = bubble(self.note_tip(rect, &self.review.notes[k]), BUBBLE);
        let focus = std::mem::take(&mut self.review.focus);
        let (mut send, mut esc) = (false, false);
        let pal = crate::theme::pal(ctx);
        let text = &mut self.review.notes[k].text;
        egui::Area::new(egui::Id::unique("note-editor"))
            .order(egui::Order::Foreground)
            .fixed_pos(head.right_top() + Vec2::new(8.0, -(BUTTON + 2.0 * space::S - BUBBLE) / 2.0))
            // (On the canvas with its marker, never over the docks.)
            .constrain_to(rect)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(pal.dock)
                    .corner_radius(BUTTON as u8 / 2 + space::S as u8)
                    .shadow(ui.style().visuals.popup_shadow)
                    .inner_margin(egui::Margin {
                        left: (space::S + space::XS) as i8,
                        right: space::S as i8,
                        top: space::S as i8,
                        bottom: space::S as i8,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = space::S;
                            // (Narrower on a narrow canvas, beside its button.)
                            let w = (crate::theme::canvas_bar_width(rect)
                                - BUTTON
                                - 3.0 * space::S
                                - space::XS)
                                .clamp(120.0, 220.0);
                            let r = ui.add(
                                egui::TextEdit::multiline(text)
                                    .id(egui::Id::unique("note-text"))
                                    .frame(egui::Frame::NONE)
                                    .margin(egui::Margin::ZERO)
                                    .font(egui::FontId::proportional(FIELD_PT))
                                    .hint_text(
                                        egui::RichText::new("Add a comment…")
                                            .color(hint_color(pal)),
                                    )
                                    .desired_rows(1)
                                    .desired_width(w)
                                    .min_size(Vec2::new(w, BUTTON))
                                    .align(egui::Align2::LEFT_CENTER)
                                    // (Enter sends, below; Shift-Enter is
                                    // the field's own new line, at the caret.)
                                    .return_key(egui::KeyboardShortcut::new(
                                        egui::Modifiers::SHIFT,
                                        Key::Enter,
                                    )),
                            );
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::Role::TextInput, true, "Comment")
                            });
                            if focus {
                                r.request_focus();
                            }
                            let (enter, shift) =
                                ui.input(|i| (i.key_pressed(Key::Enter), i.modifiers.shift));
                            if r.has_focus() && enter && !shift {
                                send = true;
                            }
                            if ui.input(|i| i.key_pressed(Key::Escape)) {
                                esc = true;
                            }
                            let ok = !text.trim().is_empty();
                            ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                                if send_button(ui, ok, false).clicked() {
                                    send = true;
                                }
                            });
                        });
                    });
            });
        let empty = self.review.notes[k].text.trim().is_empty();
        if esc {
            // Escape on a comment with nothing in it takes the marker away.
            if empty {
                self.review.notes.remove(k);
            }
            self.review.editing = None;
        } else if send && !empty {
            self.review.editing = None;
            self.send_notes(ctx);
        }
    }

    /// Give the open notes to the agent now (the prompt being typed stays
    /// for later), or when it's done with what it's doing.
    fn send_notes(&mut self, ctx: &egui::Context) {
        if self.agent.running() {
            self.review.queued = Some(self.session.rec);
            return;
        }
        let typed = std::mem::take(&mut self.agent.prompt);
        self.agent_send(ctx);
        self.agent.prompt = typed;
    }

    /// A note sent while the agent was busy goes once it's done.
    pub(crate) fn send_queued_notes(&mut self, ctx: &egui::Context) {
        if self.review.queued == Some(self.session.rec) && !self.agent.running() {
            self.review.queued = None;
            if self.review.pending(self.session.rec) > 0 {
                self.send_notes(ctx);
            }
        }
    }

    /// The comments on the canvas, in the prompt box like an attachment:
    /// one chip saying how many, which opens into the list (each one's
    /// text, with a button to take it away or, once sent, to resolve it).
    pub(crate) fn notes_list(&mut self, ui: &mut egui::Ui) -> Option<NotesAction> {
        let here = self.session.rec;
        let p = pal_ui(ui);
        let live: Vec<&Note> = self.review.live(here).collect();
        if live.is_empty() {
            return None;
        }
        let mut out = None;
        let open = self.review.notes_open;
        let mut fold = false;
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: space::S as i8,
                right: 0,
                top: space::S as i8,
                bottom: 0,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = space::XS;
                // The chip.
                let n = live.len();
                let label = if n == 1 {
                    "1 comment".to_string()
                } else {
                    format!("{n} comments")
                };
                let g = ui.painter().layout_no_wrap(
                    label.clone(),
                    egui::FontId::proportional(PANEL_PT),
                    p.secondary,
                );
                let size = Vec2::new(
                    10.0 + 12.0 + space::S + g.size().x + space::S + 10.0 + 10.0,
                    BUTTON,
                );
                let (r, chip) = ui.allocate_exact_size(size, Sense::click());
                ui.painter().rect(
                    r,
                    8.0,
                    if chip.hovered() { p.hover } else { p.control },
                    Stroke::new(1.0, p.control_border),
                    egui::StrokeKind::Inside,
                );
                let x = r.left() + 10.0;
                paint_bubble(ui.painter(), egui::pos2(x, r.center().y + 6.0), 12.0, 1.0);
                let x = x + 12.0 + space::S;
                ui.painter().galley(
                    egui::pos2(x, r.center().y - g.size().y / 2.0),
                    g.clone(),
                    p.text,
                );
                icons::paint(
                    ui.painter(),
                    Rect::from_center_size(
                        egui::pos2(r.right() - 15.0, r.center().y),
                        Vec2::splat(10.0),
                    ),
                    if open {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    p.secondary,
                );
                chip.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
                if chip
                    .on_hover_text(if open {
                        "Hide the comments"
                    } else {
                        "Show the comments"
                    })
                    .clicked()
                {
                    fold = true;
                }
                if !open {
                    return;
                }
                ui.add_space(space::XS);
                // Each: its marker, what it says, and a button to take it
                // away (or, once sent, to resolve it), all on the first line.
                let line = 18.0;
                // The list: a box of its own color, a line between each.
                egui::Frame::new()
                    .fill(p.control)
                    .corner_radius(8)
                    .inner_margin(egui::Margin {
                        left: space::S as i8 + 2,
                        right: 0,
                        top: space::XS as i8,
                        bottom: space::XS as i8,
                    })
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (k, n) in live.iter().enumerate() {
                            if k > 0 {
                                let (r, _) = ui.allocate_exact_size(
                                    Vec2::new(ui.available_width(), 1.0),
                                    Sense::hover(),
                                );
                                ui.painter().hline(
                                    r.left()..=r.right() - space::S,
                                    r.center().y,
                                    Stroke::new(1.0, p.separator),
                                );
                            }
                            ui.add_space(space::XS);
                            note_row(ui, n, line, &mut out);
                            ui.add_space(space::XS);
                        }
                    });
            });
        if fold {
            self.review.notes_open = !open;
        }
        out
    }

    pub(crate) fn notes_action(&mut self, x: NotesAction) {
        match x {
            NotesAction::Edit(id) => {
                self.review.editing = Some(id);
                self.review.focus = true;
                self.close_compare();
            }
            NotesAction::Remove(id) => {
                self.review.notes.retain(|n| n.id != id);
                if self.review.selected == Some(id) {
                    self.review.selected = None;
                }
                if self.review.editing == Some(id) {
                    self.review.editing = None;
                }
            }
            NotesAction::Resolve(id) => {
                if let Some(n) = self.review.notes.iter_mut().find(|n| n.id == id) {
                    n.state = NoteState::Resolved;
                }
            }
        }
    }

    pub(crate) fn turn_action(&mut self, x: TurnAction) {
        match x {
            TurnAction::Before(id) => self.review.open(Showing::Before(id)),
            TurnAction::After => self.close_compare(),
            TurnAction::Compare(id) => self.review.open(Showing::Compare(id)),
            TurnAction::Close => self.close_compare(),
            TurnAction::Revert(id) => {
                let Some(t) = self.review.turn(id) else {
                    return;
                };
                let (doc, prompt) = (t.before.doc.clone(), t.prompt.clone());
                self.restore(doc);
                self.review.showing = None;
                self.status = if prompt.is_empty() {
                    "Put back as it was. Undo brings the changes back.".into()
                } else {
                    format!("Put back as it was before “{prompt}”. Undo brings the changes back.")
                };
            }
            TurnAction::Use { turn, pane } => {
                let Some(t) = self.review.turn(turn) else {
                    return;
                };
                let v = match pane {
                    Pane::Before => Some(&t.before),
                    Pane::After => t.after.as_ref(),
                    Pane::Variant(k) => t.variants.get(k),
                    Pane::Live => None,
                };
                if let Some(v) = v {
                    let label = v.label.clone();
                    if v.key != doc_key(&self.doc) {
                        let doc = v.doc.clone();
                        self.restore(doc);
                    }
                    self.status = match pane {
                        Pane::Variant(k) => format!("Using version {} ({label}).", letter(k)),
                        _ => format!("Using the {}.", label.to_lowercase()),
                    };
                    self.review.chose = Some((
                        self.session.rec,
                        match pane {
                            Pane::Variant(k) => format!("version {} ({label})", letter(k)),
                            _ => format!("the {}", label.to_lowercase()),
                        },
                    ));
                }
                // The person has chosen: the round is over, whoever's it
                // was (an agent still finishing mustn't put the original
                // back over the choice).
                if let Some(t) = self.review.turn_mut(turn) {
                    t.collecting = false;
                }
                self.close_compare();
            }
        }
    }

    /// Put the comparison away (an outside agent's round of versions ends).
    fn close_compare(&mut self) {
        if let Some(Showing::Compare(id)) = self.review.showing
            && let Some(t) = self.review.turn_mut(id)
            && t.outside
        {
            t.collecting = false;
        }
        self.review.showing = None;
    }

    /// Keys for reviewing: Escape puts away what covers the canvas (or
    /// stops pinning notes), N pins notes. True if one was taken.
    pub(crate) fn review_keys(&mut self, ctx: &egui::Context) -> bool {
        if ctx.egui_wants_keyboard_input() {
            return false;
        }
        if self.review.showing.is_some() {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
                self.close_compare();
            }
            // (Nothing else acts on a document that isn't on show.)
            return true;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::N)) {
            self.set_annotating(!self.review.annotating);
            return true;
        }
        if let Some(id) = self.review.selected
            && ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, Key::Delete)
                    || i.consume_key(egui::Modifiers::NONE, Key::Backspace)
            })
        {
            self.notes_action(NotesAction::Remove(id));
            return true;
        }
        if self.review.selected.is_some()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
        {
            self.review.selected = None;
            return true;
        }
        if (self.review.annotating || self.review.editing.is_some())
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
        {
            // A note being written closes first (one with nothing in it
            // goes); then pinning stops.
            match self.review.editing.take() {
                Some(id) => self
                    .review
                    .notes
                    .retain(|n| n.id != id || !n.text.trim().is_empty()),
                None => self.set_annotating(false),
            }
            return true;
        }
        false
    }

    /// The canvas while a turn's "before", or versions side by side, are
    /// on show.
    pub(crate) fn review_canvas(&mut self, ui: &mut egui::Ui) {
        let Some(showing) = self.review.showing else {
            return;
        };
        let (Showing::Before(id) | Showing::Compare(id)) = showing;
        if self.review.turn(id).is_none() {
            self.review.showing = None;
        }
        let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = resp.rect;
        self.canvas_rect = rect;
        let ctx = ui.ctx().clone();
        let pal = crate::theme::pal(&ctx);
        painter.rect_filled(rect, 0.0, pal.desk);
        let mut act: Option<TurnAction> = None;
        match showing {
            Showing::Before(_) => {
                // The design before, where the document was: flip between.
                self.navigate(&ctx, &resp, &[rect], false);
                let Some(t) = self.review.turn(id) else {
                    return;
                };
                self.draw_pane(
                    &painter,
                    rect,
                    &self.view,
                    &t.before.design,
                    2,
                    t.before.generation,
                );
            }
            Showing::Compare(_) => {
                act = self.compare_panes(ui, &ctx, &resp, &painter, rect, id);
            }
        }
        if let Some(x) = act {
            self.turn_action(x);
        }
    }

    /// Pan and zoom: on the document's view (the "before"), or the
    /// comparison's, with the pointer over one of `panes`.
    fn navigate(
        &mut self,
        ctx: &egui::Context,
        resp: &egui::Response,
        panes: &[Rect],
        shared: bool,
    ) {
        if let Some(hover) = resp.hover_pos() {
            let (scroll, zoom) = ctx.input(|i| {
                for e in &i.events {
                    if let egui::Event::MouseWheel { unit, .. } = e {
                        self.wheel = !matches!(unit, egui::MouseWheelUnit::Point);
                    }
                }
                (i.smooth_scroll_delta, i.zoom_delta())
            });
            let factor = if self.wheel {
                zoom * (scroll.y * 0.0015).exp()
            } else {
                zoom
            };
            let at = panes.iter().copied().find(|r| r.contains(hover));
            let view = if shared {
                &mut self.review.view
            } else {
                &mut self.view
            };
            if let Some(r) = at
                && (factor - 1.0).abs() > 1e-4
            {
                view.zoom_at(r, hover, factor);
            }
            if !self.wheel && scroll != Vec2::ZERO {
                view.center = view.center
                    - P::new(scroll.x as f64, scroll.y as f64) * (10.0 / view.scale as f64);
            }
        }
        if resp.dragged() {
            let d = resp.drag_delta();
            let view = if shared {
                &mut self.review.view
            } else {
                &mut self.view
            };
            view.center = view.center - P::new(d.x as f64, d.y as f64) * (10.0 / view.scale as f64);
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }
    }

    /// One pane: the hoop and a version's stitches, through `view`.
    fn draw_pane(
        &self,
        painter: &egui::Painter,
        body: Rect,
        view: &View,
        d: &Design,
        slot: usize,
        generation: u64,
    ) {
        let p = painter.with_clip_rect(body.intersect(painter.clip_rect()));
        let (hw, hh) = self.hoop_dims();
        let cloth = self.doc.settings.cloth;
        canvas::draw_hoop(&p, body, view, hw, hh, "", false, color32(cloth));
        let opts = DrawOptions {
            show_jumps: self.prefs.show_jumps,
            limit: usize::MAX,
            thread_mm: 0.4,
            view: self.view_tools.stitch_view,
            background: cloth,
            hidden: None,
            moving: None,
            copy: false,
        };
        self.paint_stitches_in(&p, body, view, d, &opts, slot, generation);
    }

    /// A turn's versions side by side: the original first, then each the
    /// agent kept (and the one it's making), all at the same size, split
    /// by thin lines and nothing else. Hovering one outlines it and shows
    /// what it is, with a button to use it.
    fn compare_panes(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        resp: &egui::Response,
        painter: &egui::Painter,
        rect: Rect,
        id: u64,
    ) -> Option<TurnAction> {
        let pal = crate::theme::pal(ctx);
        let changed = self.changed_since(id);
        let t = self.review.turn(id)?;
        let variants = !t.variants.is_empty() || t.collecting;
        let mut panes = vec![Pane::Before];
        if variants {
            panes.extend((0..t.variants.len()).map(Pane::Variant));
        } else if t.after.is_some() {
            panes.push(Pane::After);
        }
        // The one being made: once the design is something new.
        let live = changed && (t.collecting || (!variants && t.end.is_none()));
        if live {
            panes.push(Pane::Live);
        }
        // Everything they show, to fit them all.
        let mut b = zigzag::geom::Bounds::empty();
        let mut add = |x: zigzag::geom::Bounds| {
            if !x.is_empty() {
                b.add(x.min);
                b.add(x.max);
            }
        };
        add(t.before.bounds);
        t.variants.iter().for_each(|v| add(v.bounds));
        if let Some(a) = &t.after {
            add(a.bounds);
        }
        if live {
            add(self.design.bounds());
        }
        let aspect = if b.is_empty() {
            1.0
        } else {
            (b.width() / b.height().max(1.0)) as f32
        };
        let cells = grid(rect, panes.len(), aspect, 0.0, 0.0);
        if let Some(cell) = cells.first()
            && self.review.fitted != Some(cell.size())
        {
            let (c, w, h) = if b.is_empty() {
                let (hw, hh) = self.hoop_dims();
                (P::default(), hw, hh)
            } else {
                (
                    b.center(),
                    (b.width() / 10.0).max(20.0),
                    (b.height() / 10.0).max(20.0),
                )
            };
            self.review.view.fit(*cell, c, w, h);
            self.review.fitted = Some(cell.size());
        }
        self.navigate(ctx, resp, &cells, true);
        self.review.panes = panes.iter().copied().zip(cells.iter().copied()).collect();
        let view = self.review.view;
        let t = self.review.turn(id)?;
        let mut act = None;
        // (The pointer's own position, not the canvas's hover: over the
        // pill's button the canvas isn't hovered, and the pill would go.)
        let pointer = ctx
            .input(|i| i.pointer.hover_pos())
            .filter(|p| rect.contains(*p));
        let time = ctx.input(|i| i.time);
        let original = &t.before.design;
        for (&pane, &cell) in panes.iter().zip(&cells) {
            let (name, d, generation, slot): (String, &Design, u64, usize) = match pane {
                Pane::Before => (
                    if variants { "Original" } else { "Before" }.into(),
                    &t.before.design,
                    t.before.generation,
                    2,
                ),
                Pane::After => {
                    let a = t.after.as_ref().unwrap_or(&t.before);
                    ("After".into(), &a.design, a.generation, 3)
                }
                Pane::Variant(i) => {
                    let v = &t.variants[i];
                    (
                        format!("{} · {}", letter(i), v.label),
                        &v.design,
                        v.generation,
                        3 + i,
                    )
                }
                Pane::Live => (
                    if variants {
                        format!("Making {}…", letter(t.variants.len()))
                    } else {
                        "Being made…".into()
                    },
                    &self.design,
                    self.replans,
                    0,
                ),
            };
            self.draw_pane(painter, cell, &view, d, slot, generation);
            // A thin line between it and the next (none at the canvas's edges).
            let line = Stroke::new(1.0, pal.separator);
            if cell.right() < rect.right() - 1.0 {
                painter.vline(cell.right(), cell.y_range(), line);
            }
            if cell.bottom() < rect.bottom() - 1.0 {
                painter.hline(cell.x_range(), cell.bottom(), line);
            }
            if pane == Pane::Live {
                crate::agent::spinner(
                    painter,
                    cell.right_top() + Vec2::new(-16.0, 16.0),
                    5.0,
                    pal.accent,
                    time,
                );
            }
            if !pointer.is_some_and(|p| cell.contains(p)) {
                continue;
            }
            // Hovered: outlined, and at its top, what it is and Use This.
            painter.rect_stroke(
                cell.shrink(1.0),
                0.0,
                Stroke::new(2.0, pal.accent),
                egui::StrokeKind::Inside,
            );
            let button = match (pane, variants) {
                (Pane::Before, true) => Some(("Keep Original", TurnAction::Use { turn: id, pane })),
                (Pane::Before, false) => Some(("Revert to This", TurnAction::Revert(id))),
                (Pane::After, _) => Some(("Keep This", TurnAction::Close)),
                (Pane::Variant(_), _) => Some(("Use This", TurnAction::Use { turn: id, pane })),
                (Pane::Live, _) => None,
            };
            let facts = pane_facts(d, (pane != Pane::Before).then_some(original));
            let mut h = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(cell.shrink(12.0))
                    .layout(egui::Layout::top_down(egui::Align::Center)),
            );
            egui::Frame::new()
                .fill(pal.hud)
                .corner_radius(16)
                .inner_margin(egui::Margin {
                    left: 12,
                    right: 4,
                    top: 4,
                    bottom: 4,
                })
                .show(&mut h, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = space::S;
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&name).size(12.0).color(Color32::WHITE),
                            )
                            .truncate(),
                        )
                        .on_hover_text(&facts);
                        if let Some((label, a)) = button {
                            let b = egui::Button::new(
                                egui::RichText::new(label).size(11.5).color(Color32::WHITE),
                            )
                            .fill(pal.accent)
                            .corner_radius(12);
                            if ui.add(b).on_hover_text(&facts).clicked() {
                                act = Some(a);
                            }
                        } else {
                            ui.add_space(8.0);
                        }
                    });
                });
        }
        if t.collecting && panes.len() == 1 {
            self.hud(
                painter,
                rect,
                "The agent's versions show here as it makes them.",
            );
        }
        act
    }

    /// Whether the document is no longer turn `id`'s original.
    fn changed_since(&self, id: u64) -> bool {
        let Some(t) = self.review.turn(id) else {
            return false;
        };
        if let Some((tid, plan, c)) = self.review.changed.get()
            && tid == id
            && plan == self.replans
        {
            return c;
        }
        let c = doc_key(&self.doc) != t.before.key;
        self.review.changed.set(Some((id, self.replans, c)));
        c
    }

    /// `get_annotations`: the notes on the canvas still to be dealt with.
    pub(crate) fn tool_get_annotations(&mut self) -> Result<ToolResult, String> {
        let here = self.session.rec;
        let notes: Vec<Note> = self.review.live(here).cloned().collect();
        let list: Vec<Value> = notes
            .iter()
            .map(|n| {
                let (a, b) = n.place(&self.doc);
                json!({
                    "number": n.number,
                    "note": n.text.trim(),
                    "state": if n.state == NoteState::Open { "new" } else { "sent" },
                    "x_mm": (a.x * 10.0).round() / 100.0,
                    "y_mm": (a.y * 10.0).round() / 100.0,
                    "box_mm": b.map(|b| json!([a.x / 10.0, a.y / 10.0, b.x / 10.0, b.y / 10.0])),
                    "objects": self.note_objects(n),
                })
            })
            .collect();
        Ok(if list.is_empty() {
            ToolResult::text("No notes on the canvas.")
        } else {
            ToolResult::json(&json!({ "notes": list, "how": NOTES_BRIEF }))
        })
    }

    /// `resolve_note`: the agent dealt with a note.
    pub(crate) fn tool_resolve_note(&mut self, args: &Value) -> Result<ToolResult, String> {
        // (The document being worked on: in front, or visited.)
        let here = self.session.rec;
        let number = args
            .get("number")
            .and_then(Value::as_u64)
            .ok_or("`number` must be the note's number")? as u32;
        let reply = args
            .get("reply")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        // (One that was sent before one that wasn't, should a number have
        // come round again.)
        let open =
            |n: &Note, state: NoteState| n.tab == here && n.number == number && n.state == state;
        let k = self
            .review
            .notes
            .iter()
            .position(|n| open(n, NoteState::Sent))
            .or_else(|| {
                self.review
                    .notes
                    .iter()
                    .position(|n| open(n, NoteState::Open))
            })
            .ok_or(format!("no open note {number}"))?;
        let n = &mut self.review.notes[k];
        n.state = NoteState::Resolved;
        n.reply = reply;
        Ok(ToolResult::text(format!("Resolved note {number}.")))
    }

    /// `start_variants`: the design as it is is the original that the
    /// versions to come are compared with.
    pub(crate) fn tool_start_variants(&mut self) -> Result<ToolResult, String> {
        let here = self.session.rec;
        if let Some(t) = self
            .review
            .turns
            .iter()
            .find(|t| t.collecting && t.tab == here)
        {
            return Ok(ToolResult::text(format!(
                "Already making versions ({} kept so far): make the next one and call save_variant.",
                t.variants.len()
            )));
        }
        let before = self.version("Original");
        // The Agent panel's prompt being worked on: that prompt's versions.
        if let Some(t) = self
            .review
            .turns
            .iter_mut()
            .rev()
            .find(|t| !t.outside && t.end.is_none() && t.tab == here)
        {
            t.before = before;
            t.collecting = true;
            let id = t.id;
            self.review.open(Showing::Compare(id));
            return Ok(ToolResult::text(START_VARIANTS));
        }
        self.review.next_turn += 1;
        let id = self.review.next_turn;
        self.review.turns.push(Turn {
            id,
            tab: here,
            prompt: String::new(),
            start: 0,
            end: None,
            before,
            after: None,
            variants: Vec::new(),
            collecting: true,
            outside: true,
            problems: Vec::new(),
        });
        self.review.prune();
        self.review.open(Showing::Compare(id));
        Ok(ToolResult::text(START_VARIANTS))
    }

    /// `show_comparison`: put the latest versions (or before and after)
    /// side by side on the canvas, or put them away.
    pub(crate) fn tool_show_comparison(&mut self, args: &Value) -> Result<ToolResult, String> {
        let show = args.get("show").and_then(Value::as_bool).unwrap_or(true);
        if !show {
            self.close_compare();
            return Ok(ToolResult::text("The canvas shows the design again."));
        }
        let id = self
            .latest_comparison()
            .ok_or("nothing to compare yet: make versions with start_variants and save_variant")?;
        let n = self.review.turn(id).map_or(0, |t| t.variants.len());
        self.review.open(Showing::Compare(id));
        Ok(ToolResult::text(if n > 0 {
            format!("Showing the original and {n} version(s) side by side.")
        } else {
            "Showing the design before and after side by side.".to_string()
        }))
    }

    /// `save_variant`: keep the design as it is as a version, and put the
    /// original back for the next.
    pub(crate) fn tool_save_variant(&mut self, args: &Value) -> Result<ToolResult, String> {
        let label = args
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("give the version a short `label` saying how it differs")?
            .to_string();
        let here = self.session.rec;
        let Some(k) = self
            .review
            .turns
            .iter()
            .rposition(|t| t.collecting && t.tab == here)
        else {
            return Err("No versions are being made: call start_variants first (it keeps the design as it is now as the original).".into());
        };
        if doc_key(&self.doc) == self.review.turns[k].before.key {
            return Err("This is the same as the original: change the design for this version first, then save it.".into());
        }
        let v = self.version(label.clone());
        let original = self.review.turns[k].before.doc.clone();
        let id = self.review.turns[k].id;
        self.review.turns[k].variants.push(v);
        let n = self.review.turns[k].variants.len();
        self.restore(original);
        if self.review.showing.is_none() {
            self.review.open(Showing::Compare(id));
        }
        Ok(ToolResult::text(format!(
            "Kept version {} ({label}); {n} so far. The design is back to the original for the next one.",
            letter(n - 1)
        )))
    }
}

/// A box round an area, dashed in the notes' color.
fn box_outline(painter: &egui::Painter, r: Rect, alpha: f32) {
    let color = note_color(painter.ctx());
    let path = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    painter.extend(egui::Shape::dashed_line(
        &path,
        Stroke::new(1.5, color.gamma_multiply(alpha)),
        6.0,
        4.0,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn panes_are_laid_out_to_show_the_design_biggest() {
        let r = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 600.0));
        // Two square designs on a wide canvas: side by side.
        let g = grid(r, 2, 1.0, 10.0, 0.0);
        assert!(
            g[0].top() == g[1].top() && g[0].right() < g[1].left(),
            "{g:?}"
        );
        // Four: a row of four beats two rows here.
        let g = grid(r, 4, 1.0, 10.0, 0.0);
        assert!(g.iter().all(|x| x.top() == g[0].top()), "{g:?}");
        // On a tall canvas, three stack.
        let tall = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 1200.0));
        let g = grid(tall, 3, 1.0, 10.0, 0.0);
        assert!(g[1].top() > g[0].bottom(), "{g:?}");
        // A last row that isn't full is centered.
        let g = grid(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 800.0)),
            3,
            1.0,
            10.0,
            0.0,
        );
        let mid = (g[2].left() + g[2].right()) / 2.0;
        assert!((mid - 400.0).abs() < 1.0, "{g:?}");
    }

    fn studio() -> egui_kittest::Harness<'static, Studio> {
        let path =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/badge.svg"));
        let mut h = egui_kittest::Harness::builder()
            .with_size(Vec2::new(1400.0, 900.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(Some(path), None),
            );
        h.state_mut().agent.found = [
            Some(Some(std::path::PathBuf::from("/usr/local/bin/claude"))),
            Some(Some(std::path::PathBuf::from("/usr/local/bin/codex"))),
            Some(Some(std::path::PathBuf::from("/usr/local/bin/agy"))),
        ];
        h.run_steps(4);
        h
    }

    fn uid_of(h: &egui_kittest::Harness<'_, Studio>, name: &str) -> usize {
        let i = h.state().names.iter().position(|n| n == name).unwrap();
        h.state().doc.shapes[i].uid
    }

    #[test]
    fn a_click_pins_a_note_on_what_is_there_and_a_drag_boxes_an_area() {
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().dock.reveal(crate::dock::PanelId::Agent);
        h.run_steps(5);
        h.get_by_label("Annotate").click();
        h.run_steps(3);
        assert!(h.state().review.annotating);
        // On the sun.
        let sun = uid_of(&h, "sun");
        let s = h.state();
        let at = s
            .doc
            .shapes
            .iter()
            .find(|x| x.uid == sun)
            .map(|x| shape_bounds(x).center())
            .unwrap();
        let spot = s.view.to_screen(s.canvas_rect, at);
        h.hover_at(spot);
        h.run_steps(1);
        h.drag_at(spot);
        h.run_steps(1);
        h.drop_at(spot);
        h.run_steps(3);
        assert_eq!(h.state().review.notes.len(), 1);
        assert_eq!(
            h.state().review.notes[0].on.map(|o| o.0),
            Some(sun),
            "on the sun"
        );
        assert!(h.state().review.editing.is_some(), "written at once");
        h.get_by_label("Comment").type_text("make the rays longer");
        h.key_press(Key::Enter);
        h.run_steps(3);
        assert!(h.state().review.editing.is_none());
        assert_eq!(h.state().review.notes[0].text, "make the rays longer");
        // Enter sent it to the agent (here, with no MCP server, it says so;
        // the comment stays to go with the next message).
        assert!(
            h.state()
                .agent
                .log
                .contains(&crate::agent::Entry::Notes(vec![
                    "make the rays longer".into()
                ])),
            "{:?}",
            h.state().agent.log
        );
        assert!(h.state().review.annotating, "still pinning");
        assert!(
            h.state().selected.is_none(),
            "the click didn't select anything"
        );
        // A drag: a box.
        let r = h.state().canvas_rect;
        let a = r.center() + Vec2::new(-200.0, -150.0);
        h.drag_at(a);
        h.run_steps(1);
        h.hover_at(a + Vec2::new(40.0, 40.0));
        h.run_steps(1);
        h.drop_at(a + Vec2::new(90.0, 70.0));
        h.run_steps(3);
        let n = &h.state().review.notes[1];
        assert!(n.to.is_some() && n.number == 2, "{n:?}");
        // Escape on an empty note takes it away again; Escape again stops.
        h.key_press(Key::Escape);
        h.run_steps(3);
        assert_eq!(h.state().review.notes.len(), 1);
        h.key_press(Key::Escape);
        h.run_steps(2);
        assert!(!h.state().review.annotating);
        // The pin went with the sun when it moved.
        let before = h.state().review.notes[0].place(&h.state().doc).0;
        h.state_mut().run_mcp_call(
            "transform_objects",
            &json!({ "objects": ["sun"], "move_by_mm": [5, 0] }),
        );
        let after = h.state().review.notes[0].place(&h.state().doc).0;
        assert!(
            (after.x - before.x - 50.0).abs() < 1.0,
            "{before:?} → {after:?}"
        );
    }

    #[test]
    fn the_annotation_bar_counts_deletes_and_stops_and_delete_takes_a_selected_comment() {
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().set_annotating(true);
        for k in 0..3 {
            h.state_mut()
                .add_note(P::new(-200.0 + k as f64 * 200.0, 0.0), None);
            h.state_mut().review.notes[k].text = format!("note {k}");
        }
        h.state_mut().review.editing = None;
        h.run_steps(3);
        h.get_by_label("Annotating · 3");
        // Click a marker: selected (no comment box), and Delete takes it.
        let (rect, id) = (h.state().canvas_rect, h.state().review.notes[1].id);
        let at = bubble(h.state().note_tip(rect, &h.state().review.notes[1]), BUBBLE).center();
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(3);
        assert_eq!(h.state().review.selected, Some(id));
        assert!(h.state().review.editing.is_none());
        assert_eq!(
            h.state().review.notes.len(),
            3,
            "a click on a marker adds none"
        );
        h.key_press(Key::Delete);
        h.run_steps(3);
        assert_eq!(h.state().review.notes.len(), 2);
        assert!(h.state().review.notes.iter().all(|n| n.id != id));
        assert!(
            h.state().selected.is_none(),
            "no object was deleted instead"
        );
        h.get_by_label("Annotating · 2");
        // Hide them, show them.
        h.get_by_label("Hide Comments").click();
        h.run_steps(2);
        assert!(h.state().review.hidden);
        h.get_by_label("Show Comments").click();
        h.run_steps(2);
        // Nothing selected: the bin takes them all.
        h.get_by_label("Delete All Comments").click();
        h.run_steps(2);
        assert!(h.state().review.notes.is_empty());
        h.get_by_label("Stop Annotating").click();
        h.run_steps(2);
        assert!(!h.state().review.annotating);
        assert!(h.query_by_label_contains("Annotating ·").is_none());
    }

    #[test]
    fn another_tool_or_an_edit_of_ones_own_ends_annotating_but_the_agents_dont() {
        let mut h = studio();
        h.state_mut().set_annotating(true);
        h.run_steps(2);
        // The agent changing the design: still pinning.
        let r = h
            .state_mut()
            .run_mcp_call("set_object", &json!({ "object": "sun", "kind": "satin" }));
        assert!(!r.is_error);
        h.run_steps(3);
        assert!(
            h.state().review.annotating,
            "the agent's edit doesn't end it"
        );
        // An edit of the person's own (as the panels make them): over.
        h.state_mut().doc.shapes[0].color = [1, 2, 3];
        h.run_steps(2);
        assert!(!h.state().review.annotating);
        // Another tool: over too.
        h.state_mut().set_annotating(true);
        h.run_steps(2);
        assert!(h.state().review.annotating);
        h.state_mut().tool = crate::app::Tool::Pen;
        h.run_steps(2);
        assert!(!h.state().review.annotating);
    }

    #[test]
    fn comments_are_one_chip_in_the_prompt_box_that_opens_into_a_list() {
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().dock.reveal(crate::dock::PanelId::Agent);
        h.run_steps(3);
        assert!(h.query_by_label_contains("comment").is_none(), "none yet");
        for (k, t) in ["longer rays", "thicker swoosh"].iter().enumerate() {
            h.state_mut().add_note(P::new(k as f64 * 100.0, 0.0), None);
            h.state_mut().review.notes[k].text = (*t).into();
        }
        h.state_mut().review.editing = None;
        h.run_steps(3);
        // Collapsed: just how many.
        h.get_by_label("2 comments");
        assert!(h.query_by_label("longer rays").is_none());
        h.get_by_label("2 comments").click();
        h.run_steps(3);
        h.get_by_label("longer rays");
        h.get_by_label("thicker swoosh");
        h.query_all_by_label("Remove Note").next().unwrap().click();
        h.run_steps(3);
        h.get_by_label("1 comment");
        assert!(h.query_by_label("longer rays").is_none());
        h.get_by_label("1 comment").click();
        h.run_steps(3);
        assert!(h.query_by_label("thicker swoosh").is_none(), "folded again");
    }

    #[test]
    fn notes_go_with_the_prompt_and_the_agent_resolves_them() {
        let mut h = studio();
        let tab = h.state().session.rec;
        let sun = uid_of(&h, "sun");
        let at = {
            let s = h.state();
            let x = s.doc.shapes.iter().find(|x| x.uid == sun).unwrap();
            shape_bounds(x).center()
        };
        h.state_mut().add_note(at, None);
        h.state_mut().review.notes[0].text = "longer rays".into();
        assert_eq!(h.state().review.pending(tab), 1);
        let (shown, told) = h.state_mut().notes_for_prompt();
        assert_eq!(shown, vec!["longer rays".to_string()]);
        assert!(
            told.contains("1. At (") && told.contains("on sun: longer rays"),
            "{told}"
        );
        assert!(told.contains("resolve_note"));
        // In previews: a pin (no number in it).
        let marks = h.state().preview_marks();
        assert_eq!(marks.len(), 1);
        let svg = zigzag::formats::preview::write_svg(
            &h.state().design,
            &zigzag::formats::preview::PreviewOptions {
                marks,
                ..Default::default()
            },
        );
        assert!(
            svg.contains(r#"<path d="M"#) && !svg.contains("<text"),
            "{svg}"
        );
        let r = h
            .state_mut()
            .run_mcp_call("render_preview", &json!({ "width_px": 300 }));
        assert!(!r.is_error);
        // Sent with a prompt: no longer pending, but still on the canvas.
        h.state_mut()
            .agent
            .log
            .push(crate::agent::Entry::You("x".into()));
        h.state_mut().begin_turn("x".into(), 0);
        assert_eq!(h.state().review.pending(tab), 0);
        assert_eq!(h.state().review.live(tab).count(), 1);
        let r = h.state_mut().run_mcp_call("get_annotations", &json!({}));
        let text = format!("{:?}", r.content);
        assert!(
            text.contains("longer rays") && text.contains("sent"),
            "{text}"
        );
        let r = h.state_mut().run_mcp_call(
            "resolve_note",
            &json!({ "number": 1, "reply": "Doubled them" }),
        );
        assert!(!r.is_error);
        assert_eq!(h.state().review.live(tab).count(), 0);
        assert_eq!(
            h.state().review.notes[0].reply.as_deref(),
            Some("Doubled them")
        );
        assert!(
            h.state_mut()
                .run_mcp_call("resolve_note", &json!({ "number": 1 }))
                .is_error
        );
    }

    #[test]
    fn a_turn_keeps_the_design_before_to_flip_to_and_revert_to() {
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        h.state_mut().dock.reveal(crate::dock::PanelId::Agent);
        h.state_mut()
            .agent
            .log
            .push(crate::agent::Entry::You("satin sun".into()));
        h.state_mut().begin_turn("satin sun".into(), 0);
        let was = doc_key(&h.state().doc);
        h.state_mut()
            .run_mcp_call("set_object", &json!({ "object": "sun", "kind": "satin" }));
        h.state_mut()
            .agent
            .log
            .push(crate::agent::Entry::Done(String::new()));
        h.run_steps(3);
        let t = &h.state().review.turns[0];
        assert_eq!(t.end, Some(1));
        assert!(t.after.is_some(), "it changed the design");
        // Under the turn: Before shows the design as it was, over the canvas.
        h.get_by_label("Before").click();
        h.run_steps(3);
        assert!(matches!(h.state().review.showing, Some(Showing::Before(_))));
        assert!(h.query_by_label("Show After").is_none(), "no bar over it");
        h.get_by_label("After").click();
        h.run_steps(3);
        assert!(h.state().review.showing.is_none());
        // Side by side, then back with Escape.
        h.get_by_label("Side by Side").click();
        h.run_steps(3);
        assert!(matches!(
            h.state().review.showing,
            Some(Showing::Compare(_))
        ));
        // Nothing over the panes until one is hovered.
        assert!(h.query_by_label("Revert to This").is_none());
        let panes = h.state().review.panes.clone();
        assert_eq!(
            panes.iter().map(|p| p.0).collect::<Vec<_>>(),
            [Pane::Before, Pane::After]
        );
        h.hover_at(panes[0].1.center());
        h.run_steps(2);
        h.get_by_label("Revert to This");
        assert!(h.query_by_label("Keep This").is_none());
        h.hover_at(panes[1].1.center());
        h.run_steps(2);
        h.get_by_label("Keep This");
        // No Revert or Try Again in the row: Before, After, Side by Side.
        assert!(h.query_by_label("Revert").is_none());
        assert!(h.query_by_label("Try Again").is_none());
        // Revert to This, on the Before pane: as it was, as one undo step.
        h.hover_at(panes[0].1.center());
        h.run_steps(2);
        h.get_by_label("Revert to This").click();
        h.run_steps(3);
        assert!(h.state().review.showing.is_none());
        assert_eq!(doc_key(&h.state().doc), was);
        h.state_mut().apply_history(crate::app::History::Undo);
        assert_ne!(doc_key(&h.state().doc), was, "undo brings the change back");
    }

    #[test]
    fn versions_are_kept_beside_the_original_and_one_can_be_used() {
        use egui_kittest::kittest::Queryable;
        let mut h = studio();
        let original = doc_key(&h.state().doc);
        // Nothing to save into yet.
        let r = h
            .state_mut()
            .run_mcp_call("save_variant", &json!({ "label": "x" }));
        assert!(r.is_error);
        assert!(
            !h.state_mut()
                .run_mcp_call("start_variants", &json!({}))
                .is_error
        );
        assert!(matches!(
            h.state().review.showing,
            Some(Showing::Compare(_))
        ));
        // The same as the original isn't a version.
        assert!(
            h.state_mut()
                .run_mcp_call("save_variant", &json!({ "label": "same" }))
                .is_error
        );
        for (kind, label) in [("satin", "Satin sun"), ("run", "Outlined sun")] {
            h.state_mut()
                .run_mcp_call("set_object", &json!({ "object": "sun", "kind": kind }));
            // (As if the plan had gone to the worker: the version still
            // gets the stitches of the design it keeps.)
            h.state_mut().dirty = false;
            h.state_mut().planner.pending = true;
            let r = h
                .state_mut()
                .run_mcp_call("save_variant", &json!({ "label": label }));
            assert!(!r.is_error, "{:?}", r.content);
            let v = h.state().review.turns[0].variants.last().unwrap();
            let fresh = v.doc.plan();
            assert_eq!(v.design.stitch_count(), fresh.stitch_count(), "{label}");
            h.run_steps(2);
            assert_eq!(doc_key(&h.state().doc), original, "back to the original");
        }
        h.run_steps(3);
        let panes = h.state().review.panes.clone();
        assert_eq!(
            panes.iter().map(|p| p.0).collect::<Vec<_>>(),
            [Pane::Before, Pane::Variant(0), Pane::Variant(1)]
        );
        assert!(h.query_by_label("Use This").is_none(), "only on hover");
        h.hover_at(panes[0].1.center());
        h.run_steps(2);
        h.get_by_label("Original");
        h.get_by_label("Keep Original");
        // The agent puts them away and brings them back.
        let r = h
            .state_mut()
            .run_mcp_call("show_comparison", &json!({ "show": false }));
        assert!(!r.is_error);
        assert!(h.state().review.showing.is_none());
        let r = h.state_mut().run_mcp_call("show_comparison", &json!({}));
        assert!(!r.is_error, "{:?}", r.content);
        assert!(matches!(
            h.state().review.showing,
            Some(Showing::Compare(_))
        ));
        h.run_steps(2);
        h.hover_at(panes[2].1.center());
        h.run_steps(2);
        h.get_by_label("B · Outlined sun");
        h.get_by_label("Use This").click();
        h.run_steps(3);
        assert!(h.state().review.showing.is_none());
        let i = h.state().names.iter().position(|n| n == "sun").unwrap();
        assert_eq!(
            h.state().doc.shapes[i].params.kind,
            Some(zigzag::params::Kind::Run)
        );
        assert!(!h.state().review.turns[0].collecting, "that round is over");
    }

    #[test]
    fn an_unsaved_last_version_is_kept_when_the_agent_finishes() {
        let mut h = studio();
        let original = doc_key(&h.state().doc);
        h.state_mut()
            .agent
            .log
            .push(crate::agent::Entry::You("two".into()));
        h.state_mut().begin_turn("two".into(), 0);
        assert!(h.state().review.showing.is_none());
        // The agent was asked for options: this prompt becomes its versions.
        h.state_mut().run_mcp_call("start_variants", &json!({}));
        assert!(matches!(
            h.state().review.showing,
            Some(Showing::Compare(_))
        ));
        assert_eq!(h.state().review.turns.len(), 1, "the prompt's own turn");
        assert!(h.state().review.turns[0].collecting);
        h.state_mut()
            .run_mcp_call("set_object", &json!({ "object": "sun", "kind": "satin" }));
        h.state_mut()
            .run_mcp_call("save_variant", &json!({ "label": "Satin" }));
        h.state_mut()
            .run_mcp_call("set_object", &json!({ "object": "sun", "kind": "run" }));
        h.state_mut()
            .agent
            .log
            .push(crate::agent::Entry::Done(String::new()));
        h.run_steps(3);
        let t = &h.state().review.turns[0];
        assert_eq!(t.variants.len(), 2);
        assert_eq!(t.variants[1].label, "Version B");
        assert!(t.after.is_none());
        assert_eq!(doc_key(&h.state().doc), original);
    }

    #[test]
    fn a_version_chosen_while_the_agent_finishes_stays_chosen() {
        let mut h = studio();
        h.state_mut()
            .agent
            .log
            .push(crate::agent::Entry::You("two".into()));
        h.state_mut().begin_turn("two".into(), 0);
        h.state_mut().run_mcp_call("start_variants", &json!({}));
        h.state_mut()
            .run_mcp_call("set_object", &json!({ "object": "sun", "kind": "satin" }));
        h.state_mut()
            .run_mcp_call("save_variant", &json!({ "label": "Satin" }));
        // The person picks it before the agent has said its last word.
        let turn = h.state().review.turns[0].id;
        let chosen = h.state().review.turns[0].variants[0].key.clone();
        h.state_mut().turn_action(TurnAction::Use {
            turn,
            pane: Pane::Variant(0),
        });
        assert_eq!(doc_key(&h.state().doc), chosen);
        h.state_mut()
            .agent
            .log
            .push(crate::agent::Entry::Done(String::new()));
        h.run_steps(3);
        let t = &h.state().review.turns[0];
        assert_eq!(t.variants.len(), 1, "no copy of the choice kept");
        assert_eq!(doc_key(&h.state().doc), chosen, "the choice stays");
    }

    #[test]
    fn a_turn_that_only_leaves_notes_on_the_sewing_isnt_said_to_leave_problems() {
        let mut h = studio();
        h.state_mut()
            .agent
            .log
            .push(crate::agent::Entry::You("wider".into()));
        h.state_mut().begin_turn("wider".into(), 0);
        // A satin wide enough that its stitches are split: a note, not a problem.
        h.state_mut().run_mcp_call(
            "add_svg",
            &json!({ "svg": r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 60 60"><path id="bar_satin" d="M5 30 L55 30" stroke="#222" stroke-width="9" fill="none"/></svg>"##, "width_mm": 50 }),
        );
        let s = h.state();
        assert!(
            s.issues
                .iter()
                .any(|i| i.level == zigzag::checks::Level::Info),
            "{:?}",
            s.issues
        );
        let only_notes = s
            .issues
            .iter()
            .all(|i| i.level == zigzag::checks::Level::Info);
        h.state_mut()
            .agent
            .log
            .push(crate::agent::Entry::Done(String::new()));
        h.run_steps(3);
        let said = h.state().agent.log.iter().any(
            |e| matches!(e, crate::agent::Entry::Error(m) if m.contains("didn't have before")),
        );
        assert_eq!(said, !only_notes, "{:?}", h.state().issues);
    }

    #[test]
    fn a_note_waiting_for_the_agent_doesnt_go_once_its_stopped() {
        let mut h = studio();
        h.state_mut().add_note(P::new(0.0, 0.0), None);
        let rec = h.state().session.rec;
        h.state_mut().review.queued = Some(rec);
        // Stop (nothing running here: the flag is what the frame reads).
        h.state_mut().agent.stop();
        h.run_steps(2);
        assert!(h.state().review.queued.is_none());
        assert!(!h.state().agent.running());
        assert!(h.state().agent.log.is_empty(), "nothing was sent");
    }

    #[test]
    fn one_view_button_is_lit_at_a_time() {
        assert_eq!(
            lit(None, 1),
            (false, true, false),
            "after: the design as it is"
        );
        assert_eq!(lit(Some(Showing::Before(1)), 1), (true, false, false));
        assert_eq!(lit(Some(Showing::Compare(1)), 1), (false, false, true));
        assert_eq!(
            lit(Some(Showing::Compare(2)), 1),
            (false, true, false),
            "another turn's"
        );
    }

    #[test]
    fn hidden_comments_take_no_clicks_and_a_new_one_shows_them_all() {
        let mut h = studio();
        h.state_mut().set_annotating(true);
        h.state_mut().add_note(P::new(0.0, 0.0), None);
        h.run_steps(2);
        let rect = h.state().canvas_rect;
        let s = h.state();
        let tip = s.note_tip(rect, &s.review.notes[0]) + Vec2::new(4.0, -4.0);
        assert!(s.note_at(rect, tip).is_some());
        h.state_mut().review.hidden = true;
        assert!(h.state().note_at(rect, tip).is_none(), "hidden: not there");
        // Another pinned: they all show again.
        h.state_mut().add_note(P::new(50.0, 0.0), None);
        assert!(!h.state().review.hidden);
        // And hidden doesn't outlast the bar its eye is on.
        h.state_mut().review.hidden = true;
        h.state_mut().set_annotating(false);
        assert!(!h.state().review.hidden);
    }

    #[test]
    fn only_the_latest_turns_keep_their_designs() {
        let mut h = studio();
        for k in 0..KEPT_TURNS + 3 {
            h.state_mut().begin_turn(format!("prompt {k}"), k);
            h.state_mut().review.turns.last_mut().unwrap().end = Some(k);
        }
        let turns = &h.state().review.turns;
        // (The newest is pushed, then the oldest let go.)
        assert_eq!(turns.len(), KEPT_TURNS);
        assert_eq!(turns[0].prompt, "prompt 3");
        // One on show is kept, however old.
        let first = turns[0].id;
        h.state_mut().review.open(Showing::Before(first));
        h.state_mut().begin_turn("another".into(), 99);
        assert!(h.state().review.turn(first).is_some());
    }

    #[test]
    fn numbers_read_with_commas() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(12345), "12,345");
        assert_eq!(thousands(1234567), "1,234,567");
    }
}
