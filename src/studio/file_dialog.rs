//! Open, Place, Save As and Export panels.
//!
//! On macOS a blocking `NSSavePanel` / `NSOpenPanel` (`rfd::FileDialog`)
//! runs its own modal loop from inside winit's event handler. AppKit can
//! spin the default run-loop mode during that loop, which runs the events
//! winit queued meanwhile (mouse motion over the panel, say); those re-enter
//! winit's handler, which panics inside an `extern "C"` callback, and the app
//! aborts ("panic in a function that cannot unwind", under `save_file`).
//!
//! So on macOS the panel is a sheet on the window instead: it returns at
//! once, and the choice comes back through the ordinary event loop, picked
//! up by [`Studio::poll_file_dialog`] on a later frame. Elsewhere the
//! blocking dialogs work, and stay as they were (answered the same frame).

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use crate::app::Studio;

/// What to do with the path once it's chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Then {
    Open,
    Place,
    SaveAs,
    Export(&'static str),
}

/// A panel to show: the open kind, or the save kind.
pub(crate) struct Request {
    save: bool,
    filters: Vec<(String, Vec<String>)>,
    file_name: Option<String>,
    directory: Option<PathBuf>,
}

impl Request {
    pub fn open() -> Self {
        Self {
            save: false,
            filters: Vec::new(),
            file_name: None,
            directory: None,
        }
    }

    pub fn save() -> Self {
        Self {
            save: true,
            ..Self::open()
        }
    }

    pub fn filter(mut self, name: impl Into<String>, exts: &[&str]) -> Self {
        self.filters
            .push((name.into(), exts.iter().map(|e| e.to_string()).collect()));
        self
    }

    pub fn file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = Some(name.into());
        self
    }

    pub fn directory(mut self, dir: Option<PathBuf>) -> Self {
        self.directory = dir;
        self
    }

    /// The blocking dialog (not on macOS: see the module notes).
    #[cfg(not(target_os = "macos"))]
    fn run(self) -> Option<PathBuf> {
        let mut d = rfd::FileDialog::new();
        for (name, exts) in &self.filters {
            d = d.add_filter(name, exts);
        }
        if let Some(n) = &self.file_name {
            d = d.set_file_name(n);
        }
        if let Some(dir) = &self.directory {
            d = d.set_directory(dir);
        }
        if self.save {
            d.save_file()
        } else {
            d.pick_file()
        }
    }

    /// A sheet on the window, answered later.
    #[cfg(target_os = "macos")]
    fn begin(self) -> Pin<Box<dyn Future<Output = Option<PathBuf>>>> {
        let mut d = rfd::AsyncFileDialog::new();
        for (name, exts) in &self.filters {
            d = d.add_filter(name, exts);
        }
        if let Some(n) = &self.file_name {
            d = d.set_file_name(n);
        }
        if let Some(dir) = &self.directory {
            d = d.set_directory(dir);
        }
        let path = |h: Option<rfd::FileHandle>| h.map(|h| h.path().to_path_buf());
        if self.save {
            let f = d.save_file();
            Box::pin(async move { path(f.await) })
        } else {
            let f = d.pick_file();
            Box::pin(async move { path(f.await) })
        }
    }
}

/// A panel on screen, or one answered and not yet acted on.
pub(crate) struct Pending {
    then: Then,
    answer: Answer,
}

enum Answer {
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Ready(Option<PathBuf>),
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Waiting(Pin<Box<dyn Future<Output = Option<PathBuf>>>>),
}

/// Wakes the UI when the sheet closes (the frame is idle until then).
struct Repaint(Option<eframe::egui::Context>);

impl Wake for Repaint {
    fn wake(self: Arc<Self>) {
        if let Some(ctx) = &self.0 {
            ctx.request_repaint();
        }
    }
}

impl Studio {
    /// Show a panel; `then` runs with the chosen path. Nothing happens while
    /// another is open.
    pub(crate) fn file_dialog(&mut self, req: Request, then: Then) {
        if self.file_dialog.is_some() {
            return;
        }
        #[cfg(target_os = "macos")]
        let answer = Answer::Waiting(req.begin());
        #[cfg(not(target_os = "macos"))]
        let answer = Answer::Ready(req.run());
        self.file_dialog = Some(Pending { then, answer });
        self.poll_file_dialog();
    }

    /// A panel is open: the window is the panel's until it closes.
    pub(crate) fn file_dialog_open(&self) -> bool {
        self.file_dialog.is_some()
    }

    /// Act on the panel's answer once there is one.
    pub(crate) fn poll_file_dialog(&mut self) {
        let Some(mut p) = self.file_dialog.take() else {
            return;
        };
        let path = match &mut p.answer {
            Answer::Ready(path) => path.take(),
            Answer::Waiting(f) => {
                let waker = Waker::from(Arc::new(Repaint(self.egui_ctx.clone())));
                match f.as_mut().poll(&mut Context::from_waker(&waker)) {
                    Poll::Ready(path) => path,
                    Poll::Pending => {
                        self.file_dialog = Some(p);
                        return;
                    }
                }
            }
        };
        let Some(path) = path else {
            return;
        };
        match p.then {
            Then::Open => self.load(&path),
            Then::Place => self.place_file(&path),
            Then::SaveAs => self.save(Some(&path)),
            Then::Export(ext) => {
                self.status = match self.export_to(ext, &path) {
                    Ok(()) => format!("Exported {}", path.display()),
                    Err(e) => format!("Couldn't export {}: {e}", path.display()),
                };
            }
        }
        self.dirty = true;
    }
}
