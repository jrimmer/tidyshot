//! One action table that drives every menu surface.
// Ported from omnishot/app.py (dispatch, tray menu) and
// omnishot/shortcuts.py (ACTIONS, DEFAULTS).
//!
//! The Omarchy bar panel, the tray fallback menu, and `tidyshot menu` all
//! render [`ACTIONS`]. Adding an action is one row here; no renderer keeps a
//! second copy of the list.
//!
//! Capture rows route to [`capture::CaptureRequest`]; every other row is an
//! enum stub that reports not-implemented until its module lands.

use cxx_qt_lib::{QString, QStringList};

use crate::capture::CaptureRequest;

/// What a menu row asks the application to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    /// A screenshot command handled by the capture module.
    Capture(CaptureRequest),
    Record,
    Stop,
    Pause,
    Open,
    Clipboard,
    History,
    Restore,
    Settings,
    TogglePins,
    SaveAll,
    CloseAll,
    UnlockPins,
    ClosePins,
    Quit,
}

impl MenuAction {
    /// The dispatch command this action corresponds to.
    #[must_use]
    pub const fn command(self) -> &'static str {
        match self {
            MenuAction::Capture(request) => request.command(),
            MenuAction::Record => "record",
            MenuAction::Stop => "stop",
            MenuAction::Pause => "pause",
            MenuAction::Open => "open",
            MenuAction::Clipboard => "clipboard",
            MenuAction::History => "history",
            MenuAction::Restore => "restore",
            MenuAction::Settings => "settings",
            MenuAction::TogglePins => "toggle-pins",
            MenuAction::SaveAll => "save-all",
            MenuAction::CloseAll => "close-all",
            MenuAction::UnlockPins => "unlock-pins",
            MenuAction::ClosePins => "close-pins",
            MenuAction::Quit => "quit",
        }
    }
}

/// One row of the action table: identity, label, shortcut hint, and action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionRow {
    /// Stable identifier, matching the dispatch command.
    pub id: &'static str,
    /// Human-readable label used by all three renderers.
    pub label: &'static str,
    /// Global shortcut hint, or an empty string when unbound.
    pub shortcut: &'static str,
    /// The action the row invokes.
    pub action: MenuAction,
}

impl ActionRow {
    /// Whether this row can be performed today.
    #[must_use]
    pub const fn implemented(self) -> bool {
        matches!(self.action, MenuAction::Capture(_))
    }
}

