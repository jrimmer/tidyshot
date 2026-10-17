//! Omarchy UI units, separate from Wayland scale and image coordinates.
// Ported from omnishot/ui_scale.py
//!
//! The shell theme gives a base font size and a spacing scale. Every explicit
//! UI measurement is expressed in those units and multiplied by the current
//! factor before it reaches Qt, so a text-size change reflows the interface
//! without ever resampling captured pixels.

use std::fmt;
use std::path::{Path, PathBuf};

/// The active Omarchy appearance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Appearance {
    pub font_size: i32,
    pub spacing: f64,
    pub font_spacing: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            font_size: 12,
            spacing: 1.0,
            font_spacing: true,
        }
    }
}

impl Appearance {
    /// The multiplier applied to every explicit UI measurement.
    #[must_use]
    pub fn factor(&self) -> f64 {
        self.spacing
            * if self.font_spacing {
                f64::from(self.font_size) / 12.0
            } else {
                1.0
            }
    }
}

/// Why an appearance file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppearanceError {
    message: String,
}

impl fmt::Display for AppearanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AppearanceError {}

/// The built-in accent, used when the theme does not name one.
pub const DEFAULT_ACCENT: &str = "#FFD60A";

/// The theme file first, then the user override, matching [`paths`].
#[must_use]
pub fn paths() -> (PathBuf, PathBuf) {
    let config = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".config"));
    let state = std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/state"));
    paths_for(&config, &state)
}

/// The theme file for one config and state root.
///
/// The state-dir theme wins; the config dir is the documented fallback when
/// it is absent, so a machine that keeps its theme under the config dir still
/// follows the Omarchy appearance.
fn paths_for(config: &Path, state: &Path) -> (PathBuf, PathBuf) {
    let mut theme = state.join("omarchy/current/theme/shell.toml");
    if !theme.exists() {
        theme = config.join("omarchy/current/theme/shell.toml");
    }
    (theme, config.join("omarchy/shell.toml"))
}

fn home() -> PathBuf {
    std::env::var("HOME").map_or_else(|_| PathBuf::from("."), PathBuf::from)
}

/// Read the appearance from the theme file and the user override.
///
/// Returns `None` when an existing file cannot be parsed, matching the
/// source: a broken file leaves the previous appearance in place rather than
/// silently falling back to defaults.
// Ported from omnishot/ui_scale.py (read)
#[must_use]
pub fn read(candidates: Option<&[PathBuf]>) -> Option<Appearance> {
    let defaults = paths();
    let owned;
    let files: &[PathBuf] = match candidates {
        Some(candidates) => candidates,
        None => {
            owned = [defaults.0.clone(), defaults.1.clone()];
            &owned
        }
    };
    let mut font: Vec<(String, TomlValue)> = Vec::new();
    let mut spacing: Vec<(String, TomlValue)> = Vec::new();
    for path in files {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        let parsed = match parse_toml(&text) {
            Ok(parsed) => parsed,
            Err(_) => return None,
        };
        for (section, target) in [("font", &mut font), ("spacing", &mut spacing)] {
            for (key, value) in parsed
                .iter()
                .filter(|(name, _)| name == section)
                .flat_map(|(_, entries)| entries)
            {
                if let Some(existing) = target.iter_mut().find(|(name, _)| name == key) {
                    existing.1 = value.clone();
                } else {
                    target.push((key.clone(), value.clone()));
                }
            }
        }
    }

    let font_size = match get(&font, "base-size") {
        None => 12,
        Some(value) => match value.as_int() {
            Some(size) if size >= 1 => size as i32,
            // Python's max(1, int(...)) clamps non-positive sizes and fails
            // on a value int() cannot read.
            Some(_) => 1,
            None => return None,
        },
    };
    let factor = match get(&spacing, "scale") {
        None => 1.0,
        Some(value) => value.as_float()?,
    };
    if !factor.is_finite() || factor < 0.0 {
        return None;
    }
    let follows = match get(&spacing, "scale-with-font") {
        None => true,
        Some(value) => value.as_bool(),
    };
    Some(Appearance {
        font_size,
        spacing: factor,
        font_spacing: follows,
    })
}

