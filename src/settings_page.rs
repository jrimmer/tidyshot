//! The settings pages, their writer and the managed shortcut block.
// Ported from omnishot/backend.py (Store settings) and omnishot/settings.py
// (pages, DEFAULTS) with the shortcut block scripts/install_user.py writes.
//!
//! Every page is declared here once. QML renders [`PAGES`] and sends a change
//! back by key; [`write_value`] merges it into the existing JSON and commits
//! the file atomically, so a crash never leaves a half-written settings file.
//! The runtime reads the same file before each capture, preview or recording,
//! so a change takes effect on the next action without a restart.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::json::Value;

/// A setting's value type, used by QML to pick a control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Bool,
    Number,
    Text,
    Choice,
}

impl FieldKind {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Number => "number",
            Self::Text => "text",
            Self::Choice => "choice",
        }
    }
}

/// One editable setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SettingField {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    pub default: FieldDefault,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    /// The accepted values for a [`FieldKind::Choice`] field.
    pub choices: &'static [&'static str],
    pub tooltip: &'static str,
    /// The change only takes effect after a restart.
    pub restart: bool,
}

/// A field's default value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldDefault {
    Bool(bool),
    Number(f64),
    Text(&'static str),
}

impl FieldDefault {
    #[must_use]
    pub fn value(self) -> Value {
        match self {
            Self::Bool(value) => Value::Bool(value),
            Self::Number(value) => Value::Number(value),
            Self::Text(value) => Value::String(value.to_owned()),
        }
    }
}

/// One settings page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SettingsPage {
    pub id: &'static str,
    pub title: &'static str,
    pub fields: &'static [SettingField],
}

const fn flag(key: &'static str, label: &'static str, default: bool) -> SettingField {
    SettingField {
        key,
        label,
        kind: FieldKind::Bool,
        default: FieldDefault::Bool(default),
        min: 0.0,
        max: 0.0,
        step: 0.0,
        choices: &[],
        tooltip: "",
        restart: false,
    }
}

const fn number(
    key: &'static str,
    label: &'static str,
    default: f64,
    min: f64,
    max: f64,
    step: f64,
) -> SettingField {
    SettingField {
        key,
        label,
        kind: FieldKind::Number,
        default: FieldDefault::Number(default),
        min,
        max,
        step,
        choices: &[],
        tooltip: "",
        restart: false,
    }
}

const fn text(key: &'static str, label: &'static str, default: &'static str) -> SettingField {
    SettingField {
        key,
        label,
        kind: FieldKind::Text,
        default: FieldDefault::Text(default),
        min: 0.0,
        max: 0.0,
        step: 0.0,
        choices: &[],
        tooltip: "",
        restart: false,
    }
}

const fn choice(
    key: &'static str,
    label: &'static str,
    default: &'static str,
    choices: &'static [&'static str],
) -> SettingField {
    SettingField {
        key,
        label,
        kind: FieldKind::Choice,
        default: FieldDefault::Text(default),
        min: 0.0,
        max: 0.0,
        step: 0.0,
        choices,
        tooltip: "",
        restart: false,
    }
}

/// Attach the Advanced page's restart reminder to a field.
const DECODER_TOOLTIP: &str = "Can play large videos more smoothly, but may stall the desktop when memory is low. Applies after TidyShot restarts.";

