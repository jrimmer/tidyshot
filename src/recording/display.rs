//! Capture surface selection: which output or region a recording encodes.
// Ported from omnishot/clean_capture.py
//!
//! A recording is captured either from a whole output or from one region,
//! cropped out of a single output when it fits and composed from several
//! when it spans displays. `CleanCapture` holds that layout and renders the
//! environment the clean-capture plugin and `gpu-screen-recorder` read.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use crate::json::Value;

/// One connected display, in compositor logical pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub name: String,
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    pub scale: f64,
    pub transform: i64,
    pub focused: bool,
    pub dpms_status: bool,
    pub solitary_blocked_by: Vec<String>,
}

impl Monitor {
    #[must_use]
    pub fn new(name: impl Into<String>, x: i64, y: i64, width: i64, height: i64) -> Self {
        Self {
            name: name.into(),
            x,
            y,
            width,
            height,
            scale: 1.0,
            transform: 0,
            focused: false,
            dpms_status: true,
            solitary_blocked_by: Vec::new(),
        }
    }

    /// Parse one entry of `hyprctl -j monitors`.
    #[must_use]
    pub fn from_json(value: &Value) -> Option<Self> {
        let name = value.get("name")?.as_str()?.to_owned();
        let number = |key: &str, default: f64| {
            value
                .get(key)
                .and_then(Value::as_f64)
                .filter(|n| n.is_finite())
                .unwrap_or(default)
        };
        let blocked = value
            .get("solitaryBlockedBy")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            name,
            x: number("x", 0.0) as i64,
            y: number("y", 0.0) as i64,
            width: number("width", 0.0) as i64,
            height: number("height", 0.0) as i64,
            scale: number("scale", 1.0),
            transform: number("transform", 0.0) as i64,
            focused: value
                .get("focused")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            dpms_status: value
                .get("dpmsStatus")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            solitary_blocked_by: blocked,
        })
    }
}

/// A display's bounds in logical pixels, with a rotated transform swapped.
///
/// `hyprctl` always reports a non-zero scale; a zero would divide by zero, so
/// it falls back to unscaled bounds rather than producing an infinite size.
#[must_use]
pub fn monitor_bounds(monitor: &Monitor) -> (i64, i64, i64, i64) {
    let (mut width, mut height) = (monitor.width as f64, monitor.height as f64);
    if monitor.transform.rem_euclid(2) == 1 {
        std::mem::swap(&mut width, &mut height);
    }
    let scale = if monitor.scale == 0.0 {
        1.0
    } else {
        monitor.scale
    };
    (
        monitor.x,
        monitor.y,
        python_round(width / scale),
        python_round(height / scale),
    )
}

/// `round` the way Python's `round()` does: ties go to the even integer.
fn python_round(value: f64) -> i64 {
    value.round_ties_even() as i64
}

/// The source an encoder should capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The output to capture, when a single output holds the whole region.
    pub output: Option<String>,
    /// The region relative to its output, when only part of one is captured.
    pub region: Option<(i64, i64, i64, i64)>,
    /// The region in global logical coordinates.
    pub bounds: (i64, i64, i64, i64),
}

impl Target {
    /// The target region in global coordinates.
    #[must_use]
    pub fn rect(&self) -> (i64, i64, i64, i64) {
        self.bounds
    }
}

/// Why a capture source could not be chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    NoDisplay,
    TooSmall,
    NoOverlap,
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureError::NoDisplay => f.write_str("No display is available for recording"),
            CaptureError::TooSmall => f.write_str("The recording area is too small"),
            CaptureError::NoOverlap => {
                f.write_str("The recording area does not overlap a connected display.")
            }
        }
    }
}

impl std::error::Error for CaptureError {}

/// Choose a whole output or a region entirely contained on one output.
///
/// A selection that fits no single output needs the composite source and
/// returns `Ok(None)`, which [`CleanCapture`] resolves into parts.
pub fn capture_target(
    monitors: &[Monitor],
    rect: Option<(i64, i64, i64, i64)>,
) -> Result<Option<Target>, CaptureError> {
    let Some(first) = monitors.first() else {
        return Err(CaptureError::NoDisplay);
    };
    for monitor in monitors {
        let bounds = monitor_bounds(monitor);
        let (_, _, width, height) = bounds;
        match rect {
            None => {
                if monitor.focused || monitors.len() == 1 {
                    return Ok(Some(Target {
                        output: Some(monitor.name.clone()),
                        region: None,
                        bounds,
                    }));
                }
            }
            Some((x, y, w, h)) => {
                if w < 2 || h < 2 {
                    return Err(CaptureError::TooSmall);
                }
                if x >= bounds.0
                    && y >= bounds.1
                    && x + w <= bounds.0 + width
                    && y + h <= bounds.1 + height
                {
                    return Ok(Some(Target {
                        output: Some(monitor.name.clone()),
                        region: Some((x - bounds.0, y - bounds.1, w, h)),
                        bounds,
                    }));
                }
            }
        }
    }
    if rect.is_none() {
        return capture_target(std::slice::from_ref(first), None);
    }
    Ok(None)
}

