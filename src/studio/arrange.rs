//! Object › Align and Distribute: line the selection up by an edge or a
//! center, or space it evenly. Two or more objects line up with each
//! other; one object (or one group) lines up with the hoop. The buttons
//! are in the Transform panel too.

use crate::app::Studio;
use crate::commands::Action;
use crate::icons::Icon;
use crate::theme::icon_button;
use eframe::egui;
use zigzag::document::{AlignEdge, DistributeAxis};
use zigzag::geom::{Bounds, P};

pub fn align_icon(e: AlignEdge) -> Icon {
    match e {
        AlignEdge::Left => Icon::ObjAlignLeft,
        AlignEdge::HCenter => Icon::ObjAlignHCenter,
        AlignEdge::Right => Icon::ObjAlignRight,
        AlignEdge::Top => Icon::ObjAlignTop,
        AlignEdge::VCenter => Icon::ObjAlignVCenter,
        AlignEdge::Bottom => Icon::ObjAlignBottom,
    }
}

pub fn distribute_icon(a: DistributeAxis) -> Icon {
    match a {
        DistributeAxis::Horizontal => Icon::DistributeH,
        DistributeAxis::Vertical => Icon::DistributeV,
    }
}

impl Studio {
    /// The area the hoop sews, in design units, centered on the origin.
    pub(crate) fn hoop_bounds(&self) -> Bounds {
        let (w, h) = self.hoop_dims();
        Bounds {
            min: P::new(-w * 5.0, -h * 5.0),
            max: P::new(w * 5.0, h * 5.0),
        }
    }

    /// How many things the selection lines up as (a whole group is one).
    pub(crate) fn arrange_count(&self) -> usize {
        self.doc.arrange_count(&self.sel_indices())
    }

    /// Run an arranging change as one undo step (none if nothing moved),
    /// keeping the selection. Returns how many objects moved.
    fn arrange(
        &mut self,
        f: impl FnOnce(&mut zigzag::document::Document, &[usize]) -> anyhow::Result<usize>,
    ) -> anyhow::Result<usize> {
        let idx = self.sel_indices();
        let uids: Vec<usize> = idx.iter().map(|&i| self.doc.shapes[i].uid).collect();
        let redo = self.redo.clone();
        let mut moved = 0;
        self.structural(|d| {
            moved = f(d, &idx)?;
            Ok(())
        })?;
        if moved == 0 {
            // Nothing changed: no undo step, and redo as it was.
            self.undo.pop();
            self.redo = redo;
        }
        self.restore_selection(&uids);
        Ok(moved)
    }

    /// Object › Align: with each other, or (one object) with the hoop.
    pub fn align_selected(&mut self, edge: AlignEdge) {
        let to_hoop = self.arrange_count() == 1;
        let frame = to_hoop.then(|| self.hoop_bounds());
        let what = edge.label().trim_start_matches("Align ").to_lowercase();
        self.status = match self.arrange(|d, idx| d.align(idx, edge, frame)) {
            Ok(0) => format!("Already lined up ({what})."),
            Ok(n) if to_hoop => {
                let _ = n;
                format!("Aligned to the hoop ({what}).")
            }
            Ok(n) => format!(
                "Aligned {what}: moved {n} object{}.",
                if n == 1 { "" } else { "s" }
            ),
            Err(e) => format!("Couldn't align: {e:#}"),
        };
    }

    /// Object › Align › Distribute: equal gaps between three or more.
    pub fn distribute_selected(&mut self, axis: DistributeAxis) {
        let way = match axis {
            DistributeAxis::Horizontal => "across",
            DistributeAxis::Vertical => "down",
        };
        self.status = match self.arrange(|d, idx| d.distribute(idx, axis)) {
            Ok(0) => format!("Already evenly spaced {way}."),
            Ok(n) => format!(
                "Spaced evenly {way}: moved {n} object{}.",
                if n == 1 { "" } else { "s" }
            ),
            Err(e) => format!("Couldn't distribute: {e:#}"),
        };
    }

