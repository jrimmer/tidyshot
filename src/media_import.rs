//! Validate a local recording or image before importing it into history.
// Ported from omnishot/media_import.py
//!
//! Probing only the header accepts truncated tracks, so a recording must also
//! decode its first frame. Nothing is copied or modified until validation
//! succeeds, and the user's originals are never touched.

use std::path::Path;
use std::time::Duration;

use crate::commands::{CommandError, CommandRunner, CommandSpec};
use crate::json::Value;

/// The 120 megapixel ceiling shared with screenshots and scrolling captures.
pub const MAX_PIXELS: f64 = 120_000_000.0;

/// Why a recording or image could not be imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportError {
    message: String,
}

impl ImportError {
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ImportError {}

fn failure(detail: impl std::fmt::Display) -> ImportError {
    ImportError {
        message: detail.to_string(),
    }
}

/// Confirm `path` is a decodable video with usable dimensions.
///
/// A missing or non-video first stream, dimensions outside the megapixel
/// bound, and a header-only track all fail. The returned value is the first
/// stream's pixel count, so callers can report the size without re-probing.
// Ported from omnishot/media_import.py (validate_video)
pub fn validate_video<R: CommandRunner>(
    runner: &R,
    path: &Path,
) -> Result<(u32, u32), ImportError> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("recording");
    let probe = CommandSpec::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,codec_type",
            "-of",
            "json",
        ])
        .arg(path.to_string_lossy().into_owned())
        .timeout(Duration::from_secs(30));
    let (width, height) = match runner.run(&probe) {
        Ok(output) => match parse_stream(&output.stdout_string()) {
            Ok(Some(stream)) => stream,
            Ok(None) => {
                return Err(failure(format!(
                    "Could not open recording {name}: There is no video track"
                )));
            }
            Err(error) => {
                return Err(failure(format!("Could not open recording {name}: {error}")));
            }
        },
        Err(error) => {
            return Err(failure(format!(
                "Could not open recording {name}: {}",
                characterise(&error)
            )));
        }
    };

    let pixels = f64::from(width) * f64::from(height);
    if pixels <= 0.0 || pixels > MAX_PIXELS {
        return Err(failure(format!(
            "Could not open recording {name}: Unsupported video dimensions"
        )));
    }

    // Decode the first frame only; a long recording must not be scanned
    // before its editor opens.
    let decode = CommandSpec::new("ffmpeg")
        .args(["-v", "error", "-xerror", "-i"])
        .arg(path.to_string_lossy().into_owned())
        .args([
            "-map",
            "0:v:0",
            "-frames:v",
            "1",
            "-an",
            "-f",
            "framehash",
            "-",
        ])
        .timeout(Duration::from_secs(30));
    match runner.run(&decode) {
        Ok(output) if has_frame(&output.stdout) => Ok((width, height)),
        Ok(_) => Err(failure(format!(
            "Could not open recording {name}: There are no decodable video frames"
        ))),
        Err(error) => Err(failure(format!(
            "Could not open recording {name}: {}",
            characterise(&error)
        ))),
    }
}

/// A framehash stream has at least one non-comment data line.
fn has_frame(stdout: &[u8]) -> bool {
    String::from_utf8_lossy(stdout).lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with('#')
    })
}

/// Parse the first video stream of an `ffprobe -of json` result.
fn parse_stream(text: &str) -> Result<Option<(u32, u32)>, String> {
    let value = crate::json::parse(text).map_err(|error| error.to_string())?;
    let streams = value
        .get("streams")
        .and_then(Value::as_array)
        .unwrap_or(&[]);
    let Some(first) = streams.first() else {
        return Ok(None);
    };
    if first.get("codec_type").and_then(Value::as_str) != Some("video") {
        return Ok(None);
    }
    let width = first.get("width").and_then(Value::as_f64).unwrap_or(0.0);
    let height = first.get("height").and_then(Value::as_f64).unwrap_or(0.0);
    Ok(Some((width as u32, height as u32)))
}

