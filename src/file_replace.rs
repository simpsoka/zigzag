//! Whole-file replacement shared by the engine and studio.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Replacement completed, but syncing its directory failed. New bytes are
/// visible; callers must commit identity while retaining recovery/uncertainty.
#[derive(Debug)]
pub struct DurabilityError {
    pub destination: PathBuf,
    pub source: io::Error,
}

impl std::fmt::Display for DurabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "replacement committed for {}; parent directory sync failed; durability unconfirmed",
            self.destination.display()
        )
    }
}

impl std::error::Error for DurabilityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

pub fn replacement_committed(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|source| source.is::<DurabilityError>())
}

/// Write and sync a unique sibling, then atomically replace the destination.
///
/// Errors before replacement leave the original intact. A parent-directory sync
/// error happens after commit: the new bytes are visible but durability is not
/// confirmed. Directory syncing is unavailable on Windows.
/// Existing symlinks are followed (dangling links fail); basic permissions are
/// copied. Existing non-regular targets are rejected before opening them.
/// Replacement creates a new inode/file: ownership, ACLs, hard links,
/// extended metadata and unusual/network filesystem guarantees are not preserved
/// or qualified here.
pub fn replace_file_atomic(
    destination: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    replace_with_sync(destination, write, sync_parent)
}

fn replace_with_sync(
    destination: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
    sync: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let requested_destination = destination;
    // Preserve the old write-through-symlink behavior, rather than replacing
    // the link itself. A dangling link is rejected before any file is written.
    let resolved;
    let destination = match std::fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            resolved = std::fs::canonicalize(destination)?;
            resolved.as_path()
        }
        Ok(_) => destination,
        Err(error) if error.kind() == io::ErrorKind::NotFound => destination,
        Err(error) => return Err(error),
    };
    let permissions = match std::fs::metadata(destination) {
        Ok(metadata) => {
            // Atomic replacement is a whole-file operation, not a stream write.
            // Reject special files before opening them (a FIFO open may block).
            if !metadata.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "atomic replacement destination must be a regular file",
                ));
            }
            // Rename needs directory access, but must not bypass the existing
            // file's write check. Do not truncate, and release before replacing.
            drop(std::fs::OpenOptions::new().write(true).open(destination)?);
            Some(metadata.permissions())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut builder = tempfile::Builder::new();
    builder.prefix(".zigzag-");
    #[cfg(unix)]
    if permissions.is_none() {
        // Match File::create for new documents/exports, including the umask.
        // Existing files stay private while preparing and get their old mode.
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o666));
    }
    let mut temporary = builder.tempfile_in(parent)?;
    write(temporary.as_file_mut())?;
    temporary.flush()?;
    if let Some(permissions) = permissions {
        temporary.as_file().set_permissions(permissions)?;
    }
    temporary.as_file().sync_all()?;
    // PersistError owns the temporary: dropping it also cleans up a failed rename.
    temporary
        .persist(destination)
        .map_err(|error| error.error)?;
    sync(parent).map_err(|error| {
        io::Error::new(
            error.kind(),
            DurabilityError {
                destination: requested_destination.to_path_buf(),
                source: error,
            },
        )
    })?;
    Ok(())
}

