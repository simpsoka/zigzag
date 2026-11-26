//! Stitch planning off the UI thread, for designs big enough to need it.
//!
//! Small designs plan in a few milliseconds and stay synchronous (what you
//! see is always the plan of what's there). Once a plan takes longer than
//! [`SLOW_MS`], edits hand the work to a worker thread with its own
//! [`PlanCache`]: the canvas keeps showing the last plan (the status bar
//! says it's updating) and swaps the new one in when it's done. Only the
//! latest request matters: the worker skips any it's already behind on.
//! An agent's edit asked for a short answer is planned there too, its
//! answer sent once the plan is back (see `Studio::handle_mcp`); tools
//! that read the stitches plan here, since the agent reads the result.

use crate::app::Studio;
use eframe::egui;
use std::sync::mpsc::{self, Receiver, Sender};
use zigzag::checks::{self, Issue};
use zigzag::design::{Design, PlanCache, PlanOptions};
use zigzag::svg::Shape;

/// A plan slower than this (ms) moves planning to the worker. Under this,
/// a plan fits in a frame beside drawing it; over it, every edit (each
/// keystroke, each slider step) would stall the UI for that long.
pub const SLOW_MS: f64 = 12.0;

struct Job {
    generation: u64,
    name: String,
    shapes: Vec<Shape>,
    opt: PlanOptions,
    hoop: (f64, f64),
}

struct Done {
    generation: u64,
    /// The plan, or what went wrong (an internal error: the last plan stays).
    result: Result<(Design, Vec<Issue>), String>,
    ms: f64,
}

/// What a caught panic said.
pub(crate) fn panic_text(e: &Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<String>()
        .cloned()
        .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "internal error".into())
}

#[derive(Default)]
pub struct Planner {
    jobs: Option<Sender<Job>>,
    done: Option<Receiver<Done>>,
    /// The generation of the newest request (results from older ones are dropped).
    generation: u64,
    /// A request is out and not back yet.
    pub pending: bool,
    /// The last plan was slow: plan in the background from now on.
    pub slow: bool,
    /// The last plan asked of the worker failed: what's shown (and
    /// checked) is from before the edit it was for.
    pub failed: bool,
    /// In tests: go by how long plans take, as the app does. Otherwise a
    /// test's plans are made here, where it reads them straight away,
    /// unless it sets `slow` itself: a debug build on a busy machine is
    /// slow at random, and the plan would still be with the worker.
    #[cfg(test)]
    pub timed: bool,
}

impl Planner {
    fn start(&mut self, ctx: &egui::Context) {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (done_tx, done_rx) = mpsc::channel::<Done>();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("stitch-planner".into())
            .spawn(move || {
                let mut cache = PlanCache::default();
                while let Ok(mut job) = job_rx.recv() {
                    // Only the newest request is worth planning.
                    while let Ok(newer) = job_rx.try_recv() {
                        job = newer;
                    }
                    let t = std::time::Instant::now();
                    let result = zigzag::design::caught(|| {
                        let design = zigzag::design::plan_cached(
                            &job.name,
                            &job.shapes,
                            &job.opt,
                            &mut cache,
                        );
                        let issues = checks::check(&job.shapes, &design, &job.opt, Some(job.hoop));
                        (design, issues)
                    })
                    .map_err(|e| panic_text(&e));
                    let ms = t.elapsed().as_secs_f64() * 1000.0;
                    if done_tx
                        .send(Done {
                            generation: job.generation,
                            result,
                            ms,
                        })
                        .is_err()
                    {
                        return;
                    }
                    ctx.request_repaint();
                }
            })
            .ok();
        self.jobs = Some(job_tx);
        self.done = Some(done_rx);
    }

    /// A plan took `ms`: a slow one sends the next ones to the worker.
    pub fn took(&mut self, ms: f64) {
        #[cfg(test)]
        if !self.timed {
            self.slow = false;
            return;
        }
        self.slow = ms > SLOW_MS;
    }

    /// The next plan may be slow, however quick the last one was (the
    /// text has a new font, say): it's the worker's.
    pub fn expect_slow(&mut self) {
        #[cfg(test)]
        if !self.timed {
            return;
        }
        self.slow = true;
    }

    /// A document came to the front: whether its plans are slow.
    pub fn entered(&mut self, slow: bool) {
        #[cfg(test)]
        if !self.timed {
            return;
        }
        self.slow = slow;
    }

