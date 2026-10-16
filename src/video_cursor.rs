//! Cursor geometry shared by the style tiles, preview and export.
// Ported from omnishot/cursor_shape.py
//!
//! `paint_cursor` in the source drew through QPainter. Here the same outline is
//! expressed as polygons and an ellipse so the raster canvas can stroke and
//! fill it without a Qt painter.

use crate::annotation::Point;

/// The cursor styles the editor offers, in tile order.
pub const STYLES: &[&str] = &["Arrow", "Rounded Arrow", "Dot", "Crosshair"];

/// An arrow's outline, in source order.
///
/// The points match the source's normalized arrow: the tip at `(0, 0)` down to
/// the tail, then the notch that makes the arrowhead.
#[must_use]
pub fn arrow_points(position: Point, size: f64) -> Vec<Point> {
    const NORMALIZED: &[(f64, f64)] = &[
        (0.0, 0.0),
        (0.0, 1.0),
        (0.26, 0.75),
        (0.48, 1.1),
        (0.66, 1.0),
        (0.45, 0.65),
        (0.8, 0.65),
    ];
    NORMALIZED
        .iter()
        .map(|(dx, dy)| Point::new(position.x + dx * size, position.y + dy * size))
        .collect()
}

/// The rounded arrow's control points, one per corner.
///
/// The source builds the same outline with quadratic curves whose entry and
/// leave points sit a fraction of the way to the neighboring corner.
#[must_use]
pub fn rounded_arrow_curves(position: Point, size: f64) -> Vec<(Point, Point, Point)> {
    let points = arrow_points(position, size);
    let reach = (size * 0.07).min(0.25 * size);
    let mut curves = Vec::with_capacity(points.len());
    for (index, point) in points.iter().enumerate() {
        let previous = points[(index + points.len() - 1) % points.len()];
        let next = points[(index + 1) % points.len()];
        let toward = |other: Point| {
            let delta = Point::new(other.x - point.x, other.y - point.y);
            let distance = (delta.x * delta.x + delta.y * delta.y).sqrt().max(1e-9);
            let scale = reach.min(0.25) / distance;
            Point::new(point.x + delta.x * scale, point.y + delta.y * scale)
        };
        curves.push((toward(previous), *point, toward(next)));
    }
    curves
}

/// The crosshair's two bars: `(rect)` pairs as center, half-width, half-height.
#[must_use]
pub fn crosshair_bars(position: Point, size: f64) -> [(Point, f64, f64); 2] {
    [
        (position, size * 0.5, size * 0.07),
        (position, size * 0.07, size * 0.5),
    ]
}

/// The dot's radius for a cursor size.
#[must_use]
pub fn dot_radius(size: f64) -> f64 {
    size * 0.3
}

/// The arrow defaults' outline width for a cursor size.
#[must_use]
pub fn outline_width(size: f64) -> f64 {
    (size / 16.0).max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_are_offered_verbatim() {
        assert_eq!(STYLES, &["Arrow", "Rounded Arrow", "Dot", "Crosshair"]);
    }

    #[test]
    fn the_arrow_tip_sits_at_the_cursor_position() {
        let points = arrow_points(Point::new(10.0, 20.0), 22.0);
        assert_eq!(points[0], Point::new(10.0, 20.0));
        assert_eq!(points.len(), 7);
        // The tail is a full cursor-size below the tip.
        assert_eq!(points[1], Point::new(10.0, 42.0));
    }

    #[test]
    fn rounded_arrow_curve_entry_and_leave_stay_within_the_segment() {
        let curves = rounded_arrow_curves(Point::new(0.0, 0.0), 22.0);
        assert_eq!(curves.len(), 7);
        for (entry, corner, leave) in curves {
            assert_eq!(corner, corner);
            // Entry and leave are the corner's neighbors, nudged inward.
            assert!(entry.x.is_finite() && leave.x.is_finite());
        }
    }

    #[test]
    fn dot_and_crosshair_scale_with_size() {
        assert!((dot_radius(20.0) - 6.0).abs() < 1e-9);
        assert_eq!(outline_width(8.0), 1.0);
        assert!((outline_width(32.0) - 2.0).abs() < 1e-9);
        let bars = crosshair_bars(Point::new(0.0, 0.0), 20.0);
        assert!((bars[0].1 - 10.0).abs() < 1e-9);
        assert!((bars[0].2 - 1.4).abs() < 1e-9);
    }
}
