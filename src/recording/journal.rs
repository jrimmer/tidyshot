//! Recording journals: lease an unfinished take so recovery never races it.
// Ported from omnishot/recording_journal.py
//!
//! While a take is recording, a `.recording.json` marker holds its phase and
//! options, and a `.recording.lock` flock keeps it leased. A crash leaves both
//! behind; recovery can then tell an active take from an abandoned one, and
//! cancellation renames the marker to `.discarding.json` so the intent outlives
//! a full-disk sidecar failure.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::commands::CommandSpec;
use crate::json::Value;

use super::Workspace;
use super::recovery::{self, RecoveryOutcome};

/// The suffix a live take marker uses.
const MARKER_SUFFIX: &str = ".recording.json";
/// The suffix a cancelled take marker uses.
const DISCARD_SUFFIX: &str = ".discarding.json";
/// The suffix a take whose video cannot be recovered uses.
const FAILED_SUFFIX: &str = ".failed-recording.json";

/// One recording's journal and lease.
pub struct Journal {
    workspace: Workspace,
    path: PathBuf,
    marker: PathBuf,
    lock_path: PathBuf,
    lock: Option<File>,
    record: Value,
}

impl Journal {
    /// Lease `path` and write the initial marker.
    ///
    /// The lock is taken before the marker is written; if the write fails the
    /// lock is released and the marker is not left behind.
    pub fn begin(workspace: Workspace, path: &Path, options: Value) -> io::Result<Self> {
        workspace.ensure()?;
        let marker = swap_suffix(path, MARKER_SUFFIX);
        let lock_path = swap_suffix(path, ".recording.lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        lock.try_lock().map_err(|err| match err {
            std::fs::TryLockError::WouldBlock => {
                io::Error::new(io::ErrorKind::WouldBlock, "take is already recording")
            }
            std::fs::TryLockError::Error(err) => err,
        })?;

        let mut record = BTreeMap::new();
        record.insert("version".to_owned(), Value::Number(1.0));
        record.insert("options".to_owned(), options);
        record.insert("created".to_owned(), Value::Number(now_seconds()));
        record.insert("phase".to_owned(), Value::String("recording".to_owned()));
        record.insert("old_dnd".to_owned(), Value::Null);
        let mut journal = Self {
            workspace,
            path: path.to_path_buf(),
            marker,
            lock_path,
            lock: Some(lock),
            record: Value::Object(record),
        };
        if let Err(err) = journal.update(ObjectUpdate::default()) {
            journal.release();
            let _ = fs::remove_file(&journal.lock_path);
            return Err(err);
        }
        Ok(journal)
    }

    /// The marker path, for callers that scan for markers.
    #[must_use]
    pub fn marker(&self) -> &Path {
        &self.marker
    }

    /// The path of the recording this journal leases.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The lease lock the encoder inherits, while this journal still holds it.
    ///
    /// The encoder receives this descriptor so its flock outlives the app: a
    /// crashed app leaves the lock held until the encoder exits, and recovery
    /// reports the take as pending instead of discarding a live recording.
    #[must_use]
    pub fn lease_fd(&self) -> Option<BorrowedFd<'_>> {
        self.lock.as_ref().map(AsFd::as_fd)
    }

    /// Merge `fields` into the record and persist it atomically.
    pub fn update(&mut self, fields: ObjectUpdate) -> io::Result<()> {
        if let Value::Object(entries) = &mut self.record {
            for (key, value) in fields.0 {
                entries.insert(key, value);
            }
        }
        self.workspace.atomic_json(&self.marker, &self.record)
    }

    /// Mark the take cancelled without allocating a new JSON file.
    ///
    /// Renaming the existing marker is the only step that is safe when the disk
    /// is full; the in-place phase update is a fallback.
    pub fn discard(&mut self) {
        let marker = swap_suffix(&self.path, DISCARD_SUFFIX);
        if fs::rename(&self.marker, &marker).is_ok() {
            self.marker = marker;
            return;
        }
        let mut fields = ObjectUpdate::default();
        fields.insert("phase", Value::String("discarding".to_owned()));
        let _ = self.update(fields);
    }

    /// Delete the marker and lock after a committed take.
    pub fn complete(&mut self) {
        let _ = fs::remove_file(&self.marker);
        let _ = fs::remove_file(&self.lock_path);
        self.release();
    }

    /// Release the lease without touching the marker.
    pub fn close(&mut self) {
        self.release();
    }

    fn release(&mut self) {
        // Close the descriptor without `flock(LOCK_UN)`. The source's `close()`
        // only closes, so a lease inherited by a live encoder keeps the lock
        // held after the app's own descriptor goes away.
        self.lock.take();
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        self.release();
    }
}

