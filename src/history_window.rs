//! The capture history window: filmstrip, filters, search, delete and trash.
// Ported from omnishot/widgets.py (HistoryWindow/gallery) and
// omnishot/app.py (restore, annotate-last, delete/trash).
//!
//! [`crate::history`] owns the on-disk listing and the trash move; this module
//! owns the window state QML renders: the kind tab, the search query, the
//! selection, the newest-first rows and every action. A delete moves the file
//! into the store's trash directory and never unlinks a capture.

use std::path::{Path, PathBuf};

use crate::history::{self, HistoryRow, KindFilter};

/// The tabs the window offers, in display order.
pub const TABS: &[(&str, &str)] = &[
    ("all", "All"),
    ("image", "Images"),
    ("scroll", "Scroll"),
    ("video", "Video"),
];

/// One row of the filmstrip.
#[derive(Debug, Clone, PartialEq)]
pub struct FilmstripRow {
    pub path: PathBuf,
    pub name: String,
    pub kind: String,
    pub created: f64,
    pub width: u32,
    pub height: u32,
    pub selected: bool,
}

impl FilmstripRow {
    /// Project one history row into the filmstrip.
    #[must_use]
    pub fn from_history(row: &HistoryRow, selected: bool) -> Self {
        Self {
            name: row
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("capture")
                .to_owned(),
            path: row.path.clone(),
            kind: row.kind.clone(),
            created: row.created,
            width: row.width,
            height: row.height,
            selected,
        }
    }

    /// The JSON row QML renders, including the thumbnail URL.
    #[must_use]
    pub fn to_value(&self) -> crate::json::Value {
        use crate::json::Value;
        let mut object = std::collections::BTreeMap::new();
        object.insert(
            "path".to_owned(),
            Value::String(self.path.display().to_string()),
        );
        object.insert("name".to_owned(), Value::String(self.name.clone()));
        object.insert("kind".to_owned(), Value::String(self.kind.clone()));
        object.insert("created".to_owned(), Value::Number(self.created));
        object.insert("width".to_owned(), Value::Number(f64::from(self.width)));
        object.insert("height".to_owned(), Value::Number(f64::from(self.height)));
        object.insert("selected".to_owned(), Value::Bool(self.selected));
        object.insert(
            "source".to_owned(),
            Value::String(crate::overlay::local_file_url(&self.path)),
        );
        Value::Object(object)
    }
}

/// The window's observable state.
#[derive(Debug, Clone, Default)]
pub struct HistoryView {
    root: PathBuf,
    filter: KindFilter,
    query: String,
    rows: Vec<HistoryRow>,
    selection: Vec<usize>,
    cursor: usize,
    message: String,
    open: bool,
}