/// The ten settings pages, in the order the settings window lists them.
pub const PAGES: &[SettingsPage] = &[
    SettingsPage {
        id: "general",
        title: "General",
        fields: &[
            SettingField {
                tooltip: "Tokens: %y year, %m month, %d day, %H hour, %M minute, %S second, %i counter, %r random.",
                ..text(
                    "filename_format",
                    "Capture filename",
                    "TidyShot %y-%m-%d at %H.%M.%S",
                )
            },
            number(
                "capture_delay",
                "Self-timer delay (seconds)",
                3.0,
                0.0,
                60.0,
                1.0,
            ),
        ],
    },
    SettingsPage {
        id: "wallpaper",
        title: "Wallpaper",
        fields: &[
            flag(
                "window_wallpaper",
                "Use the desktop wallpaper behind windows",
                true,
            ),
            flag("window_shadow", "Draw a window shadow", true),
            number("window_padding", "Window padding", 48.0, 0.0, 240.0, 2.0),
            text("wallpaper_source", "Wallpaper source", ""),
        ],
    },
    SettingsPage {
        id: "screenshots",
        title: "Screenshots",
        fields: &[
            choice(
                "after_capture",
                "After a capture",
                "overlay",
                &["overlay", "copy", "save", "annotate", "pin"],
            ),
            flag(
                "capture_ctrl_copy",
                "Ctrl at selection copies the capture",
                true,
            ),
            flag(
                "capture_shortcut_actions",
                "Area shortcuts also run the configured actions",
                true,
            ),
            choice(
                "export_format",
                "Default export format",
                "png",
                &["png", "jpeg", "webp", "heic"],
            ),
            number("jpeg_quality", "JPEG quality", 90.0, 1.0, 100.0, 1.0),
            number("webp_quality", "WebP quality", 90.0, 1.0, 100.0, 1.0),
            number("heic_quality", "HEIC quality", 90.0, 1.0, 100.0, 1.0),
        ],
    },
    SettingsPage {
        id: "quickaccess",
        title: "Quick Access",
        fields: &[
            choice(
                "preview_corner",
                "Preview corner",
                "bottom-right",
                &["bottom-left", "bottom-right", "top-left", "top-right"],
            ),
            number(
                "preview_timeout",
                "Auto-close delay (seconds, 0 keeps it open)",
                0.0,
                0.0,
                60.0,
                0.5,
            ),
            choice(
                "preview_action",
                "When the delay elapses",
                "close",
                &["close", "save"],
            ),
            flag(
                "close_after_drag",
                "Close after dragging a capture out",
                true,
            ),
            number("preview_size", "Preview width", 355.0, 220.0, 900.0, 5.0),
            number("preview_margin", "Screen margin", 18.0, 0.0, 200.0, 1.0),
        ],
    },
    SettingsPage {
        id: "recording",
        title: "Recording",
        fields: &[
            choice("recording_format", "Format", "mp4", &["mp4", "gif"]),
            choice("recording_fps", "Frame rate", "60", &["24", "30", "60"]),
            choice(
                "recording_quality",
                "Quality",
                "balanced",
                &["high", "balanced", "small"],
            ),
            choice(
                "recording_size",
                "Resolution",
                "original",
                &["original", "1080", "720"],
            ),
            flag("recording_audio", "Record system audio", false),
            flag("recording_microphone", "Record the microphone", false),
            flag("recording_mono", "Mix audio to mono", false),
            flag("recording_cursor", "Show the cursor", true),
            flag(
                "recording_show_controls",
                "Show the controls while recording",
                true,
            ),
            flag("recording_show_time", "Show elapsed time", true),
            flag("recording_gif", "Record GIF", false),
            number(
                "recording_gif_quality",
                "GIF quality",
                80.0,
                1.0,
                100.0,
                1.0,
            ),
            number("recording_gif_width", "GIF width", 960.0, 0.0, 3840.0, 10.0),
        ],
    },
    SettingsPage {
        id: "annotate",
        title: "Annotate",
        fields: &[
            text("annotation_color", "Default annotation color", "#ff5b61"),
            number(
                "annotation_stroke",
                "Default stroke width",
                4.0,
                1.0,
                24.0,
                1.0,
            ),
            number(
                "annotation_font_size",
                "Default text size",
                20.0,
                10.0,
                96.0,
                1.0,
            ),
            choice(
                "arrow_style",
                "Default arrow style",
                "standard",
                &["standard", "hollow", "filled", "line"],
            ),
            number(
                "highlight_alpha",
                "Highlighter opacity",
                0.35,
                0.05,
                1.0,
                0.05,
            ),
        ],
    },
    SettingsPage {
        id: "text",
        title: "Text",
        fields: &[
            text("ocr_languages", "OCR languages (auto or eng+spa)", "auto"),
            flag("ocr_linebreaks", "Keep line breaks", true),
            flag("ocr_detect_links", "Detect links", true),
        ],
    },
    SettingsPage {
        id: "advanced",
        title: "Advanced",
        fields: &[
            SettingField {
                restart: true,
                tooltip: DECODER_TOOLTIP,
                ..flag("hardware_video_decoding", "Hardware video decoding", false)
            },
            number(
                "history_retention_days",
                "Keep history for (days)",
                30.0,
                1.0,
                31.0,
                1.0,
            ),
        ],
    },
    SettingsPage {
        id: "shortcuts",
        title: "Shortcuts",
        fields: &[],
    },
    SettingsPage {
        id: "about",
        title: "About",
        fields: &[],
    },
];

/// Look up a page by id.
#[must_use]
pub fn page(id: &str) -> Option<&'static SettingsPage> {
    PAGES.iter().find(|page| page.id == id)
}

/// Look up a field by its settings key.
#[must_use]
pub fn field(key: &str) -> Option<&'static SettingField> {
    PAGES
        .iter()
        .flat_map(|page| page.fields.iter())
        .find(|field| field.key == key)
}

/// The page a field belongs to.
#[must_use]
pub fn page_of(key: &str) -> Option<&'static SettingsPage> {
    PAGES
        .iter()
        .find(|page| page.fields.iter().any(|field| field.key == key))
}

/// Read the stored settings map from an explicit file.
///
/// A missing or malformed file is an empty map, matching the read-only store
/// the rest of the app already uses.
#[must_use]
pub fn read_settings(path: &Path) -> BTreeMap<String, Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    match crate::json::parse(&text) {
        Ok(Value::Object(object)) => object,
        _ => BTreeMap::new(),
    }
}

/// Serialize a settings map with the same two-space layout the sidecars use.
#[must_use]
pub fn settings_text(values: &BTreeMap<String, Value>) -> String {
    Value::Object(values.clone()).to_string_pretty() + "\n"
}

/// Commit a whole settings map atomically.
pub fn write_settings(path: &Path, values: &BTreeMap<String, Value>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::storage::atomic_write_file(path, settings_text(values).as_bytes())
}

