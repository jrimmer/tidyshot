//! Recording durability: journal, checkpoint, and recovery.
// Ported from omnishot/recording_journal.py, omnishot/recording_checkpoint.py,
// and omnishot/recording_recovery.py.
//!
//! The app leases an unfinished take with a lock file, records its phase in a
//! sidecar marker, and on relaunch recovers or discards it. Nothing here may
//! read or delete a take while its encoder still holds the lease, and a full
//! disk must not cost the user a playable recording.

pub mod checkpoint;
pub mod display;
pub mod journal;
pub mod options;
pub mod recorder;
pub mod recovery;

use std::io;
use std::path::{Path, PathBuf};

use crate::json::Value;
use crate::storage;

/// The on-disk layout a recording lives in.
///
/// Only the parts the journal and recovery need are implemented here; the full
/// capture store grows in a later unit.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// Root data directory (holds `captures/`, `tracks/`, and sidecars).
    pub root: PathBuf,
}

impl Workspace {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    #[must_use]
    pub fn captures(&self) -> PathBuf {
        self.root.join("captures")
    }

    #[must_use]
    pub fn tracks(&self) -> PathBuf {
        self.root.join("tracks")
    }

    /// Create the capture and track directories.
    pub fn ensure(&self) -> io::Result<()> {
        std::fs::create_dir_all(self.captures())?;
        std::fs::create_dir_all(self.tracks())?;
        Ok(())
    }

    /// Read the sidecar metadata beside `path`, or an empty object.
    #[must_use]
    pub fn metadata(&self, path: &Path) -> Value {
        let sidecar = path.with_extension("json");
        std::fs::read_to_string(sidecar)
            .ok()
            .and_then(|text| crate::json::parse(&text).ok())
            .filter(|value| value.as_object().is_some())
            .unwrap_or(Value::Object(std::collections::BTreeMap::new()))
    }

    /// Atomically write a JSON sidecar beside `path` (extension `.json`).
    pub fn atomic_json(&self, path: &Path, value: &Value) -> io::Result<()> {
        storage::atomic_write_file(
            &path.with_extension("json"),
            value.to_string_pretty().as_bytes(),
        )
    }
}
