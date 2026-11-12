//! File › Open Recent: the last files opened or saved, most recent first,
//! remembered between launches (`recent.json` in ZigZag's settings folder).

use std::path::{Path, PathBuf};

/// How many files the menu keeps.
pub const MAX: usize = 10;

/// The submenu's title (the native menu bar finds the submenu by it).
pub const MENU_TITLE: &str = "Open Recent";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecentFiles {
    /// Most recent first; no file twice.
    list: Vec<PathBuf>,
    /// Where the list is remembered between launches (`None`: not saved).
    path: Option<PathBuf>,
}

impl RecentFiles {
    /// The list from last time (or none yet), saved from now on.
    pub fn load() -> RecentFiles {
        Self::load_from(crate::dock::config_file("recent.json"))
    }

    pub(crate) fn load_from(path: Option<PathBuf>) -> RecentFiles {
        let list = path
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .take(MAX)
            .collect();
        RecentFiles { list, path }
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        let list: Vec<String> = self
            .list
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(
            path,
            serde_json::to_string_pretty(&list).unwrap_or_default(),
        );
    }

    /// The files, most recent first.
    #[cfg_attr(
        not(any(target_os = "macos", feature = "menu-check")),
        allow(dead_code)
    )]
    pub fn paths(&self) -> &[PathBuf] {
        &self.list
    }

    pub fn get(&self, k: usize) -> Option<&Path> {
        self.list.get(k).map(PathBuf::as_path)
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// `path` was just opened or saved: it goes to the top (once).
    pub fn add(&mut self, path: &Path) {
        // The whole path, so it still opens from another working folder.
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if self.list.first() == Some(&path) {
            return;
        }
        self.list.retain(|p| *p != path);
        self.list.insert(0, path);
        self.list.truncate(MAX);
        self.save();
    }

    /// Take `path` out (it's gone from the disk).
    pub fn remove(&mut self, path: &Path) {
        let n = self.list.len();
        self.list.retain(|p| p != path);
        if self.list.len() != n {
            self.save();
        }
    }

    /// File › Open Recent › Clear Menu.
    pub fn clear(&mut self) {
        if !self.list.is_empty() {
            self.list.clear();
            self.save();
        }
    }

    /// Take over from `earlier`, a list kept before this one was read from
    /// the settings folder (files opened while starting up): its files go
    /// on top, in the same order.
    pub fn absorb(&mut self, earlier: &RecentFiles) {
        for p in earlier.list.iter().rev() {
            self.add(p);
        }
    }

    /// What the menu shows for file `k`: its name, then the folder it's in
    /// (with `~` for the home folder), so two files of the same name can be
    /// told apart.
    pub fn label(&self, k: usize) -> Option<String> {
        let p = self.list.get(k)?;
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.to_string_lossy().into_owned());
        let folder = p.parent().map(short_folder).filter(|f| !f.is_empty());
        Some(match folder {
            Some(f) => format!("{name} — {f}"),
            None => name,
        })
    }
}