/// The current value of `key`, defaulting to the schema default.
#[must_use]
pub fn value_of(values: &BTreeMap<String, Value>, key: &str) -> Value {
    if let Some(value) = values.get(key) {
        return value.clone();
    }
    field(key).map_or(Value::Null, |field| field.default.value())
}

/// Merge one change into the existing map and write it atomically.
///
/// Unknown keys already in the file are preserved, so a hand-edited setting
/// or a newer key is never dropped by the writer.
pub fn write_value(path: &Path, key: &str, value: Value) -> std::io::Result<()> {
    let field = field(key);
    let mut values = read_settings(path);
    values.insert(key.to_owned(), validate(field, value));
    write_settings(path, &values)
}

/// Coerce a change to the type and bounds the schema declares.
///
/// A value that does not match its field is kept as given so an unknown key
/// survives a round trip; a known field is clamped, which keeps a slider from
/// writing an out-of-range delay.
#[must_use]
pub fn validate(field: Option<&SettingField>, value: Value) -> Value {
    let Some(field) = field else {
        return value;
    };
    match (field.kind, value) {
        (FieldKind::Bool, Value::Bool(value)) => Value::Bool(value),
        (FieldKind::Number, Value::Number(value)) => {
            Value::Number(value.clamp(field.min, field.max))
        }
        (FieldKind::Text, Value::String(value)) => Value::String(value),
        (FieldKind::Choice, Value::String(value)) if field.choices.contains(&value.as_str()) => {
            Value::String(value)
        }
        // A value of the wrong type falls back to the default instead of
        // leaving the runtime to interpret a broken file.
        _ => field.default.value(),
    }
}

/// Write the settings file the app reads.
pub fn save_value(key: &str, value: Value) -> std::io::Result<()> {
    write_value(&crate::settings::settings_path(), key, value)
}

/// The live Quick Access settings, read from the same file the writer commits.
#[must_use]
pub fn preview_settings() -> crate::overlay::PreviewSettings {
    crate::overlay::preview_settings(&read_settings(&crate::settings::settings_path()))
}

/// The live OCR settings.
#[must_use]
pub fn ocr_settings() -> crate::text_result::TextSettings {
    crate::text_result::TextSettings::from_value(&Value::Object(read_settings(
        &crate::settings::settings_path(),
    )))
}

/// The live capture delay in seconds.
#[must_use]
pub fn capture_delay(values: &BTreeMap<String, Value>) -> i64 {
    values
        .get("capture_delay")
        .and_then(Value::as_f64)
        .map_or(3, |value| value.round() as i64)
}

/// The live capture filename pattern, empty meaning the built-in default.
#[must_use]
pub fn filename_format(values: &BTreeMap<String, Value>) -> String {
    values
        .get("filename_format")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(crate::filenames::DEFAULT_FORMAT)
        .to_owned()
}

/// The live annotation defaults: color and stroke width.
#[must_use]
pub fn annotate_defaults(values: &BTreeMap<String, Value>) -> (crate::color::Rgba, f64) {
    let color = values
        .get("annotation_color")
        .and_then(Value::as_str)
        .and_then(crate::color::Rgba::from_hex)
        .unwrap_or_else(|| crate::color::Rgba::new(0xff, 0x5b, 0x61, 255));
    let stroke = values
        .get("annotation_stroke")
        .and_then(Value::as_f64)
        .map_or(4.0, |value| value.clamp(1.0, 24.0));
    (color, stroke)
}

/// The managed shortcut block's start marker.
pub const BEGIN_MARKER: &str = "-- TidyShot managed bindings";
/// The managed shortcut block's end marker.
pub const END_MARKER: &str = "-- End TidyShot managed bindings";

/// One editable global shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    /// Stable id, used by the settings window.
    pub id: &'static str,
    pub label: &'static str,
    /// The default Hyprland key name.
    pub default_key: &'static str,
    /// The binding description Hyprland shows.
    pub description: &'static str,
    /// The tidyshot command the binding launches.
    pub command: &'static str,
}

/// The shortcuts the managed block owns, in install order.
pub const SHORTCUTS: &[Shortcut] = &[
    Shortcut {
        id: "print",
        label: "Capture menu",
        default_key: "PRINT",
        description: "TidyShot capture",
        command: "menu",
    },
    Shortcut {
        id: "area",
        label: "Capture area",
        default_key: "CTRL + PRINT",
        description: "TidyShot area",
        command: "area",
    },
    Shortcut {
        id: "fullscreen",
        label: "Capture display",
        default_key: "SHIFT + PRINT",
        description: "TidyShot fullscreen",
        command: "fullscreen",
    },
    Shortcut {
        id: "record",
        label: "Record screen",
        default_key: "ALT + PRINT",
        description: "TidyShot recording",
        command: "record",
    },
    Shortcut {
        id: "scroll",
        label: "Scrolling capture",
        default_key: "SUPER + SHIFT + PRINT",
        description: "TidyShot scrolling capture",
        command: "scroll",
    },
    Shortcut {
        id: "ocr",
        label: "Capture text",
        default_key: "SUPER + CTRL + PRINT",
        description: "TidyShot text capture",
        command: "ocr",
    },
];

/// Look up a shortcut by id.
#[must_use]
pub fn shortcut(id: &str) -> Option<&'static Shortcut> {
    SHORTCUTS.iter().find(|shortcut| shortcut.id == id)
}