/// One display's share of a region that spans outputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub name: String,
    /// Offset within the source display.
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    /// Offset within the captured region.
    pub dx: i64,
    pub dy: i64,
}

/// The capture layout a recording uses, plus the encoder environment it needs.
#[derive(Debug, Clone)]
pub struct CleanCapture {
    pub target: Target,
    pub encoder_rect: Option<(i64, i64, i64, i64)>,
    pub parts: Vec<Part>,
    pub controls_bounds: (i64, i64, i64, i64),
    pub cursor: bool,
    pub enabled: bool,
}

impl CleanCapture {
    /// Lay out a region or output, splitting a spanning region into parts.
    pub fn new(
        monitors: &[Monitor],
        rect: Option<(i64, i64, i64, i64)>,
        cursor: bool,
    ) -> Result<Self, CaptureError> {
        let target = capture_target(monitors, rect)?;
        let (target, encoder_rect, parts, controls_bounds) = match target {
            Some(target) => {
                let controls = target.bounds;
                (target, rect, Vec::new(), controls)
            }
            None => {
                let rect = rect.ok_or(CaptureError::NoDisplay)?;
                let (x, y, w, h) = rect;
                let mut parts = Vec::new();
                for monitor in monitors {
                    let (mx, my, mw, mh) = monitor_bounds(monitor);
                    let left = x.max(mx);
                    let top = y.max(my);
                    let right = (x + w).min(mx + mw);
                    let bottom = (y + h).min(my + mh);
                    if right > left && bottom > top {
                        parts.push(Part {
                            name: monitor.name.clone(),
                            x: left - mx,
                            y: top - my,
                            width: right - left,
                            height: bottom - top,
                            dx: left - x,
                            dy: top - y,
                        });
                    }
                }
                if parts.is_empty() {
                    return Err(CaptureError::NoOverlap);
                }
                // Controls sit on the display with the largest captured share.
                // Python's `max` keeps the first maximal part on a tie, unlike
                // `max_by_key`; two equal-area shares must pick the earlier
                // monitor so controls placement matches the source.
                let mut chosen = parts.first();
                for part in parts.iter().skip(1) {
                    if part.width * part.height > chosen.map_or(0, |best| best.width * best.height)
                    {
                        chosen = Some(part);
                    }
                }
                let chosen = chosen.map(|part| part.name.clone()).unwrap_or_default();
                let controls = monitor_bounds(
                    monitors
                        .iter()
                        .find(|monitor| monitor.name == chosen)
                        .unwrap_or(&monitors[0]),
                );
                let target = Target {
                    output: None,
                    region: None,
                    bounds: rect,
                };
                // GSR initializes its base source from the region's center
                // before the plugin draws; a center in a display gap needs a
                // valid output for that initialization.
                let (cx, cy) = (x + w / 2, y + h / 2);
                let centered = monitors.iter().any(|monitor| {
                    let (mx, my, mw, mh) = monitor_bounds(monitor);
                    mx <= cx && cx < mx + mw && my <= cy && cy < my + mh
                });
                let encoder_rect = if centered {
                    Some(rect)
                } else {
                    let (mx, my, mw, mh) = controls;
                    Some((mx + mw / 2 - w / 2, my + mh / 2 - h / 2, w, h))
                };
                (target, encoder_rect, parts, controls)
            }
        };
        Ok(Self {
            target,
            encoder_rect,
            parts,
            controls_bounds,
            cursor,
            enabled: false,
        })
    }

    /// Mark the clean-capture source active once its plugin libraries exist.
    ///
    /// As in the source, a missing `clean-mirror.so` or `clean-capture.so`
    /// fails the take with a rebuild hint; the compositor plugin lease itself
    /// is driven by the bridge, which owns the compositor connection.
    pub fn start(&mut self) -> io::Result<()> {
        self.start_in(&super::native_dir())
    }

