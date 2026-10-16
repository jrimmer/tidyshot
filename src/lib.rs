// The library target every binary and test links.
pub mod annotation;
pub mod api;
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
pub mod editor;
pub mod effects;
pub mod filenames;
pub mod geometry;
pub mod gif;
pub mod json;
pub mod media_import;
pub mod menu;
pub mod mirror;
pub mod motion_blur;
pub mod ocr;
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

/// Initialise the cxx-qt registrations and run the application.
pub fn run() {
    cxx_qt::init_crate!(tidyshot);
    cxx_qt::init_qml_module!("tidyshot");
    note_startup_ocr();
    let mut app = cxx_qt_lib::QGuiApplication::new();
    let mut engine = cxx_qt_lib::QQmlApplicationEngine::new();
    if let Some(engine) = engine.as_mut() {
        engine.load(&cxx_qt_lib::QUrl::from("qrc:/qt/qml/tidyshot/qml/Main.qml"));
    }
    if let Some(app) = app.as_mut() {
        std::process::exit(app.exec());
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
}