fn get<'a>(entries: &'a [(String, TomlValue)], key: &str) -> Option<&'a TomlValue> {
    entries
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}

/// Round like Python's `round`, which uses banker's rounding.
///
/// Qt measurements rounded with half-away-from-zero would drift from the
/// source on exact `.5` values.
#[must_use]
pub fn py_round(value: f64) -> f64 {
    let floor = value.floor();
    let fraction = value - floor;
    if (fraction - 0.5).abs() < f64::EPSILON {
        if (floor as i64) % 2 == 0 {
            floor
        } else {
            floor + 1.0
        }
    } else {
        value.round()
    }
}

/// Scale an explicit measurement into pixels at the current factor.
#[must_use]
pub fn px(value: f64, factor: f64) -> i32 {
    py_round(value * factor) as i32
}

/// Scale a size, never below one pixel on either axis.
#[must_use]
pub fn size(width: f64, height: f64, factor: f64) -> (i32, i32) {
    (px(width, factor).max(1), px(height, factor).max(1))
}

/// Rewrite every `Npx` declaration in a stylesheet, font sizes following the
/// font factor and everything else the spacing factor.
// Ported from omnishot/ui_scale.py (stylesheet)
#[must_use]
pub fn stylesheet(text: &str, appearance: &Appearance) -> String {
    let mut out = String::with_capacity(text.len());
    for segment in text.split_inclusive([';', '{', '}']) {
        let delimiter = segment
            .chars()
            .last()
            .filter(|c| [';', '{', '}'].contains(c))
            .map(|c| c.to_string())
            .unwrap_or_default();
        let body = &segment[..segment.len() - delimiter.len()];
        if let Some((property, value)) = body.split_once(':') {
            let property = property.trim().to_owned();
            if !property.is_empty()
                && property
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
            {
                let factor = if property == "font-size" {
                    f64::from(appearance.font_size) / 12.0
                } else {
                    appearance.factor()
                };
                out.push_str(&property);
                out.push(':');
                out.push_str(&scale_pixels(value, factor));
                out.push_str(&delimiter);
                continue;
            }
        }
        out.push_str(segment);
    }
    out
}

/// Replace every `<number>px` in a value with its scaled pixel count.
fn scale_pixels(value: &str, factor: f64) -> String {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut number_start: Option<usize> = None;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_digit()
            || (byte == b'-' && number_start.is_none() && index + 1 < bytes.len())
        {
            if number_start.is_none() {
                number_start = Some(index);
            }
            index += 1;
            continue;
        }
        if byte == b'.' && number_start.is_some() {
            index += 1;
            continue;
        }
        if let Some(start) = number_start {
            if value[index..].starts_with("px")
                && let Ok(number) = value[start..index].parse::<f64>()
            {
                out.push_str(&(py_round(number * factor) as i64).to_string());
                out.push_str("px");
                index += 2;
                number_start = None;
                continue;
            }
            out.push_str(&value[start..index]);
            number_start = None;
        }
        out.push(byte as char);
        index += 1;
    }
    if let Some(start) = number_start {
        out.push_str(&value[start..]);
    }
    out
}

/// A value the minimal TOML reader understands.
#[derive(Debug, Clone, PartialEq)]
enum TomlValue {
    Integer(i64),
    Float(f64),
    Bool(bool),
    Text(String),
}

impl TomlValue {
    fn as_int(&self) -> Option<i64> {
        match self {
            // Python's int() truncates toward zero; a bool is 1 or 0.
            TomlValue::Integer(value) => Some(*value),
            TomlValue::Float(value) => Some(*value as i64),
            TomlValue::Bool(value) => Some(i64::from(*value)),
            TomlValue::Text(text) => text.trim().parse().ok(),
        }
    }

