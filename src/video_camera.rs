//! Pause-aware camera framing changes on the recording's source clock.
// Ported from omnishot/camera_framing.py
//!
//! A recording can change the camera's shape, size and fullscreen state while
//! it runs. The changes are stamped on the same monotonic clock as the cursor
//! samples and normalized by the recorder's pauses, so a paused take does not
//! shift a framing change relative to the video it corresponds to.

use std::collections::BTreeMap;

use crate::json::Value;

/// One recorded framing change.
#[derive(Debug, Clone, PartialEq)]
pub struct FramingEvent {
    /// Source-clock instant in seconds.
    pub t: f64,
    pub fullscreen: bool,
    pub shape: Option<String>,
    /// `(x, y, size)` placement, when the camera was being dragged.
    pub placement: Option<(f64, f64, f64)>,
}

impl FramingEvent {
    /// Read a framing event from a manifest object.
    #[must_use]
    pub fn from_json(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let t = object.get("t").and_then(Value::as_f64)?;
        if !t.is_finite() || t < 0.0 {
            return None;
        }
        let fullscreen = object.get("fullscreen").and_then(Value::as_bool)?;
        let shape = object
            .get("shape")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let placement = object
            .get("placement")
            .and_then(Value::as_array)
            .and_then(|values| {
                if values.len() != 3 {
                    return None;
                }
                Some((
                    values[0].as_f64()?,
                    values[1].as_f64()?,
                    values[2].as_f64()?,
                ))
            });
        Some(Self {
            t,
            fullscreen,
            shape,
            placement,
        })
    }

    /// Read a manifest framing list.
    #[must_use]
    pub fn list_from_json(value: Option<&Value>) -> Vec<Self> {
        value
            .and_then(Value::as_array)
            .map(|rows| rows.iter().filter_map(Self::from_json).collect())
            .unwrap_or_default()
    }

    /// Serialize back to a manifest object.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut object = BTreeMap::new();
        object.insert("t".to_owned(), Value::Number(self.t));
        object.insert("fullscreen".to_owned(), Value::Bool(self.fullscreen));
        if let Some(shape) = &self.shape {
            object.insert("shape".to_owned(), Value::String(shape.clone()));
        }
        if let Some((x, y, size)) = self.placement {
            object.insert(
                "placement".to_owned(),
                Value::Array(vec![
                    Value::Number(x),
                    Value::Number(y),
                    Value::Number(size),
                ]),
            );
        }
        Value::Object(object)
    }
}

/// Normalize recorded framing onto the source clock.
///
/// `origin` is the first frame's monotonic instant; a pause interval removes
/// its own duration from later timestamps. Adjacent events that collapse onto
/// the same instant keep the last one, matching the source.
// Ported from omnishot/camera_framing.py (snapshot)
#[must_use]
pub fn snapshot(
    events: &[FramingEvent],
    pauses: &[(f64, f64)],
    origin: Option<f64>,
) -> Vec<FramingEvent> {
    let Some(origin) = origin else {
        return Vec::new();
    };
    let mut result: Vec<FramingEvent> = Vec::new();
    for row in events {
        let t = row.t;
        let paused: f64 = pauses
            .iter()
            .filter(|(a, _)| *a < t)
            .map(|(a, b)| (t.min(*b) - origin.max(*a)).max(0.0))
            .sum();
        let stamp = (t - origin - paused).max(0.0);
        let value = FramingEvent {
            t: stamp,
            ..row.clone()
        };
        if let Some(last) = result.last_mut()
            && (last.t - stamp).abs() < 1e-7
        {
            *last = value;
        } else {
            result.push(value);
        }
    }
    result
}

/// Apply the framing changes recorded up to `t` to the options.
///
/// Returns the options unchanged when recorded framing is disabled, when a
/// manual fullscreen choice overrides it, or when there are no changes yet.
// Ported from omnishot/camera_framing.py (options_at)
#[must_use]
pub fn options_at(
    metadata: &Value,
    options: &BTreeMap<String, Value>,
    t: f64,
) -> BTreeMap<String, Value> {
    let recorded = options
        .get("camera_recorded_framing")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let fullscreen = options
        .get("camera_fullscreen")
        .is_some_and(Value::is_truthy);
    if !recorded || fullscreen {
        return options.clone();
    }
    let events = FramingEvent::list_from_json(metadata.get("camera_framing"));
    if events.is_empty() {
        return options.clone();
    }
    let mut result = options.clone();
    result.insert("camera_fullscreen".to_owned(), Value::Bool(false));
    for event in events {
        if event.t > t {
            break;
        }
        result.insert(
            "camera_fullscreen".to_owned(),
            Value::Bool(event.fullscreen),
        );
        if let Some(shape) = &event.shape {
            result.insert("camera_shape".to_owned(), Value::String(shape.clone()));
        }
        if let Some((x, y, size)) = event.placement {
            result.insert(
                "camera_placement".to_owned(),
                Value::Array(vec![
                    Value::Number(x),
                    Value::Number(y),
                    Value::Number(size),
                ]),
            );
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(t: f64, fullscreen: bool, shape: Option<&str>) -> FramingEvent {
        FramingEvent {
            t,
            fullscreen,
            shape: shape.map(str::to_owned),
            placement: None,
        }
    }

    #[test]
    fn snapshot_normalizes_pauses_and_collapses_equal_stamps() {
        let events = vec![
            event(100.0, false, None),
            event(113.0, true, Some("Circle")),
        ];
        let pauses = [(105.0, 110.0)];
        let normalized = snapshot(&events, &pauses, Some(100.0));
        assert_eq!(normalized[0].t, 0.0);
        // 13 seconds later minus a 5 second pause.
        assert!((normalized[1].t - 8.0).abs() < 1e-9);

        let collapsed = snapshot(
            &[event(100.0, false, None), event(100.0, true, None)],
            &[],
            Some(100.0),
        );
        assert_eq!(collapsed.len(), 1);
        assert!(collapsed[0].fullscreen);
    }

    #[test]
    fn snapshot_without_origin_is_empty() {
        assert!(snapshot(&[event(100.0, false, None)], &[], None).is_empty());
    }

    #[test]
    fn options_at_applies_changes_up_to_the_instant() {
        let metadata = Value::Object(BTreeMap::from([(
            "camera_framing".to_owned(),
            Value::Array(vec![
                event(1.0, false, Some("Square")).to_json(),
                event(3.0, true, None).to_json(),
            ]),
        )]));
        let options = BTreeMap::new();
        let early = options_at(&metadata, &options, 0.5);
        assert_eq!(
            early.get("camera_fullscreen").and_then(Value::as_bool),
            Some(false)
        );
        assert!(!early.contains_key("camera_shape"));
        let middle = options_at(&metadata, &options, 2.0);
        assert_eq!(
            middle.get("camera_shape").and_then(Value::as_str),
            Some("Square")
        );
        let late = options_at(&metadata, &options, 3.5);
        assert_eq!(
            late.get("camera_fullscreen").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn manual_fullscreen_or_disabled_recorded_framing_wins() {
        let metadata = Value::Object(BTreeMap::from([(
            "camera_framing".to_owned(),
            Value::Array(vec![event(1.0, true, None).to_json()]),
        )]));
        let manual = BTreeMap::from([("camera_fullscreen".to_owned(), Value::Bool(true))]);
        assert_eq!(options_at(&metadata, &manual, 2.0), manual);
        let disabled = BTreeMap::from([("camera_recorded_framing".to_owned(), Value::Bool(false))]);
        assert_eq!(options_at(&metadata, &disabled, 2.0), disabled);
    }
}
