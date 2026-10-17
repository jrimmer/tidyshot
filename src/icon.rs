//! The TidyShot app icon, rendered from the live Omarchy accent.
//!
//! The icon is a 64-unit rounded square in the theme accent carrying a camera
//! mark whose aperture blades are cut from the tile. It is regenerated when
//! the theme changes so the desktop entry matches the running shell.

use std::io;
use std::path::Path;

use crate::storage;

/// The side of the icon's square viewBox, in SVG units.
pub const SIZE: u32 = 64;
/// The tile corner radius; the spec fixes it at 14 of 64.
pub const RADIUS: u32 = 14;
/// The mark drawn on the tile, dark enough to read on any accent.
pub const MARK: &str = "#1e1e1e";

/// Render the icon in `accent` (a `#RRGGBB` string).
///
/// The accent is normalized through the shared color parser first, so a value
/// that is not a color falls back instead of landing in the SVG unescaped.
#[must_use]
pub fn render(accent: &str) -> String {
    let tile = crate::color::Rgba::from_hex(accent)
        .map(|color| format!("#{:02X}{:02X}{:02X}", color.red, color.green, color.blue))
        .unwrap_or_else(|| crate::ui_scale::DEFAULT_ACCENT.to_owned());
    format!(
        concat!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{size}\" height=\"{size}\" ",
            "viewBox=\"0 0 {size} {size}\" role=\"img\" aria-label=\"TidyShot\" ",
            "data-accent=\"{tile}\">\n",
            "  <rect width=\"{size}\" height=\"{size}\" rx=\"{radius}\" fill=\"{tile}\"/>\n",
            "  <g fill=\"none\" stroke=\"{mark}\" stroke-width=\"3.5\" ",
            "stroke-linejoin=\"round\" stroke-linecap=\"round\">\n",
            "    <circle cx=\"32\" cy=\"32\" r=\"15\"/>\n",
            "    <path d=\"M42 32 37 40.66 27 40.66 22 32 27 23.34 37 23.34Z\"/>\n",
            "  </g>\n",
            "</svg>\n",
        ),
        size = SIZE,
        radius = RADIUS,
        tile = tile,
        mark = MARK,
    )
}

/// Atomically write the icon in `accent` to `destination`.
pub fn write(destination: &Path, accent: &str) -> io::Result<()> {
    storage::atomic_write_file(destination, render(accent).as_bytes())
}

/// The installed icon path, under `$XDG_DATA_HOME` (or `~/.local/share`).
///
/// The desktop entry names the icon by basename, so the SVGs live in the
/// hicolor scalable theme where the XDG icon lookup finds them.
#[must_use]
pub fn default_path() -> std::path::PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map_or_else(|| std::path::PathBuf::from("."), std::path::PathBuf::from);
            home.join(".local/share")
        });
    data.join("icons/hicolor/scalable/apps/tidyshot.svg")
}

/// The system-wide installed icon, owned by the native package.
///
/// Startup reads this copy first because a package install is the copy the
/// desktop entry lookup resolves when the user has none of their own.
#[must_use]
pub fn system_path() -> std::path::PathBuf {
    std::path::PathBuf::from("/usr/share/icons/hicolor/scalable/apps/tidyshot.svg")
}

/// The raw `data-accent` attribute of an SVG, if it carries one.
///
/// The value is returned as written, so a caller can tell a corrupt value
/// from an absent attribute; a file without the attribute has no accent to
/// compare and must be left alone.
#[must_use]
pub fn data_accent(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let (_, rest) = text.split_once("data-accent=\"")?;
    let (value, _) = rest.split_once('"')?;
    Some(value.to_owned())
}

/// Rewrite `user` when the installed icon was generated for another accent.
///
/// `system` is preferred for reading and may live under `/usr`; only `user`
/// is ever written. A `None` accent means no theme was read, so nothing is
/// written. An icon without a `data-accent` marker is not ours to replace and
/// round-trips unchanged. Returns whether the user icon was written.
pub fn refresh_installed(system: &Path, user: &Path, accent: Option<&str>) -> io::Result<bool> {
    let Some(accent) = accent else {
        return Ok(false);
    };
    let installed = if system.is_file() { system } else { user };
    let Some(existing) = data_accent(installed) else {
        return Ok(false);
    };
    if normalize(&existing) == normalize(accent) {
        return Ok(false);
    }
    if let Some(parent) = user.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write(user, accent)?;
    Ok(true)
}

