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
            "viewBox=\"0 0 {size} {size}\" role=\"img\" aria-label=\"TidyShot\">\n",
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
}
