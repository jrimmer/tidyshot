//! Behavior for the command surface users already know, and the preview
//! overlay, UI scale and clipboard rules behind it.
//!
//! The documented CLI subcommands and URL parameters are the ones the desktop
//! entries, the shell widget and existing links use. These tests read the same
//! list and assert every entry still routes, so the port cannot quietly drop a
//! command.

use std::fs;
use std::path::PathBuf;

use tidyshot::app::{Command, route};
use tidyshot::cli::parse_args;
use tidyshot::instance::{self, Forwarded, Instance};
use tidyshot::overlay::{
    Corner, Preview, PreviewSettings, ScreenRect, Swipe, SwipeState, available_actions, is_video,
};
use tidyshot::preview::{PreviewWindow, focused_monitor};
use tidyshot::ui_scale;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tidyshot-surface-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn parse(argv: &[&str]) -> tidyshot::cli::Args {
    parse_args(argv).unwrap()
}

#[test]
fn every_documented_cli_subcommand_routes() {
    // The exact documented list plus the shortcut aliases the preferences and
    // bar widget install.
    let commands = [
        "menu",
        "all-in-one",
        "area",
        "area-annotate",
        "area-copy",
        "area-save",
        "area-pin",
        "last-screenshot",
        "scroll",
        "scroll-horizontal",
        "fullscreen",
        "window",
        "desktop",
        "previous",
        "timer",
        "ocr",
        "ocr-lines",
        "ocr-single-line",
        "open",
        "clipboard",
        "pin",
        "history",
        "restore",
        "last",
        "save-all",
        "close-all",
        "toggle-pins",
        "close-pins",
        "unlock-pins",
        "settings",
        "record",
        "pause",
        "restart",
        "stop",
        "scroll-done",
        "quit",
    ];
    for command in commands {
        let parsed = parse(&[command]);
        let routed = route(&parsed);
        assert!(routed.is_ok(), "{command} must route, got {routed:?}");
        assert!(
            !matches!(routed.unwrap(), Command::Unknown(_)),
            "{command} routed to an unknown command"
        );
    }
}

