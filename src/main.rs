// tidyshot pilot: open the window.

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QUrl};

fn main() {
    // Pull in the library's cxx-qt initialisers; without a reference the
    // linker drops them.
    cxx_qt::init_crate!(tidyshot);
    cxx_qt::init_qml_module!("tidyshot");

    let mut app = QGuiApplication::new();
    let mut engine = QQmlApplicationEngine::new();
    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/tidyshot/qml/Main.qml"));
    }
    if let Some(app) = app.as_mut() {
        std::process::exit(app.exec());
    }
}
