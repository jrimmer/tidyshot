//! The recording runtime: plan a take, lease it, run the encoder, finish it.
// Ported from omnishot/recording.py and omnishot/widgets.py (RecordSetup,
// RecordingControls)
//!
//! One take is a `gpu-screen-recorder` child bound to a journal lease. While it
//! runs the runtime publishes `$XDG_RUNTIME_DIR/tidyshot-status.json` for the
//! bar widget, and the controls send pause, resume, stop, restart and cancel
//! through a command queue so the Qt thread never touches the encoder.
//!
//! A finished take commits its sidecar, its history row and a preview. A take
//! that loses its encoder leaves the journal marker and the Studio checkpoint
//! in place, so [`recover_on_start`] can recover or discard it on the next
//! launch.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::commands::{CommandRunner, CommandSpec, hypr};
use crate::json::Value;
use crate::recording::Workspace;
use crate::recording::checkpoint::CheckpointScheduler;
use crate::recording::display::{CleanCapture, Monitor};
use crate::recording::journal::{Journal, ObjectUpdate};
use crate::recording::options::{recording_monitors, recording_output};
use crate::recording::recorder::{CaptureFormat, RecordOptions, recorder_command};
use crate::recording::recovery::RecoveryOutcome;
use crate::storage;

/// The bar widget's state file, relative to `XDG_RUNTIME_DIR`.
pub const STATUS_FILE: &str = "tidyshot-status.json";
/// The controls window width in logical pixels.
pub const CONTROLS_WIDTH: f64 = 380.0;
/// The controls window height in logical pixels.
pub const CONTROLS_HEIGHT: f64 = 72.0;
/// The margin between the controls and their display edge.
pub const CONTROLS_MARGIN: i64 = 24;
/// How long the loop waits for a command before refreshing the status.
pub const POLL: Duration = Duration::from_millis(250);

/// The directory the runtime state file lives in.
///
/// `XDG_RUNTIME_DIR` is what the bar widget reads. A session without one falls
/// back to the data directory, so the status still has a home for tests and
/// headless runs.
#[must_use]
pub fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(crate::settings::data_dir)
}

/// The absolute path of the bar widget's status file.
#[must_use]
pub fn status_path() -> PathBuf {
    runtime_dir().join(STATUS_FILE)
}

/// What the bar widget and the controls show for one take.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordingStatus {
    /// Whether a take is running, including while paused.
    pub recording: bool,
    /// Whether the take is suspended.
    pub paused: bool,
    /// Recorded wall-clock seconds, excluding paused intervals.
    pub seconds: u64,
    /// Whether the widget shows the elapsed time.
    pub show_time: bool,
    /// Whether the always-on-top controls should be visible.
    pub show_controls: bool,
    /// Unix seconds when this snapshot was written.
    pub updated: f64,
    /// Where the controls window belongs.
    pub x: i32,
    pub y: i32,
    /// The take's output path once one exists.
    pub path: Option<PathBuf>,
    /// A status or failure line for the controls.
    pub message: String,
}

impl RecordingStatus {
    /// The `MM:SS` line the controls show.
    #[must_use]
    pub fn elapsed_text(&self) -> String {
        format!("{:02}:{:02}", self.seconds / 60, self.seconds % 60)
    }

    /// The object the bar widget reads.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut object = BTreeMap::new();
        object.insert("recording".to_owned(), Value::Bool(self.recording));
        object.insert("paused".to_owned(), Value::Bool(self.paused));
        object.insert("seconds".to_owned(), Value::Number(self.seconds as f64));
        object.insert("show_time".to_owned(), Value::Bool(self.show_time));
        object.insert("show_controls".to_owned(), Value::Bool(self.show_controls));
        object.insert("updated".to_owned(), Value::Number(self.updated));
        if let Some(path) = &self.path {
            object.insert("path".to_owned(), Value::String(path.display().to_string()));
        }
        if !self.message.is_empty() {
            object.insert("message".to_owned(), Value::String(self.message.clone()));
        }
        Value::Object(object)
    }
}

/// Write one status snapshot atomically, so the widget never reads a torn file.
pub fn write_status(path: &Path, status: &RecordingStatus) -> io::Result<()> {
    storage::atomic_write_file(path, status.to_value().to_string_pretty().as_bytes())
}

/// Remove the status file once no take is running.
pub fn clear_status(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// A command the controls send to the running take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingCommand {
    /// Suspend or resume the encoder.
    TogglePause,
    /// Finish the take and keep it.
    Stop,
    /// Finish the take without keeping it and start another.
    Restart,
    /// Finish the take without keeping it.
    Cancel,
}

#[derive(Debug, Default)]
struct ControlState {
    commands: Vec<RecordingCommand>,
    status: RecordingStatus,
    closed: bool,
}

/// The shared command queue and last status between the controls and a take.
#[derive(Clone, Default)]
pub struct RecordingControl {
    inner: Arc<(Mutex<ControlState>, Condvar)>,
}

impl RecordingControl {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a command for the running take.
    pub fn send(&self, command: RecordingCommand) {
        let (lock, wake) = &*self.inner;
        let mut state = lock.lock().expect("recording control poisoned");
        if state.closed {
            return;
        }
        state.commands.push(command);
        wake.notify_all();
    }

    /// The latest published status.
    #[must_use]
    pub fn snapshot(&self) -> RecordingStatus {
        let (lock, _) = &*self.inner;
        lock.lock()
            .expect("recording control poisoned")
            .status
            .clone()
    }

    /// Publish a status snapshot.
    pub fn publish(&self, status: RecordingStatus) {
        let (lock, _) = &*self.inner;
        lock.lock().expect("recording control poisoned").status = status;
    }

    /// Wait for a command, or return `None` when `timeout` elapses.
    pub fn wait(&self, timeout: Duration) -> Option<RecordingCommand> {
        let (lock, wake) = &*self.inner;
        let mut state = lock.lock().expect("recording control poisoned");
        let deadline = Instant::now() + timeout;
        loop {
            if !state.commands.is_empty() {
                return Some(state.commands.remove(0));
            }
            if state.closed {
                return None;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let (next, _) = wake
                .wait_timeout(state, remaining)
                .expect("recording control poisoned");
            state = next;
        }
    }

    /// Stop accepting commands and drop any still queued.
    pub fn close(&self) {
        let (lock, wake) = &*self.inner;
        let mut state = lock.lock().expect("recording control poisoned");
        state.closed = true;
        state.commands.clear();
        wake.notify_all();
    }
}

/// Why a take could not start or finish.
#[derive(Debug)]
pub enum RecordingError {
    /// No display is available for the requested take.
    NoDisplay(String),
    /// Reading the compositor, the journal or the output failed.
    Io(String),
    /// A command failed.
    Command(String),
    /// The encoder stopped without a playable file.
    Encoder(String),
    /// The take produced no usable video.
    Invalid(String),
}

impl std::fmt::Display for RecordingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDisplay(message)
            | Self::Io(message)
            | Self::Command(message)
            | Self::Encoder(message)
            | Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for RecordingError {}

impl From<io::Error> for RecordingError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<crate::commands::CommandError> for RecordingError {
    fn from(error: crate::commands::CommandError) -> Self {
        Self::Command(error.to_string())
    }
}

/// A finished take on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordingResult {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub gif: bool,
    pub screen: Option<crate::overlay::ScreenRect>,
}

