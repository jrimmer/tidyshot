//! Local `omnishot://` / `cleanshot://` URL commands.
// Ported from omnishot/api.py
//!
//! These are the capture actions the desktop entries and shell plugin can
//! invoke without a cloud service. Every cloud upload action is rejected, as
//! upstream does.

use std::collections::BTreeMap;
use std::fmt;

use crate::geometry::{MonitorGeometry, Rect};

/// Aliases from a URL host to the internal command name.
const ALIASES: &[(&str, &str)] = &[
    ("all-in-one", "select"),
    ("capture-area", "area"),
    ("capture-fullscreen", "fullscreen"),
    ("capture-window", "window"),
    ("capture-previous-area", "previous"),
    ("scrolling-capture", "scroll"),
    ("open-annotate", "open"),
    ("open-from-clipboard", "clipboard"),
    ("capture-text", "ocr"),
    ("open-history", "history"),
    ("restore-recently-closed", "restore"),
    ("open-settings", "settings"),
    ("record-screen", "record"),
    ("self-timer", "timer"),
    ("add-quick-access-overlay", "overlay"),
];

/// After-capture actions the URL API accepts; everything else (including the
/// cloud upload actions) is refused.
const ALLOWED_ACTIONS: &[&str] = &["overlay", "copy", "save", "annotate", "pin"];

const MAX_COORDINATE: i64 = 1_000_000;

/// A URL command that failed to parse or resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    message: String,
}

impl ApiError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

/// A point in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// Integer capture coordinates carried in the URL query.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Coordinates {
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub display: Option<i32>,
}

/// The parsed form of a local URL command.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UrlCommand {
    pub command: String,
    pub via_url: bool,
    pub path: Option<String>,
    pub action: Option<String>,
    pub tab: Option<String>,
    pub start: Option<bool>,
    pub autoscroll: Option<bool>,
    pub linebreaks: Option<bool>,
    pub coordinates: Option<Coordinates>,
}

/// Parse an `omnishot://` or `cleanshot://` URL.
// Ported from omnishot/api.py (parse_url)
pub fn parse_url(value: &str) -> Result<UrlCommand, ApiError> {
    let (scheme, rest) = value
        .split_once(':')
        .ok_or_else(|| ApiError::new("Unsupported URL scheme"))?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "omnishot" && scheme != "cleanshot" {
        return Err(ApiError::new("Unsupported URL scheme"));
    }

    // Authority and query; a fragment after `#` is ignored as urlparse does.
    let rest = rest.split('#').next().unwrap_or("");
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    let (authority, query) = match rest.split_once('?') {
        Some((authority, query)) => (authority, query),
        None => (rest, ""),
    };
    // A path after the host is not a command host; keep only the host.
    let host = authority.split('/').next().unwrap_or("");

    let query = parse_query(query);
    let command = ALIASES
        .iter()
        .find(|(alias, _)| *alias == host)
        .map_or_else(|| host.to_owned(), |(_, command)| (*command).to_owned());

    let mut parsed = UrlCommand {
        command,
        via_url: true,
        ..UrlCommand::default()
    };

    parsed.path = first(&query, "filepath").cloned();
    parsed.action = first(&query, "action").cloned();
    parsed.tab = first(&query, "tab").cloned();
    if let Some(action) = &parsed.action
        && !ALLOWED_ACTIONS.contains(&action.as_str())
    {
        return Err(ApiError::new(
            "Unsupported after-capture action; cloud uploads are excluded",
        ));
    }

    for (field, slot) in [("start", 0usize), ("autoscroll", 1), ("linebreaks", 2)] {
        if let Some(value) = first(&query, field) {
            let value = value.to_ascii_lowercase();
            let flag = match value.as_str() {
                "true" => true,
                "false" => false,
                _ => return Err(ApiError::new(format!("{field} must be true or false"))),
            };
            match slot {
                0 => parsed.start = Some(flag),
                1 => parsed.autoscroll = Some(flag),
                _ => parsed.linebreaks = Some(flag),
            }
        }
    }

    let mut coordinates = Coordinates::default();
    let mut has_coordinates = false;
    for (field, slot) in [
        ("x", 0usize),
        ("y", 1),
        ("width", 2),
        ("height", 3),
        ("display", 4),
    ] {
        if let Some(raw) = first(&query, field) {
            let number: i64 = raw
                .parse()
                .map_err(|_| ApiError::new("Capture coordinates are out of range"))?;
            if number.abs() > MAX_COORDINATE {
                return Err(ApiError::new("Capture coordinates are out of range"));
            }
            let number = number as i32;
            match slot {
                0 => coordinates.x = Some(number),
                1 => coordinates.y = Some(number),
                2 => coordinates.width = Some(number),
                3 => coordinates.height = Some(number),
                _ => coordinates.display = Some(number),
            }
            has_coordinates = true;
        }
    }
    if has_coordinates {
        parsed.coordinates = Some(coordinates);
    }

    Ok(parsed)
}

