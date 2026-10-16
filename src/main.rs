// tidyshot: the CLI dispatch and the GUI entry point.

use std::path::Path;

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
        Some("ocr") => {
            let path = args.get(1).map(String::as_str).unwrap_or_default();
            Some(run_ocr(path))
        }
        _ => None,
    }
}

/// Recognize `path` and copy the result.
fn run_ocr(path: &str) -> i32 {
    if path.is_empty() {
        eprintln!("ocr needs an image path");
        return 1;
    }
    let runner = tidyshot::commands::SystemRunner;
    let qr = tidyshot::text_result::CommandQrReader::new(runner, "zbarimg");
    let settings = tidyshot::text_result::TextSettings::default();
    let clipboard = tidyshot::commands::SystemRunner;
    match tidyshot::text_result::recognize(&runner, &qr, Path::new(path), &settings, None) {
        Ok(tidyshot::text_result::Presentation::Copied) => 0,
        Ok(tidyshot::text_result::Presentation::Dialog(result)) => {
            let mut result = result;
            match result.copy(&clipboard) {
                Ok(()) => {
                    print!("{}", result.text());
                    0
                }
                Err(error) => {
                    eprintln!("{error}");
                    1
                }
            }
        }
        Ok(tidyshot::text_result::Presentation::Empty) => 0,
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
    fn ocr_without_a_path_fails_without_starting_the_gui() {
        assert_eq!(run_subcommand(&["ocr".to_owned()]), Some(1));
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