impl HistoryView {
    /// A view over one store root.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            ..Self::default()
        }
    }

    /// The store root this view reads.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether the window should be visible.
    #[must_use]
    pub const fn open(&self) -> bool {
        self.open
    }

    /// Show the window and reload the listing.
    pub fn show(&mut self) {
        self.open = true;
        self.reload();
    }

    /// Hide the window.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// The current kind tab.
    #[must_use]
    pub const fn filter(&self) -> KindFilter {
        self.filter
    }

    /// The search text.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The last action's line, including an I/O failure.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The selected row indexes, in the current listing.
    #[must_use]
    pub fn selection(&self) -> &[usize] {
        &self.selection
    }

    /// The keyboard cursor row.
    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    /// The listing the filmstrip renders, newest first.
    #[must_use]
    pub fn filmstrip(&self) -> Vec<FilmstripRow> {
        self.rows
            .iter()
            .enumerate()
            .map(|(index, row)| FilmstripRow::from_history(row, self.selection.contains(&index)))
            .collect()
    }

    /// The filmstrip as JSON text rows for QML.
    #[must_use]
    pub fn rows_json(&self) -> Vec<String> {
        self.filmstrip()
            .iter()
            .map(|row| row.to_value().to_string_compact())
            .collect()
    }

    /// Reload the listing from disk, dropping a selection that no longer fits.
    pub fn reload(&mut self) {
        self.rows = history::filtered(&self.root, self.filter, &self.query);
        self.selection.retain(|index| *index < self.rows.len());
        if self.cursor >= self.rows.len() {
            self.cursor = self.rows.len().saturating_sub(1);
        }
    }

    /// Switch the kind tab, clearing the selection.
    pub fn set_filter(&mut self, filter: KindFilter) {
        self.filter = filter;
        self.selection.clear();
        self.cursor = 0;
        self.reload();
    }

    /// Apply a search query, clearing the selection.
    pub fn set_query(&mut self, query: &str) {
        self.query = query.to_owned();
        self.selection.clear();
        self.cursor = 0;
        self.reload();
    }

    /// Move the cursor by `delta`, clamped to the listing.
    pub fn move_cursor(&mut self, delta: i32) {
        if self.rows.is_empty() {
            self.cursor = 0;
            return;
        }
        let last = self.rows.len() as i32 - 1;
        self.cursor = (self.cursor as i32 + delta).clamp(0, last) as usize;
    }

    /// Point the cursor at a row and select it.
    ///
    /// A window click names the row it hit, so the row is used directly rather
    /// than as a delta from the cursor QML last saw.
    pub fn select_row(&mut self, index: usize, extend: bool) {
        if index >= self.rows.len() {
            return;
        }
        self.cursor = index;
        self.select_cursor(extend);
    }

    /// Select the cursor row, or add it to the selection.
    pub fn select_cursor(&mut self, extend: bool) {
        if self.cursor >= self.rows.len() {
            return;
        }
        if extend {
            if self.selection.contains(&self.cursor) {
                self.selection.retain(|index| *index != self.cursor);
            } else {
                self.selection.push(self.cursor);
                self.selection.sort_unstable();
            }
        } else {
            self.selection = vec![self.cursor];
        }
    }

    /// Select every visible row.
    pub fn select_all(&mut self) {
        self.selection = (0..self.rows.len()).collect();
    }

    /// The selected paths in listing order.
    #[must_use]
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selection
            .iter()
            .filter_map(|index| self.rows.get(*index).map(|row| row.path.clone()))
            .collect()
    }

    /// The row the cursor points at.
    #[must_use]
    pub fn cursor_row(&self) -> Option<&HistoryRow> {
        self.rows.get(self.cursor)
    }

    /// Move every selected capture to the trash and drop its row.
    pub fn delete_selected(&mut self) -> usize {
        let paths: Vec<PathBuf> = if self.selection.is_empty() {
            self.cursor_row()
                .map(|row| vec![row.path.clone()])
                .unwrap_or_default()
        } else {
            self.selected_paths()
        };
        let mut moved = 0;
        let mut errors: Vec<String> = Vec::new();
        for path in &paths {
            match history::delete(&self.root, path) {
                Ok(_) => moved += 1,
                Err(error) => errors.push(error.to_string()),
            }
        }
        self.selection.clear();
        self.reload();
        self.message = if !errors.is_empty() {
            errors.join("; ")
        } else if paths.is_empty() {
            "There is no capture to delete".to_owned()
        } else {
            format!(
                "Moved {moved} {} to the trash",
                if moved == 1 { "capture" } else { "captures" }
            )
        };
        moved
    }

    /// Move every capture in the history to the trash.
    pub fn clear(&mut self) -> usize {
        match history::clear(&self.root) {
            Ok(moved) => {
                self.selection.clear();
                self.reload();
                self.message = format!("Moved {moved} captures to the trash");
                moved
            }
            Err(error) => {
                self.message = error.to_string();
                0
            }
        }
    }

    /// The row the keyboard Enter should open, preferring the selection.
    #[must_use]
    pub fn open_target(&self) -> Option<PathBuf> {
        self.selection
            .first()
            .and_then(|index| self.rows.get(*index))
            .or_else(|| self.cursor_row())
            .map(|row| row.path.clone())
    }
}

/// Run the retention cleanup on startup.
///
/// The cleanup is best-effort: a failure is reported but never stops the app
/// from starting.
pub fn cleanup_on_start(root: &Path) -> Result<usize, String> {
    let days = crate::settings::retention_days();
    history::prune(root, days, now_seconds()).map_err(|error| error.to_string())
}

fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64())
}

// --- QML bridge: the `HistoryApi` singleton ------------------------------

/// The view shared by the window and a command that opens it.
static VIEW: std::sync::Mutex<Option<HistoryView>> = std::sync::Mutex::new(None);
static BOUND: std::sync::OnceLock<cxx_qt::CxxQtThread<qobject::HistoryApi>> =
    std::sync::OnceLock::new();

