//! Scrolling-capture session: grabbing, auto-scroll and the finish handshake.
// Ported from omnishot/widgets.py (ScrollPanel)
//!
//! One capture window owns a [`ScrollStitcher`] and a small state machine. The
//! UI calls [`ScrollSession::begin_grab`] to start a frame on a worker and
//! [`ScrollSession::complete`] when it lands. This keeps the UI thread free
//! while making the release ordering explicit: the keyboard lease, the capture
//! surfaces and the compositor mirror are all released before a completed
//! image is handed to the next step, so a nested modal loop cannot restart the
//! finished capture. A finish requested while a frame is in flight is deferred
//! until that frame lands, and no further grab starts.
//!
//! Auto-scroll advances the virtual pointer with the same `scroll-helper` the
//! source uses, speaking `zwlr_virtual_pointer_v1`; the pointer is moved with
//! `hyprctl` before every step.

use std::path::PathBuf;
use std::time::Duration;

use crate::commands::{CommandRunner, CommandSpec, hypr};
use crate::geometry::Rect;
use crate::json::Value;
use crate::stitch::{FrameData, ScrollStitcher, StitchError};

/// The default interval between automatic grabs, matching the source setting.
pub const DEFAULT_INTERVAL: Duration = Duration::from_millis(550);

/// How many consecutive unchanged frames end an automatic scroll.
pub const END_AFTER_UNCHANGED: u32 = 4;

/// The wheel steps injected per auto-scroll tick.
pub const AUTO_SCROLL_STEP: i32 = 3;

/// A pointer and scroll backend for auto-scroll.
pub trait ScrollInput {
    /// The absolute pointer position.
    fn cursor(&self) -> Result<(i32, i32), String>;
    /// Move the pointer to an absolute position.
    fn move_cursor(&self, x: i32, y: i32) -> Result<(), String>;
    /// Inject one wheel step along the chosen axis.
    fn scroll_step(&self, horizontal: bool, amount: i32) -> Result<(), String>;
}

/// Source of frames and the compositor mirror lease.
pub trait ScrollCapture {
    /// Grab one frame of `rect` in RGB channel order.
    fn grab(&self, rect: Rect) -> Result<FrameData, String>;
    /// Start the cursor-excluding compositor mirror.
    fn mirror_start(&self) -> Result<(), String>;
    /// Release the cursor-excluding compositor mirror.
    fn mirror_stop(&self) -> Result<(), String>;
}

/// The capture surfaces and keyboard lease the session must release.
///
/// Production supplies an adapter over the QML windows; tests supply a
/// recorder so the release order is observable.
pub trait ScrollUi {
    /// Hide the capture surfaces; returns whether the grab should wait.
    fn before_grab(&mut self) -> bool;
    /// Restore the capture surfaces after a grab.
    fn after_grab(&mut self);
    /// Show a growing preview of the stitched image.
    fn update(&mut self, frame: &FrameData);
    /// Stop the keyboard shortcut lease.
    fn keys_stop(&mut self);
    /// Close the guide and selection surfaces.
    fn visuals_close(&mut self);
}

/// A no-op UI, for headless runs and tests that do not assert surface state.
#[derive(Debug, Default)]
pub struct NoUi;

impl ScrollUi for NoUi {
    fn before_grab(&mut self) -> bool {
        false
    }
    fn after_grab(&mut self) {}
    fn update(&mut self, _frame: &FrameData) {}
    fn keys_stop(&mut self) {}
    fn visuals_close(&mut self) {}
}

/// One pending grab, tagged with the session generation that requested it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollJob {
    pub generation: u64,
    pub rect: Rect,
}

/// A running scrolling capture.
pub struct ScrollSession {
    pub rect: Rect,
    pub horizontal: bool,
    pub interval: Duration,
    pub auto: bool,
    pub running: bool,
    pub busy: bool,
    pub closed: bool,
    pub finished: bool,
    pub cancelled: bool,
    pub reached_end: bool,
    pub no_change: u32,
    pub scrolled_last: bool,
    pub status: String,
    pub start_label: String,
    pub generation: u64,
    stitcher: ScrollStitcher,
    finish_pending: bool,
    capture: Box<dyn ScrollCapture>,
    input: Box<dyn ScrollInput>,
    ui: Box<dyn ScrollUi>,
    /// Invoked with the stitched image once the controls are released.
    pub on_completed: Option<Box<dyn FnMut(FrameData)>>,
    /// Invoked once when a session is closed without finishing.
    pub on_cancelled: Option<Box<dyn FnMut()>>,
}

