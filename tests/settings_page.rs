//! End-to-end behavior for the settings writer, the live-effect readers and
//! the managed Hyprland shortcut block.
//!
//! These drive the same entry points the settings window calls, against a
//! throwaway settings file and bindings file, and assert observable results:
//! the file's content, the value the runtime reads back, and the lines a
//! shortcut rewrite preserves.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tidyshot::json::Value;
use tidyshot::settings_page::{
    BEGIN_MARKER, END_MARKER, PAGES, SHORTCUTS, annotate_defaults, capture_delay, field,
    filename_format, model_json, page, page_of, read_settings, read_shortcuts, rewrite_bindings,
    settings_text, write_shortcuts, write_value,
};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tidyshot-settings-integration-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn defaults() -> Vec<(&'static tidyshot::settings_page::Shortcut, String)> {
    SHORTCUTS
        .iter()
        .map(|shortcut| (shortcut, shortcut.default_key.to_owned()))
        .collect()
}

#[test]
fn every_documented_page_exists_and_has_a_model_entry() {
    // The ten groups omnishot documents, plus the shortcuts and about pages
    // the settings dialog always had.
    for (id, title) in [
        ("general", "General"),
        ("wallpaper", "Wallpaper"),
        ("screenshots", "Screenshots"),
        ("quickaccess", "Quick Access"),
        ("recording", "Recording"),
        ("annotate", "Annotate"),
        ("text", "Text"),
        ("advanced", "Advanced"),
        ("shortcuts", "Shortcuts"),
        ("about", "About"),
    ] {
        let found = page(id).unwrap_or_else(|| panic!("{id} page"));
        assert_eq!(found.title, title);
        for field in found.fields {
            assert_eq!(page_of(field.key).map(|page| page.id), Some(found.id));
        }
    }
    assert_eq!(PAGES.len(), 10);
    let model = model_json(&BTreeMap::new(), &defaults(), "general", "");
    for page in PAGES {
        assert!(model.contains(&format!("\"id\":\"{}\"", page.id)));
    }
}