/// Run `f` against the shared history view, creating it on first use.
///
/// This is the same view the QML window drives, so a test or another command
/// can inspect and act on the visible history without a Qt object.
pub fn view<T>(f: impl FnOnce(&mut HistoryView) -> T) -> T {
    let mut view = VIEW.lock().expect("history view poisoned");
    let view = view.get_or_insert_with(|| HistoryView::new(crate::settings::data_dir()));
    f(view)
}

/// Open the history window, creating its view if the GUI has not yet.
pub fn request_open() {
    request_open_at(&crate::settings::data_dir());
}

/// Open the history window over an explicit capture store.
///
/// The root is a parameter so a command can be exercised against a scratch
/// store; production passes [`crate::settings::data_dir`].
pub fn request_open_at(root: &Path) {
    let snapshot = {
        let mut view = VIEW.lock().expect("history view poisoned");
        if view.as_ref().is_none_or(|view| view.root() != root) {
            *view = Some(HistoryView::new(root));
        }
        let view = view.as_mut().expect("the view was just created");
        view.show();
        Snapshot::of(view)
    };
    publish(snapshot);
}

/// A projection of the view onto the QML properties.
#[derive(Debug, Clone, Default)]
struct Snapshot {
    open: bool,
    rows: Vec<String>,
    filter: String,
    query: String,
    message: String,
    cursor: i32,
    selected: i32,
}

impl Snapshot {
    fn of(view: &HistoryView) -> Self {
        Self {
            open: view.open(),
            rows: view.rows_json(),
            filter: view.filter().id().to_owned(),
            query: view.query().to_owned(),
            message: view.message().to_owned(),
            cursor: view.cursor() as i32,
            selected: view.selection().len() as i32,
        }
    }
}

/// Queue a snapshot onto the bound window, or keep it for the bind.
fn publish(snapshot: Snapshot) {
    match BOUND.get() {
        Some(thread) => {
            let _ = thread.queue(move |mut api| api.as_mut().apply(snapshot));
        }
        None => PENDING
            .lock()
            .expect("history queue poisoned")
            .push(snapshot),
    }
}

/// Snapshots produced before QML bound, replayed in order at the bind.
static PENDING: std::sync::Mutex<Vec<Snapshot>> = std::sync::Mutex::new(Vec::new());

/// Open the selected capture, or the newest one, in the annotate editor.
pub fn annotate_selected() -> Option<PathBuf> {
    let path = view(|view| view.open_target())?;
    crate::editor_window::publish_open(path.clone());
    Some(path)
}

/// Show the selected capture, or the newest one, in the corner preview.
pub fn restore_selected() -> Option<PathBuf> {
    let path = view(|view| view.open_target())?;
    crate::preview::publish_file(path.clone());
    Some(path)
}

/// Pin the selected capture, or the newest one.
pub fn pin_selected() -> Result<String, String> {
    let path =
        view(|view| view.open_target()).ok_or_else(|| "There is no capture to pin".to_owned())?;
    crate::pin_runtime::pin_file(&path).map_err(|error| error.to_string())
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
        /// The capture history window exposed to QML.
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(bool, open)]
        #[qproperty(QStringList, rows)]
        #[qproperty(QString, filter)]
        #[qproperty(QString, query)]
        #[qproperty(QString, message)]
        #[qproperty(i32, cursor)]
        #[qproperty(i32, selected)]
        #[qproperty(QStringList, tab_ids, cxx_name = "tabIds")]
        #[qproperty(QStringList, tab_labels, cxx_name = "tabLabels")]
        type HistoryApi = super::HistoryRust;

        /// Register this object and replay any pending open.
        #[qinvokable]
        fn bind(self: Pin<&mut Self>);

        /// Switch the type tab.
        #[qinvokable]
        #[cxx_name = "chooseFilter"]
        fn choose_filter(self: Pin<&mut Self>, filter: &QString);

        /// Apply a search query.
        #[qinvokable]
        #[cxx_name = "search"]
        fn search(self: Pin<&mut Self>, query: &QString);

        /// Move the cursor by a delta.
        #[qinvokable]
        fn move_cursor(self: Pin<&mut Self>, delta: i32);

        /// Select the cursor row, adding to the selection when extending.
        #[qinvokable]
        #[cxx_name = "selectCursor"]
        fn select_cursor(self: Pin<&mut Self>, extend: bool);

        /// Select one row by index, adding to the selection when extending.
        #[qinvokable]
        #[cxx_name = "selectRow"]
        fn select_row(self: Pin<&mut Self>, index: i32, extend: bool);

        /// Select every visible row.
        #[qinvokable]
        #[cxx_name = "selectAll"]
        fn select_all(self: Pin<&mut Self>);

        /// Reload the listing from disk.
        #[qinvokable]
        fn refresh(self: Pin<&mut Self>);

        /// Open the selected capture in the annotate editor.
        #[qinvokable]
        fn annotate(self: Pin<&mut Self>);

        /// Show the selected capture in the corner preview.
        #[qinvokable]
        fn restore(self: Pin<&mut Self>);

        /// Pin the selected capture.
        #[qinvokable]
        fn pin(self: Pin<&mut Self>);

        /// Move the selected captures to the trash.
        #[qinvokable]
        #[cxx_name = "deleteSelected"]
        fn delete_selected(self: Pin<&mut Self>);

        /// Move every capture in the history to the trash.
        #[qinvokable]
        #[cxx_name = "clearHistory"]
        fn clear_history(self: Pin<&mut Self>);

        /// Close the window.
        #[qinvokable]
        fn close(self: Pin<&mut Self>);
    }

    impl cxx_qt::Threading for HistoryApi {}
}

