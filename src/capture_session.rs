//! Screenshot capture modes, slurp selection and the cancelable countdown.
// Ported from omnishot/app.py (capture), omnishot/backend.py (grab,
// select_region, capture_monitors) and omnishot/capture_countdown.py
//!
//! A capture command picks a mode, optionally parses a `--geometry` region and
//! waits out its countdown before grabbing the pixels through grim. Interactive
//! modes either run the live slurp selector or hand the selection to the QML
//! overlay. Every external program runs through [`CommandRunner`], so the
//! behavior is asserted without a compositor.

use std::fmt;
use std::thread;
use std::time::{Duration, Instant};

use crate::capture::{CaptureError, Display, capture_target};
use crate::commands::{CommandError, CommandRunner, CommandSpec};
use crate::geometry::{GeometryError, Rect, geometry_text, parse_geometry};
use crate::json::Value;
use crate::mirror::{MirrorError, MirrorRegistry};

/// Capture actions offered by the CLI and URL commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    /// Interactive area selection with an overlay.
    Select,
    /// Area selection started by the area shortcut.
    Area,
    /// Fullscreen on the focused output.
    Fullscreen,
    /// The whole desktop across every output.
    Desktop,
    /// The previously remembered area.
    Previous,
    /// The untouched frozen frame at the whole desktop.
    Window,
    /// A countdown, then capture.
    Timer,
    /// Scrolling capture.
    Scroll,
    /// Horizontal scrolling capture.
    ScrollHorizontal,
    /// Reuse the area captured last.
    Ocr,
}

impl CaptureMode {
    /// Resolve a command word to a capture mode.
    #[must_use]
    pub fn from_command(command: &str) -> Option<Self> {
        match command {
            "select" | "all-in-one" => Some(Self::Select),
            "area" => Some(Self::Area),
            "fullscreen" => Some(Self::Fullscreen),
            "desktop" => Some(Self::Desktop),
            "previous" => Some(Self::Previous),
            "window" => Some(Self::Window),
            "timer" => Some(Self::Timer),
            "scroll" => Some(Self::Scroll),
            "scroll-horizontal" => Some(Self::ScrollHorizontal),
            "ocr" => Some(Self::Ocr),
            _ => None,
        }
    }

    /// The command word this mode came from.
    #[must_use]
    pub const fn command(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Area => "area",
            Self::Fullscreen => "fullscreen",
            Self::Desktop => "desktop",
            Self::Previous => "previous",
            Self::Window => "window",
            Self::Timer => "timer",
            Self::Scroll => "scroll",
            Self::ScrollHorizontal => "scroll-horizontal",
            Self::Ocr => "ocr",
        }
    }

    /// Modes whose pixels come from a scrolling session rather than one grab.
    #[must_use]
    pub const fn is_scrolling(self) -> bool {
        matches!(self, Self::Scroll | Self::ScrollHorizontal)
    }
}

/// Why a capture request could not be completed.
#[derive(Debug)]
pub enum CaptureRequestError {
    /// The mode word is not a capture command.
    UnknownMode(String),
    /// The compositor is not in a capturable state.
    Compositor(String),
    /// A geometry string was malformed.
    Geometry(GeometryError),
    /// The delay was outside the accepted range.
    Delay(i64),
    /// The user dismissed the countdown before any pixels were taken.
    Interrupted,
    /// No display is available or the region does not fit one.
    Target(CaptureError),
    /// slurp was cancelled or its output was not a region.
    Selection,
    /// The compositor mirror could not be established.
    Mirror(MirrorError),
    /// grim or slurp failed.
    Command(CommandError),
}

impl fmt::Display for CaptureRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownMode(mode) => write!(f, "Unknown OmniShot capture mode: {mode}"),
            Self::Compositor(message) => f.write_str(message),
            Self::Geometry(error) => error.fmt(f),
            Self::Delay(delay) => write!(
                f,
                "Capture delay must be between 0 and 86400 seconds (got {delay})"
            ),
            Self::Interrupted => f.write_str("capture cancelled"),
            Self::Target(error) => error.fmt(f),
            Self::Selection => f.write_str("selection cancelled"),
            Self::Mirror(error) => error.fmt(f),
            Self::Command(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CaptureRequestError {}

impl From<GeometryError> for CaptureRequestError {
    fn from(error: GeometryError) -> Self {
        Self::Geometry(error)
    }
}

impl From<CaptureError> for CaptureRequestError {
    fn from(error: CaptureError) -> Self {
        Self::Target(error)
    }
}

impl From<CommandError> for CaptureRequestError {
    fn from(error: CommandError) -> Self {
        Self::Command(error)
    }
}

/// The maximum accepted capture delay, matching the source.
pub const MAX_DELAY: i64 = 86_400;

/// A resolved capture request: what to capture and when.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureRequest {
    pub mode: CaptureMode,
    /// Absolute logical region, when the user or the CLI supplied one.
    pub rect: Option<Rect>,
    /// Delay in seconds before the pixels are taken.
    pub delay: i64,
}

impl CaptureRequest {
    /// Build a request from a command word and optional CLI arguments.
    // Ported from omnishot/app.py (capture)
    pub fn resolve(
        command: &str,
        geometry: Option<&str>,
        delay: Option<i64>,
        default_delay: i64,
    ) -> Result<Self, CaptureRequestError> {
        let mode = CaptureMode::from_command(command).ok_or_else(|| Self::unknown(command))?;
        let rect = match geometry {
            Some(text) => Some(parse_geometry(text)?),
            None => None,
        };
        let delay = match mode {
            CaptureMode::Timer => delay.unwrap_or(default_delay),
            _ => delay.unwrap_or(0),
        };
        if !(0..=MAX_DELAY).contains(&delay) {
            return Err(CaptureRequestError::Delay(delay));
        }
        Ok(Self { mode, rect, delay })
    }

    fn unknown(command: &str) -> CaptureRequestError {
        CaptureRequestError::UnknownMode(command.to_owned())
    }

