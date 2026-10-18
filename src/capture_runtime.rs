//! The capture runtime: grab pixels, save them atomically, record history.
// Ported from omnishot/app.py (capture dispatch) and omnishot/backend.py
// (grab, select_region, Store).
//!
//! One [`CaptureRuntime`] owns the loop the GUI and the CLI share: read the
//! live monitors, run [`crate::capture_session::run_capture`], answer a
//! selection request with slurp, then write the PNG and its history row. The
//! QML bridge in this module calls it from a worker thread; a command-line
//! capture calls it directly.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::app::Command;
use crate::capture::{Display, capture_target};
use crate::capture_session::{
    CaptureOptions, CaptureOutcome, CaptureRequest, CaptureRequestError, CaptureSession,
    GrabbedFrame, run_capture, slurp_region,
};
use crate::commands::{CommandRunner, hypr};
use crate::geometry::Rect;
use crate::history::{self, HistoryRow};
use crate::json::Value;
use crate::mirror::MirrorRegistry;

/// A completed capture on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureResult {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub rect: Option<Rect>,
}

/// Why a capture could not complete.
#[derive(Debug)]
pub enum RuntimeError {
    /// No output is connected, or the compositor reported none.
    NoDisplays,
    /// The user dismissed the selection.
    Cancelled,
    /// The mode has no runtime implementation yet.
    Unsupported(String),
    /// The capture session rejected the request.
    Capture(CaptureRequestError),
    /// A compositor query failed.
    Command(crate::commands::CommandError),
    /// Writing the capture or its history failed.
    Io(String),
    /// The produced pixels could not be read or composed.
    Image(String),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDisplays => f.write_str("No display is available for capture"),
            Self::Cancelled => f.write_str("Capture cancelled"),
            Self::Unsupported(message) => f.write_str(message),
            Self::Capture(error) => error.fmt(f),
            Self::Command(error) => error.fmt(f),
            Self::Io(message) => f.write_str(message),
            Self::Image(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<CaptureRequestError> for RuntimeError {
    fn from(error: CaptureRequestError) -> Self {
        match error {
            CaptureRequestError::Selection => Self::Cancelled,
            other => Self::Capture(other),
        }
    }
}

impl From<crate::commands::CommandError> for RuntimeError {
    fn from(error: crate::commands::CommandError) -> Self {
        Self::Command(error)
    }
}

impl From<std::io::Error> for RuntimeError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

/// One selectable window from `hyprctl -j clients`.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    pub title: String,
    pub class: String,
    pub rect: Rect,
    pub monitor: String,
    pub workspace: String,
}

impl WindowInfo {
    /// The line the picker shows for this window.
    #[must_use]
    pub fn label(&self) -> String {
        if self.title.is_empty() {
            self.class.clone()
        } else {
            format!("{} — {}", self.class, self.title)
        }
    }

    /// The detail line: monitor, workspace and size.
    #[must_use]
    pub fn detail(&self) -> String {
        format!(
            "{} · {} · {} × {}",
            self.monitor, self.workspace, self.rect.width, self.rect.height
        )
    }
}

/// The runtime configuration a capture runs with.
pub struct CaptureRuntime<'a, R: CommandRunner> {
    pub runner: &'a R,
    pub mirror: &'a MirrorRegistry,
    pub native: PathBuf,
    pub root: PathBuf,
    /// Per-run capture options from settings.
    pub options: CaptureOptions,
    /// Filename pattern for saved captures.
    pub filename_format: String,
    /// Default self-timer delay in seconds.
    pub default_delay: i64,
}

