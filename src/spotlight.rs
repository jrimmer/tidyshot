//! Spotlight aperture defaults and dimming.
// Ported from omnishot/spotlight.py.
//!
//! The aperture geometry lives in [`crate::annotation::spotlight`]; this
//! module owns the control defaults and the dimming percentage shown in the
//! toolbar.

use std::collections::BTreeMap;

use crate::json::Value;

/// The starting options a new spotlight annotation carries.
pub const DEFAULTS: &[(&str, &str)] = &[("spotlight_shape", "Rounded rectangle")];

/// The default corner radius.
pub const DEFAULT_RADIUS: i32 = 8;

/// The default dimming alpha.
pub const DEFAULT_ALPHA: i32 = 155;

/// The dimming percentage for a slider value.
#[must_use]
pub fn dimming_percent(alpha: i32) -> i32 {
    (f64::from(alpha.clamp(0, 255)) / 255.0 * 100.0).round() as i32
}

/// The orientation of the spotlight controls for a shape.
#[must_use]
pub fn radius_enabled(shape: &str) -> bool {
    shape == "Rounded rectangle"
}

/// Merge control values into a spotlight annotation's properties.
// Ported from omnishot/spotlight.py (SpotlightControls.values/apply)
#[must_use]
pub fn apply(props: &mut BTreeMap<String, Value>, shape: &str, radius: i32, alpha: i32) -> bool {
    let values = [
        ("spotlight_shape", Value::String(shape.to_owned())),
        ("spotlight_radius", Value::Number(f64::from(radius))),
        ("spotlight_alpha", Value::Number(f64::from(alpha))),
    ];
    let mut changed = false;
    for (key, value) in values {
        if props.get(key) != Some(&value) {
            props.insert(key.to_owned(), value);
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimming_percentages_round_like_the_label() {
        assert_eq!(dimming_percent(155), 61);
        assert_eq!(dimming_percent(0), 0);
        assert_eq!(dimming_percent(255), 100);
        assert_eq!(dimming_percent(500), 100);
    }

    #[test]
    fn apply_reports_actual_changes_only() {
        let mut props = BTreeMap::new();
        assert!(apply(&mut props, "Rounded rectangle", 8, 155));
        assert!(!apply(&mut props, "Rounded rectangle", 8, 155));
        assert!(apply(&mut props, "Ellipse", 8, 155));
        assert_eq!(
            value("spotlight_shape", &props),
            &Value::String("Ellipse".to_owned())
        );
    }

    #[test]
    fn radius_only_applies_to_the_rounded_shape() {
        assert!(radius_enabled("Rounded rectangle"));
        assert!(!radius_enabled("Rectangle"));
        assert!(!radius_enabled("Ellipse"));
    }

    fn value<'a>(key: &str, props: &'a BTreeMap<String, Value>) -> &'a Value {
        props.get(key).expect("key present")
    }
}
