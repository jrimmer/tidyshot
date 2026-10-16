// tidyshot: the CLI dispatch and the GUI entry point.

/// Run a subcommand that must stay headless, before Qt is initialised.
///
/// Returns `Some(exit_code)` when the invocation was handled without starting
/// the GUI.
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = run_subcommand(&args) {
        std::process::exit(code);
    }
    tidyshot::run();
}

#[cfg(test)]
mod tests {
    use super::{dispatch, run_subcommand};

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
}
