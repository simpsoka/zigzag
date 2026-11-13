//! Machines: the ones ZigZag comes with and the ones you add (remembered
//! between launches), and the Document panel's Machine rows. Choosing one
//! copies its settings into the document and makes it the machine new
//! documents start with.

use crate::app::{HOOPS, Studio, standard_hoop};
use crate::theme::Form;
use eframe::egui;
use std::path::PathBuf;
use zigzag::document::{DEFAULT_MIN_STITCH_MM, Document, Settings};
use zigzag::formats::{Lock, LockStyle};
use zigzag::machine::profile::{self, Profile};

#[derive(Clone, Debug, Default)]
pub struct Machines {
    /// Machines of your own.
    pub mine: Vec<Profile>,
    /// The machine new documents start with (an id), if any.
    pub current: Option<String>,
    path: Option<PathBuf>,
}

impl Machines {
    /// Your machines from last time; saved from now on.
    pub fn load() -> Machines {
        Self::load_from(crate::dock::config_file("machines.json"))
    }

    pub(crate) fn load_from(path: Option<PathBuf>) -> Machines {
        let mut m = Machines {
            path: path.clone(),
            ..Default::default()
        };
        let Some(v) = path
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        else {
            // Nothing kept yet (a new install): new documents start on the
            // first of ZigZag's machines, the Skitch, until another is
            // chosen.
            m.current = profile::builtin().first().map(|p| p.id.clone());
            return m;
        };
        // (One of yours can't take a built-in machine's id.)
        m.mine = profile::parse_list(&v)
            .into_iter()
            .filter(|p| profile::builtin().iter().all(|b| b.id != p.id))
            .collect();
        m.current = v["current"]
            .as_str()
            .filter(|id| m.find(id).is_some())
            .map(str::to_string);
        m
    }

    pub fn save(&self) {
        let Some(path) = &self.path else { return };
        let v = serde_json::json!({
            "current": self.current,
            "machines": self.mine.iter().map(Profile::to_json).collect::<Vec<_>>(),
        });
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, serde_json::to_string_pretty(&v).unwrap_or_default());
    }

    /// Every machine: ZigZag's, then yours.
    pub fn all(&self) -> impl Iterator<Item = &Profile> {
        profile::builtin().iter().chain(&self.mine)
    }

    pub fn find(&self, id: &str) -> Option<&Profile> {
        self.all().find(|p| p.id == id)
    }

    /// The machine with this id, or (any case) this name.
    pub fn named(&self, name: &str) -> Option<&Profile> {
        let n = name.trim();
        self.find(n)
            .or_else(|| self.all().find(|p| p.name.eq_ignore_ascii_case(n)))
    }

    pub fn is_mine(&self, id: &str) -> bool {
        self.mine.iter().any(|p| p.id == id)
    }

    fn mine_mut(&mut self, id: &str) -> Option<&mut Profile> {
        self.mine.iter_mut().find(|p| p.id == id)
    }

    /// Add a machine of your own called `name`, with one hoop. Returns its
    /// id (the name's, numbered if another machine has it).
    pub fn add(&mut self, name: &str, hoop: (f64, f64)) -> String {
        let base = profile::slug(name);
        let mut id = base.clone();
        let mut n = 2;
        while self.find(&id).is_some() {
            id = format!("{base}-{n}");
            n += 1;
        }
        self.mine.push(Profile::new(id.clone(), name, hoop));
        self.save();
        id
    }

    /// [`Machines::add`] from a document: its hoop, and its shortest
    /// stitch and locks where they aren't the usual.
    pub fn add_from(&mut self, name: &str, s: &Settings) -> String {
        let id = self.add(name, s.hoop);
        if let Some(m) = self.mine_mut(&id) {
            take_stitch_rules(m, s);
        }
        self.save();
        id
    }

    /// Forget one of your machines.
    pub fn remove(&mut self, id: &str) {
        self.mine.retain(|p| p.id != id);
        if self.current.as_deref() == Some(id) {
            self.current = None;
        }
        self.save();
    }
}

