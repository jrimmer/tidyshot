//! The loop that drives a scrolling capture from the QML controls or the CLI.
// Ported from omnishot/widgets.py (ScrollPanel tick/start/finish)
//!
//! [`crate::scroll::ScrollSession`] owns the state machine and the stitcher;
//! this module owns the pacing. One loop waits for Start, grabs on the
//! configured interval, folds each frame back through the session and stops on
//! Done, Cancel or a headless end condition. Commands arrive through a
//! [`ScrollControl`], so the QML thread never borrows the session.
//!
//! The headless driver is what a `tidyshot scroll` invocation without a window
//! uses: it starts immediately, auto-scrolls when the helper is available and
//! finishes at the end of the content, at a repeated row or at the 120
//! megapixel ceiling, keeping whatever was stitched.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::scroll::{END_AFTER_UNCHANGED, ScrollSession};
use crate::stitch::FrameData;

/// How long a running session waits for a command before it grabs a frame.
pub const COMMAND_POLL: Duration = Duration::from_millis(100);

/// How long the controls stay hidden before a grab, so the compositor presents
/// the desktop without them and the controls never land in a frame.
pub const HIDE_DELAY: Duration = Duration::from_millis(150);

/// A command the controls send to the running session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollCommand {
    /// Begin grabbing frames.
    Start,
    /// Flip auto-scroll.
    ToggleAuto,
    /// Set auto-scroll explicitly.
    SetAuto(bool),
    /// Stop grabbing, release the controls and hand back the stitched image.
    Done,
    /// Abandon the capture without saving.
    Cancel,
}

/// What the controls show for one moment of the capture.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScrollStatus {
    /// Whether the controls window should be on screen.
    pub visible: bool,
    /// The session's status line.
    pub status: String,
    /// Frames folded into the stitch so far.
    pub frames: u32,
    /// Width of the stitched image, in output orientation.
    pub width: usize,
    /// Height of the stitched image, in output orientation.
    pub height: usize,
    /// Whether auto-scroll is on.
    pub auto: bool,
    /// Whether frames are currently being accepted.
    pub running: bool,
    /// The session's start button label: "Retry Capture" after a failed start
    /// or grab, so the controls offer the retry the session expects.
    pub start_label: String,
    /// Whether this is a horizontal capture.
    pub horizontal: bool,
}

/// The result of a driven scrolling capture.
#[derive(Debug)]
pub enum DriveOutcome {
    /// The user finished, or a headless capture reached its stop condition.
    ///
    /// The second field is set when a frame error or the megapixel ceiling
    /// stopped the capture, so the image is shorter than the content. The
    /// image is still the right result; the caller decides how to report it.
    Finished(FrameData, Option<String>),
    /// The user cancelled, or the caller asked to stop.
    Cancelled,
    /// No frame could be produced.
    Failed(String),
}

#[derive(Default)]
struct ControlState {
    commands: VecDeque<ScrollCommand>,
    status: ScrollStatus,
    closed: bool,
}

/// The shared command queue and status snapshot between the UI and the loop.
#[derive(Clone, Default)]
pub struct ScrollControl {
    inner: Arc<(Mutex<ControlState>, Condvar)>,
}

impl ScrollControl {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a command for the running loop.
    pub fn send(&self, command: ScrollCommand) {
        let (lock, wake) = &*self.inner;
        let mut state = lock.lock().expect("scroll control poisoned");
        if state.closed {
            return;
        }
        state.commands.push_back(command);
        wake.notify_all();
    }

    /// The latest published status.
    #[must_use]
    pub fn snapshot(&self) -> ScrollStatus {
        let (lock, _) = &*self.inner;
        lock.lock().expect("scroll control poisoned").status.clone()
    }

    /// Publish a status snapshot for the UI.
    pub fn publish(&self, status: ScrollStatus) {
        let (lock, _) = &*self.inner;
        lock.lock().expect("scroll control poisoned").status = status;
    }

