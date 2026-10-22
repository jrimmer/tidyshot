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
    let dir = scratch("settings");
    let path = dir.join("settings.json");
    tidyshot::settings::write_setting_at(
        &path,
        "history_retention_days",
        tidyshot::json::Value::Number(7.0),
    )
    .unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    let parsed = tidyshot::json::parse(&saved).unwrap();
    assert_eq!(
        parsed
            .get("history_retention_days")
            .and_then(|v| v.as_f64()),
        Some(7.0)
    );
    // A second key keeps the first, so saving preferences never drops one.
    tidyshot::settings::write_setting_at(
        &path,
        "ocr_linebreaks",
        tidyshot::json::Value::Bool(false),
    )
    .unwrap();
    let parsed = tidyshot::json::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        parsed
            .get("history_retention_days")
            .and_then(|v| v.as_f64()),
        Some(7.0)
    );
    let _ = std::fs::remove_dir_all(&dir);
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
fn a_clipboard_file_list_imports_the_file_it_names() {
    use tidyshot::commands::{CommandError, CommandOutput, CommandRunner, CommandSpec};

    struct Fake {
        uri_list: Vec<u8>,
    }
    impl CommandRunner for Fake {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            if spec.args.iter().any(|arg| arg == "text/uri-list") {
                Ok(CommandOutput {
                    status: 0,
                    stdout: self.uri_list.clone(),
                    stderr: Vec::new(),
                })
            } else {
                Ok(CommandOutput {
                    status: 1,
                    stdout: Vec::new(),
                    stderr: b"no type".to_vec(),
                })
            }
        }
    }

    let dir = scratch("clipboard-import");
    let image = capture(&dir, "pasted.png");
    let runner = Fake {
        uri_list: format!("file://{}\r\n# comment\r\n", image.display()).into_bytes(),
    };
    let message = tidyshot::capture_runtime::import_clipboard_with(&runner).unwrap();
    assert!(message.contains("pasted.png"), "{message}");
}

#[test]
fn a_clipboard_without_media_is_refused_rather_than_opening_an_empty_editor() {
    use tidyshot::commands::{CommandError, CommandOutput, CommandRunner, CommandSpec};

    struct Empty;
    impl CommandRunner for Empty {
        fn run(&self, _spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            Ok(CommandOutput {
                status: 1,
                stdout: Vec::new(),
                stderr: b"no type".to_vec(),
            })
        }
    }
    let message = tidyshot::capture_runtime::import_clipboard_with(&Empty).unwrap_err();
    assert!(message.contains("no supported"), "{message}");
}

#[test]
fn the_history_route_opens_the_window() {
    let message = perform_route(Route::History).unwrap();
    assert!(!message.is_empty());
}

#[test]
fn every_routed_command_reaches_a_handler() {
    // A handler either performs the action or reports why it cannot; none of
    // these returns "no handler" or an empty success.
    let dir = scratch("commands");
    let path = capture(&dir, "shot.png");
    let commands = [
        Command::Menu,
        Command::Edit(path.clone()),
        Command::EditChooser,
        Command::Clipboard,
        Command::Pin(path.clone()),
        Command::PinChooser,
        Command::Overlay(path.clone()),
        Command::Ocr {
            path: path.clone(),
            linebreaks: None,
        },
        Command::History,
        Command::Restore,
        Command::Last,
        Command::LastScreenshot,
        Command::SaveAll,
        Command::CloseAll,
        Command::TogglePins,
        Command::ClosePins,
        Command::UnlockPins,
        Command::Settings {
            tab: "general".to_owned(),
        },
        Command::Record {
            geometry: None,
            coordinates: None,
        },
        Command::Stop,
        Command::Pause,
        Command::Restart,
        Command::ScrollDone,
        Command::Quit,
    ];
    for command in commands {
        match perform_command(command.clone()) {
            Ok(message) => assert!(
                !message.is_empty() || matches!(command, Command::Menu),
                "{command:?} succeeded with no message"
            ),
            Err(message) => assert!(!message.is_empty(), "{command:?} failed silently"),
        }
    }
    // A capture and an unknown word are refused with a message, never dropped.
    for command in [
        Command::Capture {
            command: "area".to_owned(),
            geometry: None,
            coordinates: None,
            action: None,
            delay: None,
            linebreaks: None,
            start: None,
            autoscroll: None,
        },
        Command::Unknown("frobnicate".to_owned()),
    ] {
        let message = perform_command(command.clone()).unwrap_err();
        assert!(!message.is_empty(), "{command:?} failed silently");
    }
}

#[test]
fn opening_the_history_window_shows_the_saved_captures() {
    let root = scratch("open-window");
    let path = capture(&root, "visible.png");
    history::write(&root, &[HistoryRow::screenshot(&path, 4, 3, None)]).unwrap();
    // The shared view the GUI's HistoryApi drives can be pointed at any store;
    // opening it loads the rows the filmstrip renders.
    let mut view = HistoryView::new(&root);
    view.show();
    assert!(view.open());
    let rows = view.filmstrip();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "visible.png");
    assert!(rows[0].to_value().get("source").is_some());
    assert_eq!(view.open_target(), Some(path));
}

#[test]
fn search_filters_the_listing_before_delete() {
    let root = scratch("search-delete");
    let keep = capture(&root, "keep.png");
    let drop = capture(&root, "drop-me.png");
    history::write(
        &root,
        &[
            HistoryRow::screenshot(&keep, 4, 3, None),
            HistoryRow::screenshot(&drop, 4, 3, None),
        ],
    )
    .unwrap();
    let mut view = HistoryView::new(&root);
    view.show();
    view.set_query("drop");
    assert_eq!(view.filmstrip().len(), 1);
    view.select_cursor(false);
    assert_eq!(view.delete_selected(), 1);
    assert!(!drop.exists(), "the searched capture was trashed");
    assert!(keep.exists(), "the filtered-out capture was kept");
    view.set_query("");
    assert_eq!(view.filmstrip().len(), 1);
}
