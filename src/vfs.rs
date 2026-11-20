//! Files, wherever ZigZag runs.
//!
//! On the desktop these are `std::fs`'s functions and nothing more. In the
//! browser (WebAssembly) there is no file system: the same calls work on
//! files kept in memory ([`mem`]), so the code that opens, saves and keeps
//! settings by path runs unchanged. What puts a picked file in, hands a
//! saved one to the person, and keeps the rest between visits is the web
//! build's business (`crates/studio/src/platform`): it reads
//! [`mem::take_changes`].

use std::io;
use std::path::Path;

#[cfg(not(target_arch = "wasm32"))]
mod real {
    use super::*;

    pub fn read(path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }
    pub fn read_to_string(path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }
    pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
        std::fs::write(path, contents)
    }
    pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }
    pub fn remove_file(path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }
    pub fn create_dir_all(path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }
    pub fn exists(path: &Path) -> bool {
        path.exists()
    }
}

/// Files kept in memory, by path. There are no folders: a file is there
/// or it isn't, and any folder can be "created".
pub mod mem {
    use std::collections::BTreeMap;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Files {
        files: BTreeMap<PathBuf, Vec<u8>>,
        /// Paths written or removed since [`take_changes`], each once.
        changed: Vec<PathBuf>,
    }

    static FILES: Mutex<Option<Files>> = Mutex::new(None);

    fn with<R>(f: impl FnOnce(&mut Files) -> R) -> R {
        let mut g = FILES.lock().unwrap_or_else(|e| e.into_inner());
        f(g.get_or_insert_with(Files::default))
    }

    fn missing(path: &Path) -> io::Error {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} isn't there", path.display()),
        )
    }

    impl Files {
        fn note(&mut self, path: &Path) {
            if !self.changed.iter().any(|p| p == path) {
                self.changed.push(path.to_path_buf());
            }
        }
    }

    pub fn read(path: &Path) -> io::Result<Vec<u8>> {
        with(|f| f.files.get(path).cloned().ok_or_else(|| missing(path)))
    }

    pub fn read_to_string(path: &Path) -> io::Result<String> {
        String::from_utf8(read(path)?)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not text"))
    }

    pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
        with(|f| {
            f.files.insert(path.to_path_buf(), contents.to_vec());
            f.note(path);
        });
        Ok(())
    }

    pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
        with(|f| {
            let data = f.files.remove(from).ok_or_else(|| missing(from))?;
            f.files.insert(to.to_path_buf(), data);
            f.note(from);
            f.note(to);
            Ok(())
        })
    }

    pub fn remove_file(path: &Path) -> io::Result<()> {
        with(|f| {
            f.files.remove(path).ok_or_else(|| missing(path))?;
            f.note(path);
            Ok(())
        })
    }

    pub fn create_dir_all(_path: &Path) -> io::Result<()> {
        Ok(())
    }

    /// A file at `path`, or files under it (a folder, as far as there are any).
    pub fn exists(path: &Path) -> bool {
        with(|f| f.files.keys().any(|p| p.starts_with(path)))
    }

    /// Put a file in without it counting as a change: what was kept from
    /// last time, read back in at the start.
    pub fn restore(path: &Path, contents: Vec<u8>) {
        with(|f| {
            f.files.insert(path.to_path_buf(), contents);
        });
    }

    /// Take a file out without it counting as a change (it's been dealt
    /// with: handed over, say).
    pub fn forget(path: &Path) {
        with(|f| {
            f.files.remove(path);
        });
    }

    /// The files under `dir` (every file, when it's empty).
    pub fn list(dir: &Path) -> Vec<PathBuf> {
        with(|f| {
            f.files
                .keys()
                .filter(|p| p.starts_with(dir))
                .cloned()
                .collect()
        })
    }

    /// What was written or removed since the last call, for whoever keeps
    /// the files somewhere lasting: each path with its contents now (None:
    /// it was removed).
    pub fn take_changes() -> Vec<(PathBuf, Option<Vec<u8>>)> {
        with(|f| {
            std::mem::take(&mut f.changed)
                .into_iter()
                .map(|p| {
                    let now = f.files.get(&p).cloned();
                    (p, now)
                })
                .collect()
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn files_in_memory_read_back_and_report_their_changes() {
            let dir = Path::new("/vfs-test");
            let a = dir.join("a.txt");
            let b = dir.join("b.txt");
            assert!(!exists(dir));
            assert!(read(&a).is_err());
            write(&a, b"one").unwrap();
            assert_eq!(read_to_string(&a).unwrap(), "one");
            assert!(exists(dir) && exists(&a));
            rename(&a, &b).unwrap();
            assert!(!exists(&a));
            assert_eq!(read(&b).unwrap(), b"one");
            let changes: Vec<_> = take_changes()
                .into_iter()
                .filter(|(p, _)| p.starts_with(dir))
                .collect();
            assert_eq!(
                changes,
                vec![(a.clone(), None), (b.clone(), Some(b"one".to_vec()))]
            );
            // What's restored isn't a change; what's removed is.
            restore(&a, b"kept".to_vec());
            assert_eq!(list(dir), vec![a.clone(), b.clone()]);
            remove_file(&b).unwrap();
            assert!(remove_file(&b).is_err());
            let changes: Vec<_> = take_changes()
                .into_iter()
                .filter(|(p, _)| p.starts_with(dir))
                .collect();
            assert_eq!(changes, vec![(b, None)]);
            // And what's forgotten is neither.
            forget(&a);
            assert!(!exists(dir));
            assert!(take_changes().iter().all(|(p, _)| !p.starts_with(dir)));
        }
    }
}

#[cfg(target_arch = "wasm32")]
use mem as real;

pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    real::read(path.as_ref())
}

pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    real::read_to_string(path.as_ref())
}

pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    real::write(path.as_ref(), contents.as_ref())
}

pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    real::rename(from.as_ref(), to.as_ref())
}

pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    real::remove_file(path.as_ref())
}

pub fn create_dir_all(path: impl AsRef<Path>) -> io::Result<()> {
    real::create_dir_all(path.as_ref())
}

/// Whether there's a file (or a folder) at `path`.
pub fn exists(path: impl AsRef<Path>) -> bool {
    real::exists(path.as_ref())
}
