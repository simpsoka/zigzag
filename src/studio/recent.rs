//! File › Open Recent: the designs opened or saved lately, newest first,
//! kept between launches (`recent.json` in ZigZag's settings folder; the
//! paths are this computer's, so the list isn't synced).

use std::path::{Path, PathBuf};

/// How many the menu lists.
pub const MAX: usize = 10;

#[derive(Default)]
pub struct Recent {
    /// Newest first.
    files: Vec<PathBuf>,
    path: Option<PathBuf>,
}

impl Recent {
    /// The list from last time; saved from now on.
    pub fn load() -> Recent {
        Self::load_from(crate::dock::config_file("recent.json"))
    }

    pub(crate) fn load_from(path: Option<PathBuf>) -> Recent {
        let files = path
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .map(|v| {
                v["files"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|f| f.as_str().map(PathBuf::from))
                    .take(MAX)
                    .collect()
            })
            .unwrap_or_default();
        Recent { files, path }
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        let v = serde_json::json!({
            "files": self.files.iter().map(|f| f.to_string_lossy()).collect::<Vec<_>>(),
        });
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, serde_json::to_string_pretty(&v).unwrap_or_default());
    }

    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// `file` was opened or saved just now: it goes to the top.
    pub fn note(&mut self, file: &Path) {
        // (As a whole path: the folder ZigZag was started in isn't the
        // next launch's.)
        let file = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
        if self.files.first() == Some(&file) {
            return;
        }
        self.files.retain(|f| *f != file);
        self.files.insert(0, file);
        self.files.truncate(MAX);
        self.save();
    }

    /// `file` isn't there any more: off the list.
    pub fn forget(&mut self, file: &Path) {
        let n = self.files.len();
        self.files.retain(|f| f != file);
        if self.files.len() != n {
            self.save();
        }
    }

    /// Open Recent › Clear Menu.
    pub fn clear(&mut self) {
        if !self.files.is_empty() {
            self.files.clear();
            self.save();
        }
    }

    /// What the menu calls each file: its name, and its folder too where
    /// two on the list share a name.
    pub fn labels(&self) -> Vec<String> {
        let name = |f: &Path| {
            f.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| f.display().to_string())
        };
        self.files
            .iter()
            .map(|f| {
                let n = name(f);
                let shared = self.files.iter().filter(|g| name(g) == n).count() > 1;
                match f.parent().and_then(Path::file_name) {
                    Some(dir) if shared => format!("{n} — {}", dir.to_string_lossy()),
                    _ => n,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A whole path on any system (`/designs` alone isn't one on Windows).
    fn at(name: &str) -> PathBuf {
        std::env::temp_dir().join("designs").join(name)
    }

    #[test]
    fn the_newest_is_first_once_each_and_only_so_many() {
        let mut r = Recent::default();
        for i in 0..MAX + 3 {
            r.note(&at(&format!("{i}.svg")));
        }
        assert_eq!(r.files().len(), MAX);
        assert_eq!(r.files()[0], at("12.svg"));
        // Opened again: moved to the top, not listed twice.
        r.note(&at("5.svg"));
        assert_eq!(r.files()[0], at("5.svg"));
        assert_eq!(r.files().len(), MAX);
        assert_eq!(r.files().iter().filter(|f| f.ends_with("5.svg")).count(), 1);
        r.forget(&at("5.svg"));
        assert_eq!(r.files()[0], at("12.svg"));
        r.clear();
        assert!(r.files().is_empty());
    }

    #[test]
    fn the_list_comes_back_at_the_next_launch() {
        let dir = std::env::temp_dir().join(format!("zigzag-recent-{}", std::process::id()));
        let file = dir.join("recent.json");
        let mut r = Recent::load_from(Some(file.clone()));
        assert!(r.files().is_empty());
        r.note(&at("a.svg"));
        r.note(&at("b.pes"));
        let again = Recent::load_from(Some(file.clone()));
        assert_eq!(again.files(), r.files());
        assert_eq!(again.files()[0], at("b.pes"));
        // Cleared stays cleared.
        r.clear();
        assert!(Recent::load_from(Some(file)).files().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn files_with_the_same_name_say_which_folder() {
        let mut r = Recent::default();
        let work = std::env::temp_dir().join("work");
        r.note(&work.join("hats/logo.svg"));
        r.note(&work.join("bags/logo.svg"));
        r.note(&work.join("bags/flower.svg"));
        assert_eq!(r.labels(), ["flower.svg", "logo.svg — bags", "logo.svg — hats"]);
    }
}
