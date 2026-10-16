//! Recording dimensions and output limits, in logical pixels.
// Ported from omnishot/recording_options.py
//!
//! The display a fullscreen take records is chosen from the pointer, before
//! the countdown starts, and is kept even if focus moves during the count.
//! A maximum-resolution setting is clamped to the capture's own logical size
//! so a scaled display is never upscaled.

use super::display::{Monitor, monitor_bounds};

/// The maximum-resolution choices the setup dialog offers, in order.
pub const RESOLUTIONS: &[&str] = &[
    "Native",
    "3840x2160",
    "2560x1440",
    "1920x1080",
    "1280x720",
    "960x540",
];

/// Choose the display under the pointer, independent of restored focus.
///
/// Falls back to the focused display when the pointer is outside every output.
#[must_use]
pub fn recording_output(monitors: &[Monitor], cursor: (i64, i64)) -> Option<String> {
    for monitor in monitors {
        let (x, y, w, h) = monitor_bounds(monitor);
        if x <= cursor.0 && cursor.0 < x + w && y <= cursor.1 && cursor.1 < y + h {
            return Some(monitor.name.clone());
        }
    }
    super::display::capture_target(monitors, None)
        .ok()
        .flatten()
        .and_then(|target| target.output)
}

/// Restrict a monitor list to the named output, marking it focused.
///
/// Fails when the chosen output is no longer connected, so a recording never
/// silently falls back to a different display.
pub fn recording_monitors(
    monitors: &[Monitor],
    output: Option<&str>,
) -> Result<Vec<Monitor>, crate::recording::display::CaptureError> {
    let Some(output) = output else {
        return Ok(monitors.to_vec());
    };
    for monitor in monitors {
        if monitor.name == output {
            let mut focused = monitor.clone();
            focused.focused = true;
            return Ok(vec![focused]);
        }
    }
    Err(crate::recording::display::CaptureError::NoDisplay)
}

/// The `WxH` encoder limit for a take, or `None` for native output.
///
/// `scale_video` clamps the requested size to the capture's logical size,
/// which is the region when one is given and the focused display otherwise.
#[must_use]
pub fn resolution_limit(
    size: &str,
    scale_video: bool,
    monitors: &[Monitor],
    rect: Option<(i64, i64, i64, i64)>,
) -> Option<String> {
    let mut limit = if size == "Native" {
        None
    } else {
        size.split_once('x')
            .and_then(|(w, h)| Some((w.parse::<i64>().ok()?, h.parse::<i64>().ok()?)))
    };
    if scale_video {
        let logical = if let Some(rect) = rect {
            (rect.2, rect.3)
        } else {
            super::display::capture_target(monitors, None)
                .ok()
                .flatten()
                .map_or((0, 0), |target| (target.bounds.2, target.bounds.3))
        };
        limit = match limit {
            Some((w, h)) => Some((w.min(logical.0), h.min(logical.1))),
            None => Some(logical),
        };
    }
    limit.map(|(w, h)| format!("{w}x{h}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::display::Monitor;

    fn monitor(name: &str, x: i64, y: i64, width: i64, height: i64, scale: f64) -> Monitor {
        let mut monitor = Monitor::new(name, x, y, width, height);
        monitor.scale = scale;
        monitor
    }

    fn scaled() -> Monitor {
        let mut monitor = monitor("test", 0, 0, 2880, 1800, 1.6);
        monitor.focused = true;
        monitor
    }

    #[test]
    fn resolution_limits_preserve_logical_bounds_and_rotation() {
        assert_eq!(resolution_limit("Native", false, &[scaled()], None), None);
        assert_eq!(
            resolution_limit("Native", true, &[scaled()], None).as_deref(),
            Some("1800x1125")
        );
        let mut rotated = scaled();
        rotated.transform = 1;
        assert_eq!(
            resolution_limit("Native", true, &[rotated], None).as_deref(),
            Some("1125x1800")
        );
        assert_eq!(
            resolution_limit("1920x1080", true, &[scaled()], None).as_deref(),
            Some("1800x1080")
        );
        assert_eq!(
            resolution_limit("1280x720", true, &[scaled()], Some((-100, 50, 900, 600))).as_deref(),
            Some("900x600")
        );
        assert_eq!(
            resolution_limit("Native", true, &[], Some((-200, 10, 2000, 500))).as_deref(),
            Some("2000x500")
        );
    }

    #[test]
    fn pointer_display_handles_negative_rotated_bounds_and_focus_fallback() {
        let mut first = monitor("first", 0, 0, 2880, 1800, 1.6);
        first.focused = true;
        let mut second = monitor("second", -1080, 0, 1920, 1080, 1.0);
        second.transform = 1;
        let monitors = [first, second];
        assert_eq!(
            recording_output(&monitors, (-100, 1800)).as_deref(),
            Some("second")
        );
        assert_eq!(
            recording_output(&monitors, (0, 100)).as_deref(),
            Some("first")
        );
        assert_eq!(
            recording_output(&monitors, (-1081, 100)).as_deref(),
            Some("first")
        );
    }

    #[test]
    fn explicit_output_survives_a_monitor_layout_change() {
        let first = monitor("first", 0, 0, 2880, 1800, 1.6);
        let mut second = monitor("second", -1080, 0, 1920, 1080, 1.0);
        second.transform = 1;
        let chosen = recording_monitors(&[first, second], Some("second")).unwrap();
        assert_eq!(chosen.len(), 1);
        assert!(chosen[0].focused);
        assert_eq!(
            resolution_limit("Native", true, &chosen, None).as_deref(),
            Some("1080x1920")
        );
    }

    #[test]
    fn disconnected_fullscreen_output_is_not_replaced() {
        let first = monitor("first", 0, 0, 2880, 1800, 1.6);
        assert!(recording_monitors(&[first], Some("second")).is_err());
    }
}
