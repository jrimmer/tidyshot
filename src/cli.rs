//! Command-line arguments and local URL dispatch.
// Ported from omnishot/app.py (parse_args)
//!
//! A bare command runs an action; a local file path or an `omnishot://` /
//! `cleanshot://` URL first resolves to the same action dictionary, so the
//! shell plugin, desktop entries, and the CLI all reach one dispatcher.

use std::fmt;
use std::path::Path;

use crate::api::{UrlCommand, parse_url};

/// The default command when none is given.
pub const DEFAULT_COMMAND: &str = "menu";

/// The after-capture actions the CLI accepts.
pub const ACTIONS: &[&str] = &["overlay", "annotate", "copy", "save", "pin"];

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

/// Convert a `file://` URL to a local path, rejecting remote hosts.
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
}