impl<'a, R: CommandRunner> CaptureRuntime<'a, R> {
    /// Create a runtime with the shared settings defaults.
    #[must_use]
    pub fn new(
        runner: &'a R,
        mirror: &'a MirrorRegistry,
        native: impl Into<PathBuf>,
        root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            runner,
            mirror,
            native: native.into(),
            root: root.into(),
            options: CaptureOptions {
                cursor: false,
                freeze: true,
            },
            filename_format: crate::filenames::DEFAULT_FORMAT.to_owned(),
            default_delay: 3,
        }
    }

    /// Resolve and run an application command, when it is a capture.
    ///
    /// `Ok(None)` means the command is not a capture and the caller keeps it.
    pub fn run_command(&self, command: &Command) -> Option<Result<CaptureResult, RuntimeError>> {
        match command {
            Command::Capture {
                command,
                geometry,
                delay,
                ..
            } => {
                let request = match CaptureRequest::resolve(
                    command,
                    geometry.as_deref(),
                    *delay,
                    self.default_delay,
                ) {
                    Ok(request) => request,
                    Err(error) => return Some(Err(error.into())),
                };
                Some(self.run(request, None))
            }
            _ => None,
        }
    }

    /// Run one resolved request, optionally named.
    ///
    /// `remembered` fills a `previous` capture whose request carries no area.
    /// A request that comes back asking for a selection runs slurp (with the
    /// client rectangles for a window capture) and is retried once with the
    /// chosen area.
    pub fn run(
        &self,
        request: CaptureRequest,
        remembered: Option<Rect>,
    ) -> Result<CaptureResult, RuntimeError> {
        self.run_with_cancel(request, remembered, || false)
    }

    /// Run one request with a cancellation hook.
    ///
    /// The hook is polled before every step, so a cancel while the overlay is
    /// open ends the capture as soon as the selector returns instead of
    /// saving the pixels it produced.
    pub fn run_with_cancel<F>(
        &self,
        mut request: CaptureRequest,
        remembered: Option<Rect>,
        mut should_cancel: F,
    ) -> Result<CaptureResult, RuntimeError>
    where
        F: FnMut() -> bool,
    {
        if request.mode == crate::capture_session::CaptureMode::Previous && request.rect.is_none() {
            request.rect = remembered.or_else(|| history::last_area(&self.root));
        }
        let monitors_value = hypr(self.runner, "monitors")?;
        let Some(monitors) = monitors_value.as_array() else {
            return Err(RuntimeError::NoDisplays);
        };
        if monitors.is_empty() {
            return Err(RuntimeError::NoDisplays);
        }
        let session = CaptureSession {
            runner: self.runner,
            mirror: self.mirror,
            native: self.native.clone(),
        };
        loop {
            if should_cancel() {
                return Err(RuntimeError::Cancelled);
            }
            let outcome = run_capture(
                &session,
                monitors,
                &request,
                self.options,
                |_| {},
                &mut should_cancel,
            )?;
            match outcome {
                CaptureOutcome::NeedsSelection => {
                    let rect = self.select_region(request.mode)?;
                    if should_cancel() {
                        return Err(RuntimeError::Cancelled);
                    }
                    request.rect = Some(rect);
                }
                CaptureOutcome::Scroll { .. } => {
                    return Err(RuntimeError::Unsupported(
                        "Scrolling capture is not wired yet".to_owned(),
                    ));
                }
                CaptureOutcome::Grabbed(frame) => return self.save_frame(&frame),
                CaptureOutcome::Frames(frames) => return self.save_frames(&frames),
            }
        }
    }

    /// Run the interactive selector, restricted to client rectangles for a
    /// window capture.
    fn select_region(
        &self,
        mode: crate::capture_session::CaptureMode,
    ) -> Result<Rect, RuntimeError> {
        let clients = if mode == crate::capture_session::CaptureMode::Window {
            self.windows()?
                .iter()
                .map(|window| window.rect)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        slurp_region(
            self.runner,
            mode == crate::capture_session::CaptureMode::Window,
            &clients,
        )
        .map_err(RuntimeError::from)
    }

    /// The visible windows from `hyprctl -j clients`.
    pub fn windows(&self) -> Result<Vec<WindowInfo>, RuntimeError> {
        let value = hypr(self.runner, "clients")?;
        Ok(windows_from(&value))
    }

    /// The active window's absolute rectangle, when there is one.
    pub fn active_window(&self) -> Result<Option<WindowInfo>, RuntimeError> {
        let value = hypr(self.runner, "activewindow")?;
        Ok(window_from(&value))
    }

    /// Save a grabbed frame and append its history row.
    fn save_frame(&self, frame: &GrabbedFrame) -> Result<CaptureResult, RuntimeError> {
        let (width, height) = crate::annotation::png_dimensions(&frame.png)
            .ok_or_else(|| RuntimeError::Image("the capture was not a PNG image".to_owned()))?;
        let path = self.capture_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::storage::atomic_write_file(&path, &frame.png)?;
        let row = HistoryRow::screenshot(path.clone(), width, height, frame.rect);
        history::prepend(&self.root, &row)?;
        Ok(CaptureResult {
            path,
            width,
            height,
            rect: frame.rect,
        })
    }

    /// Compose per-output frames into one desktop-sized PNG and save it.
    fn save_frames(&self, frames: &[GrabbedFrame]) -> Result<CaptureResult, RuntimeError> {
        let Some(first) = frames.first() else {
            return Err(RuntimeError::NoDisplays);
        };
        if frames.len() == 1 {
            return self.save_frame(first);
        }
        let mut bounds = first.rect.unwrap_or(Rect::new(0, 0, 0, 0));
        for frame in &frames[1..] {
            let rect = frame
                .rect
                .ok_or_else(|| RuntimeError::Image("a frame has no region".to_owned()))?;
            let right = bounds.right().max(rect.right());
            let bottom = bounds.bottom().max(rect.bottom());
            bounds = Rect::new(
                bounds.x.min(rect.x),
                bounds.y.min(rect.y),
                right - bounds.x.min(rect.x),
                bottom - bounds.y.min(rect.y),
            );
        }
        if bounds.width <= 0 || bounds.height <= 0 {
            return Err(RuntimeError::Image(
                "the captured frames have no area".to_owned(),
            ));
        }
        let mut canvas =
            crate::pngzip::Image::transparent(bounds.width as usize, bounds.height as usize);
        {
            let mut surface =
                crate::raster::Canvas::new(&mut canvas.data, canvas.width, canvas.height);
            for frame in frames {
                let rect = frame
                    .rect
                    .ok_or_else(|| RuntimeError::Image("a frame has no region".to_owned()))?;
                let image = crate::pngzip::decode_png(&frame.png)
                    .map_err(|error| RuntimeError::Image(error.to_string()))?;
                surface.draw_image(
                    &image.data,
                    image.width,
                    image.height,
                    i64::from(rect.x - bounds.x),
                    i64::from(rect.y - bounds.y),
                );
            }
        }
        let png = canvas.to_png();
        let (width, height) = (canvas.width as u32, canvas.height as u32);
        let path = self.capture_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::storage::atomic_write_file(&path, &png)?;
        let row = HistoryRow::screenshot(path.clone(), width, height, Some(bounds));
        history::prepend(&self.root, &row)?;
        Ok(CaptureResult {
            path,
            width,
            height,
            rect: Some(bounds),
        })
    }

    /// Choose a new capture path under the store, never replacing a file.
    ///
    /// The path is absolute: history and the preview both resolve it later,
    /// and a relative store would depend on the process working directory.
    fn capture_path(&self) -> Result<PathBuf, RuntimeError> {
        let directory = std::fs::create_dir_all(history::captures_dir(&self.root))
            .map(|()| {
                std::fs::canonicalize(history::captures_dir(&self.root))
                    .unwrap_or_else(|_| history::captures_dir(&self.root))
            })
            .map_err(|error| RuntimeError::Io(error.to_string()))?;
        let pattern = if self.filename_format.is_empty() {
            crate::filenames::DEFAULT_FORMAT
        } else {
            &self.filename_format
        };
        let stem =
            crate::filenames::format_name(pattern, &crate::filenames::NameOptions::default());
        let mut candidate = directory.join(format!("{stem}.png"));
        let mut sequence = 2;
        while candidate.exists() {
            candidate = directory.join(format!("{stem} ({sequence}).png"));
            sequence += 1;
        }
        Ok(candidate)
    }
}