/// The stderr or stdout text of a failed command, trimmed.
fn characterise(error: &CommandError) -> String {
    match error {
        CommandError::Failure { output, .. } => {
            let stderr = output.stderr_string();
            let stdout = output.stdout_string();
            let detail = if stderr.trim().is_empty() {
                stdout
            } else {
                stderr
            };
            let detail = detail.trim();
            if detail.is_empty() {
                "ffprobe failed".to_owned()
            } else {
                detail.to_owned()
            }
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRunner {
        results: Mutex<Vec<Result<crate::commands::CommandOutput, CommandError>>>,
        seen: Mutex<Vec<CommandSpec>>,
    }

    impl FakeRunner {
        fn push(&self, status: i32, stdout: &str) {
            self.results
                .lock()
                .unwrap()
                .push(Ok(crate::commands::CommandOutput {
                    status,
                    stdout: stdout.as_bytes().to_vec(),
                    stderr: Vec::new(),
                }));
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, spec: &CommandSpec) -> Result<crate::commands::CommandOutput, CommandError> {
            self.seen.lock().unwrap().push(spec.clone());
            self.results.lock().unwrap().remove(0)
        }
    }

    const STREAM: &str = r#"{"streams":[{"codec_type":"video","width":64,"height":64}]}"#;
    const FRAME: &str = "#format,stream,frame\n1,0,0,0,64,64";

    #[test]
    fn valid_video_returns_dimensions() {
        let runner = FakeRunner::default();
        runner.push(0, STREAM);
        runner.push(0, FRAME);
        assert_eq!(validate_video(&runner, Path::new("clip.mp4")), Ok((64, 64)));
        let seen = runner.seen.lock().unwrap();
        assert_eq!(seen[0].program, "ffprobe");
        assert_eq!(seen[1].program, "ffmpeg");
        assert!(
            seen[1]
                .args
                .windows(2)
                .any(|pair| pair == ["-frames:v", "1"])
        );
    }

    #[test]
    fn audio_only_recording_is_rejected() {
        let runner = FakeRunner::default();
        runner.push(0, r#"{"streams":[{"codec_type":"audio"}]}"#);
        let error = validate_video(&runner, Path::new("audio.mp4")).unwrap_err();
        assert!(error.message().contains("no video track"), "{error}");
        assert_eq!(
            error.message(),
            "Could not open recording audio.mp4: There is no video track"
        );
    }

    #[test]
    fn truncated_header_only_track_is_rejected() {
        let runner = FakeRunner::default();
        runner.push(0, STREAM);
        runner.push(0, "#format,stream,frame\n");
        let error = validate_video(&runner, Path::new("torn.mp4")).unwrap_err();
        assert!(
            error.message().contains("no decodable video frames"),
            "{error}"
        );
    }

    #[test]
    fn oversized_dimensions_are_rejected_before_decoding() {
        let runner = FakeRunner::default();
        runner.push(
            0,
            r#"{"streams":[{"codec_type":"video","width":20000,"height":20000}]}"#,
        );
        let error = validate_video(&runner, Path::new("huge.mp4")).unwrap_err();
        assert!(error.message().contains("Unsupported video dimensions"));
        assert_eq!(runner.seen.lock().unwrap().len(), 1, "no decode attempted");
    }

    #[test]
    fn unreadable_file_reports_the_probe_error() {
        let runner = FakeRunner::default();
        runner
            .results
            .lock()
            .unwrap()
            .push(Err(CommandError::Failure {
                program: "ffprobe".into(),
                output: crate::commands::CommandOutput {
                    status: 1,
                    stdout: Vec::new(),
                    stderr: b"Invalid data found when processing input".to_vec(),
                },
            }));
        let error = validate_video(&runner, Path::new("broken.mp4")).unwrap_err();
        assert!(error.message().contains("Invalid data found"));
        assert!(
            error
                .message()
                .starts_with("Could not open recording broken.mp4:")
        );
    }

    #[test]
    fn malformed_probe_json_is_reported_not_panicking() {
        let runner = FakeRunner::default();
        runner.push(0, "not json");
        let error = validate_video(&runner, Path::new("odd.mp4")).unwrap_err();
        assert!(
            error
                .message()
                .contains("Could not open recording odd.mp4:")
        );
    }
}
