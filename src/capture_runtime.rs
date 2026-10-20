//! The capture runtime: grab pixels, save them atomically, record history.
// Ported from omnishot/app.py (capture dispatch) and omnishot/backend.py
// (grab, select_region, Store).
//!
//! One [`CaptureRuntime`] owns the loop the GUI and the CLI share: read the
//! live monitors, run [`crate::capture_session::run_capture`], answer a
//! selection request with slurp, then write the PNG and its history row. The
//! QML bridge in this module calls it from a worker thread; a command-line
//! capture calls it directly.

use std::path::{Path, PathBuf};
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
use crate::scroll::{ScrollSession, SystemScrollCapture, SystemScrollInput};
use crate::scroll_runtime::{
    DriveMode, DriveOutcome, ScrollCommand, ScrollControl, ScrollPanel, ScrollStatus,
    drive as drive_scroll,
};

/// A completed capture on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureResult {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub rect: Option<Rect>,
    /// The focused monitor the preview anchors to, when one was resolved.
    pub screen: Option<crate::overlay::ScreenRect>,
    /// The composed picture to show, when it differs from the saved capture.
    pub display: Option<PathBuf>,
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
    /// The scrolling session stopped without an image worth saving.
    Scroll(String),
    /// The isolated window capture failed or the window vanished.
    Window(String),
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
            Self::Scroll(message) => f.write_str(message),
            Self::Window(message) => f.write_str(message),
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

/// Whether a headless scrolling capture can drive auto-scroll.
///
/// Without the helper the session still runs, but only manual scrolling could
/// add frames, which a CLI invocation has no way to do; the caller turns
/// auto-scroll off and reports that the stitch will be a single viewport.
fn scroll_helper_available(native: &std::path::Path) -> bool {
    native.join("scroll-helper").is_file()
}

/// An encoded scrolling capture.
struct Image {
    width: u32,
    height: u32,
    png: Vec<u8>,
}

/// Convert a stitched frame to a PNG, expanding RGB rows to RGBA once.
fn encode_frame(frame: &crate::stitch::FrameData) -> Result<Image, RuntimeError> {
    if frame.width == 0 || frame.height == 0 {
        return Err(RuntimeError::Image(
            "the scrolling capture has no area".to_owned(),
        ));
    }
    let pixels = frame.width * frame.height;
    let mut data = Vec::with_capacity(pixels * 4);
    match frame.channels {
        4 => data.extend_from_slice(&frame.data[..pixels * 4]),
        3 => {
            for pixel in frame.data[..pixels * 3].as_chunks::<3>().0 {
                data.extend_from_slice(pixel);
                data.push(255);
            }
        }
        channels => {
            return Err(RuntimeError::Image(format!(
                "the scrolling capture has {channels} channels"
            )));
        }
    }
    let png = crate::pngzip::Image::new(frame.width, frame.height, data).to_png();
    Ok(Image {
        width: frame.width as u32,
        height: frame.height as u32,
        png,
    })
}