/// Run a capture command with the production runner and shared store.
pub fn execute(command: &Command) -> Result<CaptureResult, RuntimeError> {
    let runner = crate::commands::SystemRunner;
    let runtime = CaptureRuntime::new(
        &runner,
        MirrorRegistry::global(),
        crate::recording::native_dir(),
        crate::settings::data_dir(),
    );
    runtime.run_command(command).unwrap_or_else(|| {
        Err(RuntimeError::Unsupported(
            "not a capture command".to_owned(),
        ))
    })
}

/// Run a capture command and save it, reporting the result on stdout.
///
/// This is the direct CLI path for an invocation that found no running
/// instance. It returns the process exit code.
pub fn cli(command: &Command) -> i32 {
    match execute(command) {
        Ok(result) => {
            println!("{}", result.path.display());
            0
        }
        Err(error) => {
            eprintln!("tidyshot: {error}");
            1
        }
    }
}

/// Build the capture requests a menu row can start.
///
/// The bridge exposes one invokable per mode; this maps the row id to the
/// request the runtime runs.
#[must_use]
pub fn request_for_id(id: &str, remembered: Option<Rect>) -> Option<CaptureRequest> {
    let mut request = CaptureRequest::resolve(id, None, None, 3).ok()?;
    if request.mode == crate::capture_session::CaptureMode::Previous && request.rect.is_none() {
        request.rect = remembered;
    }
    Some(request)
}

/// Parse `hyprctl -j clients` into the picker model.
// Ported from omnishot/backend.py (window list)
#[must_use]
pub fn windows_from(value: &Value) -> Vec<WindowInfo> {
    value
        .as_array()
        .unwrap_or(&[])
        .iter()
        .filter_map(window_from)
        .collect()
}

