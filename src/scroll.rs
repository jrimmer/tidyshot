//! Scrolling-capture session: grabbing, auto-scroll and the finish handshake.
// Ported from omnishot/widgets.py (ScrollPanel)
//!
//! One capture window owns a [`ScrollStitcher`] and a small state machine. The
//! UI calls [`ScrollSession::begin_grab`] to start a frame and
//! [`ScrollSession::run_job`] on the worker to grab the pixels, match and grow
//! the stitch. Only the small result is handed back through
//! [`ScrollSession::complete`], so the O(width) `grim` call and the O(image)
//! match never run on the owning thread. This keeps the UI thread free while
//! making the release ordering explicit: the keyboard lease, the capture
//! surfaces and the compositor mirror are all released before a completed
//! image is handed to the next step, so a nested modal loop cannot restart the
//! finished capture. A finish requested while a frame is in flight is deferred
//! until that frame lands, and no further grab starts.
//!
//! Matching borrows the previous frame and the incoming frame as slices; the
//! current stitch is moved into the worker and back, never copied, so the only
//! allocation per frame is the incoming pixels and the grown image.
//!
//! Auto-scroll advances the virtual pointer with the same `scroll-helper` the
//! source uses, speaking `zwlr_virtual_pointer_v1`. The pointer is moved to the
//! capture centre once when auto-scroll turns on; each frame only checks the
//! pointer is still inside and injects the wheel step.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::commands::{CommandRunner, CommandSpec, hypr};
use crate::geometry::Rect;
use crate::json::Value;
use crate::mirror::{MirrorLease, MirrorRegistry};
use crate::stitch::{FrameData, FrameRef, ScrollStitcher};

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
///
/// `Send + Sync` lets the session lend the capture to the worker thread that
/// runs the grab and the matching, so the owning (UI) thread never blocks.
pub trait ScrollCapture: Send + Sync {
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
/// recorder so the release order is observable. It stays on the owning
/// thread, so it needs no `Send` bound.
pub trait ScrollUi {
    /// Hide the capture surfaces; returns whether the grab should wait.
    fn before_grab(&mut self) -> bool;
    /// Restore the capture surfaces after a grab.
    fn after_grab(&mut self);
    /// Show a growing preview of the stitched image.
    fn update(&mut self, frame: FrameRef<'_>);
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
    fn update(&mut self, _frame: FrameRef<'_>) {}
    fn keys_stop(&mut self) {}
    fn visuals_close(&mut self) {}
}

/// One pending grab, tagged with the session generation that requested it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollJob {
    pub generation: u64,
    pub rect: Rect,
    /// Whether the capture surfaces were hidden, so the worker waits for the
    /// compositor to present before grabbing.
    pub wait: bool,
}

/// The `Send + Sync` half of a session: grabbing and stitching move to a worker.
///
/// The UI clones this handle, moves it to a `std::thread` and hands the result
/// back through [`ScrollSession::complete`]. It deliberately excludes the UI
/// adapter and the auto-scroll input, which stay on the owning thread; only the
/// frame source (already `Send + Sync`) and the shared stitch cross threads.
#[derive(Clone)]
pub struct ScrollWorker {
    capture: Arc<dyn ScrollCapture>,
    stitcher: Arc<Mutex<ScrollStitcher>>,
}

impl ScrollWorker {
    /// Grab one frame and fold it into the stitch.
    ///
    /// This is the reference's `work()`: the hide delay, the `grim` grab and
    /// the O(width×height) match all run on the caller's worker thread, so a
    /// large capture never blocks the thread that owns the session.
    // Ported from omnishot/widgets.py (ScrollPanel.tick work)
    pub fn run(&self, job: ScrollJob) -> Result<(bool, String), String> {
        if job.wait {
            std::thread::sleep(Duration::from_millis(150));
        }
        let frame = self.capture.grab(job.rect)?;
        self.stitcher
            .lock()
            .expect("stitcher poisoned")
            .push(frame)
            .map_err(|error| error.to_string())
    }
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
    stitcher: Arc<Mutex<ScrollStitcher>>,
    finish_pending: bool,
    capture: Arc<dyn ScrollCapture>,
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
        capture: Arc<dyn ScrollCapture>,
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
            stitcher: Arc::new(Mutex::new(stitcher)),
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
        capture: Arc<dyn ScrollCapture>,
        input: Box<dyn ScrollInput>,
        ui: Box<dyn ScrollUi>,
    ) -> Self {
        let mut session = Self::new(rect, horizontal, capture, input, ui);
        session.stitcher = Arc::new(Mutex::new(ScrollStitcher::with_limit(
            horizontal, max_pixels,
        )));
        session
    }

