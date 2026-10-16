//! Text presets and the font family each one requests.
// Ported from omnishot/text_styles.py.
//!
//! The Qt renderer resolves the family through the font database. This module
//! keeps the preset names, their families and the fallback rule in one place so
//! the model and the renderer agree on which preset is active.

/// The seven presets, in toolbar order.
pub const STYLES: &[&str] = &[
    "Standard",
    "Rounded",
    "Monospaced",
    "Outlined",
    "Boxed",
    "Rounded Boxed",
    "Monospaced Boxed",
];

/// True when `style` is one of the seven presets.
#[must_use]
pub fn is_preset(style: &str) -> bool {
    STYLES.contains(&style)
}

/// The font family a preset prefers.
///
/// Rounded presets use the bundled Nunito; monospaced presets prefer Adwaita
/// Mono and fall back to the system fixed font; everything else prefers
/// Adwaita Sans and falls back to the general system font.
// Ported from omnishot/text_styles.py (family)
#[must_use]
pub fn family(style: &str) -> &'static str {
    match style {
        "Rounded" | "Rounded Boxed" => "Nunito",
        "Monospaced" | "Monospaced Boxed" => "Adwaita Mono",
        "Standard" | "Outlined" | "Boxed" => "Adwaita Sans",
        _ => "Adwaita Sans",
    }
}

/// The system fallback family for a preset.
#[must_use]
pub fn fallback_family(style: &str) -> &'static str {
    if matches!(style, "Monospaced" | "Monospaced Boxed") {
        "monospace"
    } else {
        "sans-serif"
    }
}

/// A legacy or custom style's display name, matching the toolbar label.
///
/// A style outside the presets is shown as `Title Case (legacy)`.
// Ported from omnishot/text_styles.py (StyleChoice.set_style)
#[must_use]
pub fn legacy_label(style: &str) -> String {
    let title: String = style
        .split_whitespace()
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("{title} (legacy)")
}

/// Whether `style` paints the text with a box behind it.
#[must_use]
pub fn is_boxed(style: &str) -> bool {
    matches!(style, "Boxed" | "Rounded Boxed" | "Monospaced Boxed")
}

/// Whether `style` outlines the glyphs instead of filling them.
#[must_use]
pub fn is_outlined(style: &str) -> bool {
    style == "Outlined"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seven_presets_are_stable() {
        assert_eq!(STYLES.len(), 7);
        assert!(is_preset("Rounded Boxed"));
        assert!(!is_preset("outline"));
    }

    #[test]
    fn families_match_the_source() {
        assert_eq!(family("Rounded"), "Nunito");
        assert_eq!(family("Monospaced Boxed"), "Adwaita Mono");
        assert_eq!(family("Standard"), "Adwaita Sans");
        assert_eq!(fallback_family("Monospaced"), "monospace");
        assert_eq!(fallback_family("Standard"), "sans-serif");
    }

    #[test]
    fn legacy_labels_are_title_cased() {
        assert_eq!(legacy_label("outline"), "Outline (legacy)");
        assert_eq!(legacy_label("rounded boxed"), "Rounded Boxed (legacy)");
    }

    #[test]
    fn boxed_and_outlined_classification() {
        assert!(is_boxed("Rounded Boxed"));
        assert!(!is_boxed("Rounded"));
        assert!(is_outlined("Outlined"));
        assert!(!is_outlined("Standard"));
    }
}