fn window_from(value: &Value) -> Option<WindowInfo> {
    if value
        .get("hidden")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || value
            .get("mapped")
            .and_then(Value::as_bool)
            .is_some_and(|mapped| !mapped)
    {
        return None;
    }
    let at = value.get("at").and_then(Value::as_array)?;
    let size = value.get("size").and_then(Value::as_array)?;
    if at.len() < 2 || size.len() < 2 {
        return None;
    }
    let x = at[0].as_f64()? as i32;
    let y = at[1].as_f64()? as i32;
    let width = size[0].as_f64()? as i32;
    let height = size[1].as_f64()? as i32;
    if width < 2 || height < 2 {
        return None;
    }
    let workspace = value
        .get("workspace")
        .and_then(|workspace| workspace.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    Some(WindowInfo {
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        class: value
            .get("class")
            .and_then(Value::as_str)
            .unwrap_or("Window")
            .to_owned(),
        rect: Rect::new(x, y, width, height),
        monitor: value
            .get("monitor")
            .and_then(Value::as_f64)
            .map_or_else(String::new, |monitor| monitor.to_string()),
        workspace: workspace.to_owned(),
    })
}

/// The display list for a `hyprctl -j monitors` reply.
#[must_use]
pub fn displays(value: &Value) -> Vec<Display> {
    crate::capture_session::displays_from_monitors(value.as_array().unwrap_or(&[]))
}

/// True when the display list has an output a capture can use.
#[must_use]
pub fn has_display(value: &Value) -> bool {
    !displays(value).is_empty()
}

/// Resolve a `capture_target` for a region, for callers that need the output.
pub fn target_name(value: &Value, rect: Option<Rect>) -> Result<Option<String>, RuntimeError> {
    let displays = displays(value);
    Ok(capture_target(&displays, rect)
        .map_err(CaptureRequestError::from)?
        .and_then(|target| target.name))
}

/// One capture at a time across every UI entry point.
///
/// The GUI reports busy immediately, but two rows pressed in the same event
/// turn could still start two workers; this gate keeps the second one from
/// launching a second slurp over the first.
static CAPTURE_ACTIVE: AtomicBool = AtomicBool::new(false);

struct CaptureGate;

impl CaptureGate {
    fn enter() -> bool {
        CAPTURE_ACTIVE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}

struct CaptureLease;

impl Drop for CaptureLease {
    fn drop(&mut self) {
        CAPTURE_ACTIVE.store(false, Ordering::SeqCst);
    }
}

/// The worker half of a capture: resolve the active window, then run.
fn capture_on_worker(
    mut request: CaptureRequest,
    active: bool,
    cancel: Arc<AtomicBool>,
) -> Completion {
    let runner = crate::commands::SystemRunner;
    let runtime = CaptureRuntime::new(
        &runner,
        MirrorRegistry::global(),
        crate::recording::native_dir(),
        crate::settings::data_dir(),
    );
    if active && request.rect.is_none() {
        match runtime.active_window() {
            Ok(Some(window)) => request.rect = Some(window.rect),
            Ok(None) => return Completion::Failed("No focused window to capture".to_owned()),
            Err(error) => return Completion::Failed(error.to_string()),
        }
    }
    if cancel.load(Ordering::SeqCst) {
        return Completion::Failed(RuntimeError::Cancelled.to_string());
    }
    match runtime.run_with_cancel(request, None, || cancel.load(Ordering::SeqCst)) {
        Ok(result) => Completion::Captured(result),
        Err(error) => Completion::Failed(error.to_string()),
    }
}

/// The Qt-side capture object, once QML has created it.
static BOUND: std::sync::OnceLock<cxx_qt::CxxQtThread<qobject::Capture>> =
    std::sync::OnceLock::new();

/// Commands that arrived before the capture object existed.
static PENDING: std::sync::Mutex<Vec<Command>> = std::sync::Mutex::new(Vec::new());

/// Hand a routed command to the GUI, or queue it until the GUI is ready.
///
/// The single-instance server runs on a worker thread, so a forwarded capture
/// is queued onto the Qt event loop rather than touching QML state directly.
pub fn dispatch(command: Command) {
    match BOUND.get() {
        Some(thread) => {
            let _ = thread.queue(move |mut capture| {
                capture.as_mut().run_app_command(command);
            });
        }
        None => PENDING
            .lock()
            .expect("pending command queue poisoned")
            .push(command),
    }
}

// --- QML bridge: the Capture singleton -----------------------------------

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    extern "RustQt" {
        /// The capture commands QML can start, with their progress and result.
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(bool, busy)]
        #[qproperty(QString, message)]
        #[qproperty(QString, path)]
        #[qproperty(i32, width)]
        #[qproperty(i32, height)]
        #[qproperty(bool, window_picker_open, cxx_name = "windowPickerOpen")]
        #[qproperty(QStringList, window_labels, cxx_name = "windowLabels")]
        #[qproperty(QStringList, window_details, cxx_name = "windowDetails")]
        type Capture = super::CaptureRust;

        /// Start an area capture.
        #[qinvokable]
        fn area(self: Pin<&mut Self>);

        /// Capture the currently focused window.
        #[qinvokable]
        #[cxx_name = "windowActive"]
        fn window_active(self: Pin<&mut Self>);

        /// Capture the focused display.
        #[qinvokable]
        fn fullscreen(self: Pin<&mut Self>);

        /// Capture the previously captured area.
        #[qinvokable]
        fn previous(self: Pin<&mut Self>);

        /// Capture the whole desktop.
        #[qinvokable]
        fn desktop(self: Pin<&mut Self>);

        /// Cancel a capture that is still choosing an area.
        #[qinvokable]
        fn cancel(self: Pin<&mut Self>);

        /// Start a capture for a menu row id; false when the row is not one.
        #[qinvokable]
        #[cxx_name = "captureRow"]
        fn capture_row(self: Pin<&mut Self>, id: &QString) -> bool;

        /// Load the visible window list for the picker.
        #[qinvokable]
        #[cxx_name = "openWindowPicker"]
        fn open_window_picker(self: Pin<&mut Self>);

        /// Close the picker without capturing.
        #[qinvokable]
        #[cxx_name = "closeWindowPicker"]
        fn close_window_picker(self: Pin<&mut Self>);

        /// Capture the window at `index` in the loaded picker list.
        #[qinvokable]
        #[cxx_name = "captureWindow"]
        fn capture_window(self: Pin<&mut Self>, index: i32);

        /// Clear the last result path and message.
        #[qinvokable]
        fn clear(self: Pin<&mut Self>);

        /// Register this object for commands forwarded by a running instance.
        #[qinvokable]
        fn bind(self: Pin<&mut Self>);

        /// Emitted when a capture has been saved.
        #[qsignal]
        #[cxx_name = "captured"]
        fn captured_signal(self: Pin<&mut Self>, path: &QString, width: i32, height: i32);

        /// Emitted when a capture failed or was cancelled.
        #[qsignal]
        #[cxx_name = "failed"]
        fn failed_signal(self: Pin<&mut Self>, message: &QString);

    }

    impl cxx_qt::Threading for Capture {}
}

use core::pin::Pin;
use cxx_qt::CxxQtType as _;
use cxx_qt::Threading as _;
use cxx_qt_lib::{QString, QStringList};

/// Rust state behind the QML `Capture` singleton.
pub struct CaptureRust {
    busy: bool,
    message: QString,
    path: QString,
    width: i32,
    height: i32,
    window_picker_open: bool,
    window_labels: QStringList,
    window_details: QStringList,
    cancel: Arc<AtomicBool>,
    windows: Vec<WindowInfo>,
}

