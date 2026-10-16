// The library target every binary and test links.
pub mod bridge;
pub mod commands;
pub mod filenames;
pub mod json;
pub mod recording;
pub mod storage;

#[cfg(test)]
mod crate_tests {
    #[test]
    fn crate_links_cxx_qt() {
        cxx_qt::init_crate!(tidyshot);
        cxx_qt::init_qml_module!("tidyshot");
    }
}
