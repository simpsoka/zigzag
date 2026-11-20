//! About ZigZag: which version this is and where it lives.
//! (ZigZag's own window rather than the system's About panel, whose text
//! can't carry links.)

use crate::app::Studio;
use eframe::egui;

/// ZigZag's home: the source, the releases and the issues.
pub const GITHUB_URL: &str = "https://github.com/simpsoka/zigzag";

/// ZZZ's home (the browser version): the source and the issues.
pub const ZZZ_URL: &str = "https://github.com/quilombodigital/zigzag";

/// Where Help's GitHub item and the issues go: ZZZ's in the browser.
pub fn home_url() -> &'static str {
    if cfg!(target_arch = "wasm32") {
        ZZZ_URL
    } else {
        GITHUB_URL
    }
}

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
                // In the browser this is ZZZ: ZigZag's studio, with the
                // credit and the link to where it comes from.
                if cfg!(target_arch = "wasm32") {
                    dialog::heading(ui, "ZZZ");
                    dialog::body(
                        ui,
                        &format!(
                            "Version {} · the ZigZag embroidery studio, in the browser",
                            env!("CARGO_PKG_VERSION")
                        ),
                    );
                    ui.add_space(crate::theme::space::S);
                    if ui
                        .link(egui::RichText::new("github.com/quilombodigital/zigzag").size(13.0))
                        .on_hover_text(ZZZ_URL)
                        .clicked()
                    {
                        crate::sew::open_url(ZZZ_URL);
                    }
                    ui.add_space(crate::theme::space::S);
                    dialog::body(ui, "Based on ZigZag:");
                } else {
                    dialog::heading(ui, "ZigZag");
                    dialog::body(
                        ui,
                        &format!(
                            "Version {} · SVG to machine embroidery",
                            env!("CARGO_PKG_VERSION")
                        ),
                    );
                    ui.add_space(crate::theme::space::S);
                }
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
