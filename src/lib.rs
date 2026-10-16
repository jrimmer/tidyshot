// The library target every binary and test links.
pub mod annotation;
pub mod api;
pub mod app;
pub mod backgrounds;
pub mod bridge;
pub mod capture;
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
pub mod effects;
pub mod filenames;
pub mod geometry;
pub mod gif;
pub mod hover_keys;
pub mod instance;
pub mod json;
pub mod media_import;
pub mod menu;
pub mod mirror;
pub mod motion_blur;
pub mod ocr;
pub mod overlay;
pub mod pin;
pub mod pngzip;
pub mod project;
pub mod qml_load;
pub mod raster;
pub mod recording;
pub mod render;
pub mod scroll;
pub mod settings;
pub mod spotlight;
pub mod stitch;
pub mod storage;
pub mod text_result;
pub mod text_styles;
pub mod toolbar;
pub mod ui_scale;

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
    if let Err(error) = app::route_and_resolve(argv, &commands::SystemRunner) {
        eprintln!("tidyshot: {error}");
        std::process::exit(2);
    }
    let mut app = cxx_qt_lib::QGuiApplication::new();
    // Bind the socket after the application exists so a forwarded `quit`
    // reaches a live event loop.
    start_instance();
    let mut engine = cxx_qt_lib::QQmlApplicationEngine::new();
    if let Some(engine) = engine.as_mut() {
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
                Ok(_) => {}
                Err(error) => eprintln!("tidyshot: {error}"),
            }
            // Keep accepting: only `quit` ends the loop.
            true
        });
    });
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