/// The single action table. Every renderer iterates this slice in order.
pub const ACTIONS: &[ActionRow] = &[
    ActionRow {
        id: "all-in-one",
        label: "All-in-one",
        shortcut: "",
        action: MenuAction::Capture(CaptureRequest::AllInOne),
    },
    ActionRow {
        id: "area",
        label: "Capture area",
        shortcut: "Ctrl+Print",
        action: MenuAction::Capture(CaptureRequest::Area),
    },
    ActionRow {
        id: "window",
        label: "Capture window",
        shortcut: "",
        action: MenuAction::Capture(CaptureRequest::Window),
    },
    ActionRow {
        id: "fullscreen",
        label: "Capture display",
        shortcut: "Shift+Print",
        action: MenuAction::Capture(CaptureRequest::Display),
    },
    ActionRow {
        id: "desktop",
        label: "Capture all displays",
        shortcut: "",
        action: MenuAction::Capture(CaptureRequest::AllDisplays),
    },
    ActionRow {
        id: "previous",
        label: "Previous area",
        shortcut: "",
        action: MenuAction::Capture(CaptureRequest::PreviousArea),
    },
    ActionRow {
        id: "scroll",
        label: "Scrolling capture",
        shortcut: "Meta+Shift+Print",
        action: MenuAction::Capture(CaptureRequest::Scrolling),
    },
    ActionRow {
        id: "scroll-horizontal",
        label: "Horizontal scrolling",
        shortcut: "",
        action: MenuAction::Capture(CaptureRequest::HorizontalScrolling),
    },
    ActionRow {
        id: "timer",
        label: "Self timer",
        shortcut: "",
        action: MenuAction::Capture(CaptureRequest::SelfTimer),
    },
    ActionRow {
        id: "ocr",
        label: "Capture text",
        shortcut: "Meta+Ctrl+Print",
        action: MenuAction::Capture(CaptureRequest::Text),
    },
    ActionRow {
        id: "record",
        label: "Record screen",
        shortcut: "Alt+Print",
        action: MenuAction::Record,
    },
    ActionRow {
        id: "stop",
        label: "Stop recording",
        shortcut: "",
        action: MenuAction::Stop,
    },
    ActionRow {
        id: "pause",
        label: "Pause recording",
        shortcut: "",
        action: MenuAction::Pause,
    },
    ActionRow {
        id: "open",
        label: "Open image",
        shortcut: "",
        action: MenuAction::Open,
    },
    ActionRow {
        id: "clipboard",
        label: "From clipboard",
        shortcut: "",
        action: MenuAction::Clipboard,
    },
    ActionRow {
        id: "history",
        label: "History",
        shortcut: "",
        action: MenuAction::History,
    },
    ActionRow {
        id: "restore",
        label: "Restore preview",
        shortcut: "",
        action: MenuAction::Restore,
    },
    ActionRow {
        id: "settings",
        label: "Settings",
        shortcut: "",
        action: MenuAction::Settings,
    },
    ActionRow {
        id: "toggle-pins",
        label: "Show / hide pins",
        shortcut: "",
        action: MenuAction::TogglePins,
    },
    ActionRow {
        id: "save-all",
        label: "Save all previews",
        shortcut: "",
        action: MenuAction::SaveAll,
    },
    ActionRow {
        id: "close-all",
        label: "Close all previews",
        shortcut: "",
        action: MenuAction::CloseAll,
    },
    ActionRow {
        id: "unlock-pins",
        label: "Unlock pins",
        shortcut: "",
        action: MenuAction::UnlockPins,
    },
    ActionRow {
        id: "close-pins",
        label: "Close all pins",
        shortcut: "",
        action: MenuAction::ClosePins,
    },
    ActionRow {
        id: "quit",
        label: "Quit",
        shortcut: "",
        action: MenuAction::Quit,
    },
];

/// The result of activating a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invocation {
    /// Hand the request to the capture module.
    Capture(CaptureRequest),
    /// The action's module has not landed yet.
    NotImplemented(&'static str),
}

/// Look up a row by id.
#[must_use]
pub fn find(id: &str) -> Option<&'static ActionRow> {
    ACTIONS.iter().find(|row| row.id == id)
}

/// The index of a row, or `None` when the id is unknown.
#[must_use]
pub fn index_of(id: &str) -> Option<usize> {
    ACTIONS.iter().position(|row| row.id == id)
}

/// Activate a row: capture requests route to the capture module, everything
/// else is a not-implemented placeholder until its module lands.
pub fn invoke(row: &ActionRow) -> Invocation {
    match row.action {
        MenuAction::Capture(request) => Invocation::Capture(request),
        _ => Invocation::NotImplemented(row.id),
    }
}

// --- Renderer 1: the Omarchy-style bar panel -----------------------------

/// The model the QML panel renders, derived from the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelRow {
    pub id: &'static str,
    pub label: &'static str,
    pub shortcut: &'static str,
}

/// Build the panel model. One entry per table row, in table order.
#[must_use]
pub fn panel_rows() -> Vec<PanelRow> {
    panel_rows_from(ACTIONS)
}

/// Build the panel model from an arbitrary table, so tests can add a row.
#[must_use]
pub fn panel_rows_from(rows: &[ActionRow]) -> Vec<PanelRow> {
    rows.iter()
        .map(|row| PanelRow {
            id: row.id,
            label: row.label,
            shortcut: row.shortcut,
        })
        .collect()
}

// --- Renderer 2: the tray fallback menu ----------------------------------

/// One tray menu entry: an action or a separator between groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEntry<'a> {
    Action(&'a ActionRow),
    Separator,
}

/// Which group a row belongs to; the tray separates adjacent groups.
const fn group(action: MenuAction) -> u8 {
    match action {
        MenuAction::Capture(_)
        | MenuAction::Open
        | MenuAction::Clipboard
        | MenuAction::History
        | MenuAction::Restore
        | MenuAction::Settings => 0,
        MenuAction::Record | MenuAction::Stop | MenuAction::Pause => 1,
        MenuAction::TogglePins
        | MenuAction::SaveAll
        | MenuAction::CloseAll
        | MenuAction::UnlockPins
        | MenuAction::ClosePins => 2,
        MenuAction::Quit => 3,
    }
}

