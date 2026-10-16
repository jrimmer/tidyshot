//! Recording editor layout helpers: transport stepping and export profiles.
// Ported from omnishot/video_ui.py
//!
//! The QML layer owns the widgets; the pure rules that decide a frame step,
//! a transport timestamp and which export profile to restore live here.

use std::collections::BTreeMap;

use crate::json::Value;

/// The tools in the editor's navigation rail, in order.
pub const TOOLS: &[&str] = &[
    "Cursor",
    "Keystrokes",
    "Audio",
    "Camera",
    "Background",
    "Motion",
    "Trim",
    "Export",
];

/// The widths the export size chooser offers.
pub const SIZES: &[&str] = &[
    "Original", "1920", "1280", "960", "800", "640", "600", "400",
];
/// The export formats.
pub const FORMATS: &[&str] = &["MP4", "GIF"];
/// The playback speeds.
pub const SPEEDS: &[&str] = &["0.5", "1", "1.5", "2"];
/// The aspect ratios the background panel offers.
pub const ASPECTS: &[&str] = &[
    "Original", "16:9", "1:1", "9:16", "4:3", "3:2", "4:5", "5:4", "21:9",
];

/// The frame duration in milliseconds for a reported frame rate.
#[must_use]
pub fn frame_duration_ms(fps: Option<f64>) -> f64 {
    let rate = fps
        .filter(|rate| rate.is_finite() && *rate > 0.0)
        .unwrap_or(30.0);
    1000.0 / rate
}

/// The next playback position for a one-frame step.
///
/// `direction` is `-1` or `1`; the result is clamped to `0..=duration`.
// Ported from omnishot/video_ui.py (step_frame)
#[must_use]
pub fn step_frame(position_ms: f64, duration_ms: f64, fps: Option<f64>, direction: i32) -> f64 {
    let step = frame_duration_ms(fps) * f64::from(direction);
    (position_ms + step).clamp(0.0, duration_ms)
}

/// A `mm:ss` transport timestamp.
// Ported from omnishot/video_ui.py (update_transport stamp)
#[must_use]
pub fn stamp(milliseconds: f64) -> String {
    let seconds = ((milliseconds / 1000.0).max(0.0)) as u64;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

/// The transport label, `position / duration`.
#[must_use]
pub fn transport_label(position_ms: f64, duration_ms: f64) -> String {
    format!("{} / {}", stamp(position_ms), stamp(duration_ms))
}

/// The default export profile per format: `(fps, width)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportProfiles {
    profiles: BTreeMap<String, (i64, String)>,
    current: String,
}

impl ExportProfiles {
    /// Build the profiles from the stored settings for the file being opened.
    #[must_use]
    pub fn new(is_gif: bool, fps: i64, gif_fps: i64, gif_width: i64) -> Self {
        let gif_width = if gif_width == 0 {
            "Original".to_owned()
        } else {
            gif_width.to_string()
        };
        let mut profiles = BTreeMap::new();
        profiles.insert("MP4".to_owned(), (fps, "Original".to_owned()));
        profiles.insert("GIF".to_owned(), (gif_fps, gif_width));
        let current = if is_gif { "GIF" } else { "MP4" }.to_owned();
        Self { profiles, current }
    }

    /// The current format.
    #[must_use]
    pub fn current(&self) -> &str {
        &self.current
    }

    /// The current profile's `(fps, width)`.
    #[must_use]
    pub fn current_profile(&self) -> (i64, String) {
        self.profiles
            .get(&self.current)
            .cloned()
            .unwrap_or((30, "Original".to_owned()))
    }

    /// Switch format, remembering the controls' current values in the old
    /// profile and returning the new profile to apply.
    ///
    /// `fps` and `width` are the live control values.
    // Ported from omnishot/video_ui.py (format_changed)
    pub fn switch(&mut self, value: &str, fps: i64, width: &str) -> (i64, String) {
        if FORMATS.contains(&value) {
            self.profiles
                .insert(self.current.clone(), (fps, width.to_owned()));
            self.current = value.to_owned();
        }
        self.current_profile()
    }
}

