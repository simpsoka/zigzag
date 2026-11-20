//! Interface size (Settings › General): everything in the window drawn
//! larger or smaller, for a dense display, tired eyes or a finger.
//!
//! It's egui's zoom factor. Every size in the studio is in points, so
//! panels, text, icons, the handles on the canvas and how near a click has
//! to land all follow, with nothing to do widget by widget. Three things
//! are looked after here:
//!
//! - The design keeps the size it has on screen: its view is scaled the
//!   other way, in every tab, when the size changes.
//! - The window is never smaller than the layout needs, at any size.
//! - A size the display has no room for isn't used: the largest that fits
//!   is, until the window is on a display that has the room.

use crate::app::Studio;
use eframe::egui::{self, Vec2};

/// The smallest window the layout fits in, in points.
pub const MIN_WINDOW: Vec2 = Vec2::new(1000.0, 620.0);
/// The window as it first opens, in points at 100%.
pub const FIRST_WINDOW: Vec2 = Vec2::new(1360.0, 860.0);

/// The sizes offered, in percent.
pub const SIZES: [u32; 5] = [90, 100, 110, 125, 150];
pub const USUAL: u32 = 100;

/// A size in percent as a zoom factor.
pub fn factor(percent: u32) -> f32 {
    percent as f32 / 100.0
}

/// Whether the smallest window fits on a display of `monitor` (its size in
/// points at 100%) at `percent`. A tenth of the height is left for a menu
/// bar, a dock or a taskbar.
pub fn fits(percent: u32, monitor: Vec2) -> bool {
    let need = MIN_WINDOW * factor(percent);
    need.x <= monitor.x && need.y <= monitor.y * 0.9
}

/// The size used when `wanted` is asked for on `monitor`: `wanted`, or the
/// largest one under it the display has room for. (Only a size above 100%
/// is ever stepped down, and never below it: a small display at 100% is as
/// it always was.)
pub fn usable(wanted: u32, monitor: Option<Vec2>) -> u32 {
    let Some(monitor) = monitor else {
        return wanted;
    };
    if wanted <= USUAL {
        return wanted;
    }
    SIZES
        .into_iter()
        .rev()
        .find(|&p| p <= wanted && p > USUAL && fits(p, monitor))
        .unwrap_or(USUAL)
}

/// The window's first size and its smallest, for a size in percent (in the
/// window system's own points: nothing is zoomed before there's a window).
pub fn first_window(percent: u32) -> (Vec2, Vec2) {
    let min = MIN_WINDOW * factor(percent);
    (FIRST_WINDOW.max(min), min)
}

/// Before the first frame: the size from last time, and egui's own ⌘+ / ⌘−
/// / ⌘0 (which change the same zoom factor, unseen and unkept) off. Those
/// keys are the canvas's zoom here.
pub fn install(ctx: &egui::Context, percent: u32) {
    ctx.options_mut(|o| {
        o.zoom_with_keyboard = false;
        // (Set straight: `set_zoom_factor` is for a size changing between
        // two frames, and sizes the next one from the last.)
        o.zoom_factor = factor(percent);
    });
}

/// What `Studio::sync_ui_scale` keeps from frame to frame.
#[derive(Clone, Debug, Default)]
pub struct UiScale {
    /// The zoom factor the last frame was drawn at.
    seen: Option<f32>,
    /// The zoom factor the window's smallest size was last set for.
    min_for: Option<f32>,
    /// The display the window is on, in points at 100%.
    pub monitor: Option<Vec2>,
}

impl Studio {
    /// The interface size in use: what Settings says, or less on a display
    /// without the room.
    pub(crate) fn ui_size_in_use(&self) -> u32 {
        usable(self.prefs.ui_size, self.ui_scale.monitor)
    }