impl Machines {
    /// Put one of your machines, changed, in place of what it was (by its
    /// id). False if it isn't one of yours: ZigZag's own aren't changed.
    pub fn replace(&mut self, p: Profile) -> bool {
        match self.mine_mut(&p.id.clone()) {
            Some(m) => {
                *m = p;
                true
            }
            None => false,
        }
    }

    /// A machine of your own that starts as a copy of machine `id` (one of
    /// ZigZag's or one of yours). Its id, or `None` if there's no such
    /// machine.
    pub fn duplicate(&mut self, id: &str) -> Option<String> {
        let from = self.find(id)?.clone();
        let name = format!("{} copy", from.name);
        let new = self.add(&name, from.default_hoop);
        if let Some(m) = self.mine_mut(&new) {
            *m = Profile {
                id: new.clone(),
                name,
                // (Yours now: where its values came from, and whether
                // they've been sewn with, is no longer ZigZag's to say.)
                source: String::new(),
                tested: false,
                ..from
            };
        }
        self.save();
        Some(new)
    }
}

/// Make a document's shortest stitch and locks a machine's (none where
/// they're the usual ones).
fn take_stitch_rules(m: &mut Profile, s: &Settings) {
    m.min_stitch_mm = (s.min_stitch_mm != DEFAULT_MIN_STITCH_MM).then_some(s.min_stitch_mm);
    m.lock = (s.lock != Lock::default()).then_some(s.lock);
}

/// A hoop's name in menus: a standard hoop's, or its size.
pub(crate) fn hoop_name((w, h): (f64, f64)) -> String {
    match standard_hoop((w, h)) {
        Some(x) => x.1.to_string(),
        None => crate::units::fmt_size(w * 10.0, h * 10.0),
    }
}

impl Studio {
    /// The document's machine, if this computer knows it.
    pub fn machine(&self) -> Option<&Profile> {
        self.machines.find(self.doc.settings.machine.as_deref()?)
    }

    /// Whether to offer sending the design straight to the machine (Send
    /// to Skitch, in the toolbar and the File menu): when the document's
    /// machine is one ZigZag connects to. A document set up for no machine
    /// goes by the one new documents start on; with none chosen there
    /// either, it's offered, as before there were machines to choose. And
    /// always while the machine's window is open or a job is on it.
    pub(crate) fn sends_direct(&self) -> bool {
        if self.machine.open || self.machine.sent.is_some() {
            return true;
        }
        match self
            .doc
            .settings
            .machine
            .as_deref()
            .or(self.machines.current.as_deref())
        {
            None => true,
            Some(id) => self.machines.find(id).is_some_and(|p| p.link.is_some()),
        }
    }

    /// The document's machine by name: one this computer doesn't know is
    /// named by the id the file has.
    pub fn machine_label(&self) -> Option<String> {
        let id = self.doc.settings.machine.as_deref()?;
        Some(
            self.machines
                .find(id)
                .map_or_else(|| id.to_string(), |p| p.name.clone()),
        )
    }

    /// Read your machines (remembered from now on). A new document starts
    /// set up for the one you chose last.
    pub fn load_machines(&mut self) {
        self.machines = Machines::load();
        if self.tab_is_blank() {
            self.start_on_machine();
        }
    }

    /// An empty document, set up for the machine new documents start with.
    pub(crate) fn blank_document(&self) -> Document {
        let mut d = Document::empty();
        if let Some(p) = self
            .machines
            .current
            .as_deref()
            .and_then(|id| self.machines.find(id))
        {
            p.apply(&mut d.settings, false);
        }
        d
    }

    /// Set the empty document in front up for the machine new documents
    /// start with (not a change to it: nothing to undo or save).
    pub(crate) fn start_on_machine(&mut self) {
        self.doc.settings = self.blank_document().settings;
        self.stable.settings = self.doc.settings.clone();
    }