impl ScrollSession {
    #[must_use]
    pub fn new(
        rect: Rect,
        horizontal: bool,
        capture: Box<dyn ScrollCapture>,
        input: Box<dyn ScrollInput>,
        ui: Box<dyn ScrollUi>,
    ) -> Self {
        let stitcher = ScrollStitcher::new(horizontal);
        Self {
            rect,
            horizontal,
            interval: DEFAULT_INTERVAL,
            auto: false,
            running: false,
            busy: false,
            closed: false,
            finished: false,
            cancelled: false,
            reached_end: false,
            no_change: 0,
            scrolled_last: false,
            status: format!(
                "{} × {} · Adjust the area, then start capture.",
                rect.width, rect.height
            ),
            start_label: "Start Capture".to_owned(),
            generation: 0,
            stitcher,
            finish_pending: false,
            capture,
            input,
            ui,
            on_completed: None,
            on_cancelled: None,
        }
    }

    /// Build a session with a tighter stitcher budget, for tests.
    #[must_use]
    pub fn with_limit(
        rect: Rect,
        horizontal: bool,
        max_pixels: usize,
        capture: Box<dyn ScrollCapture>,
        input: Box<dyn ScrollInput>,
        ui: Box<dyn ScrollUi>,
    ) -> Self {
        let mut session = Self::new(rect, horizontal, capture, input, ui);
        session.stitcher = ScrollStitcher::with_limit(horizontal, max_pixels);
        session
    }

    #[must_use]
    pub fn stitcher(&self) -> &ScrollStitcher {
        &self.stitcher
    }

    /// The stitched image so far.
    #[must_use]
    pub fn output(&self) -> Option<FrameData> {
        self.stitcher.output()
    }

    /// True while a grab is in flight.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// True while the capture accepts new frames.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Start the capture, leasing the mirror first.
    ///
    /// A failed mirror start leaves the session stopped with the retry label,
    /// exactly as the source keeps the selector on screen.
    // Ported from omnishot/widgets.py (ScrollPanel.start)
    pub fn start(&mut self) {
        if self.running || self.closed {
            return;
        }
        if let Err(error) = self.capture.mirror_start() {
            self.status = error;
            self.start_label = "Retry Capture".to_owned();
            return;
        }
        self.running = true;
        self.start_label = "Start Capture".to_owned();
        self.status = "Scroll slowly inside the selection. Click Done when finished.".to_owned();
        if self.auto {
            self.set_auto(true);
        }
    }

    /// Toggle auto-scroll.
    pub fn toggle_auto(&mut self) {
        self.set_auto(!self.auto);
    }

    /// Turn auto-scroll on or off, clearing the unchanged-frame counters.
    // Ported from omnishot/widgets.py (ScrollPanel.set_auto)
    pub fn set_auto(&mut self, enabled: bool) {
        if self.closed {
            return;
        }
        self.auto = enabled;
        self.no_change = 0;
        self.scrolled_last = false;
        self.reached_end = false;
    }

    /// Begin one grab if the session is due for it.
    ///
    /// Sets the busy flag and hides the capture surfaces. The caller runs
    /// [`ScrollCapture::grab`] on a worker and hands the result back through
    /// [`ScrollSession::complete`]. A finished or paused session returns
    /// `None`, which is what makes `finish` race-free.
    // Ported from omnishot/widgets.py (ScrollPanel.tick)
    pub fn begin_grab(&mut self) -> Option<ScrollJob> {
        if !self.running || self.busy || self.closed {
            return None;
        }
        self.busy = true;
        self.ui.before_grab();
        Some(ScrollJob {
            generation: self.generation,
            rect: self.rect,
        })
    }

