//! Pinned captures as always-on-top windows.
// Ported from omnishot/widgets.py (Pin) and omnishot/pin_controls.py
//!
//! The window model, wheel gestures and control placement live in
//! [`crate::pin`]; this module owns the set of pinned windows, the JSON rows
//! the QML windows instantiate from, and the persistence of every pin's
//! position, size, opacity and click-through state. One menu command shows or
//! hides the whole set, another closes it and a third unlocks every pin.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::json::Value;
use crate::pin::{MAX_OPACITY, MIN_OPACITY, MIN_SIDE, PinState};

/// The pin listing inside the data directory.
pub const PINS_FILE: &str = "pins.json";

/// The title prefix of an unlocked pin.
pub const PIN_TITLE_PREFIX: &str = "TidyShot Pin — ";

/// The title prefix of a locked pin.
pub const LOCKED_PIN_TITLE_PREFIX: &str = "TidyShot Locked Pin — ";

/// The title a pin window carries for its file name.
#[must_use]
pub fn pin_title(name: &str, locked: bool) -> String {
    if locked {
        format!("{LOCKED_PIN_TITLE_PREFIX}{name}")
    } else {
        format!("{PIN_TITLE_PREFIX}{name}")
    }
}

/// True when a compositor title belongs to one of TidyShot's pin windows.
///
/// The title, not the window class, is what identifies a pin: a pin is always
/// on top, so offering it in the window picker or capturing it would copy
/// TidyShot itself.
#[must_use]
pub fn is_pin_title(title: &str) -> bool {
    title.starts_with(PIN_TITLE_PREFIX) || title.starts_with(LOCKED_PIN_TITLE_PREFIX)
}

/// The pin listing a data root holds.
#[must_use]
pub fn pins_path(root: &Path) -> PathBuf {
    root.join(PINS_FILE)
}

/// Why a pin could not be stored or restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinError {
    Missing(String),
    Image(String),
    Io(String),
}

impl std::fmt::Display for PinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(message) | Self::Image(message) | Self::Io(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for PinError {}

impl From<std::io::Error> for PinError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(format!("Could not save the pins: {error}"))
    }
}

/// One pinned capture.
#[derive(Debug, Clone, PartialEq)]
pub struct PinnedWindow {
    pub path: PathBuf,
    pub name: String,
    /// The top-left corner in logical pixels, as the compositor placed it.
    pub x: f64,
    pub y: f64,
    pub state: PinState,
    /// Swallow pointer input so clicks reach the window underneath.
    pub click_through: bool,
    /// Whether the whole pin set is currently showing.
    pub visible: bool,
}

impl PinnedWindow {
    /// Build a pin at its starting size and a given position.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, width: usize, height: usize, x: f64, y: f64) -> Self {
        let path = path.into();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("capture")
            .to_owned();
        Self {
            path,
            name,
            x,
            y,
            state: PinState::new(width, height),
            click_through: false,
            visible: true,
        }
    }

    /// The compositor title for this pin.
    #[must_use]
    pub fn title(&self) -> String {
        pin_title(&self.name, self.state.locked)
    }

    /// The QML row that draws this pin.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut object = BTreeMap::new();
        object.insert(
            "path".to_owned(),
            Value::String(self.path.display().to_string()),
        );
        object.insert(
            "url".to_owned(),
            Value::String(crate::overlay::local_file_url(&self.path)),
        );
        object.insert("name".to_owned(), Value::String(self.name.clone()));
        object.insert("title".to_owned(), Value::String(self.title()));
        object.insert("x".to_owned(), Value::Number(self.x));
        object.insert("y".to_owned(), Value::Number(self.y));
        object.insert("width".to_owned(), Value::Number(self.state.width));
        object.insert("height".to_owned(), Value::Number(self.state.height));
        object.insert("opacity".to_owned(), Value::Number(self.state.opacity));
        object.insert("locked".to_owned(), Value::Bool(self.state.locked));
        object.insert("clickThrough".to_owned(), Value::Bool(self.click_through));
        object.insert("visible".to_owned(), Value::Bool(self.visible));
        Value::Object(object)
    }

    /// Parse one persisted row, dropping anything without a path.
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let path = PathBuf::from(value.get("path").and_then(Value::as_str)?);
        if path.as_os_str().is_empty() {
            return None;
        }
        let number = |key: &str| value.get(key).and_then(Value::as_f64);
        let width = number("width").unwrap_or(0.0);
        let height = number("height").unwrap_or(0.0);
        let state = PinState {
            image_width: number("imageWidth").unwrap_or(0.0).max(0.0) as usize,
            image_height: number("imageHeight").unwrap_or(0.0).max(0.0) as usize,
            width,
            height,
            opacity: number("opacity").unwrap_or(1.0),
            locked: value
                .get("locked")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            hovered: false,
            dragging: false,
            rounded: true,
            border: false,
            shadow: true,
        };
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "capture".to_owned());
        Some(Self {
            path,
            name,
            x: number("x").unwrap_or(0.0),
            y: number("y").unwrap_or(0.0),
            state,
            click_through: value
                .get("clickThrough")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            visible: value
                .get("visible")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        })
    }
}

