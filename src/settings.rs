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
    ocr::model_dir()
        .parent()
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
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
    let Ok(text) = std::fs::read_to_string(settings_path()) else {
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
        // The lease has no settings file; the fallback must match defaults.
        if !settings_path().is_file() {
            let settings = ocr_settings();
            assert_eq!(settings, TextSettings::default());
            assert_eq!(settings.ocr_languages, ocr::AUTO);
        }
    }
}
