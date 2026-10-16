//! Capture and selection geometry.
// Ported from omnishot/selection_geometry.py and omnishot/clean_capture.py
// (monitor_bounds).
//!
//! All rectangles use exclusive right and bottom edges (`x + width`,
//! `y + height`), matching the arithmetic the Python app performs with Qt's
//! inclusive `QRect.right()` / `QRect.bottom()`.

use std::fmt;
use std::ops::Range;

/// Why a capture geometry string could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeometryError {
    message: String,
}

impl GeometryError {
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for GeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GeometryError {}

/// Left, top, width and height in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    #[must_use]
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// One past the last covered column.
    #[must_use]
    pub const fn right(&self) -> i32 {
        self.x.saturating_add(self.width)
    }

    /// One past the last covered row.
    #[must_use]
    pub const fn bottom(&self) -> i32 {
        self.y.saturating_add(self.height)
    }

    /// The horizontal extent as a half-open range.
    #[must_use]
    pub const fn x_range(&self) -> Range<i32> {
        self.x..self.right()
    }

    /// The vertical extent as a half-open range.
    #[must_use]
    pub const fn y_range(&self) -> Range<i32> {
        self.y..self.bottom()
    }

    /// Whether `other` lies entirely inside this rectangle.
    #[must_use]
    pub const fn contains(&self, other: &Rect) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.right() <= self.right()
            && other.bottom() <= self.bottom()
    }
}

/// A compositor output running in a possibly fractional scale or rotation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
    /// Hyprland transform; an odd value rotates the output by a quarter turn.
    pub transform: i32,
}

impl MonitorGeometry {
    #[must_use]
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
            scale: 1.0,
            transform: 0,
        }
    }

    /// Output bounds in logical pixels, with rotation applied.
    // Ported from omnishot/clean_capture.py (monitor_bounds)
    #[must_use]
    pub fn bounds(&self) -> Rect {
        let (mut width, mut height) = (self.width, self.height);
        if self.transform.rem_euclid(2) == 1 {
            std::mem::swap(&mut width, &mut height);
        }
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        Rect::new(
            self.x,
            self.y,
            python_round(width as f64 / scale),
            python_round(height as f64 / scale),
        )
    }
}

/// Which axis drives a ratio-constrained resize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    Width,
    Height,
}

/// Python's `round`, which rounds halves to the nearest even integer.
fn python_round(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    value.round_ties_even() as i32
}

/// Constrain a size to the workspace, optionally preserving a width/height
/// ratio. `minimum` is the smallest side; when a ratio applies the minimum
/// width is lifted to keep both sides at least that large.
// Ported from omnishot/selection_geometry.py (fit_size)
#[must_use]
pub fn fit_size(
    width: i32,
    height: i32,
    max_width: i32,
    max_height: i32,
    minimum: i32,
    ratio: Option<f64>,
    driver: Driver,
) -> (i32, i32) {
    let (width, height) = match ratio {
        Some(ratio) if ratio > 0.0 => {
            let mut width = if driver == Driver::Width {
                width as f64
            } else {
                height as f64 * ratio
            };
            let low = (minimum as f64).max(minimum as f64 * ratio);
            let high = (max_width as f64).min(max_height as f64 * ratio);
            // The workspace bound always wins, even when it lies below the
            // ratio-lifted minimum; only the minimum can be undershot.
            width = width.max(low).min(high);
            (width, width / ratio)
        }
        _ => (
            width.max(minimum).min(max_width) as f64,
            height.max(minimum).min(max_height) as f64,
        ),
    };
    (python_round(width).max(1), python_round(height).max(1))
}

/// Move a selection by a delta, clamped so it stays inside `bounds`.
// Ported from omnishot/selection_geometry.py (move_selection)
#[must_use]
pub fn move_selection(rect: Rect, dx: i32, dy: i32, bounds: Rect) -> Rect {
    let x = (rect.x + dx).clamp(bounds.x, bounds.x + bounds.width - rect.width);
    let y = (rect.y + dy).clamp(bounds.y, bounds.y + bounds.height - rect.height);
    Rect::new(x, y, rect.width, rect.height)
}

