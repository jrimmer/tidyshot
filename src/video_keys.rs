//! Keystroke presentation rules and the command-key filter.
// Ported from omnishot/video_keys.py
//!
//! Old recordings stored only a label; the filter falls back to parsing the
//! modifiers out of that label so those recordings still hide plain keys.

use crate::json::Value;

/// Whether a keystroke counts as a command shortcut.
///
/// A stored `command` boolean wins; otherwise a label whose leading modifiers
/// include Ctrl/Alt/Super/Cmd/Meta is a command.
// Ported from omnishot/video_keys.py (command_event)
#[must_use]
pub fn command_event(event: &Value) -> bool {
    if let Some(value) = event.get("command").and_then(Value::as_bool) {
        return value;
    }
    let label = event.get("label").and_then(Value::as_str).unwrap_or("");
    let mut parts: Vec<&str> = label.split('+').collect();
    parts.pop();
    parts
        .iter()
        .any(|part| matches!(*part, "Ctrl" | "Alt" | "Super" | "Cmd" | "Meta"))
}

/// The keystroke labels visible at `t`, in event order.
// Ported from omnishot/video_keys.py (visible_keys)
#[must_use]
pub fn visible_keys(
    events: &[Value],
    t: f64,
    options: &std::collections::BTreeMap<String, Value>,
    metadata: &Value,
) -> Vec<String> {
    let commands = options
        .get("key_commands_only")
        .and_then(Value::as_bool)
        .or_else(|| {
            metadata
                .get("capture_options")
                .and_then(|capture| capture.get("commands_only"))
                .and_then(Value::as_bool)
        })
        .unwrap_or(false);
    let duration = options
        .get("key_duration")
        .and_then(Value::as_f64)
        .unwrap_or(1.3);
    let mut result = Vec::new();
    for event in events {
        if event.get("kind").and_then(Value::as_str) != Some("key") {
            continue;
        }
        if event.get("state").and_then(Value::as_f64) != Some(1.0) {
            continue;
        }
        if event.get("hidden").is_some_and(Value::is_truthy) {
            continue;
        }
        let Some(label) = event.get("label").and_then(Value::as_str) else {
            continue;
        };
        if label.is_empty() {
            continue;
        }
        let Some(instant) = event.get("t").and_then(Value::as_f64) else {
            continue;
        };
        let age = t - instant;
        if !(0.0..duration).contains(&age) {
            continue;
        }
        if commands && !command_event(event) {
            continue;
        }
        result.push(label.to_owned());
    }
    result
}

/// The keystroke chip's background and foreground colors.
///
/// The legacy `Custom` style is inferred when either color is present.
// Ported from omnishot/video_keys.py (key_colors)
#[must_use]
pub fn key_colors(options: &std::collections::BTreeMap<String, Value>) -> (String, String) {
    let style = options
        .get("key_style")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            if options.contains_key("key_color") || options.contains_key("key_background") {
                "Custom"
            } else {
                "Dark"
            }
        });
    match style {
        "Light" => ("#e8ffffff".to_owned(), "#202124".to_owned()),
        "Dark" => ("#d7141720".to_owned(), "#ffffff".to_owned()),
        _ => (
            options
                .get("key_background")
                .and_then(Value::as_str)
                .unwrap_or("#d7141720")
                .to_owned(),
            options
                .get("key_color")
                .and_then(Value::as_str)
                .unwrap_or("#ffffff")
                .to_owned(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn event(entries: &[(&str, Value)]) -> Value {
        let mut object = BTreeMap::new();
        for (key, value) in entries {
            object.insert((*key).to_owned(), value.clone());
        }
        Value::Object(object)
    }

    fn options(entries: &[(&str, Value)]) -> BTreeMap<String, Value> {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    #[test]
    fn command_classification_prefers_a_stored_flag_and_parses_labels() {
        assert!(command_event(&event(&[(
            "label",
            Value::String("Ctrl+C".to_owned())
        )])));
        assert!(!command_event(&event(&[(
            "label",
            Value::String("A".to_owned())
        )])));
        assert!(command_event(&event(&[
            ("label", Value::String("A".to_owned())),
            ("command", Value::Bool(true)),
        ])));
        assert!(!command_event(&event(&[
            ("label", Value::String("Ctrl+C".to_owned())),
            ("command", Value::Bool(false)),
        ])));
    }

    #[test]
    fn visible_keys_respect_duration_and_the_command_filter() {
        let events = vec![
            event(&[
                ("kind", Value::String("key".to_owned())),
                ("state", Value::Number(1.0)),
                ("t", Value::Number(1.0)),
                ("label", Value::String("A".to_owned())),
            ]),
            event(&[
                ("kind", Value::String("key".to_owned())),
                ("state", Value::Number(1.0)),
                ("t", Value::Number(1.0)),
                ("label", Value::String("Ctrl+C".to_owned())),
            ]),
        ];
        assert_eq!(
            visible_keys(&events, 1.2, &options(&[]), &Value::Null),
            vec!["A", "Ctrl+C"]
        );
        // Past the default 1.3 s duration the label drops out.
        assert!(visible_keys(&events, 2.5, &options(&[]), &Value::Null).is_empty());
        assert_eq!(
            visible_keys(
                &events,
                1.2,
                &options(&[("key_commands_only", Value::Bool(true))]),
                &Value::Null
            ),
            vec!["Ctrl+C"]
        );
        let metadata = event(&[(
            "capture_options",
            event(&[("commands_only", Value::Bool(true))]),
        )]);
        assert_eq!(
            visible_keys(&events, 1.2, &options(&[]), &metadata),
            vec!["Ctrl+C"]
        );
    }

    #[test]
    fn key_colors_use_named_styles_and_legacy_custom() {
        assert_eq!(key_colors(&options(&[])).0, "#d7141720");
        assert_eq!(key_colors(&options(&[])).1, "#ffffff");
        assert_eq!(
            key_colors(&options(&[(
                "key_style",
                Value::String("Light".to_owned())
            )]))
            .0,
            "#e8ffffff"
        );
        let custom = key_colors(&options(&[(
            "key_color",
            Value::String("#ff0000".to_owned()),
        )]));
        assert_eq!(custom.1, "#ff0000");
    }
}
