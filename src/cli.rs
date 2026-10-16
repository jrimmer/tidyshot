//! Command-line arguments and local URL dispatch.
// Ported from omnishot/app.py (parse_args)
//!
//! A bare command runs an action; a local file path or an `omnishot://` /
//! `cleanshot://` URL first resolves to the same action dictionary, so the
//! shell plugin, desktop entries, and the CLI all reach one dispatcher.

use std::fmt;
use std::path::Path;

use crate::api::{UrlCommand, parse_url};
use crate::capture_session::{
    CaptureMode, CaptureOptions, CaptureOutcome, CaptureRequest, CaptureRequestError,
    CaptureSession, run_capture,
};
use crate::commands::CommandRunner;
use crate::json::Value;

/// The default command when none is given.
pub const DEFAULT_COMMAND: &str = "menu";

/// The after-capture actions the CLI accepts.
pub const ACTIONS: &[&str] = &["overlay", "annotate", "copy", "save", "pin"];

/// The command words that start a capture.
pub const CAPTURE_COMMANDS: &[&str] = &[
    "select",
    "all-in-one",
    "area",
    "fullscreen",
    "desktop",
    "previous",
    "window",
    "timer",
    "scroll",
    "scroll-horizontal",
    "ocr",
];

/// Parsed command-line arguments.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Args {
    pub command: String,
    pub path: Option<String>,
    pub geometry: Option<String>,
    pub action: Option<String>,
    pub delay: Option<i64>,
    /// Present when the command came from a local URL.
    pub url: Option<UrlCommand>,
}

/// Why the arguments could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgsError {
    message: String,
}

impl fmt::Display for ArgsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ArgsError {}

/// Parse an argument vector, as `argv` after the program name.
// Ported from omnishot/app.py (parse_args)
pub fn parse_args<I, S>(argv: I) -> Result<Args, ArgsError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut positionals: Vec<String> = Vec::new();
    let mut geometry = None;
    let mut action = None;
    let mut delay = None;

    let mut items = argv.into_iter().map(|item| item.as_ref().to_owned());
    while let Some(item) = items.next() {
        match item.as_str() {
            "--geometry" => {
                geometry = items.next();
                if geometry.is_none() {
                    return Err(ArgsError {
                        message: "--geometry needs a value".to_owned(),
                    });
                }
            }
            "--action" => {
                let value = items.next().ok_or_else(|| ArgsError {
                    message: "--action needs a value".to_owned(),
                })?;
                if !ACTIONS.contains(&value.as_str()) {
                    return Err(ArgsError {
                        message: format!("invalid choice: {value}"),
                    });
                }
                action = Some(value);
            }
            "--delay" => {
                let value = items.next().ok_or_else(|| ArgsError {
                    message: "--delay needs a value".to_owned(),
                })?;
                delay = Some(value.parse().map_err(|_| ArgsError {
                    message: format!("invalid int value: {value}"),
                })?);
            }
            other => positionals.push(other.to_owned()),
        }
    }

    let mut command = positionals
        .first()
        .cloned()
        .unwrap_or_else(|| DEFAULT_COMMAND.to_owned());
    let mut path = positionals.get(1).cloned();
    let mut url = None;

    if command.starts_with("file://") {
        let local = local_file_path(&command).ok_or_else(|| ArgsError {
            message: "Open a local file".to_owned(),
        })?;
        path = Some(local);
        command = "open".to_owned();
    } else if command.contains("://") {
        let parsed = parse_url(&command).map_err(|error| ArgsError {
            message: error.to_string(),
        })?;
        if parsed.path.is_some() {
            path = parsed.path.clone();
        }
        command = parsed.command.clone();
        url = Some(parsed);
    } else if Path::new(&command).is_file() {
        path = Some(std::mem::take(&mut command));
        command = "open".to_owned();
    }

    Ok(Args {
        command,
        path,
        geometry,
        action,
        delay,
        url,
    })
}

impl Args {
    /// Whether this command starts a capture.
    #[must_use]
    pub fn is_capture(&self) -> bool {
        CAPTURE_COMMANDS.contains(&self.command.as_str())
    }

    /// Resolve this command into a capture request, when it is one.
    ///
    /// `default_delay` is the configured `delay` setting, used by the self
    /// timer when the CLI supplies no `--delay`.
    #[must_use]
    pub fn capture_request(
        &self,
        default_delay: i64,
    ) -> Option<Result<CaptureRequest, CaptureRequestError>> {
        let mode = CaptureMode::from_command(&self.command)?;
        Some(CaptureRequest::resolve(
            mode.command(),
            self.geometry.as_deref(),
            self.delay,
            default_delay,
        ))
    }

    /// Resolve and run this command as a capture, waiting out its delay.
    ///
    /// This is the CLI layer's entry to a capture: the resolved request keeps
    /// its countdown, so `timer --delay 5` waits before the pixels are taken.
    /// `None` means the command is not a capture.
    pub fn run_capture<R: CommandRunner>(
        &self,
        session: &CaptureSession<'_, R>,
        monitors: &[Value],
        default_delay: i64,
        options: CaptureOptions,
        on_tick: impl FnMut(&str),
        should_cancel: impl FnMut() -> bool,
    ) -> Option<Result<CaptureOutcome, CaptureRequestError>> {
        let request = self.capture_request(default_delay)?;
        Some(request.and_then(|request| {
            run_capture(session, monitors, &request, options, on_tick, should_cancel)
        }))
    }
}