    /// Keep the interface at the size Settings says. Once a frame, before
    /// anything is laid out.
    pub(crate) fn sync_ui_scale(&mut self, ctx: &egui::Context) {
        let now = ctx.zoom_factor();
        if let Some(was) = self.ui_scale.seen.replace(now)
            && was != now
        {
            // The first frame at a new size. The design stays the size it
            // was on screen.
            let keep = was / now;
            self.view.rescale(keep);
            for t in &mut self.tabs.list {
                t.view.rescale(keep);
            }
            // (The window and the display were measured for this frame at
            // the old size: look at them on the next.)
            ctx.request_repaint();
            return;
        }
        let monitor = ctx.input(|i| i.viewport().monitor_size).map(|m| m * now);
        self.ui_scale.monitor = monitor;
        let want = factor(usable(self.prefs.ui_size, monitor));
        if want != now {
            ctx.set_zoom_factor(want);
            return;
        }
        if self.ui_scale.min_for != Some(now) {
            self.ui_scale.min_for = Some(now);
            // In points at this size: the layout's minimum, whatever that
            // is on the display.
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(MIN_WINDOW));
            let size = ctx.input(|i| i.viewport().inner_rect).map(|r| r.size());
            if let Some(size) = size
                && (size.x < MIN_WINDOW.x - 0.5 || size.y < MIN_WINDOW.y - 0.5)
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size.max(MIN_WINDOW)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    #[test]
    fn a_size_the_display_has_no_room_for_steps_down() {
        // A 16-inch laptop has room for every size; a 14-inch for 125%; a
        // 13-inch at its usual resolution too.
        let (big, mid, small) = (
            Vec2::new(1728.0, 1117.0),
            Vec2::new(1512.0, 982.0),
            Vec2::new(1440.0, 900.0),
        );
        assert_eq!(usable(150, Some(big)), 150);
        assert_eq!(usable(150, Some(mid)), 125);
        assert_eq!(usable(150, Some(small)), 125);
        assert_eq!(usable(125, Some(small)), 125);
        assert_eq!(usable(110, Some(small)), 110);
        // Nothing above 100% fits: 100%, not less.
        let tiny = Vec2::new(1024.0, 640.0);
        assert_eq!(usable(150, Some(tiny)), 100);
        // 100% and under are as asked, on any display; and so is any size
        // while the display isn't known.
        assert_eq!(usable(100, Some(tiny)), 100);
        assert_eq!(usable(90, Some(tiny)), 90);
        assert_eq!(usable(150, None), 150);
        // Every size offered is one `factor` and `first_window` agree on.
        for p in SIZES {
            let (first, min) = first_window(p);
            assert_eq!(min, MIN_WINDOW * factor(p));
            assert!(first.x >= min.x && first.y >= min.y);
        }
        assert_eq!(first_window(100), (FIRST_WINDOW, MIN_WINDOW));
    }

    fn harness() -> Harness<'static, Studio> {
        let path =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/badge.svg"));
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(Some(path), None),
            );
        h.run_steps(3);
        h
    }

    #[test]
    fn the_interface_follows_the_setting_and_the_design_keeps_its_size() {
        let mut h = harness();
        assert_eq!(h.ctx.zoom_factor(), 1.0);
        let before = h.state().view;
        // A second tab, behind: its view is kept too.
        h.state_mut().new_tab();
        h.run_steps(3);
        let behind = h.state().view;
        h.state_mut().prefs.ui_size = 125;
        h.run_steps(4);
        assert_eq!(h.ctx.zoom_factor(), 1.25);
        assert_eq!(h.state().ui_size_in_use(), 125);
        // A millimetre is as many pixels as it was: fewer points.
        let s = h.state();
        assert!((s.view.scale * 1.25 - behind.scale).abs() < 1e-3);
        assert_eq!(s.view.center, behind.center);
        let other = &s.tabs.list[1 - s.tabs.active];
        assert!((other.view.scale * 1.25 - before.scale).abs() < 1e-3);
        // And back.
        h.state_mut().prefs.ui_size = 100;
        h.run_steps(4);
        assert_eq!(h.ctx.zoom_factor(), 1.0);
        assert!((h.state().view.scale - behind.scale).abs() < 1e-3);
    }

    #[test]
    fn a_small_display_holds_the_interface_to_what_fits() {
        let mut h = harness();
        h.state_mut().prefs.ui_size = 150;
        let on = |h: &mut Harness<'_, Studio>, monitor: Vec2| {
            // (As the window system reports it: in points at the size in use.)
            for _ in 0..6 {
                let z = h.ctx.zoom_factor();
                h.input_mut()
                    .viewports
                    .get_mut(&egui::ViewportId::ROOT)
                    .unwrap()
                    .monitor_size = Some(monitor / z);
                h.run_steps(1);
            }
        };
        on(&mut h, Vec2::new(1440.0, 900.0));
        assert_eq!(h.ctx.zoom_factor(), 1.25);
        assert_eq!(h.state().ui_size_in_use(), 125);
        assert_eq!(h.state().prefs.ui_size, 150, "what was asked for is kept");
        // Moved to a display with the room: the size asked for.
        on(&mut h, Vec2::new(2560.0, 1440.0));
        assert_eq!(h.ctx.zoom_factor(), 1.5);
    }

    #[test]
    fn the_layout_holds_in_the_smallest_window_at_every_size() {
        // The smallest window is the same number of points at every size,
        // so what matters is that a frame lays out in it at each.
        for p in SIZES {
            let path = std::path::PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../examples/badge.svg"
            ));
            let mut s = Studio::with_file(Some(path), None);
            s.prefs.ui_size = p;
            let mut h = Harness::builder()
                .with_size(MIN_WINDOW)
                .build_ui_state(|ui, s: &mut Studio| s.frame(ui), s);
            h.run_steps(4);
            assert_eq!(h.ctx.zoom_factor(), factor(p));
        }
    }
}