/// The key currently bound for each shortcut, defaulting to the schema.
///
/// A managed block that has no line for a command keeps that shortcut's
/// default, so a partially installed block still renders every row.
#[must_use]
pub fn read_shortcuts(path: &Path) -> Vec<(&'static Shortcut, String)> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    SHORTCUTS
        .iter()
        .map(|shortcut| {
            let key = bound_key(&text, shortcut).unwrap_or_else(|| shortcut.default_key.to_owned());
            (shortcut, key)
        })
        .collect()
}

/// Find the key of the managed `o.bind` line for `shortcut`.
fn bound_key(text: &str, shortcut: &Shortcut) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("o.bind(") else {
            continue;
        };
        let Some(rest) = rest.strip_suffix(')') else {
            continue;
        };
        let parts = quoted_arguments(rest);
        if parts.len() == 3 && parts[2].ends_with(&format!(" {}", shortcut.command)) {
            return Some(parts[0].clone());
        }
    }
    None
}

/// Split a comma-separated list of double-quoted strings.
fn quoted_arguments(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut current: Option<String> = None;
    let mut escaped = false;
    for c in text.chars() {
        match (&mut current, c) {
            (None, '"') => current = Some(String::new()),
            (Some(value), '"') if !escaped => {
                values.push(std::mem::take(value));
                current = None;
            }
            (Some(value), '\\') if !escaped => escaped = true,
            (Some(value), c) => {
                value.push(c);
                escaped = false;
            }
            (None, _) => escaped = false,
        }
    }
    values
}

/// The launcher the managed block invokes.
///
/// The existing block's launcher wins so a checkout or native install keeps
/// pointing at the same executable after a shortcut edit.
#[must_use]
pub fn launcher_from(text: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("o.bind(") else {
            continue;
        };
        let Some(rest) = rest.strip_suffix(')') else {
            continue;
        };
        let parts = quoted_arguments(rest);
        if let Some(command) = parts.get(2)
            && let Some((prefix, _)) = command.rsplit_once(' ')
        {
            return prefix.to_owned();
        }
    }
    default_launcher()
}

/// The user launcher `scripts/install_user.py` installs.
#[must_use]
pub fn default_launcher() -> String {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    home.join(".local/bin/tidyshot").display().to_string()
}

/// Build the managed block for the given keys.
#[must_use]
pub fn managed_block(shortcuts: &[(&Shortcut, String)], launcher: &str) -> String {
    let mut out = String::new();
    out.push_str(BEGIN_MARKER);
    out.push('\n');
    for (shortcut, key) in shortcuts {
        out.push_str(&format!(
            "hl.unbind(\"{}\")\no.bind(\"{}\", \"{}\", \"{} {}\")\n",
            escape(key),
            escape(key),
            escape(shortcut.description),
            escape(launcher),
            shortcut.command
        ));
    }
    out.push_str(END_MARKER);
    out.push('\n');
    out
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Replace the managed block in `text`, preserving every unrelated line.
///
/// A block without the end marker (the layout the installer has written so
/// far) runs to the end of the file; rewriting it adds the end marker while
/// leaving nothing after it to lose.
#[must_use]
pub fn rewrite_bindings(text: &str, shortcuts: &[(&Shortcut, String)]) -> String {
    let launcher = launcher_from(text);
    let block = managed_block(shortcuts, &launcher);
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let begin = lines.iter().position(|line| line.trim() == BEGIN_MARKER);
    let Some(begin) = begin else {
        let mut out = text.to_owned();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&block);
        return out;
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(begin + 1)
        .find(|(_, line)| line.trim() == END_MARKER)
        .map(|(index, _)| index);
    let mut out = String::new();
    for line in &lines[..begin] {
        out.push_str(line);
    }
    out.push_str(&block);
    if let Some(end) = end {
        for line in &lines[end + 1..] {
            out.push_str(line);
        }
    }
    out
}

/// Rewrite the managed shortcut block on disk, after backing the file up.
///
/// Returns the backup path so the UI can report where the previous file
/// went. A nonexistent bindings file is created with just the managed block.
pub fn write_shortcuts(
    path: &Path,
    shortcuts: &[(&'static Shortcut, String)],
) -> std::io::Result<PathBuf> {
    let original = std::fs::read_to_string(path).unwrap_or_default();
    let backup = backup_file(path)?;
    let updated = rewrite_bindings(&original, shortcuts);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::storage::atomic_write_file(path, updated.as_bytes())?;
    Ok(backup)
}

/// Copy a configuration file beside itself with a timestamped name.
///
/// The copy happens before any write, so an interrupted edit always has the
/// previous file next to it. A missing file yields the would-be backup path
/// without creating anything.
pub fn backup_file(path: &Path) -> std::io::Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    let backup = path.with_file_name(format!("{name}.{stamp}.bak"));
    if path.is_file() {
        std::fs::copy(path, &backup)?;
    }
    Ok(backup)
}

/// The Hyprland directory holding `bindings.lua`.
///
/// `TIDYSHOT_HYPR_DIR` is authoritative, matching the installer and doctor;
/// without it the top-level config wins over the older nested layout.
#[must_use]
pub fn hypr_dir() -> PathBuf {
    if let Some(hinted) = std::env::var_os("TIDYSHOT_HYPR_DIR") {
        return PathBuf::from(hinted);
    }
    let config =
        std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home().join(".config"), PathBuf::from);
    let top = config.join("hypr");
    if top.join("bindings.lua").is_file() {
        return top;
    }
    let nested = config.join("omarchy/hypr");
    if nested.join("bindings.lua").is_file() {
        return nested;
    }
    top
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from)
}