/// The pins the app has open, newest last.
#[derive(Debug, Default, Clone)]
pub struct PinRegistry {
    entries: Vec<PinnedWindow>,
    hidden: bool,
}

impl PinRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every pin, including a hidden set.
    #[must_use]
    pub fn entries(&self) -> &[PinnedWindow] {
        &self.entries
    }

    /// Whether the whole set is hidden.
    #[must_use]
    pub const fn hidden(&self) -> bool {
        self.hidden
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Pin one image at its starting size, replacing an existing pin of the
    /// same path instead of stacking a second copy.
    pub fn add(&mut self, path: impl Into<PathBuf>, width: usize, height: usize, x: f64, y: f64) {
        let pin = PinnedWindow::new(path, width, height, x, y);
        let path = pin.path.clone();
        self.entries.retain(|entry| entry.path != path);
        self.entries.push(pin);
        self.hidden = false;
        for entry in &mut self.entries {
            entry.visible = true;
        }
    }

    /// Close one pin. Returns true when it was open.
    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.path != path);
        before != self.entries.len()
    }

    /// Close every pin.
    pub fn close_all(&mut self) {
        self.entries.clear();
        self.hidden = false;
    }

    /// Show or hide every pin, reporting whether the set is now hidden.
    pub fn toggle(&mut self) -> bool {
        self.hidden = !self.hidden;
        for entry in &mut self.entries {
            entry.visible = !self.hidden;
        }
        self.hidden
    }

    /// Unlock every pin and turn click-through off, so its controls work
    /// again.
    pub fn unlock_all(&mut self) {
        for entry in &mut self.entries {
            entry.state.locked = false;
            entry.click_through = false;
        }
    }

    /// Set one pin's opacity, clamped to the range a wheel gesture allows.
    pub fn set_opacity(&mut self, path: &Path, value: f64) -> bool {
        match self.find_mut(path) {
            Some(entry) => entry.state.set_opacity(value),
            None => false,
        }
    }

    /// Lock or unlock one pin.
    pub fn set_locked(&mut self, path: &Path, locked: bool) -> bool {
        match self.find_mut(path) {
            Some(entry) => {
                entry.state.locked = locked;
                if locked {
                    // A locked pin keeps its visible controls to unlock it,
                    // but never its resize or move gestures.
                    entry.state.dragging = false;
                }
                true
            }
            None => false,
        }
    }

    /// Turn click-through on or off for one pin.
    ///
    /// Click-through implies a locked pin: pointer input never reaches the
    /// window, so its own controls could not turn the state back off.
    pub fn set_click_through(&mut self, path: &Path, value: bool) -> bool {
        match self.find_mut(path) {
            Some(entry) => {
                entry.click_through = value;
                if value {
                    entry.state.locked = true;
                    entry.state.dragging = false;
                }
                true
            }
            None => false,
        }
    }

    /// Store a pin's new position and size.
    ///
    /// A user resize is free-form, so the persisted size is not forced back
    /// to the image's aspect ratio.
    pub fn set_geometry(&mut self, path: &Path, x: f64, y: f64, width: f64, height: f64) -> bool {
        let Some(entry) = self.find_mut(path) else {
            return false;
        };
        let width = width.max(MIN_SIDE);
        let height = height.max(MIN_SIDE);
        let changed = (entry.x - x).abs() > f64::EPSILON
            || (entry.y - y).abs() > f64::EPSILON
            || (entry.state.width - width).abs() > f64::EPSILON
            || (entry.state.height - height).abs() > f64::EPSILON;
        entry.x = x;
        entry.y = y;
        entry.state.width = width;
        entry.state.height = height;
        changed
    }

    fn find_mut(&mut self, path: &Path) -> Option<&mut PinnedWindow> {
        self.entries.iter_mut().find(|entry| entry.path == path)
    }

    /// The QML rows, newest last.
    #[must_use]
    pub fn rows(&self) -> Vec<Value> {
        self.entries.iter().map(PinnedWindow::to_value).collect()
    }

    /// The QML rows as compact JSON text.
    #[must_use]
    pub fn rows_json(&self) -> Vec<String> {
        self.rows()
            .into_iter()
            .map(|row| row.to_string_compact())
            .collect()
    }

    /// Write the registry atomically into a data root.
    pub fn save_at(&self, root: &Path) -> Result<(), PinError> {
        let rows = self.rows();
        let text = Value::Array(rows).to_string_pretty();
        std::fs::create_dir_all(root)?;
        crate::storage::atomic_write_file(&pins_path(root), text.as_bytes())?;
        Ok(())
    }

    /// Read the pins a data root holds.
    ///
    /// A missing listing is an empty set. A malformed listing is an error so
    /// the UI can say the pins could not be restored instead of silently
    /// dropping them, and a pin whose file vanished is skipped.
    pub fn restore_at(root: &Path) -> Result<Self, PinError> {
        let path = pins_path(root);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(PinError::Io(format!(
                    "Could not read the pins from {}: {error}",
                    path.display()
                )));
            }
        };
        let value = crate::json::parse(&text).map_err(|error| {
            PinError::Io(format!(
                "Could not read the pins from {}: {error}",
                path.display()
            ))
        })?;
        let mut entries: Vec<PinnedWindow> = value
            .as_array()
            .unwrap_or(&[])
            .iter()
            .filter_map(PinnedWindow::from_value)
            .filter(|entry| entry.path.is_file())
            .collect();
        // A hidden set is a per-session state, not something a restart should
        // bring back as invisible pins with no way to find them.
        let hidden = false;
        for entry in &mut entries {
            entry.visible = true;
            entry.state.opacity = entry.state.opacity.clamp(MIN_OPACITY, MAX_OPACITY);
        }
        Ok(Self { entries, hidden })
    }
}