/// Convert `file://` URL to a local path, rejecting remote hosts.
fn local_file_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, ""),
    };
    if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
        return None;
    }
    Some(percent_decode(path))
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_the_menu() {
        assert_eq!(parse_args(Vec::<String>::new()).unwrap().command, "menu");
    }

    #[test]
    fn desktop_file_uri_keeps_spaces() {
        let dir = std::env::temp_dir().join(format!("tidyshot-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("my image.png");
        std::fs::write(&file, b"image").unwrap();
        let uri = format!("file://{}", file.display());
        let args = parse_args([uri.as_str()]).unwrap();
        assert_eq!(args.command, "open");
        assert_eq!(args.path.as_deref(), Some(file.to_string_lossy().as_ref()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remote_file_uri_is_rejected() {
        assert!(parse_args(["file://example.com/tmp/x.png"]).is_err());
    }

    #[test]
    fn local_url_updates_the_command_and_path() {
        let args = parse_args(["cleanshot://open-annotate?filepath=%2Ftmp%2Fa.png"]).unwrap();
        assert_eq!(args.command, "open");
        assert_eq!(args.path.as_deref(), Some("/tmp/a.png"));
        assert!(args.url.is_some());
    }

    #[test]
    fn flags_and_positionals_are_kept() {
        let args = parse_args([
            "record",
            "--geometry",
            "10,20 640x400",
            "--action",
            "save",
            "--delay",
            "3",
        ])
        .unwrap();
        assert_eq!(args.command, "record");
        assert_eq!(args.geometry.as_deref(), Some("10,20 640x400"));
        assert_eq!(args.action.as_deref(), Some("save"));
        assert_eq!(args.delay, Some(3));
    }

    #[test]
    fn an_unsupported_action_is_rejected() {
        assert!(parse_args(["area", "--action", "upload"]).is_err());
        assert!(parse_args(["area", "--delay", "soon"]).is_err());
    }

    #[test]
    fn capture_commands_resolve_into_requests() {
        let args = parse_args(["timer", "--delay", "5"]).unwrap();
        assert!(args.is_capture());
        let request = args.capture_request(3).unwrap().unwrap();
        assert_eq!(request.mode, CaptureMode::Timer);
        assert_eq!(request.delay, 5);

        // The configured delay is used when the CLI omits one.
        let args = parse_args(["timer"]).unwrap();
        assert_eq!(args.capture_request(7).unwrap().unwrap().delay, 7);

        let args = parse_args(["menu"]).unwrap();
        assert!(!args.is_capture());
        assert!(args.capture_request(3).is_none());
    }

    #[test]
    fn capture_request_surfaces_geometry_errors() {
        let args = parse_args(["area", "--geometry", "nope"]).unwrap();
        assert!(args.capture_request(0).unwrap().is_err());
    }

    #[test]
    fn run_capture_consumes_the_resolved_delay() {
        use crate::capture_session::CaptureSession;
        use crate::commands::{CommandError, CommandOutput, CommandRunner, CommandSpec};
        use crate::mirror::MirrorRegistry;
        use std::sync::Mutex;

        #[derive(Default)]
        struct Fake {
            seen: Mutex<Vec<CommandSpec>>,
        }
        impl CommandRunner for Fake {
            fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
                self.seen.lock().unwrap().push(spec.clone());
                if spec.program == "hyprctl" {
                    // Model aarch64's unhookable plugin so the mirror degrades.
                    let stdout = match spec.args.as_slice() {
                        [flag, query] if flag == "-j" && query == "plugin list" => b"[]".to_vec(),
                        [action, sub, ..] if action == "plugin" && sub == "load" => {
                            b"Could not hook IElementRenderer::preDrawSurface".to_vec()
                        }
                        _ => b"false".to_vec(),
                    };
                    return Ok(CommandOutput {
                        status: 0,
                        stdout,
                        stderr: Vec::new(),
                    });
                }
                Ok(CommandOutput {
                    status: 0,
                    stdout: b"PNG".to_vec(),
                    stderr: Vec::new(),
                })
            }
        }
        let runner = Fake::default();
        let registry = MirrorRegistry::with_arch("aarch64");
        // A scratch native dir with the plugin files grab expects; on aarch64
        // the mirror still degrades, but the cursor mirror is best effort.
        let native =
            std::env::temp_dir().join(format!("tidyshot-cli-capture-{}", std::process::id()));
        std::fs::create_dir_all(&native).unwrap();
        for name in [crate::mirror::MIRROR_PLUGIN, crate::mirror::CAPTURE_PLUGIN] {
            std::fs::write(native.join(name), b"plugin").unwrap();
        }
        let session = CaptureSession {
            runner: &runner,
            mirror: &registry,
            native: native.clone(),
        };
        let monitors = crate::json::parse(
            r#"[{"name":"left","x":0,"y":0,"width":1920,"height":1080,"scale":1.0,"transform":0,"focused":true}]"#,
        )
        .unwrap();

        // A timer with a delay ticks the countdown before the pixels are taken.
        let args = parse_args(["timer", "--delay", "2", "--geometry", "0,0 100x100"]).unwrap();
        let mut ticks = 0;
        let outcome = args
            .run_capture(
                &session,
                monitors.as_array().unwrap(),
                3,
                CaptureOptions {
                    cursor: false,
                    freeze: true,
                },
                |_| ticks += 1,
                || false,
            )
            .unwrap()
            .unwrap();
        assert!(matches!(outcome, CaptureOutcome::Grabbed(_)));
        assert!(ticks >= 1, "the resolved delay must be waited out");

        let args = parse_args(["menu"]).unwrap();
        assert!(
            args.run_capture(
                &session,
                monitors.as_array().unwrap(),
                3,
                CaptureOptions {
                    cursor: false,
                    freeze: true,
                },
                |_| {},
                || false,
            )
            .is_none()
        );
        let _ = std::fs::remove_dir_all(&native);
    }
}
