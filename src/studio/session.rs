//! Picking up where you left off: what's open (the file, its unsaved
//! changes, the view and the selection) is written to ZigZag's folder
//! as you work, and reopened the next time the app starts, whether it was
//! quit or it crashed.
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

#[derive(Default)]
pub struct SessionState {
    /// Where the session is kept (None: not kept, as in tests).
    dir: Option<PathBuf>,
    /// When the next write is due (egui time), if something changed.
    due: Option<f64>,
    /// The document in front as last written to its recovery file.
    saved_doc: Option<(u64, Snap)>,
    /// The recovery file of the document in front (see [`recovery_file`]);
    /// each tab has its own.
    pub(crate) rec: u32,
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
    pub(crate) fn swap_doc(&mut self, saved: &mut Option<(u64, Snap)>, rec: &mut u32) {
        std::mem::swap(&mut self.saved_doc, saved);
        std::mem::swap(&mut self.rec, rec);
    }

    /// A tab was closed: its unsaved changes go with it.
    pub(crate) fn forget_recovery(&mut self, rec: u32) {
        if let Some(f) = self.file(&recovery_file(rec)) {
            let _ = std::fs::remove_file(f);
        }
        if rec == self.rec {
            self.saved_doc = None;
        }
    }
}

/// Write `text` to `path` whole or not at all (a crash mid-write leaves the
/// old file).
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
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
        if let Some(wait) = self.keep_session_at(now, closing) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }
    }

    /// [`Studio::keep_session`] at time `now`: how long until a write is
    /// due, if one is waiting.
    fn keep_session_at(&mut self, now: f64, closing: bool) -> Option<f64> {
        if self.session.dir.is_none() || self.previewing() {
            return None;
        }
        if self.session.restoring {
            // A whole frame went by with the restored documents: they're safe.
            self.session.restoring = false;
            if let Some(m) = self.session.file(RESTORING) {
                let _ = std::fs::remove_file(m);
            }
        }
        // (The tabs not in front can only have changed on their way to the
        // back, and that changes which one is in front.)
        let changed = self.doc_unsaved_changed()
            || self.session.saved_meta.as_ref() != Some(&self.session_meta(self.modified));
        if changed && self.session.due.is_none() {
            self.session.due = Some(now + SETTLE_S);
        }
        let t = self.session.due?;
        if closing || now >= t {
            self.write_session();
            None
        } else {
            Some(t - now)
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
                    }
                    // Couldn't keep it: try again next change.
                    _ => return,
                }
            } else {
                let _ = std::fs::remove_file(&svg);
                self.session.saved_doc = None;
            }
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
                        }
                        _ => return,
                    }
                }
            } else if t.saved.take().is_some() {
                let _ = std::fs::remove_file(&f);
            }
        }
        let meta = self.session_meta(recovery);
        if let Ok(text) = serde_json::to_string_pretty(&meta)
            && write_atomic(&json, &text).is_ok()
        {
            self.session.saved_meta = Some(meta);
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
                Ok(doc) => Some(doc),
                Err(e) => {
                    // Unreadable: keep it rather than lose it, and say so.
                    let kept = aside(f, "unreadable");
                    let _ = std::fs::rename(f, &kept);
                    unreadable = Some(format!(
                        "Couldn't read the unsaved changes to {name} ({e:#}); they're kept in {}.",
                        kept.display()
                    ));
                    None
                }
            }
        });
        let opened = if let Some(doc) = recovered {
            self.set_document(doc);
            self.modified = true;
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
        let _ = self.keep_sent_task();
    }

    // Tests can join the real writer; dropping its handle keeps the UI asynchronous.
    fn keep_sent_task(&self) -> Option<std::thread::JoinHandle<()>> {
        let (Some(file), Some(sent)) = (self.session.file("sent.json"), &self.machine.sent) else {
            return None;
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
        match std::thread::Builder::new()
            .name("keep-sent".into())
            .spawn(write.clone())
        {
            Ok(task) => Some(task),
            Err(_) => {
                write();
                None
            }
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
        assert!(d.keep_session_at(0.0, false).is_none());
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
    fn the_design_on_the_machine_comes_back() -> anyhow::Result<()> {
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
        if let Some(task) = a.keep_sent_task() {
            task.join()
                .map_err(|_| anyhow::anyhow!("sent-design writer panicked"))?;
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
        Ok(())
    }

    #[test]
    fn edits_are_written_once_they_settle() {
        let dir = temp_dir("settle");
        let file = dir.join("cap.svg");
        std::fs::write(&file, SVG).unwrap();
        let mut a = studio_in(&dir);
        a.load(&file);
        let at = |a: &mut Studio, t: f64| a.keep_session_at(t, false);
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
        assert_eq!(a.keep_session_at(10.1, true), None);
        let mut b = studio_in(&dir);
        assert!(b.restore_session());
        assert_eq!(b.doc.shapes.len(), a.doc.shapes.len());
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
        b.keep_session_at(0.0, false);
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