    /// Fold a completed grab into the stitch and drive auto-scroll.
    ///
    /// Returns the finished image when this completion also satisfied a
    /// pending [`ScrollSession::finish`], or when the frame could not be
    /// stitched and a finish was already requested.
    // Ported from omnishot/widgets.py (ScrollPanel.tick done/fail)
    pub fn complete(
        &mut self,
        job: ScrollJob,
        result: Result<FrameData, String>,
    ) -> Option<FrameData> {
        self.busy = false;
        if job.generation != self.generation {
            return None;
        }
        self.ui.after_grab();
        match result {
            Ok(frame) => match self.stitcher.push(frame.view()) {
                Ok((changed, message)) => {
                    if changed {
                        self.reached_end = false;
                    }
                    self.status = if self.reached_end {
                        "Reached the end. Click Done to finish.".to_owned()
                    } else {
                        message.clone()
                    };
                    if let Some(output) = self.stitcher.output() {
                        self.ui.update(&output);
                    }
                    if self.auto {
                        if message.starts_with("Could not align") {
                            self.auto = false;
                        } else if self.scrolled_last {
                            self.no_change = if changed { 0 } else { self.no_change + 1 };
                            self.scrolled_last = false;
                            if self.no_change >= END_AFTER_UNCHANGED {
                                self.auto = false;
                                self.reached_end = true;
                                self.status = "Reached the end. Click Done to finish.".to_owned();
                            }
                        } else {
                            self.scrolled_last = false;
                        }
                    }
                }
                Err(error) => {
                    // An over-limit or mismatched frame stops the session with
                    // the reason on screen; nothing already stitched is lost.
                    self.status = error.to_string();
                    self.running = false;
                    self.auto = false;
                    self.release_mirror();
                    self.start_label = "Retry Capture".to_owned();
                }
            },
            Err(message) => {
                // A failed grab stops the capture and releases the mirror, but
                // keeps the frames already stitched so Done still works.
                self.status = message;
                self.running = false;
                self.auto = false;
                self.release_mirror();
                self.start_label = "Retry Capture".to_owned();
            }
        }
        if self.finish_pending {
            return Some(self.finalize());
        }
        if self.auto {
            self.auto_advance();
        }
        None
    }

    /// Advance the auto-scroll after a completed frame.
    ///
    /// The pointer must be inside the capture or the scroll pauses with a
    /// message, matching the source's guard against scrolling another window.
    pub fn auto_advance(&mut self) {
        if self.closed || !self.auto || !self.running {
            return;
        }
        let (x, y, w, h) = (self.rect.x, self.rect.y, self.rect.width, self.rect.height);
        match self.input.cursor() {
            Ok((px, py)) => {
                if !(x <= px && px < x + w && y <= py && py < y + h) {
                    self.status =
                        "Move the pointer inside the selected area to continue.".to_owned();
                    return;
                }
            }
            Err(error) => {
                self.status = error;
                self.auto = false;
                return;
            }
        }
        let (cx, cy) = (x + w / 2, y + h / 2);
        if let Err(error) = self.input.move_cursor(cx, cy) {
            self.status = error;
            self.auto = false;
            return;
        }
        match self.input.scroll_step(self.horizontal, AUTO_SCROLL_STEP) {
            Ok(()) => self.scrolled_last = true,
            Err(error) => {
                self.status = error;
                self.auto = false;
            }
        }
    }

    /// Stop accepting frames and release the capture controls.
    ///
    /// While a grab is in flight the finish is deferred, and completes when
    /// that frame lands. Returns the stitched image when the finish completed
    /// synchronously.
    // Ported from omnishot/widgets.py (ScrollPanel.finish)
    pub fn finish(&mut self) -> Option<FrameData> {
        if self.closed || self.finished {
            return None;
        }
        self.running = false;
        if self.busy {
            self.finish_pending = true;
            return None;
        }
        Some(self.finalize())
    }

    /// Release every lease, emit the image, and mark the session finished.
    fn finalize(&mut self) -> FrameData {
        self.finish_pending = false;
        self.finished = true;
        let frame = self.stitcher.output().unwrap_or(FrameData {
            data: Vec::new(),
            width: 0,
            height: 0,
            channels: 3,
        });
        self.close_surfaces();
        if let Some(mut completed) = self.on_completed.take() {
            completed(frame.clone());
        }
        frame
    }

    /// Release the keyboard lease, capture surfaces and mirror, in that order.
    fn close_surfaces(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.running = false;
        self.generation += 1;
        self.ui.keys_stop();
        self.ui.visuals_close();
        self.release_mirror();
    }

    fn release_mirror(&mut self) {
        if let Err(error) = self.capture.mirror_stop() {
            // A mirror that will not release must not block the handoff.
            eprintln!("Could not release scrolling capture: {error}");
        }
    }

    /// Close the session, releasing every lease and marking a cancel.
    // Ported from omnishot/widgets.py (ScrollPanel.closeEvent)
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.close_surfaces();
        if !self.finished {
            self.cancelled = true;
            if let Some(mut cancelled) = self.on_cancelled.take() {
                cancelled();
            }
        }
    }

    /// Handle the accept/cancel keyboard actions the panel forwards.
    ///
    /// An accept while running finishes; otherwise it starts. A cancel closes.
    /// A nested modal loop that re-enters here after a completion sees
    /// `closed` and does nothing, which cannot restart the capture.
    // Ported from omnishot/widgets.py (ScrollPanel.keyboard_action)
    pub fn keyboard_action(&mut self, action: &str) -> Option<FrameData> {
        if self.closed {
            return None;
        }
        match action {
            "cancel" => {
                self.close();
                None
            }
            "accept" => {
                if self.running {
                    self.finish()
                } else {
                    self.start();
                    None
                }
            }
            _ => None,
        }
    }

    /// The status message the UI shows.
    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }
}

