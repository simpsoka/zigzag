//! About ZigZag: which version this is and where it lives.
//! (ZigZag's own window rather than the system's About panel, whose text
//! can't carry links.)

use crate::app::Studio;
use eframe::egui;

/// ZigZag's home: the source, the releases and the issues.
pub const GITHUB_URL: &str = "https://github.com/simpsoka/zigzag";

impl Studio {
    /// The About window.
    pub(crate) fn about_window(&mut self, ctx: &egui::Context) {
        if !self.about {
            return;
        }
        use crate::dialog;
        let mut close = false;
        dialog::show(
            ctx,
            egui::Id::unique("about"),
            dialog::Place::Center,
            |ui| {
                dialog::heading(ui, "ZigZag");
                dialog::body(
                    ui,
                    &format!(
                        "Version {} · SVG to machine embroidery",
                        env!("CARGO_PKG_VERSION")
                    ),
                );
                ui.add_space(crate::theme::space::S);
                if ui
                    .link(egui::RichText::new("github.com/simpsoka/zigzag").size(13.0))
                    .on_hover_text(GITHUB_URL)
                    .clicked()
                {
                    crate::sew::open_url(GITHUB_URL);
                }
                dialog::buttons(ui, |ui| {
                    close = dialog::primary(ui, "OK", true).clicked()
                        || ui.input(|i| {
                            i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Escape)
                        });
                });
            },
        );
        if close {
            self.about = false;
        }
    }
}
