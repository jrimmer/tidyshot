// tidyshot: the CLI dispatch and the GUI entry point.

/// Run a subcommand that must stay headless, before Qt is initialised.
///
/// Returns `Some(exit_code)` when the invocation was handled without starting
/// the GUI. These run before the single-instance socket so `doctor`, `compat`
/// and the action listing still work when the display or the running app is
/// what is broken.
// Ported from omnishot/app.py (main)
fn run_subcommand(args: &[String]) -> Option<i32> {
    dispatch(args, |tail| {
        let system = tidyshot::doctor::RealSystem::new();
        tidyshot::doctor::main(tail, &system)
    })
}

/// Route the headless subcommands, injecting the doctor handler so its
/// placement ahead of Qt can be asserted without touching a real machine.
fn dispatch<F>(args: &[String], doctor: F) -> Option<i32>
where
    F: FnOnce(&[String]) -> i32,
{
    match args.first().map(String::as_str) {
        Some("menu") => {
            print!("{}", tidyshot::menu::cli_listing());
            Some(0)
        }
        Some("doctor") => {
            // Runs without Qt or the single-instance socket, so it still works
            // when the display or the running app is what is broken.
            Some(doctor(&args[1..]))
        }
        Some("compat") => Some(tidyshot::compat::cli(&args[1..])),
        Some("ocr-models") => Some(install_ocr_models()),
        _ => None,
    }
}