/// The windows from `hyprctl -j clients`, projected for the picker.
///
/// The picker model, identity and focus ordering live in [`crate::window`];
/// this stays a thin alias so callers of the runtime keep one name.
// Ported from omnishot/backend.py (window list)
pub use crate::window::WindowInfo;

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
        // A window command with an explicit rectangle still isolates the
        // window that rectangle names, so a scripted or URL capture keeps the
        // same fidelity as the picker. A rectangle that names no window falls
        // through to the ordinary grab.
        if request.mode == crate::capture_session::CaptureMode::Window
            && let Some(rect) = request.rect
            && let Some(window) = self
                .windows()?
                .into_iter()
                .find(|window| window.rect == rect && !window.is_own_preview())
            && let Some(identity) = window.identity()
        {
            return self.capture_window(&window, &identity, self.options.cursor, false);
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
                    if request.mode == crate::capture_session::CaptureMode::Window {
                        // A window capture is not a rectangle grab: the list
                        // fallback chooses the window, then the export helper
                        // copies its own surface.
                        let Some(window) = self.select_window()? else {
                            return Err(RuntimeError::Cancelled);
                        };
                        if should_cancel() {
                            return Err(RuntimeError::Cancelled);
                        }
                        let identity = window.identity().ok_or_else(|| {
                            RuntimeError::Window("Invalid selected window".to_owned())
                        })?;
                        // The include-cursor setting applies to a window
                        // capture the same way it does to the hover path.
                        return self.capture_window(&window, &identity, self.options.cursor, false);
                    }
                    let rect = self.select_region(request.mode)?;
                    if should_cancel() {
                        return Err(RuntimeError::Cancelled);
                    }
                    request.rect = Some(rect);
                }
                CaptureOutcome::Scroll { rect, horizontal } => {
                    return self.run_scroll(request.mode, rect, horizontal, &mut should_cancel);
                }
                CaptureOutcome::Grabbed(frame) => return self.save_frame(&frame),
                CaptureOutcome::Frames(frames) => return self.save_frames(&frames),
            }
        }
    }

    /// Run a scrolling capture to completion headlessly.
    ///
    /// This is the CLI path: it selects an area when the request carries
    /// none, then grabs and stitches until the content ends, a row repeats or
    /// the megapixel ceiling is reached. The stitched image is saved exactly
    /// like a screenshot, so a headless capture is still reachable later.
    // Ported from omnishot/widgets.py (ScrollPanel start/finish)
    fn run_scroll<F>(
        &self,
        mode: crate::capture_session::CaptureMode,
        rect: Option<Rect>,
        horizontal: bool,
        should_cancel: &mut F,
    ) -> Result<CaptureResult, RuntimeError>
    where
        F: FnMut() -> bool,
    {
        let rect = match rect {
            Some(rect) => rect,
            None => self.select_region(mode)?,
        };
        if should_cancel() {
            return Err(RuntimeError::Cancelled);
        }
        let native = crate::scroll::native_dir().unwrap_or_else(|| self.native.clone());
        // A headless capture has no pointer to drive, so without the helper it
        // still grabs the first viewport manually; saying which mode ran keeps
        // the difference from a silent single screenshot visible.
        let autoscroll = scroll_helper_available(&native);
        let capture = std::sync::Arc::new(crate::scroll::SystemScrollCapture::new(
            self.runner,
            self.mirror,
            native.clone(),
        ));
        let input = Box::new(crate::scroll::SystemScrollInput {
            runner: self.runner,
            native,
        });
        let mut session = crate::scroll::ScrollSession::new(
            rect,
            horizontal,
            capture,
            input,
            Box::new(crate::scroll::NoUi),
        );
        let mut panel = crate::scroll_runtime::HeadlessPanel::default();
        let outcome = crate::scroll_runtime::drive(
            &mut session,
            &mut panel,
            crate::scroll_runtime::DriveMode::Headless { autoscroll },
            should_cancel,
        );
        match outcome {
            crate::scroll_runtime::DriveOutcome::Finished(frame, warning) => {
                if !autoscroll {
                    eprintln!(
                        "tidyshot: the scroll helper is missing, so auto-scroll is off. \
                         Run bin/native to build it."
                    );
                }
                if let Some(warning) = warning {
                    eprintln!("tidyshot: the scrolling capture stopped early: {warning}");
                }
                self.save_scroll(&frame, Some(rect))
            }
            crate::scroll_runtime::DriveOutcome::Cancelled => Err(RuntimeError::Cancelled),
            crate::scroll_runtime::DriveOutcome::Failed(message) => {
                Err(RuntimeError::Scroll(message))
            }
        }
    }

    /// Save a stitched scrolling capture and append its history row.
    ///
    /// The stitcher keeps RGB or RGBA rows; PNG output is RGBA8, so the one
    /// conversion happens here, at the save boundary, and every other stage
    /// works on the borrowed stitched buffer.
    pub fn save_scroll(
        &self,
        frame: &crate::stitch::FrameData,
        rect: Option<Rect>,
    ) -> Result<CaptureResult, RuntimeError> {
        let Image { width, height, png } = encode_frame(frame)?;
        let path = self.capture_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::storage::atomic_write_file(&path, &png)?;
        let row = HistoryRow::scroll(path.clone(), width, height, rect);
        history::prepend(&self.root, &row)?;
        Ok(CaptureResult {
            path,
            width,
            height,
            rect,
            screen: None,
            display: None,
        })
    }

    /// Run the interactive selector, restricted to client rectangles for a
    /// window capture.
    ///
    /// The rectangles are slurp's fallback list; when the user picks one, the
    /// window is captured through the export helper instead of cropping the
    /// desktop at that rectangle.
    fn select_region(
        &self,
        mode: crate::capture_session::CaptureMode,
    ) -> Result<Rect, RuntimeError> {
        if mode == crate::capture_session::CaptureMode::Window {
            return self
                .select_window()?
                .map(|window| window.rect)
                .ok_or(RuntimeError::Cancelled);
        }
        slurp_region(self.runner, false, &[]).map_err(RuntimeError::from)
    }

    /// Run the window list selector.
    ///
    /// This is the keyboard fallback: the interactive GUI offers the frozen
    /// hover picker, while a headless invocation has only slurp's list.
    // Ported from omnishot/backend.py (select_window)
    pub fn select_window(&self) -> Result<Option<WindowInfo>, RuntimeError> {
        let windows: Vec<WindowInfo> = self
            .windows()?
            .into_iter()
            .filter(|window| !window.is_own_preview())
            .collect();
        if windows.is_empty() {
            return Err(RuntimeError::Window(
                "There are no windows to capture".to_owned(),
            ));
        }
        let rows: String = windows
            .iter()
            .map(|window| {
                format!(
                    "{} {}\n",
                    crate::geometry::geometry_text(window.rect),
                    window.address
                )
            })
            .collect();
        let spec = crate::commands::CommandSpec::new("slurp")
            .args([
                "-r",
                "-f",
                "%l",
                "-b",
                "#00000000",
                "-c",
                "#ffffffff",
                "-w",
                "2",
            ])
            .stdin(rows.into_bytes())
            .timeout(std::time::Duration::from_secs(180));
        let output = match self.runner.run(&spec) {
            Ok(output) => output,
            Err(crate::commands::CommandError::Failure { .. }) => return Ok(None),
            Err(error) => return Err(RuntimeError::Command(error)),
        };
        let address = output.stdout_string().trim().to_owned();
        Ok(windows.into_iter().find(|window| window.address == address))
    }

    /// The visible windows from `hyprctl -j clients`.
    pub fn windows(&self) -> Result<Vec<WindowInfo>, RuntimeError> {
        let value = hypr(self.runner, "clients")?;
        Ok(windows_from(&value))
    }

    /// The active window's absolute rectangle, when there is one.
    pub fn active_window(&self) -> Result<Option<WindowInfo>, RuntimeError> {
        let value = hypr(self.runner, "activewindow")?;
        Ok(crate::window::windows_from(&Value::Array(vec![value]))
            .into_iter()
            .next())
    }

    /// Capture one window's own surface and save it.
    ///
    /// This is the fidelity path: the export helper copies the window without
    /// overlap or wallpaper, preserving alpha. The saved pixels keep that
    /// window's rectangle in history; the styling is a later, editable step.
    // Ported from omnishot/app.py (capture_window) and omnishot/backend.py
    // (grab_window)
    pub fn capture_window(
        &self,
        window: &WindowInfo,
        identity: &crate::window::WindowIdentity,
        cursor: bool,
        transparent: bool,
    ) -> Result<CaptureResult, RuntimeError> {
        if !crate::window::helper_available(&self.native) {
            return Err(RuntimeError::Window(
                crate::window::helper_missing_message().to_owned(),
            ));
        }
        let image = crate::window::capture(self.runner, &self.native, identity, cursor)
            .map_err(|error| RuntimeError::Window(error.to_string()))?;
        let png = image.to_png();
        let path = self.capture_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::storage::atomic_write_file(&path, &png)?;
        let row = HistoryRow::screenshot(
            path.clone(),
            image.width as u32,
            image.height as u32,
            Some(window.rect),
        );
        history::prepend(&self.root, &row)?;
        let width = image.width as u32;
        let height = image.height as u32;
        // The composition owns the window pixels, so the decoded frame moves
        // into it instead of being copied for the project.
        let display = if transparent {
            None
        } else {
            self.style_window_frame(&path, image, window)?
        };
        Ok(CaptureResult {
            path,
            width,
            height,
            rect: Some(window.rect),
            screen: None,
            display,
        })
    }

    /// Compose the editable shadow, wallpaper and rounded frame for a window.
    ///
    /// The raw isolated window stays the history capture; this writes the
    /// project and its preview beside it, so the framing can be reopened and
    /// edited. A styling failure is reported, never silently skipped.
    // Ported from omnishot/app.py (finished_capture) and
    // omnishot/backend.py (window_background)
    fn style_window_frame(
        &self,
        capture: &Path,
        image: crate::pngzip::Image,
        window: &WindowInfo,
    ) -> Result<Option<PathBuf>, RuntimeError> {
        self.style_window_frame_with(capture, image, window, &crate::settings::stored_settings())
    }

    /// Compose a window frame with an explicit settings map, for tests.
    fn style_window_frame_with(
        &self,
        capture: &Path,
        image: crate::pngzip::Image,
        window: &WindowInfo,
        settings: &std::collections::BTreeMap<String, Value>,
    ) -> Result<Option<PathBuf>, RuntimeError> {
        let shadow = settings
            .get("window_shadow")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let padding = settings
            .get("window_padding")
            .and_then(Value::as_f64)
            .map_or(crate::window::DEFAULT_PADDING, |value| value as i32);
        let wallpaper_enabled = settings
            .get("window_wallpaper")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let mut style = crate::window::hyprland_style(
            self.runner,
            &window.address,
            window.rect.width.max(1) as f64,
            window.fullscreen,
        );
        style.padding = padding;
        style.shadow = shadow;
        let mut background = style.background(image.width, window.rect.width);
        if wallpaper_enabled && background.color.alpha == 0 {
            let wallpaper = crate::wallpaper::WallpaperSettings::from_settings(settings);
            match crate::wallpaper::capture_wallpaper(self.runner, &wallpaper, &self.root) {
                Ok(image) => background = style.with_wallpaper(background, image),
                Err(error) => {
                    eprintln!("tidyshot: the wallpaper background is unavailable: {error}")
                }
            }
        }
        let project = crate::window_edit::project_for(image, Some(background));
        crate::window_edit::save_editable(&self.root, capture, &project)
            .map(Some)
            .map_err(|error| RuntimeError::Window(error.to_string()))
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
            screen: None,
            display: None,
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
            screen: None,
            display: None,
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
    let mut result = runtime.run_command(command).unwrap_or_else(|| {
        Err(RuntimeError::Unsupported(
            "not a capture command".to_owned(),
        ))
    })?;
    // The preview anchors to the focused monitor after the grab, so a
    // height change while capturing is reflected in the window position.
    result.screen = crate::commands::hypr(&runner, "monitors")
        .ok()
        .and_then(|value| {
            value
                .as_array()
                .and_then(crate::preview::focused_monitor_screen)
        });
    Ok(result)
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
#[must_use]
pub fn windows_from(value: &Value) -> Vec<WindowInfo> {
    crate::window::windows_from(value)
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
fn capture_on_worker(request: CaptureRequest, active: bool, cancel: Arc<AtomicBool>) -> Completion {
    let runner = crate::commands::SystemRunner;
    let runtime = CaptureRuntime::new(
        &runner,
        MirrorRegistry::global(),
        crate::recording::native_dir(),
        crate::settings::data_dir(),
    );
    // An active-window capture copies the focused window's own surface too;
    // the rectangle is only recorded in history.
    let focused = if active && request.rect.is_none() {
        match runtime.active_window() {
            Ok(Some(window)) => Some(window),
            Ok(None) => return Completion::Failed("No focused window to capture".to_owned()),
            Err(error) => return Completion::Failed(error.to_string()),
        }
    } else {
        None
    };
    if let Some(window) = focused
        && let Some(identity) = window.identity()
    {
        if cancel.load(Ordering::SeqCst) {
            return Completion::Failed(RuntimeError::Cancelled.to_string());
        }
        return match runtime.capture_window(&window, &identity, runtime.options.cursor, false) {
            Ok(result) => Completion::Captured(result),
            Err(error) => Completion::Failed(error.to_string()),
        };
    }
    if cancel.load(Ordering::SeqCst) {
        return Completion::Failed(RuntimeError::Cancelled.to_string());
    }
    match runtime.run_with_cancel(request, None, || cancel.load(Ordering::SeqCst)) {
        Ok(mut result) => {
            result.screen = crate::commands::hypr(&runner, "monitors")
                .ok()
                .and_then(|value| value.as_array().map(crate::preview::focused_monitor_screen))
                .flatten();
            Completion::Captured(result)
        }
        Err(error) => Completion::Failed(error.to_string()),
    }
}

/// The worker half of an isolated window capture.
///
/// The window identity is re-resolved and the helper run on this thread; only
/// the finished path crosses back to the Qt thread. `transparent` and
/// `force_copy` are carried through so the capture keeps the user's modifier
/// choice for the following step.
// Ported from omnishot/app.py (capture_window)
fn window_on_worker(
    window: WindowInfo,
    identity: crate::window::WindowIdentity,
    transparent: bool,
    _force_copy: bool,
    cancel: Arc<AtomicBool>,
) -> Completion {
    if cancel.load(Ordering::SeqCst) {
        return Completion::Failed(RuntimeError::Cancelled.to_string());
    }
    let runner = crate::commands::SystemRunner;
    let runtime = CaptureRuntime::new(
        &runner,
        MirrorRegistry::global(),
        crate::recording::native_dir(),
        crate::settings::data_dir(),
    );
    match runtime.capture_window(&window, &identity, runtime.options.cursor, transparent) {
        Ok(mut result) => {
            result.screen = crate::commands::hypr(&runner, "monitors")
                .ok()
                .and_then(|value| value.as_array().map(crate::preview::focused_monitor_screen))
                .flatten();
            Completion::Captured(result)
        }
        Err(error) => Completion::Failed(error.to_string()),
    }
}

/// A panel that publishes the session to QML and reads the controls.
struct QtScrollPanel {
    control: ScrollControl,
    thread: cxx_qt::CxxQtThread<qobject::Capture>,
}

impl ScrollPanel for QtScrollPanel {
    fn publish(&mut self, status: &ScrollStatus) {
        let status = status.clone();
        self.control.publish(status.clone());
        let _ = self.thread.queue(move |mut capture| {
            capture.as_mut().apply_scroll_status(status);
        });
    }

    fn wait_command(&mut self, timeout: std::time::Duration) -> Option<ScrollCommand> {
        self.control.wait(timeout)
    }
}

/// The worker half of an interactive scrolling capture.
///
/// The region is chosen first with slurp, then the controls drive the session
/// through [`ScrollControl`]. The stitched image is saved on this thread when
/// Done lands, so the Qt thread only receives the finished path.
// Ported from omnishot/widgets.py (ScrollPanel start/tick/finish)
fn scroll_on_worker(
    rect: Option<Rect>,
    horizontal: bool,
    control: ScrollControl,
    thread: cxx_qt::CxxQtThread<qobject::Capture>,
    cancel: Arc<AtomicBool>,
) -> Completion {
    let runner = crate::commands::SystemRunner;
    let runtime = CaptureRuntime::new(
        &runner,
        MirrorRegistry::global(),
        crate::recording::native_dir(),
        crate::settings::data_dir(),
    );
    let rect = match rect {
        Some(rect) => rect,
        None => match crate::capture_session::slurp_region(&runner, false, &[]) {
            Ok(rect) => rect,
            Err(crate::capture_session::CaptureRequestError::Selection) => {
                return Completion::Failed(RuntimeError::Cancelled.to_string());
            }
            Err(error) => return Completion::Failed(error.to_string()),
        },
    };
    if cancel.load(Ordering::SeqCst) {
        return Completion::Failed(RuntimeError::Cancelled.to_string());
    }
    let native = crate::scroll::native_dir().unwrap_or_else(|| runtime.native.clone());
    let capture = Arc::new(SystemScrollCapture::new(
        &runner,
        MirrorRegistry::global(),
        native.clone(),
    ));
    let input = Box::new(SystemScrollInput {
        runner: &runner,
        native,
    });
    let mut session = ScrollSession::new(
        rect,
        horizontal,
        capture,
        input,
        Box::new(crate::scroll::NoUi),
    );
    let mut panel = QtScrollPanel { control, thread };
    match drive_scroll(
        &mut session,
        &mut panel,
        DriveMode::Interactive,
        &mut || cancel.load(Ordering::SeqCst),
    ) {
        DriveOutcome::Finished(frame, warning) => match runtime.save_scroll(&frame, Some(rect)) {
            Ok(result) => match warning {
                Some(reason) => Completion::CapturedPartial(result, reason),
                None => Completion::Captured(result),
            },
            Err(error) => Completion::Failed(error.to_string()),
        },
        DriveOutcome::Cancelled => Completion::Failed(RuntimeError::Cancelled.to_string()),
        DriveOutcome::Failed(message) => Completion::Failed(message),
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

/// A picker model prepared off the Qt thread.
struct PreparedPicker {
    windows: Vec<WindowInfo>,
    model: crate::window_picker::PickerModel,
    accent: String,
    directory: PathBuf,
}

/// Freeze every output and load the visible window list on a worker thread.
///
/// The grabs and compositor queries never touch the Qt event loop. A failed
/// query is returned with its message so the UI can show it.
fn prepare_window_picker() -> Result<PreparedPicker, String> {
    let runner = crate::commands::SystemRunner;
    let native = crate::recording::native_dir();
    // Refusing here names the missing build step before the desktop is
    // frozen, instead of offering a picker whose capture would always fail.
    if !crate::window::helper_available(&native) {
        return Err(crate::window::helper_missing_message().to_owned());
    }
    let runtime = CaptureRuntime::new(
        &runner,
        MirrorRegistry::global(),
        native.clone(),
        crate::settings::data_dir(),
    );
    let monitors = crate::commands::hypr(&runner, "monitors").map_err(|error| error.to_string())?;
    let clients = crate::commands::hypr(&runner, "clients").map_err(|error| error.to_string())?;
    let directory = runtime.root.join("frozen");
    let session = crate::capture_session::CaptureSession {
        runner: &runner,
        mirror: MirrorRegistry::global(),
        native,
    };
    let frames = crate::window_picker::freeze_outputs(&monitors, &directory, |display| {
        session
            .grab(
                None,
                runtime.options.cursor,
                Some(&display.name),
                Some(display.geometry.scale),
            )
            .map_err(|error| error.to_string())
    });
    let accent = crate::ui_scale::accent();
    let model = crate::window_picker::build(&clients, &monitors, frames, accent.clone());
    let mut windows: Vec<WindowInfo> = model
        .rows
        .iter()
        .filter(|row| !row.window.is_own_preview())
        .map(|row| row.window.clone())
        .collect();
    if windows.is_empty() {
        windows = runtime
            .windows()
            .unwrap_or_default()
            .into_iter()
            .filter(|window| !window.is_own_preview())
            .collect();
    }
    Ok(PreparedPicker {
        windows,
        model,
        accent,
        directory,
    })
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
        #[qproperty(bool, window_hover_open, cxx_name = "windowHoverOpen")]
        #[qproperty(QStringList, window_frames, cxx_name = "windowFrames")]
        #[qproperty(QStringList, window_rows, cxx_name = "windowRows")]
        #[qproperty(QString, window_accent, cxx_name = "windowAccent")]
        #[qproperty(QString, window_hint, cxx_name = "windowHint")]
        #[qproperty(QStringList, window_labels, cxx_name = "windowLabels")]
        #[qproperty(QStringList, window_details, cxx_name = "windowDetails")]
        #[qproperty(bool, scroll_open, cxx_name = "scrollOpen")]
        #[qproperty(QString, scroll_status, cxx_name = "scrollStatus")]
        #[qproperty(QString, scroll_start_label, cxx_name = "scrollStartLabel")]
        #[qproperty(i32, scroll_frames, cxx_name = "scrollFrames")]
        #[qproperty(bool, scroll_auto, cxx_name = "scrollAuto")]
        #[qproperty(bool, scroll_running, cxx_name = "scrollRunning")]
        #[qproperty(bool, scroll_horizontal, cxx_name = "scrollHorizontal")]
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

        /// Load the frozen frames and the visible window list for the picker.
        #[qinvokable]
        #[cxx_name = "openWindowPicker"]
        fn open_window_picker(self: Pin<&mut Self>);

        /// Close either picker without capturing.
        #[qinvokable]
        #[cxx_name = "closeWindowPicker"]
        fn close_window_picker(self: Pin<&mut Self>);

        /// Capture the row chosen in the frozen-frame hover picker.
        #[qinvokable]
        #[cxx_name = "captureWindowHover"]
        fn capture_window_hover(
            self: Pin<&mut Self>,
            index: i32,
            transparent: bool,
            force_copy: bool,
        );

        /// Capture the window at `index` in the loaded picker list.
        #[qinvokable]
        #[cxx_name = "captureWindow"]
        fn capture_window(self: Pin<&mut Self>, index: i32);

        /// Begin grabbing frames for the open scrolling capture.
        #[qinvokable]
        #[cxx_name = "scrollStart"]
        fn scroll_start(self: Pin<&mut Self>);

        /// Flip auto-scroll for the open scrolling capture.
        #[qinvokable]
        #[cxx_name = "scrollToggleAuto"]
        fn scroll_toggle_auto(self: Pin<&mut Self>);

        /// Finish the scrolling capture and save the stitched image.
        #[qinvokable]
        #[cxx_name = "scrollDone"]
        fn scroll_done(self: Pin<&mut Self>);

        /// Abandon the scrolling capture without saving.
        #[qinvokable]
        #[cxx_name = "scrollCancel"]
        fn scroll_cancel(self: Pin<&mut Self>);

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
    window_hover_open: bool,
    window_frames: QStringList,
    window_rows: QStringList,
    window_accent: QString,
    window_hint: QString,
    window_labels: QStringList,
    window_details: QStringList,
    scroll_open: bool,
    scroll_status: QString,
    scroll_start_label: QString,
    scroll_frames: i32,
    scroll_auto: bool,
    scroll_running: bool,
    scroll_horizontal: bool,
    cancel: Arc<AtomicBool>,
    windows: Vec<WindowInfo>,
    picker: Option<crate::window_picker::PickerModel>,
    frozen: Option<PathBuf>,
    scroll: Option<crate::scroll_runtime::ScrollControl>,
    pending_action: Option<String>,
    /// Ctrl was held when the window was chosen; copy after the capture.
    pending_copy: bool,
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
            window_hover_open: false,
            window_frames: QStringList::default(),
            window_rows: QStringList::default(),
            window_accent: QString::from("#FFD60A"),
            window_hint: QString::from(crate::window_picker::HINT),
            window_labels: QStringList::default(),
            window_details: QStringList::default(),
            scroll_open: false,
            scroll_status: QString::from(""),
            scroll_start_label: QString::from("Start Capture"),
            scroll_frames: 0,
            scroll_auto: false,
            scroll_running: false,
            scroll_horizontal: false,
            cancel: Arc::new(AtomicBool::new(false)),
            windows: Vec::new(),
            picker: None,
            frozen: None,
            scroll: None,
            pending_action: None,
            pending_copy: false,
        }
    }
}

/// What a worker thread hands back to the Qt thread.
enum Completion {
    Captured(CaptureResult),
    /// The stitched image is saved but a frame error stopped it short.
    CapturedPartial(CaptureResult, String),
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

    /// Start an interactive scrolling capture.
    ///
    /// A nil region runs slurp on the worker; the controls window then drives
    /// Start, auto-scroll, Done and Cancel until the stitch is saved.
    // Ported from omnishot/widgets.py (ScrollPanel)
    fn start_scroll(
        mut self: Pin<&mut Self>,
        rect: Option<Rect>,
        horizontal: bool,
        begin: bool,
        autoscroll: bool,
    ) {
        if *self.busy() {
            let message = "A capture is already in progress";
            self.as_mut().set_message(QString::from(message));
            self.as_mut().failed_signal(&QString::from(message));
            return;
        }
        let control = ScrollControl::new();
        if begin {
            control.send(ScrollCommand::Start);
        }
        if autoscroll {
            control.send(ScrollCommand::SetAuto(true));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.as_mut().rust_mut().get_mut().scroll = Some(control.clone());
        self.as_mut().rust_mut().get_mut().cancel = Arc::clone(&cancel);
        // The controls wait until the worker has a region; showing them over
        // an open slurp selector would put them in the selection.
        self.as_mut().set_scroll_open(false);
        self.as_mut()
            .set_scroll_status(QString::from("Choose an area to scroll"));
        self.as_mut()
            .set_scroll_start_label(QString::from("Start Capture"));
        self.as_mut().set_scroll_frames(0);
        self.as_mut().set_scroll_auto(false);
        self.as_mut().set_scroll_running(false);
        self.as_mut().set_scroll_horizontal(horizontal);
        self.as_mut().set_busy(true);
        self.as_mut()
            .set_message(QString::from("Choose an area to scroll"));
        let thread = self.qt_thread();
        let worker_thread = self.qt_thread();
        std::thread::spawn(move || {
            if !CaptureGate::enter() {
                let _ = worker_thread.queue(move |mut capture| {
                    capture
                        .as_mut()
                        .fail("A capture is already in progress".to_owned());
                });
                return;
            }
            let completion = {
                let _lease = CaptureLease;
                scroll_on_worker(rect, horizontal, control, thread, cancel)
            };
            let _ = worker_thread.queue(move |mut capture| {
                capture.as_mut().apply_completion(completion);
                capture.as_mut().set_busy(false);
                capture.as_mut().set_scroll_open(false);
                capture.as_mut().rust_mut().get_mut().scroll = None;
            });
        });
    }

    /// Publish one status snapshot from the scrolling worker.
    fn apply_scroll_status(mut self: Pin<&mut Self>, status: ScrollStatus) {
        self.as_mut().set_scroll_open(status.visible);
        self.as_mut()
            .set_scroll_status(QString::from(status.status.clone()));
        self.as_mut()
            .set_scroll_start_label(QString::from(status.start_label.clone()));
        self.as_mut().set_scroll_frames(status.frames as i32);
        self.as_mut().set_scroll_auto(status.auto);
        self.as_mut().set_scroll_running(status.running);
        self.as_mut().set_scroll_horizontal(status.horizontal);
        if status.visible || status.running {
            self.as_mut()
                .set_message(QString::from(if status.status.is_empty() {
                    "Scrolling capture".to_owned()
                } else {
                    status.status.clone()
                }));
        }
    }

    /// Begin grabbing frames for the open scrolling capture.
    pub fn scroll_start(mut self: Pin<&mut Self>) {
        self.as_mut().send_scroll(ScrollCommand::Start);
    }

    /// Flip auto-scroll for the open scrolling capture.
    pub fn scroll_toggle_auto(mut self: Pin<&mut Self>) {
        self.as_mut().send_scroll(ScrollCommand::ToggleAuto);
    }

    /// Finish the scrolling capture and save the stitched image.
    pub fn scroll_done(mut self: Pin<&mut Self>) {
        self.as_mut().send_scroll(ScrollCommand::Done);
    }

    /// Abandon the scrolling capture without saving.
    pub fn scroll_cancel(mut self: Pin<&mut Self>) {
        self.as_mut().send_scroll(ScrollCommand::Cancel);
    }

    fn send_scroll(mut self: Pin<&mut Self>, command: ScrollCommand) {
        match self.scroll.as_ref() {
            Some(control) => {
                control.send(command);
                if command == ScrollCommand::Cancel {
                    self.as_mut().set_scroll_open(false);
                }
            }
            None => {
                if command == ScrollCommand::Done {
                    return;
                }
                self.as_mut()
                    .set_message(QString::from("No scrolling capture is running"));
                self.as_mut()
                    .failed_signal(&QString::from("No scrolling capture is running"));
            }
        }
    }

    /// Cancel a choice that is still open.
    pub fn cancel(mut self: Pin<&mut Self>) {
        self.cancel.store(true, Ordering::SeqCst);
        if *self.window_hover_open() || *self.window_picker_open() {
            self.as_mut().close_window_picker();
        }
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
            "scroll" => self.as_mut().start_scroll(None, false, false, false),
            "scroll-horizontal" => self.as_mut().start_scroll(None, true, false, false),
            "ocr" => return false,
            _ => return false,
        }
        true
    }

    /// Freeze every output and load the visible window list.
    ///
    /// The grabs run on a worker thread so the menu keeps responding while a
    /// large desktop is copied; the finished model is applied on the Qt
    /// thread. The frozen-frame hover picker is the primary chooser; the list
    /// stays available as the keyboard fallback when the frames cannot be used.
    // Ported from omnishot/app.py (window_selection)
    pub fn open_window_picker(mut self: Pin<&mut Self>) {
        if *self.window_hover_open() || *self.window_picker_open() {
            return;
        }
        self.as_mut()
            .set_message(QString::from("Preparing the window picker…"));
        let qt_thread = self.qt_thread();
        std::thread::spawn(move || {
            let prepared = prepare_window_picker();
            let _ = qt_thread.queue(move |mut capture| {
                capture.as_mut().apply_window_picker(prepared);
            });
        });
    }

    /// Apply a picker prepared off the Qt thread.
    fn apply_window_picker(mut self: Pin<&mut Self>, prepared: Result<PreparedPicker, String>) {
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.as_mut().fail(error);
                return;
            }
        };
        let count = prepared.windows.len();
        if count == 0 {
            self.as_mut()
                .set_message(QString::from("No window to capture"));
            self.as_mut()
                .failed_signal(&QString::from("No window to capture"));
            return;
        }
        let mut labels = QStringList::default();
        let mut details = QStringList::default();
        for window in &prepared.windows {
            labels.append(QString::from(window.label()));
            details.append(QString::from(window.detail()));
        }
        let (frames, rows) = crate::window_picker::projections(&prepared.model);
        let mut frame_list = QStringList::default();
        for frame in frames {
            frame_list.append(QString::from(frame));
        }
        let mut row_list = QStringList::default();
        for row in rows {
            row_list.append(QString::from(row));
        }
        let hover = prepared.model.frozen_usable();
        {
            let rust = self.as_mut().rust_mut();
            let state = rust.get_mut();
            state.windows = prepared.windows;
            state.picker = Some(prepared.model);
            state.frozen = Some(prepared.directory);
        }
        self.as_mut().set_window_labels(labels);
        self.as_mut().set_window_details(details);
        self.as_mut().set_window_frames(frame_list);
        self.as_mut().set_window_rows(row_list);
        self.as_mut()
            .set_window_accent(QString::from(prepared.accent));
        self.as_mut().set_window_hover_open(hover);
        self.as_mut().set_window_picker_open(!hover);
        self.as_mut()
            .set_message(QString::from(format!("Choose a window ({count} found)")));
    }

    /// Close either picker without capturing.
    pub fn close_window_picker(mut self: Pin<&mut Self>) {
        self.as_mut().set_window_picker_open(false);
        self.as_mut().set_window_hover_open(false);
        self.as_mut().rust_mut().get_mut().picker = None;
        if let Some(directory) = self.as_mut().rust_mut().get_mut().frozen.take() {
            let _ = std::fs::remove_dir_all(directory);
        }
    }

    /// Capture the row chosen in the frozen-frame picker.
    pub fn capture_window_hover(
        mut self: Pin<&mut Self>,
        index: i32,
        transparent: bool,
        force_copy: bool,
    ) {
        let choice = {
            let rust = self.as_mut().rust_mut();
            let Some(model) = rust.get_mut().picker.as_mut() else {
                self.as_mut()
                    .fail("That window is no longer available".to_owned());
                return;
            };
            model.set_hovered(Some(index.max(0) as usize));
            model.choose(transparent, force_copy)
        };
        let Some(choice) = choice else {
            self.as_mut()
                .fail("That window is no longer available".to_owned());
            return;
        };
        let window = self
            .picker
            .as_ref()
            .and_then(|model| model.rows.get(choice.index))
            .map(|row| row.window.clone());
        let Some(window) = window else {
            self.as_mut()
                .fail("That window is no longer available".to_owned());
            return;
        };
        self.as_mut().close_window_picker();
        self.as_mut()
            .start_window(window, choice.transparent, choice.force_copy);
    }

    /// Capture the window at `index` in the loaded list.
    pub fn capture_window(mut self: Pin<&mut Self>, index: i32) {
        let index = index.max(0) as usize;
        let Some(window) = self.windows.get(index).cloned() else {
            self.as_mut()
                .fail("That window is no longer available".to_owned());
            return;
        };
        self.as_mut().close_window_picker();
        self.as_mut().start_window(window, false, false);
    }

    /// Run one isolated window capture on a worker thread.
    fn start_window(
        mut self: Pin<&mut Self>,
        window: WindowInfo,
        transparent: bool,
        force_copy: bool,
    ) {
        if *self.busy() {
            let message = "A capture is already in progress";
            self.as_mut().set_message(QString::from(message));
            self.as_mut().failed_signal(&QString::from(message));
            return;
        }
        let Some(identity) = window.identity() else {
            self.as_mut().fail("Invalid selected window".to_owned());
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        {
            let state = self.as_mut().rust_mut().get_mut();
            state.cancel = cancel;
            state.pending_copy = force_copy;
        }
        let qt_thread = self.qt_thread();
        self.as_mut().set_busy(true);
        self.as_mut()
            .set_message(QString::from("Capturing window…"));
        std::thread::spawn(move || {
            if !CaptureGate::enter() {
                let _ = qt_thread.queue(move |mut capture| {
                    capture
                        .as_mut()
                        .fail("A capture is already in progress".to_owned());
                });
                return;
            }
            let completion = {
                let _lease = CaptureLease;
                window_on_worker(window, identity, transparent, force_copy, worker_cancel)
            };
            let _ = qt_thread.queue(move |mut capture| {
                capture.as_mut().apply_completion(completion);
                capture.as_mut().set_busy(false);
            });
        });
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
        // `scroll-done` finishes whichever scrolling capture is open, which is
        // what the shortcut sends while the controls window has no focus.
        if command == Command::ScrollDone {
            self.as_mut().scroll_done();
            return;
        }
        // Non-capture rows keep their own behaviour; the runtime only owns
        // the screenshot commands.
        match command {
            Command::Capture {
                command,
                geometry,
                delay,
                start,
                autoscroll,
                action,
                ..
            } => {
                // A window command without a region opens the picker, the
                // same chooser the menu row opens, so a forwarded link and a
                // menu click cannot diverge.
                // Ported from omnishot/app.py (capture)
                if command == "window" && geometry.is_none() {
                    self.as_mut().open_window_picker();
                    return;
                }
                // Scrolling captures drive an interactive session rather than
                // the one-shot grab the other modes use.
                if command == "scroll" || command == "scroll-horizontal" {
                    let rect = geometry
                        .as_deref()
                        .and_then(|text| crate::geometry::parse_geometry(text).ok());
                    self.as_mut().start_scroll(
                        rect,
                        command == "scroll-horizontal",
                        start.unwrap_or(false),
                        autoscroll.unwrap_or(false),
                    );
                    return;
                }
                self.as_mut().start_command(
                    CaptureRequest::resolve(&command, geometry.as_deref(), delay, 3),
                    command == "window",
                    action,
                );
            }
            Command::Edit(path) => crate::editor_window::publish_open(path),
            Command::EditChooser => crate::editor_window::publish_chooser(),
            Command::Overlay(path) => crate::preview::publish_file(path),
            Command::Restore => {
                if let Some(path) = crate::history::newest_path(&crate::settings::data_dir()) {
                    crate::preview::publish_file(path);
                }
            }
            Command::CloseAll => crate::preview::publish_dismiss(),
            Command::SaveAll => crate::preview::publish_save(),
            _ => {}
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
            None,
        );
    }

    /// Start a resolved request, taking over `rect` from the active window.
    fn start_command(
        mut self: Pin<&mut Self>,
        request: Result<CaptureRequest, CaptureRequestError>,
        active: bool,
        action: Option<String>,
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
        {
            let state = self.as_mut().rust_mut().get_mut();
            state.pending_action = action;
            // A plain capture never carries a chooser modifier.
            state.pending_copy = false;
        }
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
                crate::preview::publish_capture_display(
                    result.path.clone(),
                    result.display.clone(),
                    result.width,
                    result.height,
                    result.screen,
                );
                self.as_mut()
                    .after_capture_action(&result.path, result.display.as_deref());
                self.as_mut().captured_signal(
                    &QString::from(path),
                    result.width as i32,
                    result.height as i32,
                );
            }
            Completion::CapturedPartial(result, reason) => {
                let path = result.path.display().to_string();
                self.as_mut().set_path(QString::from(path.clone()));
                self.as_mut().set_width(result.width as i32);
                self.as_mut().set_height(result.height as i32);
                self.as_mut().set_message(QString::from(format!(
                    "Saved {} × {} before the capture stopped: {reason}",
                    result.width, result.height
                )));
                crate::preview::publish_capture_display(
                    result.path.clone(),
                    result.display.clone(),
                    result.width,
                    result.height,
                    result.screen,
                );
                self.as_mut()
                    .after_capture_action(&result.path, result.display.as_deref());
                self.as_mut().captured_signal(
                    &QString::from(path),
                    result.width as i32,
                    result.height as i32,
                );
            }
            Completion::Failed(message) => self.as_mut().fail(message),
        }
    }

    /// Run the requested after-capture action.
    ///
    /// An explicit `annotate` from a shortcut or URL opens the editor on the
    /// saved file; the preview is still published so the capture stays
    /// reachable when the editor closes. Ctrl at selection time copies the
    /// picture the preview shows, which for a window capture is the composed
    /// frame, matching the source's `force_copy`.
    // Ported from omnishot/app.py (finished_capture force_copy)
    fn after_capture_action(
        mut self: Pin<&mut Self>,
        path: &std::path::Path,
        display: Option<&std::path::Path>,
    ) {
        let (action, copy) = {
            let state = self.as_mut().rust_mut().get_mut();
            (
                state.pending_action.take(),
                std::mem::take(&mut state.pending_copy),
            )
        };
        if action.as_deref() == Some("annotate") {
            crate::editor_window::publish_open(path.to_path_buf());
        }
        let settings = crate::settings::stored_settings();
        if let Some(shown) = forced_copy_source(&settings, copy, path, display)
            && let Err(error) = crate::preview::copy_capture(&crate::commands::SystemRunner, shown)
        {
            self.as_mut().set_message(QString::from(format!(
                "The capture was saved but could not be copied: {error}"
            )));
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

/// The file a Ctrl-forced copy takes, or none when nothing should be copied.
///
/// The picture the preview shows is what the user picked, so a composed window
/// frame is copied rather than the raw saved capture. The source's
/// `capture_ctrl_copy` setting can disable the modifier altogether.
fn forced_copy_source<'a>(
    settings: &std::collections::BTreeMap<String, Value>,
    force_copy: bool,
    capture: &'a Path,
    display: Option<&'a Path>,
) -> Option<&'a Path> {
    if !force_copy
        || !settings
            .get("capture_ctrl_copy")
            .is_none_or(Value::is_truthy)
    {
        return None;
    }
    Some(display.unwrap_or(capture))
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
                    [flag, query] if flag == "-j" && query == "getprop" => "{}".to_owned(),
                    [flag, query] if flag == "-j" && query == "getoption" => "{}".to_owned(),
                    [action, ..] if action == "repl" => "false".to_owned(),
                    _ => "ok".to_owned(),
                };
                return Ok(CommandOutput {
                    status: 0,
                    stdout: text.into_bytes(),
                    stderr: Vec::new(),
                });
            }
            if spec.program.ends_with("scroll-helper") {
                return Ok(CommandOutput {
                    status: 0,
                    stdout: Vec::new(),
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
            {"address":"0x112345678","title":"Editor","class":"code","at":[10,20],"size":[600,400],"monitor":0,"workspace":{"name":"1"},"mapped":true,"focusHistoryID":0},
            {"address":"0x1abcdef01","title":"Browser","class":"firefox","at":[700,100],"size":[800,600],"monitor":0,"workspace":{"name":"1"},"mapped":true,"focusHistoryID":1},
            {"address":"0x100000002","title":"Hidden","class":"x","at":[0,0],"size":[100,100],"monitor":0,"workspace":{"name":"1"},"mapped":false}
        ]"#
    }

    fn active() -> &'static str {
        r#"{"title":"Editor","class":"code","at":[10,20],"size":[600,400],"monitor":0,"workspace":{"name":"1"},"mapped":true}"#
    }

    fn window_pam(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
        let mut data = format!(
            "P7\nWIDTH {width}\nHEIGHT {height}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n"
        )
        .into_bytes();
        for _ in 0..width * height {
            data.extend_from_slice(&pixel);
        }
        data
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
        // The helpers are built native binaries, not tracked files, so a test
        // supplies stand-ins that the fake runner answers.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            for name in ["scroll-helper", crate::window::HELPER] {
                let helper = path.join(name);
                std::fs::write(&helper, b"#!/bin/sh\n").unwrap();
                std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
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
    fn tidy_shot_own_windows_are_not_offered_for_capture() {
        let value = crate::json::parse(
            r#"[
                {"title":"TidyShot Preview — shot.png","class":"tidyshot","at":[0,0],"size":[355,300],"monitor":0,"workspace":{"name":"1"},"mapped":true},
                {"title":"TidyShot Preview — shot.png","class":"org.omarchy.TidyShot","at":[0,0],"size":[355,300],"monitor":0,"workspace":{"name":"1"},"mapped":true},
                {"title":"TidyShot Preview — shot.png","class":"com.jasonrimmer.tidyshot","at":[0,0],"size":[355,300],"monitor":0,"workspace":{"name":"1"},"mapped":true},
                {"title":"Editor","class":"code","at":[10,20],"size":[600,400],"monitor":0,"workspace":{"name":"1"},"mapped":true}
            ]"#,
        )
        .unwrap();
        let windows = windows_from(&value);
        let offered: Vec<&str> = windows
            .iter()
            .filter(|window| !window.is_own_preview())
            .map(|window| window.class.as_str())
            .collect();
        assert_eq!(offered, ["code"]);
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
        assert!(
            !grim.args.contains(&"-o".to_owned()),
            "a region grab must not also pass -o: {:?}",
            grim.args
        );
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
        runner.push_text("slurp", "0x1abcdef01\n");
        let helper = native_dir().join(crate::window::HELPER);
        runner.push(
            &helper.to_string_lossy(),
            window_pam(800, 600, [5, 6, 7, 128]),
        );
        let result = runtime(&runner, &root)
            .run(request("window"), None)
            .unwrap();
        // The saved pixels are the window's own surface, not a desktop crop.
        assert_eq!(result.rect, Some(Rect::new(700, 100, 800, 600)));
        assert_eq!((result.width, result.height), (800, 600));
        let saved = crate::pngzip::decode_png(&std::fs::read(&result.path).unwrap()).unwrap();
        assert_eq!(saved.pixel(0, 0), [5, 6, 7, 128]);
        let seen = runner.seen();
        let slurp = seen.iter().find(|spec| spec.program == "slurp").unwrap();
        assert!(slurp.args.contains(&"-r".to_owned()));
        let rows = String::from_utf8(slurp.stdin.clone().unwrap_or_default()).unwrap();
        assert!(rows.contains("10,20 600x400 0x112345678"), "{rows}");
        assert!(rows.contains("700,100 800x600 0x1abcdef01"), "{rows}");
        assert!(
            !rows.contains("0x100000002"),
            "hidden windows must not be offered: {rows}"
        );
        assert!(
            seen.iter().all(|spec| spec.program != "grim"),
            "an isolated window must not fall back to a rectangle grab"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_window_capture_writes_an_editable_composed_frame() {
        let root = scratch("window-style");
        let runner = Fake::new(monitors(), clients(), active());
        runner.push_text("slurp", "0x1abcdef01\n");
        let helper = native_dir().join(crate::window::HELPER);
        runner.push(
            &helper.to_string_lossy(),
            window_pam(800, 600, [5, 6, 7, 255]),
        );
        let runtime = runtime(&runner, &root);
        let result = runtime.run(request("window"), None).unwrap();
        // The raw window stays the history capture...
        let raw = crate::pngzip::decode_png(&std::fs::read(&result.path).unwrap()).unwrap();
        assert_eq!((raw.width, raw.height), (800, 600));
        // ...while the preview is the composed, editable frame.
        let display = result.display.clone().expect("a composed preview");
        let composed = crate::pngzip::decode_png(&std::fs::read(&display).unwrap()).unwrap();
        assert!(composed.width > raw.width && composed.height > raw.height);
        let project = crate::window_edit::load(&root, &result.path)
            .unwrap()
            .expect("the frame is editable");
        assert_eq!(project.base, raw);
        let background = project.background.expect("a background");
        assert!(background.shadow);
        assert_eq!(background.padding, crate::window::DEFAULT_PADDING);
        assert_eq!(
            crate::window_edit::display_path(&root, &result.path),
            display
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_window_request_with_a_rectangle_still_isolates_the_named_window() {
        let root = scratch("window-rect");
        let runner = Fake::new(monitors(), clients(), active());
        let helper = native_dir().join(crate::window::HELPER);
        runner.push(
            &helper.to_string_lossy(),
            window_pam(80, 60, [9, 8, 7, 255]),
        );
        let mut request = request("window");
        request.rect = Some(Rect::new(700, 100, 800, 600));
        let result = runtime(&runner, &root).run(request, None).unwrap();
        assert_eq!((result.width, result.height), (80, 60));
        assert_eq!(result.rect, Some(Rect::new(700, 100, 800, 600)));
        let seen = runner.seen();
        assert!(
            seen.iter()
                .any(|spec| spec.program.ends_with(crate::window::HELPER))
        );
        assert!(
            seen.iter()
                .all(|spec| spec.program != "grim" && spec.program != "slurp")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_transparent_window_choice_skips_the_composition() {
        let root = scratch("window-transparent");
        let runner = Fake::new(monitors(), clients(), active());
        let helper = native_dir().join(crate::window::HELPER);
        runner.push(&helper.to_string_lossy(), window_pam(20, 20, [1, 2, 3, 0]));
        let runtime = runtime(&runner, &root);
        let window = windows_from(&crate::json::parse(clients()).unwrap())
            .into_iter()
            .find(|window| window.title == "Editor")
            .unwrap();
        let identity = window.identity().unwrap();
        let result = runtime
            .capture_window(&window, &identity, false, true)
            .unwrap();
        assert!(result.display.is_none());
        assert!(
            crate::window_edit::load(&root, &result.path)
                .unwrap()
                .is_none(),
            "transparency keeps the isolated window only"
        );
        let raw = crate::pngzip::decode_png(&std::fs::read(&result.path).unwrap()).unwrap();
        assert_eq!(raw.pixel(0, 0)[3], 0);
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

    /// One binary PPM frame filled with `color`.
    fn ppm(width: usize, height: usize, color: [u8; 3]) -> Vec<u8> {
        let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
        for _ in 0..width * height {
            out.extend_from_slice(&color);
        }
        out
    }

    #[test]
    fn a_headless_scrolling_capture_saves_the_stitched_png() {
        let root = scratch("scroll");
        let runner = Fake::new(monitors(), clients(), active());
        runner.push_text("slurp", "10,20 100x200\n");
        // Every frame is identical, so the repeated-row stop condition ends
        // the capture after the first frame without waiting for a scroll.
        for _ in 0..6 {
            runner.push("grim", ppm(100, 200, [12, 34, 56]));
        }
        let result = runtime(&runner, &root)
            .run(request("scroll"), None)
            .unwrap();
        assert_eq!(result.rect, Some(Rect::new(10, 20, 100, 200)));
        assert_eq!((result.width, result.height), (100, 200));
        let rows = history::read(&root);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, "scroll");
        assert_eq!(rows[0].path, result.path);
        let image = crate::pngzip::decode_png(&std::fs::read(&result.path).unwrap()).unwrap();
        assert_eq!(image.pixel(0, 0), [12, 34, 56, 255]);
        let seen = runner.seen();
        assert!(
            seen.iter().any(|spec| spec.program == "slurp"),
            "a request without a region selects one"
        );
        let grim: Vec<&CommandSpec> = seen.iter().filter(|spec| spec.program == "grim").collect();
        assert!(!grim.is_empty());
        assert!(grim[0].args.iter().any(|arg| arg == "ppm"));
        assert!(grim[0].args.contains(&"10,20 100x200".to_owned()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_scroll_helper_decision_follows_the_native_directory() {
        let dir = scratch("scroll-helper-decision");
        assert!(!scroll_helper_available(&dir), "an empty dir has no helper");
        std::fs::write(dir.join("scroll-helper"), b"#!/bin/sh\n").unwrap();
        assert!(
            scroll_helper_available(&dir),
            "a built helper enables auto-scroll"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_scroll_request_with_a_region_does_not_run_the_selector() {
        let root = scratch("scroll-fixed");
        let runner = Fake::new(monitors(), clients(), active());
        for _ in 0..6 {
            runner.push("grim", ppm(80, 120, [1, 2, 3]));
        }
        let mut request = request("scroll-horizontal");
        request.rect = Some(Rect::new(5, 6, 80, 120));
        let result = runtime(&runner, &root).run(request, None).unwrap();
        assert_eq!(result.rect, Some(Rect::new(5, 6, 80, 120)));
        assert_eq!((result.width, result.height), (80, 120));
        assert!(
            runner.seen().iter().all(|spec| spec.program != "slurp"),
            "a pinned region must skip the selector"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_scrolling_cancel_saves_nothing() {
        let root = scratch("scroll-cancel");
        let runner = Fake::new(monitors(), clients(), active());
        let mut request = request("scroll");
        request.rect = Some(Rect::new(0, 0, 100, 100));
        let error = runtime(&runner, &root)
            .run_with_cancel(request, None, || true)
            .unwrap_err();
        assert!(matches!(error, RuntimeError::Cancelled), "{error}");
        assert!(history::read(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_stitched_rgb_frame_is_saved_as_rgba_png() {
        let root = scratch("scroll-encode");
        let runner = Fake::new(monitors(), clients(), active());
        let frame = crate::stitch::FrameData::new(vec![9, 8, 7, 6, 5, 4], 2, 1, 3);
        let result = runtime(&runner, &root)
            .save_scroll(&frame, Some(Rect::new(0, 0, 2, 1)))
            .unwrap();
        let image = crate::pngzip::decode_png(&std::fs::read(&result.path).unwrap()).unwrap();
        assert_eq!(image.pixel(0, 0), [9, 8, 7, 255]);
        assert_eq!(image.pixel(1, 0), [6, 5, 4, 255]);
        assert_eq!(history::read(&root)[0].kind, "scroll");
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
    fn a_ctrl_choice_copies_the_picture_the_preview_shows() {
        let settings = std::collections::BTreeMap::new();
        let capture = Path::new("/tmp/capture.png");
        let display = Path::new("/tmp/composed.png");
        assert_eq!(
            forced_copy_source(&settings, true, capture, Some(display)),
            Some(display)
        );
        assert_eq!(
            forced_copy_source(&settings, true, capture, None),
            Some(capture)
        );
        assert_eq!(
            forced_copy_source(&settings, false, capture, Some(display)),
            None
        );
        let disabled = std::collections::BTreeMap::from([(
            "capture_ctrl_copy".to_owned(),
            crate::json::Value::Bool(false),
        )]);
        assert_eq!(
            forced_copy_source(&disabled, true, capture, Some(display)),
            None,
            "the source's setting can disable Ctrl-copy"
        );
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
