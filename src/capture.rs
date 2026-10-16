//! Capture target selection and recording output limits.
// Ported from omnishot/clean_capture.py (capture_target) and
// omnishot/recording_options.py.
//!
//! A recording targets exactly one output whenever the requested region fits
//! one, and reports the composite case otherwise. Tying a recording to a
//! chosen output keeps it stable across focus changes during a countdown.

use std::fmt;

use crate::api::Point;
use crate::geometry::{MonitorGeometry, Rect};

/// Why a capture target could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    /// No display is connected.
    NoDisplays,
    /// The requested region has a side shorter than two pixels.
    TooSmall,
    /// The chosen output is no longer connected.
    Disconnected,
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CaptureError::NoDisplays => f.write_str("No display is available for recording"),
            CaptureError::TooSmall => f.write_str("The recording area is too small"),
            CaptureError::Disconnected => {
                f.write_str("The display selected for recording is no longer connected.")
            }
        }
    }
}

impl std::error::Error for CaptureError {}

/// One compositor output with the identity fields capture decides on.
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    pub name: String,
    pub geometry: MonitorGeometry,
    pub focused: bool,
}

impl Display {
    #[must_use]
    pub fn new(name: impl Into<String>, geometry: MonitorGeometry) -> Self {
        Self {
            name: name.into(),
            geometry,
            focused: false,
        }
    }

    #[must_use]
    pub fn focused(mut self) -> Self {
        self.focused = true;
        self
    }
}

/// Where a capture should be taken from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureTarget {
    /// Output name, or `None` when the region spans more than one output.
    pub name: Option<String>,
    /// Region relative to the output, when the target is a single output.
    pub relative: Option<Rect>,
    /// Absolute logical bounds of the target output or composite region.
    pub bounds: Rect,
}

/// Choose a whole output or a region entirely contained on one output.
///
/// Returns `Ok(None)` when a region spans outputs and needs the composite
/// source. `rect` is absolute logical pixels.
// Ported from omnishot/clean_capture.py (capture_target)
pub fn capture_target(
    displays: &[Display],
    rect: Option<Rect>,
) -> Result<Option<CaptureTarget>, CaptureError> {
    if displays.is_empty() {
        return Err(CaptureError::NoDisplays);
    }
    for display in displays {
        let bounds = display.geometry.bounds();
        match rect {
            None => {
                if display.focused || displays.len() == 1 {
                    return Ok(Some(CaptureTarget {
                        name: Some(display.name.clone()),
                        relative: None,
                        bounds,
                    }));
                }
            }
            Some(rect) => {
                if rect.width < 2 || rect.height < 2 {
                    return Err(CaptureError::TooSmall);
                }
                if rect.x >= bounds.x
                    && rect.y >= bounds.y
                    && rect.right() <= bounds.right()
                    && rect.bottom() <= bounds.bottom()
                {
                    return Ok(Some(CaptureTarget {
                        name: Some(display.name.clone()),
                        relative: Some(Rect::new(
                            rect.x - bounds.x,
                            rect.y - bounds.y,
                            rect.width,
                            rect.height,
                        )),
                        bounds,
                    }));
                }
            }
        }
    }
    if rect.is_none() {
        // Nothing focused: fall back to the first display, exactly as the
        // recursive call upstream does.
        let first = &displays[0];
        return Ok(Some(CaptureTarget {
            name: Some(first.name.clone()),
            relative: None,
            bounds: first.geometry.bounds(),
        }));
    }
    Ok(None)
}

/// The output under the pointer, falling back to the focused output.
// Ported from omnishot/recording_options.py (recording_output)
pub fn recording_output(displays: &[Display], cursor: Point) -> Result<String, CaptureError> {
    for display in displays {
        let bounds = display.geometry.bounds();
        if cursor.x >= bounds.x
            && cursor.x < bounds.right()
            && cursor.y >= bounds.y
            && cursor.y < bounds.bottom()
        {
            return Ok(display.name.clone());
        }
    }
    capture_target(displays, None)?
        .and_then(|target| target.name)
        .ok_or(CaptureError::NoDisplays)
}

/// Narrow the display list to a single explicit output.
// Ported from omnishot/recording_options.py (recording_monitors)
pub fn recording_monitors(
    displays: &[Display],
    output: Option<&str>,
) -> Result<Vec<Display>, CaptureError> {
    match output {
        None => Ok(displays.to_vec()),
        Some(output) => displays
            .iter()
            .find(|display| display.name == output)
            .map(|display| {
                vec![Display {
                    focused: true,
                    ..display.clone()
                }]
            })
            .ok_or(CaptureError::Disconnected),
    }
}