/// The bindings file the shortcut editor edits.
#[must_use]
pub fn bindings_path() -> PathBuf {
    hypr_dir().join("bindings.lua")
}

/// The settings model QML renders, as one JSON document.
#[must_use]
pub fn model_json(
    values: &BTreeMap<String, Value>,
    shortcuts: &[(&Shortcut, String)],
    tab: &str,
    message: &str,
) -> String {
    let pages: Vec<Value> = PAGES
        .iter()
        .map(|page| {
            let fields: Vec<Value> = page
                .fields
                .iter()
                .map(|field| {
                    Value::Object(
                        [
                            ("key".to_owned(), Value::String(field.key.to_owned())),
                            ("label".to_owned(), Value::String(field.label.to_owned())),
                            ("kind".to_owned(), Value::String(field.kind.id().to_owned())),
                            ("value".to_owned(), value_of(values, field.key)),
                            ("min".to_owned(), Value::Number(field.min)),
                            ("max".to_owned(), Value::Number(field.max)),
                            ("step".to_owned(), Value::Number(field.step)),
                            (
                                "choices".to_owned(),
                                Value::Array(
                                    field
                                        .choices
                                        .iter()
                                        .map(|choice| Value::String((*choice).to_owned()))
                                        .collect(),
                                ),
                            ),
                            (
                                "tooltip".to_owned(),
                                Value::String(field.tooltip.to_owned()),
                            ),
                            ("restart".to_owned(), Value::Bool(field.restart)),
                        ]
                        .into_iter()
                        .collect(),
                    )
                })
                .collect();
            Value::Object(
                [
                    ("id".to_owned(), Value::String(page.id.to_owned())),
                    ("title".to_owned(), Value::String(page.title.to_owned())),
                    ("fields".to_owned(), Value::Array(fields)),
                ]
                .into_iter()
                .collect(),
            )
        })
        .collect();
    let shortcut_rows: Vec<Value> = shortcuts
        .iter()
        .map(|(shortcut, key)| {
            Value::Object(
                [
                    ("id".to_owned(), Value::String(shortcut.id.to_owned())),
                    ("label".to_owned(), Value::String(shortcut.label.to_owned())),
                    ("key".to_owned(), Value::String(key.clone())),
                    (
                        "defaultKey".to_owned(),
                        Value::String(shortcut.default_key.to_owned()),
                    ),
                ]
                .into_iter()
                .collect(),
            )
        })
        .collect();
    Value::Object(
        [
            ("pages".to_owned(), Value::Array(pages)),
            ("shortcuts".to_owned(), Value::Array(shortcut_rows)),
            ("tab".to_owned(), Value::String(tab.to_owned())),
            ("message".to_owned(), Value::String(message.to_owned())),
            (
                "uiScale".to_owned(),
                Value::Number(
                    crate::ui_scale::read(None)
                        .unwrap_or_default()
                        .factor()
                        .max(0.5),
                ),
            ),
        ]
        .into_iter()
        .collect(),
    )
    .to_string_compact()
}

// --- The QML singleton that owns the settings window ---------------------

/// A tab request that arrived before QML bound the window.
static PENDING: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
static BOUND: std::sync::OnceLock<cxx_qt::CxxQtThread<qobject::SettingsApi>> =
    std::sync::OnceLock::new();

/// Open the settings window on `tab`.
pub fn publish_open(tab: impl Into<String>) {
    let tab = tab.into();
    match BOUND.get() {
        Some(thread) => {
            let _ = thread.queue(move |mut settings| {
                settings.as_mut().apply_tab(tab);
            });
        }
        None => {
            *PENDING.lock().expect("settings queue poisoned") = Some(tab);
        }
    }
}

/// Close the settings window.
pub fn publish_close() {
    if let Some(thread) = BOUND.get() {
        let _ = thread.queue(move |mut settings| {
            settings.as_mut().close();
        });
    }
}