/// A field update for [`Journal::update`].
#[derive(Debug, Default, Clone)]
pub struct ObjectUpdate(BTreeMap<String, Value>);

impl ObjectUpdate {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with(mut self, key: impl Into<String>, value: Value) -> Self {
        self.insert(key, value);
        self
    }

    pub fn insert(&mut self, key: impl Into<String>, value: Value) {
        self.0.insert(key.into(), value);
    }
}

impl From<BTreeMap<String, Value>> for ObjectUpdate {
    fn from(value: BTreeMap<String, Value>) -> Self {
        Self(value)
    }
}

/// Recover every interrupted take named by `markers`.
///
/// Any take whose lock is still held is reported as pending and left alone;
/// its encoder inherited the fd and is still writing.
pub fn recover_interrupted<R: crate::commands::CommandRunner>(
    workspace: &Workspace,
    runner: &R,
    markers: &[PathBuf],
) -> io::Result<RecoveryOutcome> {
    let mut outcome = RecoveryOutcome::default();
    for marker in markers {
        let suffix = suffix_of(marker);
        let Some(suffix) = suffix else { continue };
        if !marker.exists() {
            continue;
        }
        // Only markers that live in this workspace's capture directory count.
        if marker
            .parent()
            .map(Path::canonicalize)
            .transpose()
            .ok()
            .flatten()
            != workspace.captures().canonicalize().ok()
        {
            continue;
        }
        let path = swap_suffix_of(marker, suffix, ".mp4");
        let lock_path = swap_suffix(&path, ".recording.lock");
        let lock = match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
        {
            Ok(lock) => lock,
            Err(err) => {
                // A single unreadable lock must not stop recovery of later takes.
                outcome.failed.push(format!("{}: {err}", path.display()));
                continue;
            }
        };
        if lock.try_lock().is_err() {
            outcome.pending.push(marker.clone());
            continue;
        }
        // Re-check under the lock: a take may have committed between the
        // existence check above and acquiring the flock. Removing its lock
        // here matches the source and leaves the finished output untouched.
        if !marker.exists() {
            let _ = fs::remove_file(&lock_path);
            continue;
        }

        let result = recover_one(workspace, runner, &path, &lock_path, suffix, &mut outcome);
        if let Err(err) = result {
            outcome.failed.push(format!("{}: {err}", path.display()));
            // Keep the marker for a later retry after freeing space.
        }
    }
    Ok(outcome)
}

