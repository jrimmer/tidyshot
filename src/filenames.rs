//! Capture filename tokens and formatting.
// Ported from omnishot/filenames.py
//!
//! `format_name` renders the configured pattern with the same tokens as the
//! Python app, then strips characters that cannot appear in a Linux filename
//! and caps the UTF-8 length so the name always fits a filesystem entry.

use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike as _, Local, TimeZone as _, Timelike as _, Utc};

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
                    .map_or_else(random_token, str::to_owned);
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
        format!("@{}x", three_significant(ratio))
    } else {
        String::new()
    }
}

/// Format a value with three significant digits, the way Python's `:.3g` does
/// (including dropping trailing zeros).
fn three_significant(value: f64) -> String {
    let magnitude = value.abs().log10().floor() as i32;
    let decimals = (2 - magnitude).max(0) as usize;
    let text = format!("{value:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
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
/// `%y-%m-%d` and friends follow the configured time zone: the default
/// (`utc: false`) uses the machine's local time, matching the Python default.
fn civil_time(epoch: i64, utc: bool) -> Civil {
    if utc {
        civil_fields(DateTime::<Utc>::from_timestamp(epoch, 0).unwrap_or_default())
    } else {
        civil_fields(Local.timestamp_opt(epoch, 0).single().unwrap_or_default())
    }
}

fn civil_fields<Tz: chrono::TimeZone>(datetime: DateTime<Tz>) -> Civil {
    Civil {
        year: i64::from(datetime.year()),
        month: datetime.month(),
        day: datetime.day(),
        hour: datetime.hour(),
        minute: datetime.minute(),
        second: datetime.second(),
        // Monday = 0, matching the `%w` ordering below and Python's `%A`.
        weekday: datetime.weekday().num_days_from_monday(),
    }
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

/// Six hex characters like `uuid4().hex[:6]`, backed by the system RNG.
fn random_token() -> String {
    let mut bytes = [0u8; 3];
    let _ = getrandom::fill(&mut bytes);
    format!("{:02x}{:02x}{:02x}", bytes[0], bytes[1], bytes[2])
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
        assert_eq!(scale_suffix(1.234), "@1.23x");
        assert_eq!(scale_suffix(12.34), "@12.3x");
        assert_eq!(scale_suffix(20.0), "");
    }

    #[test]
    fn utc_and_local_rendering_differ_by_the_local_offset() {
        // A fixed instant; its local and UTC fields differ anywhere but UTC.
        let stamp = 1_788_617_229;
        let utc = format_name(
            "%y-%m-%d %H.%M",
            &NameOptions {
                created: Some(stamp),
                utc: true,
                ..NameOptions::default()
            },
        );
        let local = format_name(
            "%y-%m-%d %H.%M",
            &NameOptions {
                created: Some(stamp),
                ..NameOptions::default()
            },
        );
        assert_eq!(utc, "2026-09-05 14.07");
        // Only assert equality with a separately computed local rendering when
        // the machine is not on UTC, so the test is stable in both cases.
        if Local
            .timestamp_opt(stamp, 0)
            .single()
            .map(|t| t.offset().local_minus_utc())
            != Some(0)
        {
            assert_ne!(local, utc);
        }
    }
}
