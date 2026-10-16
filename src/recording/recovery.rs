//! Recovery for a recording whose presentation export failed.
// Ported from omnishot/recording_recovery.py
//!
//! When export fails, the immutable source track is still on disk. Recovery
//! hard-links (or copies) it over the failed output and records why, so the
//! history entry stays usable. A full disk must not cost the video: the sidecar
//! write is best-effort and history can discover the MP4 without it.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::commands::{CommandRunner, CommandSpec};
use crate::json::Value;

use super::{Workspace, checkpoint};

/// The result of recovering interrupted takes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecoveryOutcome {
    /// Recordings that were recovered, with the message written to the sidecar.
    pub recovered: Vec<(String, String)>,
    /// Recordings that could not be recovered and were marked failed.
    pub failed: Vec<String>,
    /// Recordings still leased by a live encoder.
    pub pending: Vec<PathBuf>,
}

/// Check a video with `ffprobe`, returning true when it has a usable stream.
// Ported from omnishot/recording_recovery.py (valid_video)
#[must_use]
pub fn valid_video<R: CommandRunner>(runner: &R, path: &Path) -> bool {
    let spec = CommandSpec::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "json",
        ])
        .arg(path.to_string_lossy().into_owned())
        .timeout(std::time::Duration::from_secs(10));
    let Ok(output) = runner.run(&spec) else {
        return false;
    };
    let Ok(value) = crate::json::parse(&output.stdout_string()) else {
        return false;
    };
    value
        .get("streams")
        .and_then(Value::as_array)
        .is_some_and(|streams| {
            streams.iter().any(|stream| {
                stream.get("width").and_then(Value::as_f64).unwrap_or(0.0) > 0.0
                    && stream.get("height").and_then(Value::as_f64).unwrap_or(0.0) > 0.0
            })
        })
}

/// Recover `path` from its source track, returning the saved path on success.
///
/// `prefer_source` forces the source track to replace an output that is
/// playable but incomplete, which is what the journal wants for an interrupted
/// export.
pub fn recover<R: CommandRunner>(
    workspace: &Workspace,
    runner: &R,
    path: &Path,
    options: &Value,
    message: &str,
    prefer_source: bool,
) -> io::Result<Option<String>> {
    let metadata_path = swap_suffix(path, ".studio.json");
    let restored = checkpoint::restore_checkpoint(workspace, runner, path).unwrap_or(false);

    let metadata = fs::read_to_string(&metadata_path)
        .ok()
        .and_then(|text| crate::json::parse(&text).ok())
        .filter(|value| value.as_object().is_some())
        .unwrap_or_else(|| Value::Object(std::collections::BTreeMap::new()));
    let mut candidates = Vec::new();
    if let Some(source) = metadata.get("source_path").and_then(Value::as_str) {
        candidates.push(PathBuf::from(source));
    }
    candidates.push(
        workspace
            .tracks()
            .join(format!("{}-source.mp4", file_stem(path))),
    );

    let output_valid = valid_video(runner, path);
    let mut raw = None;
    if prefer_source {
        raw = candidates
            .iter()
            .find(|candidate| {
                candidate.canonicalize().ok() != path.canonicalize().ok()
                    && candidate.is_file()
                    && valid_video(runner, candidate)
            })
            .cloned();
    }
    if raw.is_none() && (prefer_source || !output_valid) {
        raw = candidates
            .iter()
            .find(|candidate| candidate.is_file() && valid_video(runner, candidate))
            .cloned();
    }

    if let Some(raw) = raw {
        let temporary = path.with_file_name(format!("{}-recover-{}.mp4", file_stem(path), nonce()));
        let linked =
            fs::hard_link(&raw, &temporary).or_else(|_| fs::copy(&raw, &temporary).map(|_| ()));
        if let Err(err) = linked {
            let _ = fs::remove_file(&temporary);
            return Err(err);
        }
        if let Err(err) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(err);
        }
    } else if !output_valid {
        return Ok(None);
    }

    let mut record = workspace.metadata(path);
    if let Value::Object(entries) = &mut record {
        entries.insert("kind".to_owned(), Value::String("video".to_owned()));
        entries
            .entry("created".to_owned())
            .or_insert_with(|| Value::Number(now_seconds()));
        entries.insert("options".to_owned(), options.clone());
        entries.insert(
            "recovery_error".to_owned(),
            Value::String(message.to_owned()),
        );
    }
    // History can discover the valid MP4 without its optional sidecar, even
    // when a full disk prevented saving the recovery message.
    let sidecar = path.with_extension("json");
    let _ = crate::storage::atomic_write_file(&sidecar, record.to_string_pretty().as_bytes());

    if restored {
        let _ = fs::remove_file(swap_suffix(path, ".studio.checkpoint"));
    }
    Ok(Some(path.display().to_string()))
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn nonce() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{:x}", nanos & 0xFFFF_FFFF)
}

