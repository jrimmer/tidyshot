//! Zoom animation selection and continuous joins between adjacent zooms.
// Ported from omnishot/video_motion.py and omnishot/studio.py (zoom_at)
//!
//! The active zoom ramps in and out; when two zooms touch, the ramp is
//! replaced by a continuous join so the scale never dips between them.

use crate::json::Value;

/// One zoom region: a source interval with a focus target and scale.
#[derive(Debug, Clone, PartialEq)]
pub struct Zoom {
    pub start: f64,
    pub end: f64,
    pub scale: f64,
    pub x: f64,
    pub y: f64,
    pub follow: bool,
}

impl Zoom {
    /// Read a zoom from a JSON object, applying the same defaults the editor
    /// uses when it creates one.
    #[must_use]
    pub fn from_json(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        Some(Self {
            start: object.get("start").and_then(Value::as_f64)?,
            end: object.get("end").and_then(Value::as_f64)?,
            scale: object.get("scale").and_then(Value::as_f64).unwrap_or(1.5),
            x: object.get("x").and_then(Value::as_f64).unwrap_or(0.5),
            y: object.get("y").and_then(Value::as_f64).unwrap_or(0.5),
            follow: object
                .get("follow")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        })
    }

    /// Read a list of zooms from a JSON array.
    #[must_use]
    pub fn list_from_json(value: Option<&Value>) -> Vec<Self> {
        value
            .and_then(Value::as_array)
            .map(|rows| rows.iter().filter_map(Self::from_json).collect())
            .unwrap_or_default()
    }

    /// The zoom's center `(x, y)`, using the cursor when it follows.
    #[must_use]
    pub fn focus(&self, cursor: Option<(f64, f64)>) -> (f64, f64) {
        match (self.follow, cursor) {
            (true, Some(cursor)) => cursor,
            _ => (self.x, self.y),
        }
    }
}

/// The animation mode names the Motion inspector offers.
pub const ANIMATIONS: &[&str] = &["Smooth", "Dynamic"];

/// The zoom `(scale, x, y)` at `t`, with continuous joins between neighbors.
// Ported from omnishot/video_motion.py (zoom_at)
#[must_use]
pub fn zoom_at(
    zooms: &[Zoom],
    t: f64,
    cursor: Option<(f64, f64)>,
    animation: &str,
) -> (f64, f64, f64) {
    let Some(active) = zooms.iter().find(|z| z.start <= t && t < z.end) else {
        return (1.0, 0.5, 0.5);
    };
    let (a, b) = (active.start, active.end);
    let ramp = if animation == "Dynamic" {
        0.22_f64
    } else {
        0.35
    };
    let ramp = ramp.min((b - a) / 2.0);

    let target = |zoom: &Zoom| {
        let (x, y) = zoom.focus(cursor);
        (zoom.scale, x, y)
    };
    let ease = |value: f64| {
        let value = value.clamp(0.0, 1.0);
        if animation == "Dynamic" {
            1.0 - (1.0 - value).powi(3)
        } else {
            value * value * (3.0 - 2.0 * value)
        }
    };

    let previous = zooms
        .iter()
        .find(|z| !std::ptr::eq(*z, active) && (z.end - a).abs() < 1e-7);
    let following = zooms
        .iter()
        .find(|z| !std::ptr::eq(*z, active) && (z.start - b).abs() < 1e-7);
    let result = target(active);

    if let Some(previous) = previous
        && t < a + ramp
    {
        let before = target(previous);
        let weight = ease((t - a) / ramp);
        return (
            before.0 + (result.0 - before.0) * weight,
            before.1 + (result.1 - before.1) * weight,
            before.2 + (result.2 - before.2) * weight,
        );
    }

    let mut weight = 1.0_f64;
    if previous.is_none() {
        weight = weight.min((t - a) / ramp);
    }
    if following.is_none() {
        weight = weight.min((b - t) / ramp);
    }
    (1.0 + (result.0 - 1.0) * ease(weight), result.1, result.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zoom(start: f64, end: f64, scale: f64, x: f64, y: f64) -> Zoom {
        Zoom {
            start,
            end,
            scale,
            x,
            y,
            follow: false,
        }
    }

    #[test]
    fn joined_zoom_transitions_never_zoom_out() {
        let a = zoom(1.0, 3.0, 2.0, 0.25, 0.5);
        let b = zoom(3.0, 5.0, 3.0, 0.75, 0.5);
        assert_eq!(
            zoom_at(&[a.clone(), b.clone()], 3.0, None, "Smooth"),
            (2.0, 0.25, 0.5)
        );
        for mode in ["Smooth", "Dynamic"] {
            let samples: Vec<(f64, f64, f64)> = (2950..=3500)
                .step_by(10)
                .map(|t| zoom_at(&[a.clone(), b.clone()], f64::from(t) / 1000.0, None, mode))
                .collect();
            assert!(samples.iter().all(|v| v.0 >= 2.0 - 1e-9));
            for pair in samples.windows(2) {
                assert!(pair[1].0 >= pair[0].0 - 1e-9);
                assert!(pair[1].1 >= pair[0].1 - 1e-9);
            }
            assert_eq!(samples.last().copied().unwrap(), (3.0, 0.75, 0.5));
        }
    }

    #[test]
    fn isolated_zoom_ramps_and_dynamic_is_faster() {
        let a = zoom(1.0, 3.0, 2.0, 0.25, 0.5);
        assert_eq!(zoom_at(&[a], 3.0, None, "Smooth").0, 1.0);
        let smooth = zoom_at(&[zoom(1.0, 3.0, 2.0, 0.25, 0.5)], 1.1, None, "Smooth");
        let dynamic = zoom_at(&[zoom(1.0, 3.0, 2.0, 0.25, 0.5)], 1.1, None, "Dynamic");
        assert!(dynamic.0 > smooth.0);
    }

    #[test]
    fn follow_uses_the_cursor() {
        let a = Zoom {
            follow: true,
            ..zoom(1.0, 3.0, 2.0, 0.5, 0.5)
        };
        assert_eq!(
            zoom_at(&[a], 2.0, Some((0.6, 0.4)), "Smooth"),
            (2.0, 0.6, 0.4)
        );
    }

    #[test]
    fn smart_zoom_planning_respects_the_limit_and_spacing() {
        let events = vec![
            click(1.0, 0.3, 0.4),
            click(3.8, 0.8, 0.5),
            click(3.9, 2.0, 0.5),
        ];
        let zooms = crate::studio::smart_zooms(&events, 2.8, 1.8, Some(4.0));
        assert!(!zooms.is_empty());
        assert!(zooms.iter().all(|z| z.end <= 4.0));
        for pair in zooms.windows(2) {
            assert!(pair[0].end <= pair[1].start);
        }
    }

    fn click(t: f64, x: f64, y: f64) -> crate::json::Value {
        let mut object = std::collections::BTreeMap::new();
        object.insert("kind".to_owned(), Value::String("click".to_owned()));
        object.insert("t".to_owned(), Value::Number(t));
        object.insert("x".to_owned(), Value::Number(x));
        object.insert("y".to_owned(), Value::Number(y));
        Value::Object(object)
    }
}