/// The pins shared by the capture runtime and the QML pin windows.
static PINS: std::sync::Mutex<Option<PinRegistry>> = std::sync::Mutex::new(None);

/// The Qt pin object, once QML has created it.
static BOUND: std::sync::OnceLock<cxx_qt::CxxQtThread<qobject::PinApi>> =
    std::sync::OnceLock::new();

/// The data root the live registry persists to, filled on restore or first
/// use.
static ROOT: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

fn with_pins<T>(f: impl FnOnce(&mut PinRegistry) -> T) -> T {
    let mut guard = PINS.lock().expect("pin registry poisoned");
    if guard.is_none() {
        *guard = Some(PinRegistry::new());
    }
    f(guard.as_mut().expect("pin registry initialised"))
}

fn set_root(path: PathBuf) {
    *ROOT.lock().expect("pin root poisoned") = Some(path);
}

/// Restore the pins saved in `root` and make that root the live one.
///
/// This runs before QML loads, so the restored pins map with their saved
/// position, size and opacity on the first commit. A missing listing is an
/// empty set; an unreadable one is reported and the app starts with no pins
/// rather than dying.
pub fn restore_on_start(root: &Path) -> Result<usize, PinError> {
    let registry = PinRegistry::restore_at(root)?;
    let count = registry.len();
    set_root(root.to_path_buf());
    {
        let mut guard = PINS.lock().expect("pin registry poisoned");
        *guard = Some(registry);
    }
    Ok(count)
}