    /// Whether the request needs an interactive selection before grabbing.
    ///
    /// A frozen `select`, or a timer with no geometry, is chosen from the
    /// overlay that the caller shows after [`CaptureOutcome::NeedsSelection`].
    /// An unfrozen `select` runs the live slurp selector inside the capture;
    /// an unfrozen `area`/`timer` does too unless the compositor mirror is
    /// unavailable, in which case it degrades to the frozen overlay.
    #[must_use]
    pub fn needs_selection(&self, freeze: bool, selection_live: bool) -> bool {
        match self.mode {
            CaptureMode::Select => freeze || !selection_live,
            CaptureMode::Area | CaptureMode::Timer | CaptureMode::Ocr => {
                self.rect.is_none() && (freeze || !selection_live)
            }
            _ => false,
        }
    }

    /// The initial selection hint, if any.
    #[must_use]
    pub fn initial(&self, remembered: Option<Rect>, remember_selection: bool) -> Option<Rect> {
        if let Some(rect) = self.rect {
            return Some(rect);
        }
        match self.mode {
            CaptureMode::Select if remember_selection => remembered,
            _ => None,
        }
    }
}

/// Which outputs are available and under which monitor the pointer sits.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionDisplays {
    pub displays: Vec<Display>,
    pub cursor: crate::api::Point,
}

/// Run a capture through the supplied command runner and mirror registry.
///
/// `selection_runner` performs the interactive slurp step when the mode needs
/// it; the returned bytes are a PNG encoded by grim. The step is separate so
/// the capture itself stays testable without a display server.
pub struct CaptureSession<'a, R: CommandRunner> {
    pub runner: &'a R,
    pub mirror: &'a MirrorRegistry,
    pub native: std::path::PathBuf,
}

impl<R: CommandRunner> CaptureSession<'_, R> {
    /// Confirm the compositor can be captured at all.
    // Ported from omnishot/backend.py (capture_monitors)
    pub fn capture_ready(&self, monitors: &[Value]) -> Result<(), CaptureRequestError> {
        if monitors
            .iter()
            .any(|monitor| solitary_locked(monitor).unwrap_or(false))
        {
            return Err(CaptureRequestError::Compositor(
                "Unlock the desktop before capturing.".to_owned(),
            ));
        }
        if !monitors.is_empty()
            && monitors.iter().all(|monitor| {
                !monitor
                    .get("dpmsStatus")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
            })
        {
            return Err(CaptureRequestError::Compositor(
                "The display is asleep. Wake it before capturing.".to_owned(),
            ));
        }
        Ok(())
    }

    /// Grab the pixels for `rect` (or the whole output named by `output`).
    // Ported from omnishot/backend.py (grab)
    pub fn grab(
        &self,
        rect: Option<Rect>,
        cursor: bool,
        output: Option<&str>,
        scale: Option<f64>,
    ) -> Result<Vec<u8>, CaptureRequestError> {
        let mut args: Vec<String> = Vec::new();
        if let Some(rect) = rect {
            args.push("-g".to_owned());
            args.push(geometry_text(rect));
        }
        if cursor {
            args.push("-c".to_owned());
        }
        if let Some(output) = output {
            args.push("-o".to_owned());
            args.push(output.to_owned());
        }
        if let Some(scale) = scale {
            args.push("-s".to_owned());
            args.push(scale.to_string());
        }
        args.extend(["-t".to_owned(), "png".to_owned(), "-".to_owned()]);
        let spec = CommandSpec::new("grim")
            .args(args)
            .timeout(Duration::from_secs(15));

        // Hyprland can bake a software pointer into the output even without
        // grim's -c, so the clean mirror runs for both cursor modes.
        let mut cursor_mirror = self.mirror.lease("cursor_capture");
        cursor_mirror
            .start_best_effort(self.runner, &self.native)
            .map_err(CaptureRequestError::Mirror)?;
        let result = self.runner.run(&spec);
        let stop = cursor_mirror.stop(self.runner);
        let output = result?;
        stop?;
        Ok(output.stdout)
    }

    /// Grab every visible output as a set of full-desktop frames.
    // Ported from omnishot/backend.py (grab per monitor)
    pub fn grab_monitors(
        &self,
        displays: &[Display],
        cursor: bool,
    ) -> Result<Vec<(Display, Vec<u8>)>, CaptureRequestError> {
        let mut frames = Vec::with_capacity(displays.len());
        for display in displays {
            let frame = self.grab(
                None,
                cursor,
                Some(&display.name),
                Some(display.geometry.scale),
            )?;
            frames.push((display.clone(), frame));
        }
        Ok(frames)
    }

    /// Choose a whole output or a single-output region for recording.
    pub fn target(
        &self,
        displays: &[Display],
        rect: Option<Rect>,
    ) -> Result<crate::capture::CaptureTarget, CaptureRequestError> {
        match capture_target(displays, rect)? {
            Some(target) => Ok(target),
            None => Err(CaptureRequestError::Compositor(
                "The region spans more than one display".to_owned(),
            )),
        }
    }
}

fn solitary_locked(monitor: &Value) -> Option<bool> {
    let blocked = monitor.get("solitaryBlockedBy")?.as_array()?;
    Some(blocked.iter().any(|entry| {
        entry
            .as_str()
            .is_some_and(|name| name.to_ascii_uppercase().contains("LOCK"))
    }))
}

/// Build capture displays from the `hyprctl -j monitors` rows.
// Ported from omnishot/backend.py (capture_monitors)
#[must_use]
pub fn displays_from_monitors(monitors: &[Value]) -> Vec<Display> {
    let mut displays: Vec<Display> = monitors
        .iter()
        .filter_map(|monitor| {
            let name = monitor.get("name").and_then(Value::as_str)?;
            let geometry = crate::geometry::MonitorGeometry {
                x: number(monitor, "x"),
                y: number(monitor, "y"),
                width: number(monitor, "width"),
                height: number(monitor, "height"),
                scale: monitor.get("scale").and_then(Value::as_f64).unwrap_or(1.0),
                transform: number(monitor, "transform"),
            };
            let focused = monitor
                .get("focused")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Some(Display {
                name: name.to_owned(),
                geometry,
                focused,
            })
        })
        .collect();
    // A single connected output is the implicit focused one.
    if displays.len() == 1 {
        displays[0].focused = true;
    }
    displays
}