/// Resize `rect` to a typed size while staying inside `bounds`.
// Ported from omnishot/selection_geometry.py (size_selection)
#[must_use]
pub fn size_selection(
    rect: Rect,
    width: i32,
    height: i32,
    bounds: Rect,
    ratio: Option<f64>,
    driver: Driver,
    minimum: i32,
) -> Rect {
    let x = rect.x.max(bounds.x).min(bounds.right() - minimum);
    let y = rect.y.max(bounds.y).min(bounds.bottom() - minimum);
    let (width, height) = fit_size(
        width,
        height,
        bounds.right() - x,
        bounds.bottom() - y,
        minimum,
        ratio,
        driver,
    );
    Rect::new(x, y, width, height)
}

/// Drag one of the eight handles. The opposite edge (or the center for a
/// middle handle) stays anchored.
// Ported from omnishot/selection_geometry.py (resize_selection)
#[must_use]
pub fn resize_selection(
    rect: Rect,
    edge: &str,
    dx: i32,
    dy: i32,
    bounds: Rect,
    ratio: Option<f64>,
    minimum: i32,
) -> Rect {
    let left = edge.contains('l');
    let right = edge.contains('r');
    let top = edge.contains('t');
    let bottom = edge.contains('b');
    let horizontal = left || right;
    let vertical = top || bottom;

    let ax = if left {
        rect.right()
    } else if right {
        rect.x
    } else {
        rect.x + rect.width / 2
    };
    let ay = if top {
        rect.bottom()
    } else if bottom {
        rect.y
    } else {
        rect.y + rect.height / 2
    };

    let max_width = if left {
        ax - bounds.x
    } else if right {
        bounds.right() - ax
    } else {
        2 * (ax - bounds.x).min(bounds.right() - ax)
    };
    let max_height = if top {
        ay - bounds.y
    } else if bottom {
        bounds.bottom() - ay
    } else {
        2 * (ay - bounds.y).min(bounds.bottom() - ay)
    };

    let width = rect.width
        + if left {
            -dx
        } else if right {
            dx
        } else {
            0
        };
    let height = rect.height
        + if top {
            -dy
        } else if bottom {
            dy
        } else {
            0
        };
    let driver = if horizontal && (!vertical || width_drives(dx, dy, rect)) {
        Driver::Width
    } else {
        Driver::Height
    };
    let (width, height) = fit_size(width, height, max_width, max_height, minimum, ratio, driver);

    let x = if left {
        ax - width
    } else if right {
        ax
    } else {
        ax - width / 2
    };
    let y = if top {
        ay - height
    } else if bottom {
        ay
    } else {
        ay - height / 2
    };
    Rect::new(x, y, width, height)
}

/// True when the horizontal drag dominates the vertical one, so a ratio
/// constraint should follow the width.
fn width_drives(dx: i32, dy: i32, rect: Rect) -> bool {
    if rect.width == 0 || rect.height == 0 {
        return true;
    }
    (dx as f64 / rect.width as f64).abs() >= (dy as f64 / rect.height as f64).abs()
}

/// Format a rectangle the way `grim -g` expects it.
// Ported from omnishot/backend.py (geometry_text)
#[must_use]
pub fn geometry_text(rect: Rect) -> String {
    format!("{},{} {}x{}", rect.x, rect.y, rect.width, rect.height)
}