/// Download the OCR language, OSD, and script models for offline recognition.
///
/// Upstream installs these from its settings UI; the headless entry keeps the
/// models reachable from setup and from a terminal on the pilot.
fn install_ocr_models() -> i32 {
    let fetcher = tidyshot::ocr::CommandFetcher::new(tidyshot::commands::SystemRunner);
    match tidyshot::ocr::install_models(&fetcher, &mut |done, total| {
        println!("installing OCR models {done}/{total}");
    }) {
        Ok(languages) => {
            println!("installed {languages} OCR language models");
            0
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

/// Validate an invocation before it reaches Qt or a running instance.
///
/// Returns `Some(exit_code)` for a command that cannot be routed, `None` when
/// the command is understood. A URL that fails to parse or names an action the
/// API refuses is rejected here, so the running instance is never handed a
/// command it would merely drop.
fn validate(args: &[String]) -> Option<i32> {
    let parsed = match tidyshot::cli::parse_args(args.iter().map(String::as_str)) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("tidyshot: {error}");
            return Some(2);
        }
    };
    match tidyshot::app::route(&parsed) {
        Ok(tidyshot::app::Command::Unknown(command)) => {
            eprintln!("tidyshot: unknown command: {command}");
            Some(2)
        }
        Ok(_) => None,
        Err(error) => {
            eprintln!("tidyshot: {error}");
            Some(2)
        }
    }
}

/// Hand `args` to a running instance when one is listening.
///
/// Returns `Some(0)` when the command was delivered. `None` means no instance
/// is running, so the caller becomes the server and starts the GUI. The
/// `forward` hook is injected so the delivery path can be asserted without a
/// real socket.
fn hand_off<F>(args: &[String], forward: F) -> Option<i32>
where
    F: FnOnce(&[String]) -> bool,
{
    if forward(args) { Some(0) } else { None }
}
/// Run a capture directly when no instance is running.
///
/// A capture that never reaches a window is the CLI's normal path: the
/// invocation resolves its geometry, grabs the pixels and prints the saved
/// file. Interactive `ocr` stays with the GUI because its review dialog is
/// what presents the result. `None` means the command needs the window.
fn direct_capture(args: &[String]) -> Option<i32> {
    let argv_command =
        tidyshot::app::route_and_resolve(args, &tidyshot::commands::SystemRunner).ok()?;
    match &argv_command {
        tidyshot::app::Command::Capture { command, .. } if command != "ocr" => {
            Some(tidyshot::capture_runtime::cli(&argv_command))
        }
        _ => None,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = run_subcommand(&args) {
        std::process::exit(code);
    }
    if let Some(code) = validate(&args) {
        std::process::exit(code);
    }
    let socket = tidyshot::instance::socket_path(&tidyshot::instance::state_dir());
    let delivered = hand_off(&args, |args| {
        matches!(
            tidyshot::instance::forward(args, &socket),
            Ok(tidyshot::instance::Forwarded::Delivered)
        )
    });
    if let Some(code) = delivered {
        std::process::exit(code);
    }
    if let Some(code) = direct_capture(&args) {
        std::process::exit(code);
    }
    // Qt reads its hardware-decoding variable when the media backend starts,
    // so the preview policy is applied before the GUI constructs QGuiApplication.
    tidyshot::video_decoder::configure_preview_decoder(&tidyshot::settings::stored_settings());
    tidyshot::run();
}

#[cfg(test)]
mod tests {
    use super::{direct_capture, dispatch, hand_off, run_subcommand, validate};

    #[test]
    fn menu_subcommand_lists_actions_and_stays_headless() {
        assert_eq!(run_subcommand(&["menu".to_owned()]), Some(0));
    }

    #[test]
    fn ocr_starts_the_gui_for_the_dialog() {
        assert!(run_subcommand(&["ocr".to_owned()]).is_none());
        assert!(run_subcommand(&["ocr".to_owned(), "/tmp/a.png".to_owned()]).is_none());
    }

    #[test]
    fn no_arguments_start_the_gui() {
        assert_eq!(run_subcommand(&[]), None);
        assert_eq!(run_subcommand(&["area".to_owned()]), None);
    }

    // Ported from omnishot/test_doctor.py (test_app_routes_doctor_before_starting_qt)
    #[test]
    fn doctor_routes_before_starting_qt() {
        let mut seen: Option<Vec<String>> = None;
        let code = dispatch(&["doctor".to_owned(), "--json".to_owned()], |tail| {
            seen = Some(tail.to_vec());
            7
        });
        assert_eq!(code, Some(7));
        assert_eq!(seen, Some(vec!["--json".to_owned()]));
    }

    #[test]
    fn every_documented_capture_subcommand_validates() {
        for command in [
            "all-in-one",
            "area",
            "area-annotate",
            "area-copy",
            "area-save",
            "area-pin",
            "scroll",
            "scroll-horizontal",
            "fullscreen",
            "window",
            "desktop",
            "previous",
            "timer",
            "ocr",
            "ocr-lines",
            "ocr-single-line",
            "open",
            "clipboard",
            "history",
            "restore",
            "last",
            "last-screenshot",
            "save-all",
            "close-all",
            "toggle-pins",
            "close-pins",
            "unlock-pins",
            "settings",
            "record",
            "stop",
            "pause",
            "restart",
            "scroll-done",
            "quit",
        ] {
            assert_eq!(validate(&[command.to_owned()]), None, "{command} rejected");
        }
    }

    #[test]
    fn an_unknown_command_is_rejected_before_qt_or_the_socket() {
        assert_eq!(validate(&["frobnicate".to_owned()]), Some(2));
    }

    #[test]
    fn a_disabled_or_malformed_url_is_rejected_before_the_socket() {
        assert_eq!(validate(&["https://example.com".to_owned()]), Some(2));
        assert_eq!(
            validate(&["omnishot://capture-area?action=upload".to_owned()]),
            Some(2)
        );
        assert_eq!(
            validate(&["omnishot://open-settings?tab=cloud".to_owned()]),
            Some(2)
        );
    }

    #[test]
    fn a_running_instance_receives_the_command_and_the_sender_exits() {
        let mut forwarded: Option<Vec<String>> = None;
        let code = hand_off(&["scroll-done".to_owned()], |args| {
            forwarded = Some(args.to_vec());
            true
        });
        assert_eq!(code, Some(0));
        assert_eq!(forwarded, Some(vec!["scroll-done".to_owned()]));
    }

    #[test]
    fn without_an_instance_the_sender_starts_the_gui() {
        assert_eq!(hand_off(&["area".to_owned()], |_| false), None);
    }

    #[test]
    fn only_a_capture_runs_directly_from_the_cli() {
        // Non-capture commands have no direct execution path; they still need
        // the window. The classification is what keeps a menu command from
        // exiting the process early.
        for command in ["menu", "history", "settings", "quit"] {
            assert_eq!(direct_capture(&[command.to_owned()]), None, "{command}");
        }
    }
}
