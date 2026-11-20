//! A frame profiler for development: with `TL_PROFILE=1` in the
//! environment, every frame prints the sections of [`crate::app::Studio::frame`]
//! that took longer than a few hundred microseconds, in order, to stderr.
//! (`TL_PROFILE=quiet` times the sections without printing them, for the
//! benches, which read them with [`last`].) Off (and nearly free) otherwise.

use std::cell::RefCell;
use web_time::Instant;

thread_local! {
    static FRAME: RefCell<Option<Frame>> = const { RefCell::new(None) };
}

struct Frame {
    start: Instant,
    last: Instant,
    rows: Vec<(&'static str, f64)>,
}

thread_local! {
    static WHOLE: std::cell::Cell<Option<f32>> = const { std::cell::Cell::new(None) };
    /// The sections of the frames since [`clear`] (egui may run the UI
    /// twice for one frame on screen).
    static LAST: RefCell<Vec<(&'static str, f64)>> = const { RefCell::new(Vec::new()) };
}

/// Forget the sections kept for [`last`].
#[cfg(test)]
pub fn clear() {
    LAST.with(|l| l.borrow_mut().clear());
}

/// How long the frames since [`clear`] spent in the section called `what`
/// (ms; 0 when they had none, or nothing is being timed).
#[cfg(test)]
pub fn last(what: &str) -> f64 {
    LAST.with(|l| {
        l.borrow()
            .iter()
            .filter(|(w, _)| *w == what)
            .map(|(_, ms)| ms)
            .sum()
    })
}

fn quiet() -> bool {
    static QUIET: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *QUIET.get_or_init(|| std::env::var_os("TL_PROFILE").is_some_and(|v| v == "quiet"))
}

/// What eframe measured for the previous frame as a whole (UI, egui's
/// tessellation and the painting), seconds.
pub fn whole_frame(cpu_usage: Option<f32>) {
    if enabled() {
        WHOLE.with(|w| w.set(cpu_usage));
    }
}

pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("TL_PROFILE").is_some_and(|v| !v.is_empty() && v != "0"))
}

/// Start timing a frame.
pub fn begin() {
    if !enabled() {
        return;
    }
    let now = Instant::now();
    FRAME.with(|f| {
        *f.borrow_mut() = Some(Frame {
            start: now,
            last: now,
            rows: Vec::new(),
        })
    });
}

/// Note that the section since the last mark (or the frame start) is `what`.
pub fn mark(what: &'static str) {
    if !enabled() {
        return;
    }
    FRAME.with(|f| {
        if let Some(fr) = f.borrow_mut().as_mut() {
            let now = Instant::now();
            fr.rows
                .push((what, now.duration_since(fr.last).as_secs_f64() * 1000.0));
            fr.last = now;
        }
    });
}

/// Print the frame's sections (those over 0.3 ms) and total.
pub fn end() {
    if !enabled() {
        return;
    }
    FRAME.with(|f| {
        if let Some(fr) = f.borrow_mut().take() {
            let total = fr.start.elapsed().as_secs_f64() * 1000.0;
            if quiet() {
                LAST.with(|l| l.borrow_mut().extend(fr.rows));
                return;
            }
            let whole = WHOLE
                .with(|w| w.get())
                .map(|s| format!(" (last whole frame {:.2} ms)", s * 1000.0))
                .unwrap_or_default();
            let mut line = format!("frame {total:6.2} ms{whole}:");
            for (what, ms) in &fr.rows {
                if *ms >= 0.3 {
                    line.push_str(&format!("  {what} {ms:.2}"));
                }
            }
            eprintln!("{line}");
        }
    });
}