use core::pin::Pin;
use cxx_qt::Threading as _;
use cxx_qt_lib::{QString, QStringList};

/// Rust state behind the QML `HistoryApi` singleton.
pub struct HistoryRust {
    open: bool,
    rows: QStringList,
    filter: QString,
    query: QString,
    message: QString,
    cursor: i32,
    selected: i32,
    tab_ids: QStringList,
    tab_labels: QStringList,
}

impl Default for HistoryRust {
    fn default() -> Self {
        let mut tab_ids = QStringList::default();
        let mut tab_labels = QStringList::default();
        for (id, label) in TABS {
            tab_ids.append(QString::from(*id));
            tab_labels.append(QString::from(*label));
        }
        Self {
            open: false,
            rows: QStringList::default(),
            filter: QString::from("all"),
            query: QString::from(""),
            message: QString::from(""),
            cursor: 0,
            selected: 0,
            tab_ids,
            tab_labels,
        }
    }
}

impl qobject::HistoryApi {
    /// Register this object and replay any open that arrived before QML.
    pub fn bind(mut self: Pin<&mut Self>) {
        let _ = BOUND.set(self.qt_thread());
        // A window that was opened by a command before QML loaded reloads its
        // listing here so the filmstrip is never empty on first show. Every
        // queued snapshot is replayed in order, so a delete that followed an
        // open does not lose the open.
        let pending = std::mem::take(&mut *PENDING.lock().expect("history queue poisoned"));
        if pending.is_empty() {
            let snapshot = view(|view| Snapshot::of(view));
            self.as_mut().apply(snapshot);
        } else {
            for snapshot in pending {
                self.as_mut().apply(snapshot);
            }
        }
    }

    fn apply(mut self: Pin<&mut Self>, snapshot: Snapshot) {
        let mut rows = QStringList::default();
        for row in &snapshot.rows {
            rows.append(QString::from(row.clone()));
        }
        self.as_mut().set_open(snapshot.open);
        self.as_mut().set_rows(rows);
        self.as_mut().set_filter(QString::from(snapshot.filter));
        self.as_mut().set_query(QString::from(snapshot.query));
        self.as_mut().set_message(QString::from(snapshot.message));
        self.as_mut().set_cursor(snapshot.cursor);
        self.as_mut().set_selected(snapshot.selected);
    }

    /// Switch the type tab.
    pub fn choose_filter(mut self: Pin<&mut Self>, filter: &QString) {
        self.as_mut()
            .change(|view| view.set_filter(KindFilter::parse(&filter.to_string())));
    }

    /// Apply a search query.
    pub fn search(mut self: Pin<&mut Self>, query: &QString) {
        let query = query.to_string();
        self.as_mut().change(|view| view.set_query(&query));
    }

    /// Move the cursor by a delta.
    pub fn move_cursor(mut self: Pin<&mut Self>, delta: i32) {
        self.as_mut().change(|view| view.move_cursor(delta));
    }

    /// Select the cursor row, adding to the selection when extending.
    pub fn select_cursor(mut self: Pin<&mut Self>, extend: bool) {
        self.as_mut().change(|view| view.select_cursor(extend));
    }

    /// Select one row by index, adding to the selection when extending.
    pub fn select_row(mut self: Pin<&mut Self>, index: i32, extend: bool) {
        if index < 0 {
            return;
        }
        self.as_mut()
            .change(|view| view.select_row(index as usize, extend));
    }