/// Resolve URL coordinates to a logical rectangle on a connected display.
///
/// With no display named the display under the cursor is used, falling back
/// to the first. The vertical axis has a lower-left origin in the URL, so it
/// is flipped into the compositor's upper-left origin.
// Ported from omnishot/api.py (url_geometry)
pub fn url_geometry(
    coordinates: &Coordinates,
    monitors: &[MonitorGeometry],
    cursor: Point,
) -> Result<Rect, ApiError> {
    if monitors.is_empty() {
        return Err(ApiError::new("That display is not connected"));
    }
    let bounds: Vec<Rect> = monitors.iter().map(MonitorGeometry::bounds).collect();
    let selected = match coordinates.display {
        Some(index) => {
            if index < 1 || index as usize > monitors.len() {
                return Err(ApiError::new("That display is not connected"));
            }
            index as usize - 1
        }
        None => bounds
            .iter()
            .position(|b| {
                cursor.x >= b.x && cursor.x < b.right() && cursor.y >= b.y && cursor.y < b.bottom()
            })
            .unwrap_or(0),
    };
    let monitor = bounds[selected];
    let width = coordinates.width.unwrap_or_else(|| 640.min(monitor.width));
    let height = coordinates
        .height
        .unwrap_or_else(|| 400.min(monitor.height));
    let x = coordinates.x.unwrap_or(0);
    let y = coordinates.y.unwrap_or(0);
    if width < 2
        || height < 2
        || x < 0
        || y < 0
        || x + width > monitor.width
        || y + height > monitor.height
    {
        return Err(ApiError::new("The requested area is outside the display"));
    }
    Ok(Rect::new(
        monitor.x + x,
        monitor.y + monitor.height - y - height,
        width,
        height,
    ))
}

/// Parse one query string into decoded key/value pairs, keeping repeats.
fn parse_query(query: &str) -> BTreeMap<String, Vec<String>> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.split_once('=') {
            Some((key, value)) => (key, Some(value)),
            None => (pair, None),
        };
        // `parse_qs` drops blank values by default.
        let Some(value) = value else { continue };
        if value.is_empty() {
            continue;
        }
        let key = percent_decode(key);
        let value = percent_decode(value);
        map.entry(key).or_default().push(value);
    }
    map
}

fn first<'a>(query: &'a BTreeMap<String, Vec<String>>, key: &str) -> Option<&'a String> {
    query.get(key).and_then(|values| values.first())
}

/// Decode `%XX` escapes and turn `+` into a space, decoding as UTF-8.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    None => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(x: i32, y: i32, width: i32, height: i32, scale: f64) -> MonitorGeometry {
        MonitorGeometry {
            x,
            y,
            width,
            height,
            scale,
            transform: 0,
        }
    }

    #[test]
    fn url_coordinates_actions_and_booleans() {
        let args = parse_url(
            "omnishot://scrolling-capture?x=100&y=120&width=200&height=150&display=1&start=true&autoscroll=false",
        )
        .unwrap();
        assert_eq!(args.command, "scroll");
        assert_eq!(args.start, Some(true));
        assert_eq!(args.autoscroll, Some(false));
        let monitors = [monitor(-1920, 0, 2880, 1800, 1.5)];
        let coords = args.coordinates.unwrap();
        assert_eq!(
            url_geometry(&coords, &monitors, Point { x: 0, y: 0 }),
            Ok(Rect::new(-1820, 930, 200, 150))
        );
        assert!(
            url_geometry(
                &Coordinates {
                    width: Some(9999),
                    ..Coordinates::default()
                },
                &monitors,
                Point { x: 0, y: 0 }
            )
            .is_err()
        );
        assert!(parse_url("omnishot://capture-area?action=upload").is_err());
        assert_eq!(
            parse_url("cleanshot://open-annotate?filepath=%2Ftmp%2Fmy%20image.png")
                .unwrap()
                .path
                .as_deref(),
            Some("/tmp/my image.png")
        );
    }

    #[test]
    fn url_coordinates_respect_portrait_rotation_and_cursor_display() {
        let displays = [
            monitor(-1920, 0, 1920, 1080, 1.0),
            MonitorGeometry {
                transform: 1,
                ..monitor(0, 0, 1920, 1080, 1.0)
            },
        ];
        let coordinates = Coordinates {
            x: Some(20),
            y: Some(30),
            width: Some(400),
            height: Some(600),
            display: None,
        };
        assert_eq!(
            url_geometry(&coordinates, &displays, Point { x: 500, y: 1700 }),
            Ok(Rect::new(20, 1290, 400, 600))
        );
        let outside = Coordinates {
            x: Some(1000),
            y: Some(0),
            width: Some(400),
            height: Some(600),
            display: None,
        };
        assert!(url_geometry(&outside, &displays, Point { x: 500, y: 1700 }).is_err());
    }

    #[test]
    fn unsupported_scheme_is_rejected() {
        assert!(parse_url("https://example.com").is_err());
        assert!(parse_url("omnishot://open-annotate?action=copy").is_ok());
    }
}
