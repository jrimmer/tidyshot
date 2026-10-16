//! Recording presentation controls and input-event editing.
// Ported from omnishot/video_styles.py and omnishot/video_keys.py
//!
//! The controls are a set of named fields over an options object. Porting the
//! merge rules as pure functions keeps the inspector, the dialog and the
//! history snapshot agreeing on which keys a section owns.

use std::collections::BTreeMap;

use crate::json::Value;

/// The keys each inspector section is allowed to write.
///
/// An inspector writes only its own prefix so switching tools cannot restore
/// an old edit from another section.
// Ported from omnishot/video_ui.py (panel_fields)
#[must_use]
pub fn panel_fields(prefixes: &[&str]) -> Vec<&'static str> {
    const FIELDS: &[&str] = &[
        "cursor_style",
        "cursor_color",
        "cursor_outline",
        "click_style",
        "click_color",
        "click_size",
        "click_duration",
        "key_font",
        "key_size",
        "key_duration",
        "key_color",
        "key_background",
        "key_position",
        "key_style",
        "key_commands_only",
        "camera_radius",
        "camera_mirror",
        "camera_shadow",
        "camera_shrink",
    ];
    FIELDS
        .iter()
        .copied()
        .filter(|key| prefixes.iter().any(|prefix| key.starts_with(prefix)))
        .collect()
}

/// Merge an inspector panel's options back into the styles object.
///
/// Only keys the section owns are copied, matching `change_effects`.
// Ported from omnishot/video_ui.py (change_effects)
#[must_use]
pub fn change_effects(
    styles: &BTreeMap<String, Value>,
    panel: &BTreeMap<String, Value>,
    prefixes: &[&str],
) -> BTreeMap<String, Value> {
    let owned: Vec<&str> = panel_fields(prefixes);
    let mut result = styles.clone();
    for (key, value) in panel {
        if owned.contains(&key.as_str()) {
            result.insert(key.clone(), value.clone());
        }
    }
    result
}

/// Apply a named background preset, or clear the background for `None`.
///
/// A preset sets a gradient, rounding and shadow and grows the padding; the
/// caller mirrors the same values into the color field and padding control.
// Ported from omnishot/video_ui.py (apply_background)
#[must_use]
pub fn apply_background(
    styles: &mut BTreeMap<String, Value>,
    name: Option<&str>,
) -> (String, i32, bool) {
    match name.and_then(crate::backgrounds::Background::preset) {
        Some(background) => {
            let first = crate::color::color_text(background.color);
            let second = crate::color::color_text(background.color2);
            styles.insert("background2".to_owned(), Value::String(second));
            styles.insert("background_image_data".to_owned(), Value::Null);
            styles.insert("radius".to_owned(), Value::Number(12.0));
            styles.insert("shadow".to_owned(), Value::Bool(true));
            (first, 64, false)
        }
        None => {
            styles.insert("background2".to_owned(), Value::Null);
            styles.insert("background_image_data".to_owned(), Value::Null);
            styles.insert("radius".to_owned(), Value::Number(0.0));
            styles.insert("shadow".to_owned(), Value::Bool(false));
            ("#202633".to_owned(), 0, true)
        }
    }
}

/// One input event's edits, as the event editor stores them.
// Ported from omnishot/video_styles.py (InputEventsModel.setData)
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventEdit {
    pub t: Option<f64>,
    pub label: Option<String>,
    pub hidden: Option<bool>,
}

/// The rows the input-event editor shows: clicks and completed key strokes.
// Ported from omnishot/video_styles.py (InputEventsModel.__init__)
#[must_use]
pub fn editable_rows(events: &[Value]) -> Vec<usize> {
    events
        .iter()
        .enumerate()
        .filter(|(_, event)| {
            let kind = event.get("kind").and_then(Value::as_str);
            kind == Some("click")
                || (kind == Some("key")
                    && event.get("state").and_then(Value::as_f64) == Some(1.0)
                    && event.get("label").and_then(Value::as_str).is_some())
        })
        .map(|(index, _)| index)
        .collect()
}

/// Apply an edit to one event, returning `false` when the value is rejected.
///
/// A timestamp outside `0..=duration` or a label on a non-key event is
/// rejected, matching the editor's `setData`. A label is truncated to 200
/// characters.
// Ported from omnishot/video_styles.py (InputEventsModel.setData)
pub fn edit_event(
    edits: &mut BTreeMap<String, EventEdit>,
    index: usize,
    event: &Value,
    key: EditField,
    value: Value,
    duration: f64,
) -> bool {
    match key {
        EditField::Hidden => {
            edits.entry(index.to_string()).or_default().hidden = Some(!value.is_truthy());
            true
        }
        EditField::Time => {
            let Some(t) = value.as_f64() else {
                return false;
            };
            if !(0.0..=duration).contains(&t) {
                return false;
            }
            edits.entry(index.to_string()).or_default().t = Some(t);
            true
        }
        EditField::Label => {
            if event.get("kind").and_then(Value::as_str) != Some("key") {
                return false;
            }
            let Some(label) = value.as_str() else {
                return false;
            };
            let mut label = label.to_owned();
            label.truncate(200);
            edits.entry(index.to_string()).or_default().label = Some(label);
            true
        }
    }
}

