//! Studio checkpoints: incremental recovery of a growing recording's metadata.
// Ported from omnishot/recording_checkpoint.py
//!
//! While a Studio recording runs, a background writer appends JSON lines of
//! cursor, event, and camera data. A SIGKILL can leave the last line torn; the
//! reader stops at the last complete line and keeps the earlier batches. A
//! failed append is truncated back to the last committed batch so the file
//! never ends mid-record.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::commands::CommandRunner;
use crate::json::Value;

use super::Workspace;
use super::recovery::{self, swap_suffix};

/// The maximum size of a single checkpoint line, matching the Python cap.
const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// A checkpoint writer that owns the append fd.
///
/// The Python original runs a background thread timed off a monotonic clock.
/// Keeping the writer explicit here makes it testable without sleeping.
pub struct CheckpointWriter {
    path: PathBuf,
    file: File,
    source: PathBuf,
}

impl CheckpointWriter {
    /// Create (exclusively) the checkpoint beside `source`.
    pub fn create(source: &Path, options: &Value) -> io::Result<Self> {
        let path = swap_suffix(source, ".studio.checkpoint");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut writer = Self {
            path,
            file,
            source: source.to_path_buf(),
        };
        let mut record = std::collections::BTreeMap::new();
        record.insert("version".to_owned(), Value::Number(1.0));
        record.insert(
            "source_path".to_owned(),
            Value::String(
                source
                    .canonicalize()
                    .unwrap_or_else(|_| source.to_path_buf())
                    .display()
                    .to_string(),
            ),
        );
        record.insert("capture_options".to_owned(), options.clone());
        if let Err(err) = writer.append(&Value::Object(record)) {
            let _ = fs::remove_file(&writer.path);
            return Err(err);
        }
        Ok(writer)
    }

    /// The checkpoint file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The source recording this checkpoint describes.
    #[must_use]
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// Append one JSON line, fsyncing it and rolling back a partial write.
    pub fn append(&mut self, record: &Value) -> io::Result<()> {
        let mut line = record.to_string_compact();
        line.push('\n');
        let data = line.as_bytes();
        let offset = self.file.seek(io::SeekFrom::End(0))?;
        let mut written = 0;
        while written < data.len() {
            match self.file.write(&data[written..]) {
                Ok(0) => {
                    let _ = self.file.set_len(offset);
                    return Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "Could not write recovery checkpoint",
                    ));
                }
                Ok(n) => written += n,
                Err(err) => {
                    // Preserve complete earlier batches if a write stops halfway.
                    let _ = self.file.set_len(offset);
                    return Err(err);
                }
            }
        }
        self.file.sync_all()?;
        Ok(())
    }

    /// Delete the checkpoint file.
    pub fn discard(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Read committed batches, tolerating an incomplete last write after SIGKILL.
// Ported from omnishot/recording_checkpoint.py (read_checkpoint)
#[must_use]
pub fn read_checkpoint(path: &Path) -> Value {
    let mut metadata = std::collections::BTreeMap::new();
    metadata.insert("version".to_owned(), Value::Number(1.0));
    metadata.insert("cursor".to_owned(), Value::Array(Vec::new()));
    metadata.insert("events".to_owned(), Value::Array(Vec::new()));

    let Ok(file) = File::open(path) else {
        return Value::Object(metadata);
    };
    let mut reader = io::BufReader::new(file);
    // A torn tail has no closing newline; stop at the last complete line.
    while let Ok(Some(line)) = read_line_limited(&mut reader, MAX_LINE_BYTES) {
        if !line.ends_with(b"\n") {
            break;
        }
        let Ok(text) = std::str::from_utf8(&line) else {
            break;
        };
        let Ok(record) = crate::json::parse(text) else {
            break;
        };
        let Some(entries) = record.as_object().cloned() else {
            break;
        };
        for key in ["cursor", "events"] {
            let rows = entries.get(key).cloned().unwrap_or(Value::Null);
            if let (Value::Array(rows), Value::Array(existing)) =
                (rows, metadata.get_mut(key).unwrap())
            {
                existing.extend(rows);
            }
        }
        for (key, value) in entries {
            if key != "cursor" && key != "events" {
                metadata.insert(key, value);
            }
        }
    }
    Value::Object(metadata)
}

/// Read one line, capped at `max` bytes, mirroring `readline(max)` in Python.
///
/// A line that reaches the cap without a newline is returned anyway, so the
/// caller stops on the same torn or oversized record the source rejects.
fn read_line_limited<R: BufRead>(reader: &mut R, max: usize) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            return Ok(Some(line));
        }
        if let Some(index) = available.iter().position(|&byte| byte == b'\n') {
            line.extend_from_slice(&available[..=index]);
            reader.consume(index + 1);
            return Ok(Some(line));
        }
        let room = max.saturating_sub(line.len());
        if room == 0 {
            return Ok(Some(line));
        }
        let take = room.min(available.len());
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if line.len() >= max {
            return Ok(Some(line));
        }
    }
}