/// Whether the GIF quality controls are relevant for a format.
#[must_use]
pub fn gif_controls_visible(format: &str) -> bool {
    format == "GIF"
}

/// The `width` option value for the size chooser.
#[must_use]
pub fn width_value(size: &str) -> i64 {
    if size == "Original" {
        0
    } else {
        size.parse().unwrap_or(0)
    }
}

/// The size label for a stored `width` option.
#[must_use]
pub fn size_label(width: i64) -> String {
    if width == 0 {
        "Original".to_owned()
    } else {
        width.to_string()
    }
}

/// The speed label for a stored numeric speed.
#[must_use]
pub fn speed_label(speed: f64) -> String {
    let mut text = format!("{speed}");
    if let Some(stripped) = text.strip_suffix(".0") {
        text = stripped.to_owned();
    }
    text
}

/// Whether a background colour string is a `#RRGGBB` value.
#[must_use]
pub fn valid_background(value: &str) -> bool {
    value.len() == 7 && value.starts_with('#') && value[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// The editor's default status line.
pub const DEFAULT_STATUS: &str = "Drag the trim handles or choose a tool to edit your recording.";

/// Read a speed option, tolerating a string or number.
#[must_use]
pub fn speed_value(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Number(number)) => *number,
        Some(Value::String(text)) => text.parse().unwrap_or(1.0),
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_stepping_uses_the_reported_rate_and_clamps() {
        assert_eq!(
            step_frame(1000.0, 5000.0, Some(30.0), 1),
            1000.0 + 1000.0 / 30.0
        );
        assert_eq!(step_frame(1000.0, 5000.0, None, -1), 1000.0 - 1000.0 / 30.0);
        assert_eq!(step_frame(10.0, 5000.0, Some(30.0), -1), 0.0);
        assert_eq!(step_frame(4990.0, 5000.0, Some(30.0), 1), 5000.0);
        assert_eq!(frame_duration_ms(Some(0.0)), 1000.0 / 30.0);
    }

    #[test]
    fn transport_label_pads_to_minutes_and_seconds() {
        assert_eq!(stamp(0.0), "00:00");
        assert_eq!(stamp(65_000.0), "01:05");
        assert_eq!(transport_label(1000.0, 120_000.0), "00:01 / 02:00");
        assert_eq!(stamp(-500.0), "00:00");
    }

    #[test]
    fn export_profiles_remember_the_previous_format_controls() {
        let mut profiles = ExportProfiles::new(false, 30, 15, 800);
        assert_eq!(profiles.current(), "MP4");
        assert_eq!(profiles.current_profile(), (30, "Original".to_owned()));
        // Switch to GIF after changing the live controls; MP4 remembers them.
        let (fps, width) = profiles.switch("GIF", 24, "1280");
        assert_eq!((fps, width.as_str()), (15, "800"));
        let (fps, width) = profiles.switch("MP4", 15, "800");
        assert_eq!((fps, width.as_str()), (24, "1280"));
    }

    #[test]
    fn option_conversions_round_trip() {
        assert_eq!(width_value("Original"), 0);
        assert_eq!(width_value("1280"), 1280);
        assert_eq!(size_label(0), "Original");
        assert_eq!(size_label(800), "800");
        assert_eq!(speed_label(1.0), "1");
        assert_eq!(speed_label(0.5), "0.5");
        assert!(valid_background("#25324b"));
        assert!(!valid_background("25324b"));
        assert!(!valid_background("#gggggg"));
        assert_eq!(speed_value(Some(&Value::String("1.5".to_owned()))), 1.5);
        assert_eq!(speed_value(None), 1.0);
        assert!(gif_controls_visible("GIF"));
        assert!(!gif_controls_visible("MP4"));
    }

    #[test]
    fn the_tool_rail_matches_the_source() {
        assert_eq!(
            TOOLS,
            &[
                "Cursor",
                "Keystrokes",
                "Audio",
                "Camera",
                "Background",
                "Motion",
                "Trim",
                "Export"
            ]
        );
    }
}
