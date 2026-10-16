//! Nine-position camera placement and bounded camera framing.
// Ported from omnishot/video_position.py
//!
//! The camera rect is clamped inside the canvas margin so a large camera or a
//! fullscreen edit never spills outside the exported frame. A `camera_shrink`
//! preference pulls the camera in as the recorded zoom grows.

use std::collections::BTreeMap;

use crate::annotation::Rect;
use crate::json::Value;

/// The camera placement choices, in the grid's reading order.
pub const POSITIONS: &[&str] = &[
    "Top Left",
    "Top Center",
    "Top Right",
    "Center Left",
    "Center",
    "Center Right",
    "Bottom Left",
    "Bottom Center",
    "Bottom Right",
];

/// The camera rectangle inside an `width` x `height` canvas.
///
/// `aspect` is the camera stream's width/height; circle and square shapes use
/// a 1:1 target. `zoom` applies the optional shrink.
// Ported from omnishot/video_position.py (camera_rect)
#[must_use]
pub fn camera_rect(
    width: f64,
    height: f64,
    aspect: f64,
    options: &BTreeMap<String, Value>,
    zoom: f64,
) -> Rect {
    if options
        .get("camera_fullscreen")
        .is_some_and(Value::is_truthy)
    {
        return Rect::new(0.0, 0.0, width, height);
    }
    let shape = options
        .get("camera_shape")
        .and_then(Value::as_str)
        .unwrap_or("Circle");
    let ratio = if matches!(shape, "Circle" | "Square") {
        1.0
    } else {
        aspect.max(0.01)
    };
    let shrink = if options.get("camera_shrink").is_some_and(Value::is_truthy) {
        1.0 - 0.15 * (zoom - 1.0).clamp(0.0, 1.0)
    } else {
        1.0
    };

    if let Some(placement) = options.get("camera_placement").and_then(Value::as_array)
        && placement.len() == 3
        && let (Some(x), Some(y), Some(size)) = (
            placement[0].as_f64(),
            placement[1].as_f64(),
            placement[2].as_f64(),
        )
    {
        let size = (width * size).max(1.0) * shrink;
        return Rect::new(width * x, height * y, size, size * ratio);
    }

    let margin = 20.0_f64.min(((width.min(height) - 2.0) / 2.0).max(0.0));
    let configured = options
        .get("camera_size")
        .and_then(Value::as_f64)
        .unwrap_or(0.22);
    let size = (width * configured.max(0.01)).max(1.0) * shrink;
    let ratio = ratio.max(0.01);
    let size = size
        .min(width - 2.0 * margin)
        .min((height - 2.0 * margin) / ratio);
    let camera_height = size * ratio;
    let position = options
        .get("camera_position")
        .and_then(Value::as_str)
        .unwrap_or("Bottom Right");
    let x = if position.contains("Left") {
        margin
    } else if position.contains("Right") {
        width - size - margin
    } else {
        (width - size) / 2.0
    };
    let y = if position.contains("Top") {
        margin
    } else if position.contains("Bottom") {
        height - camera_height - margin
    } else {
        (height - camera_height) / 2.0
    };
    Rect::new(x, y, size, camera_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(entries: &[(&str, Value)]) -> BTreeMap<String, Value> {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    #[test]
    fn the_nine_positions_are_offered_in_grid_order() {
        assert_eq!(POSITIONS.len(), 9);
        assert_eq!(POSITIONS[0], "Top Left");
        assert_eq!(POSITIONS[4], "Center");
        assert_eq!(POSITIONS[8], "Bottom Right");
    }

    #[test]
    fn corner_camera_stays_inside_the_margin() {
        let rect = camera_rect(1000.0, 500.0, 1.5, &BTreeMap::new(), 1.0);
        assert!(rect.x >= 0.0 && rect.y >= 0.0);
        assert!(rect.right() <= 1000.0 + 1e-9);
        assert!(rect.bottom() <= 500.0 + 1e-9);
        assert!(rect.width > 0.0 && rect.height > 0.0);
    }

    #[test]
    fn fullscreen_ignores_size_and_placement() {
        let rect = camera_rect(
            800.0,
            600.0,
            1.0,
            &options(&[("camera_fullscreen", Value::Bool(true))]),
            1.0,
        );
        assert_eq!(rect, Rect::new(0.0, 0.0, 800.0, 600.0));
    }

    #[test]
    fn placement_and_shrink_scale_the_camera() {
        let placed = camera_rect(
            1000.0,
            800.0,
            2.0,
            &options(&[
                (
                    "camera_placement",
                    Value::Array(vec![
                        Value::Number(0.1),
                        Value::Number(0.2),
                        Value::Number(0.3),
                    ]),
                ),
                ("camera_shape", Value::String("Rectangle".to_owned())),
            ]),
            1.0,
        );
        assert!((placed.x - 100.0).abs() < 1e-9);
        assert!((placed.y - 160.0).abs() < 1e-9);
        assert!((placed.width - 300.0).abs() < 1e-9);
        assert!((placed.height - 600.0).abs() < 1e-9);

        let shrunk = camera_rect(
            1000.0,
            800.0,
            1.0,
            &options(&[
                (
                    "camera_placement",
                    Value::Array(vec![
                        Value::Number(0.0),
                        Value::Number(0.0),
                        Value::Number(0.3),
                    ]),
                ),
                ("camera_shrink", Value::Bool(true)),
            ]),
            2.0,
        );
        assert!((shrunk.width - 255.0).abs() < 1e-9, "{shrunk:?}");
    }
}