/// Save the live registry to its data root.
///
/// A registry with no configured root has not been restored by the app, so it
/// has nowhere user-owned to write; that keeps unit tests and library callers
/// from touching the real data directory.
pub fn save() -> Result<(), PinError> {
    let Some(root) = ROOT.lock().expect("pin root poisoned").clone() else {
        return Ok(());
    };
    let registry = with_pins(|pins| pins.clone());
    registry.save_at(&root)
}

/// Pin a file, reporting why it could not be pinned.
///
/// The image is decoded for its dimensions; the first pin sits under the
/// pointer when the compositor reports one, matching the source's placement.
pub fn pin_file(path: &Path) -> Result<String, PinError> {
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
    let (start_width, start_height) = crate::pin::initial_size(image.width, image.height);
    let cursor = crate::commands::hypr(&crate::commands::SystemRunner, "cursorpos").ok();
    let (cursor_x, cursor_y) = cursor.as_ref().map_or((0.0, 0.0), |value| {
        (
            value.get("x").and_then(Value::as_f64).unwrap_or(0.0),
            value.get("y").and_then(Value::as_f64).unwrap_or(0.0),
        )
    });
    let x = (cursor_x - start_width / 2.0).max(0.0);
    let y = (cursor_y - start_height / 2.0).max(0.0);
    with_pins(|pins| pins.add(path, image.width, image.height, x, y));
    save()?;
    publish();
    Ok(format!("Pinned {}", path.display()))
}

/// Show or hide every pin.
pub fn toggle() -> String {
    let hidden = with_pins(|pins| pins.toggle());
    let message = if hidden {
        "Pins hidden".to_owned()
    } else {
        "Pins shown".to_owned()
    };
    match save() {
        Ok(()) => {
            publish();
            message
        }
        Err(error) => {
            set_message(&error.to_string());
            error.to_string()
        }
    }
}

/// Close every pin.
pub fn close_all() -> String {
    with_pins(|pins| pins.close_all());
    save_and_publish("Pins closed")
}

/// Unlock every pin, including a click-through one.
pub fn unlock_all() -> String {
    with_pins(|pins| pins.unlock_all());
    save_and_publish("Pins unlocked")
}

/// Remove one pin and persist the new set.
pub fn close(path: &Path) -> String {
    with_pins(|pins| pins.remove(path));
    save_and_publish("Pin closed")
}

/// Lock or unlock one pin and persist it.
pub fn set_locked(path: &Path, locked: bool) -> String {
    with_pins(|pins| pins.set_locked(path, locked));
    save_and_publish(if locked { "Pin locked" } else { "Pin unlocked" })
}

/// Set one pin's opacity and persist it.
pub fn set_opacity(path: &Path, value: f64) -> String {
    with_pins(|pins| pins.set_opacity(path, value));
    save_and_publish("")
}

/// Turn click-through on or off for one pin and persist it.
pub fn set_click_through(path: &Path, value: bool) -> String {
    with_pins(|pins| pins.set_click_through(path, value));
    save_and_publish(if value {
        "Pin is click-through; unlock the pins to use it again"
    } else {
        "Pin accepts clicks again"
    })
}

/// Store one pin's geometry and persist it.
pub fn set_geometry(path: &Path, x: f64, y: f64, width: f64, height: f64) {
    let changed = with_pins(|pins| pins.set_geometry(path, x, y, width, height));
    if changed && let Err(error) = save() {
        set_message(&error.to_string());
    }
}

fn save_and_publish(message: &str) -> String {
    match save() {
        Ok(()) => {
            publish();
            message.to_owned()
        }
        Err(error) => {
            set_message(&error.to_string());
            error.to_string()
        }
    }
}

/// Push the current pins to the bound QML windows.
///
/// A publish before QML binds is not lost: `bind` applies the current set.
fn publish() {
    let rows = with_pins(|pins| pins.rows_json());
    if let Some(thread) = BOUND.get() {
        let _ = thread.queue(move |mut pins| {
            pins.as_mut().apply_rows(rows);
        });
    }
}

/// The current pin rows as JSON text, one per row.
#[must_use]
pub fn rows_json() -> Vec<String> {
    with_pins(|pins| pins.rows_json())
}

