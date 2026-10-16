//! Atomic persistence for user-owned files.
// Ported from omnishot/storage.py
//!
//! Every save writes a sibling temp file, fsyncs it, then renames it over the
//! destination, so a crash or a full disk can never leave a half-written file
//! in place of a committed one.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Run `write` against a temporary file beside `destination`, then commit it.
///
/// The temp file is removed if `write` fails or if the commit fails, so the
/// destination only ever holds complete content.
pub fn atomic_write<F>(destination: &Path, write: F) -> io::Result<()>
where
    F: FnOnce(&mut File) -> io::Result<()>,
{
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let name = destination
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("output");
    let temporary = temp_path(parent, name);

    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        write(&mut file)?;
        file.sync_all()?;
        drop(file);
        // fsync the directory so the rename itself survives a power loss.
        fs::rename(&temporary, destination)?;
        sync_dir(parent)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Atomically write `contents` to `destination`.
pub fn atomic_write_file(destination: &Path, contents: &[u8]) -> io::Result<()> {
    atomic_write(destination, |file| file.write_all(contents))
}

/// Pick a unique sibling temp path. The caller owns removal.
fn temp_path(parent: &Path, name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let pid = std::process::id();
    parent.join(format!(".tidyshot-{nonce:x}-{pid}-{name}"))
}

#[cfg(unix)]
fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// A temp file the caller writes to and then commits explicitly.
///
/// `Storage`-backed writers that need to encode an image or archive first use
/// this; the rename happens only when [`AtomicFile::commit`] is called.
pub struct AtomicFile {
    temporary: PathBuf,
    destination: PathBuf,
    committed: bool,
}

impl AtomicFile {
    /// Create the sibling temp file for `destination`.
    pub fn new(destination: &Path) -> io::Result<Self> {
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        let name = destination
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("output");
        let temporary = temp_path(parent, name);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        Ok(Self {
            temporary,
            destination: destination.to_path_buf(),
            committed: false,
        })
    }

    /// Path of the temp file to write into.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.temporary
    }

    /// fsync the temp file and rename it over the destination.
    pub fn commit(mut self) -> io::Result<()> {
        let file = File::open(&self.temporary)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&self.temporary, &self.destination)?;
        if let Some(parent) = self.destination.parent() {
            sync_dir(parent)?;
        }
        self.committed = true;
        Ok(())
    }
}

impl Drop for AtomicFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.temporary);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tidyshot-storage-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn committed_content_replaces_destination() {
        let dir = scratch("replace");
        let target = dir.join("out.txt");
        fs::write(&target, b"old").unwrap();
        atomic_write_file(&target, b"new").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert_eq!(siblings(&dir), 0, "no temp files left behind");
    }

    #[test]
    fn failed_write_preserves_destination_and_removes_temp() {
        let dir = scratch("failed");
        let target = dir.join("out.txt");
        fs::write(&target, b"previous").unwrap();
        let result = atomic_write(&target, |_| Err(io::Error::other("disk full")));
        assert!(result.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"previous");
        assert_eq!(siblings(&dir), 0);
    }

    #[test]
    fn atomic_file_commit_moves_the_temp_into_place() {
        let dir = scratch("handle");
        let target = dir.join("image.png");
        let file = AtomicFile::new(&target).unwrap();
        std::fs::write(file.path(), b"encoded").unwrap();
        file.commit().unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"encoded");
        assert_eq!(siblings(&dir), 0);
    }

    #[test]
    fn atomic_file_drop_without_commit_leaves_destination_untouched() {
        let dir = scratch("drop");
        let target = dir.join("image.png");
        fs::write(&target, b"keep").unwrap();
        {
            let file = AtomicFile::new(&target).unwrap();
            std::fs::write(file.path(), b"partial").unwrap();
        }
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        assert_eq!(siblings(&dir), 0);
    }

    fn siblings(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tidyshot-"))
            .count()
    }
}
