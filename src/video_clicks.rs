//! Independent cursor press feedback, driven by editable source-time clicks.
// Ported from omnishot/video_clicks.py
//!
//! A brief compression keeps the pointer tip fixed while signaling a click.
//! Existing recordings contain click instants, not button-hold intervals, and
//! overlapping clicks share a bounded pulse without compounding shrinkage.

use crate::json::Value;

/// The scale multiplier for the cursor at `t`, `1.0` when no click is active.
// Ported from omnishot/video_clicks.py (press_scale)
#[must_use]
pub fn press_scale(events: &[Value], t: f64) -> f64 {
    let mut maximum = 0.0_f64;
    for event in events {
        let Some(object) = event.as_object() else {
            continue;
        };
        if object.get("kind").and_then(Value::as_str) != Some("click") {
            continue;
        }
        if object.get("hidden").is_some_and(Value::is_truthy) {
            continue;
        }
        let x = object.get("x").and_then(Value::as_f64).unwrap_or(0.5);
        let y = object.get("y").and_then(Value::as_f64).unwrap_or(0.5);
        if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
            continue;
        }
        let Some(click) = object.get("t").and_then(Value::as_f64) else {
            continue;
        };
        let age = t - click;
        if (0.0..0.24).contains(&age) {
            maximum = maximum.max((std::f64::consts::PI * age / 0.24).sin());
        }
    }
    1.0 - 0.18 * maximum
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn event(kind: &str, t: f64, extra: &[(&str, Value)]) -> Value {
        let mut object = BTreeMap::new();
        object.insert("kind".to_owned(), Value::String(kind.to_owned()));
        object.insert("t".to_owned(), Value::Number(t));
        for (key, value) in extra {
            object.insert((*key).to_owned(), value.clone());
        }
        Value::Object(object)
    }

    #[test]
    fn a_click_pulses_and_overlapping_clicks_stay_bounded() {
        let click = event(
            "click",
            1.0,
            &[("x", Value::Number(0.5)), ("y", Value::Number(0.5))],
        );
        let baseline = press_scale(&[], 1.0);
        assert_eq!(baseline, 1.0);
        assert!(press_scale(std::slice::from_ref(&click), 1.12) < 1.0);
        assert_eq!(press_scale(std::slice::from_ref(&click), 1.5), 1.0);
        // Two clicks at nearly the same instant do not compound.
        let first = press_scale(std::slice::from_ref(&click), 1.06);
        let second = press_scale(&[click.clone(), event("click", 1.02, &[])], 1.06);
        assert!((first - second).abs() < 1e-9);
    }

    #[test]
    fn hidden_and_out_of_bounds_clicks_are_ignored() {
        let hidden = event(
            "click",
            1.0,
            &[
                ("x", Value::Number(0.5)),
                ("y", Value::Number(0.5)),
                ("hidden", Value::Bool(true)),
            ],
        );
        assert_eq!(press_scale(&[hidden], 1.05), 1.0);
        let outside = event(
            "click",
            1.0,
            &[("x", Value::Number(2.0)), ("y", Value::Number(0.5))],
        );
        assert_eq!(press_scale(&[outside], 1.05), 1.0);
    }
}