/// The maximum encoder size, capped by the logical capture bounds when the
/// video is scaled to the display.
// Ported from omnishot/recording_options.py (resolution_limit)
pub fn resolution_limit(
    size: &str,
    scale_video: bool,
    displays: &[Display],
    rect: Option<Rect>,
) -> Result<Option<String>, CaptureError> {
    let mut limit = if size == "Native" {
        None
    } else {
        parse_resolution(size)
    };
    if scale_video {
        let logical = match rect {
            Some(rect) => (rect.width, rect.height),
            None => {
                let bounds = capture_target(displays, None)?
                    .ok_or(CaptureError::NoDisplays)?
                    .bounds;
                (bounds.width, bounds.height)
            }
        };
        limit = match limit {
            Some((width, height)) => Some((width.min(logical.0), height.min(logical.1))),
            None => Some(logical),
        };
    }
    Ok(limit.map(|(width, height)| format!("{width}x{height}")))
}

/// The offered recording resolutions, `Native` meaning the display size.
pub const RESOLUTIONS: &[&str] = &[
    "Native",
    "3840x2160",
    "2560x1440",
    "1920x1080",
    "1280x720",
    "960x540",
];

fn parse_resolution(size: &str) -> Option<(i32, i32)> {
    let (width, height) = size.split_once('x')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(
        name: &str,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        scale: f64,
        transform: i32,
    ) -> Display {
        Display::new(
            name,
            MonitorGeometry {
                x,
                y,
                width,
                height,
                scale,
                transform,
            },
        )
    }

    fn displays() -> Vec<Display> {
        vec![
            display("left", -1800, 0, 2880, 1800, 1.6, 0).focused(),
            display("portrait", 0, 0, 1920, 1080, 1.0, 1),
        ]
    }

    #[test]
    fn fractional_negative_and_rotated_outputs() {
        let displays = displays();
        let target = capture_target(&displays, None).unwrap().unwrap();
        assert_eq!(target.name.as_deref(), Some("left"));
        assert_eq!(target.bounds, Rect::new(-1800, 0, 1800, 1125));

        let region = capture_target(&displays, Some(Rect::new(-1700, 100, 800, 500)))
            .unwrap()
            .unwrap();
        assert_eq!(region.relative, Some(Rect::new(100, 100, 800, 500)));
        assert_eq!(
            capture_target(&displays, Some(Rect::new(0, 0, 1080, 1920)))
                .unwrap()
                .unwrap()
                .name
                .as_deref(),
            Some("portrait")
        );
        assert_eq!(
            capture_target(&displays, Some(Rect::new(-100, 0, 200, 100))).unwrap(),
            None
        );
        assert_eq!(
            capture_target(&displays, Some(Rect::new(0, 0, 1, 100))),
            Err(CaptureError::TooSmall)
        );
    }

    #[test]
    fn pointer_display_handles_negative_rotated_bounds_and_focus_fallback() {
        let displays = vec![
            display("first", 0, 0, 2880, 1800, 1.6, 0).focused(),
            display("second", -1080, 0, 1920, 1080, 1.0, 1),
        ];
        assert_eq!(
            recording_output(&displays, Point { x: -100, y: 1800 }).unwrap(),
            "second"
        );
        assert_eq!(
            recording_output(&displays, Point { x: 0, y: 100 }).unwrap(),
            "first"
        );
        assert_eq!(
            recording_output(&displays, Point { x: -1081, y: 100 }).unwrap(),
            "first"
        );
    }

    #[test]
    fn resolution_limits_preserve_logical_bounds_and_rotation() {
        let displays = vec![display("test", 0, 0, 2880, 1800, 1.6, 0)];
        assert_eq!(
            resolution_limit("Native", false, &displays, None).unwrap(),
            None
        );
        assert_eq!(
            resolution_limit("Native", true, &displays, None).unwrap(),
            Some("1800x1125".to_owned())
        );

        let rotated = vec![display("test", 0, 0, 2880, 1800, 1.6, 1)];
        assert_eq!(
            resolution_limit("Native", true, &rotated, None).unwrap(),
            Some("1125x1800".to_owned())
        );
        assert_eq!(
            resolution_limit("1920x1080", true, &displays, None).unwrap(),
            Some("1800x1080".to_owned())
        );
        assert_eq!(
            resolution_limit(
                "1280x720",
                true,
                &displays,
                Some(Rect::new(-100, 50, 900, 600))
            )
            .unwrap(),
            Some("900x600".to_owned())
        );
        assert_eq!(
            resolution_limit("Native", true, &[], Some(Rect::new(-200, 10, 2000, 500))).unwrap(),
            Some("2000x500".to_owned())
        );
    }

    #[test]
    fn disconnected_explicit_output_is_rejected() {
        let displays = vec![display("first", 0, 0, 1920, 1080, 1.0, 0)];
        assert_eq!(
            recording_monitors(&displays, Some("second")),
            Err(CaptureError::Disconnected)
        );
        let narrowed = recording_monitors(&displays, Some("first")).unwrap();
        assert_eq!(narrowed.len(), 1);
        assert!(narrowed[0].focused);
    }

    #[test]
    fn no_displays_is_an_error() {
        assert_eq!(capture_target(&[], None), Err(CaptureError::NoDisplays));
    }
}
