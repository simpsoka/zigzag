//! Picking up where you left off: what's open (the file, its unsaved
//! changes, the view and the selection) is written to ZigZag's folder
//! as you work, and reopened the next time the app starts, whether it was
//! quit or it crashed. A file opened with the app at launch opens on top
//! of what comes back, rather than in its place.
//!
//! `session.json` says which document and how it was shown; `recovery.svg`
//! holds the document itself while it has unsaved changes (written as Save
//! would, so it opens the same way).
//!
//! `sent.json` is the design last sent to the Skitch, so a job survives the
//! app too: at the next launch the Skitch window reopens if it was open,
//! reconnects, and when the machine reports that same design, its progress
//! shows on it as before (the machine itself keeps the design and where
//! the needle is).
//!
//! If the app went down while bringing the last session back (so the
//! document itself may be what crashed it), the next launch doesn't try
//! again: it keeps the recovered document aside as `recovery-crashed.svg`
//! and says so, rather than crashing at every launch.

use crate::app::{Snap, Studio, snapshot_of};
use crate::canvas::View;
use std::path::{Path, PathBuf};
use zigzag::design::{Block, Design, Run};
use zigzag::document::Document;
use zigzag::geom::P;

/// How long edits wait before they're written (a burst of them, one write).
const SETTLE_S: f64 = 1.5;
/// A write waits for the hand to be off the mouse for this long: it stops
/// the frame for as long as the disk takes to say the file is safe (ten or
/// twenty milliseconds each, on a fast one), which shows as a stutter in
/// the middle of a drag, a pan or a zoom.
const QUIET_S: f64 = 0.4;
/// But a change never waits longer than this, however long the gesture.
const LONGEST_S: f64 = 10.0;

#[derive(Default)]
pub struct SessionState {
    /// Where the session is kept (None: not kept, as in tests).
    dir: Option<PathBuf>,
    /// When the next write is due (egui time), if something changed.
    due: Option<f64>,
    /// A document that isn't in front was changed (an agent working on
    /// it): that's to be written too.
    pub(crate) behind: bool,
    /// When the change that's waiting to be written was noticed.
    since: Option<f64>,
    /// The document in front as last written to its recovery file.
    saved_doc: Option<(u64, Snap)>,
    /// The recovery file of the document in front (see [`recovery_file`]);
    /// each tab has its own.
    pub(crate) rec: u32,
    /// The document in front's plan as kept beside its recovery file.
    pub(crate) plan: KeptPlan,
    /// What `session.json` last said.
    saved_meta: Option<serde_json::Value>,
    /// The last session is being brought back: the first frame that
    /// completes clears the `restoring` marker.
    restoring: bool,
    /// The Skitch window was open with a design on the machine: reopen it
    /// (and reconnect) once there's an egui context.
    pub resume_skitch: bool,
}

/// Marks that a restore is under way (present at launch: the last one
/// never finished).
const RESTORING: &str = "restoring";

/// A tab's recovery file: `recovery.svg` for the first, `recovery-N.svg`
/// for the others.
pub(crate) fn recovery_file(rec: u32) -> String {
    if rec == 0 {
        "recovery.svg".into()
    } else {
        format!("recovery-{rec}.svg")
    }
}

/// A recovery file's kept plan (see `plan_keep.rs`): `recovery-3.plan`
/// beside `recovery-3.svg`.
fn plan_file(svg: &Path) -> PathBuf {
    svg.with_extension("plan")
}

/// Where a document's plan stands with the file kept beside its recovery
/// file (each tab has its own).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct KeptPlan {
    /// The `plan_keep::key_of` of its recovery file as written (or read).
    pub(crate) key: Option<u64>,
    /// Its plan for that is kept (or was tried and couldn't be).
    pub(crate) kept: bool,
}

impl KeptPlan {
    /// The recovery file was just written with `text`.
    fn written(text: &str) -> KeptPlan {
        KeptPlan {
            key: Some(crate::plan_keep::key_of(text)),
            kept: false,
        }
    }

    /// Whether there's a plan to keep, given that `current` says the plan
    /// shown is the document's as it is.
    fn wanted(&self, current: bool) -> bool {
        current && self.key.is_some() && !self.kept
    }

    /// Keep `design` (and its `issues`) beside the recovery file `svg`.
    fn keep(&mut self, svg: &Path, offset: P, design: &Design, issues: &[zigzag::checks::Issue]) {
        let Some(key) = self.key else {
            return;
        };
        // (Tried once whatever happens: a disk that won't take it isn't
        // asked again at every write.)
        self.kept = true;
        let bytes = crate::plan_keep::encode(key, offset, design, issues);
        let _ = write_atomic_as(&plan_file(svg), &bytes, false);
    }
}

/// A recovery file and the plan kept beside it go.
fn remove_recovery(svg: &Path) {
    let _ = std::fs::remove_file(svg);
    let _ = std::fs::remove_file(plan_file(svg));
}

/// The recovery number in a recovery file's name.
fn recovery_number(name: &str) -> Option<u32> {
    if name == "recovery.svg" {
        return Some(0);
    }
    name.strip_prefix("recovery-")?
        .strip_suffix(".svg")?
        .parse()
        .ok()
}

/// `recovery-3.svg` kept aside as `recovery-3-crashed.svg`.
fn aside(f: &Path, why: &str) -> PathBuf {
    let stem = f
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recovery".into());
    f.with_file_name(format!("{stem}-{why}.svg"))
}

impl SessionState {
    /// Kept in ZigZag's settings folder.
    pub fn in_config() -> SessionState {
        SessionState::in_dir(crate::dock::config_file("session.json").and_then(|f| {
            let d = f.parent()?.to_path_buf();
            Some(d)
        }))
    }

    pub fn in_dir(dir: Option<PathBuf>) -> SessionState {
        SessionState {
            dir,
            ..Default::default()
        }
    }