fn number(value: &Value, key: &str) -> i32 {
    value
        .get(key)
        .and_then(Value::as_f64)
        .map_or(0, |number| number as i32)
}

/// Per-run capture options that come from settings rather than the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureOptions {
    /// Whether grim bakes the cursor into the frame.
    pub cursor: bool,
    /// Whether the selector follows a frozen frame instead of the live desktop.
    pub freeze: bool,
}

/// The pixels and region a completed capture produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrabbedFrame {
    /// Absolute region the pixels cover, when the mode selected one.
    pub rect: Option<Rect>,
    /// PNG bytes as grim encoded them.
    pub png: Vec<u8>,
}

/// The result of running a capture request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureOutcome {
    /// An interactive selector is needed; the caller shows it and retries.
    NeedsSelection,
    /// A scrolling session should start for this region.
    Scroll {
        rect: Option<Rect>,
        horizontal: bool,
    },
    /// Every visible output, for a composite desktop capture.
    Frames(Vec<GrabbedFrame>),
    /// The pixels were captured.
    Grabbed(GrabbedFrame),
}

/// Run a resolved capture request against the compositor.
///
/// A non-timer delay is waited out first, through the caller's `on_tick` and
/// `should_cancel` hooks; an interactive `select` then runs the live slurp
/// selector when the mirror is available. The self timer counts down after its
/// selection, just before the grab, matching `timed_capture`. Modes whose
/// selection is frozen return [`CaptureOutcome::NeedsSelection`] before touching
/// grim, so the QML overlay owns the pointer. Scrolling modes return a plan for
/// the stitcher instead of a single frame.
// Ported from omnishot/app.py (capture)
pub fn run_capture<R: CommandRunner>(
    session: &CaptureSession<'_, R>,
    monitors: &[Value],
    request: &CaptureRequest,
    options: CaptureOptions,
    on_tick: impl FnMut(&str),
    should_cancel: impl FnMut() -> bool,
) -> Result<CaptureOutcome, CaptureRequestError> {
    let CaptureOptions { cursor, freeze } = options;
    session.capture_ready(monitors)?;
    // The self timer owns its countdown after the selection; every other mode
    // waits before the selection, matching `QTimer.singleShot(int(delay*1000))`.
    let mut on_tick = on_tick;
    let mut should_cancel = should_cancel;
    let timer = request.mode == CaptureMode::Timer;
    if !timer
        && run_countdown(request.delay, &mut on_tick, &mut should_cancel)
            == CountdownOutcome::Cancelled
    {
        return Err(CaptureRequestError::Interrupted);
    }
    let displays = displays_from_monitors(monitors);
    if request.mode.is_scrolling() {
        return Ok(CaptureOutcome::Scroll {
            rect: request.rect,
            horizontal: request.mode == CaptureMode::ScrollHorizontal,
        });
    }
    if request.mode == CaptureMode::Previous && request.rect.is_none() {
        return Err(CaptureRequestError::Compositor(
            "Capture an area first".to_owned(),
        ));
    }
    // The mirror is only consulted for a mode that may run a live selector.
    let needs_plan = request.mode == CaptureMode::Select
        || (matches!(
            request.mode,
            CaptureMode::Area | CaptureMode::Timer | CaptureMode::Ocr
        ) && request.rect.is_none());
    let plan = if needs_plan {
        plan_selection(session.mirror, session.runner, &session.native, freeze)
    } else {
        SelectionPlan {
            live: false,
            mirror: false,
        }
    };
    if request.needs_selection(freeze, plan.live) {
        return Ok(CaptureOutcome::NeedsSelection);
    }
    // A live selection runs slurp itself; fullscreen/desktop/window keep
    // their output-based grab and every other mode uses the selected region.
    let mut rect = request.rect;
    if rect.is_none()
        && matches!(
            request.mode,
            CaptureMode::Select | CaptureMode::Area | CaptureMode::Timer | CaptureMode::Ocr
        )
    {
        rect = Some(slurp_region(session.runner, false, &[])?);
    }
    // The self timer counts down here, after the area exists and before the
    // pixels are taken, matching `timed_capture`.
    if timer
        && run_countdown(request.delay, &mut on_tick, &mut should_cancel)
            == CountdownOutcome::Cancelled
    {
        return Err(CaptureRequestError::Interrupted);
    }
    match request.mode {
        CaptureMode::Fullscreen => {
            let target = session.target(&displays, None)?;
            let name = target.name.clone();
            let scale = target
                .name
                .as_deref()
                .and_then(|name| displays.iter().find(|d| d.name == name))
                .map(|display| display.geometry.scale);
            let png = session.grab(None, cursor, name.as_deref(), scale)?;
            Ok(CaptureOutcome::Grabbed(GrabbedFrame {
                rect: Some(target.bounds),
                png,
            }))
        }
        CaptureMode::Desktop => {
            let mut widest: Option<Rect> = None;
            let mut frames: Vec<(Display, Vec<u8>)> = Vec::new();
            for display in &displays {
                let bounds = display.geometry.bounds();
                widest = Some(match widest {
                    Some(current) if current.contains(&bounds) => current,
                    Some(current) => {
                        let right = current.right().max(bounds.right());
                        let bottom = current.bottom().max(bounds.bottom());
                        let left = current.x.min(bounds.x);
                        let top = current.y.min(bounds.y);
                        Rect::new(left, top, right - left, bottom - top)
                    }
                    None => bounds,
                });
                let png = session.grab(
                    None,
                    cursor,
                    Some(&display.name),
                    Some(display.geometry.scale),
                )?;
                frames.push((display.clone(), png));
            }
            // A single output is a plain frame; several outputs are returned
            // separately for the caller to composite into the desktop region.
            if frames.len() == 1 {
                return Ok(CaptureOutcome::Grabbed(GrabbedFrame {
                    rect: widest,
                    png: frames.pop().expect("one frame").1,
                }));
            }
            Ok(CaptureOutcome::Frames(
                frames
                    .into_iter()
                    .map(|(display, png)| GrabbedFrame {
                        rect: Some(display.geometry.bounds()),
                        png,
                    })
                    .collect(),
            ))
        }
        CaptureMode::Window => {
            // The frozen desktop frame is grabbed like a fullscreen capture;
            // window hit-testing runs in the overlay.
            let target = session.target(&displays, None)?;
            let png = session.grab(None, cursor, target.name.as_deref(), None)?;
            Ok(CaptureOutcome::Grabbed(GrabbedFrame {
                rect: Some(target.bounds),
                png,
            }))
        }
        _ => {
            let output = rect
                .and_then(|rect| session.target(&displays, Some(rect)).ok())
                .and_then(|target| target.name);
            let png = session.grab(rect, cursor, output.as_deref(), None)?;
            Ok(CaptureOutcome::Grabbed(GrabbedFrame { rect, png }))
        }
    }
}