    /// Wait for a command, or return `None` when `timeout` elapses.
    pub fn wait(&self, timeout: Duration) -> Option<ScrollCommand> {
        let (lock, wake) = &*self.inner;
        let mut state = lock.lock().expect("scroll control poisoned");
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(command) = state.commands.pop_front() {
                return Some(command);
            }
            if state.closed {
                return None;
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let (next, _) = wake
                .wait_timeout(state, remaining)
                .expect("scroll control poisoned");
            state = next;
        }
    }

    /// Stop accepting commands and drop any that are still queued.
    pub fn close(&self) {
        let (lock, wake) = &*self.inner;
        let mut state = lock.lock().expect("scroll control poisoned");
        state.closed = true;
        state.commands.clear();
        wake.notify_all();
    }
}

/// The controls side of a running capture: where status goes and where
/// commands come from.
///
/// It is `Send` so the GUI loop can run on a worker thread; the Qt adapter
/// queues property updates back onto the event loop.
pub trait ScrollPanel: Send {
    /// Show one status snapshot.
    fn publish(&mut self, status: &ScrollStatus);
    /// Wait for the next command, or `None` after `timeout`.
    fn wait_command(&mut self, timeout: Duration) -> Option<ScrollCommand>;
}

/// A panel backed by a [`ScrollControl`], for the GUI and for tests.
pub struct ControlPanel {
    control: ScrollControl,
}

impl ControlPanel {
    #[must_use]
    pub fn new(control: ScrollControl) -> Self {
        Self { control }
    }
}

impl ScrollPanel for ControlPanel {
    fn publish(&mut self, status: &ScrollStatus) {
        self.control.publish(status.clone());
    }

    fn wait_command(&mut self, timeout: Duration) -> Option<ScrollCommand> {
        self.control.wait(timeout)
    }
}

/// A panel with no controls: the headless CLI capture.
///
/// It keeps the last status so a caller can report why a capture stopped, and
/// waits out the interval before the next grab instead of accepting commands.
#[derive(Debug, Default)]
pub struct HeadlessPanel {
    pub status: ScrollStatus,
}

impl ScrollPanel for HeadlessPanel {
    fn publish(&mut self, status: &ScrollStatus) {
        self.status = status.clone();
    }

    fn wait_command(&mut self, timeout: Duration) -> Option<ScrollCommand> {
        sleep_in_slices(timeout);
        None
    }
}

/// Sleep in small slices so a cancellation is noticed without waiting out a
/// whole capture interval.
fn sleep_in_slices(duration: Duration) {
    let end = std::time::Instant::now() + duration;
    loop {
        let remaining = end.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return;
        }
        std::thread::sleep(remaining.min(COMMAND_POLL));
    }
}

/// The stitched image size in output orientation, without copying it.
fn image_size(session: &ScrollSession) -> (usize, usize) {
    let stitcher = session.stitcher();
    match stitcher.image_view() {
        Some(frame) if stitcher.horizontal() => (frame.height(), frame.width()),
        Some(frame) => (frame.width(), frame.height()),
        None => (
            session.rect.width.max(0) as usize,
            session.rect.height.max(0) as usize,
        ),
    }
}

/// Project the session onto the controls model.
#[must_use]
pub fn snapshot(session: &ScrollSession, visible: bool) -> ScrollStatus {
    let (width, height) = image_size(session);
    ScrollStatus {
        visible,
        status: session.status().to_owned(),
        frames: session.stitcher().accepted(),
        width,
        height,
        auto: session.auto,
        running: session.is_running(),
        start_label: session.start_label.clone(),
        horizontal: session.horizontal,
    }
}

/// How the driver treats the session it is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveMode {
    /// The controls window owns Start, Done and Cancel.
    Interactive,
    /// Start at once; `autoscroll` asks the helper to advance the pointer and
    /// the capture ends at the first stop condition.
    Headless { autoscroll: bool },
}

impl DriveMode {
    const fn headless(self) -> bool {
        matches!(self, Self::Headless { .. })
    }
}

