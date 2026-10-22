//! User settings persisted beside the capture store.
// Ported from omnishot/backend.py (Store)
//!
//! Only the OCR keys the recognized-text path reads are exposed so far; the
//! rest of the settings store lands with the settings UI.

use std::path::PathBuf;

use crate::ocr;
use crate::text_result::TextSettings;

/// The tidyshot data directory that holds the OCR models.
///
/// The settings file lives beside that root, so the OCR model installer and
/// the preferences always agree on one data home.
#[must_use]
pub fn data_dir() -> PathBuf {
    let models = ocr::model_dir();
    match models.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// The settings file a data root holds.
#[must_use]
pub fn settings_path_at(root: &std::path::Path) -> PathBuf {
    root.join("settings.json")
}

/// The path of the JSON settings file.
#[must_use]
pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

/// Read the OCR preferences, falling back to defaults when nothing is saved.
///
/// A missing or unreadable file is not an error: defaults match the source,
/// and a broken settings file must not stop OCR.
#[must_use]
pub fn ocr_settings() -> TextSettings {
    ocr_settings_at(&settings_path())
}

/// Read the saved settings the decoder policy consults.
///
/// A missing or unreadable file yields an empty map, so software decoding
/// stays the default rather than failing startup.
#[must_use]
pub fn stored_settings() -> std::collections::BTreeMap<String, crate::json::Value> {
    stored_settings_at(&settings_path())
}

/// The Quick Access preview preferences from the saved settings.
#[must_use]
pub fn preview_settings() -> crate::overlay::PreviewSettings {
    preview_settings_at(&settings_path())
}

/// Read the Quick Access preferences from an explicit settings file.
#[must_use]
fn preview_settings_at(path: &std::path::Path) -> crate::overlay::PreviewSettings {
    crate::overlay::preview_settings(&stored_settings_at(path))
}

/// Read the saved settings from an explicit file.
#[must_use]
fn stored_settings_at(
    path: &std::path::Path,
) -> std::collections::BTreeMap<String, crate::json::Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return std::collections::BTreeMap::new();
    };
    match crate::json::parse(&text) {
        Ok(crate::json::Value::Object(object)) => object,
        _ => std::collections::BTreeMap::new(),
    }
}

/// Read the OCR preferences from an explicit settings file.
#[must_use]
fn ocr_settings_at(path: &std::path::Path) -> TextSettings {
    let Ok(text) = std::fs::read_to_string(path) else {
        return TextSettings::default();
    };
    match crate::json::parse(&text) {
        Ok(value) => TextSettings::from_value(&value),
        Err(_) => TextSettings::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ocr;

    #[test]
    fn ocr_settings_falls_back_to_defaults_without_a_file() {
        let missing = std::env::temp_dir().join(format!(
            "tidyshot-settings-missing-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&missing);
        assert_eq!(ocr_settings_at(&missing), TextSettings::default());
    }

    #[test]
    fn ocr_settings_reads_saved_keys() {
        let path =
            std::env::temp_dir().join(format!("tidyshot-settings-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"{"ocr_languages":"eng+spa","ocr_linebreaks":false,"ocr_detect_links":false}"#,
        )
        .unwrap();
        let settings = ocr_settings_at(&path);
        assert_eq!(settings.ocr_languages, "eng+spa");
        assert!(!settings.ocr_linebreaks);
        assert!(!settings.ocr_detect_links);
        let _ = std::fs::remove_file(&path);
        // A malformed file must not stop OCR either.
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(ocr_settings_at(&path).ocr_languages, ocr::AUTO);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn preview_settings_read_the_quick_access_keys() {
        let path =
            std::env::temp_dir().join(format!("tidyshot-preview-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"{"preview_timeout":4,"preview_action":"save","close_after_drag":false,"preview_corner":"top-right"}"#,
        )
        .unwrap();
        let settings = preview_settings_at(&path);
        assert_eq!(settings.auto_close_ms(), 4000);
        assert_eq!(settings.auto_close_action(), "save");
        assert!(!settings.close_after_drag);
        assert_eq!(settings.corner, crate::overlay::Corner::TopRight);
        // A missing or malformed file falls back to the fresh-preview defaults.
        let _ = std::fs::remove_file(&path);
        let defaults = preview_settings_at(&path);
        assert_eq!(defaults.auto_close_ms(), 0);
        assert_eq!(defaults.corner, crate::overlay::Corner::BottomRight);
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(preview_settings_at(&path).auto_close_ms(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stored_settings_expose_the_decoder_flag_and_tolerate_bad_files() {
        let path =
            std::env::temp_dir().join(format!("tidyshot-decoder-{}.json", std::process::id()));
        std::fs::write(&path, r#"{"hardware_video_decoding":true}"#).unwrap();
        assert_eq!(
            stored_settings_at(&path).get("hardware_video_decoding"),
            Some(&crate::json::Value::Bool(true))
        );
        // A missing or malformed file yields no settings, so software stays
        // the default instead of failing startup.
        let _ = std::fs::remove_file(&path);
        assert!(stored_settings_at(&path).is_empty());
        std::fs::write(&path, "not json").unwrap();
        assert!(stored_settings_at(&path).is_empty());
        let _ = std::fs::remove_file(&path);
    }
}