    /// A cloneable handle for grabbing and stitching on a worker thread.
    #[must_use]
    pub fn worker(&self) -> ScrollWorker {
        ScrollWorker {
            capture: Arc::clone(&self.capture),
            stitcher: Arc::clone(&self.stitcher),
        }
    }

    /// Run one job on the calling thread, grabbing and stitching in place.
    ///
    /// A convenience for synchronous callers; production moves a
    /// [`ScrollWorker`] clone to a thread instead.
    pub fn run_job(&self, job: ScrollJob) -> Result<(bool, String), String> {
        self.worker().run(job)
    }

    /// Lock the shared stitch, for status inspection and tests.
    pub fn stitcher(&self) -> std::sync::MutexGuard<'_, ScrollStitcher> {
        self.stitcher.lock().expect("stitcher poisoned")
    }

    /// The stitched image so far.
    #[must_use]
    pub fn output(&self) -> Option<FrameData> {
        self.stitcher().output()
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
    ///
    /// Enabling auto-scroll while running moves the pointer to the centre of
    /// the capture once, matching the source; the per-frame advance only
    /// checks that the pointer is still inside and injects the wheel step.
    // Ported from omnishot/widgets.py (ScrollPanel.set_auto)
    pub fn set_auto(&mut self, enabled: bool) {
        if self.closed {
            return;
        }
        self.auto = enabled;
        self.no_change = 0;
        self.scrolled_last = false;
        self.reached_end = false;
        if self.auto && self.running {
            let (x, y, w, h) = (self.rect.x, self.rect.y, self.rect.width, self.rect.height);
            if let Err(error) = self.input.move_cursor(x + w / 2, y + h / 2) {
                self.status = error;
                self.auto = false;
            }
        }
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
        let hidden = self.ui.before_grab();
        Some(ScrollJob {
            generation: self.generation,
            rect: self.rect,
            wait: hidden,
        })
    }

    /// Fold a completed worker grab into the session and drive auto-scroll.
    ///
    /// `result` is the [`ScrollWorker::run`] outcome: the worker already
    /// grabbed the pixels and grew the shared stitch, so this only updates the
    /// status, preview and auto-scroll state on the owning thread. Returns the
    /// finished image when this completion also satisfied a pending
    /// [`ScrollSession::finish`].
    // Ported from omnishot/widgets.py (ScrollPanel.tick done/fail)
    pub fn complete(
        &mut self,
        job: ScrollJob,
        result: Result<(bool, String), String>,
    ) -> Option<FrameData> {
        self.busy = false;
        if job.generation != self.generation {
            return None;
        }
        self.ui.after_grab();
        match result {
            Ok((changed, message)) => {
                if changed {
                    self.reached_end = false;
                }
                self.status = if self.reached_end {
                    "Reached the end. Click Done to finish.".to_owned()
                } else {
                    message.clone()
                };
                // The preview renders straight from the stored buffer for a
                // vertical capture; only a horizontal one is transposed.
                if self.horizontal {
                    if let Some(output) = self.output() {
                        self.ui.update(output.view());
                    }
                } else {
                    let stitcher = Arc::clone(&self.stitcher);
                    let guard = stitcher.lock().expect("stitcher poisoned");
                    if let Some(image) = guard.image_view() {
                        self.ui.update(image);
                    }
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
            Err(message) => {
                // A failed grab or an over-limit frame stops the capture and
                // releases the mirror, but keeps the frames already stitched
                // so Done still works.
                self.status = message;
                self.running = false;
                self.auto = false;
                self.release_mirror();
                self.start_label = "Retry Capture".to_owned();
            }
        }
        if self.finish_pending {
            return self.finalize();
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
        self.finalize()
    }

    /// Release every lease, emit the image when one exists, and mark the
    /// session finished.
    fn finalize(&mut self) -> Option<FrameData> {
        self.finish_pending = false;
        self.finished = true;
        let frame = self.stitcher().output();
        self.close_surfaces();
        if let Some(frame) = frame
            && let Some(mut completed) = self.on_completed.take()
        {
            completed(frame.clone());
            return Some(frame);
        }
        None
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

/// Locate the directory holding the built native helpers.
///
/// The helpers live beside the executable in an installed build and in the
/// source tree during development. Returning both candidates lets the caller
/// report a missing helper with an actionable message.
#[must_use]
pub fn native_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("native"));
        candidates.push(dir.to_path_buf());
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("native"));
    candidates
}

/// The first candidate directory that holds a built `scroll-helper`.
#[must_use]
pub fn native_dir() -> Option<PathBuf> {
    native_candidates()
        .into_iter()
        .find(|dir| dir.join("scroll-helper").is_file())
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
            return Err("Scrolling helper is missing. Run bin/native to build it.".to_owned());
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
///
/// The session holds the clean-mirror lease across the whole capture, so the
/// cursor stays out of every frame; the lease opens the same
/// `cursor_capture` role the source's `CursorMirror` uses.
#[derive(Debug)]
pub struct SystemScrollCapture<'a, R: CommandRunner> {
    pub runner: &'a R,
    pub mirror: &'a MirrorRegistry,
    pub native: PathBuf,
    lease: Mutex<MirrorLease<'a>>,
}

impl<'a, R: CommandRunner> SystemScrollCapture<'a, R> {
    #[must_use]
    pub fn new(runner: &'a R, mirror: &'a MirrorRegistry, native: impl Into<PathBuf>) -> Self {
        Self {
            runner,
            mirror,
            native: native.into(),
            lease: Mutex::new(mirror.lease("cursor_capture")),
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
        let mut lease = self.lease.lock().expect("mirror lease poisoned");
        lease
            .start(self.runner, &self.native)
            .map_err(|error| error.to_string())
    }

    fn mirror_stop(&self) -> Result<(), String> {
        let mut lease = self.lease.lock().expect("mirror lease poisoned");
        lease.stop(self.runner).map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Shared event log so the test can assert the release ordering.
    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<String>>>);

    impl Log {
        fn push(&self, event: &str) {
            self.0.lock().unwrap().push(event.to_owned());
        }

        fn events(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    /// A capture backend that records lifecycle events.
    struct FakeCapture {
        log: Log,
        reject_first: bool,
        started: std::sync::atomic::AtomicBool,
        enabled: std::sync::atomic::AtomicBool,
    }

    impl FakeCapture {
        fn new(log: Log, reject_first: bool) -> Self {
            Self {
                log,
                reject_first,
                started: std::sync::atomic::AtomicBool::new(false),
                enabled: std::sync::atomic::AtomicBool::new(false),
            }
        }
    }

    impl ScrollCapture for FakeCapture {
        fn grab(&self, _rect: Rect) -> Result<FrameData, String> {
            Err("unused".to_owned())
        }

        fn mirror_start(&self) -> Result<(), String> {
            use std::sync::atomic::Ordering;
            if self.reject_first && !self.started.swap(true, Ordering::SeqCst) {
                self.log.push("rejected");
                return Err("Capture components need rebuilding".to_owned());
            }
            self.enabled.store(true, Ordering::SeqCst);
            self.log.push("start");
            Ok(())
        }

        fn mirror_stop(&self) -> Result<(), String> {
            use std::sync::atomic::Ordering;
            if self.enabled.swap(false, Ordering::SeqCst) {
                self.log.push("stop");
            }
            Ok(())
        }
    }

    /// A pointer backend that is fixed inside the capture.
    struct FakeInput {
        position: std::cell::Cell<(i32, i32)>,
        log: Log,
    }

    impl ScrollInput for FakeInput {
        fn cursor(&self) -> Result<(i32, i32), String> {
            Ok(self.position.get())
        }

        fn move_cursor(&self, x: i32, y: i32) -> Result<(), String> {
            self.position.set((x, y));
            Ok(())
        }

        fn scroll_step(&self, horizontal: bool, amount: i32) -> Result<(), String> {
            self.log.push(&format!(
                "scroll {} {}",
                if horizontal { "h" } else { "v" },
                amount
            ));
            Ok(())
        }
    }

    /// A UI recorder that asserts the release ordering.
    struct FakeUi {
        log: Log,
        previews: usize,
    }

    impl ScrollUi for FakeUi {
        fn before_grab(&mut self) -> bool {
            false
        }
        fn after_grab(&mut self) {}
        fn update(&mut self, _frame: FrameRef<'_>) {
            self.previews += 1;
        }
        fn keys_stop(&mut self) {
            self.log.push("keys stopped");
        }
        fn visuals_close(&mut self) {
            self.log.push("visuals closed");
        }
    }

    fn frame(width: usize, height: usize, value: u8) -> FrameData {
        FrameData::new(vec![value; width * height * 3], width, height, 3)
    }

    fn session(
        rect: Rect,
        reject_first: bool,
    ) -> (ScrollSession, Log, Arc<Mutex<usize>>, Arc<Mutex<usize>>) {
        let log = Log::default();
        let completed = Arc::new(Mutex::new(0usize));
        let cancelled = Arc::new(Mutex::new(0usize));
        let capture = Arc::new(FakeCapture::new(log.clone(), reject_first));
        let input = Box::new(FakeInput {
            position: std::cell::Cell::new((10, 10)),
            log: log.clone(),
        });
        let ui = Box::new(FakeUi {
            log: log.clone(),
            previews: 0,
        });
        let mut session = ScrollSession::new(rect, false, capture, input, ui);
        {
            let completed = completed.clone();
            session.on_completed = Some(Box::new(move |_| *completed.lock().unwrap() += 1));
        }
        {
            let cancelled = cancelled.clone();
            session.on_cancelled = Some(Box::new(move || *cancelled.lock().unwrap() += 1));
        }
        (session, log, completed, cancelled)
    }

    #[test]
    fn auto_scroll_moves_the_pointer_once_when_it_starts() {
        let (mut session, _log, _completed, _cancelled) =
            session(Rect::new(10, 10, 200, 100), false);
        session.running = true;
        // Enabling auto-scroll centres the pointer once.
        session.set_auto(true);
        // auto_advance must not recentre; it only checks the pointer is
        // inside and injects the wheel step, matching the reference.
        session.auto_advance();
        assert!(session.auto);
        assert!(session.scrolled_last);
    }

    #[test]
    fn auto_scroll_pauses_when_the_pointer_leaves_the_capture() {
        let (mut session, log, _completed, _cancelled) =
            session(Rect::new(10, 10, 200, 100), false);
        session.running = true;
        session.auto = true;
        // Move the fake pointer outside the selection through the input.
        session.input.move_cursor(0, 0).unwrap();
        session.auto_advance();
        assert!(session.auto, "a pointer outside pauses without disabling");
        assert!(session.status().contains("Move the pointer inside"));
        assert!(!log.events().iter().any(|event| event.starts_with("scroll")));
    }

    #[test]
    fn finish_waits_for_the_current_frame_without_starting_another() {
        let (mut session, _log, completed, _cancelled) =
            session(Rect::new(10, 10, 200, 100), false);
        session.running = true;
        let job = session.begin_grab().expect("first grab");
        assert!(session.is_busy());

        // A finish while the frame is in flight stops new grabs but waits.
        assert!(session.finish().is_none());
        assert!(!session.is_running());
        assert!(session.begin_grab().is_none(), "no second grab while busy");

        session.stitcher().push(frame(200, 100, 125)).unwrap();
        let done = session.complete(job, Ok((true, "Ready".to_owned())));
        assert!(done.is_some());
        assert!(session.closed);
        assert_eq!(*completed.lock().unwrap(), 1);
    }

    #[test]
    fn completion_releases_controls_before_the_image_is_delivered() {
        let (mut session, log, completed, cancelled) = session(Rect::new(10, 10, 200, 100), false);
        session.start();
        session.stitcher().push(frame(200, 100, 125)).unwrap();
        let job = session.begin_grab().expect("grab");
        assert!(session.finish().is_none());
        let done = session
            .complete(job, Ok((true, "Ready".to_owned())))
            .unwrap();
        // The image is the stitched one, and release happened before delivery.
        assert_eq!(done.data, frame(200, 100, 125).data);
        assert_eq!(
            log.events(),
            ["start", "keys stopped", "visuals closed", "stop"]
        );
        assert!(!session.is_running());
        assert_eq!(*cancelled.lock().unwrap(), 0);
        assert_eq!(*completed.lock().unwrap(), 1);
        // A nested modal loop re-enters accept after completion; it must not
        // restart the finished capture.
        assert!(session.keyboard_action("accept").is_none());
        assert!(session.closed);
        // No mirror restart after completion.
        assert_eq!(log.events().iter().filter(|e| *e == "start").count(), 1);
    }

    #[test]
    fn end_message_survives_unchanged_frames() {
        let (mut session, _log, _completed, _cancelled) =
            session(Rect::new(10, 10, 200, 100), false);
        session.running = true;
        session.set_auto(true);
        session.stitcher().push(frame(200, 100, 125)).unwrap();
        session.scrolled_last = true;
        session.no_change = 3;
        let job = session.begin_grab().expect("grab");
        session.complete(job, Ok((false, "No new content.".to_owned())));
        assert!(session.reached_end);
        assert!(!session.auto);
        assert!(session.status().contains("Reached the end"));

        // A later unchanged frame keeps the end message rather than replacing
        // it with "No new content".
        session.scrolled_last = false;
        let job = session.begin_grab().expect("grab");
        session.complete(job, Ok((false, "No new content.".to_owned())));
        assert!(session.status().contains("Reached the end"));
    }

    #[test]
    fn mirror_failure_retry_and_cancel_during_frame() {
        let (mut session, log, _completed, _cancelled) = session(Rect::new(10, 10, 200, 100), true);
        session.start();
        assert!(!session.is_running());
        assert!(session.start_label.contains("Retry"));
        assert!(session.status.contains("rebuilding"));

        session.start();
        assert!(session.is_running());
        assert!(log.events().contains(&"start".to_owned()));
        let job = session.begin_grab().expect("grab");
        session.complete(job, Err("Display disconnected".to_owned()));
        assert!(!session.is_running());
        assert!(session.status.contains("Display disconnected"));

        session.start();
        assert!(session.begin_grab().is_some());
        session.close();
        assert!(session.cancelled);
        let events = log.events();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.as_str() == "rejected"
                    || e.as_str() == "start"
                    || e.as_str() == "stop")
                .cloned()
                .collect::<Vec<_>>(),
            ["rejected", "start", "stop", "start", "stop"]
        );
        // Completing the in-flight frame after cancel is ignored.
        let stale = ScrollJob {
            generation: 0,
            rect: session.rect,
            wait: false,
        };
        assert!(
            session
                .complete(stale, Ok((true, "Ready".to_owned())))
                .is_none()
        );
    }

    #[test]
    fn alignment_failure_keeps_the_stitched_image_and_pauses_auto() {
        let (mut session, _log, _completed, _cancelled) =
            session(Rect::new(10, 10, 200, 100), false);
        session.running = true;
        session.set_auto(true);
        session.scrolled_last = true;
        let source: Vec<u8> = (0..200 * 100 * 3)
            .map(|index| ((index * 31) % 200) as u8)
            .collect();
        session
            .stitcher()
            .push(FrameData::new(source.clone(), 200, 100, 3))
            .unwrap();
        let job = session.begin_grab().expect("grab");
        let noise: Vec<u8> = (0..200 * 100 * 3)
            .map(|index| ((index * 97 + 13) % 255) as u8)
            .collect();
        // The worker pushes the frame; the unalignable result reaches the
        // session through complete exactly as the real worker would.
        let result = session
            .stitcher()
            .push(FrameData::new(noise, 200, 100, 3))
            .map_err(|error| error.to_string());
        let done = session.complete(job, result);
        assert!(done.is_none());
        assert!(!session.auto, "auto pauses on an unalignable frame");
        assert!(session.output().is_some(), "stitched image is preserved");
    }

    /// A `ScrollWorker` clone must be `Send`, so the grab and the stitch can
    /// run on another thread while the UI keeps the session.
    #[test]
    fn a_worker_clone_stitches_from_another_thread() {
        struct FrameCapture;
        impl ScrollCapture for FrameCapture {
            fn grab(&self, _rect: Rect) -> Result<FrameData, String> {
                Ok(frame(200, 100, 125))
            }
            fn mirror_start(&self) -> Result<(), String> {
                Ok(())
            }
            fn mirror_stop(&self) -> Result<(), String> {
                Ok(())
            }
        }

        let mut session = ScrollSession::new(
            Rect::new(10, 10, 200, 100),
            false,
            Arc::new(FrameCapture),
            Box::new(FakeInput {
                position: std::cell::Cell::new((10, 10)),
                log: Log::default(),
            }),
            Box::new(NoUi),
        );
        session.running = true;
        let worker = session.worker();
        let handle = std::thread::spawn(move || {
            worker.run(ScrollJob {
                generation: 0,
                rect: Rect::new(10, 10, 200, 100),
                wait: false,
            })
        });
        let result = handle.join().expect("worker thread finishes");
        assert!(result.is_ok(), "{result:?}");
        // The frame the worker stitched is visible to the session.
        assert!(session.output().is_some());
        // Completing the job keeps the session consistent.
        let job = session.begin_grab().expect("grab");
        session.complete(job, result);
        assert!(session.is_running());
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;
    use crate::commands::{CommandError, CommandOutput};
    use std::sync::Mutex;

    /// A runner that returns queued outputs and records the specs it saw.
    #[derive(Default)]
    struct FakeRunner {
        results: Mutex<Vec<Result<CommandOutput, CommandError>>>,
        seen: Mutex<Vec<CommandSpec>>,
    }

    impl FakeRunner {
        fn push(&self, status: i32, stdout: &[u8]) {
            self.results.lock().unwrap().push(Ok(CommandOutput {
                status,
                stdout: stdout.to_vec(),
                stderr: Vec::new(),
            }));
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            self.seen.lock().unwrap().push(spec.clone());
            self.results.lock().unwrap().remove(0)
        }
    }

    /// A runner that emulates the mirror plugin endpoints and records evals.
    struct MirrorRunner {
        plugins: Mutex<Vec<String>>,
        evals: Mutex<Vec<String>>,
    }

    impl MirrorRunner {
        fn new() -> Self {
            Self {
                plugins: Mutex::new(Vec::new()),
                evals: Mutex::new(Vec::new()),
            }
        }
    }

    impl CommandRunner for MirrorRunner {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            let args = &spec.args;
            if args == &["-j", "plugin list"] {
                let names = self.plugins.lock().unwrap().clone();
                let rows: Vec<String> = names
                    .iter()
                    .map(|name| format!("{{\"name\":\"{name}\"}}"))
                    .collect();
                return Ok(CommandOutput {
                    status: 0,
                    stdout: format!("[{}]", rows.join(",")).into_bytes(),
                    stderr: Vec::new(),
                });
            }
            if args.first().map(String::as_str) == Some("plugin")
                && args.get(1).map(String::as_str) == Some("load")
            {
                self.plugins
                    .lock()
                    .unwrap()
                    .push(crate::mirror::PLUGIN_NAME.to_owned());
            }
            if args.first().map(String::as_str) == Some("repl") {
                self.plugins.lock().unwrap().clear();
            }
            if args.first().map(String::as_str) == Some("eval") {
                self.evals.lock().unwrap().push(args[1].clone());
            }
            Ok(CommandOutput {
                status: 0,
                stdout: b"ok".to_vec(),
                stderr: Vec::new(),
            })
        }
    }

    /// A native directory holding both mirror plugin files.
    struct NativePluginDir(std::path::PathBuf);

    impl NativePluginDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "tidyshot-scroll-native-{}-{tag}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            for name in [crate::mirror::MIRROR_PLUGIN, crate::mirror::CAPTURE_PLUGIN] {
                std::fs::write(path.join(name), b"plugin").unwrap();
            }
            Self(path)
        }
    }

    impl Drop for NativePluginDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The session's mirror lease must drive the real plugin: it loads the
    /// shared mirror and registers the `cursor_capture` role, then releases it.
    #[test]
    fn the_system_capture_leases_the_real_cursor_mirror() {
        let runner = MirrorRunner::new();
        let registry = crate::mirror::MirrorRegistry::new();
        let native = NativePluginDir::new("lease");
        let capture = SystemScrollCapture::new(&runner, &registry, native.0.clone());
        capture.mirror_start().unwrap();
        assert_eq!(
            runner.plugins.lock().unwrap().as_slice(),
            [crate::mirror::PLUGIN_NAME],
            "the shared plugin is loaded"
        );
        let evals = runner.evals.lock().unwrap().clone();
        assert_eq!(evals.len(), 1, "{evals:?}");
        assert!(evals[0].contains("cursor_capture"), "{evals:?}");
        assert!(
            evals[0].contains(&std::process::id().to_string()),
            "{evals:?}"
        );

        capture.mirror_stop().unwrap();
        let evals = runner.evals.lock().unwrap().clone();
        assert!(evals[1].contains("cursor_capture(0)"), "{evals:?}");
    }

    /// Without the plugin, a scrolling capture must report the rebuild
    /// instruction rather than pretend the mirror is leased.
    #[test]
    fn a_missing_mirror_plugin_is_reported() {
        let runner = MirrorRunner::new();
        let registry = crate::mirror::MirrorRegistry::new();
        let empty = std::env::temp_dir().join(format!(
            "tidyshot-scroll-native-empty-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&empty).unwrap();
        let capture = SystemScrollCapture::new(&runner, &registry, empty.clone());
        let error = capture.mirror_start().unwrap_err();
        assert!(error.contains("rebuilding"), "{error}");
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn auto_scroll_moves_the_pointer_then_injects_a_wheel_step() {
        let runner = FakeRunner::default();
        runner.push(0, b"");
        runner.push(0, b"");
        runner.push(0, b"");
        let native = std::env::temp_dir().join("tidyshot-scroll-test");
        std::fs::create_dir_all(&native).unwrap();
        std::fs::write(native.join("scroll-helper"), b"#!/bin/sh\n").unwrap();
        let input = SystemScrollInput {
            runner: &runner,
            native,
        };
        input.move_cursor(120, 40).unwrap();
        input.scroll_step(false, 3).unwrap();
        let seen = runner.seen.lock().unwrap();
        assert_eq!(seen[0].program, "hyprctl");
        assert!(seen[0].args[1].contains("cursor.move"));
        assert!(seen[1].program.ends_with("scroll-helper"));
        assert_eq!(seen[1].args, ["vertical", "3"]);
    }

    #[test]
    fn horizontal_scroll_uses_the_horizontal_helper_axis() {
        let runner = FakeRunner::default();
        runner.push(0, b"");
        let native = std::env::temp_dir().join("tidyshot-scroll-test-h");
        std::fs::create_dir_all(&native).unwrap();
        std::fs::write(native.join("scroll-helper"), b"#!/bin/sh\n").unwrap();
        let input = SystemScrollInput {
            runner: &runner,
            native,
        };
        input.scroll_step(true, 3).unwrap();
        let seen = runner.seen.lock().unwrap();
        assert_eq!(seen[0].args, ["horizontal", "3"]);
    }

    #[test]
    fn a_missing_scroll_helper_is_reported_not_silently_skipped() {
        let runner = FakeRunner::default();
        let native = std::env::temp_dir().join("tidyshot-scroll-missing");
        let input = SystemScrollInput {
            runner: &runner,
            native,
        };
        let error = input.scroll_step(false, 3).unwrap_err();
        assert!(error.contains("bin/native"), "{error}");
        assert!(runner.seen.lock().unwrap().is_empty(), "helper never ran");
    }

    #[test]
    fn cursor_position_is_read_from_hyprctl() {
        let runner = FakeRunner::default();
        runner.push(0, br#"{"x":100,"y":200}"#);
        let native = std::env::temp_dir();
        let input = SystemScrollInput {
            runner: &runner,
            native,
        };
        assert_eq!(input.cursor().unwrap(), (100, 200));
    }

    #[test]
    fn ppm_grab_parses_grim_output() {
        let runner = FakeRunner::default();
        let mut ppm = b"P6\n2 2\n255\n".to_vec();
        ppm.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        runner.push(0, &ppm);
        let capture =
            SystemScrollCapture::new(&runner, MirrorRegistry::global(), std::env::temp_dir());
        let frame = capture.grab(Rect::new(0, 0, 2, 2)).unwrap();
        assert_eq!(frame.data, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        let seen = runner.seen.lock().unwrap();
        assert_eq!(seen[0].program, "grim");
        assert!(seen[0].args.iter().any(|a| a == "ppm"));
    }

    #[test]
    fn a_truncated_ppm_is_an_error() {
        let runner = FakeRunner::default();
        runner.push(0, b"P6\n2 2\n255\n\x01\x02");
        let capture =
            SystemScrollCapture::new(&runner, MirrorRegistry::global(), std::env::temp_dir());
        let error = capture.grab(Rect::new(0, 0, 2, 2)).unwrap_err();
        assert!(error.contains("truncated"), "{error}");
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<String>>>);

    struct Capture {
        log: Log,
        frame: Option<Vec<u8>>,
    }

    impl ScrollCapture for Capture {
        fn grab(&self, _rect: Rect) -> Result<FrameData, String> {
            self.frame.clone().map_or_else(
                || Err("unused".to_owned()),
                |data| Ok(FrameData::new(data, 200, 100, 3)),
            )
        }
        fn mirror_start(&self) -> Result<(), String> {
            self.log.0.lock().unwrap().push("start".to_owned());
            Ok(())
        }
        fn mirror_stop(&self) -> Result<(), String> {
            self.log.0.lock().unwrap().push("stop".to_owned());
            Ok(())
        }
    }

    struct Input;

    impl ScrollInput for Input {
        fn cursor(&self) -> Result<(i32, i32), String> {
            Ok((10, 10))
        }
        fn move_cursor(&self, _x: i32, _y: i32) -> Result<(), String> {
            Ok(())
        }
        fn scroll_step(&self, _horizontal: bool, _amount: i32) -> Result<(), String> {
            Ok(())
        }
    }

    struct Ui;

    impl ScrollUi for Ui {
        fn before_grab(&mut self) -> bool {
            false
        }
        fn after_grab(&mut self) {}
        fn update(&mut self, _frame: FrameRef<'_>) {}
        fn keys_stop(&mut self) {}
        fn visuals_close(&mut self) {}
    }

    /// A rejected frame must leave position, origin and the accumulated image
    /// untouched, so the user can scroll back and continue.
    #[test]
    fn a_rejected_alignment_does_not_disturb_stitch_state() {
        let log = Log::default();
        let mut session = ScrollSession::new(
            Rect::new(10, 10, 200, 100),
            false,
            Arc::new(Capture { log, frame: None }),
            Box::new(Input),
            Box::new(Ui),
        );
        session.start();
        let source: Vec<u8> = (0..200 * 100 * 3)
            .map(|index| ((index * 31) % 200) as u8)
            .collect();
        session
            .stitcher()
            .push(FrameData::new(source.clone(), 200, 100, 3))
            .unwrap();
        let position = session.stitcher().position();
        let accepted = session.stitcher().accepted();

        let job = session.begin_grab().unwrap();
        let noise: Vec<u8> = (0..200 * 100 * 3)
            .map(|index| ((index * 97 + 13) % 255) as u8)
            .collect();
        let result = session
            .stitcher()
            .push(FrameData::new(noise, 200, 100, 3))
            .map_err(|error| error.to_string());
        let done = session.complete(job, result);
        assert!(done.is_none());
        assert!(session.status().contains("Could not align"));
        assert_eq!(session.stitcher().position(), position);
        assert_eq!(session.stitcher().accepted(), accepted);
        assert_eq!(session.output().unwrap().data, source);
        // The session keeps running so the next frame can recover.
        assert!(session.is_running());
        assert!(session.begin_grab().is_some());
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    struct Capture;
    impl ScrollCapture for Capture {
        fn grab(&self, _rect: Rect) -> Result<FrameData, String> {
            Err("unused".to_owned())
        }
        fn mirror_start(&self) -> Result<(), String> {
            Ok(())
        }
        fn mirror_stop(&self) -> Result<(), String> {
            Ok(())
        }
    }

    struct Input;
    impl ScrollInput for Input {
        fn cursor(&self) -> Result<(i32, i32), String> {
            Ok((10, 10))
        }
        fn move_cursor(&self, _x: i32, _y: i32) -> Result<(), String> {
            Ok(())
        }
        fn scroll_step(&self, _horizontal: bool, _amount: i32) -> Result<(), String> {
            Ok(())
        }
    }

    struct Ui;
    impl ScrollUi for Ui {
        fn before_grab(&mut self) -> bool {
            false
        }
        fn after_grab(&mut self) {}
        fn update(&mut self, _frame: FrameRef<'_>) {}
        fn keys_stop(&mut self) {}
        fn visuals_close(&mut self) {}
    }

    /// A tall page with a unique pseudo-random texture in every row.
    fn textured_page(height: usize, width: usize) -> Vec<u8> {
        let mut state: u64 = 12;
        let mut data = vec![0u8; height * width * 3];
        for row in data.chunks_mut(width * 3) {
            for pixel in row.chunks_mut(3) {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let value = (state >> 33) as u8;
                pixel[0] = value;
                pixel[1] = value.wrapping_add(37);
                pixel[2] = value.wrapping_mul(3);
            }
        }
        data
    }

    /// A frame that would exceed the 120 MP budget stops the session with the
    /// reason on screen and keeps the rows already stitched.
    #[test]
    fn exceeding_the_megapixel_cap_stops_with_the_reason_and_keeps_the_image() {
        // A tall page whose every row carries unique high-frequency detail, so
        // a 180-row jump aligns and then crosses the cap.
        let width = 200usize;
        let page = textured_page(1200, width);
        let first = FrameData::new(page[..600 * width * 3].to_vec(), width, 600, 3);
        let first_image = first.clone();
        let mut session = ScrollSession::with_limit(
            Rect::new(0, 0, width as i32, 600),
            false,
            width * 700,
            Arc::new(Capture),
            Box::new(Input),
            Box::new(Ui),
        );
        session.start();
        session.stitcher().push(first).unwrap();
        // Jumping forward 200 rows needs an 800-row image, over the cap.
        let second = FrameData::new(
            page[200 * width * 3..800 * width * 3].to_vec(),
            width,
            600,
            3,
        );
        let job = session.begin_grab().unwrap();
        let result = session
            .stitcher()
            .push(second)
            .map_err(|error| error.to_string());
        session.complete(job, result);
        assert!(!session.is_running(), "status={:?}", session.status());
        assert!(
            session.status().contains("120 megapixel"),
            "{}",
            session.status()
        );
        assert_eq!(session.output().unwrap().data, first_image.data);
        // A retry is offered and a fresh start is possible.
        assert!(session.start_label.contains("Retry"));
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;

    #[test]
    fn native_dir_lookup_prefers_a_directory_with_the_helper() {
        // The workspace has a built helper once bin/native has run; the
        // lookup must return a path that actually contains the binary.
        if let Some(dir) = native_dir() {
            assert!(dir.join("scroll-helper").is_file());
        }
        // Candidates are always non-empty and include the manifest directory,
        // so a missing helper is reported against a concrete location.
        let candidates = native_candidates();
        assert!(!candidates.is_empty());
        assert!(
            candidates.iter().any(|dir| dir.ends_with("native")),
            "{candidates:?}"
        );
    }
}