/// The plan for one take: where it writes, what it records and where the
/// controls go.
#[derive(Debug, Clone)]
pub struct RecordingPlan {
    /// The MP4 the encoder writes.
    pub encoder_path: PathBuf,
    /// The file the user ends up with, which is the GIF for a GIF take.
    pub output_path: PathBuf,
    pub rect: Option<(i64, i64, i64, i64)>,
    pub monitors: Vec<Monitor>,
    pub controls_bounds: (i64, i64, i64, i64),
    pub command: Vec<String>,
    pub options: RecordOptions,
}

/// The recording runtime shared by the GUI and the CLI.
pub struct RecordingRuntime<'a, R: CommandRunner> {
    pub runner: &'a R,
    pub root: PathBuf,
    pub native: PathBuf,
    pub filename_format: String,
    pub status_path: PathBuf,
}

impl<'a, R: CommandRunner> RecordingRuntime<'a, R> {
    /// Build a runtime over the production directories.
    pub fn new(runner: &'a R, root: impl Into<PathBuf>, native: impl Into<PathBuf>) -> Self {
        Self {
            runner,
            root: root.into(),
            native: native.into(),
            filename_format: crate::filenames::DEFAULT_FORMAT.to_owned(),
            status_path: status_path(),
        }
    }

    /// Plan one take from the live compositor.
    ///
    /// A region take records exactly the requested rectangle; every other take
    /// records the display under the pointer, chosen before the encoder starts
    /// so focus moving during the take cannot change it.
    pub fn plan(
        &self,
        options: RecordOptions,
        geometry: Option<&str>,
    ) -> Result<RecordingPlan, RecordingError> {
        let monitors_value = hypr(self.runner, "monitors")?;
        let monitors: Vec<Monitor> = monitors_value
            .as_array()
            .unwrap_or(&[])
            .iter()
            .filter_map(Monitor::from_json)
            .collect();
        if monitors.is_empty() {
            return Err(RecordingError::NoDisplay(
                "No display is available for recording".to_owned(),
            ));
        }
        let rect = match geometry {
            Some(text) => {
                let rect = crate::geometry::parse_geometry(text)
                    .map_err(|error| RecordingError::NoDisplay(error.to_string()))?;
                Some((
                    i64::from(rect.x),
                    i64::from(rect.y),
                    i64::from(rect.width),
                    i64::from(rect.height),
                ))
            }
            None => None,
        };
        let mut options = options;
        if rect.is_none() && options.capture_output.is_none() {
            let cursor = hypr(self.runner, "cursorpos")
                .ok()
                .map(|value| {
                    (
                        value.get("x").and_then(Value::as_f64).unwrap_or(0.0) as i64,
                        value.get("y").and_then(Value::as_f64).unwrap_or(0.0) as i64,
                    )
                })
                .unwrap_or((0, 0));
            options.capture_output = recording_output(&monitors, cursor);
        }
        let target_monitors = recording_monitors(&monitors, options.capture_output.as_deref())
            .map_err(|error| RecordingError::NoDisplay(error.to_string()))?;
        let controls = CleanCapture::new(&monitors, rect, options.cursor)
            .map(|capture| capture.controls_bounds)
            .or_else(|_| {
                crate::recording::display::capture_target(&monitors, rect)
                    .map(|target| target.map_or((0, 0, 0, 0), |target| target.bounds))
            })
            .map_err(|error| RecordingError::NoDisplay(error.to_string()))?;

        let (encoder_path, output_path, options) = self.take_paths(&options)?;
        let command = recorder_command(&encoder_path, rect, &options, None, &target_monitors)
            .map_err(|error| RecordingError::NoDisplay(error.to_string()))?;
        Ok(RecordingPlan {
            encoder_path,
            output_path,
            rect,
            monitors: target_monitors,
            controls_bounds: controls,
            command,
            options,
        })
    }

    /// Resolve the take's paths, applying the GIF profile when needed.
    fn take_paths(
        &self,
        options: &RecordOptions,
    ) -> Result<(PathBuf, PathBuf, RecordOptions), RecordingError> {
        std::fs::create_dir_all(crate::history::captures_dir(&self.root))?;
        let pattern = if self.filename_format.is_empty() {
            crate::filenames::DEFAULT_FORMAT
        } else {
            &self.filename_format
        };
        let stem =
            crate::filenames::format_name(pattern, &crate::filenames::NameOptions::default());
        let extension = if options.format == CaptureFormat::Gif {
            "gif"
        } else {
            "mp4"
        };
        let mut sequence = 2;
        let mut candidate = self
            .root
            .join("captures")
            .join(format!("{stem}.{extension}"));
        while candidate.exists()
            || candidate.with_extension("recording.json").exists()
            || candidate.with_extension("mp4").exists()
        {
            candidate = self
                .root
                .join("captures")
                .join(format!("{stem} ({sequence}).{extension}"));
            sequence += 1;
        }
        let encoder_path = candidate.with_extension("mp4");
        let mut options = options.clone();
        crate::recording::gif::apply_format(&mut options);
        Ok((encoder_path, candidate, options))
    }

    /// Run one take through `control`, publishing every status to `publish`.
    ///
    /// Returns `Ok(None)` when the user cancelled, and `Err` when the encoder
    /// failed. A failed take keeps its journal marker and Studio checkpoint so
    /// [`recover_on_start`] can salvage it.
    pub fn run(
        &self,
        options: RecordOptions,
        geometry: Option<&str>,
        control: &RecordingControl,
        publish: &mut dyn FnMut(&RecordingStatus),
    ) -> Result<Option<RecordingResult>, RecordingError> {
        let mut options = options;
        loop {
            let plan = self.plan(options.clone(), geometry)?;
            match self.run_plan(&plan, control, publish)? {
                TakeOutcome::Saved(result) => return Ok(Some(result)),
                TakeOutcome::Cancelled => return Ok(None),
                TakeOutcome::Restart => {
                    options = plan.options;
                }
            }
        }
    }

