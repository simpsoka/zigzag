//! Every menu command in one place. The same list drives the macOS menu bar,
//! the in-window menus on other systems, and keyboard shortcuts.

use crate::app::{History, Studio, Tool};
use crate::dock::PanelId;
use crate::shapes::ShapeKind;
use eframe::egui::{self, Key, KeyboardShortcut, Modifiers};
use zigzag::boolean::PathOp;
use zigzag::geom::{rotate_about, scale_about, translation};
use zigzag::params::Kind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    New,
    Open,
    /// File › Close: the tab in front (asks first if it has unsaved changes).
    CloseTab,
    NextTab,
    PreviousTab,
    Place,
    Save,
    SaveAs,
    Export(&'static str),
    SendToSkitch,
    Undo,
    Redo,
    Copy,
    Paste,
    Duplicate,
    Delete,
    SelectAll,
    Deselect,
    Kind(Kind),
    FlipH,
    FlipV,
    RotateLeft,
    RotateRight,
    CenterInHoop,
    FitToHoop,
    BringToFront,
    BringForward,
    SendBackward,
    SendToBack,
    GroupColors,
    AddText,
    Tool(Tool),
    ToggleGrid,
    ToggleSmartGuides,
    /// Show lengths in this unit (View › Units).
    Units(crate::units::Unit),
    /// The contextual task bar under the selection.
    ToggleTaskBar,
    /// Collapse the right dock to icons (or expand it).
    IconicPanels,
    /// Put every panel back where it started.
    ResetPanels,
    /// Show the prompt bar and put the cursor in it.
    AskAgent,
    /// Open the issues list (in the status bar).
    ShowIssues,
    TransformAgain,
    Pathfinder(zigzag::boolean::PathOp),
    OffsetPath,
    OutlineStroke,
    /// Object › Path › Release Compound Path: one object per separate shape.
    ReleaseCompound,
    SimplifyPath,
    RoundCorners,
    /// Edit › Copy / Paste Stitch Settings (Ink/Stitch's Duplicate Params).
    CopySettings,
    PasteSettings,
    /// Edit › Select Same Stitch Type.
    SelectSameKind,
    /// Object › Path › Reverse Direction: sew the selection's paths the
    /// other way (a satin from its other end).
    ReversePath,
    /// Object › Test Swatches….
    TestSwatches,
    /// Type › Create Outlines: lettering into plain paths.
    CreateOutlines,
    /// Type › Convert to Text: outlines made from text back into text.
    ConvertToText,
    /// Type › Change Case.
    ChangeCase(zigzag::document::Case),
    /// Type › Align.
    AlignText(zigzag::document::Align),
    /// Type › Type on a Path: the selected text onto the selected path.
    TypeOnPath,
    CleanUp,
    ToggleStitchLines,
    TogglePoints,
    ToggleJumps,
    /// Tint where stitches pile up (View › Density Map).
    ToggleDensity,
    /// Number the objects in sewing order on the canvas.
    ToggleOrder,
    /// Draw the stitches on the GPU (off: egui's painter, as before; for
    /// comparing the two, or a GL driver that gets the shader wrong).
    ToggleGpuStitches,
    ZoomIn,
    ZoomOut,
    ZoomFit,
    PlayPause,
    Panel(PanelId),
    ToggleLeftDock,
    ToggleRightDock,
}

/// A keyboard shortcut. `cmd` is ⌘ on macOS and Ctrl elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sc {
    pub key: Key,
    pub cmd: bool,
    pub shift: bool,
}

const fn cmd(key: Key) -> Option<Sc> {
    Some(Sc {
        key,
        cmd: true,
        shift: false,
    })
}
const fn cmd_shift(key: Key) -> Option<Sc> {
    Some(Sc {
        key,
        cmd: true,
        shift: true,
    })
}
const fn plain(key: Key) -> Option<Sc> {
    Some(Sc {
        key,
        cmd: false,
        shift: false,
    })
}