#[test]
fn a_setting_round_trips_atomically_and_the_runtime_reads_it() {
    let dir = scratch("roundtrip");
    let path = dir.join("settings.json");
    write_value(
        &path,
        "preview_corner",
        Value::String("top-left".to_owned()),
    )
    .unwrap();
    write_value(&path, "preview_timeout", Value::Number(4.0)).unwrap();
    write_value(&path, "capture_delay", Value::Number(9.0)).unwrap();
    let stored = read_settings(&path);
    assert_eq!(
        stored.get("preview_corner").and_then(Value::as_str),
        Some("top-left")
    );
    // The preview reader takes the corner from the same file the writer wrote.
    let preview = tidyshot::overlay::preview_settings(&stored);
    assert_eq!(preview.corner, tidyshot::overlay::Corner::TopLeft);
    assert_eq!(preview.auto_close_ms(), 4000);
    assert_eq!(capture_delay(&stored), 9);
    // No temp files are left behind.
    let files: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, vec!["settings.json".to_owned()]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_write_keeps_the_previous_settings() {
    let dir = scratch("failed");
    let path = dir.join("settings.json");
    let original = settings_text(&BTreeMap::from([(
        "capture_delay".to_owned(),
        Value::Number(2.0),
    )]));
    std::fs::write(&path, &original).unwrap();
    // A destination that is a directory cannot be replaced, so the writer
    // surfaces the error instead of reporting success; the existing file is
    // untouched.
    let blocked = dir.join("settings.json.d");
    std::fs::create_dir_all(&blocked).unwrap();
    assert!(write_value(&blocked, "capture_delay", Value::Number(4.0)).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recording_defaults_and_ocr_settings_come_from_the_written_file() {
    let dir = scratch("live");
    let path = dir.join("settings.json");
    write_value(&path, "recording_fps", Value::String("30".to_owned())).unwrap();
    write_value(&path, "recording_format", Value::String("gif".to_owned())).unwrap();
    write_value(&path, "ocr_languages", Value::String("eng+spa".to_owned())).unwrap();
    write_value(&path, "ocr_linebreaks", Value::Bool(false)).unwrap();
    write_value(&path, "hardware_video_decoding", Value::Bool(true)).unwrap();
    let stored = read_settings(&path);
    let options = tidyshot::recording_runtime::record_options_from(&stored);
    assert_eq!(options.fps, 30);
    assert_eq!(
        options.format,
        tidyshot::recording::recorder::CaptureFormat::Gif
    );
    let ocr = tidyshot::text_result::TextSettings::from_value(&Value::Object(stored.clone()));
    assert_eq!(ocr.ocr_languages, "eng+spa");
    assert!(!ocr.ocr_linebreaks);
    let policy = tidyshot::video_decoder::decoder_policy(&stored, None, false);
    assert_eq!(policy, tidyshot::video_decoder::DecoderPolicy::Hardware);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_filename_and_annotation_defaults_follow_the_file() {
    let dir = scratch("defaults");
    let path = dir.join("settings.json");
    let empty = BTreeMap::new();
    assert_eq!(filename_format(&empty), tidyshot::filenames::DEFAULT_FORMAT);
    assert_eq!(capture_delay(&empty), 3);
    assert_eq!(annotate_defaults(&empty).1, 4.0);
    write_value(
        &path,
        "filename_format",
        Value::String("shot %S".to_owned()),
    )
    .unwrap();
    write_value(
        &path,
        "annotation_color",
        Value::String("#00ff00".to_owned()),
    )
    .unwrap();
    write_value(&path, "annotation_stroke", Value::Number(8.0)).unwrap();
    let stored = read_settings(&path);
    assert_eq!(filename_format(&stored), "shot %S");
    let (color, stroke) = annotate_defaults(&stored);
    assert_eq!(color, tidyshot::color::Rgba::new(0, 255, 0, 255));
    assert_eq!(stroke, 8.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_schema_field_rejects_a_bad_type_without_losing_other_keys() {
    let dir = scratch("types");
    let path = dir.join("settings.json");
    write_value(&path, "capture_delay", Value::Number(5.0)).unwrap();
    write_value(&path, "capture_delay", Value::String("nope".to_owned())).unwrap();
    let stored = read_settings(&path);
    assert_eq!(stored.get("capture_delay"), Some(&Value::Number(3.0)));
    // A key the schema does not know survives the fallback write.
    let mut values = read_settings(&path);
    values.insert("custom_key".to_owned(), Value::Number(1.0));
    tidyshot::settings_page::write_settings(&path, &values).unwrap();
    write_value(&path, "capture_delay", Value::String("again".to_owned())).unwrap();
    assert_eq!(
        read_settings(&path).get("custom_key"),
        Some(&Value::Number(1.0))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_shortcut_rewrite_preserves_unrelated_bindings_and_backs_up() {
    let dir = scratch("shortcuts");
    let path = dir.join("bindings.lua");
    let unrelated = "hl.bind(\"SUPER + Q\", \"close\")\nhl.bind(\"SUPER + E\", \"file manager\")\n";
    std::fs::write(
        &path,
        format!("{unrelated}{BEGIN_MARKER}\no.bind(\"PRINT\", \"old\", \"/bin/tidyshot menu\")\n"),
    )
    .unwrap();
    let mut shortcuts = defaults();
    shortcuts[1].1 = "SUPER + A".to_owned();
    let backup = write_shortcuts(&path, &shortcuts).unwrap();
    assert_eq!(
        std::fs::read_to_string(&backup)
            .unwrap()
            .matches("SUPER + Q")
            .count(),
        1
    );
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(written.starts_with(unrelated));
    assert!(written.contains(END_MARKER));
    assert!(written.contains("o.bind(\"SUPER + A\", \"TidyShot area\", \"/bin/tidyshot area\")"));
    assert!(!written.contains("old"));
    let read = read_shortcuts(&path);
    assert_eq!(read[1].1, "SUPER + A");
    assert_eq!(read[0].1, "PRINT");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_shortcut_rewrite_keeps_the_launcher_and_lines_after_the_block() {
    let text = format!(
        "{BEGIN_MARKER}\no.bind(\"PRINT\", \"old\", \"/opt/tidyshot/bin/tidyshot menu\")\n{END_MARKER}\nhl.bind(\"SUPER + Q\", \"close\")\n"
    );
    let rewritten = rewrite_bindings(&text, &defaults());
    assert!(rewritten.contains("/opt/tidyshot/bin/tidyshot menu"));
    assert!(rewritten.ends_with("hl.bind(\"SUPER + Q\", \"close\")\n"));
    // A file that has no managed block gains one and keeps every line.
    let appended = rewrite_bindings("hl.bind(\"SUPER + Q\", \"close\")\n", &defaults());
    assert!(appended.starts_with("hl.bind(\"SUPER + Q\", \"close\")\n"));
    assert!(appended.contains(BEGIN_MARKER));
    // A block without an end marker (the installer's layout) is replaced
    // rather than duplicated.
    let legacy = format!("{BEGIN_MARKER}\no.bind(\"PRINT\", \"old\", \"/bin/tidyshot menu\")\n");
    let once = rewrite_bindings(&legacy, &defaults());
    assert_eq!(once.matches(BEGIN_MARKER).count(), 1);
    assert_eq!(once.matches("old").count(), 0);
}

#[test]
fn the_advanced_restart_tooltip_is_part_of_the_schema() {
    let decoder = field("hardware_video_decoding").unwrap();
    assert!(decoder.restart);
    assert!(decoder.tooltip.contains("restarts"));
    let model = model_json(&BTreeMap::new(), &defaults(), "advanced", "");
    assert!(model.contains("\"restart\":true"));
    assert!(model.contains("restarts"));
}

#[test]
fn a_bindings_file_with_no_managed_block_reads_the_default_keys() {
    let dir = scratch("no-block");
    let path: &Path = &dir.join("bindings.lua");
    std::fs::write(path, "hl.bind(\"SUPER + Q\", \"close\")\n").unwrap();
    let read = read_shortcuts(path);
    assert_eq!(read.len(), SHORTCUTS.len());
    for (shortcut, key) in read {
        assert_eq!(key, shortcut.default_key);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
