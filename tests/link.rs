// Integration tests link the library so the cxx-qt initialisers resolve.
extern crate tidyshot;

#[test]
fn crate_links_cxx_qt() {
    cxx_qt::init_crate!(tidyshot);
    cxx_qt::init_qml_module!("tidyshot");
}

// The offscreen load assertion. The round-2 hang came from proving the window
// by running its event loop; this loads the packaged Main.qml in a fresh
// engine and inspects QQmlApplicationEngine::rootObjects() instead, so a load
// failure is a plain assertion and no event loop ever starts.
#[test]
fn main_qml_loads_offscreen() {
    cxx_qt::init_crate!(tidyshot);
    cxx_qt::init_qml_module!("tidyshot");

    let mut app = cxx_qt_lib::QGuiApplication::new();
    assert!(
        app.as_mut().is_some(),
        "QGuiApplication could not be created; run with QT_QPA_PLATFORM=offscreen"
    );

    let roots = tidyshot::qml_load::root_count("qrc:/qt/qml/tidyshot/qml/Main.qml");
    assert_eq!(roots, 1, "Main.qml should produce exactly one root object");

    // A missing document yields no root objects, so the count above is the
    // document loading and not a constant.
    let missing = tidyshot::qml_load::root_count("qrc:/qt/qml/tidyshot/qml/Missing.qml");
    assert_eq!(
        missing, 0,
        "a missing document should produce no root objects"
    );
}