    fn as_float(&self) -> Option<f64> {
        match self {
            TomlValue::Integer(value) => Some(*value as f64),
            TomlValue::Float(value) => Some(*value),
            TomlValue::Text(text) => text.trim().parse().ok(),
            TomlValue::Bool(_) => None,
        }
    }

    fn as_bool(&self) -> bool {
        match self {
            TomlValue::Bool(value) => *value,
            // Python treats a non-string scale-with-font by truthiness.
            TomlValue::Integer(value) => *value != 0,
            TomlValue::Float(value) => *value != 0.0,
            // A string is false only for the documented false words;
            // anything else, including "enabled", is true.
            TomlValue::Text(text) => !matches!(
                text.trim().to_ascii_lowercase().as_str(),
                "false" | "0" | "no" | "off"
            ),
        }
    }
}

/// Parse the subset of TOML the shell theme uses: `[section]` headers and
/// `key = value` pairs with integers, floats, quoted strings or booleans.
type TomlSections = Vec<(String, Vec<(String, TomlValue)>)>;

fn parse_toml(text: &str) -> Result<TomlSections, AppearanceError> {
    // Values before the first header belong to the unnamed default section,
    // which is how Omarchy's `colors.toml` stores `accent`.
    let mut sections: TomlSections = vec![(String::new(), Vec::new())];
    let mut current: usize = 0;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            sections.push((name.trim().to_owned(), Vec::new()));
            current = sections.len() - 1;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(AppearanceError {
                message: format!("invalid TOML line: {line}"),
            });
        };
        sections[current]
            .1
            .push((key.trim().to_owned(), parse_value(value.trim())?));
    }
    Ok(sections)
}

fn parse_value(value: &str) -> Result<TomlValue, AppearanceError> {
    if value.is_empty() {
        return Err(AppearanceError {
            message: "empty TOML value".to_owned(),
        });
    }
    if let Some(text) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    {
        return Ok(TomlValue::Text(text.to_owned()));
    }
    match value {
        "true" => Ok(TomlValue::Bool(true)),
        "false" => Ok(TomlValue::Bool(false)),
        _ => {
            let normalized = value.replace('_', "");
            if let Ok(integer) = normalized.parse::<i64>() {
                return Ok(TomlValue::Integer(integer));
            }
            if let Ok(float) = normalized.parse::<f64>() {
                return Ok(TomlValue::Float(float));
            }
            Err(AppearanceError {
                message: format!("unsupported TOML value: {value}"),
            })
        }
    }
}

/// The theme files as a slice, for `read(None)`.
#[must_use]
pub fn theme_files() -> Vec<PathBuf> {
    let (theme, override_path) = paths();
    vec![theme, override_path]
}

/// The files that may carry the Omarchy accent, in precedence order.
///
/// Omarchy keeps the palette in `colors.toml` beside `shell.toml`; the
/// override file is last so a user edit wins.
#[must_use]
pub fn accent_files() -> Vec<PathBuf> {
    let (theme, override_path) = paths();
    let mut files = Vec::new();
    if let Some(dir) = theme.parent() {
        files.push(dir.join("colors.toml"));
    }
    files.push(theme);
    files.push(override_path);
    files
}

/// The active Omarchy accent as `#RRGGBB`.
#[must_use]
pub fn accent() -> String {
    accent_from(&accent_files()).unwrap_or_else(|| DEFAULT_ACCENT.to_owned())
}

/// The accent read from an explicit candidate list, for tests.
///
/// The last parseable file that names a valid `accent` color wins, so the
/// user override at the end of [`accent_files`] takes precedence. A
/// malformed file is skipped so one bad theme cannot hide the next.
#[must_use]
pub fn accent_from(files: &[PathBuf]) -> Option<String> {
    let mut found = None;
    for path in files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(parsed) = parse_toml(&text) else {
            continue;
        };
        if let Some(value) = accent_value(&parsed) {
            found = Some(value);
        }
    }
    found
}