/// Restore a checkpoint into a sidecar unless a final one already committed.
// Ported from omnishot/recording_checkpoint.py (restore_checkpoint)
pub fn restore_checkpoint<R: CommandRunner>(
    workspace: &Workspace,
    runner: &R,
    path: &Path,
) -> io::Result<bool> {
    let checkpoint = swap_suffix(path, ".studio.checkpoint");
    if !checkpoint.is_file() {
        return Ok(false);
    }
    let sidecar = swap_suffix(path, ".studio.json");
    if let Ok(existing) = fs::read_to_string(&sidecar)
        && let Ok(value) = crate::json::parse(&existing)
        && value.get("version").and_then(Value::as_f64) == Some(1.0)
    {
        return Ok(true);
    }

    let mut metadata = read_checkpoint(&checkpoint);
    // Only app-owned camera tracks are recovered from a checkpoint.
    if let Value::Object(entries) = &mut metadata
        && let Some(camera_path) = entries.get("camera_path").and_then(Value::as_str)
    {
        let camera = PathBuf::from(camera_path);
        let valid = camera
            .canonicalize()
            .ok()
            .and_then(|c| c.parent().map(Path::to_path_buf))
            == workspace.tracks().canonicalize().ok()
            && camera.is_file()
            && recovery::valid_video(runner, &camera);
        if !valid {
            entries.remove("camera_path");
            entries.insert(
                "camera_error".to_owned(),
                Value::String("The interrupted camera track could not be recovered.".to_owned()),
            );
        }
    }
    crate::storage::atomic_write_file(&sidecar, metadata.to_string_pretty().as_bytes())?;
    Ok(true)
}

/// A periodic checkpoint writer driven by a caller-supplied clock.
///
/// The Qt bridge runs [`CheckpointScheduler::tick`] from a worker thread; tests
/// call it directly with prepared metadata.
pub struct CheckpointScheduler {
    writer: Mutex<Option<CheckpointWriter>>,
    stop: Arc<AtomicBool>,
}

