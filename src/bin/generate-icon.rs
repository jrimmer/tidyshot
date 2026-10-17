//! Rewrite the TidyShot icon from the live Omarchy accent.
//!
//! Usage: `generate-icon [output]`. The output defaults to the installed
//! icon under the XDG data dir, so the theme hook can call it with no
//! arguments. The accent comes from [`tidyshot::ui_scale::accent`], the same
//! parser the UI scale uses, so the icon never invents its own theme logic.

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args_os().skip(1);
    let output = args
        .next()
        .map_or_else(tidyshot::icon::default_path, PathBuf::from);
    let accent = tidyshot::ui_scale::accent();
    if let Some(parent) = output.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("tidyshot: cannot create {}: {error}", parent.display());
        std::process::exit(1);
    }
    match tidyshot::icon::write(&output, &accent) {
        Ok(()) => println!("wrote {} in {accent}", output.display()),
        Err(error) => {
            eprintln!("tidyshot: cannot write {}: {error}", output.display());
            std::process::exit(1);
        }
    }
}