/// A folder as the menu shows it: `~` for the home folder at its start.
fn short_folder(dir: &Path) -> String {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|h| !h.as_os_str().is_empty());
    match home.and_then(|h| dir.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => dir.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_no_repeats_and_at_most_ten() {
        let mut r = RecentFiles::default();
        for k in 0..12 {
            r.add(Path::new(&format!("/designs/logo-{k}.svg")));
        }
        assert_eq!(r.paths().len(), MAX);
        assert_eq!(r.get(0), Some(Path::new("/designs/logo-11.svg")));
        assert_eq!(r.get(MAX - 1), Some(Path::new("/designs/logo-2.svg")));
        // Opening one again moves it to the top, without a second entry.
        r.add(Path::new("/designs/logo-5.svg"));
        assert_eq!(r.paths().len(), MAX);
        assert_eq!(r.get(0), Some(Path::new("/designs/logo-5.svg")));
        assert_eq!(
            r.paths()
                .iter()
                .filter(|p| p.ends_with("logo-5.svg"))
                .count(),
            1
        );
        r.remove(Path::new("/designs/logo-5.svg"));
        assert_eq!(r.get(0), Some(Path::new("/designs/logo-11.svg")));
        assert_eq!(r.paths().len(), MAX - 1);
        r.clear();
        assert!(r.is_empty());
        assert_eq!(r.get(0), None);
        assert_eq!(r.label(0), None);
    }

    #[test]
    fn a_relative_path_is_kept_whole() {
        let mut r = RecentFiles::default();
        r.add(Path::new("examples/badge.svg"));
        let p = r.get(0).unwrap();
        assert!(p.is_absolute(), "{}", p.display());
        assert!(p.ends_with("examples/badge.svg"));
        // The same file by its whole path is the same entry.
        let whole = p.to_path_buf();
        r.add(&whole);
        assert_eq!(r.paths().len(), 1);
    }

    #[test]
    fn labels_name_the_file_and_its_folder() {
        let mut r = RecentFiles::default();
        r.add(Path::new("/work/caps/badge.svg"));
        r.add(Path::new("/work/patches/badge.svg"));
        assert_eq!(r.label(0).unwrap(), "badge.svg — /work/patches");
        assert_eq!(r.label(1).unwrap(), "badge.svg — /work/caps");
        if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            let home = PathBuf::from(home);
            r.add(&home.join("Designs").join("logo.pes"));
            let sep = std::path::MAIN_SEPARATOR;
            assert_eq!(r.label(0).unwrap(), format!("logo.pes — ~{sep}Designs"));
            r.add(&home.join("top.svg"));
            assert_eq!(r.label(0).unwrap(), "top.svg — ~");
        }
    }

    #[test]
    fn the_list_is_remembered_between_launches() {
        let dir = std::env::temp_dir().join(format!("zigzag-recent-{}", std::process::id()));
        let file = dir.join("recent.json");
        let _ = std::fs::remove_file(&file);
        let mut r = RecentFiles::load_from(Some(file.clone()));
        assert!(r.is_empty(), "nothing yet (and no file to read)");
        r.add(Path::new("/designs/a.svg"));
        r.add(Path::new("/designs/b.dst"));
        let again = RecentFiles::load_from(Some(file.clone()));
        assert_eq!(again.paths(), r.paths());
        assert_eq!(again.get(0), Some(Path::new("/designs/b.dst")));
        // Files opened before the list was read go on top of it, in order.
        let mut early = RecentFiles::default();
        early.add(Path::new("/designs/c.svg"));
        early.add(Path::new("/designs/a.svg"));
        let mut merged = RecentFiles::load_from(Some(file.clone()));
        merged.absorb(&early);
        let names: Vec<String> = merged
            .paths()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.svg", "c.svg", "b.dst"]);
        // Clear Menu empties the file too.
        merged.clear();
        assert!(RecentFiles::load_from(Some(file.clone())).is_empty());
        // A file that isn't a list of paths is no list.
        std::fs::write(&file, "{ not json").unwrap();
        assert!(RecentFiles::load_from(Some(file.clone())).is_empty());
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir(&dir);
    }

    // ---- In the studio: the list fills as files are opened and saved, and
    // File › Open Recent opens from it.

    use crate::app::Studio;
    use crate::commands::Action;

    fn example(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join(name)
    }

    #[test]
    fn opening_and_saving_files_fills_open_recent() {
        let mut s = Studio::with_file(None, None);
        assert!(s.recent.is_empty());
        assert!(!s.action_enabled(Action::OpenRecent(0)));
        assert!(!s.action_enabled(Action::ClearRecent));
        assert_eq!(s.action_label(Action::OpenRecent(0)), None);
        s.load(&example("badge.svg"));
        s.load(&example("groups.svg"));
        assert_eq!(
            s.recent.paths(),
            [example("groups.svg"), example("badge.svg")]
        );
        assert!(s.action_enabled(Action::OpenRecent(1)));
        assert!(!s.action_enabled(Action::OpenRecent(2)));
        assert!(s.action_enabled(Action::ClearRecent));
        let label = s.action_label(Action::OpenRecent(0)).unwrap();
        assert!(label.starts_with("groups.svg — "), "{label}");
        assert_eq!(s.doc.name, "groups");
        // The second one down: its tab comes forward, and it tops the list.
        let tabs = s.tab_count();
        s.run_action(Action::OpenRecent(1));
        assert_eq!(s.doc.name, "badge");
        assert_eq!(s.tab_count(), tabs, "no second tab for the same file");
        assert_eq!(
            s.recent.paths(),
            [example("badge.svg"), example("groups.svg")]
        );
        // A file that failed to open isn't listed.
        s.load(Path::new("/nowhere/missing.svg"));
        assert_eq!(s.recent.paths().len(), 2);
        // Save As lists the new file.
        let dir = std::env::temp_dir().join(format!("zigzag-recent-app-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let copy = dir.join("badge copy.svg");
        s.save(Some(&copy));
        assert_eq!(s.recent.get(0), Some(copy.as_path()), "{}", s.status);
        assert_eq!(s.recent.paths().len(), 3);
        // Gone from the disk: choosing it says so and takes it off the list.
        s.load(&example("groups.svg"));
        std::fs::remove_file(&copy).unwrap();
        let _ = std::fs::remove_dir(&dir);
        assert_eq!(s.recent.get(1), Some(copy.as_path()));
        s.run_action(Action::OpenRecent(1));
        assert_eq!(
            s.recent.paths(),
            [example("groups.svg"), example("badge.svg")]
        );
        assert!(s.status.contains("isn't there any more"), "{}", s.status);
        assert_eq!(s.doc.name, "groups", "the document in front stays");
        // A file an agent opens (the open_file tool) is listed too.
        let lettering = example("lettering.svg");
        let r = s.run_tool(
            "open_file",
            &serde_json::json!({ "path": lettering.to_string_lossy() }),
        );
        assert!(!r.is_error);
        assert_eq!(s.recent.get(0), Some(lettering.as_path()));
        s.recent.remove(&lettering);
        // A place past the end of the list does nothing.
        s.run_action(Action::OpenRecent(9));
        assert_eq!(s.recent.paths().len(), 2);
        // Clear Menu.
        s.run_action(Action::ClearRecent);
        assert!(s.recent.is_empty());
        assert!(!s.action_enabled(Action::ClearRecent));
    }

    // (The menus are drawn in the window everywhere but on a Mac, where
    // they're the real menu bar: nothing to click in a test there. The
    // list behind them is covered above on every platform.)
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn open_recent_is_in_the_file_menu() {
        use eframe::egui::Vec2;
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;
        let mut h = Harness::builder()
            .with_size(Vec2::new(1400.0, 1000.0))
            .build_ui_state(
                |ui, s: &mut Studio| s.frame(ui),
                Studio::with_file(None, None),
            );
        h.run_steps(3);
        // Nothing opened yet: the submenu says so, and Clear Menu is off.
        h.get_by_label("File").click();
        h.run_steps(3);
        h.get_by_label(MENU_TITLE).click();
        h.run_steps(3);
        assert!(h.query_by_label_contains("No Recent Files").is_some());
        assert!(!h.state().action_enabled(Action::ClearRecent));
        h.key_press(eframe::egui::Key::Escape);
        h.run_steps(3);
        // Two files opened: listed latest first, by name and folder.
        h.state_mut().load(&example("badge.svg"));
        h.state_mut().load(&example("groups.svg"));
        h.run_steps(3);
        h.get_by_label("File").click();
        h.run_steps(3);
        h.get_by_label(MENU_TITLE).click();
        h.run_steps(3);
        assert!(h.query_by_label_contains("No Recent Files").is_none());
        assert!(h.query_by_label_contains("groups.svg — ").is_some());
        assert!(h.query_by_label_contains("Clear Menu").is_some());
        assert!(h.state().action_enabled(Action::ClearRecent));
        // Choosing one opens it (here: brings its tab forward).
        h.get_by_label_contains("badge.svg — ").click();
        h.run_steps(4);
        assert_eq!(h.state().doc.name, "badge");
        assert_eq!(
            h.state().recent.get(0),
            Some(example("badge.svg").as_path())
        );
    }
}