fn set_message(message: &str) {
    if let Some(thread) = BOUND.get() {
        let message = message.to_owned();
        let _ = thread.queue(move |mut pins| {
            pins.as_mut()
                .set_message(cxx_qt_lib::QString::from(message.as_str()));
        });
    }
}

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
        #[qproperty(QString, message)]
        type PinApi = super::PinRust;

        /// Register this object and replay the current pins.
        #[qinvokable]
        fn bind(self: Pin<&mut Self>);

        /// Show or hide every pin.
        #[qinvokable]
        fn toggle(self: Pin<&mut Self>);

        /// Close every pin.
        #[qinvokable]
        #[cxx_name = "closeAll"]
        fn close_all(self: Pin<&mut Self>);

        /// Unlock every pin.
        #[qinvokable]
        #[cxx_name = "unlockAll"]
        fn unlock_all(self: Pin<&mut Self>);

        /// Close one pin.
        #[qinvokable]
        fn close(self: Pin<&mut Self>, path: &QString);

        /// Lock one pin.
        #[qinvokable]
        fn lock(self: Pin<&mut Self>, path: &QString);

        /// Unlock one pin.
        #[qinvokable]
        fn unlock(self: Pin<&mut Self>, path: &QString);

        /// Set one pin's opacity.
        #[qinvokable]
        #[cxx_name = "setOpacity"]
        fn set_opacity(self: Pin<&mut Self>, path: &QString, value: f64);

        /// Turn click-through on or off for one pin.
        #[qinvokable]
        #[cxx_name = "setClickThrough"]
        fn set_click_through(self: Pin<&mut Self>, path: &QString, value: bool);

        /// Persist one pin's position and size after a move or resize.
        #[qinvokable]
        #[cxx_name = "saveGeometry"]
        fn save_geometry(
            self: Pin<&mut Self>,
            path: &QString,
            x: f64,
            y: f64,
            width: f64,
            height: f64,
        );
    }

    impl cxx_qt::Threading for PinApi {}
}

use core::pin::Pin;
use cxx_qt::Threading as _;

/// Rust state behind the QML `PinApi` singleton.
pub struct PinRust {
    rows: cxx_qt_lib::QStringList,
    count: i32,
    message: cxx_qt_lib::QString,
}

impl Default for PinRust {
    fn default() -> Self {
        let rows = rows_json();
        Self {
            count: rows.len() as i32,
            rows: strings(rows),
            message: cxx_qt_lib::QString::from(""),
        }
    }
}

fn strings(rows: Vec<String>) -> cxx_qt_lib::QStringList {
    let mut list = cxx_qt_lib::QStringList::default();
    for row in rows {
        list.append(cxx_qt_lib::QString::from(row.as_str()));
    }
    list
}

impl qobject::PinApi {
    /// Register this object and replay any pins that arrived before QML.
    pub fn bind(mut self: Pin<&mut Self>) {
        let _ = BOUND.set(self.qt_thread());
        let rows = rows_json();
        self.as_mut().apply_rows(rows);
    }

    fn apply_rows(mut self: Pin<&mut Self>, rows: Vec<String>) {
        self.as_mut().set_count(rows.len() as i32);
        self.as_mut().set_rows(strings(rows));
    }

    /// Show or hide every pin.
    pub fn toggle(mut self: Pin<&mut Self>) {
        let message = toggle();
        self.as_mut().report(&message);
    }

    /// Close every pin.
    pub fn close_all(mut self: Pin<&mut Self>) {
        let message = close_all();
        self.as_mut().report(&message);
    }

    /// Unlock every pin.
    pub fn unlock_all(mut self: Pin<&mut Self>) {
        let message = unlock_all();
        self.as_mut().report(&message);
    }

    /// Close one pin.
    pub fn close(mut self: Pin<&mut Self>, path: &cxx_qt_lib::QString) {
        let message = close(Path::new(&path.to_string()));
        self.as_mut().report(&message);
    }

    /// Lock one pin.
    pub fn lock(mut self: Pin<&mut Self>, path: &cxx_qt_lib::QString) {
        let message = set_locked(Path::new(&path.to_string()), true);
        self.as_mut().report(&message);
    }