    fn run_plan(
        &self,
        plan: &RecordingPlan,
        control: &RecordingControl,
        publish: &mut dyn FnMut(&RecordingStatus),
    ) -> Result<TakeOutcome, RecordingError> {
        let workspace = Workspace::new(self.root.clone());
        workspace.ensure()?;
        let options_value = record_options_value(&plan.options);
        let mut journal =
            Journal::begin(workspace.clone(), &plan.encoder_path, options_value.clone())
                .map_err(|error| RecordingError::Io(error.to_string()))?;
        let (x, y) = controls_position(plan.controls_bounds);
        let mut status = RecordingStatus {
            recording: true,
            paused: false,
            seconds: 0,
            show_time: plan.options.show_time,
            show_controls: plan.options.show_controls,
            updated: now_seconds(),
            x,
            y,
            path: Some(plan.output_path.clone()),
            message: String::new(),
        };
        write_live_status(&self.status_path, &mut status, publish, control);

        let checkpoint = if plan.options.studio {
            match CheckpointScheduler::start(&plan.encoder_path, &options_value) {
                Ok(scheduler) => Some(scheduler),
                Err(error) => {
                    journal.complete();
                    return Err(RecordingError::Io(error.to_string()));
                }
            }
        } else {
            None
        };

        let mut spec = CommandSpec::new(
            plan.command
                .first()
                .cloned()
                .unwrap_or_else(|| "gpu-screen-recorder".to_owned()),
        )
        .args(plan.command.iter().skip(1).cloned());
        if let Some(lease) = journal.lease_fd() {
            use std::os::fd::AsRawFd as _;
            spec = spec.inherit_fd(lease.as_raw_fd());
        }
        let mut child = match self.runner.spawn(&spec) {
            Ok(child) => child,
            Err(error) => {
                journal.complete();
                if let Some(checkpoint) = checkpoint.as_ref() {
                    checkpoint.discard();
                }
                return Err(RecordingError::Command(error.to_string()));
            }
        };

        let started = Instant::now();
        let mut paused_total = Duration::ZERO;
        let mut paused_at: Option<Instant> = None;
        let mut last_checkpoint = Instant::now();
        // The widget treats a stale file as idle, so refresh it even while the
        // clock is frozen by a pause.
        let mut last_status_write = Instant::now();
        let mut outcome = LoopOutcome::Cancelled;
        let mut stopped_externally = false;

        loop {
            let command = control.wait(POLL);
            if let Some(command) = command {
                match command {
                    RecordingCommand::TogglePause => {
                        if status.paused {
                            child.resume()?;
                            if let Some(at) = paused_at.take() {
                                paused_total += at.elapsed();
                            }
                            status.paused = false;
                            update_phase(&mut journal, "recording");
                        } else {
                            child.pause()?;
                            paused_at = Some(Instant::now());
                            status.paused = true;
                            update_phase(&mut journal, "paused");
                        }
                    }
                    RecordingCommand::Stop => {
                        outcome = LoopOutcome::Stop;
                        break;
                    }
                    RecordingCommand::Cancel => {
                        outcome = LoopOutcome::Cancelled;
                        break;
                    }
                    RecordingCommand::Restart => {
                        outcome = LoopOutcome::Restart;
                        break;
                    }
                }
                publish_status(
                    &self.status_path,
                    control,
                    publish,
                    &mut status,
                    &started,
                    paused_total,
                    paused_at,
                );
            }
            let elapsed = elapsed_seconds(started, paused_total, paused_at);
            if elapsed > status.seconds || last_status_write.elapsed() >= Duration::from_secs(1) {
                status.seconds = elapsed;
                publish_status(
                    &self.status_path,
                    control,
                    publish,
                    &mut status,
                    &started,
                    paused_total,
                    paused_at,
                );
                last_status_write = Instant::now();
            }
            if let Some(checkpoint) = checkpoint.as_ref()
                && last_checkpoint.elapsed() >= crate::recording::checkpoint::CHECKPOINT_INTERVAL
            {
                let batch = checkpoint_batch(&status, plan);
                if let Err(error) = checkpoint.tick(&batch) {
                    status.message = error.to_string();
                }
                last_checkpoint = Instant::now();
            }
            match child.running() {
                Ok(true) => {}
                Ok(false) => {
                    stopped_externally = true;
                    break;
                }
                Err(error) => {
                    status.message = error.to_string();
                    stopped_externally = true;
                    break;
                }
            }
        }

        if stopped_externally && matches!(outcome, LoopOutcome::Cancelled) {
            // The encoder exited on its own. Keep the journal marker and
            // checkpoint so the next launch can recover the file.
            if let Some(checkpoint) = checkpoint.as_ref() {
                checkpoint.stop();
            }
            clear_status(&self.status_path);
            status.recording = false;
            status.message =
                "The recorder stopped unexpectedly. The take will be recovered next time TidyShot starts."
                    .to_owned();
            let message = status.message.clone();
            publish(&status);
            control.publish(status);
            return Err(RecordingError::Encoder(message));
        }

        match outcome {
            LoopOutcome::Cancelled => {
                let _ = child.stop();
                if let Some(checkpoint) = checkpoint.as_ref() {
                    checkpoint.discard();
                }
                journal.discard();
                // The discard marker exists only so a crash mid-cancel can
                // finish the deletion; this take is already gone.
                journal.complete();
                remove_take(&plan.encoder_path, &plan.output_path);
                clear_status(&self.status_path);
                status.recording = false;
                status.message = "Recording cancelled".to_owned();
                publish(&status);
                control.publish(status);
                Ok(TakeOutcome::Cancelled)
            }
            LoopOutcome::Restart => {
                let _ = child.stop();
                if let Some(checkpoint) = checkpoint.as_ref() {
                    checkpoint.discard();
                }
                journal.discard();
                journal.complete();
                remove_take(&plan.encoder_path, &plan.output_path);
                clear_status(&self.status_path);
                Ok(TakeOutcome::Restart)
            }
            LoopOutcome::Stop => {
                let _ = child.stop();
                let result = match self.finish(plan, &mut journal, checkpoint.as_ref(), &status) {
                    Ok(result) => result,
                    Err(error) => {
                        clear_status(&self.status_path);
                        status.recording = false;
                        status.message = error.to_string();
                        publish(&status);
                        control.publish(status);
                        return Err(error);
                    }
                };
                if let Some(checkpoint) = checkpoint.as_ref() {
                    checkpoint.stop();
                }
                journal.complete();
                clear_status(&self.status_path);
                status.recording = false;
                status.paused = false;
                status.path = Some(result.path.clone());
                status.message = format!("Saved {}", result.path.display());
                publish(&status);
                control.publish(status);
                Ok(TakeOutcome::Saved(result))
            }
        }
    }

    /// Convert, validate and commit one finished take.
    fn finish(
        &self,
        plan: &RecordingPlan,
        journal: &mut Journal,
        checkpoint: Option<&CheckpointScheduler>,
        status: &RecordingStatus,
    ) -> Result<RecordingResult, RecordingError> {
        let workspace = Workspace::new(self.root.clone());
        let output = if plan.options.format == CaptureFormat::Gif {
            convert_to_gif(
                self.runner,
                &plan.encoder_path,
                &plan.output_path,
                &plan.options,
            )?;
            std::fs::remove_file(&plan.encoder_path).ok();
            plan.output_path.clone()
        } else {
            plan.encoder_path.clone()
        };
        let (width, height) = match crate::media_import::validate_video(self.runner, &output) {
            Ok(size) => size,
            Err(error) => {
                // Leave the marker so the next launch recovers what it can.
                update_phase(journal, "recovering");
                return Err(RecordingError::Invalid(error.to_string()));
            }
        };
        let mut metadata = workspace.metadata(&plan.encoder_path);
        if let Value::Object(entries) = &mut metadata {
            entries.insert("kind".to_owned(), Value::String("video".to_owned()));
            entries.insert("created".to_owned(), Value::Number(now_seconds()));
            entries.insert("width".to_owned(), Value::Number(f64::from(width)));
            entries.insert("height".to_owned(), Value::Number(f64::from(height)));
            entries.insert("options".to_owned(), record_options_value(&plan.options));
            entries.insert(
                "recording_seconds".to_owned(),
                Value::Number(status.seconds as f64),
            );
        }
        workspace.atomic_json(&plan.output_path, &metadata)?;
        if plan.output_path != plan.encoder_path {
            let encoder_sidecar = plan.encoder_path.with_extension("json");
            let _ = std::fs::remove_file(encoder_sidecar);
        }
        if let Some(checkpoint) = checkpoint {
            checkpoint.stop();
            let _ = self.write_studio_sidecar(plan);
            checkpoint.discard();
        }
        crate::history::prepend(
            &self.root,
            &crate::history::HistoryRow::video(&output, width, height),
        )?;
        Ok(RecordingResult {
            path: output,
            width,
            height,
            gif: plan.options.format == CaptureFormat::Gif,
            screen: None,
        })
    }

