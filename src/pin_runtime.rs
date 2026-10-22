//! Pinned captures as always-on-top windows.
// Ported from omnishot/widgets.py (Pin) and omnishot/pin_controls.py
//!
//! The window model, wheel gestures and control placement live in
//! [`crate::pin`]; this module owns the set of visible pins and the QML
//! bridge. One menu command shows or hides the whole set,
//! another closes it and a third unlocks every pin, matching the source.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::pin::PinState;

/// One pin shown in the UI.
#[derive(Debug, Clone, PartialEq)]
pub struct PinEntry {
    pub path: PathBuf,
    pub state: PinState,
    pub visible: bool,
}

impl PinEntry {
    /// Build a pin for an image at its starting size.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, width: usize, height: usize) -> Self {
        Self {
            path: path.into(),
            state: PinState::new(width, height),
            visible: true,
        }
    }

    /// The JSON row the QML window instantiates from.
    #[must_use]
    pub fn to_value(&self) -> crate::json::Value {
        let mut object = BTreeMap::new();
        object.insert(
            "path".to_owned(),
            crate::json::Value::String(self.path.display().to_string()),
        );
        object.insert(
            "name".to_owned(),
            crate::json::Value::String(
                self.path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("capture")
                    .to_owned(),
            ),
        );
        object.insert(
            "width".to_owned(),
            crate::json::Value::Number(self.state.width),
        );
        object.insert(
            "height".to_owned(),
            crate::json::Value::Number(self.state.height),
        );
        object.insert(
            "opacity".to_owned(),
            crate::json::Value::Number(self.state.opacity),
        );
        object.insert(
            "locked".to_owned(),
            crate::json::Value::Bool(self.state.locked),
        );
        object.insert("visible".to_owned(), crate::json::Value::Bool(self.visible));
        crate::json::Value::Object(object)
    }
}

/// Why a pin could not be created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinError {
    /// The file is no longer readable.
    Missing(String),
    /// The file is not a decodable PNG.
    Image(String),
}

impl std::fmt::Display for PinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(message) | Self::Image(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for PinError {}

/// The pins a window renders, newest last.
#[derive(Debug, Default, Clone)]
pub struct PinSet {
    entries: Vec<PinEntry>,
    hidden: bool,
}

impl PinSet {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The visible (or hidden) pins.
    #[must_use]
    pub fn entries(&self) -> &[PinEntry] {
        &self.entries
    }

    /// Whether the whole set is hidden.
    #[must_use]
    pub const fn hidden(&self) -> bool {
        self.hidden
    }

    /// Pin one image file, replacing an existing pin of the same path.
    pub fn pin(&mut self, path: &Path) -> Result<&PinEntry, PinError> {
        if !path.is_file() {
            return Err(PinError::Missing(format!(
                "the capture is no longer available: {}",
                path.display()
            )));
        }
        let bytes = std::fs::read(path)
            .map_err(|error| PinError::Missing(format!("Could not open the capture: {error}")))?;
        let image = crate::pngzip::decode_png(&bytes)
            .map_err(|error| PinError::Image(format!("Could not read the capture: {error}")))?;
        self.entries.retain(|entry| entry.path != path);
        self.entries
            .push(PinEntry::new(path, image.width, image.height));
        self.entries
            .last()
            .ok_or_else(|| PinError::Image("the capture produced no pin".to_owned()))
    }

    /// Show or hide every pin.
    pub fn toggle(&mut self) {
        self.hidden = !self.hidden;
        for entry in &mut self.entries {
            entry.visible = !self.hidden;
        }
    }

    /// Close every pin.
    pub fn close_all(&mut self) {
        self.entries.clear();
        self.hidden = false;
    }

    /// Unlock every pin so its wheel gestures work again.
    pub fn unlock_all(&mut self) {
        for entry in &mut self.entries {
            entry.state.locked = false;
        }
    }

    /// The number of pins.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The JSON rows QML renders.
    #[must_use]
    pub fn rows(&self) -> Vec<crate::json::Value> {
        self.entries.iter().map(PinEntry::to_value).collect()
    }
}

/// The pins shared by the capture runtime and the QML pin windows.
static PINS: std::sync::Mutex<PinSet> = std::sync::Mutex::new(PinSet {
    entries: Vec::new(),
    hidden: false,
});

fn with_pins<T>(f: impl FnOnce(&mut PinSet) -> T) -> T {
    let mut pins = PINS.lock().expect("pin set poisoned");
    f(&mut pins)
}

/// The current pin rows as JSON text, one per row.
#[must_use]
pub fn rows_json() -> Vec<String> {
    with_pins(|pins| {
        pins.rows()
            .into_iter()
            .map(|row| row.to_string_compact())
            .collect()
    })
}

/// Pin a file, reporting why it could not be pinned.
pub fn pin_file(path: &Path) -> Result<String, PinError> {
    with_pins(|pins| pins.pin(path).map(|_| ()))?;
    publish();
    Ok(format!("Pinned {}", path.display()))
}

/// Show or hide every pin.
pub fn toggle() -> String {
    let hidden = with_pins(|pins| {
        pins.toggle();
        pins.hidden()
    });
    publish();
    if hidden {
        "Pins hidden".to_owned()
    } else {
        "Pins shown".to_owned()
    }
}

/// Close every pin.
pub fn close_all() -> String {
    with_pins(PinSet::close_all);
    publish();
    "Pins closed".to_owned()
}

/// Unlock every pin.
pub fn unlock_all() -> String {
    with_pins(PinSet::unlock_all);
    publish();
    "Pins unlocked".to_owned()
}

/// Push the current pins to the bound QML windows.
///
/// A publish before QML binds is not lost: `bind` applies the current set.
fn publish() {
    let rows = rows_json();
    if let Some(thread) = BOUND.get() {
        let _ = thread.queue(move |mut pins| {
            pins.as_mut().apply_rows(rows);
        });
    }
}

static BOUND: std::sync::OnceLock<cxx_qt::CxxQtThread<qobject::PinApi>> =
    std::sync::OnceLock::new();

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    extern "RustQt" {
        /// The pinned captures the QML windows render.
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QStringList, rows)]
        #[qproperty(i32, count)]
        type PinApi = super::PinRust;

        /// Register this object and replay the current pins.
        #[qinvokable]
        fn bind(self: Pin<&mut Self>);

        /// Close every pin.
        #[qinvokable]
        #[cxx_name = "closeAll"]
        fn close_all(self: Pin<&mut Self>);

        /// Unlock every pin.
        #[qinvokable]
        #[cxx_name = "unlockAll"]
        fn unlock_all(self: Pin<&mut Self>);

        /// Show or hide every pin.
        #[qinvokable]
        fn toggle(self: Pin<&mut Self>);
    }

    impl cxx_qt::Threading for PinApi {}
}

