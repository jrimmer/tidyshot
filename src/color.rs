//! Annotation color formatting and favorite swatches.
// Ported from omnishot/color_picker.py (color_text, favorite_colors) and the
// channel/alpha conversions the picker performs.
//!
//! Colors are RGBA8. Their display form is `#RRGGBB` while fully opaque and
//! `#AARRGGBB` otherwise, matching Qt's `HexRgb`/`HexArgb` name formats.

use std::collections::BTreeMap;

use crate::json::Value;

/// The built-in annotation swatches, in display order.
pub const SWATCHES: &[&str] = &[
    "#15171d", "#ff5b61", "#ff8a34", "#ffbc42", "#53d49b", "#26c6be", "#60a5fa", "#b48bfa",
    "#ef4b9b", "#ffffff",
];

/// An 8-bit RGBA color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

impl Rgba {
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    /// Parse `#rgb`, `#rrggbb` or `#aarrggbb` (the leading `#` optional).
    #[must_use]
    pub fn from_hex(value: &str) -> Option<Self> {
        let hex = value.strip_prefix('#').unwrap_or(value);
        let parse = |text: &str| u8::from_str_radix(text, 16).ok();
        match hex.len() {
            3 => {
                let digits: Vec<u8> = hex
                    .chars()
                    .map(|c| c.to_digit(16).map(|d| d as u8))
                    .collect::<Option<_>>()?;
                Some(Self::new(
                    digits[0] * 17,
                    digits[1] * 17,
                    digits[2] * 17,
                    255,
                ))
            }
            6 => Some(Self::new(
                parse(&hex[0..2])?,
                parse(&hex[2..4])?,
                parse(&hex[4..6])?,
                255,
            )),
            8 => Some(Self::new(
                parse(&hex[2..4])?,
                parse(&hex[4..6])?,
                parse(&hex[6..8])?,
                parse(&hex[0..2])?,
            )),
            _ => None,
        }
    }

    /// The channels as `[red, green, blue]`.
    #[must_use]
    pub const fn rgb(&self) -> [u8; 3] {
        [self.red, self.green, self.blue]
    }

    #[must_use]
    pub const fn is_opaque(&self) -> bool {
        self.alpha == 255
    }

    /// The 0-100 opacity shown by the picker's alpha control.
    #[must_use]
    pub fn opacity_percent(&self) -> u8 {
        ((u16::from(self.alpha) * 100 + 127) / 255) as u8
    }

    /// Build a color from an opacity percentage, keeping the RGB channels.
    #[must_use]
    pub fn with_opacity_percent(self, percent: u8) -> Self {
        let percent = percent.min(100);
        let alpha = ((u32::from(percent) * 255 + 50) / 100) as u8;
        Self { alpha, ..self }
    }
}

/// Render a color the way the picker's hex field and tooltips do.
// Ported from omnishot/color_picker.py (color_text)
#[must_use]
pub fn color_text(color: Rgba) -> String {
    if color.is_opaque() {
        format!("#{:02x}{:02x}{:02x}", color.red, color.green, color.blue)
    } else {
        format!(
            "#{:02x}{:02x}{:02x}{:02x}",
            color.alpha, color.red, color.green, color.blue
        )
    }
}

/// The user's favorite annotation colors, in first-seen order.
///
/// `custom_colors` wins; otherwise the palette entries that are not built-in
/// defaults are used. Invalid and duplicate entries are dropped.
// Ported from omnishot/color_picker.py (favorite_colors)
#[must_use]
pub fn favorite_colors(settings: &BTreeMap<String, Value>) -> Vec<String> {
    let palette_defaults: Vec<String> = SWATCHES.iter().map(|s| (*s).to_owned()).collect();
    let values: Vec<String> = match settings.get("custom_colors").and_then(Value::as_array) {
        Some(custom) => custom
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        None => settings
            .get("palette")
            .and_then(Value::as_array)
            .map(|palette| {
                palette
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|value| !palette_defaults.iter().any(|d| d == value))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
    };
    let mut seen: Vec<String> = Vec::new();
    for value in values {
        let Some(color) = Rgba::from_hex(&value) else {
            continue;
        };
        let text = color_text(color);
        if !seen.contains(&text) {
            seen.push(text);
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(entries: &[(&str, Value)]) -> BTreeMap<String, Value> {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    #[test]
    fn hex_text_is_rgb_only_when_opaque() {
        assert_eq!(color_text(Rgba::new(0x24, 0x68, 0xac, 255)), "#2468ac");
        assert_eq!(color_text(Rgba::new(0x24, 0x68, 0xac, 0x80)), "#802468ac");
    }

    #[test]
    fn parsing_accepts_short_and_alpha_forms() {
        assert_eq!(
            Rgba::from_hex("#b48bfa"),
            Some(Rgba::new(0xb4, 0x8b, 0xfa, 255))
        );
        assert_eq!(
            Rgba::from_hex("b48bfa"),
            Some(Rgba::new(0xb4, 0x8b, 0xfa, 255))
        );
        assert_eq!(
            Rgba::from_hex("#802468ac"),
            Some(Rgba::new(0x24, 0x68, 0xac, 0x80))
        );
        assert_eq!(Rgba::from_hex("#fff"), Some(Rgba::new(255, 255, 255, 255)));
        assert_eq!(Rgba::from_hex("#12345"), None);
        assert_eq!(Rgba::from_hex("nope"), None);
    }

    #[test]
    fn opacity_channel_round_trips_the_picker_value() {
        let color = Rgba::new(0x24, 0x68, 0xac, 255).with_opacity_percent(50);
        assert_eq!(color.alpha, 128);
        assert_eq!(color.opacity_percent(), 50);
    }

    #[test]
    fn favorites_dedupe_and_ignore_invalid_entries() {
        let adjusted = Rgba::new(0x24, 0x68, 0xac, 0x80);
        let stored = settings(&[(
            "custom_colors",
            Value::Array(vec![
                Value::String("#802468ac".into()),
                Value::String("#802468ac".into()),
                Value::String("not a color".into()),
            ]),
        )]);
        assert_eq!(favorite_colors(&stored), vec![color_text(adjusted)]);

        let palette = settings(&[(
            "palette",
            Value::Array(vec![
                Value::String("#ff5b61".into()),
                Value::String("#123456".into()),
                Value::String("#123456".into()),
            ]),
        )]);
        assert_eq!(favorite_colors(&palette), vec!["#123456".to_owned()]);
        assert!(favorite_colors(&BTreeMap::new()).is_empty());
    }
}
