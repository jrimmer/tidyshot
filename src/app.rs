//! Command routing shared by the CLI, the URL API and the single instance.
// Ported from omnishot/app.py (Controller.dispatch, capture_actions,
// screenshot_actions, parse_args) and omnishot/backend.py.
//!
//! [`route`] turns parsed arguments into one [`Command`]. It is pure, so the
//! CLI, an `omnishot://` URL and a forwarded socket command all reach the same
//! behavior and can be asserted without a display server.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::cli::Args;
use crate::json::Value;

/// The `area-<action>` shortcut commands.
pub const AREA_SHORTCUTS: &[(&str, &str)] = &[
    ("area-copy", "copy"),
    ("area-save", "save"),
    ("area-annotate", "annotate"),
    ("area-pin", "pin"),
];

/// The OCR shortcut commands and their line-break preference.
pub const OCR_SHORTCUTS: &[(&str, bool)] = &[("ocr-lines", true), ("ocr-single-line", false)];

/// The settings tabs a `settings` URL may open.
pub const SETTINGS_TABS: &[&str] = &[
    "general",
    "wallpaper",
    "screenshots",
    "quickaccess",
    "recording",
    "annotate",
    "text",
    "advanced",
    "shortcuts",
    "about",
];

/// The after-capture actions a screenshot can run.
pub const CAPTURE_ACTIONS: &[&str] = &["copy", "save", "overlay", "annotate", "pin"];

/// The after-recording actions.
pub const RECORDING_ACTIONS: &[&str] = &["copy", "save", "overlay", "edit"];

/// A resolved command the application performs.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Show the capture menu.
    Menu,
    /// Start a capture with its resolved mode and after-capture action.
    Capture {
        command: String,
        geometry: Option<String>,
        action: Option<String>,
        delay: Option<i64>,
        linebreaks: Option<bool>,
        start: Option<bool>,
        autoscroll: Option<bool>,
    },
    /// Open one file in the annotate editor.
    Edit(PathBuf),
    /// Pin one image.
    Pin(PathBuf),
    /// Show a corner preview for one file.
    Overlay(PathBuf),
    /// Recognize text in a file.
    Ocr { path: PathBuf, linebreaks: Option<bool> },
    /// Routed with no argument: open the chooser in the UI.
    EditChooser,
    PinChooser,
    #[allow(dead_code)]
    OverlayChooser,
    /// Show the capture history.
    History,
    /// Restore the most recently closed preview.
    Restore,
    /// Open the newest capture in the editor.
    Last,
    /// Reopen the newest named screenshot in the editor.
    LastScreenshot,
    /// Save every visible preview.
    SaveAll,
    /// Close every visible preview.
    CloseAll,
    /// Show or hide every pin.
    TogglePins,
    /// Close every pin.
    ClosePins,
    /// Unlock every pin.
    UnlockPins,
    /// Open settings on a tab.
    Settings { tab: String },
    /// Open recording setup.
    Record { geometry: Option<String> },
    /// Stop the active recording.
    Stop,
    /// Pause or resume the active recording.
    Pause,
    /// Restart the active recording.
    Restart,
    /// Finish the active scrolling capture.
    ScrollDone,
    /// Quit the application.
    Quit,
    /// A command word the app does not know.
    Unknown(String),
}

/// Why a command could not be routed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteError {
    message: String,
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RouteError {}

