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

/// Read the saved settings from the store under an explicit root.
///
/// The root is a parameter so a test never reads the user's settings file.
#[must_use]
pub fn stored_settings_in(
    root: &std::path::Path,
) -> std::collections::BTreeMap<String, crate::json::Value> {
    stored_settings_at(&root.join("settings.json"))
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

/// The number of days a capture is kept before the startup cleanup trashes it.
///
/// A missing key keeps a month, the longest retention the preferences offer;
/// zero disables cleanup and keeps everything.
#[must_use]
pub fn retention_days() -> u32 {
    retention_days_at(&settings_path())
}

/// Read the retention setting from an explicit settings file.
#[must_use]
fn retention_days_at(path: &std::path::Path) -> u32 {
    stored_settings_at(path)
        .get("history_retention_days")
        .and_then(crate::json::Value::as_f64)
        .map_or(DEFAULT_RETENTION_DAYS, |value| {
            value.clamp(0.0, 3650.0) as u32
        })
}

/// The retention a new install keeps: one month.
pub const DEFAULT_RETENTION_DAYS: u32 = 30;

/// Write one settings key, keeping every other saved key.
///
/// The file is rewritten atomically so a crash cannot leave a partial
/// settings file, and a malformed existing file is replaced rather than
/// allowed to block the write.
pub fn write_setting(key: &str, value: crate::json::Value) -> std::io::Result<()> {
    write_setting_at(&settings_path(), key, value)
}

/// Write one settings key to an explicit file.
pub fn write_setting_at(
    path: &std::path::Path,
    key: &str,
    value: crate::json::Value,
) -> std::io::Result<()> {
    let mut object = stored_settings_at(path);
    object.insert(key.to_owned(), value);
    let text = crate::json::Value::Object(object).to_string_pretty();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::storage::atomic_write_file(path, text.as_bytes())
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
    fn retention_defaults_to_a_month_and_reads_a_saved_value() {
        let path =
            std::env::temp_dir().join(format!("tidyshot-retention-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        assert_eq!(retention_days_at(&path), DEFAULT_RETENTION_DAYS);
        std::fs::write(&path, r#"{"history_retention_days":7}"#).unwrap();
        assert_eq!(retention_days_at(&path), 7);
        std::fs::write(&path, r#"{"history_retention_days":0}"#).unwrap();
        assert_eq!(retention_days_at(&path), 0, "zero disables cleanup");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn writing_one_setting_keeps_the_others() {
        let path = std::env::temp_dir().join(format!("tidyshot-write-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, r#"{"ocr_languages":"eng"}"#).unwrap();
        write_setting_at(
            &path,
            "history_retention_days",
            crate::json::Value::Number(14.0),
        )
        .unwrap();
        let settings = stored_settings_at(&path);
        assert_eq!(
            settings.get("ocr_languages"),
            Some(&crate::json::Value::String("eng".to_owned()))
        );
        assert_eq!(
            settings.get("history_retention_days"),
            Some(&crate::json::Value::Number(14.0))
        );
        // A malformed file is replaced instead of blocking the write.
        std::fs::write(&path, "not json").unwrap();
        write_setting_at(
            &path,
            "history_retention_days",
            crate::json::Value::Number(2.0),
        )
        .unwrap();
        assert_eq!(retention_days_at(&path), 2);
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