    fn file(&self, name: &str) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(name))
    }

    /// Swap what's kept about the document in front with a tab's (see
    /// `tabs.rs`).
    pub(crate) fn swap_doc(
        &mut self,
        saved: &mut Option<(u64, Snap)>,
        rec: &mut u32,
        plan: &mut KeptPlan,
    ) {
        std::mem::swap(&mut self.saved_doc, saved);
        std::mem::swap(&mut self.rec, rec);
        std::mem::swap(&mut self.plan, plan);
    }

    /// A tab was closed: its unsaved changes go with it.
    pub(crate) fn forget_recovery(&mut self, rec: u32) {
        if let Some(f) = self.file(&recovery_file(rec)) {
            remove_recovery(&f);
        }
        if rec == self.rec {
            self.saved_doc = None;
            self.plan = KeptPlan::default();
        }
    }
}

/// Write `text` to `path` whole or not at all (a crash mid-write leaves the
/// old file).
pub(crate) fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    write_atomic_as(path, text.as_bytes(), false)
}

/// `write_atomic`, for a file only this user may read (on Unix `0600`,
/// whatever the umask; elsewhere the settings folder is the user's own).
pub(crate) fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    write_atomic_as(path, text.as_bytes(), true)
}

fn write_atomic_as(path: &Path, text: &[u8], private: bool) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut open = std::fs::OpenOptions::new();
        open.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            if private {
                open.mode(0o600);
            }
        }
        let mut f = open.open(&tmp)?;
        #[cfg(unix)]
        {
            // (A leftover temporary file keeps its mode when opened again.)
            use std::os::unix::fs::PermissionsExt;
            if private {
                f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
        }
        #[cfg(not(unix))]
        let _ = private;
        f.write_all(text)?;
        // On disk before it takes the old file's place: a power cut or a
        // kernel panic right after the rename can't leave an empty file.
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// What `session.json` says about one tab.
fn tab_meta(
    doc: &Document,
    recovery: bool,
    rec: u32,
    view: &View,
    selected: Option<usize>,
) -> serde_json::Value {
    serde_json::json!({
        "path": doc.path.as_ref().map(|p| p.to_string_lossy()),
        "name": doc.name,
        "recovery": recovery,
        "recovery_file": recovery_file(rec),
        "view": [view.center.x, view.center.y, view.scale],
        "selected": selected,
    })
}

impl Studio {
    fn session_meta(&self, recovery: bool) -> serde_json::Value {
        let front = tab_meta(
            &self.doc,
            recovery,
            self.session.rec,
            &self.view,
            self.sel_index(),
        );
        let tabs: Vec<serde_json::Value> = (0..self.tab_count())
            .map(|i| {
                if i == self.tabs.active {
                    front.clone()
                } else {
                    let t = &self.tabs.list[i];
                    tab_meta(&t.doc, t.modified, t.rec, &t.view, t.sel_index())
                }
            })
            .collect();
        // The document in front at the top level too (what older versions
        // read).
        let mut meta = front;
        meta["skitch"] = (self.machine.open && self.machine.sent.is_some()).into();
        meta["tabs"] = tabs.into();
        meta["active"] = self.tabs.active.into();
        meta
    }

    /// Call every frame: notes changes and writes them once they settle
    /// (or at once, when the window is closing).
    pub(crate) fn keep_session(&mut self, ctx: &eframe::egui::Context) {
        let now = ctx.input(|i| i.time);
        let closing = ctx.input(|i| i.viewport().close_requested());
        // A gesture under way: a button held, or the view being scrolled
        // or zoomed.
        let busy = ctx.input(|i| {
            i.pointer.any_down()
                || i.smooth_scroll_delta != eframe::egui::Vec2::ZERO
                || i.zoom_delta() != 1.0
        });
        if let Some(wait) = self.keep_session_at(now, closing, busy) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }
    }

    /// [`Studio::keep_session`] at time `now`: how long until a write is
    /// due, if one is waiting. `busy`: a gesture is under way, and the
    /// write waits for it to end (see [`QUIET_S`]).
    fn keep_session_at(&mut self, now: f64, closing: bool, busy: bool) -> Option<f64> {
        if self.session.dir.is_none() || self.previewing() {
            return None;
        }
        // A whole frame went by with the restored documents: they're safe.
        self.restore_survived();
        // (The tabs not in front can only have changed on their way to the
        // back, and that changes which one is in front.)
        let changed = std::mem::take(&mut self.session.behind)
            || self.doc_unsaved_changed()
            || (self.modified && self.session.plan.wanted(self.plan_is_current()))
            || self.session.saved_meta.as_ref() != Some(&self.session_meta(self.modified));
        if changed && self.session.due.is_none() {
            self.session.due = Some(now + SETTLE_S);
            self.session.since = Some(now);
        }
        let t = self.session.due?;
        if !closing
            && busy
            && now + QUIET_S > t
            && self.session.since.is_some_and(|s| now - s < LONGEST_S)
        {
            self.session.due = Some(now + QUIET_S);
            return Some(QUIET_S);
        }
        if closing || now >= t {
            self.write_session();
            None
        } else {
            Some(t - now)
        }
    }

    /// On the way out: write what the last moments changed, without
    /// waiting for it to settle.
    pub(crate) fn flush_session(&mut self) {
        // (Documents just restored, not yet through a frame, stay as kept.)
        if !self.session.restoring {
            self.keep_session_at(f64::INFINITY, true, false);
        }
    }

    /// Unsaved changes that the recovery file doesn't have yet.
    fn doc_unsaved_changed(&self) -> bool {
        if !self.modified {
            return self.session.saved_doc.is_some();
        }
        match &self.session.saved_doc {
            Some((rev, snap)) => *rev != self.doc.rev() || *snap != snapshot_of(&self.doc),
            None => true,
        }
    }

    /// Write the session now.
    pub(crate) fn write_session(&mut self) {
        self.session.due = None;
        self.session.since = None;
        let (Some(json), Some(svg)) = (
            self.session.file("session.json"),
            self.session.file(&recovery_file(self.session.rec)),
        ) else {
            return;
        };
        let recovery = self.modified;
        if self.doc_unsaved_changed() {
            if recovery {
                match self.doc.to_svg() {
                    Ok(text) if write_atomic(&svg, &text).is_ok() => {
                        self.session.saved_doc = Some((self.doc.rev(), snapshot_of(&self.doc)));
                        self.session.plan = KeptPlan::written(&text);
                    }
                    // Couldn't keep it: try again next change.
                    _ => return,
                }
            } else {
                remove_recovery(&svg);
                self.session.saved_doc = None;
                self.session.plan = KeptPlan::default();
            }
        }
        // Its stitches as planned for what's in the recovery file, once
        // they are.
        if recovery && self.session.plan.wanted(self.plan_is_current()) {
            let offset = self.doc.offset;
            self.session
                .plan
                .keep(&svg, offset, &self.design, &self.issues);
        }
        // The tabs at the back, each in its own file.
        let active = self.tabs.active;
        for (i, t) in self.tabs.list.iter_mut().enumerate() {
            if i == active {
                continue;
            }
            let Some(f) = self.session.file(&recovery_file(t.rec)) else {
                continue;
            };
            if t.modified {
                let stale = t
                    .saved
                    .as_ref()
                    .is_none_or(|(rev, snap)| *rev != t.doc.rev() || *snap != snapshot_of(&t.doc));
                if stale {
                    match t.doc.to_svg() {
                        Ok(text) if write_atomic(&f, &text).is_ok() => {
                            t.saved = Some((t.doc.rev(), snapshot_of(&t.doc)));
                            t.plan = KeptPlan::written(&text);
                        }
                        _ => return,
                    }
                }
                // (A tab at the back was planned when it was last in front,
                // unless it's been changed since.)
                if t.plan.wanted(t.plan_is_current()) {
                    let (design, issues) = t.plan_shown();
                    let mut plan = t.plan;
                    plan.keep(&f, t.doc.offset, design, issues);
                    t.plan = plan;
                }
            } else if t.saved.take().is_some() {
                remove_recovery(&f);
                t.plan = KeptPlan::default();
            }
        }
        let meta = self.session_meta(recovery);
        if let Ok(text) = serde_json::to_string_pretty(&meta)
            && write_atomic(&json, &text).is_ok()
        {
            self.session.saved_meta = Some(meta);
        }
    }

    /// At launch: what was open last time comes back (when `reopen`), and
    /// a file asked for (on the command line, or opened with ZigZag from
    /// the desktop) opens on top of it, in a tab of its own, or its tab
    /// comes forward if it was open already. Either way nothing that was
    /// open is lost.
    pub(crate) fn start_with(&mut self, paths: &[PathBuf], select: Option<usize>, reopen: bool) {
        let mut notice = None;
        if reopen {
            let before = self.status.clone();
            self.restore_session();
            // What the restore had to say (a session it wouldn't reopen,
            // changes it couldn't read) still shows once the file is open.
            if self.status != before
                && (self.status.contains("went down") || self.status.contains("Couldn't"))
            {
                notice = Some(self.status.clone());
            }
        }
        if !paths.is_empty() {
            // The tabs that came back are all in; should a file asked for
            // take the app down, that's the file's doing, not the session's,
            // so the next launch still brings the session back.
            self.restore_survived();
            for p in paths {
                self.load(p);
            }
            if let Some(i) = select.filter(|&i| i < self.doc.shapes.len()) {
                self.select_index(Some(i));
            }
            if let Some(n) = notice {
                self.status = format!("{} {n}", self.status);
            }
        }
    }

    /// The documents just restored got through: no longer marked as being
    /// restored (see [`RESTORING`]).
    fn restore_survived(&mut self) {
        if self.session.restoring {
            self.session.restoring = false;
            if let Some(m) = self.session.file(RESTORING) {
                let _ = std::fs::remove_file(m);
            }
        }
    }

    /// Reopen what was open last time: every tab, the one in front in
    /// front. False if there was nothing to open.
    pub(crate) fn restore_session(&mut self) -> bool {
        let Some(meta) = self
            .session
            .file("session.json")
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        else {
            return false;
        };
        // (A session from before tabs: just the one document.)
        let entries: Vec<serde_json::Value> = match meta["tabs"].as_array() {
            Some(v) if !v.is_empty() => v.clone(),
            _ => vec![meta.clone()],
        };
        let front = (meta["active"].as_u64().unwrap_or(0) as usize).min(entries.len() - 1);
        let rec_of = |e: &serde_json::Value| {
            e["recovery_file"]
                .as_str()
                .and_then(recovery_number)
                .unwrap_or(0)
        };
        let name_of = |e: &serde_json::Value| e["name"].as_str().unwrap_or("untitled").to_string();
        let names = entries.iter().map(name_of).collect::<Vec<_>>();
        let what = if names.len() == 1 {
            names[0].clone()
        } else {
            format!("your {} documents", names.len())
        };
        let marker = self.session.file(RESTORING);
        if let Some(m) = marker.as_ref().filter(|m| m.exists()) {
            // The last launch went down bringing this back. Don't go round
            // again: keep what there was, aside, and start fresh.
            let _ = std::fs::remove_file(m);
            let kept: Vec<PathBuf> = entries
                .iter()
                .filter(|e| e["recovery"].as_bool().unwrap_or(false))
                .filter_map(|e| {
                    let svg = self.session.file(&recovery_file(rec_of(e)))?;
                    let aside = aside(&svg, "crashed");
                    let _ = std::fs::remove_file(plan_file(&svg));
                    std::fs::rename(&svg, &aside).ok().map(|_| aside)
                })
                .collect();
            self.session.saved_doc = None;
            self.session.saved_meta = None;
            self.status = match kept.as_slice() {
                [] => format!(
                    "The app went down last time while reopening {what}, so it wasn't reopened."
                ),
                [one] => format!(
                    "The app went down last time while reopening {what}, so it wasn't reopened. \
                     Its unsaved changes are kept in {}.",
                    one.display()
                ),
                many => format!(
                    "The app went down last time while reopening {what}, so they weren't reopened. \
                     Their unsaved changes are kept in {}.",
                    many[0]
                        .parent()
                        .map(|d| d.display().to_string())
                        .unwrap_or_default()
                ),
            };
            return false;
        }
        if let Some(m) = &marker {
            let _ = std::fs::write(m, names.join("\n"));
            self.session.restoring = true;
        }
        self.tabs.next_rec = entries
            .iter()
            .map(|e| rec_of(e) + 1)
            .max()
            .unwrap_or(1)
            .max(1);
        let mut opened = 0;
        let mut front_tab = None;
        let mut notes: Vec<String> = Vec::new();
        for (k, e) in entries.iter().enumerate() {
            if opened > 0 {
                self.new_tab();
            }
            self.session.rec = rec_of(e);
            if self.restore_tab(e) {
                opened += 1;
                if k == front {
                    front_tab = Some(self.tabs.active);
                }
                if self.status.contains("Couldn't") {
                    notes.push(self.status.clone());
                }
            } else if opened > 0 {
                // Nothing to show in the tab made for it.
                if self.status.contains("Couldn't") {
                    notes.push(self.status.clone());
                }
                self.discard_tab(self.tabs.active);
            }
        }
        if opened == 0 {
            self.session.restoring = false;
            if let Some(m) = &marker {
                let _ = std::fs::remove_file(m);
            }
            return false;
        }
        if let Some(i) = front_tab {
            self.switch_tab(i);
        }
        if opened > 1 && notes.is_empty() {
            self.status = format!("Picked up where you left off: {opened} documents.");
        } else if let Some(n) = notes.into_iter().next() {
            self.status = n;
        }
        self.session.resume_skitch = meta["skitch"].as_bool().unwrap_or(false);
        self.session.saved_meta = Some(self.session_meta(self.modified));
        true
    }

    /// Reopen one tab from its `session.json` entry into the document in
    /// front (an empty one). False if there was nothing to open.
    fn restore_tab(&mut self, e: &serde_json::Value) -> bool {
        let path = e["path"].as_str().map(PathBuf::from);
        let name = e["name"].as_str().unwrap_or("untitled").to_string();
        let recovery = e["recovery"].as_bool().unwrap_or(false);
        let recovery_file = recovery
            .then(|| self.session.file(&recovery_file(self.session.rec)))
            .flatten();
        let mut unreadable = None;
        let recovered = recovery_file.as_ref().and_then(|f| {
            let xml = std::fs::read_to_string(f).ok()?;
            match Document::from_svg(&xml, &name, path.clone()) {
                Ok(doc) => {
                    // Its stitches as they were, if they were kept for it.
                    let key = crate::plan_keep::key_of(&xml);
                    let kept = std::fs::read(plan_file(f)).ok().and_then(|b| {
                        crate::plan_keep::decode(&b, key, doc.shapes.len(), doc.offset)
                    });
                    Some((doc, key, kept))
                }
                Err(e) => {
                    // Unreadable: keep it rather than lose it, and say so.
                    let kept = aside(f, "unreadable");
                    let _ = std::fs::remove_file(plan_file(f));
                    let _ = std::fs::rename(f, &kept);
                    unreadable = Some(format!(
                        "Couldn't read the unsaved changes to {name} ({e:#}); they're kept in {}.",
                        kept.display()
                    ));
                    None
                }
            }
        });
        let opened = if let Some((doc, key, kept)) = recovered {
            self.set_document(doc);
            self.modified = true;
            self.session.plan = KeptPlan {
                key: Some(key),
                kept: kept.is_some(),
            };
            if let Some((design, issues)) = kept {
                // Shown while it's planned again, on the worker: the first
                // plan of a launch can take seconds (its letters split for
                // the first time).
                self.design = design;
                self.issues = issues;
                self.names = zigzag::svg::display_names(&self.doc.shapes);
                self.replans += 1;
                self.planner.expect_slow();
            }
            self.status =
                format!("Picked up where you left off: {name}, with its unsaved changes.");
            true
        } else if let Some(p) = path.filter(|p| p.exists()) {
            self.load(&p);
            self.status = unreadable
                .take()
                .unwrap_or_else(|| format!("Picked up where you left off: {name}."));
            !self.tab_is_blank() || self.doc.path.is_some()
        } else {
            false
        };
        if !opened {
            return false;
        }
        if let Some(v) = e["view"].as_array()
            && let [Some(x), Some(y), Some(s)] = [0, 1, 2].map(|k| v.get(k)?.as_f64())
            && s > 0.0
        {
            self.view = View {
                center: P { x, y },
                scale: s as f32,
            };
            self.fit_pending = false;
        }
        if let Some(i) = e["selected"].as_u64() {
            self.select_index(Some(i as usize).filter(|&i| i < self.doc.shapes.len()));
        }
        // What's on disk is what's open: nothing to write until it changes.
        self.session.saved_doc = self
            .modified
            .then(|| (self.doc.rev(), snapshot_of(&self.doc)));
        self.dirty = true;
        true
    }
}