    /// Select every visible row.
    pub fn select_all(mut self: Pin<&mut Self>) {
        self.as_mut().change(HistoryView::select_all);
    }

    /// Reload the listing from disk.
    pub fn refresh(mut self: Pin<&mut Self>) {
        self.as_mut().change(HistoryView::reload);
    }

    /// Open the selected capture in the annotate editor.
    pub fn annotate(mut self: Pin<&mut Self>) {
        let result = match annotate_selected() {
            Some(path) => format!("Opened {}", path.display()),
            None => "There is no capture to annotate".to_owned(),
        };
        self.as_mut().change(|view| view.message = result);
    }

    /// Show the selected capture in the corner preview.
    pub fn restore(mut self: Pin<&mut Self>) {
        let result = match restore_selected() {
            Some(path) => format!("Restored {}", path.display()),
            None => "There is no capture to restore".to_owned(),
        };
        self.as_mut().change(|view| view.message = result);
    }

    /// Pin the selected capture.
    pub fn pin(mut self: Pin<&mut Self>) {
        let result = pin_selected().unwrap_or_else(|error| error);
        self.as_mut().change(|view| view.message = result);
    }

    /// Move the selected captures to the trash.
    pub fn delete_selected(mut self: Pin<&mut Self>) {
        self.as_mut().change(|view| {
            view.delete_selected();
        });
    }

    /// Move every capture in the history to the trash.
    pub fn clear_history(mut self: Pin<&mut Self>) {
        self.as_mut().change(|view| {
            view.clear();
        });
    }

    /// Close the window.
    pub fn close(mut self: Pin<&mut Self>) {
        self.as_mut().change(HistoryView::close);
    }

    /// Run `f` against the shared view, then push the new state to QML.
    fn change<T>(mut self: Pin<&mut Self>, f: impl FnOnce(&mut HistoryView) -> T) -> T {
        let result = view(f);
        let snapshot = view(|view| Snapshot::of(view));
        self.as_mut().apply(snapshot);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{HistoryRow, captures_dir, read, write};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tidyshot-hwin-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(captures_dir(&dir)).unwrap();
        dir
    }