    /// Unlock one pin.
    pub fn unlock(mut self: Pin<&mut Self>, path: &cxx_qt_lib::QString) {
        let message = set_locked(Path::new(&path.to_string()), false);
        self.as_mut().report(&message);
    }

    /// Set one pin's opacity.
    pub fn set_opacity(mut self: Pin<&mut Self>, path: &cxx_qt_lib::QString, value: f64) {
        let message = set_opacity(Path::new(&path.to_string()), value);
        self.as_mut().report(&message);
    }

    /// Turn click-through on or off for one pin.
    pub fn set_click_through(mut self: Pin<&mut Self>, path: &cxx_qt_lib::QString, value: bool) {
        let message = set_click_through(Path::new(&path.to_string()), value);
        self.as_mut().report(&message);
    }

    /// Persist one pin's position and size after a move or resize.
    pub fn save_geometry(
        self: Pin<&mut Self>,
        path: &cxx_qt_lib::QString,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) {
        set_geometry(Path::new(&path.to_string()), x, y, width, height);
    }

    fn report(mut self: Pin<&mut Self>, message: &str) {
        self.as_mut()
            .set_message(cxx_qt_lib::QString::from(message));
        let rows = rows_json();
        self.as_mut().apply_rows(rows);
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
    fn adding_a_pin_keeps_its_starting_size_and_replaces_the_same_path() {
        let dir = scratch("add");
        let path = dir.join("shot.png");
        std::fs::write(&path, png(640, 320)).unwrap();
        let mut pins = PinRegistry::new();
        pins.add(&path, 640, 320, 10.0, 20.0);
        assert_eq!(pins.len(), 1);
        assert_eq!(pins.entries()[0].state.width, 520.0);
        assert_eq!(pins.entries()[0].state.image_width, 640);
        assert_eq!((pins.entries()[0].x, pins.entries()[0].y), (10.0, 20.0));
        // The same file pinned again replaces the old entry.
        pins.add(&path, 640, 320, 50.0, 60.0);
        assert_eq!(pins.len(), 1);
        assert_eq!((pins.entries()[0].x, pins.entries()[0].y), (50.0, 60.0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn removing_one_pin_leaves_the_rest_and_close_all_clears_the_set() {
        let root = scratch("remove");
        let first = root.join("a.png");
        let second = root.join("b.png");
        std::fs::write(&first, png(100, 80)).unwrap();
        std::fs::write(&second, png(120, 90)).unwrap();
        let mut pins = PinRegistry::new();
        pins.add(&first, 100, 80, 0.0, 0.0);
        pins.add(&second, 120, 90, 40.0, 40.0);
        assert!(pins.remove(&first));
        assert!(!pins.remove(&first));
        assert_eq!(pins.len(), 1);
        assert_eq!(pins.entries()[0].path, second);
        pins.close_all();
        assert!(pins.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn toggle_hides_and_shows_every_pin() {
        let root = scratch("toggle");
        let path = root.join("shot.png");
        std::fs::write(&path, png(100, 80)).unwrap();
        let mut pins = PinRegistry::new();
        pins.add(&path, 100, 80, 0.0, 0.0);
        assert!(pins.toggle());
        assert!(pins.hidden());
        assert!(!pins.entries()[0].visible);
        assert!(!pins.toggle());
        assert!(pins.entries()[0].visible);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unlock_all_clears_locks_and_click_through() {
        let root = scratch("unlock");
        let path = root.join("shot.png");
        std::fs::write(&path, png(100, 80)).unwrap();
        let mut pins = PinRegistry::new();
        pins.add(&path, 100, 80, 0.0, 0.0);
        pins.set_locked(&path, true);
        assert!(pins.entries()[0].state.locked);
        pins.unlock_all();
        assert!(!pins.entries()[0].state.locked);
        pins.set_click_through(&path, true);
        assert!(pins.entries()[0].click_through);
        pins.unlock_all();
        assert!(!pins.entries()[0].click_through);
        assert!(!pins.entries()[0].state.locked);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn click_through_locks_a_pin_so_its_controls_cannot_be_reached() {
        let root = scratch("click-through");
        let path = root.join("shot.png");
        std::fs::write(&path, png(100, 80)).unwrap();
        let mut pins = PinRegistry::new();
        pins.add(&path, 100, 80, 0.0, 0.0);
        pins.set_opacity(&path, 0.5);
        assert!((pins.entries()[0].state.opacity - 0.5).abs() < f64::EPSILON);
        assert!(pins.set_click_through(&path, true));
        assert!(pins.entries()[0].click_through);
        assert!(pins.entries()[0].state.locked);
        assert!(pins.entries()[0].title().contains("Locked"));
        pins.set_click_through(&path, false);
        assert!(!pins.entries()[0].click_through);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn geometry_updates_are_persisted_and_clamped() {
        let root = scratch("geometry");
        let path = root.join("shot.png");
        std::fs::write(&path, png(200, 100)).unwrap();
        let mut pins = PinRegistry::new();
        pins.add(&path, 200, 100, 10.0, 10.0);
        assert!(pins.set_geometry(&path, 30.0, 40.0, 260.0, 130.0));
        assert!(!pins.set_geometry(&path, 30.0, 40.0, 260.0, 130.0));
        let entry = &pins.entries()[0];
        assert_eq!((entry.x, entry.y), (30.0, 40.0));
        assert_eq!((entry.state.width, entry.state.height), (260.0, 130.0));
        pins.set_geometry(&path, 0.0, 0.0, 1.0, 1.0);
        assert_eq!(pins.entries()[0].state.width, MIN_SIDE);
        assert_eq!(pins.entries()[0].state.height, MIN_SIDE);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_pin_listing_round_trips_position_size_opacity_and_click_through() {
        let root = scratch("persist");
        let path = root.join("shot.png");
        std::fs::write(&path, png(200, 100)).unwrap();
        let mut pins = PinRegistry::new();
        pins.add(&path, 200, 100, 12.0, 34.0);
        pins.set_opacity(&path, 0.4);
        pins.set_click_through(&path, true);
        pins.save_at(&root).unwrap();

        let restored = PinRegistry::restore_at(&root).unwrap();
        assert_eq!(restored.len(), 1);
        let entry = &restored.entries()[0];
        assert_eq!(entry.path, path);
        assert_eq!((entry.x, entry.y), (12.0, 34.0));
        assert_eq!(entry.state.opacity, 0.4);
        assert!(entry.click_through);
        assert!(entry.state.locked);
        // A hidden session state is not restored, so pins stay reachable.
        assert!(!restored.hidden());
        assert!(entry.visible);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_listing_is_empty_and_a_malformed_one_is_reported() {
        let root = scratch("listing");
        assert!(PinRegistry::restore_at(&root).unwrap().is_empty());
        std::fs::write(pins_path(&root), "not json").unwrap();
        assert!(PinRegistry::restore_at(&root).is_err());
        // A pin whose file vanished is skipped instead of restored broken.
        std::fs::write(
            pins_path(&root),
            r#"[{"path":"/tmp/definitely-missing-1.png"}]"#,
        )
        .unwrap();
        assert!(PinRegistry::restore_at(&root).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pin_title_identifies_tidyshots_own_window_for_the_picker() {
        assert!(is_pin_title(&pin_title("shot.png", false)));
        assert!(is_pin_title(&pin_title("shot.png", true)));
        assert!(!is_pin_title("TidyShot Preview — shot.png"));
        assert!(!is_pin_title("firefox"));
        let window = crate::window::WindowInfo {
            title: pin_title("shot.png", false),
            class: "com.example.Other".to_owned(),
            rect: crate::geometry::Rect::new(0, 0, 100, 80),
            monitor: "0".to_owned(),
            workspace: "1".to_owned(),
            address: "0x1".to_owned(),
            stable_id: None,
            focus_history: 0,
            pinned: true,
            fullscreen: false,
        };
        assert!(window.is_own_preview());
        let foreign = crate::window::WindowInfo {
            title: "shot.png".to_owned(),
            class: "com.example.Other".to_owned(),
            ..window
        };
        assert!(!foreign.is_own_preview());
    }
}
