//! Motion blur shutter timing for editable cursor and framing motion.
// Ported from omnishot/motion_blur.py
//!
//! The shutter samples several instants inside one frame and averages them.
//! Sampling stays inside the current shot: a cut resets the window, so seeking
//! or a cut boundary can never pull pixels from a different shot.

use std::collections::BTreeMap;

use crate::json::Value;

/// The offered motion blur levels, matching the editor's slider.
pub const LEVELS: &[&str] = &["None", "Low", "Medium", "High"];

/// The blur level for an options object.
///
/// A legacy `blur` boolean maps to `Medium`; an unset value is `None`.
// Ported from omnishot/motion_blur.py (blur_level)
#[must_use]
pub fn blur_level(options: &BTreeMap<String, Value>) -> u32 {
    let value = match options.get("motion_blur") {
        Some(value) => value,
        None => {
            if options.get("blur").is_some_and(Value::is_truthy) {
                return 2;
            }
            return 0;
        }
    };
    let number = match value {
        Value::Number(n) if n.is_finite() => *n,
        Value::Bool(true) => 2.0,
        Value::Bool(false) => 0.0,
        _ => return 0,
    };
    number.clamp(0.0, 3.0) as u32
}

/// Every shutter instant for displaying `t` seconds.
///
/// A cut containing `t` returns just `t`; a `start` after the window's start
/// clamps the window forward.
// Ported from omnishot/motion_blur.py (shutter_times)
#[must_use]
pub fn shutter_times(t: f64, options: &BTreeMap<String, Value>) -> Vec<f64> {
    let level = blur_level(options);
    if level == 0 {
        return vec![t];
    }
    let mut lower = options.get("start").and_then(Value::as_f64).unwrap_or(0.0);
    if !lower.is_finite() {
        lower = 0.0;
    }
    lower = lower.max(0.0);

    if let Some(cuts) = options.get("cuts").and_then(Value::as_array) {
        for cut in cuts {
            let Some(pair) = cut.as_array() else { continue };
            let (Some(a), Some(b)) = (
                pair.first().and_then(Value::as_f64),
                pair.get(1).and_then(Value::as_f64),
            ) else {
                continue;
            };
            if b <= t {
                lower = lower.max(b);
            } else if a <= t && t < b {
                return vec![t];
            }
        }
    }

    let fps = options
        .get("fps")
        .and_then(Value::as_f64)
        .filter(|f| f.is_finite())
        .unwrap_or(30.0)
        .max(1.0);
    let speed = options
        .get("speed")
        .and_then(Value::as_f64)
        .filter(|s| s.is_finite())
        .unwrap_or(1.0)
        .max(0.01);
    let begin = lower.max(t - f64::from(level) / 3.0 * speed / fps);
    if begin >= t {
        return vec![t];
    }
    (0..9)
        .map(|index| begin + (t - begin) * f64::from(index) / 8.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(entries: &[(&str, Value)]) -> BTreeMap<String, Value> {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    #[test]
    fn legacy_blur_boolean_maps_to_medium() {
        assert_eq!(blur_level(&options(&[("blur", Value::Bool(true))])), 2);
        assert_eq!(blur_level(&options(&[("blur", Value::Bool(false))])), 0);
        // Any truthy legacy value means Medium, not just Bool(true).
        assert_eq!(blur_level(&options(&[("blur", Value::Number(1.0))])), 2);
        assert_eq!(blur_level(&options(&[("blur", Value::Number(0.0))])), 0);
        assert_eq!(
            blur_level(&options(&[("blur", Value::String("yes".into()))])),
            2
        );
        assert_eq!(blur_level(&BTreeMap::new()), 0);
    }

    #[test]
    fn levels_clamp_and_reject_non_numeric_values() {
        assert_eq!(
            blur_level(&options(&[("motion_blur", Value::Number(3.0))])),
            3
        );
        assert_eq!(
            blur_level(&options(&[("motion_blur", Value::Number(9.0))])),
            3
        );
        assert_eq!(
            blur_level(&options(&[("motion_blur", Value::Number(-2.0))])),
            0
        );
        assert_eq!(
            blur_level(&options(&[("motion_blur", Value::String("High".into()))])),
            0
        );
    }

    #[test]
    fn low_level_returns_a_single_instant() {
        let times = shutter_times(0.5, &options(&[("motion_blur", Value::Number(0.0))]));
        assert_eq!(times, vec![0.5]);
    }

    #[test]
    fn blur_increases_the_window_and_respects_speed_and_start() {
        let base = options(&[
            ("motion_blur", Value::Number(3.0)),
            ("fps", Value::Number(10.0)),
            ("speed", Value::Number(2.0)),
        ]);
        let times = shutter_times(0.6, &base);
        assert_eq!(times.len(), 9);
        assert!((times[0] - 0.4).abs() < 1e-9, "{times:?}");
        assert!((times[8] - 0.6).abs() < 1e-9);

        let clamped = options(&[
            ("motion_blur", Value::Number(3.0)),
            ("fps", Value::Number(10.0)),
            ("speed", Value::Number(2.0)),
            ("start", Value::Number(0.55)),
        ]);
        assert!((shutter_times(0.6, &clamped)[0] - 0.55).abs() < 1e-9);
    }

    #[test]
    fn a_cut_containing_the_frame_yields_no_blur() {
        let cuts = Value::Array(vec![Value::Array(vec![
            Value::Number(0.2),
            Value::Number(0.5),
        ])]);
        let inside = options(&[("motion_blur", Value::Number(3.0)), ("cuts", cuts.clone())]);
        assert_eq!(shutter_times(0.5, &inside), vec![0.5]);

        let after = options(&[("motion_blur", Value::Number(3.0)), ("cuts", cuts)]);
        let times = shutter_times(0.6, &after);
        // The cut that ended at 0.5 raises the window floor.
        assert!(times[0] >= 0.5 - 1e-9, "{times:?}");
    }
}
