//! Capture filename tokens and formatting.
// Ported from omnishot/filenames.py
//!
//! `format_name` renders the configured pattern with the same tokens as the
//! Python app, then strips characters that cannot appear in a Linux filename
//! and caps the UTF-8 length so the name always fits a filesystem entry.

use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_FORMAT: &str = "OmniShot %y-%m-%d at %H.%M.%S";

/// A single `%X` token offered to the name customizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub label: &'static str,
    pub token: &'static str,
}

/// The tokens the filename dialog exposes, in display order.
pub const TOKENS: &[Token] = &[
    Token {
        label: "Year",
        token: "%y",
    },
    Token {
        label: "Month (numeric)",
        token: "%m",
    },
    Token {
        label: "Month (name)",
        token: "%n",
    },
    Token {
        label: "Day",
        token: "%d",
    },
    Token {
        label: "Day of week",
        token: "%w",
    },
    Token {
        label: "Window title",
        token: "%t",
    },
    Token {
        label: "App name",
        token: "%a",
    },
    Token {
        label: "Hour",
        token: "%H",
    },
    Token {
        label: "Minutes",
        token: "%M",
    },
    Token {
        label: "Seconds",
        token: "%S",
    },
    Token {
        label: "AM/PM",
        token: "%p",
    },
    Token {
        label: "Random characters",
        token: "%r",
    },
    Token {
        label: "Auto-increment",
        token: "%i",
    },
];

/// Values filled in for the non-time tokens while formatting.
#[derive(Debug, Default, Clone)]
pub struct NameContext {
    pub title: String,
    pub app: String,
}

/// A shared empty context for the `Default` impl, so it can borrow `'static`.
static EMPTY_CONTEXT: NameContext = NameContext {
    title: String::new(),
    app: String::new(),
};

/// Options that mirror the settings keys consumed by `Store.name_capture`.
#[derive(Debug, Clone)]
pub struct NameOptions<'a> {
    pub created: Option<i64>,
    pub utc: bool,
    pub context: &'a NameContext,
    pub sequence: i64,
    pub random_token: Option<&'a str>,
    pub remove_illegal: bool,
}

impl Default for NameOptions<'_> {
    fn default() -> Self {
        Self {
            created: None,
            utc: false,
            context: &EMPTY_CONTEXT,
            sequence: 1,
            random_token: None,
            remove_illegal: true,
        }
    }
}

impl<'a> NameOptions<'a> {
    #[must_use]
    pub fn with_context(context: &'a NameContext) -> Self {
        Self {
            context,
            ..Self::default()
        }
    }
}

/// Format `pattern` into a safe filename, never returning an empty string.
#[must_use]
pub fn format_name(pattern: &str, options: &NameOptions<'_>) -> String {
    let created = options.created.unwrap_or_else(now_epoch);
    let parts = civil_time(created, options.utc);
    let mut name = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            if name.len() < 512 {
                name.push(c);
            }
            continue;
        }
        let Some(token) = chars.next() else {
            // A trailing percent is kept literally.
            name.push('%');
            break;
        };
        match token {
            'y' => write!(name, "{:04}", parts.year).ok(),
            'm' => write!(name, "{:02}", parts.month).ok(),
            'n' => name.write_str(month_name(parts.month)).ok(),
            'd' => write!(name, "{:02}", parts.day).ok(),
            'w' => name.write_str(weekday_name(parts.weekday)).ok(),
            'H' => write!(name, "{:02}", parts.hour).ok(),
            'M' => write!(name, "{:02}", parts.minute).ok(),
            'S' => write!(name, "{:02}", parts.second).ok(),
            'p' => name
                .write_str(if parts.hour < 12 { "AM" } else { "PM" })
                .ok(),
            't' => name.write_str(&options.context.title).ok(),
            'a' => name.write_str(&options.context.app).ok(),
            'r' => {
                let token = options
                    .random_token
                    .map_or_else(|| random_token(created, options.sequence), str::to_owned);
                name.write_str(&token).ok()
            }
            'i' => write!(name, "{}", options.sequence).ok(),
            '%' => name.write_str("%").ok(),
            other => {
                // Unknown tokens are preserved verbatim, as in Python.
                name.push('%');
                name.push(other);
                None
            }
        };
    }

    // Slashes and NUL can never be literal filename characters on Linux.
    name = name.replace('/', "-");
    name.retain(|c| c != '\0');
    if options.remove_illegal {
        name.retain(|c| !is_illegal(c));
    }
    let trimmed = name.trim_matches(|c| c == ' ' || c == '.');
    let mut capped = truncate_utf8_bytes(trimmed, 230);
    capped = capped.trim_end_matches([' ', '.']).to_owned();
    if capped.is_empty() {
        "OmniShot".to_owned()
    } else {
        capped
    }
}