/// The settings singleton QML binds to.
#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        /// The settings window's model and writer.
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(bool, open)]
        #[qproperty(QString, tab)]
        #[qproperty(QString, model_json, cxx_name = "modelJson")]
        #[qproperty(QString, message)]
        #[cxx_name = "SettingsApi"]
        type SettingsApi = super::SettingsRust;

        /// Register this object and replay a queued tab request.
        #[qinvokable]
        fn bind(self: Pin<&mut Self>);

        /// Open on a page.
        #[qinvokable]
        #[cxx_name = "openTab"]
        fn open_tab(self: Pin<&mut Self>, tab: &QString);

        /// Write one setting by key; `value` is a JSON scalar.
        #[qinvokable]
        #[cxx_name = "setValue"]
        fn set_value(self: Pin<&mut Self>, key: &QString, value: &QString) -> bool;

        /// Write one managed shortcut key.
        #[qinvokable]
        #[cxx_name = "setShortcut"]
        fn set_shortcut(self: Pin<&mut Self>, id: &QString, key: &QString) -> bool;

        /// Restore every managed shortcut to its default key.
        #[qinvokable]
        #[cxx_name = "resetShortcuts"]
        fn reset_shortcuts(self: Pin<&mut Self>);

        /// Read the managed block again, reporting an error on the window.
        #[qinvokable]
        fn reload(self: Pin<&mut Self>);

        /// Close the window.
        #[qinvokable]
        fn close(self: Pin<&mut Self>);
    }

    impl cxx_qt::Threading for SettingsApi {}
}

use core::pin::Pin;
use cxx_qt::CxxQtType as _;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

/// Rust state behind the QML `SettingsApi` singleton.
pub struct SettingsRust {
    open: bool,
    tab: QString,
    model_json: QString,
    message: QString,
    shortcuts: Vec<(&'static Shortcut, String)>,
}

impl Default for SettingsRust {
    fn default() -> Self {
        let mut rust = Self {
            open: false,
            tab: QString::from("general"),
            model_json: QString::from(""),
            message: QString::from(""),
            shortcuts: read_shortcuts(&bindings_path()),
        };
        if let Some(tab) = PENDING.lock().expect("settings queue poisoned").take() {
            rust.tab = QString::from(tab);
            rust.open = true;
        }
        rust.refresh();
        rust
    }
}

impl SettingsRust {
    fn values(&self) -> BTreeMap<String, Value> {
        read_settings(&crate::settings::settings_path())
    }

    fn refresh(&mut self) {
        let values = self.values();
        self.model_json = QString::from(model_json(
            &values,
            &self.shortcuts,
            &self.tab.to_string(),
            &self.message.to_string(),
        ));
    }

    fn set_message(&mut self, message: impl Into<String>) {
        self.message = QString::from(message.into());
        self.refresh();
    }
}

impl qobject::SettingsApi {
    /// Copy every field onto the QObject properties.
    fn sync(mut self: Pin<&mut Self>) {
        let (open, tab, model, message) = {
            let rust = self.as_mut().rust_mut().get_mut();
            rust.refresh();
            (
                rust.open,
                rust.tab.clone(),
                rust.model_json.clone(),
                rust.message.clone(),
            )
        };
        self.as_mut().set_open(open);
        self.as_mut().set_tab(tab);
        self.as_mut().set_model_json(model);
        self.as_mut().set_message(message);
    }

    /// Register this object and replay a queued tab request.
    pub fn bind(mut self: Pin<&mut Self>) {
        let pending = PENDING.lock().expect("settings queue poisoned").take();
        let _ = BOUND.set(self.qt_thread());
        if let Some(tab) = pending {
            self.as_mut().rust_mut().get_mut().tab = QString::from(tab);
            self.as_mut().rust_mut().get_mut().open = true;
        }
        self.as_mut().sync();
    }

    /// Apply a tab request on the Qt thread.
    pub fn apply_tab(mut self: Pin<&mut Self>, tab: String) {
        {
            let rust = self.as_mut().rust_mut().get_mut();
            rust.tab = QString::from(tab);
            rust.open = true;
        }
        self.as_mut().sync();
    }

    /// Open on a page.
    pub fn open_tab(mut self: Pin<&mut Self>, tab: &QString) {
        self.as_mut().apply_tab(tab.to_string());
    }

    /// Write one setting by key.
    pub fn set_value(mut self: Pin<&mut Self>, key: &QString, value: &QString) -> bool {
        let key = key.to_string();
        let Ok(parsed) = crate::json::parse(&value.to_string()) else {
            self.as_mut()
                .rust_mut()
                .get_mut()
                .set_message("The value was not valid JSON");
            self.as_mut().sync();
            return false;
        };
        match save_value(&key, parsed) {
            Ok(()) => {
                let field = field(&key);
                let mut message =
                    format!("Saved {}", field.map_or(key.as_str(), |field| field.label));
                if field.is_some_and(|field| field.restart) {
                    message.push_str(" — applies after a restart");
                }
                self.as_mut().rust_mut().get_mut().set_message(message);
                self.as_mut().sync();
                true
            }
            Err(error) => {
                self.as_mut()
                    .rust_mut()
                    .get_mut()
                    .set_message(format!("Could not save the setting: {error}"));
                self.as_mut().sync();
                false
            }
        }
    }