impl Sc {
    pub fn egui(&self) -> KeyboardShortcut {
        let mut m = Modifiers::NONE;
        if self.cmd {
            m |= Modifiers::COMMAND;
        }
        if self.shift {
            m |= Modifiers::SHIFT;
        }
        KeyboardShortcut::new(m, self.key)
    }
}

pub enum Entry {
    Item(Action, &'static str, Option<Sc>),
    Sep,
    Sub(&'static str, Vec<Entry>),
}

use Entry::{Item, Sep, Sub};

pub fn menus() -> Vec<(&'static str, Vec<Entry>)> {
    let mut export: Vec<Entry> = zigzag::formats::MACHINE_FORMATS
        .iter()
        .map(|&(ext, label)| Item(Action::Export(ext), label, None))
        .collect();
    export.push(Sep);
    export.push(Item(
        Action::Export("html"),
        "Production Worksheet (HTML)…",
        None,
    ));
    let kinds = vec![
        Item(Action::Kind(Kind::Fill), "Fill", plain(Key::F)),
        Item(Action::Kind(Kind::Satin), "Satin", plain(Key::S)),
        Item(Action::Kind(Kind::Run), "Run", plain(Key::R)),
        Item(Action::Kind(Kind::Contour), "Contour Fill", plain(Key::C)),
        Item(Action::Kind(Kind::Blanket), "Blanket", None),
        Item(Action::Kind(Kind::Applique), "Appliqué", None),
        Item(Action::Kind(Kind::Meander), "Meander Fill", None),
        Item(Action::Kind(Kind::Ripple), "Ripple Fill", None),
        Item(Action::Kind(Kind::Cross), "Cross-Stitch", None),
        Item(Action::Kind(Kind::Tartan), "Tartan", None),
        Item(Action::Kind(Kind::Manual), "Manual Stitches", None),
        Sep,
        Item(Action::Kind(Kind::Skip), "Don't Stitch", plain(Key::X)),
    ];
    vec![
        (
            "File",
            vec![
                Item(Action::New, "New", cmd(Key::N)),
                Item(Action::Open, "Open…", cmd(Key::O)),
                Item(Action::Place, "Place SVG…", cmd_shift(Key::P)),
                Sep,
                Item(Action::CloseTab, "Close", cmd(Key::W)),
                Item(Action::Save, "Save", cmd(Key::S)),
                Item(Action::SaveAs, "Save As…", cmd_shift(Key::S)),
                Sep,
                Sub("Export", export),
                Item(Action::SendToSkitch, "Send to Skitch…", None),
            ],
        ),
        (
            "Edit",
            vec![
                Item(Action::Undo, "Undo", cmd(Key::Z)),
                Item(Action::Redo, "Redo", cmd_shift(Key::Z)),
                Sep,
                Item(Action::Copy, "Copy", cmd(Key::C)),
                Item(Action::Paste, "Paste", cmd(Key::V)),
                Item(Action::Duplicate, "Duplicate", None),
                Item(Action::Delete, "Delete", plain(Key::Backspace)),
                Sep,
                Item(Action::CopySettings, "Copy Stitch Settings", None),
                Item(Action::PasteSettings, "Paste Stitch Settings", None),
                Sep,
                Item(Action::SelectAll, "Select All", cmd(Key::A)),
                Item(Action::Deselect, "Deselect All", cmd_shift(Key::A)),
                Item(Action::SelectSameKind, "Select Same Stitch Type", None),
            ],
        ),
        (
            "Object",
            vec![
                Sub("Stitch Type", kinds),
                Sep,
                Item(Action::TransformAgain, "Transform Again", cmd(Key::D)),
                Item(Action::FlipH, "Flip Horizontal", None),
                Item(Action::FlipV, "Flip Vertical", None),
                Item(Action::RotateLeft, "Rotate Left 90°", None),
                Item(Action::RotateRight, "Rotate Right 90°", None),
                Item(Action::CenterInHoop, "Center in Hoop", None),
                Item(Action::FitToHoop, "Fit to Hoop", None),
                Sep,
                Sub(
                    "Path",
                    vec![
                        Item(Action::OffsetPath, "Offset Path…", None),
                        Item(Action::OutlineStroke, "Outline Stroke", None),
                        Item(Action::ReleaseCompound, "Release Compound Path", None),
                        Item(Action::SimplifyPath, "Simplify…", None),
                        Item(Action::RoundCorners, "Round Corners…", None),
                        Item(Action::ReversePath, "Reverse Direction", None),
                        Sep,
                        Item(Action::CleanUp, "Clean Up…", None),
                    ],
                ),
                Sub(
                    "Pathfinder",
                    vec![
                        Item(Action::Pathfinder(PathOp::Unite), "Unite", None),
                        Item(Action::Pathfinder(PathOp::MinusFront), "Minus Front", None),
                        Item(Action::Pathfinder(PathOp::Intersect), "Intersect", None),
                        Item(Action::Pathfinder(PathOp::Exclude), "Exclude", None),
                    ],
                ),
                Sep,
                Sub(
                    "Sewing Order",
                    vec![
                        Item(
                            Action::BringToFront,
                            "Sew Last (on Top)",
                            cmd_shift(Key::CloseBracket),
                        ),
                        Item(Action::BringForward, "Sew Later", cmd(Key::CloseBracket)),
                        Item(Action::SendBackward, "Sew Earlier", cmd(Key::OpenBracket)),
                        Item(Action::SendToBack, "Sew First", cmd_shift(Key::OpenBracket)),
                        Sep,
                        Item(Action::GroupColors, "Group Colors", None),
                    ],
                ),
                Sep,
                Item(Action::TestSwatches, "Test Swatches…", None),
            ],
        ),
        (
            "Type",
            vec![
                Item(Action::AddText, "Add Text…", None),
                Item(Action::TypeOnPath, "Type on a Path", None),
                Sep,
                Sub(
                    "Change Case",
                    vec![
                        Item(
                            Action::ChangeCase(zigzag::document::Case::Upper),
                            "UPPERCASE",
                            None,
                        ),
                        Item(
                            Action::ChangeCase(zigzag::document::Case::Lower),
                            "lowercase",
                            None,
                        ),
                        Item(
                            Action::ChangeCase(zigzag::document::Case::Title),
                            "Title Case",
                            None,
                        ),
                        Item(
                            Action::ChangeCase(zigzag::document::Case::AsTyped),
                            "As Typed",
                            None,
                        ),
                    ],
                ),
                Sub(
                    "Align",
                    vec![
                        Item(
                            Action::AlignText(zigzag::document::Align::Left),
                            "Left",
                            None,
                        ),
                        Item(
                            Action::AlignText(zigzag::document::Align::Center),
                            "Center",
                            None,
                        ),
                        Item(
                            Action::AlignText(zigzag::document::Align::Right),
                            "Right",
                            None,
                        ),
                    ],
                ),
                Sep,
                Item(Action::CreateOutlines, "Create Outlines", cmd_shift(Key::O)),
                Item(Action::ConvertToText, "Convert to Text", None),
            ],
        ),
        (
            "View",
            vec![
                Item(Action::ZoomIn, "Zoom In", cmd(Key::Equals)),
                Item(Action::ZoomOut, "Zoom Out", cmd(Key::Minus)),
                Item(Action::ZoomFit, "Zoom to Fit", cmd(Key::Num0)),
                Sep,
                Item(Action::ToggleStitchLines, "Stitch Lines", cmd(Key::Y)),
                Item(Action::ToggleGrid, "Grid", cmd(Key::Quote)),
                Item(Action::ToggleSmartGuides, "Smart Guides", cmd(Key::U)),
                Sub(
                    "Units",
                    crate::units::Unit::ALL
                        .iter()
                        .map(|&u| Item(Action::Units(u), u.label(), None))
                        .collect(),
                ),
                Item(Action::ToggleTaskBar, "Contextual Task Bar", None),
                Item(Action::TogglePoints, "Needle Points", None),
                Item(Action::ToggleJumps, "Jump Stitches", None),
                Item(Action::ToggleDensity, "Density Map", None),
                Item(Action::ToggleOrder, "Sewing Order", None),
                Item(Action::ToggleGpuStitches, "Draw Stitches on GPU", None),
                Sep,
                Item(Action::PlayPause, "Play Stitch-Out", plain(Key::Space)),
            ],
        ),
        (
            "Window",
            vec![
                Item(Action::ToggleLeftDock, "Left Dock", None),
                Item(Action::ToggleRightDock, "Panels", None),
                Item(Action::IconicPanels, "Collapse Panels to Icons", None),
                Item(Action::ResetPanels, "Reset Panels", None),
                Item(Action::AskAgent, "Ask Agent…", cmd(Key::K)),
                Sep,
                // (Also ⌃Tab and ⌃⇧Tab.)
                Item(Action::NextTab, "Next Document", None),
                Item(Action::PreviousTab, "Previous Document", None),
                Sep,
                Item(Action::Panel(PanelId::Objects), "Objects", None),
                Item(Action::ShowIssues, "Issues", None),
                Item(Action::Panel(PanelId::Stitch), "Stitch", None),
                Item(Action::Panel(PanelId::Transform), "Transform", None),
                Item(Action::Panel(PanelId::Lettering), "Lettering", None),
                Item(Action::Panel(PanelId::Threads), "Threads", None),
                Item(Action::Panel(PanelId::Document), "Document", None),
                Item(Action::Panel(PanelId::Agent), "Agent", None),
            ],
        ),
    ]
}

/// Tools, with their single-key shortcuts (handled with the rest).
pub const TOOLS: [(Tool, &str, Key); 9] = [
    (Tool::Select, "Select", Key::V),
    (Tool::Direct, "Direct Selection", Key::A),
    (Tool::Hand, "Hand", Key::H),
    (Tool::Pen, "Pen", Key::P),
    (Tool::Brush, "Brush", Key::B),
    (Tool::Shape, "Shape", Key::M),
    (Tool::Angle, "Fill Angle", Key::G),
    (Tool::Measure, "Measure", Key::I),
    (Tool::Text, "Type", Key::T),
];

fn walk(entries: &[Entry], out: &mut Vec<(Action, &'static str, Option<Sc>)>) {
    for e in entries {
        match e {
            Item(a, l, s) => out.push((*a, *l, *s)),
            Sub(_, v) => walk(v, out),
            Sep => {}
        }
    }
}

/// Every action with its label and shortcut, flattened.
pub fn all_actions() -> Vec<(Action, &'static str, Option<Sc>)> {
    let mut out = Vec::new();
    for (_, v) in &menus() {
        walk(v, &mut out);
    }
    out
}

impl Studio {
    pub fn action_enabled(&self, a: Action) -> bool {
        let has_sel = !self.sel_indices().is_empty();
        let has_design = !self.design.blocks.is_empty();
        match a {
            Action::Undo => !self.undo.is_empty(),
            Action::Redo => !self.redo.is_empty(),
            Action::Copy | Action::Duplicate | Action::Delete | Action::Deselect => has_sel,
            Action::CopySettings
            | Action::SelectSameKind
            | Action::ReversePath
            | Action::TestSwatches => has_sel,
            Action::PasteSettings => has_sel && self.layout.settings_clip.is_some(),
            Action::ConvertToText => self.sel_indices().iter().any(|&i| self.doc.records_text(i)),
            Action::CreateOutlines | Action::ChangeCase(_) | Action::AlignText(_) => self
                .sel_indices()
                .iter()
                .any(|&i| self.doc.lettering(i).is_some()),
            Action::TypeOnPath => {
                let sel = self.sel_indices();
                sel.len() == 2
                    && sel
                        .iter()
                        .filter(|&&i| self.doc.lettering(i).is_some())
                        .count()
                        == 1
            }
            Action::Paste => !self.layout.clipboard.is_empty(),
            Action::SelectAll | Action::GroupColors | Action::FitToHoop | Action::CleanUp => {
                !self.doc.shapes.is_empty()
            }
            Action::Export(_) | Action::SendToSkitch | Action::PlayPause => has_design,
            Action::FlipH
            | Action::FlipV
            | Action::RotateLeft
            | Action::RotateRight
            | Action::CenterInHoop
            | Action::TransformAgain
            | Action::OffsetPath
            | Action::OutlineStroke
            | Action::ReleaseCompound
            | Action::SimplifyPath
            | Action::RoundCorners
            | Action::BringToFront
            | Action::BringForward
            | Action::SendBackward
            | Action::SendToBack => has_sel,
            Action::Pathfinder(_) => self.sel_indices().len() >= 2,
            Action::NextTab | Action::PreviousTab => self.tab_count() > 1,
            Action::Kind(k) => self
                .sel_indices()
                .iter()
                .any(|&i| crate::widgets::allowed_kinds(&self.doc.shapes[i]).contains(&k)),
            _ => true,
        }
    }

    pub fn action_checked(&self, a: Action) -> Option<bool> {
        Some(match a {
            Action::ToggleGrid => self.view_tools.grid,
            Action::ToggleSmartGuides => self.view_tools.smart_guides,
            Action::Units(u) => self.dock.units == u,
            Action::ChangeCase(c) => {
                let i = self.sel_index()?;
                self.doc.lettering(i)?.case == c
            }
            Action::AlignText(a) => {
                let i = self.sel_index()?;
                self.doc.lettering(i)?.align == a
            }
            Action::ToggleTaskBar => self.dock.task_bar,
            Action::IconicPanels => self.dock.iconic[1],
            Action::ToggleStitchLines => {
                self.view_tools.stitch_view == crate::canvas::StitchView::Lines
            }
            Action::TogglePoints => self.view_tools.points,
            Action::ToggleJumps => self.show_jumps,
            Action::ToggleDensity => self.view_tools.density,
            Action::ToggleOrder => self.view_tools.order_labels,
            Action::ToggleGpuStitches => self.view_tools.gpu_stitches && self.gpu.is_some(),
            Action::Panel(p) => self.dock.visible(p),
            Action::ToggleLeftDock => self.dock.left,
            Action::ToggleRightDock => self.dock.right,
            Action::Tool(t) => self.tool == t,
            Action::Kind(k) => self
                .sel_index()
                .is_some_and(|i| self.doc.shapes[i].params.kind.unwrap_or(Kind::Fill) == k),
            _ => return None,
        })
    }

    pub fn run_action(&mut self, a: Action) {
        if !self.action_enabled(a) {
            return;
        }
        let c = self.selection_bounds().center();
        match a {
            Action::New => {
                self.new_tab();
                self.status = "New document.".into();
            }
            Action::CloseTab => {
                // The last, empty tab: ⌘W closes the window, as on macOS.
                if self.tab_count() == 1 && self.tab_is_blank() {
                    if let Some(ctx) = &self.egui_ctx {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                } else {
                    self.close_tab(self.active_tab());
                }
            }
            Action::NextTab => {
                let n = self.tab_count();
                self.tabs.pending = Some(crate::tabs::TabOp::Switch((self.active_tab() + 1) % n));
            }
            Action::PreviousTab => {
                let n = self.tab_count();
                self.tabs.pending =
                    Some(crate::tabs::TabOp::Switch((self.active_tab() + n - 1) % n));
            }
            Action::Open => {
                let mut exts = vec!["svg"];
                exts.extend(zigzag::formats::READABLE_FORMATS);
                exts.extend(zigzag::trace::IMAGE_FORMATS);
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Designs", &exts)
                    .add_filter("SVG artwork", &["svg"])
                    .add_filter("Images to trace", &zigzag::trace::IMAGE_FORMATS)
                    .add_filter("Machine files", &zigzag::formats::READABLE_FORMATS)
                    .pick_file()
                {
                    self.load(&p);
                }
            }
            Action::Place => {
                let mut exts = vec!["svg"];
                exts.extend(zigzag::trace::IMAGE_FORMATS);
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Artwork", &exts)
                    .pick_file()
                {
                    self.place_svg(&p);
                }
            }
            Action::Save => {
                if self.doc.path.is_some() {
                    self.save(None)
                } else {
                    self.save_as_dialog()
                }
            }
            Action::SaveAs => self.save_as_dialog(),
            Action::Export(ext) => self.export_dialog(ext),
            Action::SendToSkitch => self.send_action(),
            Action::Undo => self.pending_history = Some(History::Undo),
            Action::Redo => self.pending_history = Some(History::Redo),
            Action::Copy => self.copy_selected(),
            Action::Paste => self.paste(),
            Action::Duplicate => self.duplicate_selected(),
            Action::Delete => self.delete_selection(),
            Action::SelectAll => {
                // Inside a group, Select All means everything in it.
                let idx: Vec<usize> = (0..self.doc.shapes.len())
                    .filter(|&i| self.in_scope(i))
                    .collect();
                self.set_selection(&idx);
            }
            Action::Deselect => self.set_selection(&[]),
            Action::Kind(k) => self.set_kind_selected(k),
            Action::FlipH => {
                self.transform_selected(scale_about(c, -1.0, 1.0), "Flipped");
            }
            Action::FlipV => {
                self.transform_selected(scale_about(c, 1.0, -1.0), "Flipped");
            }
            Action::RotateLeft => {
                self.transform_selected(rotate_about(c, -90.0), "Rotated");
            }
            Action::RotateRight => {
                self.transform_selected(rotate_about(c, 90.0), "Rotated");
            }
            Action::FitToHoop => {
                self.fit_to_hoop(false);
            }
            Action::CenterInHoop => {
                self.transform_selected(translation(zigzag::geom::P::default() - c), "Centered");
            }
            Action::BringToFront => self.move_selected_to(false),
            Action::SendToBack => self.move_selected_to(true),
            Action::BringForward => self.move_selected(1),
            Action::SendBackward => self.move_selected(-1),
            Action::GroupColors => self.group_colors(),
            Action::AddText => self.open_text_window(),
            Action::Tool(t) => self.set_tool(t),
            Action::ToggleGrid => self.view_tools.grid = !self.view_tools.grid,
            Action::Units(u) => self.set_units(u),
            Action::ToggleSmartGuides => {
                self.view_tools.smart_guides = !self.view_tools.smart_guides
            }
            Action::TransformAgain => self.transform_again(),
            Action::ToggleTaskBar => self.dock.task_bar = !self.dock.task_bar,
            Action::IconicPanels => {
                self.dock.iconic[1] = !self.dock.iconic[1];
                self.dock_ui.flyout = None;
            }
            Action::ResetPanels => {
                self.dock.reset_layout();
                self.dock_ui.flyout = None;
            }
            Action::AskAgent => self.ask_agent(),
            Action::ShowIssues => self.open_issues = true,
            Action::Pathfinder(op) => self.pathfinder_selected(op),
            Action::OffsetPath => self.layout.offset_dialog = Some(1.0),
            Action::OutlineStroke => self.outline_stroke_selected(),
            Action::ReleaseCompound => self.release_compound_selected(),
            Action::SimplifyPath => self.open_simplify_dialog(),
            Action::CleanUp => self.open_cleanup_dialog(),
            Action::CopySettings => {
                if let Some(i) = self.sel_index() {
                    self.layout.settings_clip = Some(self.doc.shapes[i].params.clone());
                    self.status =
                        "Stitch settings copied. Select other objects and Paste Stitch Settings."
                            .into();
                }
            }
            Action::PasteSettings => self.paste_settings(),
            Action::SelectSameKind => self.select_same_kind(),
            Action::ReversePath => self.reverse_paths(),
            Action::TestSwatches => self.open_swatch_dialog(),
            Action::CreateOutlines => self.create_outlines(),
            Action::ConvertToText => self.convert_to_text(),
            Action::ChangeCase(c) => self.edit_selected_text(|t| t.case = c),
            Action::AlignText(a) => self.edit_selected_text(|t| t.align = a),
            Action::TypeOnPath => self.type_on_selected_path(),
            Action::RoundCorners => self.open_corner_dialog(),
            Action::ToggleStitchLines => {
                use crate::canvas::StitchView;
                self.view_tools.stitch_view = if self.view_tools.stitch_view == StitchView::Lines {
                    StitchView::Thread
                } else {
                    StitchView::Lines
                };
            }
            Action::TogglePoints => self.view_tools.points = !self.view_tools.points,
            Action::ToggleJumps => self.show_jumps = !self.show_jumps,
            Action::ToggleGpuStitches => {
                self.view_tools.gpu_stitches = !self.view_tools.gpu_stitches;
                self.status = match (self.view_tools.gpu_stitches, self.gpu.is_some()) {
                    (true, true) => "Stitches drawn on the GPU.".into(),
                    (true, false) => {
                        "No GPU stitch renderer on this context: the painter draws them.".into()
                    }
                    (false, _) => "Stitches drawn by the painter (slower on big designs).".into(),
                };
            }
            Action::ToggleDensity => {
                self.view_tools.density = !self.view_tools.density;
                if self.view_tools.density {
                    let m = self.density_map();
                    self.status = if m.heavy_cells() == 0 {
                        format!("No heavy spots: at most {:.1} stitches per mm².", m.peak())
                    } else {
                        format!(
                            "Yellow to red: {} to {} stitches per mm² and up (peak {:.1}). Two fills on top of each other, or dense satin over fill, get there.",
                            crate::density::HEAVY,
                            crate::density::TOO_HEAVY,
                            m.peak()
                        )
                    };
                }
            }
            Action::ToggleOrder => self.view_tools.order_labels = !self.view_tools.order_labels,
            Action::ZoomIn => self.zoom_by(1.25),
            Action::ZoomOut => self.zoom_by(0.8),
            Action::ZoomFit => self.request_fit(),
            Action::PlayPause => self.toggle_play(),
            Action::Panel(p) => self.dock.toggle(p),
            Action::ToggleLeftDock => self.dock.left = !self.dock.left,
            Action::ToggleRightDock => self.dock.right = !self.dock.right,
        }
    }

    pub fn set_tool(&mut self, t: Tool) {
        if self.tool == Tool::Text && t != Tool::Text {
            self.finish_typing();
        }
        // Another tool ends the path being drawn, keeping it (Illustrator).
        if self.tool == Tool::Pen && t != Tool::Pen && !self.view_tools.pen.is_empty() {
            self.finish_pen(false);
        }
        if crate::flyout::PEN_GROUP.iter().any(|g| g.0 == t) {
            self.view_tools.pen_slot = t;
        }
        if crate::flyout::BRUSH_GROUP.iter().any(|g| g.0 == t) {
            self.view_tools.brush_slot = t;
        }
        self.tool = t;
        self.view_tools.pen.clear();
        self.path_edit.selected.clear();
        self.path_edit.drag = None;
        self.path_edit.show_all = false;
    }

    /// Keyboard shortcuts for every action. With the macOS menu bar active,
    /// ⌘-shortcuts arrive as menu events instead and are skipped here.
    pub fn action_shortcuts(&mut self, ctx: &egui::Context) {
        // Typing with the Type tool: every key is text.
        if self.lettering.typing.is_some() {
            return;
        }
        let typing = ctx.egui_wants_keyboard_input();
        let native = self.native_menu_active();
        let mut todo = Vec::new();
        let mut shape = None;
        // Shift+C is the Anchor Point tool: taken before the plain C (Contour)
        // shortcut, which would match it too.
        if !ctx.egui_wants_keyboard_input()
            && ctx.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::C))
        {
            self.set_tool(Tool::Anchor);
            return;
        }
        // + (or =) and - : Add and Delete Anchor Point, as in Illustrator.
        if !ctx.egui_wants_keyboard_input() {
            let (add, del) = ctx.input_mut(|i| {
                let add = i.consume_key(Modifiers::NONE, Key::Plus)
                    || i.consume_key(Modifiers::SHIFT, Key::Plus)
                    || i.consume_key(Modifiers::NONE, Key::Equals)
                    || i.consume_key(Modifiers::SHIFT, Key::Equals);
                let del = i.consume_key(Modifiers::NONE, Key::Minus);
                (add, del)
            });
            if add {
                self.set_tool(Tool::AddAnchor);
                return;
            }
            if del {
                self.set_tool(Tool::DeleteAnchor);
                return;
            }
        }
        let mut all = all_actions();
        // ⇧ variants first: egui matches ⌘Z inside ⌘⇧Z otherwise.
        all.sort_by_key(|(_, _, s)| std::cmp::Reverse(s.is_some_and(|s| s.shift)));
        ctx.input_mut(|i| {
            for (a, _, sc) in &all {
                let Some(sc) = sc else { continue };
                // Space is handled by the canvas: hold to pan, tap to play.
                if *a == Action::PlayPause {
                    continue;
                }
                if (native && sc.cmd)
                    || (typing
                        && !matches!(
                            a,
                            Action::Save
                                | Action::SaveAs
                                | Action::Open
                                | Action::New
                                | Action::CloseTab
                        ))
                {
                    continue;
                }
                if !sc.cmd && (i.modifiers.command || i.modifiers.alt) {
                    continue;
                }
                if i.consume_shortcut(&sc.egui()) {
                    todo.push(*a);
                }
            }
            // Copy / paste arrive as clipboard events on most platforms.
            if !typing && !native {
                for e in &i.events {
                    match e {
                        egui::Event::Copy => todo.push(Action::Copy),
                        egui::Event::Paste(_) => todo.push(Action::Paste),
                        _ => {}
                    }
                }
            }
            if !typing {
                for (t, _, key) in TOOLS {
                    if !i.modifiers.command && i.key_pressed(key) {
                        todo.push(Action::Tool(t));
                    }
                }
                // Shape tools, as in Illustrator: M rectangle, L ellipse, \ line.
                for (k, key) in [
                    (ShapeKind::Rect, Key::M),
                    (ShapeKind::Ellipse, Key::L),
                    (ShapeKind::Line, Key::Backslash),
                ] {
                    if !i.modifiers.command && i.key_pressed(key) {
                        shape = Some(k);
                    }
                }
                if i.key_pressed(Key::Delete) && !i.modifiers.command {
                    todo.push(Action::Delete);
                }
            }
        });
        todo.dedup();
        if let Some(k) = shape {
            self.shapes.kind = k;
            todo.retain(|a| *a != Action::Tool(Tool::Shape));
            self.set_tool(Tool::Shape);
        }
        for a in todo {
            self.run_action(a);
        }
    }

    /// In-window menus (Linux, Windows). macOS uses the real menu bar.
    pub fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // Plain menu buttons (not a MenuBar, which claims the whole row).
        ui.scope(|ui| {
            ui.visuals_mut().button_frame = false;
            ui.spacing_mut().item_spacing.x = 2.0;
            for (title, entries) in menus() {
                ui.menu_button(title, |ui| self.menu_entries(ui, &ctx, &entries));
            }
        });
    }

    fn menu_entries(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, entries: &[Entry]) {
        ui.set_min_width(220.0);
        for e in entries {
            match e {
                Sep => {
                    ui.separator();
                }
                Sub(title, v) => {
                    ui.menu_button(*title, |ui| self.menu_entries(ui, ctx, v));
                }
                Item(a, label, sc) => {
                    let checked = self.action_checked(*a).unwrap_or(false);
                    let text = if checked {
                        format!("✓  {label}")
                    } else {
                        format!("     {label}")
                    };
                    let mut b = egui::Button::new(text);
                    if let Some(sc) = sc {
                        b = b.shortcut_text(ctx.format_shortcut(&sc.egui()));
                    }
                    if ui.add_enabled(self.action_enabled(*a), b).clicked() {
                        self.run_action(*a);
                        ui.close();
                    }
                }
            }
        }
    }

    pub fn zoom_by(&mut self, f: f32) {
        let rect = self.canvas_rect;
        if rect.width() > 0.0 {
            self.view.zoom_at(rect, rect.center(), f);
        }
    }

    pub fn native_menu_active(&self) -> bool {
        self.native_menu.is_some()
    }
}
