// The library target every binary and test links.
pub mod api;
pub mod bridge;
pub mod capture;
pub mod cli;
pub mod color;
pub mod commands;
pub mod effects;
pub mod filenames;
pub mod geometry;
pub mod gif;
pub mod json;
pub mod media_import;
pub mod motion_blur;
pub mod recording;
pub mod storage;

/// Initialise the cxx-qt registrations and run the application.
pub fn run() {
    cxx_qt::init_crate!(tidyshot);
    cxx_qt::init_qml_module!("tidyshot");
    let mut app = cxx_qt_lib::QGuiApplication::new();
    let mut engine = cxx_qt_lib::QQmlApplicationEngine::new();
    if let Some(engine) = engine.as_mut() {
        engine.load(&cxx_qt_lib::QUrl::from("qrc:/qt/qml/tidyshot/qml/Main.qml"));
    }
    if let Some(app) = app.as_mut() {
        std::process::exit(app.exec());
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
