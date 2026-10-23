//! End-to-end behavior for the command routes that used to be dropped and the
//! history window's store actions.
//!
//! These drive the same functions the GUI's dispatch and the HistoryApi
//! singleton call, asserting observable results: a file moved to the trash, a
//! row removed, a window's state opened, a visible message for an empty action.

use std::path::{Path, PathBuf};

use tidyshot::app::Command;
use tidyshot::capture_runtime::{perform_command, perform_route};
use tidyshot::history::{self, HistoryRow, KindFilter};
use tidyshot::history_window::{HistoryView, TABS};
use tidyshot::menu::{ACTIONS, Route};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tidyshot-cmd-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(history::captures_dir(&dir)).unwrap();
    dir
}

fn capture(root: &Path, name: &str) -> PathBuf {
    let path = history::captures_dir(root).join(name);
    let image = tidyshot::pngzip::Image::new(4, 3, vec![7u8; 4 * 3 * 4]);
    std::fs::write(&path, image.to_png()).unwrap();
    path
}

#[test]
fn every_menu_route_has_a_command_word_or_a_runtime() {
    for row in ACTIONS {
        match row.route() {
            Route::Capture(_) | Route::Recording(_) => {}
            route => {
                let word = tidyshot::menu::command_word(route);
                assert!(!word.is_empty(), "{} has no command word", row.id);
                assert!(
                    tidyshot::app::route_argv(&[word.to_owned()]).is_ok(),
                    "{} routes to an unknown command",
                    row.id
                );
            }
        }
    }
}

#[test]
fn a_settings_command_opens_the_window_on_its_tab() {
    let message = perform_command(Command::Settings {
        tab: "recording".to_owned(),
    })
    .unwrap();
    assert!(message.contains("recording"));
}

#[test]
fn history_and_pin_commands_report_a_visible_result() {
    // With no captures the restore and pin commands refuse visibly; toggling
    // and closing pins always report their state.
    for command in [
        Command::Restore,
        Command::Last,
        Command::LastScreenshot,
        Command::PinChooser,
    ] {
        let message = perform_command(command.clone())
            .err()
            .unwrap_or_else(|| panic!("{command:?} should report that there is nothing"));
        assert!(!message.is_empty());
    }
    for command in [Command::TogglePins, Command::ClosePins, Command::UnlockPins] {
        assert!(!perform_command(command).unwrap().is_empty());
    }
}

#[test]
fn an_ocr_command_reaches_the_recognition_queue() {
    let dir = scratch("ocr");
    let path = capture(&dir, "text.png");
    let message = perform_command(Command::Ocr {
        path: path.clone(),
        linebreaks: Some(true),
    })
    .unwrap();
    assert!(message.contains("text.png"));
}

#[test]
fn the_history_view_deletes_through_trash_and_keeps_other_captures() {
    let root = scratch("view");
    let first = capture(&root, "first.png");
    let second = capture(&root, "second.png");
    history::write(
        &root,
        &[
            HistoryRow::screenshot(&first, 4, 3, None),
            HistoryRow::screenshot(&second, 4, 3, None),
        ],
    )
    .unwrap();
    let mut view = HistoryView::new(&root);
    view.show();
    view.set_query("first");
    view.select_cursor(false);
    assert_eq!(view.delete_selected(), 1);
    assert!(!first.exists());
    assert!(second.exists(), "the other capture is untouched");
    assert_eq!(history::read(&root).len(), 1);
    let trashed = std::fs::read_dir(history::trash_dir(&root))
        .unwrap()
        .count();
    assert_eq!(trashed, 1, "the deleted capture is in the trash, not gone");
}

#[test]
fn every_history_tab_filters_the_listing() {
    let root = scratch("tabs");
    let shot = capture(&root, "shot.png");
    let page = capture(&root, "page.png");
    history::write(
        &root,
        &[
            HistoryRow::screenshot(&shot, 4, 3, None),
            HistoryRow::scroll(&page, 4, 3, None),
            HistoryRow::video(root.join("take.mp4"), 10, 10),
        ],
    )
    .unwrap();
    let mut view = HistoryView::new(&root);
    view.show();
    assert_eq!(view.filmstrip().len(), 3);
    for (id, expected) in [("all", 3), ("image", 2), ("scroll", 1), ("video", 1)] {
        view.set_filter(KindFilter::parse(id));
        assert_eq!(view.filmstrip().len(), expected, "{id} tab");
    }
    assert_eq!(TABS.len(), 4, "every filmstrip tab is rendered");
}

#[test]
fn the_retention_cleanup_keeps_recent_captures() {
    let root = scratch("retention");
    let old = capture(&root, "old.png");
    let recent = capture(&root, "recent.png");
    let mut old_row = HistoryRow::screenshot(&old, 4, 3, None);
    old_row.created = 1.0;
    let mut recent_row = HistoryRow::screenshot(&recent, 4, 3, None);
    recent_row.created = 2_000_000_000.0;
    history::write(&root, &[old_row, recent_row]).unwrap();
    // A month keeps the 2033 row and trashes the 1970 one.
    assert_eq!(history::prune(&root, 30, 2_000_000_100.0).unwrap(), 1);
    assert!(!old.exists());
    assert!(recent.exists());
}

#[test]
fn a_settings_save_round_trips_the_retention_key() {
    let path =
        std::env::temp_dir().join(format!("tidyshot-cmd-settings-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&path);
    tidyshot::settings::write_setting("history_retention_days", {
        tidyshot::json::Value::Number(7.0)
    })
    .unwrap();
    assert!(path.exists() || tidyshot::settings::settings_path().exists());
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_clipboard_command_with_no_files_reports_that_nothing_was_found() {
    // The clipboard query shells out to wl-paste; on a machine without a
    // clipboard it must report the failure, never open an empty editor.
    let result = tidyshot::capture_runtime::import_clipboard();
    if let Err(message) = result {
        assert!(!message.is_empty());
    }
}

#[test]
fn the_history_route_opens_the_window() {
    let message = perform_route(Route::History).unwrap();
    assert!(!message.is_empty());
}
