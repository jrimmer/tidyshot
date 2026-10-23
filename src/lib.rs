// The library target every binary and test links.

/// The application version the About page and the window title report.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod annotation;
pub mod api;
pub mod app;
pub mod backgrounds;
pub mod bridge;
pub mod capture;
pub mod capture_runtime;
pub mod capture_session;
pub mod clean_capture;
pub mod cli;
pub mod clipboard;
pub mod color;
pub mod commands;
pub mod compat;
pub mod compress;
pub mod crop;
pub mod doctor;
pub mod drag;
pub mod editor;
pub mod editor_image;
pub mod editor_ui;
pub mod editor_window;
pub mod effects;
pub mod export;
pub mod filenames;
pub mod geometry;
pub mod gif;
pub mod history;
pub mod history_window;
pub mod hover_keys;
pub mod icon;
pub mod instance;
pub mod json;
pub mod media_import;
pub mod menu;
pub mod mirror;
pub mod motion_blur;
pub mod ocr;
pub mod overlay;
pub mod pin;
pub mod pin_window;
pub mod pngzip;
pub mod preview;
pub mod preview_placement;
pub mod project;
pub mod qml_load;
pub mod raster;
pub mod recording;
pub mod recording_runtime;
pub mod render;
pub mod scroll;
pub mod scroll_runtime;
pub mod selection_ui;
pub mod settings;
pub mod settings_page;
pub mod spotlight;
pub mod stitch;
pub mod storage;
pub mod studio;
pub mod text_result;
pub mod text_styles;
pub mod timeline;
pub mod timeline_thumbnails;
pub mod toolbar;
pub mod ui_scale;
pub mod video_audio_tracks;
pub mod video_camera;
pub mod video_clicks;
pub mod video_cursor;
pub mod video_decoder;
pub mod video_history;
pub mod video_keys;
pub mod video_motion;
pub mod video_position;
pub mod video_preview;
pub mod video_project;
pub mod video_splits;
pub mod video_styles;
pub mod video_ui;
pub mod wallpaper;
pub mod window;
pub mod window_edit;
pub mod window_picker;
pub mod zoom_focus;

/// Initialise the cxx-qt registrations and run the application.
pub fn run() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    run_with(&argv);
}

/// Run the application for one argument vector.
///
/// The first invocation resolves any URL coordinates against the live
/// compositor before the window loads, so a direct coordinate link behaves
/// the same as one forwarded to a running instance. A resolution failure is
/// reported and exits instead of starting an empty GUI.
pub fn run_with(argv: &[String]) {
    cxx_qt::init_crate!(tidyshot);
    cxx_qt::init_qml_module!("tidyshot");
    note_startup_ocr();
    match app::route_and_resolve(argv, &commands::SystemRunner) {
        // An `open` command starts only the editor window; the menu would be
        // an empty window behind it, and closing the editor ends the process.
        Ok(app::Command::Edit(path)) => editor_window::publish_open(path),
        Ok(app::Command::EditChooser) => editor_window::publish_chooser(),
        Ok(command) => capture_runtime::dispatch(command),
        Err(error) => {
            eprintln!("tidyshot: {error}");
            std::process::exit(2);
        }
    }
    let mut app = cxx_qt_lib::QGuiApplication::new();
    if let Some(mut application) = app.as_mut() {
        // The About page and any other version display read this back.
        application
            .as_mut()
            .set_application_version(&cxx_qt_lib::QString::from(VERSION));
    }
    // The icon follows the live theme; refresh the user copy before QML loads
    // so the desktop entry matches the running shell. A failed write is
    // reported but never blocks the GUI.
    refresh_startup_icon();
    // Bind the socket after the application exists so a forwarded `quit`
    // reaches a live event loop.
    start_instance();
    restore_pins();
    recover_recordings();
    cleanup_history();
    let mut engine = cxx_qt_lib::QQmlApplicationEngine::new();
    if let Some(mut engine) = engine.as_mut() {
        // The editor publishes its rendered canvas through this provider.
        editor_image::install(engine.as_mut());
        engine.load(&cxx_qt_lib::QUrl::from("qrc:/qt/qml/tidyshot/qml/Main.qml"));
    }
    if let Some(app) = app.as_mut() {
        std::process::exit(app.exec());
    }
}

