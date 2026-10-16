// tidyshot: the CLI dispatch and the GUI entry point.

/// Run `tidyshot menu` from the command line, printing the action listing.
///
/// Returns `true` when the invocation was handled without starting the GUI.
fn run_subcommand(args: &[String]) -> bool {
    match args.first().map(String::as_str) {
        Some("menu") => {
            print!("{}", tidyshot::menu::cli_listing());
            true
        }
        _ => false,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if run_subcommand(&args) {
        return;
    }
    tidyshot::run();
}

#[cfg(test)]
mod tests {
    use super::run_subcommand;

    #[test]
    fn menu_subcommand_lists_actions_and_stays_headless() {
        let handled = run_subcommand(&["menu".to_owned()]);
        assert!(handled);
    }

    #[test]
    fn no_arguments_start_the_gui() {
        assert!(!run_subcommand(&[]));
        assert!(!run_subcommand(&["area".to_owned()]));
    }
}