/// Route parsed arguments into one command.
// Ported from omnishot/app.py (Controller.dispatch)
pub fn route(args: &Args) -> Result<Command, RouteError> {
    let command = args.command.as_str();

    if let Some((_, linebreaks)) = OCR_SHORTCUTS.iter().find(|(name, _)| *name == command) {
        return Ok(Command::Ocr {
            path: PathBuf::from(args.path.clone().unwrap_or_default()),
            linebreaks: Some(*linebreaks),
        });
    }
    if let Some((_, action)) = AREA_SHORTCUTS.iter().find(|(name, _)| *name == command) {
        return Ok(Command::Capture {
            command: "area".to_owned(),
            geometry: args.geometry.clone(),
            action: Some(action.to_string()),
            delay: args.delay,
            linebreaks: None,
            start: args.url.as_ref().and_then(|url| url.start),
            autoscroll: args.url.as_ref().and_then(|url| url.autoscroll),
        });
    }

    let capture = |name: &str, action: Option<String>| Command::Capture {
        command: name.to_owned(),
        geometry: args.geometry.clone(),
        action,
        delay: args.delay,
        linebreaks: args.url.as_ref().and_then(|url| url.linebreaks),
        start: args.url.as_ref().and_then(|url| url.start),
        autoscroll: args.url.as_ref().and_then(|url| url.autoscroll),
    };

    // A URL `action` parameter behaves like `--action`. An explicit flag wins
    // so a link cannot override the command line.
    let action = args
        .action
        .clone()
        .or_else(|| args.url.as_ref().and_then(|url| url.action.clone()));

    match command {
        "menu" | "all-in-one" => {
            if command == "all-in-one" {
                Ok(capture("select", action.clone()))
            } else {
                Ok(Command::Menu)
            }
        }
        "select" | "area" | "window" | "fullscreen" | "desktop" | "previous" | "scroll"
        | "scroll-horizontal" | "timer" => Ok(capture(command, action.clone())),
        "ocr" => {
            if let Some(path) = &args.path {
                Ok(Command::Ocr {
                    path: PathBuf::from(path),
                    linebreaks: args.url.as_ref().and_then(|url| url.linebreaks),
                })
            } else {
                Ok(capture("ocr", action.clone()))
            }
        }
        "open" => match &args.path {
            Some(path) if !path.is_empty() => Ok(Command::Edit(PathBuf::from(path))),
            _ => Ok(Command::EditChooser),
        },
        "clipboard" => Ok(Command::EditChooser),
        "pin" => match &args.path {
            Some(path) if !path.is_empty() => Ok(Command::Pin(PathBuf::from(path))),
            _ => Ok(Command::PinChooser),
        },
        "overlay" => match &args.path {
            Some(path) if !path.is_empty() => Ok(Command::Overlay(PathBuf::from(path))),
            _ => Err(RouteError {
                message: "An overlay needs an image or video filepath".to_owned(),
            }),
        },
        "history" => Ok(Command::History),
        "restore" => Ok(Command::Restore),
        "last" => Ok(Command::Last),
        "last-screenshot" => Ok(Command::LastScreenshot),
        "save-all" => Ok(Command::SaveAll),
        "close-all" => Ok(Command::CloseAll),
        "toggle-pins" => Ok(Command::TogglePins),
        "close-pins" => Ok(Command::ClosePins),
        "unlock-pins" => Ok(Command::UnlockPins),
        "settings" => {
            let tab = args
                .url
                .as_ref()
                .and_then(|url| url.tab.clone())
                .unwrap_or_else(|| "general".to_owned());
            if tab == "cloud" {
                return Err(RouteError {
                    message: "Cloud services are excluded from OmniShot. Captures stay on your device."
                        .to_owned(),
                });
            }
            Ok(Command::Settings { tab })
        }
        "record" => Ok(Command::Record {
            geometry: args.geometry.clone(),
        }),
        "stop" => Ok(Command::Stop),
        "pause" => Ok(Command::Pause),
        "restart" => Ok(Command::Restart),
        "scroll-done" => Ok(Command::ScrollDone),
        "quit" => Ok(Command::Quit),
        other => Ok(Command::Unknown(other.to_owned())),
    }
}

/// The screenshot actions a capture runs.
///
/// An explicit area shortcut runs its named action plus the configured
/// screenshot actions when the preference allows it. A plain `--action`
/// selects only that action. No action selects the configured set.
// Ported from omnishot/backend.py (screenshot_actions)
#[must_use]
pub fn screenshot_actions(settings: &Value, action: Option<&str>) -> BTreeSet<String> {
    if let Some((_, named)) = AREA_SHORTCUTS.iter().find(|(name, _)| Some(*name) == action) {
        let mut actions = if settings
            .get("capture_shortcut_actions")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            capture_actions(settings, false)
        } else {
            BTreeSet::new()
        };
        actions.insert((*named).to_owned());
        return actions;
    }
    match action {
        Some(action) => BTreeSet::from([action.to_owned()]),
        None => capture_actions(settings, false),
    }
}