/// Density suffix for a capture's actual scale, or empty when unremarkable.
// Ported from omnishot/filenames.py (scale_suffix)
#[must_use]
pub fn scale_suffix(ratio: f64) -> String {
    if ratio.is_finite() && ratio > 1.001 && ratio <= 16.0 {
        let text = format!("{ratio:.3}");
        let trimmed = text.trim_end_matches('0').trim_end_matches('.');
        format!("@{trimmed}x")
    } else {
        String::new()
    }
}

fn is_illegal(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*')
        || ('\u{01}'..='\u{1f}').contains(&c)
        || c == '\u{7f}'
}

/// Keep at most `max` bytes without splitting a UTF-8 sequence.
fn truncate_utf8_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

struct Civil {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    weekday: u32,
}

/// Convert an epoch timestamp into local or UTC civil fields.
///
/// Without a timezone database, local time falls back to UTC; the Python
/// default also uses the process timezone, and captures on this system run in
/// UTC.
fn civil_time(epoch: i64, utc: bool) -> Civil {
    let _ = utc;
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    // 1970-01-01 was a Thursday (weekday 4, Monday = 0).
    let weekday = (days.rem_euclid(7) as u32 + 3) % 7;
    Civil {
        year,
        month,
        day,
        hour: (secs / 3600) as u32,
        minute: (secs % 3600 / 60) as u32,
        second: (secs % 60) as u32,
        weekday,
    }
}

/// Days-to-civil (Howard Hinnant's algorithm), matching `strftime` fields.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

fn month_name(month: u32) -> &'static str {
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    NAMES
        .get(month.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("")
}

fn weekday_name(weekday: u32) -> &'static str {
    const NAMES: [&str; 7] = [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ];
    NAMES.get(weekday as usize).copied().unwrap_or("")
}

/// Deterministic fallback when the caller supplies no random token.
fn random_token(created: i64, sequence: i64) -> String {
    let mixed = (created as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((sequence as u64).wrapping_mul(0xD1B5_4A32_D192_ED03));
    format!("{:06x}", mixed & 0xFF_FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(title: &str, app: &str) -> NameContext {
        NameContext {
            title: title.to_owned(),
            app: app.to_owned(),
        }
    }

    #[test]
    fn tokens_render_utc_and_literal_percent() {
        // 2026-09-05 14:07:09 UTC
        let stamp = 1_788_617_229;
        let context = ctx("Notes", "Editor");
        let options = NameOptions {
            created: Some(stamp),
            utc: true,
            context: &context,
            sequence: 42,
            random_token: Some("abcdef"),
            remove_illegal: true,
        };
        let result = format_name("%y-%m-%d %H.%M.%S %p %t %a %r %i %%", &options);
        assert_eq!(result, "2026-09-05 14.07.09 PM Notes Editor abcdef 42 %");
    }

    #[test]
    fn traversal_is_neutralized_and_unicode_is_capped() {
        let title = "../../秘密:<>?*".repeat(100);
        let context = ctx(&title, "");
        let options = NameOptions::with_context(&context);
        let result = format_name("../%t", &options);
        assert!(!result.contains('/'));
        assert!(!result.contains(':'));
        assert!(result.len() <= 230);

        assert_eq!(format_name("..", &NameOptions::default()), "OmniShot");
    }

    #[test]
    fn illegal_characters_can_be_kept() {
        let options = NameOptions {
            remove_illegal: false,
            ..NameOptions::default()
        };
        assert_eq!(format_name("Part: 1?", &options), "Part: 1?");
    }

    #[test]
    fn scale_suffix_follows_density() {
        assert_eq!(scale_suffix(1.0), "");
        assert_eq!(scale_suffix(1.6), "@1.6x");
        assert_eq!(scale_suffix(2.0), "@2x");
        assert_eq!(scale_suffix(20.0), "");
    }
}