    /// Commit the Studio metadata a checkpoint would have restored.
    fn write_studio_sidecar(&self, plan: &RecordingPlan) -> io::Result<()> {
        let sidecar = plan.encoder_path.with_extension("studio.json");
        let mut record = BTreeMap::new();
        record.insert("version".to_owned(), Value::Number(1.0));
        record.insert(
            "source_path".to_owned(),
            Value::String(plan.encoder_path.display().to_string()),
        );
        record.insert("cursor".to_owned(), Value::Array(Vec::new()));
        record.insert("events".to_owned(), Value::Array(Vec::new()));
        record.insert(
            "capture_options".to_owned(),
            record_options_value(&plan.options),
        );
        storage::atomic_write_file(
            &sidecar,
            Value::Object(record).to_string_pretty().as_bytes(),
        )
    }
}

enum TakeOutcome {
    Saved(RecordingResult),
    Cancelled,
    Restart,
}

/// What the wait loop saw before the take is finished or discarded.
enum LoopOutcome {
    Stop,
    Cancelled,
    Restart,
}

fn update_phase(journal: &mut Journal, phase: &str) {
    let mut update = ObjectUpdate::new();
    update.insert("phase", Value::String(phase.to_owned()));
    let _ = journal.update(update);
}

/// Publish the first status of a take and write the widget's file.
fn write_live_status(
    path: &Path,
    status: &mut RecordingStatus,
    publish: &mut dyn FnMut(&RecordingStatus),
    control: &RecordingControl,
) {
    status.updated = now_seconds();
    if let Err(error) = write_status(path, status) {
        status.message = error.to_string();
    }
    control.publish(status.clone());
    publish(status);
}

fn publish_status(
    path: &Path,
    control: &RecordingControl,
    publish: &mut dyn FnMut(&RecordingStatus),
    status: &mut RecordingStatus,
    started: &Instant,
    paused_total: Duration,
    paused_at: Option<Instant>,
) {
    status.seconds = elapsed_seconds(*started, paused_total, paused_at);
    status.updated = now_seconds();
    if let Err(error) = write_status(path, status) {
        status.message = error.to_string();
    }
    control.publish(status.clone());
    publish(status);
}

fn elapsed_seconds(started: Instant, paused_total: Duration, paused_at: Option<Instant>) -> u64 {
    let paused = paused_total + paused_at.map_or(Duration::ZERO, |at| at.elapsed());
    started.elapsed().saturating_sub(paused).as_secs()
}

fn checkpoint_batch(status: &RecordingStatus, plan: &RecordingPlan) -> Value {
    let mut record = BTreeMap::new();
    record.insert("cursor".to_owned(), Value::Array(Vec::new()));
    record.insert("events".to_owned(), Value::Array(Vec::new()));
    record.insert(
        "recording_seconds".to_owned(),
        Value::Number(status.seconds as f64),
    );
    record.insert(
        "capture_options".to_owned(),
        record_options_value(&plan.options),
    );
    Value::Object(record)
}

/// Where the controls sit: the bottom-right corner of their display.
#[must_use]
pub fn controls_position(bounds: (i64, i64, i64, i64)) -> (i32, i32) {
    let (x, y, width, height) = bounds;
    if width <= 0 || height <= 0 {
        return (0, 0);
    }
    let left = x + width - CONTROLS_WIDTH as i64 - CONTROLS_MARGIN;
    let top = y + height - CONTROLS_HEIGHT as i64 - CONTROLS_MARGIN;
    (left.max(x) as i32, top.max(y) as i32)
}

/// Remove a discarded take and every sidecar the runtime may have written.
fn remove_take(encoder_path: &Path, output_path: &Path) {
    let _ = std::fs::remove_file(encoder_path);
    let _ = std::fs::remove_file(encoder_path.with_extension("json"));
    let _ = std::fs::remove_file(encoder_path.with_extension("studio.json"));
    let _ = std::fs::remove_file(encoder_path.with_extension("studio.checkpoint"));
    let _ = std::fs::remove_file(encoder_path.with_extension("recording.lock"));
    if output_path != encoder_path {
        let _ = std::fs::remove_file(output_path);
        let _ = std::fs::remove_file(output_path.with_extension("json"));
    }
}

/// Convert a finished MP4 into the GIF the take asked for.
fn convert_to_gif<R: CommandRunner>(
    runner: &R,
    source: &Path,
    destination: &Path,
    options: &RecordOptions,
) -> Result<(), RecordingError> {
    let filters = if options.gif_width == 0 {
        Vec::new()
    } else {
        vec![format!("scale={}:-1:flags=lanczos", options.gif_width)]
    };
    let chain =
        crate::recording::gif::palette_chain(&filters, options.gif_quality, options.gif_optimize);
    let target = storage::AtomicFile::new(destination)?;
    let spec = CommandSpec::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(source.to_string_lossy().into_owned())
        .args(["-vf", &chain, "-loop", "0"])
        .arg(target.path().to_string_lossy().into_owned())
        .timeout(Duration::from_secs(86_400));
    runner.run(&spec)?;
    target.commit()?;
    Ok(())
}

/// Serialize the options a take carries into its journal and sidecar.
#[must_use]
pub fn record_options_value(options: &RecordOptions) -> Value {
    let mode = match options.mode {
        crate::recording::recorder::CaptureMode::Area => "area",
        crate::recording::recorder::CaptureMode::Window => "window",
        crate::recording::recorder::CaptureMode::Fullscreen => "fullscreen",
    };
    let format = if options.format == CaptureFormat::Gif {
        "gif"
    } else {
        "mp4"
    };
    let mut object = BTreeMap::new();
    object.insert("mode".to_owned(), Value::String(mode.to_owned()));
    object.insert("format".to_owned(), Value::String(format.to_owned()));
    object.insert("fps".to_owned(), Value::Number(f64::from(options.fps)));
    object.insert("quality".to_owned(), Value::String(options.quality.clone()));
    object.insert("size".to_owned(), Value::String(options.size.clone()));
    object.insert("scale_video".to_owned(), Value::Bool(options.scale_video));
    object.insert("system".to_owned(), Value::Bool(options.system));
    object.insert("microphone".to_owned(), Value::Bool(options.microphone));
    object.insert("mono".to_owned(), Value::Bool(options.mono));
    object.insert(
        "separate_audio".to_owned(),
        Value::Bool(options.separate_audio),
    );
    object.insert("cursor".to_owned(), Value::Bool(options.cursor));
    object.insert("studio".to_owned(), Value::Bool(options.studio));
    object.insert("show_time".to_owned(), Value::Bool(options.show_time));
    object.insert(
        "show_controls".to_owned(),
        Value::Bool(options.show_controls),
    );
    object.insert(
        "capture_output".to_owned(),
        options
            .capture_output
            .clone()
            .map_or(Value::Null, Value::String),
    );
    object.insert(
        "gif_width".to_owned(),
        Value::Number(f64::from(options.gif_width)),
    );
    object.insert(
        "gif_quality".to_owned(),
        Value::Number(f64::from(options.gif_quality)),
    );
    object.insert("gif_optimize".to_owned(), Value::Bool(options.gif_optimize));
    Value::Object(object)
}

