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
        /// URL coordinates still to be resolved into `geometry` using the
        /// live monitors at dispatch time.
        coordinates: Option<crate::api::Coordinates>,
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
    Ocr {
        path: PathBuf,
        linebreaks: Option<bool>,
    },
    /// Routed with no argument: open the chooser in the UI.
    EditChooser,
    PinChooser,
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
    Settings {
        tab: String,
    },
    /// Open recording setup.
    Record {
        geometry: Option<String>,
        /// URL coordinates still to be resolved into `geometry`.
        coordinates: Option<crate::api::Coordinates>,
    },
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
            coordinates: args.url.as_ref().and_then(|url| url.coordinates),
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
        coordinates: args.url.as_ref().and_then(|url| url.coordinates),
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
                    message:
                        "Cloud services are excluded from OmniShot. Captures stay on your device."
                            .to_owned(),
                });
            }
            Ok(Command::Settings { tab })
        }
        "record" => Ok(Command::Record {
            geometry: args.geometry.clone(),
            coordinates: args.url.as_ref().and_then(|url| url.coordinates),
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
    if let Some((_, named)) = AREA_SHORTCUTS
        .iter()
        .find(|(name, _)| Some(*name) == action)
    {
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
    if let Some(extra) = settings
        .get(&format!("{key}_extra"))
        .and_then(Value::as_array)
    {
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

/// Validate an explicit after-capture action from a URL.
pub fn valid_action(action: &str) -> bool {
    CAPTURE_ACTIONS.contains(&action)
}

/// Fill a command's `geometry` from its URL coordinates.
///
/// A capture or recording link that names `x`/`y`/`width`/`height`/
/// `display` is resolved against the live monitors and cursor here, so a
/// forwarded coordinate URL keeps its region instead of silently degrading
/// to an interactive capture. Commands without coordinates are returned
/// unchanged.
pub fn resolve_geometry(
    command: Command,
    monitors: &[crate::geometry::MonitorGeometry],
    cursor: crate::api::Point,
) -> Result<Command, RouteError> {
    match command {
        Command::Capture {
            command,
            geometry,
            coordinates,
            action,
            delay,
            linebreaks,
            start,
            autoscroll,
        } => {
            let geometry = geometry.or(resolve_coordinates(coordinates, monitors, cursor)?);
            Ok(Command::Capture {
                command,
                geometry,
                coordinates,
                action,
                delay,
                linebreaks,
                start,
                autoscroll,
            })
        }
        Command::Record {
            geometry,
            coordinates,
        } => {
            let geometry = geometry.or(resolve_coordinates(coordinates, monitors, cursor)?);
            Ok(Command::Record {
                geometry,
                coordinates,
            })
        }
        command => Ok(command),
    }
}

fn resolve_coordinates(
    coordinates: Option<crate::api::Coordinates>,
    monitors: &[crate::geometry::MonitorGeometry],
    cursor: crate::api::Point,
) -> Result<Option<String>, RouteError> {
    let Some(coordinates) = coordinates else {
        return Ok(None);
    };
    let rect =
        crate::api::url_geometry(&coordinates, monitors, cursor).map_err(|error| RouteError {
            message: error.to_string(),
        })?;
    Ok(Some(crate::geometry::geometry_text(rect)))
}

/// Read the connected monitors and pointer position for coordinate links.
///
/// The URL API uses a lower-left origin and a display index, so the live
/// Hyprland data is needed before the rectangle can be computed. A missing
/// or malformed reply surfaces an error rather than guessing.
// Ported from omnishot/backend.py (capture_monitors, hypr)
pub fn live_monitors<R: crate::commands::CommandRunner>(
    runner: &R,
) -> Result<(Vec<crate::geometry::MonitorGeometry>, crate::api::Point), RouteError> {
    let monitors = crate::commands::hypr(runner, "monitors").map_err(|error| RouteError {
        message: error.to_string(),
    })?;
    let rows = monitors.as_array().unwrap_or(&[]);
    let geometries: Vec<crate::geometry::MonitorGeometry> = rows
        .iter()
        .map(|monitor| crate::geometry::MonitorGeometry {
            x: number(monitor, "x"),
            y: number(monitor, "y"),
            width: number(monitor, "width"),
            height: number(monitor, "height"),
            scale: monitor.get("scale").and_then(Value::as_f64).unwrap_or(1.0),
            transform: number(monitor, "transform"),
        })
        .collect();
    let cursor = crate::commands::hypr(runner, "cursorpos")
        .ok()
        .map(|value| crate::api::Point {
            x: number(&value, "x"),
            y: number(&value, "y"),
        })
        .unwrap_or(crate::api::Point { x: 0, y: 0 });
    Ok((geometries, cursor))
}

fn number(value: &Value, key: &str) -> i32 {
    value
        .get(key)
        .and_then(Value::as_f64)
        .map_or(0, |number| number as i32)
}

/// Resolve a command's URL coordinates against the live compositor.
pub fn resolve_command<R: crate::commands::CommandRunner>(
    command: Command,
    runner: &R,
) -> Result<Command, RouteError> {
    let needs_coordinates = matches!(
        &command,
        Command::Capture {
            coordinates: Some(_),
            ..
        } | Command::Record {
            coordinates: Some(_),
            ..
        }
    );
    if !needs_coordinates {
        return Ok(command);
    }
    let (monitors, cursor) = live_monitors(runner)?;
    resolve_geometry(command, &monitors, cursor)
}

/// True when a settings tab is one the dialog knows.
#[must_use]
pub fn known_settings_tab(tab: &str) -> bool {
    SETTINGS_TABS.contains(&tab)
}

/// Route an argument vector through parsing into one command.
///
/// This is what the running instance uses for a command forwarded over the
/// socket: the sender and the receiver share one parser and one router, so
/// they can never disagree about what a command means.
pub fn route_argv(argv: &[String]) -> Result<Command, String> {
    let args = crate::cli::parse_args(argv.iter().map(String::as_str))
        .map_err(|error| error.to_string())?;
    match route(&args).map_err(|error| error.to_string())? {
        Command::Unknown(command) => Err(format!("unknown command: {command}")),
        command => Ok(command),
    }
}

/// Parse, route and resolve a forwarded or direct command for dispatch.
///
/// This is the runtime entry the single-instance server and the initial
/// invocation share. A URL that carries `x`/`y`/`width`/`height`/`display`
/// has its region resolved here, against the live compositor, so the
/// dispatch step app.py performs is not skipped.
// Ported from omnishot/app.py (Controller.dispatch)
pub fn route_and_resolve<R: crate::commands::CommandRunner>(
    argv: &[String],
    runner: &R,
) -> Result<Command, String> {
    let command = route_argv(argv)?;
    resolve_command(command, runner).map_err(|error| error.to_string())
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
            Command::Capture {
                command, action, ..
            } => {
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
                coordinates: None,
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
                coordinates: None,
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
    fn url_coordinates_convert_into_a_geometry_string() {
        use crate::api::Point;
        use crate::geometry::MonitorGeometry;
        let command =
            routed(&["omnishot://capture-area?x=100&y=120&width=200&height=150&display=1"]);
        let monitors = [MonitorGeometry {
            x: -1920,
            y: 0,
            width: 2880,
            height: 1800,
            scale: 1.5,
            transform: 0,
        }];
        let resolved = resolve_geometry(command, &monitors, Point { x: 0, y: 0 }).unwrap();
        match resolved {
            Command::Capture { geometry, .. } => {
                assert_eq!(geometry.as_deref(), Some("-1820,930 200x150"));
            }
            other => panic!("{other:?}"),
        }
        // A command without coordinates is untouched.
        let plain = routed(&["area"]);
        assert_eq!(
            resolve_geometry(plain, &monitors, Point { x: 0, y: 0 }).unwrap(),
            routed(&["area"])
        );
    }

    #[test]
    fn a_forwarded_coordinate_url_keeps_its_region() {
        // The live compositor supplies the monitors and pointer position; the
        // forwarded command must come out with a concrete geometry, not an
        // interactive capture.
        use crate::commands::{CommandError, CommandOutput, CommandRunner, CommandSpec};
        use std::sync::Mutex;

        struct Fake {
            seen: Mutex<Vec<CommandSpec>>,
        }
        impl CommandRunner for Fake {
            fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
                self.seen.lock().unwrap().push(spec.clone());
                let stdout = match spec.args.as_slice() {
                    [flag, query] if flag == "-j" && query == "monitors" => {
                        br#"[{"name":"left","x":0,"y":0,"width":1920,"height":1080,"scale":1.0,"transform":0,"focused":true}]"#.to_vec()
                    }
                    [flag, query] if flag == "-j" && query == "cursorpos" => {
                        br#"{"x":100,"y":200}"#.to_vec()
                    }
                    _ => b"false".to_vec(),
                };
                Ok(CommandOutput {
                    status: 0,
                    stdout,
                    stderr: Vec::new(),
                })
            }
        }
        let runner = Fake {
            seen: Mutex::new(Vec::new()),
        };
        let argv = vec!["omnishot://record-screen?x=10&y=20&width=300&height=200".to_owned()];
        match route_and_resolve(&argv, &runner).unwrap() {
            Command::Record { geometry, .. } => {
                assert_eq!(geometry.as_deref(), Some("10,860 300x200"));
            }
            other => panic!("{other:?}"),
        }
        // Both compositor queries were made.
        let seen = runner.seen.lock().unwrap();
        assert!(seen.iter().any(|spec| spec.args == ["-j", "monitors"]));
        assert!(seen.iter().any(|spec| spec.args == ["-j", "cursorpos"]));
    }

    #[test]
    fn a_coordinate_url_without_a_display_is_an_error() {
        use crate::commands::{CommandError, CommandOutput, CommandRunner, CommandSpec};
        struct Empty;
        impl CommandRunner for Empty {
            fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
                let stdout = if spec.args == ["-j", "monitors"] {
                    b"[]".to_vec()
                } else {
                    b"{}".to_vec()
                };
                Ok(CommandOutput {
                    status: 0,
                    stdout,
                    stderr: Vec::new(),
                })
            }
        }
        let argv = vec!["omnishot://capture-area?x=1&y=2&width=30&height=40".to_owned()];
        assert!(route_and_resolve(&argv, &Empty).is_err());
    }

    #[test]
    fn unknown_commands_are_reported_not_panicking() {
        assert_eq!(
            routed(&["frobnicate"]),
            Command::Unknown("frobnicate".into())
        );
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