    /// The Transform panel's Align row: six align buttons, two distribute.
    /// Returns the one clicked.
    pub(crate) fn align_buttons(&self, ui: &mut egui::Ui) -> Option<Action> {
        let mut out = None;
        let to_hoop = self.arrange_count() == 1;
        ui.spacing_mut().item_spacing.x = 1.0;
        for e in AlignEdge::ALL {
            if e == AlignEdge::Top {
                ui.add_space(5.0);
            }
            let tip = if to_hoop {
                format!("{} (to the hoop)", e.label())
            } else {
                e.label().to_string()
            };
            if icon_button(ui, align_icon(e), e.label(), false, 22.0)
                .on_hover_text(tip)
                .clicked()
            {
                out = Some(Action::Align(e));
            }
        }
        ui.add_space(5.0);
        for a in DistributeAxis::ALL {
            let on = self.action_enabled(Action::Distribute(a));
            ui.add_enabled_ui(on, |ui| {
                if icon_button(ui, distribute_icon(a), a.label(), false, 22.0)
                    .on_hover_text(format!("{}: equal gaps between the objects", a.label()))
                    .on_disabled_hover_text("Select three or more objects to space them evenly")
                    .clicked()
                {
                    out = Some(Action::Distribute(a));
                }
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::shape_bounds;
    use eframe::egui::{Key, Modifiers, Vec2};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    fn studio() -> Harness<'static, Studio> {
        let path =
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/badge.svg"));
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(Some(path), None),
            );
        h.run_steps(4);
        h
    }

    fn index_of(h: &Harness<'_, Studio>, name: &str) -> usize {
        h.state().names.iter().position(|n| n == name).unwrap()
    }

    fn bounds(h: &Harness<'_, Studio>, name: &str) -> Bounds {
        shape_bounds(&h.state().doc.shapes[index_of(h, name)])
    }

    #[test]
    fn align_buttons_line_the_selection_up() {
        let mut h = studio();
        // (Room for the whole Transform panel in the test's window.)
        h.state_mut().dock.toggle(crate::dock::PanelId::Objects);
        let (sun, swoosh) = (index_of(&h, "sun"), index_of(&h, "swoosh"));
        // Nothing selected: nothing to align.
        assert!(!h.state().action_enabled(Action::Align(AlignEdge::Left)));
        h.state_mut().set_selection(&[sun, swoosh]);
        h.run_steps(3);
        assert!(h.state().action_enabled(Action::Align(AlignEdge::Left)));
        assert!(
            !h.state()
                .action_enabled(Action::Distribute(DistributeAxis::Horizontal)),
            "two objects can't be spaced evenly"
        );
        let left = bounds(&h, "sun").min.x.min(bounds(&h, "swoosh").min.x);
        h.get_by_label("Align Left").click();
        h.run_steps(4);
        assert!(
            (bounds(&h, "sun").min.x - left).abs() < 0.1,
            "{}",
            h.state().status
        );
        assert!((bounds(&h, "swoosh").min.x - left).abs() < 0.1);
        // Still selected, and one undo step.
        assert_eq!(h.state().sel_indices().len(), 2);
        assert_eq!(h.state().undo.len(), 1);
        // Already there: no second step.
        h.state_mut().run_action(Action::Align(AlignEdge::Left));
        assert_eq!(h.state().undo.len(), 1);
        assert!(h.state().status.starts_with("Already"));
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.run_steps(4);
        assert!((bounds(&h, "sun").min.x - left).abs() > 1.0);
    }

    #[test]
    fn one_object_aligns_to_the_hoop_and_three_distribute() {
        let mut h = studio();
        h.state_mut().dock.toggle(crate::dock::PanelId::Objects);
        let sun = index_of(&h, "sun");
        h.state_mut().set_selection(&[sun]);
        h.run_steps(2);
        let hoop = h.state().hoop_bounds();
        h.state_mut().run_action(Action::Align(AlignEdge::Top));
        h.run_steps(3);
        assert!((bounds(&h, "sun").min.y - hoop.min.y).abs() < 0.1);
        assert!(h.state().status.contains("hoop"), "{}", h.state().status);
        h.state_mut().run_action(Action::Align(AlignEdge::HCenter));
        assert!(bounds(&h, "sun").center().x.abs() < 0.1);
        // Three objects, spaced evenly down the hoop.
        let all: Vec<usize> = ["disc", "sun", "swoosh"]
            .iter()
            .map(|n| index_of(&h, n))
            .collect();
        h.state_mut().set_selection(&all);
        h.run_steps(3);
        assert!(
            h.state()
                .action_enabled(Action::Distribute(DistributeAxis::Vertical))
        );
        h.get_by_label("Distribute Vertically").click();
        h.run_steps(4);
        let mut bs: Vec<Bounds> = ["disc", "sun", "swoosh"]
            .iter()
            .map(|n| bounds(&h, n))
            .collect();
        bs.sort_by(|a, b| a.center().y.total_cmp(&b.center().y));
        let (g1, g2) = (bs[1].min.y - bs[0].max.y, bs[2].min.y - bs[1].max.y);
        assert!((g1 - g2).abs() < 0.1, "{g1} vs {g2}: {}", h.state().status);
        assert_eq!(h.state().sel_indices().len(), 3);
    }
}