/// The configured after-capture (or after-recording) actions.
// Ported from omnishot/backend.py (capture_actions)
#[must_use]
pub fn capture_actions(settings: &Value, recording: bool) -> BTreeSet<String> {
    let key = if recording {
        "after_recording"
    } else {
        "after_capture"
    };
    let allowed: &[&str] = if recording {
        RECORDING_ACTIONS
    } else {
        CAPTURE_ACTIONS
    };
    let mut actions: BTreeSet<String> = BTreeSet::new();
    if let Some(value) = settings.get(key).and_then(Value::as_str)
        && allowed.contains(&value)
        && !value.is_empty()
    {
        actions.insert(value.to_owned());
    }
    if let Some(extra) = settings.get(&format!("{key}_extra")).and_then(Value::as_array) {
        for value in extra {
            if let Some(value) = value.as_str()
                && allowed.contains(&value)
            {
                actions.insert(value.to_owned());
            }
        }
    }
    actions
}

/// Resolve URL coordinates into a CLI geometry string.
///
/// The URL API uses a lower-left origin; `--geometry` uses the compositor's
/// upper-left origin, so this converts once at the boundary.
// Ported from omnishot/app.py (dispatch)
pub fn url_geometry_text(
    args: &Args,
    monitors: &[crate::geometry::MonitorGeometry],
    cursor: crate::api::Point,
) -> Result<Option<String>, RouteError> {
    let Some(url) = &args.url else {
        return Ok(None);
    };
    let Some(coordinates) = &url.coordinates else {
        return Ok(None);
    };
    let rect = crate::api::url_geometry(coordinates, monitors, cursor).map_err(|error| {
        RouteError {
            message: error.to_string(),
        }
    })?;
    Ok(Some(crate::geometry::geometry_text(rect)))
}

/// Validate an explicit after-capture action from a URL.
pub fn valid_action(action: &str) -> bool {
    CAPTURE_ACTIONS.contains(&action)
}

/// True when a settings tab is one the dialog knows.
#[must_use]
pub fn known_settings_tab(tab: &str) -> bool {
    SETTINGS_TABS.contains(&tab)
}

/// The newest capture in a history listing.
///
/// The listing is newest-first, so this is the first row with a path.
// Ported from omnishot/app.py (dispatch: last)
#[must_use]
pub fn newest_capture(history: &[Value]) -> Option<String> {
    history
        .iter()
        .find_map(|row| row.get("path").and_then(Value::as_str).map(str::to_owned))
}