    fn seed(root: &Path) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for (name, row) in [
            ("new.png", HistoryRow::screenshot("", 1, 1, None)),
            ("page.png", HistoryRow::scroll("", 1, 1, None)),
            ("take.mp4", HistoryRow::video("", 1, 1)),
        ] {
            let path = captures_dir(root).join(name);
            std::fs::write(&path, b"data").unwrap();
            paths.push(path.clone());
            let mut row = row;
            row.path = path;
            let mut rows = read(root);
            rows.insert(0, row);
            write(root, &rows).unwrap();
        }
        paths
    }

    #[test]
    fn the_filmstrip_is_newest_first_and_filters_by_tab() {
        let root = scratch("filmstrip");
        let paths = seed(&root);
        let mut view = HistoryView::new(&root);
        view.show();
        let rows = view.filmstrip();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].path, paths[2], "the video was seeded last");
        assert_eq!(rows[0].kind, "video");
        view.set_filter(KindFilter::Scroll);
        assert_eq!(view.filmstrip().len(), 1);
        view.set_filter(KindFilter::Image);
        assert_eq!(view.filmstrip().len(), 2);
        view.set_filter(KindFilter::All);
        assert_eq!(view.filmstrip().len(), 3);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn search_narrows_by_name_and_clears_on_a_new_tab() {
        let root = scratch("search");
        seed(&root);
        let mut view = HistoryView::new(&root);
        view.show();
        view.set_query("PAGE");
        assert_eq!(view.filmstrip().len(), 1);
        assert_eq!(view.filmstrip()[0].name, "page.png");
        view.set_query("");
        assert_eq!(view.filmstrip().len(), 3);
        view.set_query("nothing-here");
        assert!(view.filmstrip().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cursor_and_selection_move_through_the_listing() {
        let root = scratch("selection");
        seed(&root);
        let mut view = HistoryView::new(&root);
        view.show();
        assert_eq!(view.cursor(), 0);
        view.move_cursor(1);
        view.select_cursor(true);
        assert_eq!(view.selection(), [1]);
        view.move_cursor(1);
        view.select_cursor(true);
        assert_eq!(view.selection(), [1, 2]);
        assert_eq!(view.selected_paths().len(), 2);
        view.move_cursor(-10);
        assert_eq!(view.cursor(), 0);
        view.move_cursor(100);
        assert_eq!(view.cursor(), 2);
        view.select_cursor(false);
        assert_eq!(view.selection(), [2]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deleting_moves_the_file_to_trash_and_keeps_other_captures() {
        let root = scratch("delete");
        let paths = seed(&root);
        let mut view = HistoryView::new(&root);
        view.show();
        // Seed order makes the video first; select the screenshot row.
        view.set_query("new.png");
        view.select_cursor(false);
        assert_eq!(view.delete_selected(), 1);
        assert!(!paths[0].exists(), "the deleted capture left its path");
        assert!(paths[1].exists() && paths[2].exists());
        assert!(view.filmstrip().is_empty());
        assert!(view.message().contains("trash"));
        // The trashed file still exists under the trash directory.
        let trashed = std::fs::read_dir(history::trash_dir(&root))
            .unwrap()
            .count();
        assert_eq!(trashed, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_delete_of_a_missing_file_is_surfaced_and_keeps_the_row() {
        let root = scratch("missing");
        let path = captures_dir(&root).join("gone.png");
        write(&root, &[HistoryRow::screenshot(&path, 1, 1, None)]).unwrap();
        let mut view = HistoryView::new(&root);
        view.show();
        view.select_cursor(false);
        assert_eq!(view.delete_selected(), 0);
        assert!(!view.message().is_empty());
        assert_eq!(read(&root).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clear_moves_everything_and_reports_the_count() {
        let root = scratch("clear");
        seed(&root);
        let mut view = HistoryView::new(&root);
        view.show();
        assert_eq!(view.clear(), 3);
        assert!(view.filmstrip().is_empty());
        assert!(view.message().contains('3'));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_row_click_selects_the_row_it_names() {
        let root = scratch("click");
        seed(&root);
        let mut view = HistoryView::new(&root);
        view.show();
        // A click carries the row it hit; no cursor delta is involved, so a
        // fast second click cannot land on the wrong row.
        view.select_row(2, false);
        assert_eq!(view.cursor(), 2);
        assert_eq!(view.selection(), [2]);
        view.select_row(0, true);
        assert_eq!(view.cursor(), 0);
        assert_eq!(view.selection(), [0, 2]);
        view.select_row(0, true);
        assert_eq!(view.selection(), [2], "Ctrl-click toggles a row off");
        // A row past the listing is ignored rather than wrapping.
        view.select_row(9, false);
        assert_eq!(view.cursor(), 0);
        assert_eq!(view.selection(), [2]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_move_to_trash_never_overwrites_or_unlinks_a_capture() {
        let root = scratch("trash-safe");
        let paths = seed(&root);
        let trashed = super::history::move_to_trash(&root, &paths[0]).unwrap();
        assert_eq!(std::fs::read(&trashed).unwrap(), b"data");
        // Deleting the already trashed file again is refused, so its bytes
        // stay where the first move put them.
        assert!(super::history::move_to_trash(&root, &trashed).is_err());
        assert_eq!(std::fs::read(&trashed).unwrap(), b"data");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn queued_snapshots_before_the_bind_are_replayed_in_order() {
        // A command can open the window and a second one delete before QML
        // binds; the bind must end on the later state, not the first snapshot.
        let mut first = HistoryView::new(scratch("pending"));
        first.message = "opened".to_owned();
        let mut second = HistoryView::new(scratch("pending-2"));
        second.message = "deleted".to_owned();
        {
            let mut pending = PENDING.lock().unwrap();
            pending.clear();
            pending.push(Snapshot::of(&first));
            pending.push(Snapshot::of(&second));
        }
        let queued = PENDING.lock().unwrap().clone();
        assert_eq!(queued.len(), 2);
        assert_eq!(queued[0].message, "opened");
        assert_eq!(queued[1].message, "deleted");
        // A bind applies them all, so the last one wins.
        let mut applied = Vec::new();
        for snapshot in queued {
            applied.push(snapshot.message);
        }
        assert_eq!(applied, ["opened", "deleted"]);
        PENDING.lock().unwrap().clear();
    }

    #[test]
    fn enter_prefers_the_selection_then_the_cursor() {
        let root = scratch("open");
        let paths = seed(&root);
        let mut view = HistoryView::new(&root);
        view.show();
        assert_eq!(view.open_target(), Some(paths[2].clone()));
        view.move_cursor(2);
        view.select_cursor(false);
        assert_eq!(view.open_target(), Some(paths[0].clone()));
        let _ = std::fs::remove_dir_all(&root);
    }
}