/// Normalize a color string to the `#RRGGBB` form [`render`] writes.
fn normalize(value: &str) -> Option<String> {
    crate::color::Rgba::from_hex(value)
        .map(|color| format!("#{:02X}{:02X}{:02X}", color.red, color.green, color.blue))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba;
    use crate::ui_scale::DEFAULT_ACCENT;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tidyshot-icon-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn render_draws_the_requested_accent_on_a_rounded_square() {
        let svg = render("#123456");
        assert!(svg.contains("viewBox=\"0 0 64 64\""));
        assert!(svg.contains("rx=\"14\""));
        assert!(svg.contains("fill=\"#123456\""));
        assert!(svg.contains("stroke=\"#1e1e1e\""));
    }

    #[test]
    fn render_substitutes_the_accent_without_touching_the_mark() {
        let first = render("#FFD60A");
        let second = render("#89b4fa");
        assert_ne!(first, second);
        assert!(first.contains("#FFD60A"));
        assert!(second.contains("#89B4FA"));
        assert_eq!(
            first.matches("stroke=\"#1e1e1e\"").count(),
            second.matches("stroke=\"#1e1e1e\"").count()
        );
    }

    #[test]
    fn render_falls_back_when_the_accent_is_not_a_color() {
        assert!(render("not-a-color").contains(DEFAULT_ACCENT));
        // The fallback keeps a literal value out of the attribute.
        assert!(!render("\"><script>").contains("<script>"));
    }

    #[test]
    fn write_replaces_the_destination_in_one_step() {
        let dir = scratch("write");
        let target = dir.join("tidyshot.svg");
        std::fs::write(&target, b"stale").unwrap();
        write(&target, "#FFD60A").unwrap();
        let written = std::fs::read_to_string(&target).unwrap();
        assert_eq!(written, render("#FFD60A"));
        // No partial temp file is left beside the target.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name != "tidyshot.svg")
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn the_accent_parser_round_trips_every_icon() {
        // The icon's own fallback and the parser's default agree, so a theme
        // that names no accent still matches `ui_scale`.
        assert_eq!(
            Rgba::from_hex(DEFAULT_ACCENT).unwrap().rgb(),
            [0xFF, 0xD6, 0x0A]
        );
    }

    #[test]
    fn a_different_accent_rewrites_the_user_icon() {
        let dir = scratch("refresh-differs");
        let system = dir.join("system.svg");
        let user = dir.join("user.svg");
        std::fs::write(&system, render("#89b4fa")).unwrap();
        std::fs::write(&user, render("#89b4fa")).unwrap();
        assert!(refresh_installed(&system, &user, Some("#FFD60A")).unwrap());
        // Only the user copy changes; the system copy is never touched.
        assert_eq!(std::fs::read_to_string(&user).unwrap(), render("#FFD60A"));
        assert_eq!(std::fs::read_to_string(&system).unwrap(), render("#89b4fa"));
    }

    #[test]
    fn a_matching_accent_writes_nothing() {
        let dir = scratch("refresh-same");
        let system = dir.join("system.svg");
        let user = dir.join("user.svg");
        std::fs::write(&system, render("#89B4FA")).unwrap();
        std::fs::write(&user, render("#000000")).unwrap();
        assert!(!refresh_installed(&system, &user, Some("#89b4fa")).unwrap());
        assert_eq!(std::fs::read_to_string(&user).unwrap(), render("#000000"));
    }

    #[test]
    fn no_theme_writes_nothing() {
        let dir = scratch("refresh-notheme");
        let system = dir.join("system.svg");
        let user = dir.join("user.svg");
        std::fs::write(&system, render("#89b4fa")).unwrap();
        std::fs::write(&user, render("#000000")).unwrap();
        assert!(!refresh_installed(&system, &user, None).unwrap());
        assert_eq!(std::fs::read_to_string(&user).unwrap(), render("#000000"));
    }

    #[test]
    fn a_corrupt_accent_is_treated_as_different() {
        let dir = scratch("refresh-corrupt");
        let system = dir.join("system.svg");
        let user = dir.join("user.svg");
        let corrupt =
            render("#89b4fa").replace("data-accent=\"#89B4FA\"", "data-accent=\"nonsense\"");
        std::fs::write(&system, &corrupt).unwrap();
        std::fs::write(&user, &corrupt).unwrap();
        assert!(refresh_installed(&system, &user, Some("#FFD60A")).unwrap());
        assert_eq!(std::fs::read_to_string(&user).unwrap(), render("#FFD60A"));
    }

    #[test]
    fn content_without_an_accent_round_trips() {
        let dir = scratch("refresh-plain");
        let system = dir.join("system.svg");
        let user = dir.join("user.svg");
        let plain = "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect/></svg>\n";
        std::fs::write(&system, plain).unwrap();
        std::fs::write(&user, plain).unwrap();
        assert!(!refresh_installed(&system, &user, Some("#FFD60A")).unwrap());
        assert_eq!(std::fs::read_to_string(&user).unwrap(), plain);
    }
}