/// Run slurp and parse its `X,Y WxH` output.
///
/// An empty result or a non-zero exit is reported as a cancellation, matching
/// the source's handling of a dismissed selector.
// Ported from omnishot/backend.py (select_region)
pub fn slurp_region<R: CommandRunner>(
    runner: &R,
    window: bool,
    clients: &[Rect],
) -> Result<Rect, CaptureRequestError> {
    let mut spec = CommandSpec::new("slurp")
        .args(["-b", "#00000000", "-c", "#ffffffff", "-w", "2"])
        .timeout(Duration::from_secs(180));
    if window {
        let rows: String = clients
            .iter()
            .map(|rect| format!("{}\n", geometry_text(*rect)))
            .collect();
        spec = spec.args(["-r"]).stdin(rows.into_bytes());
    }
    let result = runner.run(&spec).map_err(|error| match error {
        CommandError::Failure { .. } => CaptureRequestError::Selection,
        other => CaptureRequestError::Command(other),
    })?;
    let text = result.stdout_string();
    if text.trim().is_empty() {
        return Err(CaptureRequestError::Selection);
    }
    parse_geometry(&text).map_err(CaptureRequestError::Geometry)
}

/// The state a countdown reaches when it stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountdownOutcome {
    /// The delay elapsed and the capture should proceed.
    Finished,
    /// The user cancelled before the delay elapsed.
    Cancelled,
}

/// A countdown whose remaining time is computed from a monotonic deadline.
///
/// The UI renders [`Countdown::remaining_seconds`] and calls [`Countdown::tick`]
/// on its timer; the deadline arithmetic keeps a delayed tick from extending
/// the wait.
// Ported from omnishot/capture_countdown.py (CaptureCountdown)
pub struct Countdown {
    deadline: Instant,
    terminal: bool,
    seconds: i64,
}

impl Countdown {
    /// Start a countdown of `seconds`, clamped to the accepted range.
    #[must_use]
    pub fn start(seconds: i64) -> Self {
        let seconds = seconds.clamp(0, MAX_DELAY);
        Self {
            deadline: Instant::now() + Duration::from_secs(seconds as u64),
            terminal: false,
            seconds,
        }
    }

    /// The configured delay, in seconds.
    #[must_use]
    pub const fn seconds(&self) -> i64 {
        self.seconds
    }

    /// Whole seconds still remaining, rounded up like the source's label.
    #[must_use]
    pub fn remaining_seconds(&self) -> i64 {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        remaining.as_secs_f64().ceil() as i64
    }

    /// The text the countdown window displays.
    #[must_use]
    pub fn label(&self) -> String {
        format!("Capturing in {}…", self.remaining_seconds())
    }

    /// Advance the countdown. Returns [`CountdownOutcome::Finished`] once when
    /// the deadline has passed, and [`CountdownOutcome::Cancelled`] after
    /// [`Countdown::cancel`].
    pub fn tick(&mut self) -> Option<CountdownOutcome> {
        if self.terminal {
            return None;
        }
        if Instant::now() >= self.deadline {
            self.terminal = true;
            return Some(CountdownOutcome::Finished);
        }
        None
    }

    /// Cancel the countdown before it finishes.
    pub fn cancel(&mut self) -> CountdownOutcome {
        if !self.terminal {
            self.terminal = true;
        }
        CountdownOutcome::Cancelled
    }

    /// Whether the countdown has stopped, by completion or cancellation.
    #[must_use]
    pub const fn terminal(&self) -> bool {
        self.terminal
    }
}

/// How a selection overlay should be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionPlan {
    /// Whether the selector follows the live desktop instead of a frozen frame.
    pub live: bool,
    /// Whether the selection mirror must be held while the overlay is open.
    pub mirror: bool,
}

/// Choose the selection strategy, degrading live selection when the plugin
/// cannot load.
///
/// A live overlay would appear in its own capture, so when the mirror is
/// unavailable the selection freezes instead of following the desktop.
// Ported from omnishot/app.py (frozen_selection) and omnishot/clean_capture.py
pub fn plan_selection<R: CommandRunner>(
    registry: &MirrorRegistry,
    runner: &R,
    native: &std::path::Path,
    freeze: bool,
) -> SelectionPlan {
    if freeze {
        return SelectionPlan {
            live: false,
            mirror: false,
        };
    }
    let mut lease = registry.lease("selection_capture");
    match lease.start(runner, native) {
        Ok(()) => {
            let _ = lease.stop(runner);
            SelectionPlan {
                live: true,
                mirror: true,
            }
        }
        Err(MirrorError::Unavailable(_)) => SelectionPlan {
            live: false,
            mirror: false,
        },
        Err(_) => SelectionPlan {
            live: true,
            mirror: true,
        },
    }
}