    /// Like [`CleanCapture::start`], checking an explicit native directory.
    pub fn start_in(&mut self, native_dir: &Path) -> io::Result<()> {
        for name in ["clean-mirror.so", "clean-capture.so"] {
            if !native_dir.join(name).is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "OmniShot's capture components need rebuilding. Run install.sh from the OmniShot folder.",
                ));
            }
        }
        self.enabled = true;
        Ok(())
    }

    pub fn stop(&mut self) {
        self.enabled = false;
    }

    /// The environment the encoder and clean-capture plugin read.
    #[must_use]
    pub fn environment(&self) -> BTreeMap<String, String> {
        let base: BTreeMap<String, String> = std::env::vars().collect();
        self.environment_with(&base)
    }

    /// Like [`CleanCapture::environment`], over a caller-supplied base env.
    ///
    /// A recording never inherits a stale socket, region, or layout from a
    /// parent process, so those keys are cleared before the live ones go in.
    #[must_use]
    pub fn environment_with(&self, base: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        const STALE: [&str; 5] = [
            "OMNISHOT_CAPTURE_SOCKET",
            "OMNISHOT_CAPTURE_REGION",
            "OMNISHOT_CAPTURE_OUTPUT",
            "OMNISHOT_CAPTURE_CURSOR",
            "OMNISHOT_CAPTURE_PARTS",
        ];
        let mut env = base.clone();
        for key in STALE {
            env.remove(key);
        }
        if !self.enabled {
            return env;
        }
        env.insert(
            "OMNISHOT_CAPTURE_CURSOR".to_owned(),
            if self.cursor { "1" } else { "0" }.to_owned(),
        );
        if self.parts.is_empty() {
            if let Some(output) = &self.target.output {
                env.insert("OMNISHOT_CAPTURE_OUTPUT".to_owned(), output.clone());
            }
        } else {
            let (w, h) = (self.target.bounds.2, self.target.bounds.3);
            let mut layout = format!("{w},{h}");
            for part in &self.parts {
                layout.push('\n');
                layout.push_str(&format!(
                    "{}\t{},{} {}x{}\t{},{}",
                    part.name, part.x, part.y, part.width, part.height, part.dx, part.dy
                ));
            }
            env.insert("OMNISHOT_CAPTURE_PARTS".to_owned(), layout);
        }
        if let Some((x, y, w, h)) = self.target.region {
            env.insert(
                "OMNISHOT_CAPTURE_REGION".to_owned(),
                format!("{x},{y} {w}x{h}"),
            );
        }
        env
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: Monitor = Monitor {
        name: String::new(),
        x: 0,
        y: 0,
        width: 0,
        height: 0,
        scale: 1.0,
        transform: 0,
        focused: false,
        dpms_status: true,
        solitary_blocked_by: Vec::new(),
    };

    fn monitor(name: &str, x: i64, y: i64, w: i64, h: i64, scale: f64) -> Monitor {
        Monitor {
            name: name.to_owned(),
            x,
            y,
            width: w,
            height: h,
            scale,
            ..LEFT.clone()
        }
    }

    fn displays() -> Vec<Monitor> {
        let mut left = monitor("left", -1800, 0, 2880, 1800, 1.6);
        let right = monitor("right", 0, 0, 1920, 1080, 1.0);
        left.focused = false;
        vec![left, right]
    }

    #[test]
    fn bounds_use_logical_pixels_and_swap_rotated_transforms() {
        let mut rotated = monitor("rot", 0, 0, 2880, 1800, 1.6);
        rotated.transform = 1;
        assert_eq!(monitor_bounds(&rotated), (0, 0, 1125, 1800));
        assert_eq!(
            monitor_bounds(&monitor("plain", 10, 20, 1920, 1080, 1.0)),
            (10, 20, 1920, 1080)
        );
    }

    #[test]
    fn spanning_region_is_split_into_display_parts() {
        let mut capture =
            CleanCapture::new(&displays(), Some((-100, 100, 300, 200)), true).unwrap();
        capture.enabled = true;
        let env = capture.environment_with(&BTreeMap::new());
        assert_eq!(
            env["OMNISHOT_CAPTURE_PARTS"],
            "300,200\nleft\t1700,100 100x200\t0,0\nright\t0,100 200x200\t100,0"
        );
        assert_eq!(env["OMNISHOT_CAPTURE_CURSOR"], "1");
        assert!(!env.contains_key("OMNISHOT_CAPTURE_OUTPUT"));
        assert_eq!(capture.target.bounds, (-100, 100, 300, 200));
    }

    #[test]
    fn single_output_reports_only_its_region() {
        let mut capture = CleanCapture::new(&displays(), Some((100, 100, 200, 200)), true).unwrap();
        capture.enabled = true;
        let env = capture.environment_with(&BTreeMap::new());
        assert_eq!(env["OMNISHOT_CAPTURE_OUTPUT"], "right");
        assert_eq!(env["OMNISHOT_CAPTURE_REGION"], "100,100 200x200");
        assert!(!env.contains_key("OMNISHOT_CAPTURE_PARTS"));
    }

    #[test]
    fn full_output_capture_names_the_focused_display() {
        let mut list = displays();
        list[1].focused = true;
        let target = capture_target(&list, None).unwrap().unwrap();
        assert_eq!(target.output.as_deref(), Some("right"));
        assert_eq!(target.region, None);
    }

    #[test]
    fn inherited_composite_layout_is_cleared() {
        let stale = BTreeMap::from([
            ("OMNISHOT_CAPTURE_PARTS".to_owned(), "stale".to_owned()),
            ("OMNISHOT_CAPTURE_SOCKET".to_owned(), "/tmp/x".to_owned()),
        ]);
        let mut capture =
            CleanCapture::new(&displays(), Some((100, 100, 200, 200)), false).unwrap();
        capture.enabled = true;
        let env = capture.environment_with(&stale);
        assert!(!env.contains_key("OMNISHOT_CAPTURE_PARTS"));
        assert!(!env.contains_key("OMNISHOT_CAPTURE_SOCKET"));
    }

    #[test]
    fn region_outside_every_display_is_rejected() {
        assert_eq!(
            CleanCapture::new(&displays(), Some((5000, 0, 200, 200)), false).unwrap_err(),
            CaptureError::NoOverlap
        );
    }

    #[test]
    fn region_spanning_a_gap_centres_the_encoder_sizing_rect() {
        let mut first = monitor("left", 0, 0, 2880, 1800, 1.6);
        first.focused = false;
        let mut second = monitor("right", 2100, 250, 1920, 1080, 1.0);
        second.focused = false;
        let mut capture =
            CleanCapture::new(&[first, second], Some((1700, 200, 600, 300)), false).unwrap();
        capture.enabled = true;
        assert_eq!(capture.encoder_rect, Some((2760, 640, 600, 300)));
        assert_eq!(capture.controls_bounds, (2100, 250, 1920, 1080));
        assert_eq!(capture.target.bounds, (1700, 200, 600, 300));
        let env = capture.environment_with(&BTreeMap::new());
        assert_eq!(
            env["OMNISHOT_CAPTURE_PARTS"],
            "600,300\nleft\t1700,200 100x300\t0,0\nright\t0,0 200x250\t400,50"
        );
    }

    #[test]
    fn rotated_display_in_a_spanning_region_keeps_its_layout() {
        let left = monitor("left", -1800, 0, 2880, 1800, 1.6);
        let mut right = monitor("right", 100, 50, 1920, 1080, 1.0);
        right.transform = 1;
        let mut capture =
            CleanCapture::new(&[left, right], Some((-100, 0, 400, 150)), false).unwrap();
        capture.enabled = true;
        assert_eq!(
            capture.environment_with(&BTreeMap::new())["OMNISHOT_CAPTURE_PARTS"],
            "400,150\nleft\t1700,0 100x150\t0,0\nright\t0,0 200x100\t200,50"
        );
    }

    #[test]
    fn too_small_a_region_is_rejected() {
        assert_eq!(
            capture_target(&displays(), Some((0, 0, 1, 10))).unwrap_err(),
            CaptureError::TooSmall
        );
    }

    #[test]
    fn equal_area_parts_choose_the_first_display_for_controls() {
        // Two identical displays side by side, split evenly across the seam.
        let left = monitor("left", 0, 0, 1920, 1080, 1.0);
        let right = monitor("right", 1920, 0, 1920, 1080, 1.0);
        let capture = CleanCapture::new(&[left, right], Some((960, 0, 1920, 1080)), false).unwrap();
        assert_eq!(capture.parts.len(), 2);
        assert_eq!(capture.parts[0].width * capture.parts[0].height, 960 * 1080);
        assert_eq!(capture.parts[1].width * capture.parts[1].height, 960 * 1080);
        // Python's max keeps the first maximal part; controls must land on the
        // left display rather than the right.
        assert_eq!(capture.controls_bounds, (0, 0, 1920, 1080));
    }

    #[test]
    fn start_requires_the_native_capture_libraries() {
        let dir =
            std::env::temp_dir().join(format!("tidyshot-display-start-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut capture = CleanCapture::new(&displays(), Some((100, 100, 200, 200)), true).unwrap();
        let err = capture.start_in(&dir).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        assert!(
            err.to_string()
                .contains("capture components need rebuilding")
        );
        assert!(!capture.enabled);

        std::fs::write(dir.join("clean-mirror.so"), b"placeholder").unwrap();
        std::fs::write(dir.join("clean-capture.so"), b"placeholder").unwrap();
        capture.start_in(&dir).unwrap();
        assert!(capture.enabled);
    }
}