/// Bind the single-instance socket and serve forwarded commands.
///
/// The listener is moved into a worker thread that owns it for the process
/// lifetime, so a command sent by a later invocation reaches this instance.
/// A bind failure is surfaced but not fatal: the GUI still starts, and a
/// later command launches a second instance rather than being dropped
/// silently.
fn start_instance() {
    let socket = instance::socket_path(&instance::state_dir());
    let instance = match instance::Instance::bind(&socket) {
        Ok(instance) => instance,
        Err(error) => {
            eprintln!("tidyshot: {error}");
            return;
        }
    };
    std::thread::spawn(move || {
        let _ = instance::serve(&instance, |argv| {
            // Re-parse and route so a forwarded command is understood exactly
            // as the command line understands it, and a malformed payload is
            // reported rather than dropped silently. A coordinate URL is
            // resolved against the live compositor here. Window actions are
            // handled by the layer that owns the windows; the headless ones
            // are performed here.
            match app::route_and_resolve(&argv, &commands::SystemRunner) {
                Ok(app::Command::Quit) => {
                    // Queue the quit onto the Qt thread and stop accepting;
                    // the listener is dropped so its socket file is removed.
                    crate::bridge::qobject::tidyshot_request_quit();
                    return false;
                }
                Ok(command) => crate::capture_runtime::dispatch(command),
                Err(error) => eprintln!("tidyshot: {error}"),
            }
            // Keep accepting: only `quit` ends the loop.
            true
        });
    });
}

/// Restore the pins a previous session left on screen.
///
/// The listing is read before QML loads so a restored pin maps with its saved
/// position, size and opacity on the first commit. A broken listing is
/// reported and the app starts with no pins rather than dying.
fn restore_pins() {
    match pin_window::restore_on_start(&settings::data_dir()) {
        Ok(0) => {}
        Ok(_) => {}
        Err(error) => eprintln!("tidyshot: {error}"),
    }
}

/// Move captures past the retention setting to the trash on startup.
///
/// The cleanup runs on a worker thread so a large history never delays the
/// first window, and a failure is reported instead of stopping the app.
fn cleanup_history() {
    std::thread::spawn(
        || match history_window::cleanup_on_start(&settings::data_dir()) {
            Ok(0) => {}
            Ok(pruned) => eprintln!("tidyshot: moved {pruned} old captures to the trash"),
            Err(error) => eprintln!("tidyshot: could not clean up the capture history: {error}"),
        },
    );
}

/// Recover recordings a crashed instance left behind.
///
/// A take still leased by a live encoder is left alone; a finished take is
/// committed and shown in the preview, and a cancelled one is discarded. This
/// runs on a worker thread so a slow probe never delays the first window.
///
// Ported from omnishot/recording_journal.py (recover_interrupted)
fn recover_recordings() {
    std::thread::spawn(|| {
        let runner = commands::SystemRunner;
        match recording_runtime::recover_on_start(&settings::data_dir(), &runner) {
            Ok(outcome) => {
                for (path, _message) in &outcome.recovered {
                    preview::publish_file(std::path::PathBuf::from(path));
                }
                for path in &outcome.failed {
                    eprintln!("tidyshot: could not recover the recording {path}");
                }
            }
            Err(error) => {
                eprintln!("tidyshot: could not check for an interrupted recording: {error}")
            }
        }
    });
}

/// Rewrite the installed icon when the live Omarchy accent differs.
///
/// The theme is read through the same parser the UI scale uses, so a machine
/// with no theme writes nothing.
fn refresh_startup_icon() {
    let accent = ui_scale::accent_from(&ui_scale::accent_files());
    if let Err(error) = icon::refresh_installed(
        &icon::system_path(),
        &icon::default_path(),
        accent.as_deref(),
    ) {
        eprintln!("tidyshot: could not refresh the app icon: {error}");
    }
}

/// Record a startup OCR request before QML loads.
///
/// A `tidyshot ocr <file>` or `capture-text` URL starts the GUI so the
/// recognized-text dialog can review links; the `Args` singleton reads this
/// request so the dialog runs the completion path once it is created. A bare
/// `ocr` carries no path and the dialog runs the interactive capture instead.
fn note_startup_ocr() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Ok(parsed) = cli::parse_args(args)
        && parsed.command == "ocr"
    {
        bridge::set_startup_ocr(parsed.path, parsed.linebreaks);
    }
}

#[cfg(test)]
mod crate_tests {
    #[test]
    fn crate_links_cxx_qt() {
        cxx_qt::init_crate!(tidyshot);
        cxx_qt::init_qml_module!("tidyshot");
    }

    #[test]
    fn forwarded_commands_route_through_the_shared_parser() {
        let command = super::app::route_argv(&["area-annotate".to_owned()]).unwrap();
        assert!(matches!(
            command,
            super::app::Command::Capture { action: Some(ref action), .. } if action == "annotate"
        ));
        assert!(matches!(
            super::app::route_argv(&["frobnicate".to_owned()]),
            Err(ref message) if message.contains("unknown command")
        ));
        assert!(super::app::route_argv(&["https://example.com".to_owned()]).is_err());
    }

    #[test]
    fn the_command_socket_is_isolated_per_user_and_data_dir() {
        use super::instance;
        use std::path::Path;
        let dir = Path::new("/run/user/1000/tidyshot");
        let socket = instance::socket_path(dir);
        assert!(socket.starts_with(dir));
        assert_eq!(socket.extension().and_then(|e| e.to_str()), Some("sock"));
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        assert!(socket.to_string_lossy().contains(&format!("-{uid}.sock")));
    }
}