impl Default for CaptureRust {
    fn default() -> Self {
        Self {
            busy: false,
            message: QString::from(""),
            path: QString::from(""),
            width: 0,
            height: 0,
            window_picker_open: false,
            window_labels: QStringList::default(),
            window_details: QStringList::default(),
            cancel: Arc::new(AtomicBool::new(false)),
            windows: Vec::new(),
        }
    }
}

/// What a worker thread hands back to the Qt thread.
enum Completion {
    Captured(CaptureResult),
    Failed(String),
}

impl qobject::Capture {
    /// Start an area capture.
    pub fn area(mut self: Pin<&mut Self>) {
        self.as_mut().start_request("area", None, false);
    }

    /// Capture the focused window.
    pub fn window_active(mut self: Pin<&mut Self>) {
        self.as_mut().start_request("window", None, true);
    }

    /// Capture the focused display.
    pub fn fullscreen(mut self: Pin<&mut Self>) {
        self.as_mut().start_request("fullscreen", None, false);
    }

    /// Capture the previously captured area.
    pub fn previous(mut self: Pin<&mut Self>) {
        self.as_mut().start_request("previous", None, false);
    }

    /// Capture the whole desktop.
    pub fn desktop(mut self: Pin<&mut Self>) {
        self.as_mut().start_request("desktop", None, false);
    }

    /// Cancel a choice that is still open.
    pub fn cancel(mut self: Pin<&mut Self>) {
        self.cancel.store(true, Ordering::SeqCst);
        self.as_mut().set_busy(false);
        self.as_mut()
            .set_message(QString::from("Capture cancelled"));
        self.as_mut()
            .failed_signal(&QString::from("Capture cancelled"));
    }

    /// Start a capture for a menu row id.
    pub fn capture_row(mut self: Pin<&mut Self>, id: &QString) -> bool {
        let id = id.to_string();
        match id.as_str() {
            "all-in-one" | "select" => self.area(),
            "area" => self.area(),
            "window" => self.open_window_picker(),
            "fullscreen" => self.fullscreen(),
            "desktop" => self.desktop(),
            "previous" => self.previous(),
            "timer" => self.as_mut().start_request("timer", None, false),
            "scroll" | "scroll-horizontal" => {
                self.as_mut()
                    .set_message(QString::from("Scrolling capture is not wired yet"));
                self.as_mut()
                    .failed_signal(&QString::from("Scrolling capture is not wired yet"));
            }
            "ocr" => return false,
            _ => return false,
        }
        true
    }

    /// Load the visible window list for the picker.
    pub fn open_window_picker(mut self: Pin<&mut Self>) {
        let runner = crate::commands::SystemRunner;
        let runtime = CaptureRuntime::new(
            &runner,
            MirrorRegistry::global(),
            crate::recording::native_dir(),
            crate::settings::data_dir(),
        );
        match runtime.windows() {
            Ok(windows) => {
                let mut labels = QStringList::default();
                let mut details = QStringList::default();
                for window in &windows {
                    labels.append(QString::from(window.label()));
                    details.append(QString::from(window.detail()));
                }
                let count = windows.len();
                self.as_mut().rust_mut().get_mut().windows = windows;
                self.as_mut().set_window_labels(labels);
                self.as_mut().set_window_details(details);
                self.as_mut().set_window_picker_open(count > 0);
                self.as_mut()
                    .set_message(QString::from(format!("Choose a window ({count} found)")));
                if count == 0 {
                    self.as_mut()
                        .set_message(QString::from("No window to capture"));
                    self.as_mut().set_window_picker_open(false);
                    self.as_mut()
                        .failed_signal(&QString::from("No window to capture"));
                }
            }
            Err(error) => {
                self.as_mut().set_message(QString::from(error.to_string()));
                self.as_mut()
                    .failed_signal(&QString::from(error.to_string()));
            }
        }
    }

    /// Close the picker without capturing.
    pub fn close_window_picker(mut self: Pin<&mut Self>) {
        self.as_mut().set_window_picker_open(false);
    }

    /// Capture the window at `index` in the loaded list.
    pub fn capture_window(mut self: Pin<&mut Self>, index: i32) {
        let index = index.max(0) as usize;
        let Some(window) = self.windows.get(index).cloned() else {
            self.as_mut()
                .fail("That window is no longer available".to_owned());
            return;
        };
        self.as_mut().set_window_picker_open(false);
        self.as_mut()
            .start_request("window", Some(window.rect), true);
    }

    /// Register this object for forwarded commands and run any queued ones.
    pub fn bind(mut self: Pin<&mut Self>) {
        let queued = {
            let mut pending = PENDING.lock().expect("pending command queue poisoned");
            std::mem::take(&mut *pending)
        };
        let _ = BOUND.set(self.qt_thread());
        for command in queued {
            self.as_mut().run_app_command(command);
        }
    }

    /// Run one routed application command through the runtime worker.
    fn run_app_command(mut self: Pin<&mut Self>, command: Command) {
        // Non-capture rows keep their own behaviour; the runtime only owns
        // the screenshot commands.
        if let Command::Capture {
            command,
            geometry,
            delay,
            ..
        } = command
        {
            self.as_mut().start_command(
                CaptureRequest::resolve(&command, geometry.as_deref(), delay, 3),
                command == "window",
            );
        }
    }