/// Build the tray menu, inserting a separator whenever the group changes.
#[must_use]
pub fn tray_entries() -> Vec<TrayEntry<'static>> {
    tray_entries_from(ACTIONS)
}

/// Build the tray menu from an arbitrary table, so tests can add a row.
#[must_use]
pub fn tray_entries_from(rows: &[ActionRow]) -> Vec<TrayEntry<'_>> {
    let mut entries = Vec::with_capacity(rows.len() + 3);
    let mut previous: Option<u8> = None;
    for row in rows {
        let current = group(row.action);
        if let Some(previous) = previous
            && previous != current
        {
            entries.push(TrayEntry::Separator);
        }
        entries.push(TrayEntry::Action(row));
        previous = Some(current);
    }
    entries
}

// --- Renderer 3: the `tidyshot menu` CLI listing -------------------------

/// One formatted line per table row, for the CLI listing.
#[must_use]
pub fn cli_lines() -> Vec<String> {
    cli_lines_from(ACTIONS)
}

/// Format an arbitrary table, so tests can add a row.
#[must_use]
pub fn cli_lines_from(rows: &[ActionRow]) -> Vec<String> {
    rows.iter()
        .map(|row| {
            let shortcut = if row.shortcut.is_empty() {
                "-"
            } else {
                row.shortcut
            };
            format!("{:<18} {:<24} {shortcut}", row.id, row.label)
        })
        .collect()
}

/// The complete CLI listing, newline terminated.
#[must_use]
pub fn cli_listing() -> String {
    let mut out = String::new();
    for line in cli_lines() {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

// --- Keyboard navigation shared by the panel -----------------------------

/// A key the panel understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    /// Move left.
    H,
    /// Move down.
    J,
    /// Move up.
    K,
    /// Move right.
    L,
    /// Activate the highlighted row.
    Enter,
    /// Close the menu.
    Escape,
    /// Move to the next panel.
    Tab,
    /// Move to the previous panel.
    Backtab,
}

/// What handling a key did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuEvent {
    SelectionChanged(usize),
    Activated(&'static ActionRow),
    Dismissed,
    Tabbed(i32),
    Ignored,
}

/// The keyboard cursor over the action table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MenuState {
    selected: usize,
}

impl MenuState {
    /// The highlighted row index.
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// Move the cursor to an explicit index, clamped to the table.
    pub fn select(&mut self, index: usize) {
        let last = ACTIONS.len().saturating_sub(1);
        self.selected = index.min(last);
    }

    /// Move the cursor by a delta, clamped to the table.
    pub fn move_by(&mut self, dx: i32, dy: i32) {
        let last = ACTIONS.len().saturating_sub(1) as i32;
        let next = (self.selected as i32 + dx + dy).clamp(0, last);
        self.selected = next as usize;
    }

    /// Apply one key, returning the observable result.
    pub fn handle_key(&mut self, key: MenuKey) -> MenuEvent {
        match key {
            MenuKey::H => {
                self.move_by(-1, 0);
                MenuEvent::SelectionChanged(self.selected)
            }
            MenuKey::L => {
                self.move_by(1, 0);
                MenuEvent::SelectionChanged(self.selected)
            }
            MenuKey::J => {
                self.move_by(0, 1);
                MenuEvent::SelectionChanged(self.selected)
            }
            MenuKey::K => {
                self.move_by(0, -1);
                MenuEvent::SelectionChanged(self.selected)
            }
            MenuKey::Enter => match ACTIONS.get(self.selected) {
                Some(row) => MenuEvent::Activated(row),
                None => MenuEvent::Ignored,
            },
            MenuKey::Escape => MenuEvent::Dismissed,
            MenuKey::Tab => MenuEvent::Tabbed(1),
            MenuKey::Backtab => MenuEvent::Tabbed(-1),
        }
    }
}