/// Normalize the `accent` key from the parsed sections to `#RRGGBB`.
fn accent_value(sections: &TomlSections) -> Option<String> {
    for (name, entries) in sections {
        if !matches!(name.as_str(), "" | "colors" | "color" | "theme") {
            continue;
        }
        let Some(TomlValue::Text(text)) = get(entries, "accent") else {
            continue;
        };
        if let Some(color) = crate::color::Rgba::from_hex(text) {
            return Some(format!(
                "#{:02X}{:02X}{:02X}",
                color.red, color.green, color.blue
            ));
        }
    }
    None
}

/// The `[font] base-size` from a file, used by tests as a tiny parser probe.
#[must_use]
pub fn base_size(path: &Path) -> Option<i32> {
    let text = std::fs::read_to_string(path).ok()?;
    let parsed = parse_toml(&text).ok()?;
    let font = parsed.iter().find(|(name, _)| name == "font")?;
    let value = font.1.iter().find(|(name, _)| name == "base-size")?;
    value.1.as_int().map(|size| size.max(1) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tidyshot-ui-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // Ported from omnishot/test_ui_scale.py (test_shell_theme_and_user_override_merge)
    #[test]
    fn shell_theme_and_user_override_merge() {
        let dir = scratch("merge");
        let palette = dir.join("theme.toml");
        let override_file = dir.join("user.toml");
        std::fs::write(
            &palette,
            "[font]\nbase-size=14\n[spacing]\nscale=1.2\nscale-with-font=true\n",
        )
        .unwrap();
        std::fs::write(&override_file, "[font]\nbase-size=20\n").unwrap();
        let appearance = read(Some(&[palette.clone(), override_file.clone()])).unwrap();
        assert_eq!(appearance.font_size, 20);
        assert_eq!(appearance.factor(), 2.0);

        std::fs::write(&override_file, "[spacing]\nscale-with-font=false\n").unwrap();
        assert_eq!(
            read(Some(&[palette.clone(), override_file.clone()]))
                .unwrap()
                .factor(),
            1.2
        );

        std::fs::write(&override_file, "[font]\nbase-size=\n").unwrap();
        assert!(read(Some(&[palette, override_file])).is_none());
    }

    #[test]
    fn a_missing_file_is_skipped() {
        let dir = scratch("missing");
        let only = dir.join("theme.toml");
        std::fs::write(&only, "[font]\nbase-size=16\n").unwrap();
        let appearance = read(Some(&[dir.join("absent.toml"), only])).unwrap();
        assert_eq!(appearance.font_size, 16);
        assert_eq!(appearance.spacing, 1.0);
    }

    // Ported from omnishot/test_ui_scale.py (test_config_theme_is_the_fallback)
    #[test]
    fn the_config_theme_is_used_when_the_state_theme_is_absent() {
        let dir = scratch("fallback");
        let config = dir.join("config");
        let state = dir.join("state");
        let config_theme = config.join("omarchy/current/theme/shell.toml");
        std::fs::create_dir_all(config_theme.parent().unwrap()).unwrap();
        std::fs::write(&config_theme, "[font]\nbase-size=18\n").unwrap();
        let (theme, override_path) = paths_for(&config, &state);
        assert_eq!(theme, config_theme);
        assert_eq!(override_path, config.join("omarchy/shell.toml"));
        // A state theme wins over the config fallback.
        let state_theme = state.join("omarchy/current/theme/shell.toml");
        std::fs::create_dir_all(state_theme.parent().unwrap()).unwrap();
        std::fs::write(&state_theme, "[font]\nbase-size=22\n").unwrap();
        assert_eq!(paths_for(&config, &state).0, state_theme);
    }

    #[test]
    fn scale_with_font_accepts_non_boolean_values_like_python() {
        let dir = scratch("follows");
        let file = dir.join("theme.toml");
        // A numeric 1 and a non-false word are both true upstream.
        for value in ["1", "\"enabled\"", "\"yes\""] {
            std::fs::write(
                &file,
                format!("[spacing]\nscale=1.0\nscale-with-font={value}\n"),
            )
            .unwrap();
            assert!(
                read(Some(std::slice::from_ref(&file)))
                    .unwrap()
                    .font_spacing,
                "{value} must be true"
            );
        }
        for value in ["0", "\"false\"", "\"off\""] {
            std::fs::write(
                &file,
                format!("[spacing]\nscale=1.0\nscale-with-font={value}\n"),
            )
            .unwrap();
            assert!(
                !read(Some(std::slice::from_ref(&file)))
                    .unwrap()
                    .font_spacing,
                "{value} must be false"
            );
        }
    }

    #[test]
    fn accent_reads_the_default_section_used_by_omarchy_colors() {
        let dir = scratch("accent");
        let colors = dir.join("colors.toml");
        std::fs::write(&colors, "accent = \"#89b4fa\"\nbackground = \"#1e1e1e\"\n").unwrap();
        assert_eq!(accent_from(&[colors.clone()]), Some("#89B4FA".to_owned()));
        // An `accent` under a section is read too.
        std::fs::write(&colors, "[colors]\naccent = \"#ff5b61\"\n").unwrap();
        assert_eq!(accent_from(&[colors]), Some("#FF5B61".to_owned()));
    }

    #[test]
    fn accent_walks_candidates_and_lets_the_last_one_win() {
        let dir = scratch("accent-order");
        let broken = dir.join("broken.toml");
        let good = dir.join("good.toml");
        std::fs::write(&broken, "this is not toml\n").unwrap();
        std::fs::write(&good, "[colors]\naccent = \"#53d49b\"\n").unwrap();
        assert_eq!(
            accent_from(&[broken, good.clone()]),
            Some("#53D49B".to_owned())
        );
        let override_file = dir.join("override.toml");
        std::fs::write(&override_file, "[colors]\naccent = \"#ffffff\"\n").unwrap();
        assert_eq!(
            accent_from(&[good, override_file]),
            Some("#FFFFFF".to_owned())
        );
    }

    #[test]
    fn accent_falls_back_when_no_candidate_names_a_color() {
        let dir = scratch("accent-missing");
        let file = dir.join("colors.toml");
        std::fs::write(&file, "[colors]\naccent = \"not-a-color\"\n").unwrap();
        assert_eq!(accent_from(&[file]), None);
        assert_eq!(
            accent_from(&[dir.join("absent.toml")]),
            None,
            "a missing file is skipped"
        );
    }

    #[test]
    fn pixels_and_sizes_scale_and_never_reach_zero() {
        let appearance = Appearance {
            font_size: 20,
            spacing: 1.0,
            font_spacing: true,
        };
        let factor = appearance.factor();
        assert_eq!(px(165.0, factor), 275);
        assert_eq!(size(180.0, 90.0, factor), (300, 150));
        assert_eq!(size(0.0, 0.0, 3.0), (1, 1));
    }

    #[test]
    fn stylesheet_scales_font_sizes_by_the_font_factor_and_the_rest_by_spacing() {
        let appearance = Appearance {
            font_size: 20,
            spacing: 1.0,
            font_spacing: true,
        };
        let value = stylesheet("font-size:24px;padding:4px 6px;border:1px;", &appearance);
        assert_eq!(value, "font-size:40px;padding:7px 10px;border:2px;");
    }

    #[test]
    fn non_positive_spacing_is_rejected_but_zero_is_allowed() {
        let dir = scratch("spacing");
        let file = dir.join("theme.toml");
        std::fs::write(&file, "[spacing]\nscale=-1\n").unwrap();
        assert!(read(Some(std::slice::from_ref(&file))).is_none());
        std::fs::write(&file, "[spacing]\nscale=0\n").unwrap();
        assert_eq!(read(Some(&[file])).unwrap().factor(), 0.0);
    }

    #[test]
    fn banker_rounding_matches_python() {
        assert_eq!(py_round(0.5), 0.0);
        assert_eq!(py_round(1.5), 2.0);
        assert_eq!(py_round(2.5), 2.0);
        assert_eq!(py_round(-0.5), 0.0);
        assert_eq!(py_round(2.4), 2.0);
    }
}
