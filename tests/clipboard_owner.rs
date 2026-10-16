//! The reviewed OCR dialog's clipboard path, end to end.
//!
//! `wl-copy` forks a long-lived owner that inherits the caller's stdout and
//! stderr. Piping those would make the copy block until the owner exits; these
//! tests drive a `wl-copy` stand-in that forks a pipe-holding owner, so the
//! regression that froze the dialog is caught without a compositor.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tidyshot::commands::{CommandOutput, CommandRunner, CommandSpec, SystemRunner};
use tidyshot::text_result::{Presentation, TextSettings, present};

/// Write a `wl-copy` stand-in that records its stdin and forks an owner that
/// keeps the caller's stdout and stderr open, exactly like wl-clipboard.
#[cfg(unix)]
fn fork_copy_stub(dir: &Path, capture: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let stub = dir.join("wl-copy");
    let body = format!(
        "#!/bin/sh\ncat > '{capture}'\n( exec sleep 5 ) <&0 >&1 2>&2 &\nexit 0\n",
        capture = capture.display()
    );
    std::fs::write(&stub, body).unwrap();
    let mut permissions = std::fs::metadata(&stub).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&stub, permissions).unwrap();
    stub
}

/// The clipboard write must return as soon as the parent exits, even though a
/// forked owner still holds the inherited pipes open.
#[cfg(unix)]
#[test]
fn clipboard_write_does_not_wait_for_the_forked_owner() {
    let dir = std::env::temp_dir().join(format!("tidyshot-copy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let capture = dir.join("clipboard.txt");
    let stub = fork_copy_stub(&dir, &capture);

    let mut spec =
        tidyshot::commands::clipboard_spec("text/plain;charset=utf-8", b"recognized".to_vec());
    spec.program = stub.to_string_lossy().into_owned();
    let started = Instant::now();
    SystemRunner.run(&spec).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the clipboard write must not wait for the owner"
    );
    assert_eq!(std::fs::read(&capture).unwrap(), b"recognized");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A recorder that returns success and keeps the specs it saw.
#[derive(Default)]
struct RecordingRunner {
    seen: std::sync::Mutex<Vec<CommandSpec>>,
}

impl CommandRunner for RecordingRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, tidyshot::commands::CommandError> {
        self.seen.lock().unwrap().push(spec.clone());
        Ok(CommandOutput {
            status: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }
}

/// Plain recognized text with link detection off copies and reports no dialog,
/// and the copy spec keeps stderr off a pipe.
#[test]
fn present_copies_through_a_non_blocking_spec() {
    let runner = RecordingRunner::default();
    let settings = TextSettings {
        ocr_detect_links: false,
        ..TextSettings::default()
    };
    assert_eq!(
        present(&runner, "plain recognized text", &settings, false),
        Presentation::Copied
    );
    let seen = runner.seen.lock().unwrap();
    let copy = seen.iter().find(|spec| spec.program == "wl-copy").unwrap();
    assert!(copy.stderr_to_file);
    assert!(
        copy.timeout <= Some(Duration::from_secs(10)),
        "the copy keeps a short timeout: {:?}",
        copy.timeout
    );
    assert_eq!(
        copy.stdin.as_deref(),
        Some(b"plain recognized text".as_slice())
    );
}