    /// Make `id` the document's machine (or none), and the one new
    /// documents start with. A document with nothing in it moves to the
    /// machine's default hoop; a design stays in its hoop if the machine
    /// has it.
    pub fn choose_machine(&mut self, id: Option<&str>) {
        match id.and_then(|id| self.machines.find(id)).cloned() {
            Some(p) => p.apply(&mut self.doc.settings, !self.doc.shapes.is_empty()),
            None => self.doc.settings.machine = None,
        }
        self.hoop_custom = false;
        self.machines.current = self.doc.settings.machine.clone();
        self.machines.save();
    }

    /// What the document's machine asks of the stitches, in a few words:
    /// nothing, when it's the usual.
    pub(crate) fn stitch_rules_note(&self) -> Option<String> {
        let s = &self.doc.settings;
        let mut parts: Vec<String> = Vec::new();
        if s.min_stitch_mm != DEFAULT_MIN_STITCH_MM {
            parts.push(format!(
                "no stitch under {} mm",
                zigzag::params::fmt_num(s.min_stitch_mm)
            ));
        }
        if s.lock != Lock::default() {
            parts.push(match s.lock.style {
                LockStyle::None => "no locks".into(),
                LockStyle::Back => format!(
                    "{} × {} mm locks",
                    s.lock.count,
                    zigzag::params::fmt_num(s.lock.len_mm)
                ),
                LockStyle::Triangle => "triangle locks".into(),
            });
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }

    /// The Document panel's Machine row: which machine this design is set
    /// up for, and the way to Settings, where machines are made and
    /// changed. Under it, what the machine asks of the stitches when that
    /// isn't the usual.
    pub(crate) fn machine_rows(&mut self, fm: &mut Form) {
        let here = self.doc.settings.machine.clone();
        let mut choose: Option<Option<String>> = None;
        let mut settings = false;
        fm.row("Machine", |ui, lid| {
            let about = match self.machine() {
                Some(p) if p.tested => format!("{} Sewn from ZigZag.", p.source),
                Some(p) if !p.source.is_empty() => {
                    format!("{} Not yet sewn from ZigZag.", p.source)
                }
                Some(_) => "One of your machines.".to_string(),
                None if here.is_some() => {
                    "This design was set up for a machine this computer doesn't have.".to_string()
                }
                None => "The machine this design is for: its hoops, the file it reads and what it asks of the stitches. New documents start with the one you choose.".to_string(),
            };
            egui::ComboBox::from_id_salt("machine")
                .selected_text(self.machine_label().unwrap_or("Any machine".into()))
                .width(142.0)
                .height(400.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(here.is_none(), "Any machine").clicked() {
                        choose = Some(None);
                    }
                    ui.separator();
                    for (k, p) in self.machines.all().enumerate() {
                        if k == profile::builtin().len() {
                            ui.separator();
                        }
                        let on = here.as_deref() == Some(p.id.as_str());
                        if ui.selectable_label(on, &p.name).clicked() {
                            choose = Some(Some(p.id.clone()));
                        }
                    }
                    ui.separator();
                    if ui
                        .selectable_label(false, "Machine Settings…")
                        .on_hover_text("Add a machine of your own, or change one")
                        .clicked()
                    {
                        settings = true;
                    }
                })
                .response
                .labelled_by(lid)
                .on_hover_text(about);
            if crate::theme::icon_button(ui, crate::icons::Icon::Gear, "Machine Settings", false, 22.0)
                .on_hover_text("Hoops, file format, shortest stitch and locks for your machines")
                .clicked()
            {
                settings = true;
            }
        });
        if let Some(note) = self.stitch_rules_note() {
            fm.wide(|ui| {
                crate::theme::caption(ui, note).on_hover_text(
                    "From the machine. To change it, change the machine in Settings.",
                );
            });
        }
        if let Some(id) = choose {
            self.choose_machine(id.as_deref());
        }
        if settings {
            self.open_settings(crate::settings::Section::Machines);
        }
    }