/// Parse the `X,Y WxH` form used by `--geometry` and slurp output.
// Ported from omnishot/backend.py (parse_geometry)
pub fn parse_geometry(text: &str) -> Result<Rect, GeometryError> {
    let text = text.trim();
    let (rest, x) = parse_signed(text).ok_or_else(invalid)?;
    let rest = rest.strip_prefix(',').ok_or_else(invalid)?;
    let (rest, y) = parse_signed(rest).ok_or_else(invalid)?;
    let rest = strip_required_whitespace(rest).ok_or_else(invalid)?;
    let (rest, width) = parse_unsigned(rest).ok_or_else(invalid)?;
    let rest = rest.strip_prefix('x').ok_or_else(invalid)?;
    let (rest, height) = parse_unsigned(rest).ok_or_else(invalid)?;
    if !rest.is_empty() {
        return Err(invalid());
    }
    if width < 2 || height < 2 {
        return Err(GeometryError {
            message: "Selection is too small".to_owned(),
        });
    }
    let (Ok(x), Ok(y), Ok(width), Ok(height)) = (
        i32::try_from(x),
        i32::try_from(y),
        i32::try_from(width),
        i32::try_from(height),
    ) else {
        return Err(invalid());
    };
    Ok(Rect::new(x, y, width, height))
}

fn invalid() -> GeometryError {
    GeometryError {
        message: "Invalid capture geometry".to_owned(),
    }
}

fn parse_signed(text: &str) -> Option<(&str, i64)> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (rest, value) = parse_unsigned(digits)?;
    Some((rest, if negative { -value } else { value }))
}

fn parse_unsigned(text: &str) -> Option<(&str, i64)> {
    let end = text
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_digit())
        .map(|(index, c)| index + c.len_utf8())
        .last()?;
    let value = text[..end].parse().ok()?;
    Some((&text[end..], value))
}