// --- QML bridge: the `Menu` singleton ------------------------------------

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    extern "RustQt" {
        /// The action table exposed to QML; every panel reads this singleton.
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QStringList, ids)]
        #[qproperty(QStringList, labels)]
        #[qproperty(QStringList, shortcuts)]
        #[qproperty(i32, selected_index, cxx_name = "selectedIndex")]
        type Menu = super::MenuRust;

        /// Move the cursor by a keyboard delta, clamped to the table.
        #[qinvokable]
        #[cxx_name = "moveSelection"]
        fn move_selection(self: Pin<&mut Self>, dx: i32, dy: i32);

        /// Highlight an explicit index.
        #[qinvokable]
        #[cxx_name = "selectIndex"]
        fn select_index(self: Pin<&mut Self>, index: i32);

        /// Activate the highlighted row; true when it routed to capture.
        #[qinvokable]
        #[cxx_name = "activate"]
        fn activate(self: Pin<&mut Self>) -> bool;

        /// Activate an explicit row index.
        #[qinvokable]
        #[cxx_name = "activateIndex"]
        fn activate_index(self: Pin<&mut Self>, index: i32) -> bool;
    }
}

use core::pin::Pin;

/// Rust state behind the QML `Menu` singleton.
pub struct MenuRust {
    ids: QStringList,
    labels: QStringList,
    shortcuts: QStringList,
    selected_index: i32,
}

impl Default for MenuRust {
    fn default() -> Self {
        let rows = panel_rows();
        let mut ids = QStringList::default();
        let mut labels = QStringList::default();
        let mut shortcuts = QStringList::default();
        for row in &rows {
            ids.append(QString::from(row.id));
            labels.append(QString::from(row.label));
            shortcuts.append(QString::from(row.shortcut));
        }
        Self {
            ids,
            labels,
            shortcuts,
            selected_index: 0,
        }
    }
}

impl qobject::Menu {
    /// Move the cursor by `dx` columns and `dy` rows.
    pub fn move_selection(self: Pin<&mut Self>, dx: i32, dy: i32) {
        let mut state = MenuState {
            selected: *self.selected_index() as usize,
        };
        state.move_by(dx, dy);
        self.set_selected_index(state.selected() as i32);
    }

    /// Highlight an explicit row, clamped to the table.
    pub fn select_index(self: Pin<&mut Self>, index: i32) {
        let mut state = MenuState {
            selected: *self.selected_index() as usize,
        };
        state.select(index.max(0) as usize);
        self.set_selected_index(state.selected() as i32);
    }

    /// Activate the highlighted row.
    pub fn activate(self: Pin<&mut Self>) -> bool {
        let index = *self.selected_index();
        self.activate_index(index)
    }