    /// Clear the last result path and message.
    pub fn clear(mut self: Pin<&mut Self>) {
        self.as_mut().set_path(QString::from(""));
        self.as_mut().set_message(QString::from(""));
        self.as_mut().set_width(0);
        self.as_mut().set_height(0);
    }

    fn start_request(
        self: Pin<&mut Self>,
        command: &'static str,
        rect: Option<Rect>,
        active: bool,
    ) {
        self.start_command(
            CaptureRequest::resolve(
                command,
                rect.map(crate::geometry::geometry_text).as_deref(),
                None,
                3,
            ),
            active,
        );
    }

    /// Start a resolved request, taking over `rect` from the active window.
    fn start_command(
        mut self: Pin<&mut Self>,
        request: Result<CaptureRequest, CaptureRequestError>,
        active: bool,
    ) {
        if *self.busy() {
            // The running capture owns the busy flag; report the refusal
            // without clearing its state.
            let message = "A capture is already in progress";
            self.as_mut().set_message(QString::from(message));
            self.as_mut().failed_signal(&QString::from(message));
            return;
        }
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                self.as_mut().fail(error.to_string());
                return;
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.as_mut().rust_mut().get_mut().cancel = cancel;
        let qt_thread = self.qt_thread();
        self.as_mut().set_busy(true);
        self.as_mut().set_message(QString::from("Capturing…"));
        std::thread::spawn(move || {
            if !CaptureGate::enter() {
                let _ = qt_thread.queue(move |mut qobject| {
                    qobject
                        .as_mut()
                        .fail("A capture is already in progress".to_owned());
                });
                return;
            }
            let completion = {
                let _lease = CaptureLease;
                capture_on_worker(request, active, worker_cancel)
            };
            let _ = qt_thread.queue(move |mut qobject| {
                qobject.as_mut().apply_completion(completion);
                qobject.as_mut().set_busy(false);
            });
        });
    }

    fn apply_completion(mut self: Pin<&mut Self>, completion: Completion) {
        match completion {
            Completion::Captured(result) => {
                let path = result.path.display().to_string();
                self.as_mut().set_path(QString::from(path.clone()));
                self.as_mut().set_width(result.width as i32);
                self.as_mut().set_height(result.height as i32);
                self.as_mut().set_message(QString::from(format!(
                    "Saved {} × {}",
                    result.width, result.height
                )));
                self.as_mut().captured_signal(
                    &QString::from(path),
                    result.width as i32,
                    result.height as i32,
                );
            }
            Completion::Failed(message) => self.as_mut().fail(message),
        }
    }

    fn fail(mut self: Pin<&mut Self>, message: String) {
        self.as_mut().set_message(QString::from(message.clone()));
        self.as_mut().set_path(QString::from(""));
        self.as_mut().set_width(0);
        self.as_mut().set_height(0);
        self.as_mut().set_busy(false);
        self.as_mut().failed_signal(&QString::from(message));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandError, CommandOutput, CommandSpec};
    use std::collections::VecDeque;
    use std::path::Path;
    use std::sync::Mutex;

    /// A runner that answers hyprctl from canned JSON and queues grim/slurp.
    #[derive(Default)]
    struct Fake {
        monitors: Mutex<String>,
        clients: Mutex<String>,
        active: Mutex<String>,
        results: Mutex<VecDeque<(String, Result<CommandOutput, CommandError>)>>,
        seen: Mutex<Vec<CommandSpec>>,
    }

    impl Fake {
        fn new(monitors: &str, clients: &str, active: &str) -> Self {
            Self {
                monitors: Mutex::new(monitors.to_owned()),
                clients: Mutex::new(clients.to_owned()),
                active: Mutex::new(active.to_owned()),
                ..Self::default()
            }
        }

        fn push(&self, program: &str, stdout: Vec<u8>) {
            self.results.lock().unwrap().push_back((
                program.to_owned(),
                Ok(CommandOutput {
                    status: 0,
                    stdout,
                    stderr: Vec::new(),
                }),
            ));
        }

