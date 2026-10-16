// Integration tests link the library so the cxx-qt initialisers resolve.
extern crate tidyshot;

#[test]
fn crate_links_cxx_qt() {
    cxx_qt::init_crate!(tidyshot);
    cxx_qt::init_qml_module!("tidyshot");
}
