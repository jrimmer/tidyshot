//! Recording audio post-processing: mono conversion without re-encoding video.
// Ported from omnishot/recording_audio.py
//!
//! A mono take copies the video stream and re-encodes only audio, writing
//! through the atomic storage helper so a failed pass leaves the original
//! recording untouched.

use std::io;
use std::path::Path;

use crate::commands::{CommandError, CommandRunner, CommandSpec};
use crate::json::Value;
use crate::storage;

/// The `ffprobe` audio-channel query, capped like the source's 15 seconds.
fn probe_spec(path: &Path) -> CommandSpec {
    CommandSpec::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream=channels",
            "-of",
            "json",
        ])
        .arg(path.to_string_lossy().into_owned())
        .timeout(std::time::Duration::from_secs(15))
}

/// True when any audio stream has more than one channel.
fn has_multichannel_audio<R: CommandRunner>(runner: &R, path: &Path) -> Result<bool, CommandError> {
    let output = runner.run(&probe_spec(path))?;
    let value = crate::json::parse(&output.stdout_string()).map_err(|_| CommandError::Failure {
        program: "ffprobe".to_owned(),
        output: output.clone(),
    })?;
    Ok(value
        .get("streams")
        .and_then(Value::as_array)
        .is_some_and(|streams| {
            streams.iter().any(|stream| {
                stream
                    .get("channels")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
                    > 1.0
            })
        }))
}

/// Re-encode a recording's audio to mono, copying the video stream.
///
/// A recording that has no multichannel audio is left exactly as it is, so a
/// caller never rewrites a file it does not need to. The encoded result is
/// committed atomically over the original.
pub fn make_mono<R: CommandRunner>(runner: &R, path: &Path) -> Result<(), MakeMonoError> {
    if !has_multichannel_audio(runner, path)? {
        return Ok(());
    }
    let output = storage::AtomicFile::new(path).map_err(MakeMonoError::Io)?;
    let spec = CommandSpec::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(path.to_string_lossy().into_owned())
        .args([
            "-map",
            "0:v:0",
            "-map",
            "0:a?",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-ac",
            "1",
            "-movflags",
            "+faststart",
        ])
        .arg(output.path().to_string_lossy().into_owned())
        .timeout(std::time::Duration::from_secs(86_400));
    runner.run(&spec)?;
    output.commit().map_err(MakeMonoError::Io)
}

/// Why a mono pass could not complete.
#[derive(Debug)]
pub enum MakeMonoError {
    /// An external command failed.
    Command(CommandError),
    /// The atomic output could not be created or committed.
    Io(io::Error),
}

impl std::fmt::Display for MakeMonoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MakeMonoError::Command(error) => write!(f, "{error}"),
            MakeMonoError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for MakeMonoError {}

impl From<CommandError> for MakeMonoError {
    fn from(error: CommandError) -> Self {
        MakeMonoError::Command(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandOutput, CommandRunner};
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRunner {
        seen: Mutex<Vec<CommandSpec>>,
        channels: u32,
        fail_ffmpeg: bool,
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            self.seen.lock().unwrap().push(spec.clone());
            if spec.program == "ffprobe" {
                return Ok(CommandOutput {
                    status: 0,
                    stdout: format!(r#"{{"streams":[{{"channels":{}}}]}}"#, self.channels)
                        .into_bytes(),
                    stderr: Vec::new(),
                });
            }
            if self.fail_ffmpeg {
                return Err(CommandError::Failure {
                    program: spec.program.clone(),
                    output: CommandOutput {
                        status: 1,
                        stdout: Vec::new(),
                        stderr: b"invalid data".to_vec(),
                    },
                });
            }
            // Emulate the encoder writing the committed file beside the
            // destination. The real AtomicFile path is a sibling temp file.
            let destination = std::path::PathBuf::from(spec.args.last().unwrap());
            std::fs::write(&destination, b"mono").unwrap();
            Ok(CommandOutput {
                status: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tidyshot-mono-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn stereo_is_downmixed_with_the_video_copied() {
        let dir = scratch("stereo");
        let path = dir.join("take.mp4");
        std::fs::write(&path, b"original").unwrap();
        let runner = FakeRunner {
            channels: 2,
            ..FakeRunner::default()
        };
        make_mono(&runner, &path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"mono");
        let seen = runner.seen.lock().unwrap();
        let ffmpeg = seen
            .iter()
            .find(|spec| spec.program == "ffmpeg")
            .expect("ffmpeg ran");
        assert!(ffmpeg.args.iter().any(|arg| arg == "-c:v"));
        assert!(ffmpeg.args.iter().any(|arg| arg == "copy"));
        assert!(ffmpeg.args.windows(2).any(|pair| pair == ["-ac", "1"]));
    }

    #[test]
    fn a_mono_recording_is_left_untouched() {
        let dir = scratch("mono");
        let path = dir.join("take.mp4");
        std::fs::write(&path, b"original").unwrap();
        let runner = FakeRunner {
            channels: 1,
            ..FakeRunner::default()
        };
        make_mono(&runner, &path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        let seen = runner.seen.lock().unwrap();
        assert!(seen.iter().all(|spec| spec.program != "ffmpeg"));
    }

    #[test]
    fn a_failed_pass_preserves_the_recording() {
        let dir = scratch("failed");
        let path = dir.join("take.mp4");
        std::fs::write(&path, b"original").unwrap();
        let runner = FakeRunner {
            channels: 2,
            fail_ffmpeg: true,
            ..FakeRunner::default()
        };
        let error = make_mono(&runner, &path).unwrap_err();
        assert!(error.to_string().contains("invalid data"));
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        // The temp file beside the recording is cleaned up.
        let siblings: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".tidyshot-")
            })
            .collect();
        assert!(siblings.is_empty());
    }
}