/// Replace the extension of `path` with `suffix` (which includes the dot).
pub(crate) fn swap_suffix(path: &Path, suffix: &str) -> PathBuf {
    let stem = path.with_extension("");
    PathBuf::from(format!("{}{suffix}", stem.display()))
}

fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandError, CommandOutput, CommandRunner};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct StubRunner {
        valid_paths: Arc<Mutex<Vec<PathBuf>>>,
    }

    impl StubRunner {
        fn new(valid: Vec<PathBuf>) -> Self {
            Self {
                valid_paths: Arc::new(Mutex::new(valid)),
            }
        }
    }

    impl CommandRunner for StubRunner {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            let target = PathBuf::from(spec.args.last().cloned().unwrap_or_default());
            let valid = self
                .valid_paths
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.canonicalize().ok() == target.canonicalize().ok());
            let streams = if valid {
                r#"[{"width":640,"height":480}]"#
            } else {
                "[]"
            };
            Ok(CommandOutput {
                status: 0,
                stdout: format!(r#"{{"streams":{streams}}}"#).into_bytes(),
                stderr: Vec::new(),
            })
        }
    }

    fn scratch(name: &str) -> Workspace {
        let dir =
            std::env::temp_dir().join(format!("tidyshot-recovery-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let workspace = Workspace::new(dir);
        workspace.ensure().unwrap();
        workspace
    }

    #[test]
    fn recovers_source_when_output_is_partial() {
        let workspace = scratch("partial");
        let path = workspace.captures().join("capture.mp4");
        fs::write(&path, b"partial export").unwrap();
        let source = workspace.tracks().join("capture-source.mp4");
        fs::write(&source, b"valid original").unwrap();
        let runner = StubRunner::new(vec![source.clone()]);

        let result = recover(
            &workspace,
            &runner,
            &path,
            &Value::Null,
            "export failed",
            false,
        )
        .unwrap();
        assert_eq!(result, Some(path.display().to_string()));
        assert_eq!(fs::read(&path).unwrap(), b"valid original");
    }

    #[test]
    fn full_disk_sidecar_failure_keeps_video_discoverable() {
        let workspace = scratch("diskfull");
        let path = workspace.captures().join("capture.mp4");
        let source = workspace.tracks().join("capture-source.mp4");
        fs::write(&source, b"valid original").unwrap();
        let runner = StubRunner::new(vec![source.clone()]);

        // Make the sidecar path a directory so the atomic write fails.
        fs::create_dir_all(path.with_extension("json")).unwrap();
        let result = recover(&workspace, &runner, &path, &Value::Null, "disk full", false).unwrap();
        assert_eq!(result, Some(path.display().to_string()));
        assert_eq!(fs::read(&path).unwrap(), b"valid original");
    }

    #[test]
    fn invalid_sources_remain_untouched() {
        let workspace = scratch("invalid");
        let path = workspace.captures().join("capture.mp4");
        fs::write(&path, b"invalid source").unwrap();
        let runner = StubRunner::new(Vec::new());
        let result = recover(
            &workspace,
            &runner,
            &path,
            &Value::Null,
            "encoder failed",
            false,
        )
        .unwrap();
        assert_eq!(result, None);
        assert_eq!(fs::read(&path).unwrap(), b"invalid source");
    }

    #[test]
    fn prefer_source_replaces_a_playable_but_incomplete_output() {
        let workspace = scratch("prefer");
        let path = workspace.captures().join("capture.mp4");
        fs::write(&path, b"first few frames").unwrap();
        let source = workspace.tracks().join("capture-source.mp4");
        fs::write(&source, b"complete take").unwrap();
        let runner = StubRunner::new(vec![path.clone(), source.clone()]);

        assert_eq!(
            recover(
                &workspace,
                &runner,
                &path,
                &Value::Null,
                "interrupted",
                true
            )
            .unwrap(),
            Some(path.display().to_string())
        );
        assert_eq!(fs::read(&path).unwrap(), b"complete take");
        assert_eq!(fs::read(&source).unwrap(), b"complete take");
    }
}