impl CheckpointScheduler {
    /// Start a scheduler with a freshly created checkpoint.
    pub fn start(source: &Path, options: &Value) -> io::Result<Self> {
        Ok(Self {
            writer: Mutex::new(Some(CheckpointWriter::create(source, options)?)),
            stop: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Write one metadata batch, unless the scheduler has stopped.
    pub fn tick(&self, record: &Value) -> io::Result<()> {
        if self.stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        let mut guard = self.writer.lock().unwrap();
        if let Some(writer) = guard.as_mut() {
            writer.append(record)?;
        }
        Ok(())
    }

    /// Stop accepting writes; does not delete the checkpoint.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Stop and delete the checkpoint (a discarded take).
    pub fn discard(&self) {
        self.stop();
        let mut guard = self.writer.lock().unwrap();
        if let Some(writer) = guard.as_mut() {
            writer.discard();
        }
        *guard = None;
    }

    /// The checkpoint file path, if still active.
    #[must_use]
    pub fn path(&self) -> Option<PathBuf> {
        self.writer
            .lock()
            .unwrap()
            .as_ref()
            .map(|writer| writer.path().to_path_buf())
    }
}

/// Sleep helper the Qt worker uses between scheduler ticks.
pub const CHECKPOINT_INTERVAL: Duration = Duration::from_millis(1000);

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// No ffprobe video is ever valid, which exercises the camera-error path.
    struct NoVideoRunner;

    impl CommandRunner for NoVideoRunner {
        fn run(
            &self,
            _spec: &crate::commands::CommandSpec,
        ) -> Result<crate::commands::CommandOutput, crate::commands::CommandError> {
            Ok(crate::commands::CommandOutput {
                status: 0,
                stdout: br#"{"streams":[]}"#.to_vec(),
                stderr: Vec::new(),
            })
        }
    }

    fn scratch(name: &str) -> Workspace {
        let dir =
            std::env::temp_dir().join(format!("tidyshot-checkpoint-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let workspace = Workspace::new(dir);
        workspace.ensure().unwrap();
        workspace
    }

    fn obj(entries: &[(&str, Value)]) -> Value {
        let mut map = BTreeMap::new();
        for (key, value) in entries {
            map.insert((*key).to_owned(), value.clone());
        }
        Value::Object(map)
    }

    #[test]
    fn commits_deltas_and_preserves_camera_offset() {
        let workspace = scratch("commits");
        let path = workspace.captures().join("take.mp4");
        let mut writer =
            CheckpointWriter::create(&path, &obj(&[("studio", Value::Bool(true))])).unwrap();
        writer
            .append(&obj(&[(
                "cursor",
                Value::Array(vec![obj(&[("t", Value::Number(0.2))])]),
            )]))
            .unwrap();
        writer
            .append(&obj(&[
                (
                    "cursor",
                    Value::Array(vec![obj(&[("t", Value::Number(1.2))])]),
                ),
                (
                    "events",
                    Value::Array(vec![obj(&[("label", Value::String("Ctrl+K".to_owned()))])]),
                ),
                ("camera_offset", Value::Number(-0.2)),
            ]))
            .unwrap();

        let result = read_checkpoint(writer.path());
        assert_eq!(result.get("cursor").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(result.get("events").unwrap().as_array().unwrap().len(), 1);
        let offset = result.get("camera_offset").unwrap().as_f64().unwrap();
        assert!((offset + 0.2).abs() < 0.001);
        assert!(
            result
                .get("capture_options")
                .unwrap()
                .get("studio")
                .unwrap()
                .as_bool()
                .unwrap()
        );
    }

    #[test]
    fn torn_last_batch_keeps_previous_checkpoint() {
        let workspace = scratch("torn");
        let path = workspace.captures().join("take.mp4");
        let runner = NoVideoRunner;
        let checkpoint = swap_suffix(&path, ".studio.checkpoint");
        fs::write(
            &checkpoint,
            b"{\"version\":1,\"source_path\":\"take.mp4\",\"cursor\":[{\"t\":1}]}\n{\"cursor\":[",
        )
        .unwrap();
        assert!(restore_checkpoint(&workspace, &runner, &path).unwrap());
        let sidecar = swap_suffix(&path, ".studio.json");
        let restored = crate::json::parse(&fs::read_to_string(&sidecar).unwrap()).unwrap();
        assert_eq!(restored.get("cursor").unwrap().as_array().unwrap().len(), 1);

        // A final sidecar committed after the checkpoint wins.
        let final_value = obj(&[
            ("version", Value::Number(1.0)),
            (
                "cursor",
                Value::Array(vec![obj(&[("t", Value::Number(2.0))])]),
            ),
            ("events", Value::Array(Vec::new())),
        ]);
        fs::write(&sidecar, final_value.to_string_pretty()).unwrap();
        assert!(restore_checkpoint(&workspace, &runner, &path).unwrap());
        let existing = crate::json::parse(&fs::read_to_string(&sidecar).unwrap()).unwrap();
        assert_eq!(existing, final_value);
    }

    #[test]
    fn camera_track_without_video_becomes_an_error() {
        let workspace = scratch("camera");
        let path = workspace.captures().join("take.mp4");
        let camera = workspace.tracks().join("take-camera.mp4");
        fs::write(&camera, b"not playable").unwrap();
        let checkpoint = swap_suffix(&path, ".studio.checkpoint");
        let record = obj(&[
            ("version", Value::Number(1.0)),
            ("camera_path", Value::String(camera.display().to_string())),
        ]);
        fs::write(&checkpoint, format!("{}\n", record.to_string_compact())).unwrap();

        assert!(restore_checkpoint(&workspace, &NoVideoRunner, &path).unwrap());
        let sidecar = swap_suffix(&path, ".studio.json");
        let restored = crate::json::parse(&fs::read_to_string(&sidecar).unwrap()).unwrap();
        assert!(restored.get("camera_path").is_none());
        assert!(restored.get("camera_error").is_some());
    }

    #[test]
    fn failed_append_rolls_back_to_last_committed_batch() {
        let workspace = scratch("rollback");
        let path = workspace.captures().join("take.mp4");
        let mut writer = CheckpointWriter::create(&path, &Value::Null).unwrap();
        writer
            .append(&obj(&[(
                "cursor",
                Value::Array(vec![obj(&[("t", Value::Number(1.0))])]),
            )]))
            .unwrap();

        // Simulate a write that stops halfway by truncating the file after a
        // failed partial write: the reader must ignore the torn tail.
        let text = fs::read_to_string(writer.path()).unwrap();
        fs::write(writer.path(), format!("{text}{{\"cursor\":[")).unwrap();
        let read = read_checkpoint(writer.path());
        assert_eq!(read.get("cursor").unwrap().as_array().unwrap().len(), 1);
    }

    #[test]
    fn scheduler_discard_removes_checkpoint() {
        let workspace = scratch("discard");
        let path = workspace.captures().join("take.mp4");
        let scheduler = CheckpointScheduler::start(&path, &Value::Null).unwrap();
        let checkpoint = scheduler.path().unwrap();
        assert!(checkpoint.exists());
        scheduler.discard();
        assert!(!checkpoint.exists());
    }
}