fn strip_required_whitespace(text: &str) -> Option<&str> {
    let trimmed = text.trim_start_matches(char::is_whitespace);
    if trimmed.len() == text.len() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspect_resize_anchors_and_screen_edges_on_every_handle() {
        let bounds = Rect::new(-1800, -200, 3720, 1325);
        let original = Rect::new(-300, 100, 640, 400);
        for edge in ["lt", "t", "rt", "r", "rb", "b", "lb", "l"] {
            for (dx, dy) in [
                (130, 80),
                (-130, -80),
                (5000, 5000),
                (-5000, -5000),
                (130, -80),
            ] {
                let result = resize_selection(original, edge, dx, dy, bounds, Some(1.6), 40);
                assert!(bounds.contains(&result), "{edge} {dx},{dy} -> {result:?}");
                assert!(result.width.min(result.height) >= 40);
                assert!((result.width as f64 - result.height as f64 * 1.6).abs() <= 1.6);
                if edge.contains('l') {
                    assert_eq!(result.right(), original.right());
                }
                if edge.contains('r') {
                    assert_eq!(result.x, original.x);
                }
                if edge.contains('t') {
                    assert_eq!(result.bottom(), original.bottom());
                }
                if edge.contains('b') {
                    assert_eq!(result.y, original.y);
                }
            }
        }
    }

    #[test]
    fn ratio_resize_never_exceeds_a_bound_below_the_minimum() {
        // Regression: the workspace bound is narrower than the ratio-lifted
        // minimum, so the clamp must let the bound win and undershoot the
        // minimum instead of spilling outside the bounds.
        let bounds = Rect::new(-592, -1400, 808, 2905);
        let result = resize_selection(
            Rect::new(179, 1180, 37, 403),
            "rb",
            784,
            4763,
            bounds,
            Some(16.0 / 9.0),
            40,
        );
        assert_eq!(result, Rect::new(179, 1180, 37, 21));
        assert!(bounds.contains(&result), "{result:?}");
    }

    #[test]
    fn free_resize_uses_the_bound_when_it_is_below_the_minimum() {
        let bounds = Rect::new(0, 0, 50, 30);
        let original = Rect::new(0, 0, 40, 40);
        // Dragging an edge outward when the bound is narrower than the
        // minimum: the bound must win so the result stays inside.
        let result = resize_selection(original, "rb", -1000, -1000, bounds, None, 400);
        assert_eq!(result, Rect::new(0, 0, 50, 30));
        assert!(bounds.contains(&result), "{result:?}");
    }

    #[test]
    fn typed_size_uses_changed_dimension_and_keeps_ratio_at_bounds() {
        let bounds = Rect::new(0, 0, 1800, 1125);
        let original = Rect::new(120, 140, 640, 400);
        let ratio = Some(16.0 / 9.0);
        assert_eq!(
            size_selection(original, 800, 400, bounds, ratio, Driver::Width, 2),
            Rect::new(120, 140, 800, 450)
        );
        assert_eq!(
            size_selection(original, 800, 360, bounds, ratio, Driver::Height, 2),
            Rect::new(120, 140, 640, 360)
        );
        let clamped = size_selection(original, 20000, 360, bounds, ratio, Driver::Width, 2);
        assert_eq!(clamped, Rect::new(120, 140, 1680, 945));
        assert_eq!(
            move_selection(clamped, 5000, 5000, bounds),
            Rect::new(120, 180, 1680, 945)
        );
    }

    #[test]
    fn free_resize_stays_inside_and_respects_the_minimum() {
        let bounds = Rect::new(0, 0, 500, 300);
        let original = Rect::new(100, 80, 200, 120);
        for edge in ["lt", "t", "rt", "r", "rb", "b", "lb", "l"] {
            for (dx, dy) in [(1000, 1000), (-1000, -1000), (-30, -15)] {
                let result = resize_selection(original, edge, dx, dy, bounds, None, 2);
                assert!(bounds.contains(&result), "{edge} {dx},{dy} -> {result:?}");
                assert!(result.width >= 2 && result.height >= 2);
            }
        }
    }

    #[test]
    fn move_selection_clamps_at_every_edge() {
        let bounds = Rect::new(0, 0, 1920, 1080);
        let rect = Rect::new(100, 100, 400, 300);
        assert_eq!(
            move_selection(rect, -5000, -5000, bounds),
            Rect::new(0, 0, 400, 300)
        );
        assert_eq!(
            move_selection(rect, 5000, 5000, bounds),
            Rect::new(1520, 780, 400, 300)
        );
    }

    #[test]
    fn monitor_bounds_apply_scale_and_rotation() {
        let monitor = MonitorGeometry {
            x: -1920,
            y: 0,
            width: 2880,
            height: 1800,
            scale: 1.5,
            transform: 0,
        };
        assert_eq!(monitor.bounds(), Rect::new(-1920, 0, 1920, 1200));

        let rotated = MonitorGeometry {
            transform: 1,
            ..MonitorGeometry::new(0, 0, 1920, 1080)
        };
        assert_eq!(rotated.bounds(), Rect::new(0, 0, 1080, 1920));
    }

    #[test]
    fn geometry_text_matches_grim_grammar() {
        assert_eq!(geometry_text(Rect::new(10, 20, 640, 400)), "10,20 640x400");
    }

    #[test]
    fn geometry_parsing_round_trips_and_rejects_bad_input() {
        assert_eq!(
            parse_geometry(" 10,20 640x400 ").unwrap(),
            Rect::new(10, 20, 640, 400)
        );
        assert_eq!(
            parse_geometry("-1800,0\t800x500").unwrap(),
            Rect::new(-1800, 0, 800, 500)
        );
        for bad in [
            "",
            "10,20",
            "10,20 640",
            "10,20 640x",
            "10,20 640x400 extra",
            "10,20 -640x400",
            "10 20 640x400",
            "0,0 4294967297x400",
        ] {
            assert_eq!(
                parse_geometry(bad).unwrap_err().message(),
                "Invalid capture geometry",
                "{bad:?}"
            );
        }
        assert_eq!(
            parse_geometry("0,0 1x100").unwrap_err().message(),
            "Selection is too small"
        );
        assert_eq!(
            parse_geometry("0,0 100x1").unwrap_err().message(),
            "Selection is too small"
        );
    }
}