// ------------------------------------------------- the design on the machine

/// A stitch plan as JSON (its blocks: what drawing it and matching the
/// machine's progress to it need), read back exactly.
fn design_json(d: &Design) -> serde_json::Value {
    serde_json::json!({
        "name": d.name,
        "blocks": d.blocks.iter().map(|b| serde_json::json!({
            "color": b.color,
            "runs": b.runs.iter().map(|r| serde_json::json!({
                "trim": r.trim_before,
                "pts": r.pts.iter().flat_map(|p| [p.x, p.y]).collect::<Vec<f64>>(),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}

fn design_from_json(v: &serde_json::Value) -> Option<Design> {
    let color = |c: &serde_json::Value| -> Option<[u8; 3]> {
        let c = c.as_array()?;
        Some([
            c.first()?.as_u64()? as u8,
            c.get(1)?.as_u64()? as u8,
            c.get(2)?.as_u64()? as u8,
        ])
    };
    let blocks = v["blocks"]
        .as_array()?
        .iter()
        .map(|b| {
            let runs = b["runs"]
                .as_array()?
                .iter()
                .map(|r| {
                    let xs: Vec<f64> = r["pts"]
                        .as_array()?
                        .iter()
                        .map(|x| x.as_f64())
                        .collect::<Option<_>>()?;
                    Some(Run {
                        pts: xs.chunks_exact(2).map(|c| P::new(c[0], c[1])).collect(),
                        trim_before: r["trim"].as_bool().unwrap_or(false),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(Block {
                color: color(&b["color"])?,
                runs,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Design {
        name: v["name"].as_str().unwrap_or_default().to_string(),
        blocks,
        ..Design::default()
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<[u8; 16]> {
    if s.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

impl Studio {
    /// Keep what was just sent to the machine (`sent.json`), so the job
    /// can be picked up if the app quits or crashes. Written off the UI
    /// thread: a big design is a few megabytes of JSON.
    pub(crate) fn keep_sent(&self) {
        let (Some(file), Some(sent)) = (self.session.file("sent.json"), &self.machine.sent) else {
            return;
        };
        let (design, uuid, centered) = (sent.design.clone(), sent.uuid, sent.centered);
        let write = move || {
            let v = serde_json::json!({
                "uuid": hex(&uuid),
                "centered": centered,
                "design": design_json(&design),
            });
            if let Ok(text) = serde_json::to_string(&v) {
                let _ = write_atomic(&file, &text);
            }
        };
        if std::thread::Builder::new()
            .name("keep-sent".into())
            .spawn(write.clone())
            .is_err()
        {
            write();
        }
    }

    /// The job is over (the design was taken off the machine).
    pub(crate) fn forget_sent(&self) {
        if let Some(f) = self.session.file("sent.json") {
            let _ = std::fs::remove_file(f);
        }
    }

    /// Bring back the design last sent to the machine, if the job wasn't
    /// ended. The machine's progress on it shows again once it's connected
    /// and reports the same design.
    pub(crate) fn restore_sent(&mut self) -> bool {
        let Some(v) = self
            .session
            .file("sent.json")
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        else {
            return false;
        };
        let (Some(uuid), Some(design)) = (
            v["uuid"].as_str().and_then(unhex),
            design_from_json(&v["design"]),
        ) else {
            return false;
        };
        if design.blocks.is_empty() {
            return false;
        }
        let pen = zigzag::formats::pen::encode(&design);
        self.machine.sent = Some(crate::machine::Sent {
            pen: std::sync::Arc::new(pen),
            bounds: design.bounds(),
            design: std::sync::Arc::new(design),
            uuid,
            centered: v["centered"].as_bool().unwrap_or(false),
        });
        self.machine.follow = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn studio_in(dir: &Path) -> Studio {
        let mut s = Studio::with_file(None, None);
        s.session = SessionState::in_dir(Some(dir.to_path_buf()));
        s
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tl-session-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="60mm" height="40mm" viewBox="0 0 60 40">
<rect id="box" x="5" y="5" width="30" height="20" fill="#c0392b"/>
<circle id="dot" cx="45" cy="20" r="8" fill="#2c3e50"/>
</svg>"##;

    #[test]
    fn unsaved_changes_come_back_after_a_crash() {
        let dir = temp_dir("crash");
        let file = dir.join("logo.svg");
        std::fs::write(&file, SVG).unwrap();

        let mut a = studio_in(&dir);
        a.load(&file);
        a.replan();
        let n = a.doc.shapes.len();
        a.structural(|d| d.delete(0)).unwrap();
        a.select_index(Some(0));
        a.view.scale = 3.25;
        a.write_session();
        // (No save: the app went away.)
        drop(a);

        let mut b = studio_in(&dir);
        assert!(b.restore_session());
        assert_eq!(b.doc.shapes.len(), n - 1, "the delete came back");
        assert!(b.modified, "still unsaved");
        assert_eq!(
            b.doc.path.as_deref(),
            Some(file.as_path()),
            "saves to the same file"
        );
        assert_eq!(b.doc.name, "logo");
        assert_eq!(b.view.scale, 3.25);
        assert_eq!(b.sel_index(), Some(0));
        assert!(!b.fit_pending);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            SVG,
            "the file itself is untouched"
        );
    }

    /// A document that crashes the app while it's being brought back is
    /// not brought back again at the next launch: it's kept aside instead.
    #[test]
    fn a_restore_that_crashed_is_not_tried_again() {
        let dir = temp_dir("loop");
        let file = dir.join("logo.svg");
        std::fs::write(&file, SVG).unwrap();
        let mut a = studio_in(&dir);
        a.load(&file);
        a.structural(|d| d.delete(0)).unwrap();
        a.write_session();
        drop(a);

        // Launch 1: restores, then goes down before its first frame ends.
        let mut b = studio_in(&dir);
        assert!(b.restore_session());
        assert!(dir.join(RESTORING).exists(), "marked while restoring");
        drop(b);

        // Launch 2: doesn't reopen it; keeps the unsaved changes aside.
        let mut c = studio_in(&dir);
        assert!(!c.restore_session(), "not reopened");
        assert!(c.status.contains("went down last time"), "{}", c.status);
        assert!(dir.join("recovery-crashed.svg").exists());
        assert!(!dir.join("recovery.svg").exists());
        assert!(!dir.join(RESTORING).exists());
        assert_eq!(c.doc.shapes.len(), 0, "starts fresh");

        // A restore that gets through its first frame clears the mark.
        std::fs::rename(dir.join("recovery-crashed.svg"), dir.join("recovery.svg")).unwrap();
        let mut d = studio_in(&dir);
        assert!(d.restore_session());
        assert!(d.keep_session_at(0.0, false, false).is_none());
        assert!(!dir.join(RESTORING).exists(), "cleared after a frame");
        let mut e = studio_in(&dir);
        assert!(e.restore_session(), "reopens as usual");
        assert_eq!(e.doc.shapes.len(), 1);
    }

    /// Unsaved changes that can't be read back aren't thrown away.
    #[test]
    fn an_unreadable_recovery_is_kept_aside() {
        let dir = temp_dir("unreadable");
        let file = dir.join("logo.svg");
        std::fs::write(&file, SVG).unwrap();
        let mut a = studio_in(&dir);
        a.load(&file);
        a.structural(|d| d.delete(0)).unwrap();
        a.write_session();
        std::fs::write(dir.join("recovery.svg"), "<svg").unwrap();
        let mut b = studio_in(&dir);
        assert!(b.restore_session(), "the file itself still opens");
        assert_eq!(b.doc.shapes.len(), 2);
        assert!(b.status.contains("kept in"), "{}", b.status);
        assert!(dir.join("recovery-unreadable.svg").exists());
    }

    /// The design on the machine comes back after a restart, and is
    /// recognized as the machine's (same id, same stitches) so its
    /// progress can show on it again.
    #[test]
    fn the_design_on_the_machine_comes_back() {
        let dir = temp_dir("sent");
        let file = dir.join("logo.svg");
        std::fs::write(&file, SVG).unwrap();
        let mut a = studio_in(&dir);
        a.load(&file);
        a.replan();
        let pen = zigzag::formats::pen::encode(&a.design);
        a.machine
            .send(&a.design, pen, &eframe::egui::Context::default());
        let sent = a.machine.sent.clone().unwrap();
        a.keep_sent();
        // (Written on a thread.)
        for _ in 0..200 {
            if dir.join("sent.json").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        drop(a);

        let mut b = studio_in(&dir);
        assert!(b.restore_sent());
        let back = b.machine.sent.as_ref().unwrap();
        assert_eq!(back.uuid, sent.uuid);
        assert_eq!(back.pen.bytes, sent.pen.bytes, "the same stream");
        assert_eq!(back.pen.design_at, sent.pen.design_at);
        assert_eq!(back.design.stitch_count(), sent.design.stitch_count());
        // The machine reports that design: it's ours.
        let mut snap = b.machine.snapshot();
        snap.uuid = Some(sent.uuid);
        assert!(b.machine.ours(&snap).is_some());
        snap.uuid = Some([9; 16]);
        assert!(b.machine.ours(&snap).is_none());

        // Ending the job forgets it.
        b.forget_sent();
        let mut c = studio_in(&dir);
        assert!(!c.restore_sent());
    }

    #[test]
    fn edits_are_written_once_they_settle() {
        let dir = temp_dir("settle");
        let file = dir.join("cap.svg");
        std::fs::write(&file, SVG).unwrap();
        let mut a = studio_in(&dir);
        a.load(&file);
        let at = |a: &mut Studio, t: f64| a.keep_session_at(t, false, false);
        at(&mut a, 0.0);
        at(&mut a, 2.0);
        assert!(dir.join("session.json").exists(), "the open file is noted");
        a.structural(|d| d.delete(0)).unwrap();
        at(&mut a, 2.5);
        assert!(
            !dir.join("recovery.svg").exists(),
            "waits for the edits to settle"
        );
        at(&mut a, 2.5 + SETTLE_S);
        assert!(dir.join("recovery.svg").exists());
        // Closing writes at once.
        a.structural(|d| d.delete(0)).unwrap();
        assert!(at(&mut a, 10.0).is_some());
        assert_eq!(a.keep_session_at(10.1, true, false), None);
        let mut b = studio_in(&dir);
        assert!(b.restore_session());
        assert_eq!(b.doc.shapes.len(), a.doc.shapes.len());
    }

    #[test]
    fn a_write_waits_for_the_gesture_to_end() {
        let dir = temp_dir("quiet");
        let file = dir.join("cap.svg");
        std::fs::write(&file, SVG).unwrap();
        let mut a = studio_in(&dir);
        a.load(&file);
        a.keep_session_at(0.0, false, false);
        a.keep_session_at(2.0, false, false);
        a.structural(|d| d.delete(0)).unwrap();
        let rec = dir.join("recovery.svg");
        // Dragging on past the time the write was due: it waits.
        assert!(a.keep_session_at(3.0, false, true).is_some());
        assert!(a.keep_session_at(4.6, false, true).is_some());
        assert!(a.keep_session_at(6.0, false, true).is_some());
        assert!(!rec.exists(), "not in the middle of a drag");
        // The button's up: a moment later it's written.
        assert!(a.keep_session_at(6.1, false, false).is_some());
        assert!(!rec.exists());
        assert_eq!(a.keep_session_at(6.0 + QUIET_S, false, false), None);
        assert!(rec.exists());
        // A gesture that never ends doesn't keep a change waiting for ever.
        assert!(a.apply_history(crate::app::History::Undo));
        let before = std::fs::read_to_string(&rec).unwrap();
        let mut t = 10.0;
        while t < 10.0 + LONGEST_S + 1.0 {
            a.keep_session_at(t, false, true);
            t += 0.1;
        }
        assert_ne!(std::fs::read_to_string(&rec).unwrap(), before);
        // Closing never waits.
        assert!(a.apply_history(crate::app::History::Redo));
        let before = std::fs::read_to_string(&rec).unwrap();
        a.keep_session_at(30.0, false, true);
        assert_eq!(a.keep_session_at(30.1, true, true), None);
        assert_ne!(std::fs::read_to_string(&rec).unwrap(), before);
    }

    #[test]
    fn a_saved_document_just_reopens() {
        let dir = temp_dir("saved");
        let file = dir.join("hat.svg");
        std::fs::write(&file, SVG).unwrap();
        let mut a = studio_in(&dir);
        a.load(&file);
        a.structural(|d| d.delete(1)).unwrap();
        a.write_session();
        assert!(dir.join("recovery.svg").exists());
        a.save(None);
        a.write_session();
        assert!(
            !dir.join("recovery.svg").exists(),
            "saved: nothing to recover"
        );

        let mut b = studio_in(&dir);
        assert!(b.restore_session());
        assert!(!b.modified);
        assert_eq!(b.doc.shapes.len(), a.doc.shapes.len());
    }

    /// Every tab comes back, in order, each with its own unsaved changes,
    /// view and selection, and the one that was in front is in front.
    #[test]
    fn every_tab_comes_back() {
        let dir = temp_dir("tabs");
        let (one, two, three) = (
            dir.join("one.svg"),
            dir.join("two.svg"),
            dir.join("three.svg"),
        );
        for f in [&one, &two, &three] {
            std::fs::write(f, SVG).unwrap();
        }
        let mut a = studio_in(&dir);
        a.load(&one);
        a.structural(|d| d.delete(0)).unwrap();
        a.view.scale = 2.5;
        a.load(&two);
        a.select_index(Some(1));
        a.load(&three);
        a.structural(|d| d.delete(1)).unwrap();
        a.switch_tab(1);
        a.write_session();
        assert!(dir.join("recovery.svg").exists(), "one's changes");
        assert!(
            std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with("recovery-"))
                .count()
                == 1,
            "and three's, in a file of its own"
        );
        drop(a);

        let mut b = studio_in(&dir);
        assert!(b.restore_session());
        // (A first frame: the restore got through.)
        b.keep_session_at(0.0, false, false);
        let names: Vec<String> = b.tab_infos().into_iter().map(|t| t.name).collect();
        assert_eq!(names, ["one", "two", "three"]);
        assert_eq!(b.active_tab(), 1, "two in front, as it was");
        assert_eq!(b.sel_index(), Some(1));
        assert!(!b.modified);
        b.switch_tab(0);
        assert!(b.modified);
        assert_eq!(b.doc.shapes.len(), 1, "one's delete came back");
        assert_eq!(b.view.scale, 2.5);
        b.switch_tab(2);
        assert!(b.modified);
        assert_eq!(b.doc.shapes.len(), 1, "three's too");
        assert!(b.status.contains("3 documents"), "{}", b.status);

        // Closing a tab with changes, without saving, lets them go.
        b.discard_tab(2);
        b.write_session();
        let mut c = studio_in(&dir);
        assert!(c.restore_session());
        assert_eq!(c.tab_count(), 2);
        assert!(
            !std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .any(|e| e.file_name().to_string_lossy().starts_with("recovery-")),
            "three's recovery is gone"
        );
    }

    /// A document that comes back with unsaved changes shows the stitches
    /// it had straight away, kept beside its recovery file, while they're
    /// planned again; kept stitches that aren't for what's in the recovery
    /// file aren't shown.
    #[test]
    fn unsaved_changes_come_back_with_their_stitches() {
        let dir = temp_dir("kept-plan");
        let file = dir.join("logo.svg");
        std::fs::write(&file, SVG).unwrap();
        let mut a = studio_in(&dir);
        a.load(&file);
        a.structural(|d| d.delete(0)).unwrap();
        a.write_session();
        let plan = dir.join("recovery.plan");
        assert!(!plan.exists(), "not until it's planned");
        a.replan();
        assert!(a.plan_is_current());
        a.keep_session_at(0.0, false, false);
        a.keep_session_at(100.0, false, false);
        assert!(plan.exists(), "kept once it's planned");
        let issues = a.issues.clone();
        let pts = |d: &Design| -> Vec<P> {
            d.blocks
                .iter()
                .flat_map(|b| &b.runs)
                .flat_map(|r| r.pts.iter().copied())
                .collect()
        };
        let count = a.design.stitch_count();
        drop(a);

        let mut b = studio_in(&dir);
        assert!(b.restore_session());
        assert!(b.dirty, "planned again all the same");
        assert_eq!(b.design.stitch_count(), count, "shown at once");
        assert_eq!(b.issues, issues);
        // Where the document read back plans them (its artwork, one object
        // gone, is placed again when it's read).
        let shown = pts(&b.design);
        b.replan();
        let now = pts(&b.design);
        assert_eq!(now.len(), shown.len());
        let worst = now
            .iter()
            .zip(&shown)
            .map(|(p, q)| (*p - *q).len())
            .fold(0.0, f64::max);
        assert!(worst < 1e-6, "{worst}");
        // (A first frame: the restore got through.)
        b.keep_session_at(0.0, false, false);
        drop(b);

        // The recovery file changed (and its plan wasn't kept again): what's
        // kept is for something else.
        let svg = dir.join("recovery.svg");
        let text = std::fs::read_to_string(&svg).unwrap();
        std::fs::write(&svg, text.replace("#2c3e50", "#27ae60")).unwrap();
        let mut c = studio_in(&dir);
        assert!(c.restore_session());
        c.keep_session_at(0.0, false, false);
        assert_eq!(c.design.stitch_count(), 0, "nothing kept for this");

        // Its changes let go: the kept plan goes with them.
        c.replan();
        c.write_session();
        assert!(plan.exists());
        c.discard_tab(0);
        c.write_session();
        assert!(!plan.exists() && !svg.exists());
    }

    /// Bringing such a tab forward shows its stitches in that frame, and
    /// plans it again on the worker rather than holding the window up,
    /// however quick the plans of the tab it came from were.
    #[test]
    fn a_tab_with_kept_stitches_comes_forward_at_once() {
        use eframe::egui::Vec2;
        let dir = temp_dir("kept-switch");
        let (one, two) = (dir.join("one.svg"), dir.join("two.svg"));
        std::fs::write(&one, SVG).unwrap();
        std::fs::write(&two, SVG).unwrap();
        let mut a = studio_in(&dir);
        a.load(&one);
        a.load(&two);
        a.structural(|d| d.delete(0)).unwrap();
        a.replan();
        a.write_session();
        a.switch_tab(0);
        a.replan();
        a.write_session();
        assert!(
            std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .any(|e| e.file_name().to_string_lossy().ends_with(".plan")),
            "two's stitches kept"
        );
        drop(a);

        let mut b = studio_in(&dir);
        assert!(b.restore_session());
        assert_eq!(b.active_tab(), 0);
        let mut h = egui_kittest::Harness::builder()
            .with_size(Vec2::new(1400.0, 900.0))
            .build_ui_state(|ui, s: &mut Studio| s.frame(ui), b);
        h.state_mut().planner.timed = true;
        h.run_steps(3);
        // (One's plans are quick: they're made here.)
        h.state_mut().planner.took(1.0);
        h.state_mut().tabs.pending = Some(crate::tabs::TabOp::Switch(1));
        h.step();
        let s = h.state();
        assert_eq!(s.active_tab(), 1);
        assert!(s.design.stitch_count() > 0, "its stitches show");
        assert!(s.planner.pending, "planned again on the worker");
        for _ in 0..400 {
            h.step();
            if !h.state().planner.pending {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!h.state().planner.pending && !h.state().dirty);
    }

    /// Opening a file with ZigZag (from the command line, or the desktop's
    /// Open With) doesn't lose the tabs that were open: they come back,
    /// with the file on top; one already open just comes forward.
    #[test]
    fn opening_a_file_at_launch_keeps_the_other_tabs() {
        let dir = temp_dir("launchfile");
        let (one, two, three) = (
            dir.join("one.svg"),
            dir.join("two.svg"),
            dir.join("three.svg"),
        );
        for f in [&one, &two, &three] {
            std::fs::write(f, SVG).unwrap();
        }
        let mut a = studio_in(&dir);
        a.load(&one);
        a.structural(|d| d.delete(0)).unwrap();
        a.load(&two);
        a.write_session();
        drop(a);

        let mut b = studio_in(&dir);
        b.start_with(std::slice::from_ref(&three), Some(1), true);
        b.keep_session_at(0.0, false, false);
        let names: Vec<String> = b.tab_infos().into_iter().map(|t| t.name).collect();
        assert_eq!(names, ["one", "two", "three"]);
        assert_eq!(b.doc.name, "three", "the file asked for is in front");
        assert_eq!(b.sel_index(), Some(1));
        b.switch_tab(0);
        assert!(b.modified, "one's unsaved changes are still there");
        b.write_session();
        drop(b);

        // A file that's open already comes forward: no second tab.
        let mut c = studio_in(&dir);
        c.start_with(std::slice::from_ref(&one), None, true);
        assert_eq!(c.tab_count(), 3);
        assert_eq!(c.doc.name, "one");
        assert!(c.modified);

        // A script taking pictures of a file gets just that file.
        let mut d = studio_in(&dir);
        d.start_with(std::slice::from_ref(&two), None, false);
        assert_eq!(d.tab_count(), 1);
        assert_eq!(d.doc.name, "two");
    }

    /// A file opened at launch that takes the app down isn't blamed on the
    /// session that came back with it; and when the session isn't reopened
    /// (the launch before went down restoring it), opening the file doesn't
    /// hide why.
    #[test]
    fn a_file_opened_at_launch_and_a_crash() {
        let dir = temp_dir("launchcrash");
        let (one, two) = (dir.join("one.svg"), dir.join("two.svg"));
        for f in [&one, &two] {
            std::fs::write(f, SVG).unwrap();
        }
        let mut a = studio_in(&dir);
        a.load(&one);
        a.structural(|d| d.delete(0)).unwrap();
        a.write_session();
        drop(a);

        // The session is in before the file is opened: no longer marked.
        let mut b = studio_in(&dir);
        b.start_with(std::slice::from_ref(&two), None, true);
        assert!(!dir.join(RESTORING).exists(), "{}", b.status);
        assert_eq!(b.tab_count(), 2);
        drop(b);

        // Went down restoring (marked, no frame): the next launch with a
        // file says so beside "Opened".
        let mut c = studio_in(&dir);
        assert!(c.restore_session());
        drop(c);
        let mut d = studio_in(&dir);
        d.start_with(std::slice::from_ref(&two), None, true);
        assert!(d.status.starts_with("Opened"), "{}", d.status);
        assert!(d.status.contains("went down last time"), "{}", d.status);
        assert_eq!(d.doc.name, "two");
    }

    /// A session written before tabs (one document) still opens.
    #[test]
    fn a_session_from_before_tabs_opens() {
        let dir = temp_dir("oldformat");
        let file = dir.join("logo.svg");
        std::fs::write(&file, SVG).unwrap();
        let meta = serde_json::json!({
            "path": file.to_string_lossy(),
            "name": "logo",
            "recovery": false,
            "view": [0.0, 0.0, 1.5],
            "selected": null,
            "skitch": false,
        });
        std::fs::write(dir.join("session.json"), meta.to_string()).unwrap();
        let mut a = studio_in(&dir);
        assert!(a.restore_session());
        assert_eq!(a.tab_count(), 1);
        assert_eq!(a.doc.name, "logo");
        assert_eq!(a.view.scale, 1.5);
    }

    #[test]
    fn untitled_work_comes_back_and_nothing_is_nothing() {
        let dir = temp_dir("untitled");
        let mut fresh = studio_in(&dir);
        assert!(!fresh.restore_session(), "first run: nothing to reopen");

        let mut a = studio_in(&dir);
        let svg = Document::from_svg(SVG, "drawing", None).unwrap();
        a.set_document(svg);
        a.modified = true;
        a.write_session();

        let mut b = studio_in(&dir);
        assert!(b.restore_session());
        assert!(b.modified);
        assert_eq!(b.doc.path, None);
        assert_eq!(b.doc.shapes.len(), 2);

        // An untitled document with nothing in it isn't worth reopening.
        let mut c = studio_in(&dir);
        c.write_session();
        let mut d = studio_in(&dir);
        assert!(!d.restore_session());
    }
}