fn sync_parent(parent: &Path) -> io::Result<()> {
    #[cfg(unix)]
    match File::open(parent)?.sync_all() {
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::Unsupported | io::ErrorKind::InvalidInput
            ) => {}
        result => result?,
    }
    #[cfg(not(unix))]
    let _ = parent;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::ensure;

    #[test]
    fn non_file_destination_is_rejected_before_writing() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let destination = dir.path().join("directory.svg");
        std::fs::create_dir(&destination)?;
        let mut wrote = false;
        let result = replace_file_atomic(&destination, |_| {
            wrote = true;
            Ok(())
        });
        let error = result
            .err()
            .ok_or_else(|| anyhow::anyhow!("directory accepted"))?;
        ensure!(
            error.kind() == io::ErrorKind::InvalidInput,
            "wrong target-kind error: {error}"
        );
        ensure!(!wrote && destination.is_dir(), "destination was touched");
        ensure!(
            std::fs::read_dir(dir.path())?.count() == 1,
            "temporary leaked"
        );
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn named_pipe_and_symlink_target_are_preserved() -> anyhow::Result<()> {
        use std::os::unix::fs::{FileTypeExt, symlink};
        for linked in [false, true] {
            let dir = tempfile::tempdir()?;
            let pipe = dir.path().join("output.svg");
            let status = std::process::Command::new("mkfifo").arg(&pipe).status()?;
            ensure!(status.success(), "mkfifo failed");
            // Linux permits O_RDWR on a FIFO without another peer. Keep both
            // ends open so even the old write-access probe cannot block.
            let _peer = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&pipe)?;
            let link = dir.path().join("link.svg");
            let destination = if linked {
                symlink(&pipe, &link)?;
                &link
            } else {
                &pipe
            };
            let mut wrote = false;
            let result = replace_file_atomic(destination, |file| {
                wrote = true;
                file.write_all(b"replacement")
            });
            let error = result
                .err()
                .ok_or_else(|| anyhow::anyhow!("FIFO accepted (symlink={linked})"))?;
            ensure!(
                error.kind() == io::ErrorKind::InvalidInput,
                "wrong target-kind error: {error}"
            );
            ensure!(!wrote, "writer ran for special destination");
            ensure!(
                std::fs::symlink_metadata(&pipe)?.file_type().is_fifo(),
                "FIFO was replaced"
            );
            if linked {
                ensure!(
                    std::fs::symlink_metadata(&link)?.file_type().is_symlink(),
                    "link was replaced"
                );
            }
            ensure!(
                std::fs::read_dir(dir.path())?.count() == if linked { 2 } else { 1 },
                "temporary leaked"
            );
        }
        Ok(())
    }

    #[test]
    fn replacement_respects_the_operating_system_write_check() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("readonly.svg");
        std::fs::write(&path, b"original")?;
        let original_permissions = std::fs::metadata(&path)?.permissions();
        let mut readonly = original_permissions.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&path, readonly)?;
        let ordinary_writable = std::fs::OpenOptions::new().write(true).open(&path).is_ok();
        eprintln!("ordinary non-truncating write permitted: {ordinary_writable}");
        let result = replace_file_atomic(&path, |file| file.write_all(b"replacement"));
        let bytes = std::fs::read(&path)?;
        std::fs::set_permissions(&path, original_permissions)?;
        ensure!(
            result.is_ok() == ordinary_writable,
            "replacement bypassed the OS write-access result"
        );
        ensure!(
            bytes
                == if ordinary_writable {
                    b"replacement".as_slice()
                } else {
                    b"original".as_slice()
                },
            "unexpected destination contents"
        );
        ensure!(
            std::fs::read_dir(dir.path())?.count() == 1,
            "temporary leaked"
        );
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn failed_windows_replacement_preserves_existing_file() -> anyhow::Result<()> {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("locked.svg");
        std::fs::write(&path, b"original")?;
        let mut held = None;
        let result = replace_file_atomic(&path, |file| {
            file.write_all(b"complete new bytes")?;
            // Lock after preparation, so the commit itself fails.
            held = Some(
                std::fs::OpenOptions::new()
                    .read(true)
                    .share_mode(0)
                    .open(&path)?,
            );
            Ok(())
        });
        let reached_replacement = held.is_some();
        drop(held);
        ensure!(
            reached_replacement,
            "writer did not reach the replacement boundary"
        );
        ensure!(
            result.is_err(),
            "replacement of locked destination succeeded"
        );
        ensure!(
            std::fs::read(&path)? == b"original",
            "old destination changed"
        );
        ensure!(
            std::fs::read_dir(dir.path())?.count() == 1,
            "temporary leaked"
        );
        Ok(())
    }

    #[test]
    fn post_commit_failure_reports_visible_replacement() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("design.svg");
        std::fs::write(&path, b"old")?;
        let result = replace_with_sync(
            &path,
            |file| file.write_all(b"new"),
            |_| Err(io::Error::other("injected sync failure")),
        );
        let error = result
            .err()
            .ok_or_else(|| anyhow::anyhow!("sync failure swallowed"))?;
        ensure!(
            error.to_string().contains("replacement committed"),
            "ambiguous outcome"
        );
        ensure!(replacement_committed(&error), "typed commit outcome lost");
        ensure!(std::fs::read(&path)? == b"new", "commit was not visible");
        ensure!(
            std::fs::read_dir(dir.path())?.count() == 1,
            "temporary leaked"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn replacement_preserves_mode_and_symlink_target() -> anyhow::Result<()> {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir()?;
        let reference = dir.path().join("reference");
        let new = dir.path().join("new.svg");
        std::fs::write(&reference, b"reference")?;
        replace_file_atomic(&new, |file| file.write_all(b"new"))?;
        ensure!(
            std::fs::metadata(&new)?.permissions().mode() & 0o777
                == std::fs::metadata(&reference)?.permissions().mode() & 0o777,
            "new file mode differs from standard creation"
        );
        let path = dir.path().join("target.svg");
        let link = dir.path().join("link.svg");
        std::fs::write(&path, b"old")?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))?;
        symlink("target.svg", &link)?;
        replace_file_atomic(&link, |file| file.write_all(b"new"))?;
        ensure!(
            std::fs::symlink_metadata(&link)?.file_type().is_symlink(),
            "link replaced"
        );
        ensure!(std::fs::read(&path)? == b"new", "target not replaced");
        ensure!(
            std::fs::metadata(&path)?.permissions().mode() & 0o777 == 0o640,
            "mode changed"
        );
        Ok(())
    }

    #[test]
    fn replacement_commits_complete_bytes_and_cleans_failures() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let target = dir.path().join("design.svg");
        std::fs::write(&target, b"original")?;
        let failed = replace_file_atomic(&target, |file| {
            file.write_all(b"partial")?;
            Err(io::Error::other("injected write failure"))
        });
        ensure!(failed.is_err(), "write failure was swallowed");
        ensure!(std::fs::read(&target)? == b"original", "original changed");
        ensure!(
            std::fs::read_dir(dir.path())?.count() == 1,
            "temporary leaked"
        );
        replace_file_atomic(&target, |file| file.write_all(b"complete replacement"))?;
        ensure!(
            std::fs::read(&target)? == b"complete replacement",
            "incomplete commit"
        );
        ensure!(
            std::fs::read_dir(dir.path())?.count() == 1,
            "temporary leaked"
        );
        let directory = dir.path().join("directory");
        std::fs::create_dir(&directory)?;
        std::fs::write(directory.join("keep"), b"original")?;
        ensure!(
            replace_file_atomic(&directory, |file| file.write_all(b"new")).is_err(),
            "replaced directory"
        );
        ensure!(
            std::fs::read(directory.join("keep"))? == b"original",
            "replacement failure changed target"
        );
        ensure!(
            std::fs::read_dir(dir.path())?.count() == 2,
            "temporary leaked"
        );
        Ok(())
    }
}