/// The field an input-event edit targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditField {
    Hidden,
    Time,
    Label,
}

/// Serialize the event edits into the `event_edits` options object.
#[must_use]
pub fn edits_json(edits: &BTreeMap<String, EventEdit>) -> Value {
    let mut object = BTreeMap::new();
    for (index, edit) in edits {
        let mut entry = BTreeMap::new();
        if let Some(t) = edit.t {
            entry.insert("t".to_owned(), Value::Number(t));
        }
        if let Some(label) = &edit.label {
            entry.insert("label".to_owned(), Value::String(label.clone()));
        }
        if let Some(hidden) = edit.hidden {
            entry.insert("hidden".to_owned(), Value::Bool(hidden));
        }
        object.insert(index.clone(), Value::Object(entry));
    }
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_fields_only_own_the_section_prefix() {
        assert_eq!(
            panel_fields(&["cursor_"]),
            vec!["cursor_style", "cursor_color", "cursor_outline"]
        );
        assert_eq!(
            panel_fields(&["click_"]),
            vec!["click_style", "click_color", "click_size", "click_duration"]
        );
        assert!(panel_fields(&["key_"]).contains(&"key_commands_only"));
        assert!(panel_fields(&["camera_"]).contains(&"camera_mirror"));
    }

    #[test]
    fn change_effects_copies_only_owned_keys() {
        let styles = BTreeMap::from([("padding".to_owned(), Value::Number(8.0))]);
        let panel = BTreeMap::from([
            (
                "cursor_color".to_owned(),
                Value::String("#ff0000".to_owned()),
            ),
            ("padding".to_owned(), Value::Number(20.0)),
        ]);
        let merged = change_effects(&styles, &panel, &["cursor_"]);
        assert_eq!(
            merged.get("cursor_color").and_then(Value::as_str),
            Some("#ff0000")
        );
        assert_eq!(merged.get("padding").and_then(Value::as_f64), Some(8.0));
    }

    #[test]
    fn apply_background_sets_a_gradient_and_clears_it() {
        let mut styles = BTreeMap::new();
        let (color, padding, cleared) = apply_background(&mut styles, Some("Ocean"));
        assert_eq!(color, "#46c7e7");
        assert_eq!(padding, 64);
        assert!(!cleared);
        assert_eq!(
            styles.get("background2").and_then(Value::as_str),
            Some("#184496")
        );
        assert!(styles.get("shadow").and_then(Value::as_bool).unwrap());

        let (color, padding, cleared) = apply_background(&mut styles, None);
        assert_eq!(color, "#202633");
        assert_eq!(padding, 0);
        assert!(cleared);
        assert_eq!(styles.get("background2"), Some(&Value::Null));
    }

    #[test]
    fn editable_rows_skip_hidden_key_releases() {
        let events = vec![
            event("click", &[("t", Value::Number(1.0))]),
            event(
                "key",
                &[
                    ("t", Value::Number(1.0)),
                    ("state", Value::Number(1.0)),
                    ("label", Value::String("A".to_owned())),
                ],
            ),
            event(
                "key",
                &[
                    ("t", Value::Number(1.0)),
                    ("state", Value::Number(0.0)),
                    ("label", Value::String("A".to_owned())),
                ],
            ),
        ];
        assert_eq!(editable_rows(&events), vec![0, 1]);
    }

    #[test]
    fn edit_event_validates_time_and_label() {
        let events = event(
            "key",
            &[
                ("t", Value::Number(1.0)),
                ("state", Value::Number(1.0)),
                ("label", Value::String("A".to_owned())),
            ],
        );
        let mut edits = BTreeMap::new();
        assert!(edit_event(
            &mut edits,
            0,
            &events,
            EditField::Time,
            Value::Number(2.0),
            5.0
        ));
        assert_eq!(edits.get("0").unwrap().t, Some(2.0));
        assert!(!edit_event(
            &mut edits,
            0,
            &events,
            EditField::Time,
            Value::Number(9.0),
            5.0
        ));
        // A rejected edit must not create an empty slot in the map.
        assert!(!edit_event(
            &mut edits,
            1,
            &events,
            EditField::Time,
            Value::Number(9.0),
            5.0
        ));
        assert!(!edits.contains_key("1"));
        assert!(edit_event(
            &mut edits,
            0,
            &events,
            EditField::Label,
            Value::String("Ctrl+V".to_owned()),
            5.0
        ));
        assert_eq!(edits.get("0").unwrap().label.as_deref(), Some("Ctrl+V"));
        let json = edits_json(&edits);
        assert_eq!(
            json.get("0")
                .and_then(|e| e.get("label"))
                .and_then(Value::as_str),
            Some("Ctrl+V")
        );
    }

    fn event(kind: &str, extra: &[(&str, Value)]) -> Value {
        let mut object = BTreeMap::new();
        object.insert("kind".to_owned(), Value::String(kind.to_owned()));
        for (key, value) in extra {
            object.insert((*key).to_owned(), value.clone());
        }
        Value::Object(object)
    }
}
