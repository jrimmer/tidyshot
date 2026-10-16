// Offscreen QML load probe for tests. cxx-qt-lib 0.10 does not wrap
// QQmlApplicationEngine::rootObjects, so a small C++ helper loads the document
// and reports the root-object count; no event loop runs.

#[cxx::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("qml_load.h");
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        fn tidyshot_qml_root_count(url: &QString) -> i32;
    }
}

/// Load `url` in a fresh engine and return its root-object count.
///
/// A non-zero count means the document loaded; zero means it failed to load.
pub fn root_count(url: &str) -> i32 {
    ffi::tidyshot_qml_root_count(&cxx_qt_lib::QString::from(url))
}