fn recover_one<R: crate::commands::CommandRunner>(
    workspace: &Workspace,
    runner: &R,
    path: &Path,
    lock_path: &Path,
    suffix: &str,
    outcome: &mut RecoveryOutcome,
) -> io::Result<()> {
    let marker = swap_suffix_of(path, ".mp4", suffix);
    let record = fs::read_to_string(&marker)
        .ok()
        .and_then(|text| crate::json::parse(&text).ok())
        .filter(|value| value.as_object().is_some());

    restore_dnd(runner, record.as_ref());

    if suffix == DISCARD_SUFFIX
        || record
            .as_ref()
            .and_then(|r| r.get("phase"))
            .and_then(Value::as_str)
            == Some("discarding")
    {
        remove_capture(workspace, path);
        let _ = fs::remove_file(&marker);
        let _ = fs::remove_file(lock_path);
        return Ok(());
    }

    let metadata = workspace.metadata(path);
    // A final sidecar may have committed just before the app died.
    if matches!(
        metadata.get("kind").and_then(Value::as_str),
        Some("video" | "gif")
    ) && metadata.get("created").is_some()
    {
        let completed = if metadata.get("kind").and_then(Value::as_str) == Some("gif") {
            path.with_extension("gif")
        } else {
            path.to_path_buf()
        };
        if completed.is_file() {
            let _ = fs::remove_file(&marker);
            let _ = fs::remove_file(lock_path);
            return Ok(());
        }
    }

    let options = record
        .as_ref()
        .and_then(|r| r.get("options"))
        .cloned()
        .unwrap_or(Value::Null);
    let message = "OmniShot closed unexpectedly. The recorded video has been recovered.";
    match recovery::recover(workspace, runner, path, &options, message, true) {
        Ok(Some(saved)) => {
            let mut metadata = workspace.metadata(path);
            if let Value::Object(entries) = &mut metadata {
                entries.insert("created".to_owned(), Value::Number(now_seconds()));
            }
            let _ = workspace.atomic_json(path, &metadata);
            outcome.recovered.push((saved, message.to_owned()));
            let _ = fs::remove_file(&marker);
            let _ = fs::remove_file(lock_path);
        }
        Ok(None) => {
            let source = workspace
                .tracks()
                .join(format!("{}-source.mp4", file_stem(path)));
            if !path.exists() && !source.exists() {
                // The app exited before the encoder wrote any video.
                let _ = fs::remove_file(&marker);
                let _ = fs::remove_file(lock_path);
            } else {
                outcome.failed.push(path.display().to_string());
                let failed = swap_suffix_of(path, ".mp4", FAILED_SUFFIX);
                let _ = fs::rename(&marker, &failed);
                let _ = fs::remove_file(lock_path);
            }
        }
        Err(err) => return Err(err),
    }
    Ok(())
}

/// Remove a capture and every sidecar that belongs to it.
fn remove_capture(workspace: &Workspace, path: &Path) {
    let _ = fs::remove_file(path);
    let _ = fs::remove_file(path.with_extension("json"));
    let _ = fs::remove_file(path.with_extension("log"));
    let _ = fs::remove_file(swap_suffix(path, ".studio.json"));
    let _ = fs::remove_file(swap_suffix(path, ".studio.checkpoint"));
    let _ = fs::remove_file(swap_suffix(path, ".video-edit.json"));
    // The frame-origin trace the stitcher writes next to the video.
    let _ = fs::remove_file(PathBuf::from(format!("{}.ts", path.display())));
    if path.extension().is_some_and(|ext| ext == "gif") {
        let mp4 = path.with_extension("mp4");
        let _ = fs::remove_file(&mp4);
        let _ = fs::remove_file(swap_suffix(&mp4, ".video-edit.json"));
    }
    let stem = file_stem(path);
    // An abrupt exit can precede the camera sidecar write.
    for suffix in ["-camera.mp4", "-camera.camera-log"] {
        let _ = fs::remove_file(workspace.tracks().join(format!("{stem}{suffix}")));
    }
}

/// Only live markers are recoverable; `.failed-recording.json` is terminal.
fn suffix_of(marker: &Path) -> Option<&'static str> {
    let text = marker.to_string_lossy();
    [MARKER_SUFFIX, DISCARD_SUFFIX]
        .into_iter()
        .find(|suffix| text.ends_with(suffix))
}

/// Put the user's Do-Not-Disturb state back after a crash left it toggled.
///
/// The source only recognizes the string states the desktop shell reports; a
/// missing or unrecognized value is ignored, and a failed restore must not
/// stop the rest of recovery.
fn restore_dnd<R: crate::commands::CommandRunner>(runner: &R, record: Option<&Value>) {
    let Some(Value::String(state)) = record.and_then(|r| r.get("old_dnd")) else {
        return;
    };
    if matches!(state.as_str(), "on" | "off" | "true" | "false") {
        let spec = CommandSpec::new("omarchy-shell").args(["notifications", "setDnd", state]);
        let _ = runner.run(&spec);
    }
}

/// Replace `old` with `new` at the end of `path`'s file name.
fn swap_suffix_of(path: &Path, old: &str, new: &str) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(stem) = text.strip_suffix(old) {
        PathBuf::from(format!("{stem}{new}"))
    } else {
        path.to_path_buf()
    }
}