    /// Write one managed shortcut key.
    pub fn set_shortcut(mut self: Pin<&mut Self>, id: &QString, key: &QString) -> bool {
        let id = id.to_string();
        let key = key.to_string().trim().to_owned();
        if key.is_empty() {
            self.as_mut()
                .rust_mut()
                .get_mut()
                .set_message("A shortcut needs a key");
            self.as_mut().sync();
            return false;
        }
        let Some(shortcut) = shortcut(&id) else {
            self.as_mut()
                .rust_mut()
                .get_mut()
                .set_message("Unknown shortcut");
            self.as_mut().sync();
            return false;
        };
        let mut shortcuts = self.as_mut().rust_mut().get_mut().shortcuts.clone();
        for (entry, entry_key) in &mut shortcuts {
            if entry.id == shortcut.id {
                *entry_key = key.clone();
            }
        }
        match write_shortcuts(&bindings_path(), &shortcuts) {
            Ok(backup) => {
                self.as_mut().rust_mut().get_mut().shortcuts = shortcuts;
                self.as_mut().rust_mut().get_mut().set_message(format!(
                    "Saved the shortcut; the previous bindings are at {}",
                    backup.display()
                ));
                self.as_mut().sync();
                true
            }
            Err(error) => {
                self.as_mut()
                    .rust_mut()
                    .get_mut()
                    .set_message(format!("Could not write the bindings file: {error}"));
                self.as_mut().sync();
                false
            }
        }
    }