/// Wait out a countdown on the calling thread, emitting its label.
///
/// Used by worker threads; the QML layer drives [`Countdown`] directly when a
/// visible timer window is wanted.
pub fn run_countdown(
    seconds: i64,
    mut on_tick: impl FnMut(&str),
    mut should_cancel: impl FnMut() -> bool,
) -> CountdownOutcome {
    let mut countdown = Countdown::start(seconds);
    loop {
        if should_cancel() {
            return countdown.cancel();
        }
        on_tick(&countdown.label());
        if let Some(outcome) = countdown.tick() {
            return outcome;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CommandOutput;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    fn output(stdout: &str) -> CommandOutput {
        CommandOutput {
            status: 0,
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    #[derive(Default)]
    struct Fake {
        results: Mutex<VecDeque<Result<CommandOutput, CommandError>>>,
        seen: Mutex<Vec<CommandSpec>>,
    }

    impl Fake {
        fn push(&self, stdout: &str) {
            self.results.lock().unwrap().push_back(Ok(output(stdout)));
        }
    }

    impl CommandRunner for Fake {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            self.seen.lock().unwrap().push(spec.clone());
            // Model the compositor endpoints the mirror uses; every other
            // command takes the next queued result.
            if spec.program == "hyprctl" {
                match spec.args.as_slice() {
                    [flag, query] if flag == "-j" && query == "plugin list" => {
                        return Ok(output("[]"));
                    }
                    [action, sub, ..] if action == "plugin" && sub == "load" => {
                        return Ok(output("Could not hook IElementRenderer::preDrawSurface"));
                    }
                    [action, ..] if action == "repl" => return Ok(output("false")),
                    _ => return Ok(output("")),
                }
            }
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(output("")))
        }
    }

    fn degraded_registry() -> MirrorRegistry {
        // aarch64 cannot hook functions, so grab must degrade cleanly.
        MirrorRegistry::with_arch("aarch64")
    }

    /// A fake that loads the mirror and queues results for every other program.
    #[derive(Default)]
    struct Live {
        loaded: std::sync::atomic::AtomicBool,
        results: Mutex<VecDeque<Result<CommandOutput, CommandError>>>,
        seen: Mutex<Vec<CommandSpec>>,
    }

    impl Live {
        fn push(&self, stdout: &str) {
            self.results.lock().unwrap().push_back(Ok(output(stdout)));
        }
    }

    impl CommandRunner for Live {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            use std::sync::atomic::Ordering;
            self.seen.lock().unwrap().push(spec.clone());
            if spec.program == "hyprctl" {
                let out = |text: &str| CommandOutput {
                    status: 0,
                    stdout: text.as_bytes().to_vec(),
                    stderr: Vec::new(),
                };
                return match spec.args.as_slice() {
                    [flag, query] if flag == "-j" && query == "plugin list" => {
                        if self.loaded.load(Ordering::SeqCst) {
                            Ok(out("[{\"name\":\"omnishot-clean-mirror\"}]"))
                        } else {
                            Ok(out("[]"))
                        }
                    }
                    [action, sub, ..] if action == "plugin" && sub == "load" => {
                        self.loaded.store(true, Ordering::SeqCst);
                        Ok(out("ok"))
                    }
                    _ => Ok(out("false")),
                };
            }
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(output("")))
        }
    }

    fn display(name: &str, x: i32, y: i32) -> Display {
        Display::new(
            name,
            crate::geometry::MonitorGeometry {
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

    #[test]
    fn modes_resolve_from_commands_and_reject_unknown() {
        assert_eq!(
            CaptureRequest::resolve("all-in-one", None, None, 3)
                .unwrap()
                .mode,
            CaptureMode::Select
        );
        assert_eq!(
            CaptureRequest::resolve("area", None, None, 3).unwrap().mode,
            CaptureMode::Area
        );
        assert_eq!(
            CaptureRequest::resolve("timer", None, None, 3)
                .unwrap()
                .delay,
            3
        );
        assert_eq!(
            CaptureRequest::resolve("timer", None, Some(10), 3)
                .unwrap()
                .delay,
            10
        );
        assert_eq!(
            CaptureRequest::resolve("area", None, Some(10), 3)
                .unwrap()
                .delay,
            10
        );
        assert!(matches!(
            CaptureRequest::resolve("bogus", None, None, 3),
            Err(CaptureRequestError::UnknownMode(_))
        ));
    }

    #[test]
    fn delay_is_validated_against_the_supported_range() {
        assert!(CaptureRequest::resolve("timer", None, Some(-1), 3).is_err());
        assert!(CaptureRequest::resolve("timer", None, Some(86_401), 3).is_err());
        assert!(CaptureRequest::resolve("timer", None, Some(86_400), 3).is_ok());
    }

    #[test]
    fn geometry_is_parsed_for_the_request() {
        let request = CaptureRequest::resolve("area", Some("10,20 640x400"), None, 0).unwrap();
        assert_eq!(request.rect, Some(Rect::new(10, 20, 640, 400)));
        assert!(matches!(
            CaptureRequest::resolve("area", Some("nope"), None, 0),
            Err(CaptureRequestError::Geometry(_))
        ));
    }

    #[test]
    fn selection_step_follows_mode_and_freeze_setting() {
        let select = CaptureRequest::resolve("select", None, None, 0).unwrap();
        assert!(select.needs_selection(true, false));
        // A live select runs slurp inside the capture, not in the overlay.
        assert!(!select.needs_selection(false, true));
        let area = CaptureRequest::resolve("area", None, None, 0).unwrap();
        assert!(area.needs_selection(true, true));
        assert!(area.needs_selection(false, false));
        assert!(!area.needs_selection(false, true));
        let fixed = CaptureRequest::resolve("area", Some("0,0 100x100"), None, 0).unwrap();
        assert!(!fixed.needs_selection(true, true));
        let fullscreen = CaptureRequest::resolve("fullscreen", None, None, 0).unwrap();
        assert!(!fullscreen.needs_selection(true, true));
        // OCR with no remembered area selects like area.
        let ocr = CaptureRequest::resolve("ocr", None, None, 0).unwrap();
        assert!(ocr.needs_selection(true, false));
        assert!(ocr.needs_selection(false, false));
        assert!(!ocr.needs_selection(false, true));
    }

    #[test]
    fn initial_selection_prefers_explicit_geometry() {
        let remembered = Rect::new(5, 5, 50, 50);
        let select = CaptureRequest::resolve("select", None, None, 0).unwrap();
        assert_eq!(select.initial(Some(remembered), true), Some(remembered));
        assert_eq!(select.initial(Some(remembered), false), None);
        let fixed = CaptureRequest::resolve("area", Some("1,2 30x40"), None, 0).unwrap();
        assert_eq!(
            fixed.initial(Some(remembered), true),
            Some(Rect::new(1, 2, 30, 40))
        );
    }

    /// Scratch native directory holding the plugin files grab expects.
    struct NativeDir(std::path::PathBuf);

    static NATIVE_NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    impl NativeDir {
        fn new() -> Self {
            let nonce = NATIVE_NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("tidyshot-capture-{}-{nonce}", std::process::id(),));
            std::fs::create_dir_all(&path).unwrap();
            for name in [crate::mirror::MIRROR_PLUGIN, crate::mirror::CAPTURE_PLUGIN] {
                std::fs::write(path.join(name), b"plugin").unwrap();
            }
            Self(path)
        }
    }

    impl Drop for NativeDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn grab_builds_the_grim_arguments_in_order() {
        let runner = Fake::default();
        runner.push("PNGDATA");
        let registry = degraded_registry();
        let dir = NativeDir::new();
        let session = CaptureSession {
            runner: &runner,
            mirror: &registry,
            native: dir.0.clone(),
        };
        let bytes = session
            .grab(
                Some(Rect::new(10, 20, 640, 400)),
                true,
                Some("DP-1"),
                Some(1.5),
            )
            .unwrap();
        assert_eq!(bytes, b"PNGDATA");
        let seen = runner.seen.lock().unwrap();
        let grim = seen.iter().find(|spec| spec.program == "grim").unwrap();
        assert_eq!(
            grim.args,
            [
                "-g",
                "10,20 640x400",
                "-c",
                "-o",
                "DP-1",
                "-s",
                "1.5",
                "-t",
                "png",
                "-"
            ]
        );
        assert_eq!(grim.timeout, Some(Duration::from_secs(15)));
    }

    #[test]
    fn slurp_cancellation_and_window_rows_are_reported() {
        let runner = Fake::default();
        runner.push("");
        assert!(matches!(
            slurp_region(&runner, false, &[]),
            Err(CaptureRequestError::Selection)
        ));

        let runner = Fake::default();
        runner.push("5,6 100x200\n");
        let rect = slurp_region(&runner, true, &[Rect::new(1, 2, 30, 40)]).unwrap();
        assert_eq!(rect, Rect::new(5, 6, 100, 200));
        let seen = runner.seen.lock().unwrap();
        assert_eq!(seen[0].program, "slurp");
        assert!(seen[0].args.contains(&"-r".to_owned()));
        assert_eq!(seen[0].stdin.as_deref(), Some(b"1,2 30x40\n".as_slice()));
    }

    #[test]
    fn asleep_and_locked_desktops_are_rejected() {
        let runner = Fake::default();
        let registry = degraded_registry();
        let session = CaptureSession {
            runner: &runner,
            mirror: &registry,
            native: std::env::temp_dir(),
        };
        let locked = crate::json::parse(r#"[{"solitaryBlockedBy":["LOCK"]}]"#).unwrap();
        assert!(session.capture_ready(locked.as_array().unwrap()).is_err());
        let asleep = crate::json::parse(r#"[{"dpmsStatus":false}]"#).unwrap();
        assert!(session.capture_ready(asleep.as_array().unwrap()).is_err());
        let awake = crate::json::parse(r#"[{"dpmsStatus":true}]"#).unwrap();
        assert!(session.capture_ready(awake.as_array().unwrap()).is_ok());
    }

    #[test]
    fn countdown_finishes_once_and_reports_remaining() {
        let countdown = Countdown::start(2);
        assert_eq!(countdown.remaining_seconds(), 2);
        let mut countdown = Countdown::start(0);
        assert_eq!(countdown.remaining_seconds(), 0);
        assert_eq!(countdown.tick(), Some(CountdownOutcome::Finished));
        assert!(countdown.terminal());
        assert_eq!(countdown.tick(), None, "finished only once");
        assert!(countdown.label().starts_with("Capturing in "));
    }

    #[test]
    fn countdown_can_be_cancelled_before_it_finishes() {
        let mut countdown = Countdown::start(300);
        assert_eq!(countdown.cancel(), CountdownOutcome::Cancelled);
        assert!(countdown.terminal());
        assert_eq!(countdown.tick(), None);
    }

    #[test]
    fn countdown_sleeps_then_finishes() {
        let mut ticks = 0;
        let outcome = run_countdown(0, |_| ticks += 1, || false);
        assert_eq!(outcome, CountdownOutcome::Finished);
        assert!(ticks >= 1);
    }

    #[test]
    fn countdown_stops_when_cancelled() {
        let mut checks = 0;
        let outcome = run_countdown(
            300,
            |_| {},
            || {
                checks += 1;
                checks > 2
            },
        );
        assert_eq!(outcome, CountdownOutcome::Cancelled);
    }

    #[test]
    fn monitor_grab_uses_the_output_and_scale() {
        let runner = Fake::default();
        runner.push("left");
        runner.push("right");
        let registry = degraded_registry();
        let native = NativeDir::new();
        let session = CaptureSession {
            runner: &runner,
            mirror: &registry,
            native: native.0.clone(),
        };
        let frames = session
            .grab_monitors(&[display("left", 0, 0), display("right", 1920, 0)], false)
            .unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].1, b"left");
        let seen = runner.seen.lock().unwrap();
        let outputs: Vec<&str> = seen
            .iter()
            .filter(|spec| spec.program == "grim")
            .flat_map(|spec| spec.args.iter())
            .filter(|arg| {
                arg.starts_with("DP-") || arg.starts_with("left") || arg.starts_with("right")
            })
            .map(String::as_str)
            .collect();
        assert_eq!(outputs, ["left", "right"]);
    }

    fn monitors() -> Vec<Value> {
        crate::json::parse(
            r#"[
                {"name":"left","x":0,"y":0,"width":1920,"height":1080,"scale":1.0,"transform":0,"focused":true},
                {"name":"right","x":1920,"y":0,"width":1920,"height":1080,"scale":1.0,"transform":0,"focused":false}
            ]"#,
        )
        .unwrap()
        .as_array()
        .unwrap()
        .to_vec()
    }

    #[test]
    fn displays_are_built_from_monitor_rows() {
        let displays = displays_from_monitors(&monitors());
        assert_eq!(displays.len(), 2);
        assert_eq!(displays[0].name, "left");
        assert!(displays[0].focused);
        assert_eq!(displays[0].geometry.bounds(), Rect::new(0, 0, 1920, 1080));
        // A lone output is implicitly focused.
        let single = displays_from_monitors(&monitors()[..1]);
        assert!(single[0].focused);
    }

    #[test]
    fn needs_selection_modes_do_not_touch_grim() {
        let runner = Fake::default();
        let registry = degraded_registry();
        let native = NativeDir::new();
        let session = CaptureSession {
            runner: &runner,
            mirror: &registry,
            native: native.0.clone(),
        };
        let request = CaptureRequest::resolve("select", None, None, 0).unwrap();
        let outcome = run_capture(
            &session,
            &monitors(),
            &request,
            CaptureOptions {
                cursor: false,
                freeze: true,
            },
            |_| {},
            || false,
        )
        .unwrap();
        assert_eq!(outcome, CaptureOutcome::NeedsSelection);
        assert!(
            runner
                .seen
                .lock()
                .unwrap()
                .iter()
                .all(|spec| spec.program != "grim"),
            "the overlay owns the pointer, grim must not run"
        );
    }

    #[test]
    fn unfrozen_area_degrades_to_a_frozen_selector_without_the_plugin() {
        let request = CaptureRequest::resolve("area", None, None, 0).unwrap();
        let runner = Fake::default();
        let registry = degraded_registry();
        let native = NativeDir::new();
        let session = CaptureSession {
            runner: &runner,
            mirror: &registry,
            native: native.0.clone(),
        };
        // Without the mirror a live selector would appear in its own capture,
        // so the caller is asked to show the frozen overlay instead.
        assert_eq!(
            run_capture(
                &session,
                &monitors(),
                &request,
                CaptureOptions {
                    cursor: false,
                    freeze: false,
                },
                |_| {},
                || false,
            )
            .unwrap(),
            CaptureOutcome::NeedsSelection
        );
        assert_eq!(
            run_capture(
                &session,
                &monitors(),
                &request,
                CaptureOptions {
                    cursor: false,
                    freeze: true,
                },
                |_| {},
                || false,
            )
            .unwrap(),
            CaptureOutcome::NeedsSelection
        );
        assert!(
            runner
                .seen
                .lock()
                .unwrap()
                .iter()
                .all(|spec| spec.program != "grim"),
        );
    }

    #[test]
    fn unfrozen_area_selects_live_then_grabs_the_selected_region() {
        let request = CaptureRequest::resolve("area", None, None, 0).unwrap();
        let runner = Live::default();
        runner.push("5,6 100x200\n");
        runner.push("PNG");
        let registry = MirrorRegistry::with_arch("x86_64");
        let native = NativeDir::new();
        let session = CaptureSession {
            runner: &runner,
            mirror: &registry,
            native: native.0.clone(),
        };
        let outcome = run_capture(
            &session,
            &monitors(),
            &request,
            CaptureOptions {
                cursor: false,
                freeze: false,
            },
            |_| {},
            || false,
        )
        .unwrap();
        match outcome {
            CaptureOutcome::Grabbed(frame) => {
                assert_eq!(frame.rect, Some(Rect::new(5, 6, 100, 200)));
                assert_eq!(frame.png, b"PNG");
            }
            other => panic!("expected a live grab, got {other:?}"),
        }
        let seen = runner.seen.lock().unwrap();
        let slurp = seen.iter().find(|spec| spec.program == "slurp").unwrap();
        assert!(!slurp.args.contains(&"-r".to_owned()));
        let grim = seen.iter().find(|spec| spec.program == "grim").unwrap();
        assert!(grim.args.contains(&"-g".to_owned()));
        assert!(grim.args.contains(&"5,6 100x200".to_owned()));
    }

    #[test]
    fn live_select_runs_slurp_inside_the_capture() {
        let request = CaptureRequest::resolve("select", None, None, 0).unwrap();
        let runner = Live::default();
        runner.push("0,0 800x600\n");
        runner.push("PNG");
        let registry = MirrorRegistry::with_arch("x86_64");
        let native = NativeDir::new();
        let session = CaptureSession {
            runner: &runner,
            mirror: &registry,
            native: native.0.clone(),
        };
        match run_capture(
            &session,
            &monitors(),
            &request,
            CaptureOptions {
                cursor: false,
                freeze: false,
            },
            |_| {},
            || false,
        )
        .unwrap()
        {
            CaptureOutcome::Grabbed(frame) => {
                assert_eq!(frame.rect, Some(Rect::new(0, 0, 800, 600)));
            }
            other => panic!("expected a live select, got {other:?}"),
        }
        assert!(
            runner
                .seen
                .lock()
                .unwrap()
                .iter()
                .any(|spec| spec.program == "slurp")
        );
    }

    #[test]
    fn delay_is_waited_out_before_the_pixels_are_taken() {
        // The self timer counts down before the grab, matching timed_capture.
        let request = CaptureRequest::resolve("timer", Some("0,0 100x100"), Some(2), 0).unwrap();
        let runner = Fake::default();
        runner.push("PNG");
        let registry = degraded_registry();
        let native = NativeDir::new();
        let session = CaptureSession {
            runner: &runner,
            mirror: &registry,
            native: native.0.clone(),
        };
        let mut ticks = 0;
        let outcome = run_capture(
            &session,
            &monitors(),
            &request,
            CaptureOptions {
                cursor: false,
                freeze: true,
            },
            |label| {
                ticks += 1;
                assert!(label.starts_with("Capturing in "));
            },
            || false,
        )
        .unwrap();
        assert!(ticks >= 1, "the countdown must tick before grabbing");
        match outcome {
            CaptureOutcome::Grabbed(frame) => {
                assert_eq!(frame.rect, Some(Rect::new(0, 0, 100, 100)));
                assert_eq!(frame.png, b"PNG");
            }
            other => panic!("expected a grab, got {other:?}"),
        }
    }

    #[test]
    fn a_cancelled_countdown_takes_no_pixels() {
        let request = CaptureRequest::resolve("fullscreen", None, Some(300), 0).unwrap();
        let runner = Fake::default();
        let registry = degraded_registry();
        let native = NativeDir::new();
        let session = CaptureSession {
            runner: &runner,
            mirror: &registry,
            native: native.0.clone(),
        };
        let mut checks = 0;
        let error = run_capture(
            &session,
            &monitors(),
            &request,
            CaptureOptions {
                cursor: false,
                freeze: true,
            },
            |_| {},
            || {
                checks += 1;
                checks > 1
            },
        )
        .unwrap_err();
        assert!(matches!(error, CaptureRequestError::Interrupted), "{error}");
        assert!(
            runner
                .seen
                .lock()
                .unwrap()
                .iter()
                .all(|spec| spec.program != "grim"),
            "a cancelled countdown must not capture"
        );
    }

    #[test]
    fn fullscreen_targets_the_focused_output() {
        let request = CaptureRequest::resolve("fullscreen", None, None, 0).unwrap();
        let runner = Fake::default();
        runner.push("PNG");
        let registry = degraded_registry();
        let native = NativeDir::new();
        let session = CaptureSession {
            runner: &runner,
            mirror: &registry,
            native: native.0.clone(),
        };
        match run_capture(
            &session,
            &monitors(),
            &request,
            CaptureOptions {
                cursor: true,
                freeze: true,
            },
            |_| {},
            || false,
        )
        .unwrap()
        {
            CaptureOutcome::Grabbed(frame) => {
                assert_eq!(frame.rect, Some(Rect::new(0, 0, 1920, 1080)));
            }
            other => panic!("expected a grab, got {other:?}"),
        }
        let seen = runner.seen.lock().unwrap();
        let grim = seen.iter().find(|spec| spec.program == "grim").unwrap();
        assert!(grim.args.contains(&"-o".to_owned()));
        assert!(grim.args.contains(&"left".to_owned()));
        assert!(grim.args.contains(&"-c".to_owned()));
    }

    #[test]
    fn scrolling_modes_return_a_plan_instead_of_a_frame() {
        let runner = Fake::default();
        let registry = degraded_registry();
        let native = NativeDir::new();
        let session = CaptureSession {
            runner: &runner,
            mirror: &registry,
            native: native.0.clone(),
        };
        let request =
            CaptureRequest::resolve("scroll-horizontal", Some("10,20 640x400"), None, 0).unwrap();
        assert_eq!(
            run_capture(
                &session,
                &monitors(),
                &request,
                CaptureOptions {
                    cursor: false,
                    freeze: true,
                },
                |_| {},
                || false,
            )
            .unwrap(),
            CaptureOutcome::Scroll {
                rect: Some(Rect::new(10, 20, 640, 400)),
                horizontal: true,
            }
        );
    }

    #[test]
    fn previous_without_a_remembered_area_is_rejected() {
        let request = CaptureRequest::resolve("previous", None, None, 0).unwrap();
        let runner = Fake::default();
        let registry = degraded_registry();
        let native = NativeDir::new();
        let session = CaptureSession {
            runner: &runner,
            mirror: &registry,
            native: native.0.clone(),
        };
        let error = run_capture(
            &session,
            &monitors(),
            &request,
            CaptureOptions {
                cursor: false,
                freeze: true,
            },
            |_| {},
            || false,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("Capture an area first"),
            "{error}"
        );
    }

    #[test]
    fn desktop_grabs_every_output() {
        let request = CaptureRequest::resolve("desktop", None, None, 0).unwrap();
        let runner = Fake::default();
        runner.push("LEFT");
        runner.push("RIGHT");
        let registry = degraded_registry();
        let native = NativeDir::new();
        let session = CaptureSession {
            runner: &runner,
            mirror: &registry,
            native: native.0.clone(),
        };
        match run_capture(
            &session,
            &monitors(),
            &request,
            CaptureOptions {
                cursor: false,
                freeze: false,
            },
            |_| {},
            || false,
        )
        .unwrap()
        {
            CaptureOutcome::Frames(frames) => {
                assert_eq!(frames.len(), 2);
                assert_eq!(frames[0].png, b"LEFT");
                assert_eq!(frames[1].rect, Some(Rect::new(1920, 0, 1920, 1080)));
            }
            other => panic!("expected per-output frames, got {other:?}"),
        }
    }

    #[test]
    fn live_selection_degrades_to_a_frozen_frame_without_the_plugin() {
        let runner = Fake::default();
        let native = NativeDir::new();
        // aarch64 rejects the hook, so a live overlay is impossible.
        let registry = degraded_registry();
        let plan = plan_selection(&registry, &runner, &native.0, false);
        assert_eq!(
            plan,
            SelectionPlan {
                live: false,
                mirror: false,
            }
        );
        // Freezing never needs the mirror.
        let plan = plan_selection(&registry, &runner, &native.0, true);
        assert!(!plan.live && !plan.mirror);
    }

    #[test]
    fn live_selection_uses_the_mirror_when_it_loads() {
        // A real load succeeds on x86_64 with a valid plugin.
        struct Loading {
            loaded: std::sync::atomic::AtomicBool,
        }
        impl CommandRunner for Loading {
            fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
                use std::sync::atomic::Ordering;
                let out = |text: &str| CommandOutput {
                    status: 0,
                    stdout: text.as_bytes().to_vec(),
                    stderr: Vec::new(),
                };
                match spec.args.as_slice() {
                    [flag, query] if flag == "-j" && query == "plugin list" => {
                        if self.loaded.load(Ordering::SeqCst) {
                            Ok(out("[{\"name\":\"omnishot-clean-mirror\"}]"))
                        } else {
                            Ok(out("[]"))
                        }
                    }
                    [action, sub, ..] if action == "plugin" && sub == "load" => {
                        self.loaded.store(true, Ordering::SeqCst);
                        Ok(out("ok"))
                    }
                    _ => Ok(out("false")),
                }
            }
        }
        let native = NativeDir::new();
        let registry = MirrorRegistry::with_arch("x86_64");
        let loading = Loading {
            loaded: std::sync::atomic::AtomicBool::new(false),
        };
        let plan = plan_selection(&registry, &loading, &native.0, false);
        assert!(plan.live && plan.mirror);
    }
}