    /// Activate an explicit row; true when the row routed to capture.
    pub fn activate_index(self: Pin<&mut Self>, index: i32) -> bool {
        let index = index.max(0) as usize;
        matches!(ACTIONS.get(index).map(invoke), Some(Invocation::Capture(_)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_rows_are_unique_and_well_formed() {
        let mut ids: Vec<&str> = ACTIONS.iter().map(|row| row.id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "action ids must be unique");
        for row in ACTIONS {
            assert!(!row.id.is_empty(), "every row needs an id");
            assert!(!row.label.is_empty(), "every row needs a label");
        }
    }

    #[test]
    fn panel_rows_render_the_whole_table() {
        let rows = panel_rows();
        assert_eq!(rows.len(), ACTIONS.len());
        for (panel, table) in rows.iter().zip(ACTIONS) {
            assert_eq!(panel.id, table.id);
            assert_eq!(panel.label, table.label);
            assert_eq!(panel.shortcut, table.shortcut);
        }
    }

    #[test]
    fn tray_entries_render_the_whole_table_in_order() {
        let rendered: Vec<&ActionRow> = tray_entries()
            .into_iter()
            .filter_map(|entry| match entry {
                TrayEntry::Action(row) => Some(row),
                TrayEntry::Separator => None,
            })
            .collect();
        assert_eq!(rendered.len(), ACTIONS.len());
        for (row, table) in rendered.iter().zip(ACTIONS) {
            assert_eq!(row.id, table.id, "tray must keep table order");
        }
        assert!(
            tray_entries().contains(&TrayEntry::Separator),
            "groups must be separated"
        );
    }

    #[test]
    fn cli_listing_render_the_whole_table() {
        let lines = cli_lines();
        assert_eq!(lines.len(), ACTIONS.len());
        let listing = cli_listing();
        assert_eq!(listing.lines().count(), ACTIONS.len());
        for row in ACTIONS {
            assert!(
                lines.iter().any(|line| line.starts_with(row.id)),
                "{} should appear in the listing",
                row.id
            );
            assert!(
                listing.contains(row.label),
                "{} should appear in the listing",
                row.label
            );
        }
    }

    #[test]
    fn adding_a_row_updates_all_three_renderers() {
        // The renderers are generic over the table, so one appended row shows
        // up in the panel, the tray, and the CLI listing without other edits.
        let extended = [
            ACTIONS[0],
            ActionRow {
                id: "new-action",
                label: "New action",
                shortcut: "Ctrl+N",
                action: MenuAction::Settings,
            },
        ];
        let panel = panel_rows_from(&extended);
        let cli = cli_lines_from(&extended);
        let tray = tray_entries_from(&extended);
        assert_eq!(panel.len(), 2);
        assert_eq!(panel[1].id, "new-action");
        assert_eq!(cli.len(), 2);
        assert!(cli[1].contains("New action"));
        let tray_actions = tray
            .iter()
            .filter_map(|entry| match entry {
                TrayEntry::Action(row) => Some(row.id),
                TrayEntry::Separator => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(tray_actions, vec!["all-in-one", "new-action"]);
    }

    #[test]
    fn capture_rows_route_to_capture_requests() {
        let pairs = [
            ("all-in-one", CaptureRequest::AllInOne),
            ("area", CaptureRequest::Area),
            ("window", CaptureRequest::Window),
            ("fullscreen", CaptureRequest::Display),
            ("desktop", CaptureRequest::AllDisplays),
            ("scroll", CaptureRequest::Scrolling),
            ("scroll-horizontal", CaptureRequest::HorizontalScrolling),
            ("previous", CaptureRequest::PreviousArea),
            ("timer", CaptureRequest::SelfTimer),
            ("ocr", CaptureRequest::Text),
        ];
        for (id, request) in pairs {
            let row = find(id).unwrap_or_else(|| panic!("{id} row"));
            assert_eq!(row.action, MenuAction::Capture(request));
            assert_eq!(invoke(row), Invocation::Capture(request));
            assert!(row.implemented());
        }
    }

    #[test]
    fn non_capture_rows_report_not_implemented() {
        let row = find("settings").expect("settings row");
        assert_eq!(invoke(row), Invocation::NotImplemented("settings"));
        assert!(!row.implemented());
    }

    #[test]
    fn keyboard_navigation_clamps_to_the_table() {
        let mut state = MenuState::default();
        assert_eq!(state.selected(), 0);
        assert_eq!(state.handle_key(MenuKey::K), MenuEvent::SelectionChanged(0));
        assert_eq!(state.handle_key(MenuKey::J), MenuEvent::SelectionChanged(1));
        assert_eq!(state.handle_key(MenuKey::L), MenuEvent::SelectionChanged(2));
        assert_eq!(state.handle_key(MenuKey::H), MenuEvent::SelectionChanged(1));
        state.select(1000);
        assert_eq!(state.selected(), ACTIONS.len() - 1);
        state.move_by(0, 1);
        assert_eq!(state.selected(), ACTIONS.len() - 1);
    }

    #[test]
    fn enter_activates_escape_dismisses_and_tab_steps() {
        let mut state = MenuState::default();
        assert_eq!(
            state.handle_key(MenuKey::Enter),
            MenuEvent::Activated(&ACTIONS[0])
        );
        assert_eq!(state.handle_key(MenuKey::Escape), MenuEvent::Dismissed);
        assert_eq!(state.handle_key(MenuKey::Tab), MenuEvent::Tabbed(1));
        assert_eq!(state.handle_key(MenuKey::Backtab), MenuEvent::Tabbed(-1));
    }

    #[test]
    fn qml_singleton_lists_every_row() {
        let menu = MenuRust::default();
        assert_eq!(menu.ids.len(), ACTIONS.len() as isize);
        assert_eq!(menu.labels.len(), ACTIONS.len() as isize);
        assert_eq!(menu.shortcuts.len(), ACTIONS.len() as isize);
        assert_eq!(menu.selected_index, 0);
        assert_eq!(
            menu.labels.get(0).map(ToString::to_string).as_deref(),
            Some(ACTIONS[0].label)
        );
        assert_eq!(
            menu.ids.get(0).map(ToString::to_string).as_deref(),
            Some(ACTIONS[0].id)
        );
    }
}
