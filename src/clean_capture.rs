//! Lifecycle for the clean Hyprland mirror and GSR capture source.
// Ported from omnishot/clean_capture.py (CleanCapture)
//!
//! A recording targets one output or, when the region spans displays, a set of
//! per-output parts. When the compositor plugin cannot load, [`CleanCapture`]
//! degrades to a plain capture: it reports `false`, exposes no capture
//! variables, and leaves grim with its own cursor flag.

use std::collections::BTreeMap;

use crate::capture::{CaptureError, CaptureTarget, Display, capture_target};
use crate::commands::CommandRunner;
use crate::geometry::Rect;
use crate::mirror::{CAPTURE_PLUGIN, MIRROR_PLUGIN, MirrorError, MirrorRegistry};

/// One output slice of a cross-display recording region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturePart {
    pub name: String,
    /// Offset within the owning output, in logical pixels.
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Offset within the requested region, in logical pixels.
    pub dx: i32,
    pub dy: i32,
}

/// Why a clean capture could not be prepared.
#[derive(Debug)]
pub enum CleanCaptureError {
    /// No output overlaps the requested region.
    NoOverlap,
    /// The requested region is unusable.
    Target(CaptureError),
}

impl std::fmt::Display for CleanCaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoOverlap => {
                f.write_str("The recording area does not overlap a connected display.")
            }
            Self::Target(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CleanCaptureError {}

impl From<CaptureError> for CleanCaptureError {
    fn from(error: CaptureError) -> Self {
        Self::Target(error)
    }
}

/// A prepared clean capture: either a single output or a composite of parts.
// Ported from omnishot/clean_capture.py (CleanCapture)
#[derive(Debug)]
pub struct CleanCapture {
    /// Output name and region for a single-output target, or `None` for a
    /// composite region.
    target: Option<CaptureTarget>,
    /// The user's requested absolute region, when there is one.
    encoder_rect: Option<Rect>,
    /// The display slices of a composite region, in display order.
    parts: Vec<CapturePart>,
    /// Bounds of the display chosen to anchor the recording controls.
    controls_bounds: Rect,
    cursor: bool,
    enabled: bool,
}

impl CleanCapture {
    /// Build a capture plan for `displays` and an optional absolute region.
    pub fn new(
        displays: &[Display],
        rect: Option<Rect>,
        cursor: bool,
    ) -> Result<Self, CleanCaptureError> {
        let target = capture_target(displays, rect)?;
        let (parts, encoder_rect, controls_bounds) = match target {
            Some(ref single) => (Vec::new(), rect, single.bounds),
            None => {
                let rect = rect.expect("composite target requires a region");
                let parts = build_parts(displays, rect);
                if parts.is_empty() {
                    return Err(CleanCaptureError::NoOverlap);
                }
                let name = parts
                    .iter()
                    .max_by_key(|part| part.width * part.height)
                    .map(|part| part.name.clone())
                    .expect("parts is not empty");
                let bounds = displays
                    .iter()
                    .find(|display| display.name == name)
                    .map(|display| display.geometry.bounds())
                    .expect("part name comes from displays");
                // GSR initializes its base source from the region's center.
                // A center in a display gap needs a valid output for that
                // initialization; the plugin still captures the exact region.
                let cx = rect.x + rect.width / 2;
                let cy = rect.y + rect.height / 2;
                let centered = displays.iter().any(|display| {
                    let b = display.geometry.bounds();
                    cx >= b.x && cx < b.right() && cy >= b.y && cy < b.bottom()
                });
                let encoder_rect = if centered {
                    rect
                } else {
                    Rect::new(
                        bounds.x + bounds.width / 2 - rect.width / 2,
                        bounds.y + bounds.height / 2 - rect.height / 2,
                        rect.width,
                        rect.height,
                    )
                };
                (parts, Some(encoder_rect), bounds)
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

    /// The target this capture will record.
    #[must_use]
    pub fn target(&self) -> Option<&CaptureTarget> {
        self.target.as_ref()
    }

    /// The display slices of a composite region.
    #[must_use]
    pub fn parts(&self) -> &[CapturePart] {
        &self.parts
    }

    /// Bounds of the display anchoring the recording controls.
    #[must_use]
    pub fn controls_bounds(&self) -> Rect {
        self.controls_bounds
    }

    /// Whether the clean source is active.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Start the clean capture, degrading to a plain one when the compositor
    /// plugin cannot load.
    ///
    /// Returns `true` when the clean source is active. The caller then uses
    /// [`CleanCapture::environment`].
    // Ported from omnishot/clean_capture.py (CleanCapture.start)
    pub fn start<R: CommandRunner>(
        &mut self,
        registry: &MirrorRegistry,
        runner: &R,
        native: &std::path::Path,
    ) -> Result<bool, MirrorError> {
        for name in [MIRROR_PLUGIN, CAPTURE_PLUGIN] {
            if !native.join(name).is_file() {
                return Err(MirrorError::Rebuild(crate::mirror::REBUILD.to_owned()));
            }
        }
        let mut lease = registry.lease("clean_capture");
        if let Err(MirrorError::Unavailable(_)) = lease.start(runner, native) {
            // Record the plain output instead; the controls stay outside
            // region captures or visible in full-display captures.
            return Ok(false);
        }
        drop(lease);
        self.enabled = true;
        Ok(true)
    }

    /// Stop the clean source.
    pub fn stop<R: CommandRunner>(
        &mut self,
        registry: &MirrorRegistry,
        runner: &R,
    ) -> Result<(), crate::commands::CommandError> {
        if self.enabled {
            let mut lease = registry.lease("clean_capture");
            lease.stop(runner)?;
            self.enabled = false;
        } else {
            registry.release_unused(runner)?;
        }
        Ok(())
    }

    /// Environment additions for the recorder, with inherited capture
    /// variables stripped so a stale region or test socket cannot leak in.
    // Ported from omnishot/clean_capture.py (CleanCapture.environment)
    #[must_use]
    pub fn environment(&self, inherited: &BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut env = inherited.clone();
        for key in [
            "OMNISHOT_CAPTURE_SOCKET",
            "OMNISHOT_CAPTURE_REGION",
            "OMNISHOT_CAPTURE_OUTPUT",
            "OMNISHOT_CAPTURE_CURSOR",
            "OMNISHOT_CAPTURE_PARTS",
        ] {
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
            if let Some(target) = &self.target
                && let Some(name) = &target.name
            {
                env.insert("OMNISHOT_CAPTURE_OUTPUT".to_owned(), name.clone());
            }
            if let Some(target) = &self.target
                && let Some(region) = target.relative
            {
                env.insert(
                    "OMNISHOT_CAPTURE_REGION".to_owned(),
                    format!(
                        "{},{} {}x{}",
                        region.x, region.y, region.width, region.height
                    ),
                );
            }
        } else if let Some(rect) = self.encoder_rect {
            let mut value = format!("{},{}", rect.width, rect.height);
            for part in &self.parts {
                value.push('\n');
                value.push_str(&format!(
                    "{}\t{},{} {}x{}\t{},{}",
                    part.name, part.x, part.y, part.width, part.height, part.dx, part.dy
                ));
            }
            env.insert("OMNISHOT_CAPTURE_PARTS".to_owned(), value);
        }
        env
    }
}

fn build_parts(displays: &[Display], rect: Rect) -> Vec<CapturePart> {
    let mut parts = Vec::new();
    for display in displays {
        let bounds = display.geometry.bounds();
        let left = rect.x.max(bounds.x);
        let top = rect.y.max(bounds.y);
        let right = rect.right().min(bounds.right());
        let bottom = rect.bottom().min(bounds.bottom());
        if right > left && bottom > top {
            parts.push(CapturePart {
                name: display.name.clone(),
                x: left - bounds.x,
                y: top - bounds.y,
                width: right - left,
                height: bottom - top,
                dx: left - rect.x,
                dy: top - rect.y,
            });
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandError, CommandOutput, CommandSpec};
    use crate::geometry::MonitorGeometry;
    use std::path::PathBuf;
    use std::sync::Mutex;

    fn monitor(name: &str, x: i32, y: i32) -> Display {
        Display::new(
            name,
            MonitorGeometry {
                x,
                y,
                width: 1920,
                height: 1080,
                scale: 1.0,
                transform: 0,
            },
        )
        .focused()
    }

    /// A runner that emulates the plugin endpoints used by the lease.
    #[derive(Default)]
    struct Fake {
        loaded: Mutex<bool>,
        calls: Mutex<Vec<Vec<String>>>,
        degrade: Mutex<bool>,
    }

    impl Fake {
        fn degraded() -> Self {
            let fake = Self::default();
            *fake.degrade.lock().unwrap() = true;
            fake
        }

        fn plugin_calls(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|args| args.first().map(String::as_str) == Some("plugin"))
                .filter_map(|args| args.get(1).cloned())
                .collect()
        }
    }

    impl CommandRunner for Fake {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            self.calls.lock().unwrap().push(spec.args.clone());
            let out = |text: &str| CommandOutput {
                status: 0,
                stdout: text.as_bytes().to_vec(),
                stderr: Vec::new(),
            };
            match spec.args.as_slice() {
                [flag, query]
                    if spec.program == "hyprctl" && flag == "-j" && query == "plugin list" =>
                {
                    let loaded = *self.loaded.lock().unwrap();
                    Ok(out(if loaded {
                        "[{\"name\":\"omnishot-clean-mirror\"}]"
                    } else {
                        "[]"
                    }))
                }
                [action, sub, ..]
                    if spec.program == "hyprctl" && action == "plugin" && sub == "load" =>
                {
                    if *self.degrade.lock().unwrap() {
                        Ok(out("Could not hook IElementRenderer::preDrawSurface"))
                    } else {
                        *self.loaded.lock().unwrap() = true;
                        Ok(out("ok"))
                    }
                }
                [action, sub, ..]
                    if spec.program == "hyprctl" && action == "plugin" && sub == "unload" =>
                {
                    *self.loaded.lock().unwrap() = false;
                    Ok(out("ok"))
                }
                [action, ..] if spec.program == "hyprctl" && action == "repl" => Ok(out("false")),
                _ => Ok(out("")),
            }
        }
    }

    fn native_dir() -> TempNative {
        TempNative::new()
    }

    struct TempNative(PathBuf);

    impl TempNative {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let path = std::env::temp_dir().join(format!("tidyshot-clean-{nonce}"));
            std::fs::create_dir_all(&path).unwrap();
            for name in [MIRROR_PLUGIN, CAPTURE_PLUGIN] {
                std::fs::write(path.join(name), b"plugin").unwrap();
            }
            Self(path)
        }
    }

    impl Drop for TempNative {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn single_output_plan_and_environment() {
        let displays = [monitor("display", 0, 0)];
        let mut capture =
            CleanCapture::new(&displays, Some(Rect::new(10, 20, 640, 400)), true).unwrap();
        assert_eq!(capture.parts(), &[]);
        let registry = MirrorRegistry::new();
        let runner = Fake::default();
        let native = native_dir();
        assert!(capture.start(&registry, &runner, &native.0).unwrap());
        assert!(capture.enabled());
        let env = capture.environment(&BTreeMap::new());
        assert_eq!(env["OMNISHOT_CAPTURE_OUTPUT"], "display");
        assert_eq!(env["OMNISHOT_CAPTURE_REGION"], "10,20 640x400");
        assert_eq!(env["OMNISHOT_CAPTURE_CURSOR"], "1");
    }

    #[test]
    fn environment_strips_inherited_capture_variables() {
        let displays = [monitor("display", 0, 0)];
        let capture = CleanCapture::new(&displays, None, false).unwrap();
        let mut inherited = BTreeMap::new();
        inherited.insert("OMNISHOT_CAPTURE_SOCKET".to_owned(), "/test".to_owned());
        inherited.insert("OMNISHOT_CAPTURE_REGION".to_owned(), "bad".to_owned());
        inherited.insert("KEEP".to_owned(), "yes".to_owned());
        let env = capture.environment(&inherited);
        assert!(!env.contains_key("OMNISHOT_CAPTURE_SOCKET"));
        assert!(!env.contains_key("OMNISHOT_CAPTURE_REGION"));
        assert_eq!(env["KEEP"], "yes");
        assert!(
            !env.contains_key("OMNISHOT_CAPTURE_OUTPUT"),
            "disabled adds nothing"
        );
    }

    #[test]
    fn aarch64_rejection_degrades_and_omits_capture_output() {
        let displays = [monitor("display", 0, 0)];
        let mut capture = CleanCapture::new(&displays, None, false).unwrap();
        let registry = MirrorRegistry::with_arch("aarch64");
        let runner = Fake::degraded();
        let native = native_dir();
        assert!(!capture.start(&registry, &runner, &native.0).unwrap());
        assert!(!capture.enabled());
        let env = capture.environment(&BTreeMap::new());
        assert!(!env.contains_key("OMNISHOT_CAPTURE_OUTPUT"));
        assert_eq!(runner.plugin_calls(), ["load"], "load attempted once");
    }

    #[test]
    fn composite_region_produces_parts_and_encoder_rect() {
        let displays = [
            Display::new(
                "left",
                MonitorGeometry {
                    x: -1920,
                    y: 0,
                    width: 1920,
                    height: 1080,
                    scale: 1.0,
                    transform: 0,
                },
            )
            .focused(),
            Display::new(
                "right",
                MonitorGeometry {
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                    scale: 1.0,
                    transform: 0,
                },
            ),
        ];
        let rect = Rect::new(-100, 100, 300, 200);
        let capture = CleanCapture::new(&displays, Some(rect), false).unwrap();
        assert!(capture.target().is_none(), "spans outputs");
        assert_eq!(capture.parts().len(), 2);
        assert_eq!(capture.parts()[0].dx, 0);
        assert_eq!(capture.parts()[1].dx, 100);
        assert_eq!(capture.controls_bounds().width, 1920);
    }

    #[test]
    fn empty_overlap_is_rejected() {
        let displays = [monitor("display", 0, 0)];
        // capture_target rejects non-overlapping single-output regions by
        // returning None; build_parts then finds no overlap.
        let error = CleanCapture::new(&displays, Some(Rect::new(5000, 5000, 100, 100)), false)
            .expect_err("region outside every display");
        assert!(matches!(error, CleanCaptureError::NoOverlap), "{error:?}");
    }
}