    /// The Document panel's Narrow satin row.
    pub(crate) fn narrow_satin_row(&mut self, fm: &mut Form) {
        fm.row("Narrow satin", |ui, lid| {
            ui.add(crate::units::drag_mm(
                &mut self.doc.settings.narrow_satin_mm,
                zigzag::document::NARROW_SATIN_MM,
                0.01,
            ))
            .labelled_by(lid)
            .on_hover_text("Satins narrower than this on average get a warning suggesting Run. How narrow a column still sews cleanly depends on your fabric and thread: set it to what does for you (0 never warns). It doesn't change the stitches.");
        });
    }

    /// The Document panel's Hoop menu entries: the machine's hoops (if the
    /// document has one this computer knows), then the standard ones it
    /// hasn't. Each is (size, name), with `true` on the first of the
    /// second group when both have entries (a separator goes before it).
    pub(crate) fn hoop_menu(&self) -> Vec<((f64, f64), String, bool)> {
        let mut out: Vec<((f64, f64), String, bool)> = Vec::new();
        let machine = self.machine();
        for &h in machine.iter().flat_map(|p| &p.hoops) {
            out.push((h, hoop_name(h), false));
        }
        let own = out.len();
        for h in &HOOPS {
            if machine.is_none_or(|p| !p.has_hoop((h.2, h.3))) {
                out.push(((h.2, h.3), h.1.to_string(), own > 0 && out.len() == own));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Vec2;

    #[test]
    fn your_machines_are_remembered_and_told_apart() {
        let file =
            std::env::temp_dir().join(format!("zigzag-machines-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        // A new install: none of your own, and new documents start on
        // the Skitch until you choose another (or none).
        let mut m = Machines::load_from(Some(file.clone()));
        assert!(m.mine.is_empty());
        assert_eq!(m.current.as_deref(), Some("brother-skitch"));
        let a = m.add("Shop Tajima", (300.0, 200.0));
        let b = m.add("Shop Tajima", (150.0, 150.0));
        // (Nor can one of yours take the id of one of ZigZag's.)
        let c = m.add("Melco Summit", (100.0, 100.0));
        assert_eq!(
            [a.as_str(), b.as_str(), c.as_str()],
            ["shop-tajima", "shop-tajima-2", "melco-summit-2"]
        );
        m.mine[0].format = profile::machine_format("dst");
        m.current = Some(a.clone());
        m.save();
        let mut back = Machines::load_from(Some(file.clone()));
        assert_eq!(back.mine, m.mine);
        assert_eq!(back.current, Some(a.clone()));
        assert_eq!(
            back.named("shop tajima").map(|p| p.id.as_str()),
            Some("shop-tajima")
        );
        assert_eq!(back.all().count(), profile::builtin().len() + 3);
        // Forgetting the machine new documents start with leaves none.
        back.remove(&a);
        let again = Machines::load_from(Some(file.clone()));
        assert_eq!((again.mine.len(), again.current), (2, None));
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn send_is_offered_for_a_machine_zigzag_sends_to() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 900.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(3);
        let offered = |h: &Harness<'_, Studio>| h.query_by_label("Send to Skitch").is_some();
        // No machine chosen anywhere: as it was before there were any.
        assert!(h.state().sends_direct() && offered(&h));
        // A machine that takes a file: no Send in the toolbar, and the
        // File menu's is greyed.
        h.state_mut().choose_machine(Some("melco-summit"));
        h.run_steps(3);
        assert!(!h.state().sends_direct() && !offered(&h));
        assert!(
            !h.state()
                .action_enabled(crate::commands::Action::SendToSkitch)
        );
        // An agent is told why, and what to do instead.
        let r = h
            .state_mut()
            .run_tool("send_to_skitch", &serde_json::json!({}));
        let said = match &r.content[0] {
            crate::mcp::Content::Text(t) => t.clone(),
            _ => String::new(),
        };
        assert!(
            r.is_error && said.contains("Melco Summit") && said.contains("export"),
            "{said}"
        );
        assert!(!h.state().machine.open, "its window stays shut");
        // The Skitch: there again.
        h.state_mut().choose_machine(Some("brother-skitch"));
        h.run_steps(3);
        assert!(h.state().sends_direct() && offered(&h));
        // A document set up for none goes by the machine new documents
        // start on.
        h.state_mut().doc.settings.machine = None;
        h.state_mut().machines.current = Some("janome-550e".into());
        h.run_steps(3);
        assert!(!offered(&h));
        // One of your own that's a Skitch too (a copy of ZigZag's keeps
        // how it's sent to), and one of your own that isn't.
        let copy = h.state_mut().machines.duplicate("brother-skitch").unwrap();
        assert_eq!(h.state().machines.find(&copy).unwrap().link, Some("skitch"));
        h.state_mut().choose_machine(Some(&copy));
        h.run_steps(3);
        assert!(offered(&h));
        let mut p = h.state().machines.find(&copy).unwrap().clone();
        p.link = None;
        h.state_mut().update_machine(p.clone());
        h.run_steps(3);
        assert!(!offered(&h));
        // (And it's kept that way in the file of your machines.)
        let back = Profile::from_json(&p.to_json()).unwrap();
        assert_eq!(back.link, None);
        p.link = Some("skitch");
        assert_eq!(
            Profile::from_json(&p.to_json()).unwrap().link,
            Some("skitch")
        );
        // While its window is open it stays, whatever the machine: there's
        // a job to see through.
        h.state_mut().choose_machine(Some("melco-summit"));
        h.state_mut().machine.open = true;
        h.run_steps(3);
        assert!(h.state().sends_direct());
    }

    #[test]
    fn a_new_document_starts_on_the_machine_chosen() {
        let mut s = Studio::with_file(None, None);
        s.choose_machine(Some("brother-5x7"));
        assert_eq!(s.doc.settings.hoop, (130.0, 180.0));
        assert_eq!(s.machines.current.as_deref(), Some("brother-5x7"));
        s.new_tab();
        assert!(s.tab_is_blank(), "set up for the machine, not changed");
        assert_eq!(s.doc.settings.machine.as_deref(), Some("brother-5x7"));
        assert_eq!(s.doc.settings.hoop, (130.0, 180.0));
        // Closing the only tab leaves a new document, set up the same.
        s.switch_tab(0);
        s.close_tab(0);
        assert_eq!(s.tab_count(), 1);
        s.close_tab(0);
        assert_eq!(s.doc.settings.machine.as_deref(), Some("brother-5x7"));
        // Any machine: new documents are as they always were.
        s.choose_machine(None);
        s.new_tab();
        assert_eq!(s.doc.settings, Settings::default());
    }

    #[test]
    fn the_document_panel_chooses_a_machine_and_settings_makes_them() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(4);
        h.get_by_label("Machine").click();
        h.run_steps(3);
        h.get_by_label("Janome Memory Craft 550E").click();
        h.run_steps(4);
        assert_eq!(
            h.state().doc.settings.machine.as_deref(),
            Some("janome-550e")
        );
        assert_eq!(h.state().hoop_dims(), (200.0, 360.0));
        assert_eq!(
            h.state().hoop_label(),
            "200 × 360 mm · Janome Memory Craft 550E"
        );
        // Its hoops lead the Hoop menu, then the standard ones it hasn't.
        let menu = h.state().hoop_menu();
        let names: Vec<&str> = menu.iter().map(|x| x.1.as_str()).collect();
        assert_eq!(
            names[..5],
            [
                "200 × 360 mm",
                "8×8 in (200 × 200 mm)",
                "140 × 200 mm · Janome B",
                "140 × 140 mm",
                "1×2.5 in (20 × 60 mm)"
            ]
        );
        assert_eq!(menu.len(), 4 + HOOPS.len() - 2);
        let rules: Vec<usize> = (0..menu.len()).filter(|&k| menu[k].2).collect();
        assert_eq!(rules, [4]);
        // One of the machine's hoops isn't a custom hoop, standard or not.
        assert!(h.query_by_label("Hoop height").is_none());
        h.get_by_label("Hoop").click();
        h.run_steps(3);
        h.get_by_label("140 × 140 mm").click();
        h.run_steps(4);
        assert_eq!(h.state().hoop_dims(), (140.0, 140.0));
        // The panel chooses machines; making and changing them is in
        // Settings, a gear away. Nothing of that is in the panel itself.
        for gone in ["Min stitch", "Locks", "Reads", "Stitch Rules"] {
            assert!(
                h.query_by_label(gone).is_none(),
                "{gone} is in Settings now"
            );
        }
        assert!(!h.state().settings_ui.open);
        h.get_by_label("Machine Settings").click();
        h.run_steps(4);
        assert!(h.state().settings_ui.open);
        assert_eq!(
            h.state().settings_ui.machine.as_deref(),
            Some("janome-550e")
        );
        // One of ZigZag's can't be changed there, but it can be copied.
        assert!(h.query_by_label("Machine name").is_none());
        h.get_by_label("Duplicate").click();
        h.run_steps(4);
        let copy = h.state().settings_ui.machine.clone().unwrap();
        assert!(h.state().machines.is_mine(&copy));
        assert_eq!(h.state().machines.find(&copy).unwrap().hoops.len(), 4);
        assert!(h.query_by_label("Machine name").is_some());
        // A new one starts from the document: its hoop.
        h.get_by_label("New Machine").click();
        h.run_steps(4);
        let new = h.state().settings_ui.machine.clone().unwrap();
        assert_ne!(new, copy);
        assert_eq!(
            h.state().machines.find(&new).unwrap().hoops,
            [(140.0, 140.0)]
        );
        // Start new documents on it; Done closes the window.
        h.get_by_label("Start on this machine").click();
        h.run_steps(3);
        assert_eq!(h.state().machines.current.as_deref(), Some(new.as_str()));
        h.get_by_label("Done").click();
        h.run_steps(4);
        assert!(!h.state().settings_ui.open);
        h.state_mut().new_tab();
        h.run_steps(3);
        assert_eq!(
            h.state().doc.settings.machine.as_deref(),
            Some(new.as_str())
        );
        assert_eq!(h.state().doc.settings.hoop, (140.0, 140.0));
        // A machine that asks something of the stitches says so under the
        // Machine row; one that doesn't, doesn't.
        assert!(h.state().stitch_rules_note().is_none());
        h.state_mut().choose_machine(Some("melco-summit"));
        h.run_steps(3);
        assert_eq!(
            h.state().stitch_rules_note().as_deref(),
            Some("no stitch under 0.4 mm")
        );
        assert!(h.query_by_label("no stitch under 0.4 mm").is_some());
        // The other sections: units, appearance and the GPU; the agent.
        h.state_mut()
            .open_settings(crate::settings::Section::General);
        h.run_steps(4);
        assert!(h.query_by_label("Draw stitches on the GPU").is_some());
        h.get_by_label("Dark").click();
        h.run_steps(3);
        assert_eq!(
            h.state().prefs.appearance,
            crate::settings::Appearance::Dark
        );
        // (The Agent panel has the same name as the section: opened here
        // as the menu would.)
        h.state_mut().open_settings(crate::settings::Section::Agent);
        h.run_steps(4);
        assert!(h.query_by_label("Let agents connect").is_some());
        assert!(
            h.query_by_label("Keep a log of what agents do").is_some()
                && h.query_by_label("Show Log").is_some()
        );
        h.get_by_label("Let agents connect").click();
        h.run_steps(3);
        assert_eq!(h.state().prefs.agent_port(), None);
    }
}
