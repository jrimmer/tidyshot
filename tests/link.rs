// Integration tests link the library so the cxx-qt initialisers resolve.
extern crate tidyshot;

#[test]
fn crate_links_cxx_qt() {
    cxx_qt::init_crate!(tidyshot);
    cxx_qt::init_qml_module!("tidyshot");
}

// The offscreen load assertion. Proving the window by running its event loop
// hangs when there is no compositor; this loads the packaged Main.qml in a
// fresh engine and inspects QQmlApplicationEngine::rootObjects() instead, so a
// load failure is a plain assertion and no event loop ever starts.
#[test]
fn main_qml_loads_offscreen() {
    // The harness exports the offscreen platform, but set it here too so a
    // bare `cargo test` needs no display. Nothing else reads the variable
    // before QGuiApplication is created, so the write is safe.
    if std::env::var_os("QT_QPA_PLATFORM").is_none() {
        unsafe { std::env::set_var("QT_QPA_PLATFORM", "offscreen") };
    }

    cxx_qt::init_crate!(tidyshot);
    cxx_qt::init_qml_module!("tidyshot");

    let mut app = cxx_qt_lib::QGuiApplication::new();
    assert!(
        app.as_mut().is_some(),
        "QGuiApplication could not be created; run with QT_QPA_PLATFORM=offscreen"
    );

    let roots = tidyshot::qml_load::root_count("qrc:/qt/qml/tidyshot/qml/Main.qml");
    assert_eq!(roots, 1, "Main.qml should produce exactly one root object");

    // The editor window loads too, so a toolbar or singleton failure is a
    // load assertion instead of a silent missing window.
    let editor = tidyshot::qml_load::root_count("qrc:/qt/qml/tidyshot/qml/Editor.qml");
    assert_eq!(editor, 1, "Editor.qml should produce one root window");

    // A missing document yields no root objects, so the count above is the
    // document loading and not a constant.
    let missing = tidyshot::qml_load::root_count("qrc:/qt/qml/tidyshot/qml/Missing.qml");
    assert_eq!(
        missing, 0,
        "a missing document should produce no root objects"
    );
}