    /// A synchronous plan was just made: anything still out is stale.
    pub fn superseded(&mut self) {
        self.generation += 1;
        self.pending = false;
    }
}

impl Studio {
    /// Plan the stitches for the document as it is now: right away, or (for
    /// a big design) on the worker, the current plan staying until it's done.
    pub fn plan_changes(&mut self, ctx: &egui::Context) {
        if !self.planner.slow {
            self.replan();
            return;
        }
        if self.planner.jobs.is_none() {
            self.planner.start(ctx);
        }
        self.planner.generation += 1;
        let job = Job {
            generation: self.planner.generation,
            name: self.doc.name.clone(),
            shapes: self.doc.shapes.clone(),
            opt: self.doc.plan_options(),
            hoop: self.hoop_dims(),
        };
        let sent = self
            .planner
            .jobs
            .as_ref()
            .is_some_and(|tx| tx.send(job).is_ok());
        if !sent {
            // No worker after all: plan here.
            self.planner.jobs = None;
            self.replan();
            return;
        }
        // Names are cheap, and the lists show them straight away.
        self.names = self.doc.object_names();
        self.planner.pending = true;
        self.dirty = false;
    }

    /// Bring the plan up to date with the document, here and now: before
    /// the stitches are written out, sent to the machine or read by an
    /// agent. (Otherwise an edit still with the worker, or made this
    /// frame, would be missing from them.)
    pub fn plan_now(&mut self) {
        // (A plan with stitches taken out for show isn't one to sew.)
        if self.dirty || self.planner.pending || self.plan_cut {
            self.replan();
        }
    }

    /// Take in a plan the worker finished (if it's still the newest).
    pub fn take_plan(&mut self) {
        let Some(rx) = &self.planner.done else {
            return;
        };
        let mut newest = None;
        while let Ok(d) = rx.try_recv() {
            if d.generation == self.planner.generation {
                newest = Some(d);
            }
        }
        let Some(d) = newest else {
            return;
        };
        self.planner.pending = false;
        self.planner.failed = d.result.is_err();
        let (design, issues) = match d.result {
            Ok(x) => x,
            Err(e) => {
                self.status = format!("Couldn't update the stitches ({e}); showing the last plan.");
                return;
            }
        };
        let was = self.design.stitch_count();
        self.design = design;
        self.issues = issues;
        self.follow_plan(was);
        self.plan_ms = d.ms;
        self.planner.took(d.ms);
        self.planner.pending = false;
        self.replans += 1;
        self.plan_cut = false;
        self.hide_planned_text();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Vec2;
    use egui_kittest::Harness;

    fn studio() -> Harness<'static, Studio> {
        let path =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/badge.svg"));
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 900.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(Some(path), None),
            );
        h.run_steps(3);
        h
    }

    fn wait(h: &mut Harness<'_, Studio>) {
        for _ in 0..400 {
            h.run_steps(1);
            if !h.state().planner.pending {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the worker never answered");
    }

    #[test]
    fn a_slow_design_plans_on_the_worker() {
        let mut h = studio();
        let before = h.state().design.stitch_count();
        // Pretend the last plan was slow: the next edit goes to the worker.
        h.state_mut().planner.slow = true;
        let sun = h.state().names.iter().position(|n| n == "sun").unwrap();
        h.state_mut().doc.shapes[sun].params.kind = Some(zigzag::params::Kind::Skip);
        h.state_mut().dirty = true;
        h.run_steps(1);
        assert!(h.state().planner.pending, "planning in the background");
        assert_eq!(
            h.state().design.stitch_count(),
            before,
            "the old plan shows meanwhile"
        );
        wait(&mut h);
        assert_ne!(
            h.state().design.stitch_count(),
            before,
            "the new plan arrived"
        );
        // A plan made here meanwhile wins over a stale one still out.
        h.state_mut().planner.slow = true;
        h.state_mut().doc.shapes[sun].params.kind = Some(zigzag::params::Kind::Fill);
        h.state_mut().dirty = true;
        h.run_steps(1);
        assert!(h.state().planner.pending);
        h.state_mut().replan();
        let now = h.state().design.stitch_count();
        assert!(!h.state().planner.pending);
        std::thread::sleep(std::time::Duration::from_millis(200));
        h.run_steps(2);
        assert_eq!(
            h.state().design.stitch_count(),
            now,
            "the stale result was dropped"
        );
    }
}
