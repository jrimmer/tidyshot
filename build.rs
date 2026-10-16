fn main() {
    cxx_qt_build::CxxQtBuilder::new_qml_module(cxx_qt_build::QmlModule::new("tidyshot").qml_files(
        [
            "qml/Main.qml",
            "qml/Panel.qml",
            "qml/KeyboardPanel.qml",
            "qml/MenuButton.qml",
            "qml/MenuPanel.qml",
        ],
    ))
    .qt_module("Quick")
    .qt_module("QuickControls2")
    .qt_module("QuickTest")
    .files(["src/bridge.rs", "src/menu.rs"])
    .build();
}