/// The newest named screenshot, skipping imported files and recordings.
///
/// "Annotate last screenshot" reopens the newest capture whose pixels are a
/// screenshot or a scroll stitch, that has a capture name, and that did not
/// come from an external source path.
// Ported from omnishot/app.py (dispatch: last-screenshot)
#[must_use]
pub fn newest_screenshot(history: &[Value]) -> Option<String> {
    history.iter().find_map(|row| {
        let kind = row.get("kind").and_then(Value::as_str).unwrap_or("image");
        let named = row
            .get("capture_named")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let external = row.get("source_path").is_some();
        if (kind == "image" || kind == "scroll") && named && !external {
            row.get("path").and_then(Value::as_str).map(str::to_owned)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::parse_args;

    fn routed(argv: &[&str]) -> Command {
        route(&parse_args(argv).unwrap()).unwrap()
    }

    #[test]
    fn capture_actions_match_the_configured_set() {
        let settings = crate::json::parse(
            r#"{"after_capture":"overlay","after_capture_extra":["copy","save"]}"#,
        )
        .unwrap();
        let actions: Vec<String> = capture_actions(&settings, false).into_iter().collect();
        assert_eq!(actions, vec!["copy", "overlay", "save"]);
        // A recording allows edit but not annotate or pin.
        let settings =
            crate::json::parse(r#"{"after_recording":"edit","after_recording_extra":["pin"]}"#)
                .unwrap();
        let actions: Vec<String> = capture_actions(&settings, true).into_iter().collect();
        assert_eq!(actions, vec!["edit"]);
    }

    // Ported from omnishot/test_capture_actions.py
    // (test_area_shortcuts_respect_configured_actions_without_changing_explicit_actions)
    #[test]
    fn area_shortcuts_respect_configured_actions() {
        let combined = crate::json::parse(
            r#"{"after_capture":"overlay","after_capture_extra":["copy","save"],"capture_shortcut_actions":true}"#,
        )
        .unwrap();
        for action in ["copy", "save", "annotate", "pin"] {
            let named = format!("area-{action}");
            let actions = screenshot_actions(&combined, Some(&named));
            assert!(actions.contains(action));
            assert!(actions.contains("overlay"));
            assert!(actions.contains("copy"));
            assert!(actions.contains("save"));
        }
        // An explicit `--action` selects only that action.
        assert_eq!(
            screenshot_actions(&combined, Some("copy"))
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["copy"]
        );
        // No action selects the configured set.
        assert_eq!(
            screenshot_actions(&combined, None)
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["copy", "overlay", "save"]
        );
        let exclusive = crate::json::parse(
            r#"{"after_capture":"overlay","after_capture_extra":["copy","save"],"capture_shortcut_actions":false}"#,
        )
        .unwrap();
        assert_eq!(
            screenshot_actions(&exclusive, Some("area-pin"))
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["pin"]
        );
    }

    #[test]
    fn capture_commands_route_with_their_action() {
        match routed(&["area", "--action", "annotate"]) {
            Command::Capture { command, action, .. } => {
                assert_eq!(command, "area");
                assert_eq!(action.as_deref(), Some("annotate"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            routed(&["all-in-one"]),
            Command::Capture {
                command: "select".into(),
                geometry: None,
                action: None,
                delay: None,
                linebreaks: None,
                start: None,
                autoscroll: None,
            }
        );
    }

    #[test]
    fn area_and_ocr_shortcuts_route_without_mutating_arguments() {
        assert_eq!(
            routed(&["area-save"]),
            Command::Capture {
                command: "area".into(),
                geometry: None,
                action: Some("save".into()),
                delay: None,
                linebreaks: None,
                start: None,
                autoscroll: None,
            }
        );
        assert_eq!(
            routed(&["ocr-lines"]),
            Command::Ocr {
                path: PathBuf::new(),
                linebreaks: Some(true),
            }
        );
        assert_eq!(
            routed(&["ocr-single-line"]),
            Command::Ocr {
                path: PathBuf::new(),
                linebreaks: Some(false),
            }
        );
    }

    #[test]
    fn unknown_commands_are_reported_not_panicking() {
        assert_eq!(routed(&["frobnicate"]), Command::Unknown("frobnicate".into()));
    }

    #[test]
    fn settings_tabs_are_validated_and_cloud_is_refused() {
        let args = parse_args(["omnishot://open-settings?tab=about"]).unwrap();
        assert_eq!(
            route(&args).unwrap(),
            Command::Settings {
                tab: "about".into()
            }
        );
        let args = parse_args(["omnishot://open-settings?tab=cloud"]).unwrap();
        assert!(route(&args).is_err());
        assert!(known_settings_tab("quickaccess"));
        assert!(!known_settings_tab("cloud"));
    }

    #[test]
    fn overlay_without_a_path_is_an_error() {
        let args = parse_args(["overlay"]).unwrap();
        assert!(route(&args).is_err());
    }

    // Ported from omnishot/test_capture_actions.py
    // (test_last_screenshot_skips_newer_imports_clipboard_images_and_recordings)
    #[test]
    fn last_screenshot_skips_imports_and_recordings() {
        let history = crate::json::parse(
            r#"[
                {"path":"/c/video.mp4","kind":"video","capture_named":true},
                {"path":"/c/clipboard.png","kind":"image"},
                {"path":"/c/imported.png","kind":"image","capture_named":true,"source_path":"/tmp/imported.png"},
                {"path":"/c/screenshot.png","kind":"image","capture_named":true}
            ]"#,
        )
        .unwrap();
        let rows = history.as_array().unwrap();
        assert_eq!(
            newest_screenshot(rows).as_deref(),
            Some("/c/screenshot.png")
        );
        assert_eq!(newest_capture(rows).as_deref(), Some("/c/video.mp4"));
    }

    #[test]
    fn a_scroll_stitch_counts_as_a_screenshot() {
        let history = crate::json::parse(
            r#"[{"path":"/c/scroll.png","kind":"scroll","capture_named":true}]"#,
        )
        .unwrap();
        assert_eq!(
            newest_screenshot(history.as_array().unwrap()).as_deref(),
            Some("/c/scroll.png")
        );
    }
}