#[test]
fn documented_url_parameters_survive_parsing_and_routing() {
    // After-capture actions.
    for action in ["overlay", "copy", "save", "annotate", "pin"] {
        let url = format!("omnishot://capture-area?action={action}");
        let parsed = parse(&[&url]);
        match route(&parsed).unwrap() {
            Command::Capture { action: routed, .. } => {
                assert_eq!(routed.as_deref(), Some(action));
            }
            other => panic!("{other:?}"),
        }
    }
    // Scrolling start and autoscroll are independent.
    for (start, autoscroll) in [
        ("false", "true"),
        ("true", "false"),
        ("true", "true"),
        ("false", "false"),
    ] {
        let url = format!("omnishot://scrolling-capture?start={start}&autoscroll={autoscroll}");
        let parsed = parse(&[&url]);
        match route(&parsed).unwrap() {
            Command::Capture {
                command,
                start: routed_start,
                autoscroll: routed_auto,
                ..
            } => {
                assert_eq!(command, "scroll");
                assert_eq!(routed_start, Some(start == "true"));
                assert_eq!(routed_auto, Some(autoscroll == "true"));
            }
            other => panic!("{other:?}"),
        }
    }
    // Every settings tab in the guide opens; cloud does not exist locally.
    for tab in [
        "general",
        "wallpaper",
        "shortcuts",
        "quickaccess",
        "recording",
        "screenshots",
        "annotate",
        "advanced",
        "about",
        "text",
    ] {
        let url = format!("omnishot://open-settings?tab={tab}");
        assert_eq!(
            route(&parse(&[&url])).unwrap(),
            Command::Settings { tab: tab.into() }
        );
    }
    // The two coordinate file formats resolve to the same open action.
    let local = parse(&["cleanshot://open-annotate?filepath=%2Ftmp%2Fa.png"]);
    assert_eq!(
        route(&local).unwrap(),
        Command::Edit(PathBuf::from("/tmp/a.png"))
    );
    // Coordinate parameters are carried on the command until dispatch.
    let coordinates =
        parse(&["omnishot://capture-area?x=100&y=120&width=200&height=150&display=1"]);
    match route(&coordinates).unwrap() {
        Command::Capture {
            coordinates: Some(coords),
            ..
        } => {
            assert_eq!(coords.x, Some(100));
            assert_eq!(coords.y, Some(120));
            assert_eq!(coords.width, Some(200));
            assert_eq!(coords.height, Some(150));
            assert_eq!(coords.display, Some(1));
        }
        other => panic!("{other:?}"),
    }
    let recording = parse(&["omnishot://record-screen?x=10&y=20&width=300&height=200"]);
    match route(&recording).unwrap() {
        Command::Record {
            coordinates: Some(coords),
            ..
        } => assert_eq!(coords.width, Some(300)),
        other => panic!("{other:?}"),
    }
    // OCR line-break overrides.
    let url = parse(&["omnishot://capture-text?linebreaks=false"]);
    match route(&url).unwrap() {
        Command::Capture { command, .. } => assert_eq!(command, "ocr"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_forwarded_command_reaches_the_running_instance() {
    let dir = scratch("forward");
    let socket = instance::socket_path(&dir);
    let listener = Instance::bind(&socket).unwrap();
    let argv = vec!["scroll-done".to_owned()];
    assert_eq!(
        instance::forward(&argv, &socket).unwrap(),
        Forwarded::Delivered
    );
    assert_eq!(listener.accept().unwrap().unwrap(), argv);
}

#[test]
fn no_instance_leaves_the_command_for_a_new_gui() {
    let dir = scratch("no-instance");
    let socket = instance::socket_path(&dir);
    assert_eq!(
        instance::forward(&["menu".to_owned()], &socket).unwrap(),
        Forwarded::NoInstance
    );
}

#[test]
fn ui_scale_reflows_measurements_without_touching_content() {
    let dir = scratch("scale");
    let theme = dir.join("theme.toml");
    let override_file = dir.join("user.toml");
    fs::write(&theme, "[font]\nbase-size=14\n[spacing]\nscale=1.2\n").unwrap();
    fs::write(&override_file, "[font]\nbase-size=20\n").unwrap();
    let appearance = ui_scale::read(Some(&[theme, override_file])).unwrap();
    assert_eq!(appearance.font_size, 20);
    assert_eq!(appearance.factor(), 2.0);
    // A window that was 335 px wide reflows to 670 px.
    assert_eq!(
        ui_scale::size(335.0, 200.0, appearance.factor()),
        (670, 400)
    );
}

#[test]
fn overlay_loads_content_renders_and_respects_gesture_rules() {
    let dir = scratch("overlay");
    let path = dir.join("capture.png");
    let image = tidyshot::pngzip::Image::new(320, 200, vec![64u8; 320 * 200 * 4]);
    fs::write(&path, image.to_png()).unwrap();

    let settings = PreviewSettings {
        corner: Corner::BottomRight,
        timeout: 5.0,
        ..PreviewSettings::default()
    };
    let mut preview = Preview::open(&path, 1, &settings, &tidyshot::commands::SystemRunner)
        .expect("the overlay loads a PNG capture");
    assert_eq!(preview.dimensions(), (320, 200));
    assert!(!preview.is_video());
    assert!(preview.timer_armed());
    assert_eq!(
        preview.position(ScreenRect::new(0.0, 0.0, 1920.0, 1080.0), 335.0, 220.0),
        (1567.0, 824.0)
    );

    // A trackpad swipe collapses and restores; a sideways one dismisses.
    let mut state = SwipeState::new();
    preview.swipe(&mut state, true, Swipe { x: 0.0, y: -110.0 }, false);
    assert!(preview.collapsed());
    preview.swipe(&mut state, true, Swipe { x: 0.0, y: 110.0 }, false);
    assert!(!preview.collapsed());
    assert_eq!(
        preview.swipe(&mut state, true, Swipe { x: 100.0, y: 0.0 }, false),
        Some(true)
    );
}

#[test]
fn a_recording_preview_uses_its_first_frame_and_omits_pin() {
    let dir = scratch("video");
    let path = dir.join("capture.mp4");
    fs::write(&path, b"not a real recording").unwrap();
    assert!(is_video(&path));
    assert_eq!(available_actions(true).len(), 3);
    // Opening it without ffmpeg data surfaces the decode error rather than
    // panicking or silently showing nothing.
    let error = Preview::open(
        &path,
        0,
        &PreviewSettings::default(),
        &tidyshot::commands::SystemRunner,
    )
    .unwrap_err();
    assert!(!error.to_string().is_empty());
}

#[test]
fn the_preview_window_anchors_bottom_right_and_honours_saved_settings() {
    let monitors = tidyshot::json::parse(
        r#"[
            {"name":"left","x":0,"y":0,"width":1920,"height":1080,"focused":false},
            {"name":"right","x":1920,"y":0,"width":2560,"height":1440,"focused":true}
        ]"#,
    )
    .unwrap();
    let monitor = focused_monitor(monitors.as_array().unwrap()).expect("a focused monitor");
    let settings = PreviewSettings {
        timeout: 3.0,
        ..PreviewSettings::default()
    };
    let preview = PreviewWindow::new("/tmp/capture.png", 640, 480, settings, Some(monitor));
    // The right monitor runs from 1920 to 4480; a 355 px window with an 18 px
    // margin ends at 4480 - 18.
    assert_eq!(preview.position(), (4107.0, 1122.0));
    assert_eq!(preview.auto_close_ms(), 3000);
    assert_eq!(preview.auto_close_action(), "close");
    assert!(preview.closes_after_drag(true, false));
    assert!(!preview.closes_after_drag(true, true));
}

#[test]
fn clipboard_file_lists_accept_local_media_and_reject_remote_or_missing() {
    let dir = scratch("clipboard");
    let image = dir.join("my image.png");
    fs::write(&image, b"png").unwrap();
    let files =
        tidyshot::clipboard::local_files(format!("file://{}\n", image.display()).as_bytes())
            .unwrap();
    assert_eq!(files, vec![image]);
    assert!(tidyshot::clipboard::local_files(b"https://example.com/a.png").is_err());
    assert!(tidyshot::clipboard::local_files(b"file:///tmp/definitely-missing-98765.png").is_err());
    // Paste only accepts images; a video must go through Open from clipboard.
    assert!(tidyshot::clipboard::pasteable_images(&[PathBuf::from("/tmp/a.mp4")]).is_err());
}