/// Production auto-scroll through `hyprctl` and the wlr virtual-pointer helper.
#[derive(Debug)]
pub struct SystemScrollInput<'a, R: CommandRunner> {
    pub runner: &'a R,
    pub native: PathBuf,
}

impl<R: CommandRunner> ScrollInput for SystemScrollInput<'_, R> {
    fn cursor(&self) -> Result<(i32, i32), String> {
        let value = hypr(self.runner, "cursorpos").map_err(|error| error.to_string())?;
        let x = value.get("x").and_then(Value::as_f64).unwrap_or(0.0) as i32;
        let y = value.get("y").and_then(Value::as_f64).unwrap_or(0.0) as i32;
        Ok((x, y))
    }

    fn move_cursor(&self, x: i32, y: i32) -> Result<(), String> {
        let spec = CommandSpec::new("hyprctl").args([
            "eval",
            &format!("hl.dispatch(hl.dsp.cursor.move({{ x = {x}, y = {y} }}))"),
        ]);
        self.runner
            .run(&spec)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn scroll_step(&self, horizontal: bool, amount: i32) -> Result<(), String> {
        let helper = self.native.join("scroll-helper");
        if !helper.is_file() {
            return Err("Scrolling helper is missing. Run install.sh.".to_owned());
        }
        let axis = if horizontal { "horizontal" } else { "vertical" };
        let spec = CommandSpec::new(helper.to_string_lossy().into_owned())
            .args([axis, &amount.to_string()]);
        self.runner
            .run(&spec)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// Production frame source: `grim` in binary PPM, which needs no decode crate.
#[derive(Debug)]
pub struct SystemScrollCapture<'a, R: CommandRunner> {
    pub runner: &'a R,
    mirror_enabled: std::cell::Cell<bool>,
}

impl<'a, R: CommandRunner> SystemScrollCapture<'a, R> {
    #[must_use]
    pub fn new(runner: &'a R) -> Self {
        Self {
            runner,
            mirror_enabled: std::cell::Cell::new(false),
        }
    }

    /// Parse a binary PPM (`P6`) frame into RGB.
    fn parse_ppm(bytes: &[u8]) -> Result<FrameData, String> {
        if bytes.len() < 2 || &bytes[..2] != b"P6" {
            return Err("grim did not return a PPM image".to_owned());
        }
        let mut index = 2usize;
        let mut fields = Vec::new();
        while fields.len() < 3 {
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if index < bytes.len() && bytes[index] == b'#' {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
                continue;
            }
            let start = index;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
            let text = std::str::from_utf8(&bytes[start..index])
                .map_err(|_| "malformed PPM header".to_owned())?;
            fields.push(
                text.parse::<usize>()
                    .map_err(|_| "malformed PPM header".to_owned())?,
            );
        }
        let (width, height, maxval) = (fields[0], fields[1], fields[2]);
        if maxval != 255 {
            return Err("grim returned an unsupported PPM depth".to_owned());
        }
        index += 1; // single whitespace byte before the pixel data
        let pixels = &bytes[index..];
        if pixels.len() < width * height * 3 {
            return Err("grim returned a truncated PPM image".to_owned());
        }
        Ok(FrameData::new(
            pixels[..width * height * 3].to_vec(),
            width,
            height,
            3,
        ))
    }
}

impl<R: CommandRunner> ScrollCapture for SystemScrollCapture<'_, R> {
    fn grab(&self, rect: Rect) -> Result<FrameData, String> {
        let spec = CommandSpec::new("grim")
            .args([
                "-g",
                &crate::geometry::geometry_text(rect),
                "-t",
                "ppm",
                "-",
            ])
            .timeout(Duration::from_secs(15));
        let output = self.runner.run(&spec).map_err(|error| error.to_string())?;
        Self::parse_ppm(&output.stdout)
    }

    fn mirror_start(&self) -> Result<(), String> {
        self.mirror_enabled.set(true);
        Ok(())
    }

    fn mirror_stop(&self) -> Result<(), String> {
        self.mirror_enabled.set(false);
        Ok(())
    }
}

/// Render a stitch failure for the status line.
#[must_use]
pub fn stitch_message(error: &StitchError) -> String {
    error.to_string()
}