/// Replace the extension of `path` with `suffix` (which includes the dot).
fn swap_suffix(path: &Path, suffix: &str) -> PathBuf {
    let stem = path.with_extension("");
    PathBuf::from(format!("{}{suffix}", stem.display()))
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandOutput, CommandRunner};
    use std::sync::Mutex;

    struct StubRunner {
        valid: bool,
        seen: Mutex<Vec<String>>,
    }

    impl CommandRunner for StubRunner {
        fn run(
            &self,
            spec: &crate::commands::CommandSpec,
        ) -> Result<CommandOutput, crate::commands::CommandError> {
            self.seen.lock().unwrap().push(spec.program.clone());
            if self.valid {
                Ok(CommandOutput {
                    status: 0,
                    stdout: br#"{"streams":[{"width":640,"height":480}]}"#.to_vec(),
                    stderr: Vec::new(),
                })
            } else {
                Ok(CommandOutput {
                    status: 0,
                    stdout: br#"{"streams":[]}"#.to_vec(),
                    stderr: Vec::new(),
                })
            }
        }
    }

    fn scratch(name: &str) -> Workspace {
        let dir =
            std::env::temp_dir().join(format!("tidyshot-journal-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let workspace = Workspace::new(dir);
        workspace.ensure().unwrap();
        workspace
    }

    fn empty_options() -> Value {
        Value::Object(BTreeMap::new())
    }

    #[test]
    fn active_take_is_pending_not_recovered() {
        let workspace = scratch("pending");
        let path = workspace.captures().join("take.mp4");
        fs::write(&path, b"video").unwrap();
        let journal = Journal::begin(workspace.clone(), &path, empty_options()).unwrap();
        let marker = journal.marker().to_path_buf();

        let runner = StubRunner {
            valid: true,
            seen: Mutex::new(Vec::new()),
        };
        // The journal still holds the lease, so recovery must leave it alone.
        let outcome =
            recover_interrupted(&workspace, &runner, std::slice::from_ref(&marker)).unwrap();
        assert_eq!(outcome.pending, vec![marker.clone()]);
        assert!(outcome.recovered.is_empty());
        assert!(outcome.failed.is_empty());
        assert!(path.exists());
    }

    #[test]
    fn failed_marker_is_terminal_and_never_reprocessed() {
        let workspace = scratch("failed-marker");
        let path = workspace.captures().join("take.mp4");
        fs::write(&path, b"partial").unwrap();
        let marker = swap_suffix(&path, MARKER_SUFFIX);
        let failed = swap_suffix(&path, FAILED_SUFFIX);
        fs::write(&marker, b"{\"phase\":\"recording\"}").unwrap();
        fs::rename(&marker, &failed).unwrap();

        let runner = StubRunner {
            valid: false,
            seen: Mutex::new(Vec::new()),
        };
        let outcome =
            recover_interrupted(&workspace, &runner, std::slice::from_ref(&failed)).unwrap();
        assert!(outcome.recovered.is_empty());
        assert!(outcome.failed.is_empty());
        assert!(outcome.pending.is_empty());
        assert!(failed.exists());
    }

    #[test]
    fn unreadable_lock_does_not_stop_later_markers() {
        let workspace = scratch("bad-lock");
        let blocked = workspace.captures().join("blocked.mp4");
        fs::write(blocked.with_extension("recording.json"), b"{}").unwrap();
        // A directory where the lock file belongs makes open() fail.
        fs::create_dir_all(blocked.with_extension("recording.lock")).unwrap();

        let good = workspace.captures().join("good.mp4");
        let source = workspace.tracks().join("good-source.mp4");
        fs::write(&source, b"complete take").unwrap();
        let journal = Journal::begin(workspace.clone(), &good, empty_options()).unwrap();
        let good_marker = journal.marker().to_path_buf();
        drop(journal);

        let runner = StubRunner {
            valid: true,
            seen: Mutex::new(Vec::new()),
        };
        let markers = vec![
            blocked.with_extension("recording.json"),
            good_marker.clone(),
        ];
        let outcome = recover_interrupted(&workspace, &runner, &markers).unwrap();
        assert_eq!(outcome.recovered.len(), 1);
        assert_eq!(outcome.recovered[0].0, good.display().to_string());
        assert!(outcome.failed.len() == 1);
        assert!(!good_marker.exists());
    }

    #[test]
    fn dnd_state_is_restored_when_recovering() {
        let workspace = scratch("dnd");
        let path = workspace.captures().join("take.mp4");
        fs::write(&path, b"partial").unwrap();
        let marker = swap_suffix(&path, MARKER_SUFFIX);
        fs::write(&marker, b"{\"old_dnd\":\"on\"}").unwrap();

        let runner = StubRunner {
            valid: false,
            seen: Mutex::new(Vec::new()),
        };
        recover_interrupted(&workspace, &runner, &[marker]).unwrap();
        let seen = runner.seen.lock().unwrap();
        assert_eq!(seen.first().map(String::as_str), Some("omarchy-shell"));
    }

    #[test]
    fn cancelled_take_is_discarded_with_camera_sidecars() {
        let workspace = scratch("discard");
        let path = workspace.captures().join("take.mp4");
        fs::write(&path, b"partial").unwrap();
        let camera = workspace.tracks().join("take-camera.mp4");
        fs::write(&camera, b"camera").unwrap();
        let log = workspace.tracks().join("take-camera.camera-log");
        fs::write(&log, b"log").unwrap();

        let mut journal = Journal::begin(workspace.clone(), &path, empty_options()).unwrap();
        journal.discard();
        assert!(journal.marker().to_string_lossy().ends_with(DISCARD_SUFFIX));
        journal.close();

        let runner = StubRunner {
            valid: true,
            seen: Mutex::new(Vec::new()),
        };
        let markers = vec![journal.marker().to_path_buf()];
        let outcome = recover_interrupted(&workspace, &runner, &markers).unwrap();
        assert!(outcome.recovered.is_empty());
        assert!(outcome.failed.is_empty());
        assert!(outcome.pending.is_empty());
        assert!(!path.exists());
        assert!(!camera.exists());
        assert!(!log.exists());
    }

    #[test]
    fn unrecoverable_take_is_kept_and_marked_failed() {
        let workspace = scratch("failed");
        let path = workspace.captures().join("take.mp4");
        fs::write(&path, b"partial").unwrap();
        let journal = Journal::begin(workspace.clone(), &path, empty_options()).unwrap();
        let marker = journal.marker().to_path_buf();

        let runner = StubRunner {
            valid: false,
            seen: Mutex::new(Vec::new()),
        };
        // The journal still holds the lease; drop it so recovery can take over.
        drop(journal);
        let outcome = recover_interrupted(&workspace, &runner, &[marker]).unwrap();
        assert_eq!(outcome.failed, vec![path.display().to_string()]);
        assert_eq!(fs::read(&path).unwrap(), b"partial");
        assert!(swap_suffix(&path, FAILED_SUFFIX).exists());
    }

    #[test]
    fn recoverable_take_is_moved_back_and_marker_removed() {
        let workspace = scratch("recover");
        let path = workspace.captures().join("take.mp4");
        let source = workspace.tracks().join("take-source.mp4");
        fs::write(&source, b"complete take").unwrap();
        let journal = Journal::begin(workspace.clone(), &path, empty_options()).unwrap();
        let marker = journal.marker().to_path_buf();
        drop(journal);

        let runner = StubRunner {
            valid: true,
            seen: Mutex::new(Vec::new()),
        };
        let outcome =
            recover_interrupted(&workspace, &runner, std::slice::from_ref(&marker)).unwrap();
        assert_eq!(outcome.recovered.len(), 1);
        assert!(outcome.failed.is_empty());
        assert!(!marker.exists());
        assert_eq!(fs::read(&path).unwrap(), b"complete take");
    }

    #[test]
    fn completed_take_does_not_reappear() {
        let workspace = scratch("complete");
        let path = workspace.captures().join("take.mp4");
        fs::write(&path, b"video").unwrap();

        let mut journal = Journal::begin(workspace.clone(), &path, empty_options()).unwrap();
        journal.complete();
        assert!(!journal.marker().exists());
    }
}
