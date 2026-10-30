//! Completion barriers for UI tests that observe background stitch plans.

use crate::app::Studio;
use egui_kittest::Harness;
use std::time::{Duration, Instant};

pub(crate) fn finish_planning(h: &mut Harness<'_, Studio>) -> anyhow::Result<()> {
    let started = Instant::now();
    loop {
        // Admit edits and consume completed results through the real frame path.
        h.run_steps(1);
        if !h.state().dirty && !h.state().planner.pending {
            return Ok(());
        }
        anyhow::ensure!(
            started.elapsed() < Duration::from_secs(10),
            "stitch planning did not complete: dirty={}, pending={}, status={}",
            h.state().dirty,
            h.state().planner.pending,
            h.state().status
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
