//! Annotation tools, shortcuts and the pin shortcut validator.
// Ported from omnishot/editor_toolbar.py (TOOLS) and
// omnishot/annotation_shortcuts.py.

/// Every annotation tool, its label and its single-key shortcut.
pub const TOOLS: &[(&str, &str, &str)] = &[
    ("select", "Select", "V"),
    ("rect", "Rectangle", "R"),
    ("fill", "Filled rectangle", "F"),
    ("ellipse", "Ellipse", "E"),
    ("line", "Line", "L"),
    ("arrow", "Arrow", "A"),
    ("text", "Text", "T"),
    ("pixelate", "Pixelate", "X"),
    ("blur", "Blur", "B"),
    ("redact", "Black Out", "D"),
    ("spotlight", "Spotlight", "S"),
    ("counter", "Counter", "N"),
    ("pencil", "Pencil", "P"),
    ("highlight", "Highlighter", "H"),
];

/// Tools the toolbar keeps behind a menu button.
pub const HIDDEN_TOOLS: &[&str] = &["pixelate", "blur", "redact"];

/// The reserved editing commands a pin shortcut may not shadow.
const RESERVED: &[&str] = &[
    "C",
    "Ctrl+O",
    "Ctrl+S",
    "Ctrl+Shift+S",
    "Ctrl+I",
    "Ctrl+C",
    "Ctrl+V",
    "Ctrl+P",
    "Ctrl+Z",
    "Ctrl+Shift+Z",
    "Delete",
    "Backspace",
    "Ctrl+D",
    "Space",
    "Escape",
    "Return",
    "Enter",
    "Left",
    "Right",
    "Up",
    "Down",
    "Ctrl+Return",
    "Ctrl+Enter",
    "Ctrl+A",
    "Ctrl+X",
    "Ctrl+Y",
];

/// Why a pin shortcut was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutError {
    message: String,
}

impl ShortcutError {
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for ShortcutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ShortcutError {}

/// Validate a candidate pin shortcut against the annotation and global set.
///
/// `value` is empty to clear the shortcut. `global_shortcuts` maps action
/// names to their assigned sequences.
// Ported from omnishot/annotation_shortcuts.py (validate_pin)
pub fn validate_pin(value: &str, global_shortcuts: &[(&str, &str)]) -> Result<(), ShortcutError> {
    if value.is_empty() {
        return Ok(());
    }
    let sequence = normalize(value);
    if sequence.is_empty() {
        return Err(ShortcutError {
            message: "Choose one key combination for Pin in Annotate.".to_owned(),
        });
    }
    // A modifier on its own is not a key combination.
    if ["shift", "ctrl", "alt", "meta"]
        .iter()
        .any(|modifier| sequence == *modifier)
    {
        return Err(ShortcutError {
            message: "Choose one key combination for Pin in Annotate.".to_owned(),
        });
    }
    let reserved = TOOLS
        .iter()
        .map(|(_, _, shortcut)| *shortcut)
        .chain(RESERVED.iter().copied());
    if matches!(sequence.as_str(), "Left" | "Right" | "Up" | "Down")
        || reserved.into_iter().any(|key| normalize(key) == sequence)
    {
        return Err(ShortcutError {
            message: "That shortcut is already used for an annotation or text-editing command."
                .to_owned(),
        });
    }
    if global_shortcuts
        .iter()
        .any(|(_, key)| !key.is_empty() && normalize(key) == sequence)
    {
        return Err(ShortcutError {
            message: "That shortcut is already assigned to a global action.".to_owned(),
        });
    }
    Ok(())
}

/// True when a shortcut keeps working while an inline text editor has focus.
// Ported from omnishot/annotation_shortcuts.py (available_while_typing)
#[must_use]
pub fn available_while_typing(sequence: &str) -> bool {
    if sequence.is_empty() {
        return false;
    }
    sequence.starts_with("Ctrl+")
        || sequence.starts_with("Alt+")
        || sequence.starts_with("Meta+")
        || function_key(sequence)
}

fn function_key(sequence: &str) -> bool {
    let Some(number) = sequence.strip_prefix('F') else {
        return false;
    };
    number
        .parse::<u32>()
        .map(|number| (1..=35).contains(&number))
        .unwrap_or(false)
}

/// Normalize a Qt key sequence to a comparable, canonical form.
///
/// Qt compares key sequences case-insensitively and ignores modifier order,
/// so the comparison must do the same.
fn normalize(value: &str) -> String {
    let parts: Vec<String> = value
        .split('+')
        .filter(|part| !part.is_empty())
        .map(|part| part.to_ascii_lowercase())
        .collect();
    if parts.is_empty() {
        return String::new();
    }
    let mut modifiers: Vec<String> = parts[..parts.len() - 1].to_vec();
    let key = parts[parts.len() - 1].clone();
    modifiers.sort_unstable();
    modifiers.dedup();
    let mut out = modifiers.join("+");
    if !out.is_empty() {
        out.push('+');
    }
    out.push_str(&key);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tool_table_matches_the_toolbar() {
        assert_eq!(TOOLS.len(), 14);
        assert!(HIDDEN_TOOLS.contains(&"pixelate"));
        assert_eq!(TOOLS[0], ("select", "Select", "V"));
    }

    #[test]
    fn reserved_and_global_shortcuts_are_refused() {
        assert!(validate_pin("", &[]).is_ok());
        assert!(validate_pin("Ctrl+K", &[]).is_ok());
        for bad in ["A", "R", "Ctrl+S", "Left", "Delete", "Space"] {
            assert!(validate_pin(bad, &[]).is_err(), "accepted {bad}");
        }
        assert!(validate_pin("Ctrl+G", &[("search", "Ctrl+G")]).is_err());
        assert!(validate_pin("Ctrl+G", &[("search", "Ctrl+H")]).is_ok());
        assert!(validate_pin("Shift", &[]).is_err());
    }

    #[test]
    fn shortcuts_are_normalized_before_comparison() {
        // Modifier order and letter case must not let a duplicate slip through.
        assert!(validate_pin("Shift+Ctrl+S", &[]).is_err());
        assert!(validate_pin("ctrl+shift+s", &[]).is_err());
        assert!(validate_pin("Ctrl+K", &[]).is_ok());
    }

    #[test]
    fn modifier_and_function_shortcuts_survive_typing() {
        assert!(available_while_typing("Ctrl+A"));
        assert!(available_while_typing("Alt+P"));
        assert!(available_while_typing("Meta+P"));
        assert!(available_while_typing("F5"));
        assert!(!available_while_typing("F0"));
        assert!(!available_while_typing("A"));
        assert!(!available_while_typing(""));
    }
}