use core::pin::Pin;
use cxx_qt::Threading as _;

/// Rust state behind the QML `PinApi` singleton.
pub struct PinRust {
    rows: cxx_qt_lib::QStringList,
    count: i32,
}

impl Default for PinRust {
    fn default() -> Self {
        let rows = rows_json();
        let mut list = cxx_qt_lib::QStringList::default();
        for row in &rows {
            list.append(cxx_qt_lib::QString::from(row.clone()));
        }
        Self {
            count: rows.len() as i32,
            rows: list,
        }
    }
}

impl qobject::PinApi {
    /// Register this object and replay any pins that arrived before QML.
    pub fn bind(mut self: Pin<&mut Self>) {
        let _ = BOUND.set(self.qt_thread());
        let rows = rows_json();
        self.as_mut().apply_rows(rows);
    }

    fn apply_rows(mut self: Pin<&mut Self>, rows: Vec<String>) {
        let mut list = cxx_qt_lib::QStringList::default();
        for row in &rows {
            list.append(cxx_qt_lib::QString::from(row.clone()));
        }
        self.as_mut().set_count(rows.len() as i32);
        self.as_mut().set_rows(list);
    }

    /// Close every pin.
    pub fn close_all(self: Pin<&mut Self>) {
        close_all();
    }

    /// Unlock every pin.
    pub fn unlock_all(self: Pin<&mut Self>) {
        unlock_all();
    }

    /// Show or hide every pin.
    pub fn toggle(self: Pin<&mut Self>) {
        toggle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tidyshot-pin-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn png(width: usize, height: usize) -> Vec<u8> {
        crate::pngzip::Image::new(width, height, vec![9u8; width * height * 4]).to_png()
    }

    #[test]
    fn pinning_an_image_keeps_its_starting_size() {
        let dir = scratch("pin");
        let path = dir.join("shot.png");
        std::fs::write(&path, png(640, 320)).unwrap();
        let mut pins = PinSet::new();
        let entry = pins.pin(&path).unwrap();
        assert_eq!(entry.state.image_width, 640);
        assert_eq!(entry.state.width, 520.0);
        assert_eq!(pins.len(), 1);
        // Pinning the same path replaces the old entry instead of stacking.
        pins.pin(&path).unwrap();
        assert_eq!(pins.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn toggle_close_and_unlock_change_every_pin() {
        let dir = scratch("set");
        let path = dir.join("shot.png");
        std::fs::write(&path, png(100, 80)).unwrap();
        let mut pins = PinSet::new();
        pins.pin(&path).unwrap();
        pins.entries[0].state.locked = true;
        pins.toggle();
        assert!(pins.hidden());
        assert!(!pins.entries()[0].visible);
        pins.toggle();
        assert!(!pins.hidden());
        pins.unlock_all();
        assert!(!pins.entries()[0].state.locked);
        pins.close_all();
        assert!(pins.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_or_unreadable_file_is_reported() {
        let dir = scratch("bad");
        let missing = dir.join("absent.png");
        let mut pins = PinSet::new();
        assert!(pins.pin(&missing).is_err());
        let bad = dir.join("bad.png");
        std::fs::write(&bad, b"not a png").unwrap();
        assert!(pins.pin(&bad).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