/// Finish the session and take the stitched image it produced.
///
/// [`ScrollSession::finish`] returns the image only when a completion
/// callback is installed; the driver reads the stitch directly so it works
/// for the CLI and the controls alike. `warning` carries a frame error that
/// stopped the capture, so an image saved from it is not silently partial.
fn finish_with(session: &mut ScrollSession<'_>, warning: Option<String>) -> DriveOutcome {
    let _ = session.finish();
    if !session.finished {
        // A frame is still in flight; the next completion finalises.
        return DriveOutcome::Failed(String::new());
    }
    match session.output() {
        Some(frame) => DriveOutcome::Finished(frame, warning),
        None => DriveOutcome::Failed("The scrolling capture has no frames to save".to_owned()),
    }
}

fn finish(session: &mut ScrollSession<'_>) -> DriveOutcome {
    finish_with(session, None)
}

/// True when [`finish`] could not finalise yet because a frame is in flight.
fn deferred(outcome: &DriveOutcome) -> bool {
    matches!(outcome, DriveOutcome::Failed(message) if message.is_empty())
}

/// Drive one session until it finishes, is cancelled or fails.
// Ported from omnishot/widgets.py (ScrollPanel.tick, start, finish, closeEvent)
pub fn drive<'a>(
    session: &mut ScrollSession<'a>,
    panel: &mut dyn ScrollPanel,
    mode: DriveMode,
    should_cancel: &mut dyn FnMut() -> bool,
) -> DriveOutcome {
    let headless = mode.headless();
    // The controls stay on screen for an interactive capture; a headless one
    // has no controls to show.
    let controls_visible = !headless;
    if let DriveMode::Headless { autoscroll } = mode {
        session.auto = autoscroll;
        session.start();
    }
    let mut idle = 0u32;
    loop {
        if should_cancel() {
            session.close();
            panel.publish(&snapshot(session, false));
            return DriveOutcome::Cancelled;
        }
        if !session.is_running() {
            if headless {
                // A failed start or a stopped session keeps what it stitched.
                let status = snapshot(session, controls_visible);
                panel.publish(&status);
                return match session.output() {
                    Some(frame) => DriveOutcome::Finished(frame, Some(status.status)),
                    None => DriveOutcome::Failed(status.status),
                };
            }
            panel.publish(&snapshot(session, controls_visible));
            match panel.wait_command(COMMAND_POLL) {
                Some(ScrollCommand::Start) => session.start(),
                Some(ScrollCommand::Done) => {
                    // Done before any frame is a cancel from the shortcut, not
                    // a reason to close the session with nothing to save.
                    if session.stitcher().accepted() == 0 {
                        session.close();
                        panel.publish(&snapshot(session, false));
                        return DriveOutcome::Cancelled;
                    }
                    let outcome = finish(session);
                    if !deferred(&outcome) {
                        panel.publish(&snapshot(session, false));
                        return outcome;
                    }
                }
                Some(ScrollCommand::Cancel) => {
                    session.close();
                    panel.publish(&snapshot(session, false));
                    return DriveOutcome::Cancelled;
                }
                Some(ScrollCommand::ToggleAuto) => session.toggle_auto(),
                Some(ScrollCommand::SetAuto(enabled)) => session.set_auto(enabled),
                None => {}
            }
            continue;
        }

        match panel.wait_command(session.interval) {
            Some(ScrollCommand::Cancel) => {
                session.close();
                panel.publish(&snapshot(session, false));
                return DriveOutcome::Cancelled;
            }
            Some(ScrollCommand::Done) => {
                let outcome = finish(session);
                if !deferred(&outcome) {
                    panel.publish(&snapshot(session, false));
                    return outcome;
                }
                continue;
            }
            Some(ScrollCommand::ToggleAuto) => {
                session.toggle_auto();
                panel.publish(&snapshot(session, controls_visible));
                continue;
            }
            Some(ScrollCommand::SetAuto(enabled)) => {
                session.set_auto(enabled);
                panel.publish(&snapshot(session, controls_visible));
                continue;
            }
            Some(ScrollCommand::Start) => {
                panel.publish(&snapshot(session, controls_visible));
                continue;
            }
            None => {}
        }

        let Some(job) = session.begin_grab() else {
            continue;
        };
        if controls_visible {
            panel.publish(&snapshot(session, false));
            std::thread::sleep(HIDE_DELAY);
        }
        let result = session.run_job(job);
        let changed = result.as_ref().ok().map(|(changed, _)| *changed);
        let unalignable = result
            .as_ref()
            .is_ok_and(|(_, message)| message.starts_with("Could not align"));
        let completed = session.complete(job, result);
        panel.publish(&snapshot(session, controls_visible));
        if let Some(frame) = completed {
            panel.publish(&snapshot(session, false));
            return DriveOutcome::Finished(frame, None);
        }
        if session.finished {
            // A deferred Done finalised with this frame.
            panel.publish(&snapshot(session, false));
            return match session.output() {
                Some(frame) => DriveOutcome::Finished(frame, None),
                None => {
                    DriveOutcome::Failed("The scrolling capture has no frames to save".to_owned())
                }
            };
        }
        if !headless {
            continue;
        }
        if unalignable {
            // A headless capture cannot be nudged back into alignment, so the
            // rows already stitched are the result.
            let outcome = finish(session);
            if !deferred(&outcome) {
                panel.publish(&snapshot(session, controls_visible));
                return outcome;
            }
        }
        match changed {
            Some(true) => idle = 0,
            Some(false) => {
                idle += 1;
                if idle >= END_AFTER_UNCHANGED || session.reached_end {
                    let outcome = finish(session);
                    if !deferred(&outcome) {
                        panel.publish(&snapshot(session, controls_visible));
                        return outcome;
                    }
                }
            }
            None => {
                // A failed grab or an over-limit frame stops the session but
                // keeps the rows already stitched, so Done still works. The
                // reason travels with the image so a headless caller can warn
                // that the stitch is short.
                let status = snapshot(session, controls_visible);
                panel.publish(&status);
                return match session.output() {
                    Some(frame) => DriveOutcome::Finished(frame, Some(status.status)),
                    None => DriveOutcome::Failed(status.status),
                };
            }
        }
        if !session.is_running() {
            let status = snapshot(session, controls_visible);
            panel.publish(&status);
            return match session.output() {
                Some(frame) => DriveOutcome::Finished(frame, Some(status.status)),
                None => DriveOutcome::Failed(status.status),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;
    use crate::scroll::{ScrollCapture, ScrollInput, ScrollSession, ScrollUi};
    use std::cell::Cell;
    use std::sync::Mutex;

    /// A capture that hands out queued frames, then repeats the last one.
    struct Frames {
        queue: Mutex<VecDeque<FrameData>>,
        last: Mutex<Option<FrameData>>,
        started: Mutex<u32>,
        stopped: Mutex<u32>,
    }

    impl Frames {
        fn new(frames: Vec<FrameData>) -> Self {
            Self {
                queue: Mutex::new(frames.into()),
                last: Mutex::new(None),
                started: Mutex::new(0),
                stopped: Mutex::new(0),
            }
        }
    }

    impl ScrollCapture for Frames {
        fn grab(&self, _rect: Rect) -> Result<FrameData, String> {
            if let Some(frame) = self.queue.lock().unwrap().pop_front() {
                *self.last.lock().unwrap() = Some(frame.clone());
                return Ok(frame);
            }
            self.last
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| "no frame queued".to_owned())
        }

        fn mirror_start(&self) -> Result<(), String> {
            *self.started.lock().unwrap() += 1;
            Ok(())
        }

        fn mirror_stop(&self) -> Result<(), String> {
            *self.stopped.lock().unwrap() += 1;
            Ok(())
        }
    }

    /// A pointer kept inside the capture that records wheel steps.
    struct Pointer {
        position: Cell<(i32, i32)>,
        scrolls: Mutex<u32>,
    }

    impl Pointer {
        fn inside() -> Self {
            Self {
                position: Cell::new((50, 50)),
                scrolls: Mutex::new(0),
            }
        }
    }

    impl ScrollInput for Pointer {
        fn cursor(&self) -> Result<(i32, i32), String> {
            Ok(self.position.get())
        }

        fn move_cursor(&self, x: i32, y: i32) -> Result<(), String> {
            self.position.set((x, y));
            Ok(())
        }

        fn scroll_step(&self, _horizontal: bool, _amount: i32) -> Result<(), String> {
            *self.scrolls.lock().unwrap() += 1;
            Ok(())
        }
    }

    struct Ui;
    impl ScrollUi for Ui {
        fn before_grab(&mut self) -> bool {
            false
        }
        fn after_grab(&mut self) {}
        fn update(&mut self, _frame: crate::stitch::FrameRef<'_>) {}
        fn keys_stop(&mut self) {}
        fn visuals_close(&mut self) {}
    }

    /// A panel that records every status it shows.
    struct Recorder {
        published: Arc<Mutex<Vec<ScrollStatus>>>,
        control: Option<ScrollControl>,
    }

    impl ScrollPanel for Recorder {
        fn publish(&mut self, status: &ScrollStatus) {
            self.published.lock().unwrap().push(status.clone());
        }

        fn wait_command(&mut self, timeout: Duration) -> Option<ScrollCommand> {
            match &self.control {
                Some(control) => control.wait(timeout),
                None => {
                    sleep_in_slices(timeout);
                    None
                }
            }
        }
    }

    fn textured_page(height: usize, width: usize) -> Vec<u8> {
        let mut state: u64 = 71;
        let mut data = vec![0u8; height * width * 3];
        for row in data.chunks_mut(width * 3) {
            for pixel in row.chunks_mut(3) {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let value = (state >> 33) as u8;
                pixel[0] = value;
                pixel[1] = value.wrapping_add(29);
                pixel[2] = value.wrapping_mul(5);
            }
        }
        data
    }

    fn vertical_frames(page_height: usize, width: usize, viewport: usize) -> Vec<FrameData> {
        let page = textured_page(page_height, width);
        (0..=page_height - viewport)
            .step_by(viewport / 2)
            .map(|offset| {
                let start = offset * width * 3;
                FrameData::new(
                    page[start..start + viewport * width * 3].to_vec(),
                    width,
                    viewport,
                    3,
                )
            })
            .collect()
    }

    fn session(frames: Vec<FrameData>, rect: Rect, horizontal: bool) -> ScrollSession<'static> {
        ScrollSession::new(
            rect,
            horizontal,
            Arc::new(Frames::new(frames)),
            Box::new(Pointer::inside()),
            Box::new(Ui),
        )
    }

    fn recorder() -> (Recorder, Arc<Mutex<Vec<ScrollStatus>>>) {
        let published = Arc::new(Mutex::new(Vec::new()));
        (
            Recorder {
                published: Arc::clone(&published),
                control: None,
            },
            published,
        )
    }

    #[test]
    fn a_headless_capture_finishes_at_the_end_of_the_content() {
        let width = 200;
        let viewport = 200;
        let frames = vertical_frames(800, width, viewport);
        let mut session = session(
            frames,
            Rect::new(0, 0, width as i32, viewport as i32),
            false,
        );
        let (mut panel, published) = recorder();
        let outcome = drive(
            &mut session,
            &mut panel,
            DriveMode::Headless { autoscroll: true },
            &mut || false,
        );
        let DriveOutcome::Finished(image, _) = outcome else {
            panic!("expected a finished capture, got {outcome:?}");
        };
        assert_eq!(image.width, width);
        assert_eq!(image.height, 800);
        assert_eq!(session.stitcher().accepted(), 7);
        let statuses = published.lock().unwrap();
        assert!(
            statuses.iter().all(|status| !status.visible),
            "a headless capture has no controls to show"
        );
        assert!(statuses.iter().any(|status| status.frames > 0));
    }

    #[test]
    fn repeated_rows_end_the_capture_without_growing_the_image() {
        let width = 200;
        let page = textured_page(200, width);
        let first = FrameData::new(page, width, 200, 3);
        let identical = (0..8).map(|_| first.clone()).collect();
        let mut session = session(identical, Rect::new(0, 0, width as i32, 200), false);
        let (mut panel, published) = recorder();
        let outcome = drive(
            &mut session,
            &mut panel,
            DriveMode::Headless { autoscroll: true },
            &mut || false,
        );
        let DriveOutcome::Finished(image, _) = outcome else {
            panic!("expected the first frame, got {outcome:?}");
        };
        assert_eq!(image.height, 200);
        assert_eq!(image.data, first.data);
        assert_eq!(session.stitcher().accepted(), 1);
        assert!(
            published
                .lock()
                .unwrap()
                .iter()
                .all(|status| status.frames <= 1)
        );
    }

    /// A tall, structured page like the stitcher fixtures: repeated rules
    /// carry the anchors and a random block per band keeps rows unique.
    fn page(height: usize, width: usize) -> Vec<u8> {
        let mut state: u64 = 12;
        let mut data = vec![245u8; height * width * 3];
        for y in (0..height).step_by(37) {
            for x in 20..width.saturating_sub(20) {
                if (x + y) % 7 < 3 {
                    let offset = (y * width + x) * 3;
                    data[offset] = 30;
                    data[offset + 1] = 40;
                    data[offset + 2] = 55;
                }
            }
            let block = if width > 80 { width - 80 } else { width / 2 };
            for x in block..(block + 50).min(width) {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let offset = (y * width + x) * 3;
                data[offset] = (state >> 33) as u8;
                data[offset + 1] = (state >> 41) as u8;
                data[offset + 2] = (state >> 49) as u8;
            }
        }
        data
    }

    #[test]
    fn a_headless_capture_stops_on_an_unalignable_frame() {
        let width = 600;
        let height = 600;
        let first = page(height * 2, width);
        let first = FrameData::new(first[..height * width * 3].to_vec(), width, height, 3);
        // Unrelated noise cannot be matched, so the capture must stop with the
        // first frame instead of grabbing forever.
        let mut state: u64 = 99;
        let mut noise = vec![0u8; width * height * 3];
        for byte in &mut noise {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *byte = (state >> 33) as u8;
        }
        let noise = FrameData::new(noise, width, height, 3);
        let frames = vec![first.clone(), noise];
        let mut session = session(frames, Rect::new(0, 0, width as i32, height as i32), false);
        let (mut panel, _published) = recorder();
        let outcome = drive(
            &mut session,
            &mut panel,
            DriveMode::Headless { autoscroll: true },
            &mut || false,
        );
        let DriveOutcome::Finished(image, _) = outcome else {
            panic!("expected the first frame, got {outcome:?}");
        };
        assert_eq!(image.data, first.data);
        assert_eq!(session.stitcher().accepted(), 1);
    }

    #[test]
    fn a_headless_capture_without_auto_scroll_still_saves_the_first_viewport() {
        // The CLI falls back to this mode when the scroll helper is missing:
        // no pointer can be driven, so identical frames end the capture and
        // the one viewport it grabbed is still the result.
        let width = 200;
        let page = textured_page(200, width);
        let first = FrameData::new(page, width, 200, 3);
        let frames = (0..6).map(|_| first.clone()).collect();
        let mut session = session(frames, Rect::new(0, 0, width as i32, 200), false);
        let (mut panel, _published) = recorder();
        let outcome = drive(
            &mut session,
            &mut panel,
            DriveMode::Headless { autoscroll: false },
            &mut || false,
        );
        let DriveOutcome::Finished(image, warning) = outcome else {
            panic!("expected a finished capture, got {outcome:?}");
        };
        assert_eq!(image.data, first.data);
        assert!(
            warning.is_none(),
            "a normal end is not a warning: {warning:?}"
        );
        assert!(!session.auto);
    }

    #[test]
    fn a_horizontal_headless_capture_grows_sideways() {
        let height = 600;
        let page_width = 3200;
        let viewport = 600;
        // Transpose the tall page into a wide one: each column now carries the
        // unique texture, so a sideways match is unambiguous.
        let tall = page(page_width, height);
        let mut wide = vec![0u8; page_width * height * 3];
        for y in 0..height {
            for x in 0..page_width {
                let source = (x * height + y) * 3;
                let target = (y * page_width + x) * 3;
                wide[target..target + 3].copy_from_slice(&tall[source..source + 3]);
            }
        }
        let frames: Vec<FrameData> = [0usize, 180, 360, 540]
            .into_iter()
            .map(|offset| {
                let mut data = vec![0u8; height * viewport * 3];
                for row in 0..height {
                    let source = (row * page_width + offset) * 3;
                    let target = row * viewport * 3;
                    data[target..target + viewport * 3]
                        .copy_from_slice(&wide[source..source + viewport * 3]);
                }
                FrameData::new(data, viewport, height, 3)
            })
            .collect();
        let mut session = session(
            frames,
            Rect::new(0, 0, viewport as i32, height as i32),
            true,
        );
        let (mut panel, _published) = recorder();
        let outcome = drive(
            &mut session,
            &mut panel,
            DriveMode::Headless { autoscroll: true },
            &mut || false,
        );
        let DriveOutcome::Finished(image, _) = outcome else {
            panic!("expected a finished capture, got {outcome:?}");
        };
        assert_eq!(image.height, height);
        assert_eq!(image.width, 1140);
    }

    #[test]
    fn a_cancel_request_stops_without_saving() {
        let width = 200;
        let frames = vertical_frames(800, width, 200);
        let mut session = session(frames, Rect::new(0, 0, width as i32, 200), false);
        let (mut panel, _published) = recorder();
        let mut captured = 0u32;
        let outcome = drive(
            &mut session,
            &mut panel,
            DriveMode::Headless { autoscroll: true },
            &mut || {
                captured += 1;
                captured > 2
            },
        );
        assert!(matches!(outcome, DriveOutcome::Cancelled), "{outcome:?}");
        assert!(session.closed);
    }

    #[test]
    fn interactive_start_then_cancel_releases_the_capture() {
        let width = 200;
        let page = textured_page(200, width);
        let frames = vec![FrameData::new(
            page[..200 * width * 3].to_vec(),
            width,
            200,
            3,
        )];
        let control = ScrollControl::new();
        control.send(ScrollCommand::Start);
        control.send(ScrollCommand::Cancel);
        let mut panel = ControlPanel::new(control.clone());
        let mut session = session(frames, Rect::new(0, 0, width as i32, 200), false);
        let outcome = drive(
            &mut session,
            &mut panel,
            DriveMode::Interactive,
            &mut || false,
        );
        assert!(matches!(outcome, DriveOutcome::Cancelled), "{outcome:?}");
        assert!(session.closed);
        assert!(!control.snapshot().visible);
    }

    #[test]
    fn an_interactive_done_saves_the_stitched_image() {
        let width = 200;
        let viewport = 200;
        let frames = vertical_frames(400, width, viewport);
        let control = ScrollControl::new();
        control.send(ScrollCommand::Start);
        // Done as soon as a frame has landed keeps the test deterministic:
        // the user presses Done after watching the preview grow.
        struct DoneAfterFirst {
            inner: ControlPanel,
            done: ControlControl,
        }
        struct ControlControl(ScrollControl);
        impl ScrollPanel for DoneAfterFirst {
            fn publish(&mut self, status: &ScrollStatus) {
                self.inner.publish(status);
                if status.frames > 0 && status.visible {
                    self.done.0.send(ScrollCommand::Done);
                }
            }
            fn wait_command(&mut self, timeout: Duration) -> Option<ScrollCommand> {
                self.inner.wait_command(timeout)
            }
        }
        let mut watch = DoneAfterFirst {
            inner: ControlPanel::new(control.clone()),
            done: ControlControl(control.clone()),
        };
        let mut session = session(
            frames,
            Rect::new(0, 0, width as i32, viewport as i32),
            false,
        );
        let outcome = drive(
            &mut session,
            &mut watch,
            DriveMode::Interactive,
            &mut || false,
        );
        let DriveOutcome::Finished(image, _) = outcome else {
            panic!("expected a finished capture, got {outcome:?}");
        };
        assert_eq!(image.width, width);
        assert_eq!(image.height, viewport);
    }

    #[test]
    fn an_interactive_done_before_start_cancels_without_closing_on_a_failure() {
        let width = 200;
        let frames = vertical_frames(400, width, 200);
        let control = ScrollControl::new();
        // A `scroll-done` shortcut reaches Done while the session is open but
        // never started, so no frame could have landed yet.
        control.send(ScrollCommand::Done);
        let mut panel = ControlPanel::new(control.clone());
        let mut session = session(frames, Rect::new(0, 0, width as i32, 200), false);
        let outcome = drive(
            &mut session,
            &mut panel,
            DriveMode::Interactive,
            &mut || false,
        );
        assert!(matches!(outcome, DriveOutcome::Cancelled), "{outcome:?}");
        assert!(session.closed, "the session must not linger open");
        assert!(!control.snapshot().visible);
    }

    #[test]
    fn a_headless_stop_that_keeps_the_image_reports_why_it_stopped() {
        struct FailsAfterFirst {
            first: Mutex<Option<FrameData>>,
        }
        impl ScrollCapture for FailsAfterFirst {
            fn grab(&self, _rect: Rect) -> Result<FrameData, String> {
                let mut first = self.first.lock().unwrap();
                match first.take() {
                    Some(frame) => Ok(frame),
                    None => Err("Display disconnected".to_owned()),
                }
            }
            fn mirror_start(&self) -> Result<(), String> {
                Ok(())
            }
            fn mirror_stop(&self) -> Result<(), String> {
                Ok(())
            }
        }
        let width = 200;
        let page = textured_page(400, width);
        let first = FrameData::new(page[..200 * width * 3].to_vec(), width, 200, 3);
        let mut session = ScrollSession::new(
            Rect::new(0, 0, width as i32, 200),
            false,
            Arc::new(FailsAfterFirst {
                first: Mutex::new(Some(first.clone())),
            }),
            Box::new(Pointer::inside()),
            Box::new(Ui),
        );
        let (mut panel, _published) = recorder();
        let outcome = drive(
            &mut session,
            &mut panel,
            DriveMode::Headless { autoscroll: true },
            &mut || false,
        );
        let DriveOutcome::Finished(image, warning) = outcome else {
            panic!("expected the stitched image, got {outcome:?}");
        };
        assert_eq!(image.data, first.data);
        let warning = warning.expect("a stopped capture must carry its reason");
        assert!(warning.contains("Display disconnected"), "{warning}");
    }

    #[test]
    fn the_headless_driver_keeps_the_image_when_the_grab_fails() {
        struct Broken;
        impl ScrollCapture for Broken {
            fn grab(&self, _rect: Rect) -> Result<FrameData, String> {
                Err("Display disconnected".to_owned())
            }
            fn mirror_start(&self) -> Result<(), String> {
                Ok(())
            }
            fn mirror_stop(&self) -> Result<(), String> {
                Ok(())
            }
        }
        let mut session = ScrollSession::new(
            Rect::new(0, 0, 200, 200),
            false,
            Arc::new(Broken),
            Box::new(Pointer::inside()),
            Box::new(Ui),
        );
        let (mut panel, _published) = recorder();
        let outcome = drive(
            &mut session,
            &mut panel,
            DriveMode::Headless { autoscroll: true },
            &mut || false,
        );
        match outcome {
            DriveOutcome::Failed(message) => assert!(message.contains("Display disconnected")),
            other => panic!("expected a failure, got {other:?}"),
        }
    }
}