    /// Restore every managed shortcut to its default key.
    pub fn reset_shortcuts(mut self: Pin<&mut Self>) {
        let shortcuts: Vec<(&'static Shortcut, String)> = SHORTCUTS
            .iter()
            .map(|shortcut| (shortcut, shortcut.default_key.to_owned()))
            .collect();
        match write_shortcuts(&bindings_path(), &shortcuts) {
            Ok(backup) => {
                self.as_mut().rust_mut().get_mut().shortcuts = shortcuts;
                self.as_mut().rust_mut().get_mut().set_message(format!(
                    "Restored the default shortcuts; the previous bindings are at {}",
                    backup.display()
                ));
            }
            Err(error) => {
                self.as_mut()
                    .rust_mut()
                    .get_mut()
                    .set_message(format!("Could not write the bindings file: {error}"));
            }
        }
        self.as_mut().sync();
    }

    /// Read the managed block again.
    pub fn reload(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().get_mut().shortcuts = read_shortcuts(&bindings_path());
        self.as_mut()
            .rust_mut()
            .get_mut()
            .set_message("Reloaded the shortcut bindings");
        self.as_mut().sync();
    }

    /// Close the window.
    pub fn close(mut self: Pin<&mut Self>) {
        self.as_mut().set_open(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tidyshot-settings-page-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn every_page_field_writes_and_reads_back() {
        let dir = scratch("roundtrip");
        let path = dir.join("settings.json");
        let mut expected = BTreeMap::new();
        for field in PAGES.iter().flat_map(|page| page.fields.iter()) {
            let value = match field.kind {
                FieldKind::Bool => Value::Bool(!matches!(field.default, FieldDefault::Bool(true))),
                FieldKind::Number => Value::Number(field.max.min(field.min + 7.0)),
                FieldKind::Text => Value::String(format!("value-{}", field.key)),
                FieldKind::Choice => Value::String(field.choices.last().unwrap().to_string()),
            };
            write_value(&path, field.key, value.clone()).unwrap();
            expected.insert(field.key.to_owned(), validate(Some(field), value));
        }
        let stored = read_settings(&path);
        for (key, value) in &expected {
            assert_eq!(stored.get(key), Some(value), "{key} did not round-trip");
        }
        // A hand-edited key the schema does not know survives a write.
        let mut merged = read_settings(&path);
        merged.insert("future_key".to_owned(), Value::String("kept".to_owned()));
        write_settings(&path, &merged).unwrap();
        write_value(&path, "capture_delay", Value::Number(9.0)).unwrap();
        assert_eq!(
            read_settings(&path)
                .get("future_key")
                .and_then(Value::as_str),
            Some("kept")
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with('\n'));
        assert!(text.contains("\"capture_delay\": 9"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writes_are_atomic_and_leave_no_temp_files() {
        let dir = scratch("atomic");
        let path = dir.join("settings.json");
        write_value(&path, "ocr_languages", Value::String("eng".to_owned())).unwrap();
        write_value(&path, "ocr_languages", Value::String("eng+spa".to_owned())).unwrap();
        let entries: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["settings.json".to_owned()]);
        assert_eq!(
            read_settings(&path)
                .get("ocr_languages")
                .and_then(Value::as_str),
            Some("eng+spa")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_values_fall_back_to_the_schema_default() {
        let delay = field("capture_delay").unwrap();
        assert_eq!(
            validate(Some(delay), Value::String("soon".to_owned())),
            Value::Number(3.0)
        );
        assert_eq!(
            validate(Some(delay), Value::Number(900.0)),
            Value::Number(60.0)
        );
        let corner = field("preview_corner").unwrap();
        assert_eq!(
            validate(Some(corner), Value::String("middle".to_owned())),
            Value::String("bottom-right".to_owned())
        );
        // An unknown key keeps the value so a future setting is never rewritten.
        assert_eq!(
            validate(None, Value::String("x".to_owned())),
            Value::String("x".to_owned())
        );
    }

    #[test]
    fn live_readers_see_a_written_value() {
        let dir = scratch("live");
        let path = dir.join("settings.json");
        write_value(&path, "capture_delay", Value::Number(12.0)).unwrap();
        write_value(
            &path,
            "filename_format",
            Value::String("shot %S".to_owned()),
        )
        .unwrap();
        write_value(
            &path,
            "annotation_color",
            Value::String("#112233".to_owned()),
        )
        .unwrap();
        let values = read_settings(&path);
        assert_eq!(capture_delay(&values), 12);
        assert_eq!(filename_format(&values), "shot %S");
        let (color, stroke) = annotate_defaults(&values);
        assert_eq!(color, crate::color::Rgba::new(0x11, 0x22, 0x33, 255));
        assert_eq!(stroke, 4.0);
        // The preview reader uses the same file the writer commits.
        let preview = crate::overlay::preview_settings(&values);
        assert_eq!(preview.corner, crate::overlay::Corner::BottomRight);
        let _ = std::fs::remove_dir_all(&dir);
    }

    const UNRELATED: &str = "hl.bind(\"SUPER + Q\", \"close window\")\nhl.unbind(\"SUPER + W\")\n";

    #[test]
    fn the_shortcut_rewrite_preserves_unrelated_lines_and_adds_an_end_marker() {
        let original = format!(
            "{UNRELATED}{BEGIN_MARKER}\no.bind(\"PRINT\", \"old\", \"/opt/tidyshot menu\")\n"
        );
        let shortcuts: Vec<(&'static Shortcut, String)> = SHORTCUTS
            .iter()
            .map(|shortcut| {
                let key = if shortcut.id == "area" {
                    "SUPER + A".to_owned()
                } else {
                    shortcut.default_key.to_owned()
                };
                (shortcut, key)
            })
            .collect();
        let rewritten = rewrite_bindings(&original, &shortcuts);
        assert!(rewritten.starts_with(UNRELATED), "{rewritten}");
        assert!(rewritten.contains(BEGIN_MARKER));
        assert!(rewritten.contains(END_MARKER));
        assert!(
            rewritten.contains("o.bind(\"SUPER + A\", \"TidyShot area\", \"/opt/tidyshot area\")")
        );
        assert!(!rewritten.contains("\"PRINT\", \"old\""));
        // A second rewrite replaces the block rather than appending another.
        let twice = rewrite_bindings(&rewritten, &shortcuts);
        assert_eq!(twice, rewritten);
    }

    #[test]
    fn lines_after_the_managed_block_are_kept() {
        let text = format!(
            "{BEGIN_MARKER}\no.bind(\"PRINT\", \"old\", \"/bin/tidyshot menu\")\n{END_MARKER}\n{UNRELATED}"
        );
        let shortcuts: Vec<(&'static Shortcut, String)> = SHORTCUTS
            .iter()
            .map(|shortcut| (shortcut, shortcut.default_key.to_owned()))
            .collect();
        let rewritten = rewrite_bindings(&text, &shortcuts);
        assert!(rewritten.ends_with(UNRELATED), "{rewritten}");
    }

    #[test]
    fn writing_shortcuts_backs_the_file_up_first() {
        let dir = scratch("shortcut-write");
        let path = dir.join("bindings.lua");
        std::fs::write(&path, UNRELATED).unwrap();
        let shortcuts: Vec<(&'static Shortcut, String)> = SHORTCUTS
            .iter()
            .map(|shortcut| (shortcut, shortcut.default_key.to_owned()))
            .collect();
        let backup = write_shortcuts(&path, &shortcuts).unwrap();
        assert!(backup.is_file(), "the previous file is copied aside");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), UNRELATED);
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.starts_with(UNRELATED));
        assert!(written.contains(BEGIN_MARKER));
        let read = read_shortcuts(&path);
        assert_eq!(read.len(), SHORTCUTS.len());
        assert_eq!(read[1].1, "CTRL + PRINT");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reading_a_partial_block_keeps_the_defaults() {
        let dir = scratch("shortcut-read");
        let path = dir.join("bindings.lua");
        std::fs::write(
            &path,
            format!(
                "{BEGIN_MARKER}\no.bind(\"SUPER + A\", \"TidyShot area\", \"/bin/tidyshot area\")\n"
            ),
        )
        .unwrap();
        let read = read_shortcuts(&path);
        assert_eq!(read[1].1, "SUPER + A");
        assert_eq!(read[0].1, SHORTCUTS[0].default_key);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_model_lists_every_page_and_shortcut() {
        let mut values = BTreeMap::new();
        values.insert(
            "preview_corner".to_owned(),
            Value::String("top-left".to_owned()),
        );
        let shortcuts: Vec<(&'static Shortcut, String)> = SHORTCUTS
            .iter()
            .map(|shortcut| (shortcut, shortcut.default_key.to_owned()))
            .collect();
        let model = model_json(&values, &shortcuts, "quickaccess", "");
        let parsed = crate::json::parse(&model).unwrap();
        assert_eq!(
            parsed
                .get("pages")
                .and_then(Value::as_array)
                .map(<[Value]>::len),
            Some(PAGES.len())
        );
        assert_eq!(
            parsed
                .get("shortcuts")
                .and_then(Value::as_array)
                .map(<[Value]>::len),
            Some(SHORTCUTS.len())
        );
        assert_eq!(
            parsed.get("tab").and_then(Value::as_str),
            Some("quickaccess")
        );
        for page in PAGES {
            assert!(model.contains(&format!("\"id\":\"{}\"", page.id)));
        }
    }
}