        fn push_text(&self, program: &str, stdout: &str) {
            self.push(program, stdout.as_bytes().to_vec());
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
                    [flag, query] if flag == "-j" && query == "monitors" => {
                        self.monitors.lock().unwrap().clone()
                    }
                    [flag, query] if flag == "-j" && query == "clients" => {
                        self.clients.lock().unwrap().clone()
                    }
                    [flag, query] if flag == "-j" && query == "activewindow" => {
                        self.active.lock().unwrap().clone()
                    }
                    [flag, query] if flag == "-j" && query == "plugin list" => "[]".to_owned(),
                    [action, ..] if action == "repl" => "false".to_owned(),
                    _ => "ok".to_owned(),
                };
                return Ok(CommandOutput {
                    status: 0,
                    stdout: text.into_bytes(),
                    stderr: Vec::new(),
                });
            }
            let mut results = self.results.lock().unwrap();
            let position = results
                .iter()
                .position(|(program, _)| *program == spec.program)
                .unwrap_or_else(|| {
                    panic!("no queued {} command; saw {:?}", spec.program, spec.args)
                });
            results.remove(position).expect("checked position").1
        }
    }

    fn monitors() -> &'static str {
        r#"[{"name":"DP-1","x":0,"y":0,"width":1920,"height":1080,"scale":1.0,"transform":0,"focused":true,"dpmsStatus":true}]"#
    }

    fn clients() -> &'static str {
        r#"[
            {"title":"Editor","class":"code","at":[10,20],"size":[600,400],"monitor":0,"workspace":{"name":"1"},"mapped":true},
            {"title":"Browser","class":"firefox","at":[700,100],"size":[800,600],"monitor":0,"workspace":{"name":"1"},"mapped":true},
            {"title":"Hidden","class":"x","at":[0,0],"size":[100,100],"monitor":0,"workspace":{"name":"1"},"mapped":false}
        ]"#
    }

    fn active() -> &'static str {
        r#"{"title":"Editor","class":"code","at":[10,20],"size":[600,400],"monitor":0,"workspace":{"name":"1"},"mapped":true}"#
    }

    fn png(width: usize, height: usize, color: [u8; 4]) -> Vec<u8> {
        let data = color
            .iter()
            .copied()
            .cycle()
            .take(width * height * 4)
            .collect();
        crate::pngzip::Image::new(width, height, data).to_png()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tidyshot-runtime-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A native directory holding the mirror plugin files a grab expects.
    fn native_dir() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("tidyshot-runtime-native-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        for name in [crate::mirror::MIRROR_PLUGIN, crate::mirror::CAPTURE_PLUGIN] {
            std::fs::write(path.join(name), b"plugin").unwrap();
        }
        path
    }

    fn runtime<'a>(runner: &'a Fake, root: &Path) -> CaptureRuntime<'a, Fake> {
        CaptureRuntime {
            runner,
            mirror: Box::leak(Box::new(MirrorRegistry::with_arch("aarch64"))),
            native: native_dir(),
            root: root.to_path_buf(),
            options: CaptureOptions {
                cursor: false,
                freeze: true,
            },
            filename_format: "shot %S".to_owned(),
            default_delay: 0,
        }
    }

    fn request(command: &str) -> CaptureRequest {
        CaptureRequest::resolve(command, None, None, 0).unwrap()
    }

    #[test]
    fn fullscreen_saves_the_png_and_adds_a_history_row() {
        let root = scratch("fullscreen");
        let runner = Fake::new(monitors(), clients(), active());
        let image = png(64, 48, [10, 20, 30, 255]);
        runner.push("grim", image.clone());
        let result = runtime(&runner, &root)
            .run(request("fullscreen"), None)
            .unwrap();
        assert_eq!((result.width, result.height), (64, 48));
        assert!(result.path.starts_with(history::captures_dir(&root)));
        assert_eq!(std::fs::read(&result.path).unwrap(), image);
        let rows = history::read(&root);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, result.path);
        assert_eq!((rows[0].width, rows[0].height), (64, 48));
        assert_eq!(rows[0].rect, Some(Rect::new(0, 0, 1920, 1080)));
        let grim = runner
            .seen()
            .into_iter()
            .find(|s| s.program == "grim")
            .unwrap();
        assert!(grim.args.contains(&"-o".to_owned()));
        assert!(grim.args.contains(&"DP-1".to_owned()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_selection_request_runs_slurp_then_grabs_the_chosen_area() {
        let root = scratch("selection");
        let runner = Fake::new(monitors(), clients(), active());
        runner.push_text("slurp", "10,20 100x200\n");
        let image = png(100, 200, [1, 2, 3, 255]);
        runner.push("grim", image.clone());
        let result = runtime(&runner, &root).run(request("area"), None).unwrap();
        assert_eq!(result.rect, Some(Rect::new(10, 20, 100, 200)));
        assert_eq!((result.width, result.height), (100, 200));
        let seen = runner.seen();
        let slurp = seen.iter().find(|spec| spec.program == "slurp").unwrap();
        assert!(!slurp.args.contains(&"-r".to_owned()));
        let grim = seen.iter().find(|spec| spec.program == "grim").unwrap();
        assert!(grim.args.contains(&"10,20 100x200".to_owned()));
        assert_eq!(
            history::read(&root)[0].rect,
            Some(Rect::new(10, 20, 100, 200))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_window_capture_offers_the_client_rectangles_to_slurp() {
        let root = scratch("window");
        let runner = Fake::new(monitors(), clients(), active());
        runner.push_text("slurp", "700,100 800x600\n");
        runner.push("grim", png(800, 600, [5, 6, 7, 255]));
        let result = runtime(&runner, &root)
            .run(request("window"), None)
            .unwrap();
        assert_eq!(result.rect, Some(Rect::new(700, 100, 800, 600)));
        let seen = runner.seen();
        let slurp = seen.iter().find(|spec| spec.program == "slurp").unwrap();
        assert!(slurp.args.contains(&"-r".to_owned()));
        let rows = String::from_utf8(slurp.stdin.clone().unwrap_or_default()).unwrap();
        assert!(rows.contains("10,20 600x400"), "{rows}");
        assert!(rows.contains("700,100 800x600"), "{rows}");
        assert!(
            !rows.contains("0,0 100x100"),
            "hidden windows must not be offered: {rows}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_cancelled_selection_saves_nothing() {
        let root = scratch("cancel");
        let runner = Fake::new(monitors(), clients(), active());
        runner.push_text("slurp", "");
        let error = runtime(&runner, &root)
            .run(request("area"), None)
            .unwrap_err();
        assert!(matches!(error, RuntimeError::Cancelled), "{error}");
        assert_eq!(error.to_string(), "Capture cancelled");
        assert!(history::read(&root).is_empty());
        assert!(
            runner.seen().iter().all(|spec| spec.program != "grim"),
            "a cancelled selection must not grab pixels"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_cancel_before_the_grab_takes_no_pixels() {
        let root = scratch("cancel-first");
        let runner = Fake::new(monitors(), clients(), active());
        let error = runtime(&runner, &root)
            .run_with_cancel(request("fullscreen"), None, || true)
            .unwrap_err();
        assert!(matches!(error, RuntimeError::Cancelled), "{error}");
        assert!(
            runner
                .seen()
                .iter()
                .all(|spec| spec.program != "grim" && spec.program != "slurp"),
            "a cancelled capture must not reach the selector or grim"
        );
        assert!(history::read(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn previous_without_history_reports_that_an_area_is_needed() {
        let root = scratch("previous-empty");
        let runner = Fake::new(monitors(), clients(), active());
        let error = runtime(&runner, &root)
            .run(request("previous"), None)
            .unwrap_err();
        assert!(
            error.to_string().contains("Capture an area first"),
            "{error}"
        );
        assert!(history::read(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scrolling_capture_reports_the_unwired_state() {
        let root = scratch("scroll");
        let runner = Fake::new(monitors(), clients(), active());
        let error = runtime(&runner, &root)
            .run(request("scroll-horizontal"), None)
            .unwrap_err();
        assert!(matches!(error, RuntimeError::Unsupported(_)), "{error}");
        assert!(error.to_string().contains("not wired"), "{error}");
        assert!(history::read(&root).is_empty());
        assert!(runner.seen().iter().all(|spec| spec.program != "grim"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_empty_monitor_list_gives_the_no_display_message() {
        let root = scratch("empty");
        let runner = Fake::new("[]", clients(), active());
        let error = runtime(&runner, &root)
            .run(request("fullscreen"), None)
            .unwrap_err();
        assert!(matches!(error, RuntimeError::NoDisplays), "{error}");
        assert_eq!(error.to_string(), "No display is available for capture");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn previous_reuses_the_area_from_history() {
        let root = scratch("previous");
        history::write(
            &root,
            &[HistoryRow::screenshot(
                "/tmp/older.png",
                120,
                80,
                Some(Rect::new(30, 40, 120, 80)),
            )],
        )
        .unwrap();
        let runner = Fake::new(monitors(), clients(), active());
        runner.push("grim", png(120, 80, [9, 9, 9, 255]));
        let result = runtime(&runner, &root)
            .run(request("previous"), None)
            .unwrap();
        assert_eq!(result.rect, Some(Rect::new(30, 40, 120, 80)));
        let grim = runner
            .seen()
            .into_iter()
            .find(|s| s.program == "grim")
            .unwrap();
        assert!(grim.args.contains(&"30,40 120x80".to_owned()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_active_window_rectangle_is_read_from_the_compositor() {
        let runner = Fake::new(monitors(), clients(), active());
        let runtime = runtime(&runner, &std::env::temp_dir());
        let window = runtime.active_window().unwrap().unwrap();
        assert_eq!(window.rect, Rect::new(10, 20, 600, 400));
        assert_eq!(window.label(), "code — Editor");
    }

    #[test]
    fn windows_are_projected_for_the_picker() {
        let value = crate::json::parse(clients()).unwrap();
        let windows = windows_from(&value);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label(), "code — Editor");
        assert!(windows[0].detail().contains("1"));
        assert_eq!(windows[1].rect, Rect::new(700, 100, 800, 600));
    }

    #[test]
    fn capture_files_never_replace_an_existing_capture() {
        let root = scratch("unique");
        let runner = Fake::new(monitors(), clients(), active());
        runner.push("grim", png(20, 10, [1, 1, 1, 255]));
        runner.push("grim", png(20, 10, [2, 2, 2, 255]));
        let runtime = runtime(&runner, &root);
        let first = runtime.run(request("fullscreen"), None).unwrap();
        let second = runtime.run(request("fullscreen"), None).unwrap();
        assert_ne!(first.path, second.path);
        assert!(first.path.exists() && second.path.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn desktop_frames_are_composed_into_one_capture() {
        let root = scratch("desktop");
        let monitor_rows = r#"[
            {"name":"left","x":0,"y":0,"width":40,"height":30,"scale":1.0,"transform":0,"focused":true,"dpmsStatus":true},
            {"name":"right","x":40,"y":0,"width":30,"height":20,"scale":1.0,"transform":0,"focused":false,"dpmsStatus":true}
        ]"#;
        let runner = Fake::new(monitor_rows, clients(), active());
        runner.push("grim", png(40, 30, [255, 0, 0, 255]));
        runner.push("grim", png(30, 20, [0, 255, 0, 255]));
        let result = runtime(&runner, &root)
            .run(request("desktop"), None)
            .unwrap();
        assert_eq!((result.width, result.height), (70, 30));
        let bytes = std::fs::read(&result.path).unwrap();
        let image = crate::pngzip::decode_png(&bytes).unwrap();
        assert_eq!(image.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(image.pixel(45, 0), [0, 255, 0, 255]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