/// Recover takes a crashed instance left behind.
///
/// Markers still leased by a live encoder are reported as pending and left
/// alone; a finished-but-uncommitted file is recovered with its Studio
/// checkpoint restored, and a cancelled take is discarded.
pub fn recover_on_start<R: CommandRunner>(root: &Path, runner: &R) -> io::Result<RecoveryOutcome> {
    let workspace = Workspace::new(root);
    let captures = workspace.captures();
    let mut markers = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&captures) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".recording.json") || name.ends_with(".discarding.json") {
                markers.push(entry.path());
            }
        }
    }
    markers.sort();
    let outcome = crate::recording::journal::recover_interrupted(&workspace, runner, &markers)?;
    for (path, _message) in &outcome.recovered {
        if let Ok(dimensions) = crate::media_import::validate_video(runner, Path::new(path)) {
            let row = crate::history::HistoryRow::video(path, dimensions.0, dimensions.1);
            crate::history::prepend(root, &row)?;
        }
    }
    Ok(outcome)
}

fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandError, CommandOutput, RunningCommand};
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeEncoder {
        stopped: bool,
        paused: bool,
    }

    struct FakeChild {
        state: Arc<Mutex<FakeEncoder>>,
        path: PathBuf,
    }

    impl RunningCommand for FakeChild {
        fn pid(&self) -> u32 {
            4242
        }

        fn pause(&mut self) -> Result<(), CommandError> {
            self.state.lock().unwrap().paused = true;
            Ok(())
        }

        fn resume(&mut self) -> Result<(), CommandError> {
            self.state.lock().unwrap().paused = false;
            Ok(())
        }

        fn running(&mut self) -> Result<bool, CommandError> {
            Ok(!self.state.lock().unwrap().stopped)
        }

        fn wait(&mut self) -> Result<CommandOutput, CommandError> {
            Ok(CommandOutput {
                status: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }

        fn stop(&mut self) -> Result<CommandOutput, CommandError> {
            self.state.lock().unwrap().stopped = true;
            std::fs::write(&self.path, b"fake video").unwrap();
            self.wait()
        }
    }

    #[derive(Default)]
    struct Fake {
        monitors: String,
        seen: Mutex<Vec<CommandSpec>>,
        state: Arc<Mutex<FakeEncoder>>,
        crash: bool,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                monitors: r#"[{"name":"DP-1","x":0,"y":0,"width":1920,"height":1080,"scale":1.0,"transform":0,"focused":true,"dpmsStatus":true}]"#.to_owned(),
                ..Self::default()
            }
        }

        fn seen(&self) -> Vec<CommandSpec> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl CommandRunner for Fake {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            self.seen.lock().unwrap().push(spec.clone());
            if spec.program == "hyprctl" {
                let text = match spec.args.as_slice() {
                    [flag, query] if flag == "-j" && query == "monitors" => self.monitors.clone(),
                    [flag, query] if flag == "-j" && query == "cursorpos" => {
                        r#"{"x":10,"y":10}"#.to_owned()
                    }
                    _ => "false".to_owned(),
                };
                return Ok(CommandOutput {
                    status: 0,
                    stdout: text.into_bytes(),
                    stderr: Vec::new(),
                });
            }
            if spec.program == "ffprobe" {
                return Ok(CommandOutput {
                    status: 0,
                    stdout: br#"{"streams":[{"width":640,"height":480,"codec_type":"video"}]}"#
                        .to_vec(),
                    stderr: Vec::new(),
                });
            }
            if spec.program == "ffmpeg" {
                if spec.args.iter().any(|arg| arg == "framehash") {
                    return Ok(CommandOutput {
                        status: 0,
                        stdout: b"0, 0, 0, 1, 1.000000, abcdef\n".to_vec(),
                        stderr: Vec::new(),
                    });
                }
                if let Some(target) = spec.args.last() {
                    let target = PathBuf::from(target);
                    if target.parent().is_some() {
                        std::fs::write(target, b"GIF89a").unwrap();
                    }
                }
                return Ok(CommandOutput {
                    status: 0,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                });
            }
            Ok(CommandOutput {
                status: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }

        fn spawn(&self, spec: &CommandSpec) -> Result<Box<dyn RunningCommand>, CommandError> {
            self.seen.lock().unwrap().push(spec.clone());
            let position = spec
                .args
                .iter()
                .position(|arg| arg == "-o")
                .expect("recorder command names its output");
            let path = PathBuf::from(&spec.args[position + 1]);
            if self.crash {
                std::fs::write(&path, b"partial").unwrap();
                self.state.lock().unwrap().stopped = true;
            } else {
                std::fs::write(&path, b"recording").unwrap();
            }
            Ok(Box::new(FakeChild {
                state: Arc::clone(&self.state),
                path,
            }))
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tidyshot-recording-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn runtime<'a>(runner: &'a Fake, root: &Path) -> RecordingRuntime<'a, Fake> {
        RecordingRuntime {
            runner,
            root: root.to_path_buf(),
            native: root.join("native"),
            filename_format: "recording %S".to_owned(),
            status_path: root.join(STATUS_FILE),
        }
    }

    #[test]
    fn a_region_take_records_the_chosen_rectangle_and_commits_it() {
        let root = scratch("region");
        let runner = Fake::new();
        let control = RecordingControl::new();
        control.send(RecordingCommand::Stop);
        let mut statuses = Vec::new();
        let mut status_file_seen = false;
        let status_file = root.join(STATUS_FILE);
        let result = runtime(&runner, &root)
            .run(
                RecordOptions::default(),
                Some("10,20 640x480"),
                &control,
                &mut |status| {
                    if !status_file_seen && let Ok(text) = std::fs::read_to_string(&status_file) {
                        let value = crate::json::parse(&text).unwrap();
                        assert_eq!(value.get("recording").and_then(Value::as_bool), Some(true));
                        status_file_seen = true;
                    }
                    statuses.push(status.clone());
                },
            )
            .unwrap()
            .expect("a stopped take is saved");
        assert_eq!((result.width, result.height), (640, 480));
        assert_eq!(
            result.path.extension().and_then(|ext| ext.to_str()),
            Some("mp4")
        );
        assert!(result.path.exists());
        let spawn = runner
            .seen()
            .into_iter()
            .find(|spec| spec.program == "gpu-screen-recorder")
            .expect("the encoder was started");
        assert!(spawn.args.contains(&"region".to_owned()));
        assert!(spawn.args.contains(&"640x480+10+20".to_owned()));
        assert!(!spawn.inherit_fds.is_empty(), "the encoder holds the lease");
        // The status file follows the recording and is removed on completion.
        assert!(status_file_seen, "the widget was fed while the take ran");
        assert!(!root.join(STATUS_FILE).exists());
        assert!(statuses.iter().any(|status| status.recording));
        assert!(statuses.last().is_some_and(|status| !status.recording));
        // The capture has a sidecar and a history row.
        let sidecar = std::fs::read_to_string(result.path.with_extension("json")).unwrap();
        assert!(sidecar.contains("\"kind\": \"video\""), "{sidecar}");
        let rows = crate::history::read(&root);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "video");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_bar_widget_status_carries_its_fields() {
        let status = RecordingStatus {
            recording: true,
            paused: true,
            seconds: 75,
            show_time: true,
            show_controls: true,
            updated: 42.0,
            x: 1,
            y: 2,
            path: Some(PathBuf::from("/tmp/take.mp4")),
            message: String::new(),
        };
        let value = status.to_value();
        assert_eq!(value.get("recording").and_then(Value::as_bool), Some(true));
        assert_eq!(value.get("paused").and_then(Value::as_bool), Some(true));
        assert_eq!(value.get("seconds").and_then(Value::as_f64), Some(75.0));
        assert_eq!(value.get("show_time").and_then(Value::as_bool), Some(true));
        assert_eq!(
            value.get("show_controls").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(value.get("updated").and_then(Value::as_f64), Some(42.0));
        assert_eq!(status.elapsed_text(), "01:15");
    }

    #[test]
    fn a_cancelled_take_is_removed_and_leaves_no_status() {
        let root = scratch("cancel");
        let runner = Fake::new();
        let control = RecordingControl::new();
        control.send(RecordingCommand::Cancel);
        let result = runtime(&runner, &root)
            .run(
                RecordOptions::default(),
                Some("0,0 100x100"),
                &control,
                &mut |_| {},
            )
            .unwrap();
        assert!(result.is_none());
        assert!(!root.join(STATUS_FILE).exists());
        assert!(crate::history::read(&root).is_empty());
        let captures = crate::history::captures_dir(&root);
        let files: Vec<_> = std::fs::read_dir(&captures)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(files.is_empty(), "{files:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pause_freezes_the_elapsed_clock_and_resumes_the_encoder() {
        let root = scratch("pause");
        let runner = Fake::new();
        let control = RecordingControl::new();
        control.send(RecordingCommand::TogglePause);
        control.send(RecordingCommand::TogglePause);
        control.send(RecordingCommand::Stop);
        let mut paused_states = Vec::new();
        let result = runtime(&runner, &root)
            .run(
                RecordOptions::default(),
                Some("0,0 320x200"),
                &control,
                &mut |status| paused_states.push(status.paused),
            )
            .unwrap()
            .unwrap();
        assert!(paused_states.contains(&true));
        assert_eq!(paused_states.last(), Some(&false));
        assert!(result.path.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_crashed_encoder_keeps_its_marker_for_recovery() {
        let root = scratch("crash");
        let runner = Fake {
            crash: true,
            ..Fake::new()
        };
        let control = RecordingControl::new();
        let error = runtime(&runner, &root)
            .run(
                RecordOptions::default(),
                Some("0,0 320x200"),
                &control,
                &mut |_| {},
            )
            .unwrap_err();
        assert!(error.to_string().contains("recovered"), "{error}");
        let captures = crate::history::captures_dir(&root);
        let markers: Vec<_> = std::fs::read_dir(&captures)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".recording.json")
            })
            .collect();
        assert_eq!(markers.len(), 1);
        // A restart recovers the take instead of throwing it away.
        let outcome = recover_on_start(&root, &runner).unwrap();
        assert_eq!(outcome.recovered.len(), 1);
        assert!(outcome.pending.is_empty());
        assert!(
            !markers[0].path().exists(),
            "a recovered take drops its marker"
        );
        let rows = crate::history::read(&root);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "video");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_gif_take_is_converted_and_the_intermediate_removed() {
        let root = scratch("gif");
        let runner = Fake::new();
        let control = RecordingControl::new();
        control.send(RecordingCommand::Stop);
        let options = RecordOptions {
            format: CaptureFormat::Gif,
            gif_width: 0,
            ..RecordOptions::default()
        };
        let result = runtime(&runner, &root)
            .run(options, Some("0,0 320x200"), &control, &mut |_| {})
            .unwrap()
            .unwrap();
        assert!(result.gif);
        assert_eq!(
            result.path.extension().and_then(|ext| ext.to_str()),
            Some("gif")
        );
        assert!(result.path.exists());
        assert!(!result.path.with_extension("mp4").exists());
        assert!(runner.seen().iter().any(|spec| spec.program == "ffmpeg"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_second_take_never_replaces_the_first() {
        let root = scratch("unique");
        let runner = Fake::new();
        let runtime = runtime(&runner, &root);
        let first_control = RecordingControl::new();
        first_control.send(RecordingCommand::Stop);
        let first = runtime
            .run(
                RecordOptions::default(),
                Some("0,0 100x100"),
                &first_control,
                &mut |_| {},
            )
            .unwrap()
            .unwrap();
        let second_control = RecordingControl::new();
        second_control.send(RecordingCommand::Stop);
        let second = runtime
            .run(
                RecordOptions::default(),
                Some("0,0 100x100"),
                &second_control,
                &mut |_| {},
            )
            .unwrap()
            .unwrap();
        assert_ne!(first.path, second.path);
        assert!(first.path.exists() && second.path.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn restart_starts_a_new_take_after_discarding_the_first() {
        let root = scratch("restart");
        let runner = Fake::new();
        let control = RecordingControl::new();
        control.send(RecordingCommand::Restart);
        control.send(RecordingCommand::Stop);
        let result = runtime(&runner, &root)
            .run(
                RecordOptions::default(),
                Some("0,0 100x100"),
                &control,
                &mut |_| {},
            )
            .unwrap()
            .unwrap();
        assert!(result.path.exists());
        let captures = crate::history::captures_dir(&root);
        let takes: Vec<_> = std::fs::read_dir(&captures)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "mp4"))
            .collect();
        assert_eq!(takes.len(), 1, "the first take was discarded");
        assert_eq!(crate::history::read(&root).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_crashed_studio_take_restores_its_checkpoint() {
        let root = scratch("studio-crash");
        let runner = Fake {
            crash: true,
            ..Fake::new()
        };
        let control = RecordingControl::new();
        let options = RecordOptions {
            studio: true,
            ..RecordOptions::default()
        };
        runtime(&runner, &root)
            .run(options, Some("0,0 320x200"), &control, &mut |_| {})
            .unwrap_err();
        let captures = crate::history::captures_dir(&root);
        let take = std::fs::read_dir(&captures)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == "mp4"))
            .expect("the crashed take left its file");
        let checkpoint = take.with_extension("studio.checkpoint");
        assert!(checkpoint.is_file(), "the Studio checkpoint survived");

        let outcome = recover_on_start(&root, &runner).unwrap();
        assert_eq!(outcome.recovered.len(), 1);
        let sidecar = take.with_extension("studio.json");
        assert!(sidecar.is_file(), "the checkpoint was restored");
        let restored = crate::json::parse(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
        assert_eq!(restored.get("version").and_then(Value::as_f64), Some(1.0));
        assert!(restored.get("capture_options").is_some());
        // The recovered take is a playable history row.
        assert_eq!(crate::history::read(&root)[0].kind, "video");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_paused_take_keeps_the_widget_status_fresh() {
        let root = scratch("paused-status");
        let runner = Fake::new();
        let control = RecordingControl::new();
        control.send(RecordingCommand::TogglePause);
        let status_file = root.join(STATUS_FILE);
        let reader = std::thread::spawn({
            let status_file = status_file.clone();
            move || {
                // Pause lands within one poll; the refresh then rewrites the
                // file while the elapsed clock is frozen.
                std::thread::sleep(Duration::from_millis(1_400));
                std::fs::read_to_string(&status_file).ok()
            }
        });
        std::thread::spawn({
            let control = control.clone();
            move || {
                std::thread::sleep(Duration::from_millis(1_600));
                control.send(RecordingCommand::Stop);
            }
        });
        let result = runtime(&runner, &root)
            .run(
                RecordOptions::default(),
                Some("0,0 320x200"),
                &control,
                &mut |_| {},
            )
            .unwrap()
            .expect("the take was stopped");
        assert!(result.path.exists());
        let text = reader.join().unwrap().expect("the status file existed");
        let value = crate::json::parse(&text).unwrap();
        assert_eq!(value.get("recording").and_then(Value::as_bool), Some(true));
        assert_eq!(value.get("paused").and_then(Value::as_bool), Some(true));
        assert!(
            !status_file.exists(),
            "the status is cleared after the take"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn controls_sit_inside_the_chosen_display() {
        let (x, y) = controls_position((1920, 0, 2560, 1440));
        assert_eq!(
            x,
            1920 + 2560 - CONTROLS_WIDTH as i32 - CONTROLS_MARGIN as i32
        );
        assert_eq!(y, 1440 - CONTROLS_HEIGHT as i32 - CONTROLS_MARGIN as i32);
        assert_eq!(controls_position((0, 0, 0, 0)), (0, 0));
    }
}

// --- QML bridge: the Recording singleton ---------------------------------

/// A recording command that arrived before the QML singleton existed.
#[derive(Debug, Clone, PartialEq)]
enum PendingRecording {
    Start(Option<String>),
    Pause,
    Stop,
    Restart,
    Cancel,
}

/// The Qt-side recording object, once QML has created it.
static BOUND: std::sync::OnceLock<cxx_qt::CxxQtThread<qobject::Recording>> =
    std::sync::OnceLock::new();

/// Commands that arrived before the recording object existed.
static PENDING: Mutex<Vec<PendingRecording>> = Mutex::new(Vec::new());

/// Start a take from a forwarded command or the command line.
///
/// `geometry` is an explicit `x,y WxH` region; `None` records the display
/// under the pointer. The request waits for QML when the window is not up yet.
pub fn request_start(geometry: Option<String>) {
    enqueue(PendingRecording::Start(geometry));
}

/// Suspend or resume the active take.
pub fn request_pause() {
    enqueue(PendingRecording::Pause);
}

/// Finish the active take and keep it.
pub fn request_stop() {
    enqueue(PendingRecording::Stop);
}

/// Finish the active take without keeping it and start another.
pub fn request_restart() {
    enqueue(PendingRecording::Restart);
}

/// Abandon the active take.
pub fn request_cancel() {
    enqueue(PendingRecording::Cancel);
}

fn enqueue(command: PendingRecording) {
    match BOUND.get() {
        Some(thread) => {
            let _ = thread.queue(move |mut recording| {
                recording.as_mut().apply_command(command);
            });
        }
        None => PENDING
            .lock()
            .expect("pending recording queue poisoned")
            .push(command),
    }
}

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        /// The recording controls and status QML drives.
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(bool, open)]
        #[qproperty(bool, active)]
        #[qproperty(bool, paused)]
        #[qproperty(QString, elapsed)]
        #[qproperty(QString, message)]
        #[qproperty(bool, show_controls, cxx_name = "showControls")]
        #[qproperty(i32, x)]
        #[qproperty(i32, y)]
        #[qproperty(i32, window_width, cxx_name = "windowWidth")]
        #[qproperty(i32, window_height, cxx_name = "windowHeight")]
        type Recording = super::RecordingRust;

        /// Register this object and replay requests that arrived first.
        #[qinvokable]
        fn bind(self: Pin<&mut Self>);

        /// Start a take, optionally on an explicit `x,y WxH` region.
        #[qinvokable]
        fn start(self: Pin<&mut Self>, geometry: &QString);

        /// Suspend or resume the active take.
        #[qinvokable]
        fn pause(self: Pin<&mut Self>);

        /// Finish the active take and keep it.
        #[qinvokable]
        fn stop(self: Pin<&mut Self>);

        /// Finish the active take without keeping it and start another.
        #[qinvokable]
        fn restart(self: Pin<&mut Self>);

        /// Abandon the active take.
        #[qinvokable]
        fn cancel(self: Pin<&mut Self>);
    }

    impl cxx_qt::Threading for Recording {}
}

use core::pin::Pin;
use cxx_qt::CxxQtType as _;
use cxx_qt::Threading as _;
use cxx_qt_lib::QString;

/// Rust state behind the QML `Recording` singleton.
pub struct RecordingRust {
    open: bool,
    active: bool,
    paused: bool,
    elapsed: QString,
    message: QString,
    show_controls: bool,
    x: i32,
    y: i32,
    window_width: i32,
    window_height: i32,
    control: Option<RecordingControl>,
}

impl Default for RecordingRust {
    fn default() -> Self {
        Self {
            open: false,
            active: false,
            paused: false,
            elapsed: QString::from("00:00"),
            message: QString::from(""),
            show_controls: true,
            x: 0,
            y: 0,
            window_width: CONTROLS_WIDTH as i32,
            window_height: CONTROLS_HEIGHT as i32,
            control: None,
        }
    }
}

impl qobject::Recording {
    /// Register this object and replay any queued request.
    pub fn bind(mut self: Pin<&mut Self>) {
        let queued = {
            let mut pending = PENDING.lock().expect("pending recording queue poisoned");
            std::mem::take(&mut *pending)
        };
        let _ = BOUND.set(self.qt_thread());
        for command in queued {
            self.as_mut().apply_command(command);
        }
    }

    /// Start a take with the configured options.
    pub fn start(mut self: Pin<&mut Self>, geometry: &QString) {
        self.as_mut().apply_command(PendingRecording::Start(
            Some(geometry.to_string()).filter(|value| !value.is_empty()),
        ));
    }

    /// Suspend or resume the active take.
    pub fn pause(mut self: Pin<&mut Self>) {
        self.as_mut().apply_command(PendingRecording::Pause);
    }

    /// Finish the active take and keep it.
    pub fn stop(mut self: Pin<&mut Self>) {
        self.as_mut().apply_command(PendingRecording::Stop);
    }

    /// Finish the active take without keeping it and start another.
    pub fn restart(mut self: Pin<&mut Self>) {
        self.as_mut().apply_command(PendingRecording::Restart);
    }

    /// Abandon the active take.
    pub fn cancel(mut self: Pin<&mut Self>) {
        self.as_mut().apply_command(PendingRecording::Cancel);
    }

    fn apply_command(mut self: Pin<&mut Self>, command: PendingRecording) {
        match command {
            PendingRecording::Start(geometry) => self.as_mut().begin(geometry),
            PendingRecording::Pause => match self.control.clone() {
                Some(control) => control.send(RecordingCommand::TogglePause),
                None => self.as_mut().report("No recording is running"),
            },
            PendingRecording::Stop => match self.control.clone() {
                Some(control) => control.send(RecordingCommand::Stop),
                None => self.as_mut().report("No recording is running"),
            },
            PendingRecording::Restart => match self.control.clone() {
                Some(control) => control.send(RecordingCommand::Restart),
                None => self.as_mut().report("No recording is running"),
            },
            PendingRecording::Cancel => match self.control.clone() {
                Some(control) => control.send(RecordingCommand::Cancel),
                None => self.as_mut().report("No recording is running"),
            },
        }
    }

    fn report(mut self: Pin<&mut Self>, message: &str) {
        self.as_mut().set_message(QString::from(message));
    }

    /// Start the worker that owns the encoder.
    fn begin(mut self: Pin<&mut Self>, geometry: Option<String>) {
        if *self.active() {
            self.as_mut().report("A recording is already in progress");
            return;
        }
        let control = RecordingControl::new();
        self.as_mut().rust_mut().get_mut().control = Some(control.clone());
        self.as_mut().set_open(true);
        self.as_mut().set_active(true);
        self.as_mut().set_paused(false);
        self.as_mut().set_elapsed(QString::from("00:00"));
        self.as_mut().set_message(QString::from("Recording"));
        let thread = self.qt_thread();
        let worker_control = control.clone();
        std::thread::spawn(move || {
            let runner = crate::commands::SystemRunner;
            let runtime = RecordingRuntime::new(
                &runner,
                crate::settings::data_dir(),
                crate::recording::native_dir(),
            );
            let options = configured_record_options();
            let mut publish = {
                let thread = thread.clone();
                move |status: &RecordingStatus| {
                    let status = status.clone();
                    let _ = thread.queue(move |mut recording| {
                        recording.as_mut().apply_status(&status);
                    });
                }
            };
            let completion =
                runtime.run(options, geometry.as_deref(), &worker_control, &mut publish);
            let _ = thread.queue(move |mut recording| {
                recording.as_mut().apply_completion(completion);
            });
        });
    }

    /// Apply one status snapshot from the worker.
    fn apply_status(mut self: Pin<&mut Self>, status: &RecordingStatus) {
        self.as_mut().set_active(status.recording);
        self.as_mut().set_open(status.recording);
        self.as_mut().set_paused(status.paused);
        self.as_mut()
            .set_elapsed(QString::from(status.elapsed_text()));
        self.as_mut().set_x(status.x);
        self.as_mut().set_y(status.y);
        if !status.message.is_empty() {
            self.as_mut()
                .set_message(QString::from(status.message.clone()));
        }
    }

    fn apply_completion(
        mut self: Pin<&mut Self>,
        completion: Result<Option<RecordingResult>, RecordingError>,
    ) {
        self.as_mut().rust_mut().get_mut().control = None;
        self.as_mut().set_active(false);
        self.as_mut().set_open(false);
        self.as_mut().set_paused(false);
        match completion {
            Ok(Some(result)) => {
                self.as_mut()
                    .set_message(QString::from(format!("Saved {}", result.path.display())));
                let screen = crate::commands::hypr(&crate::commands::SystemRunner, "monitors")
                    .ok()
                    .and_then(|value| {
                        value
                            .as_array()
                            .and_then(crate::preview::focused_monitor_screen)
                    });
                crate::preview::publish_capture(
                    result.path.clone(),
                    result.width,
                    result.height,
                    screen,
                );
            }
            Ok(None) => self
                .as_mut()
                .set_message(QString::from("Recording cancelled")),
            Err(error) => {
                let message = QString::from(error.to_string());
                self.as_mut().set_message(message);
            }
        }
    }
}

/// The recording options a take reads from settings, defaulting like the source.
#[must_use]
pub fn configured_record_options() -> RecordOptions {
    record_options_from(&crate::settings::stored_settings())
}

/// Build recording options from stored settings, ignoring unknown or bad values.
#[must_use]
pub fn record_options_from(settings: &BTreeMap<String, crate::json::Value>) -> RecordOptions {
    let mut options = RecordOptions::default();
    let string = |key: &str| settings.get(key).and_then(crate::json::Value::as_str);
    let number = |key: &str| settings.get(key).and_then(crate::json::Value::as_f64);
    let flag = |key: &str| settings.get(key).and_then(crate::json::Value::as_bool);
    if let Some(value) = string("recording_fps").and_then(|value| value.parse().ok()) {
        options.fps = value;
    }
    if let Some(value) = string("recording_quality") {
        options.quality = value.to_owned();
    }
    if let Some(value) = string("recording_size") {
        options.size = value.to_owned();
    }
    if let Some(value) = flag("recording_scale") {
        options.scale_video = value;
    }
    if let Some(value) = flag("recording_audio") {
        options.system = value;
    }
    if let Some(value) = flag("recording_microphone") {
        options.microphone = value;
    }
    if let Some(value) = flag("recording_mono") {
        options.mono = value;
    }
    if let Some(value) = flag("recording_separate_audio") {
        options.separate_audio = value;
    }
    if let Some(value) = flag("recording_cursor") {
        options.cursor = value;
    }
    if let Some(value) = flag("recording_studio") {
        options.studio = value;
    }
    if let Some(value) = flag("recording_show_controls") {
        options.show_controls = value;
    }
    if let Some(value) = flag("recording_show_time") {
        options.show_time = value;
    }
    if let Some(true) = flag("recording_gif") {
        options.format = CaptureFormat::Gif;
    }
    if let Some(value) = string("recording_format") {
        options.format = if value.eq_ignore_ascii_case("gif") {
            CaptureFormat::Gif
        } else {
            CaptureFormat::Mp4
        };
    }
    if let Some(value) = number("recording_gif_width") {
        options.gif_width = value.max(0.0) as u32;
    }
    if let Some(value) = number("recording_gif_quality") {
        options.gif_quality = value.clamp(1.0, 100.0) as u32;
    }
    if let Some(value) = flag("recording_gif_optimize") {
        options.gif_optimize = value;
    }
    if let Some(value) = string("recording_system_device") {
        options.system_device = Some(value.to_owned());
    }
    if let Some(value) = string("recording_microphone_device") {
        options.microphone_device = Some(value.to_owned());
    }
    options
}
