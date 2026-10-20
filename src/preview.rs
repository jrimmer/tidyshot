//! The Quick Access preview window: one capture, its corner and its actions.
// Ported from omnishot/widgets.py (QuickOverlay window placement) and
// omnishot/app.py (after-capture actions).
//!
//! The window is frameless and stays on top. QML owns the layout; this module
//! owns where the window sits, what its buttons do and how auto-close behaves,
//! so the offscreen tests can drive every decision without a compositor.

use std::path::{Path, PathBuf};

use crate::commands::{CommandError, CommandRunner, clipboard_spec};
use crate::json::Value;
use crate::overlay::{Corner, PreviewSettings, ScreenRect, corner_position, is_video};

/// The default preview window size in logical pixels.
pub const WINDOW_WIDTH: f64 = 355.0;
/// The expandable body height; a collapsed preview keeps only its title bar.
pub const WINDOW_HEIGHT: f64 = 300.0;
/// The collapsed height that leaves the restore control visible.
pub const COLLAPSED_HEIGHT: f64 = 52.0;
/// The screen width assumed when no monitor can be resolved.
pub const FALLBACK_SCREEN_WIDTH: f64 = 1920.0;
/// The screen height assumed when no monitor can be resolved.
pub const FALLBACK_SCREEN_HEIGHT: f64 = 1080.0;

/// Why a preview action failed.
#[derive(Debug)]
pub enum PreviewError {
    Io(String),
    Command(CommandError),
    Image(String),
}

impl std::fmt::Display for PreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) => f.write_str(message),
            Self::Command(error) => error.fmt(f),
            Self::Image(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for PreviewError {}

impl From<std::io::Error> for PreviewError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<CommandError> for PreviewError {
    fn from(error: CommandError) -> Self {
        Self::Command(error)
    }
}

/// The monitor a preview is anchored to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MonitorScreen {
    pub bounds: ScreenRect,
}

impl MonitorScreen {
    #[must_use]
    pub fn new(bounds: ScreenRect) -> Self {
        Self { bounds }
    }
}

/// Choose the focused monitor from `hyprctl -j monitors`, falling back to the
/// first connected output.
#[must_use]
pub fn focused_monitor(monitors: &[Value]) -> Option<MonitorScreen> {
    let displays = crate::capture_session::displays_from_monitors(monitors);
    let display = displays
        .iter()
        .find(|display| display.focused)
        .or_else(|| displays.first())?;
    let bounds = display.geometry.bounds();
    Some(MonitorScreen::new(ScreenRect::new(
        f64::from(bounds.x),
        f64::from(bounds.y),
        f64::from(bounds.width),
        f64::from(bounds.height),
    )))
}

/// The focused monitor rectangle from a raw monitor list.
#[must_use]
pub fn focused_monitor_screen(monitors: &[Value]) -> Option<ScreenRect> {
    focused_monitor(monitors).map(|monitor| monitor.bounds)
}

/// Where the preview window sits on its monitor.
#[must_use]
pub fn window_position(
    screen: MonitorScreen,
    width: f64,
    height: f64,
    settings: &PreviewSettings,
) -> (f64, f64) {
    corner_position(
        screen.bounds,
        width,
        height,
        settings.corner,
        0,
        settings.margin,
    )
}

/// A preview window before QML renders it.
#[derive(Debug, Clone)]
pub struct PreviewWindow {
    path: PathBuf,
    width: u32,
    height: u32,
    video: bool,
    settings: PreviewSettings,
    monitor: Option<MonitorScreen>,
    collapsed: bool,
    dismissed: bool,
}

impl PreviewWindow {
    /// Build a preview for a saved capture.
    #[must_use]
    pub fn new(
        path: impl Into<PathBuf>,
        width: u32,
        height: u32,
        settings: PreviewSettings,
        monitor: Option<MonitorScreen>,
    ) -> Self {
        let path = path.into();
        let video = is_video(&path);
        Self {
            path,
            width,
            height,
            video,
            settings,
            monitor,
            collapsed: false,
            dismissed: false,
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub const fn is_video(&self) -> bool {
        self.video
    }

    #[must_use]
    pub const fn collapsed(&self) -> bool {
        self.collapsed
    }

    #[must_use]
    pub const fn dismissed(&self) -> bool {
        self.dismissed
    }

    #[must_use]
    pub const fn settings(&self) -> &PreviewSettings {
        &self.settings
    }

    /// The title matches the compositor rule that floats a preview.
    #[must_use]
    pub fn title(&self) -> String {
        let name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("capture");
        format!("TidyShot Preview — {name}")
    }

    /// The line under the thumbnail: dimensions.
    #[must_use]
    pub fn info(&self) -> String {
        if self.width > 0 && self.height > 0 {
            format!("{} × {}", self.width, self.height)
        } else {
            String::new()
        }
    }

    /// The auto-close delay in milliseconds; zero keeps the window open.
    #[must_use]
    pub fn auto_close_ms(&self) -> i32 {
        self.settings.auto_close_ms()
    }

    /// Whether an elapsed timeout saves before closing.
    #[must_use]
    pub const fn auto_close_action(&self) -> &'static str {
        self.settings.auto_close_action()
    }

    #[must_use]
    pub const fn close_after_drag(&self) -> bool {
        self.settings.close_after_drag
    }

    /// The current window height: a collapsed preview shows only its title.
    #[must_use]
    pub fn window_height(&self) -> f64 {
        if self.collapsed {
            COLLAPSED_HEIGHT
        } else {
            WINDOW_HEIGHT
        }
    }

    /// The window position on the focused monitor.
    ///
    /// With no resolvable monitor the window sits at the bottom-right of the
    /// virtual desktop, so a failed monitor query cannot move it away from
    /// the corner the user expects.
    #[must_use]
    pub fn position(&self) -> (f64, f64) {
        let screen = self
            .monitor
            .map_or_else(fallback_screen, |monitor| monitor.bounds);
        window_position(
            MonitorScreen::new(screen),
            WINDOW_WIDTH,
            self.window_height(),
            &self.settings,
        )
    }

    pub fn set_collapsed(&mut self, collapsed: bool) {
        self.collapsed = collapsed;
    }

    /// Dismiss the preview.
    pub fn dismiss(&mut self) {
        self.dismissed = true;
    }
}

/// The note shown when the preview cannot anchor to a real monitor.
#[must_use]
pub fn monitor_hint(screen: Option<ScreenRect>) -> &'static str {
    if screen.is_some() {
        ""
    } else {
        "Monitor unavailable; using the default corner"
    }
}

/// The screen a preview assumes when the monitor query failed.
///
/// The virtual desktop is unbounded, so the fallback is a desktop-sized area
/// whose bottom-right corner is the only anchor the window can still honour.
#[must_use]
fn fallback_screen() -> ScreenRect {
    ScreenRect::new(0.0, 0.0, FALLBACK_SCREEN_WIDTH, FALLBACK_SCREEN_HEIGHT)
}

/// Copy a capture or recording to the clipboard.
pub fn copy_capture<R: CommandRunner>(runner: &R, path: &Path) -> Result<(), PreviewError> {
    if is_video(path) {
        let data = format!("file://{}\n", path.display()).into_bytes();
        runner.run(&clipboard_spec("text/uri-list", data))?;
    } else {
        let bytes = std::fs::read(path)?;
        runner.run(&clipboard_spec("image/png", bytes))?;
    }
    Ok(())
}

/// Save a preview back through the atomic store.
///
/// The capture already lives on disk; saving again rewrites the same path so a
/// failure is surfaced instead of silently reporting success.
pub fn save_capture(path: &Path) -> Result<(), PreviewError> {
    let bytes = std::fs::read(path)?;
    crate::storage::atomic_write_file(path, &bytes)?;
    Ok(())
}

/// Open the annotate editor on a capture.
///
/// The editor decodes the file itself, so this only checks that it still
/// exists before handing the path over; a malformed file is reported by the
/// editor window instead of being decoded twice here.
pub fn annotate_capture(path: &Path) -> Result<(), PreviewError> {
    if path.is_file() {
        Ok(())
    } else {
        Err(PreviewError::Io(format!(
            "the capture is no longer available: {}",
            path.display()
        )))
    }
}

/// Pin a capture using the pin module's starting geometry.
///
/// The pin window itself is not ported yet, so this only resolves the
/// geometry the pin would start with.
pub fn pin_capture(path: &Path) -> Result<(f64, f64), PreviewError> {
    let bytes = std::fs::read(path)?;
    let image = crate::pngzip::decode_png(&bytes)
        .map_err(|error| PreviewError::Image(error.to_string()))?;
    Ok(crate::pin::initial_size(image.width, image.height))
}

/// The corner a new preview uses when no settings override it.
#[must_use]
pub const fn default_corner() -> Corner {
    Corner::BottomRight
}

// --- The preview state shared by captures and the QML window --------------

/// A capture that should be shown, and who it belongs to.
#[derive(Debug, Clone)]
struct Published {
    path: PathBuf,
    width: u32,
    height: u32,
    screen: Option<ScreenRect>,
    standalone: bool,
}

static PENDING: std::sync::Mutex<Option<Published>> = std::sync::Mutex::new(None);
static STARTUP: std::sync::Mutex<Option<Published>> = std::sync::Mutex::new(None);
static BOUND: std::sync::OnceLock<cxx_qt::CxxQtThread<qobject::Preview>> =
    std::sync::OnceLock::new();

/// Show a preview for a capture the runtime just wrote.
pub fn publish_capture(path: PathBuf, width: u32, height: u32, screen: Option<ScreenRect>) {
    let published = Published {
        path,
        width,
        height,
        screen,
        standalone: false,
    };
    match BOUND.get() {
        Some(thread) => {
            let _ = thread.queue(move |mut preview| {
                preview.as_mut().apply_published(published);
            });
        }
        None => {
            *PENDING.lock().expect("preview queue poisoned") = Some(published);
        }
    }
}

/// Show a preview for an existing file, used by `overlay` and `restore`.
pub fn publish_file(path: PathBuf) {
    let dimensions = std::fs::read(&path)
        .ok()
        .and_then(|bytes| crate::annotation::png_dimensions(&bytes))
        .unwrap_or((0, 0));
    let screen = crate::commands::hypr(&crate::commands::SystemRunner, "monitors")
        .ok()
        .and_then(|value| value.as_array().and_then(focused_monitor_screen));
    publish_capture(path, dimensions.0, dimensions.1, screen);
}

/// A command for the visible preview that may arrive before it is bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingAction {
    Save,
    Dismiss,
}

static ACTIONS: std::sync::Mutex<Vec<PendingAction>> = std::sync::Mutex::new(Vec::new());

/// Save the visible preview.
pub fn publish_save() {
    queue_action(PendingAction::Save);
}

/// Close the visible preview.
pub fn publish_dismiss() {
    queue_action(PendingAction::Dismiss);
}

/// Run a preview action on the Qt thread, or hold it until QML binds.
fn queue_action(action: PendingAction) {
    match BOUND.get() {
        Some(thread) => {
            let _ = thread.queue(move |mut preview| run_action(preview.as_mut(), action));
        }
        None => ACTIONS
            .lock()
            .expect("preview action queue poisoned")
            .push(action),
    }
}

fn run_action(mut preview: Pin<&mut qobject::Preview>, action: PendingAction) {
    match action {
        PendingAction::Save => preview.as_mut().save(),
        PendingAction::Dismiss => preview.as_mut().dismiss(),
    }
}

/// Hand the GUI a capture that a command-line invocation already saved.
///
/// The capture waits until QML binds the preview, so only one startup entry
/// is ever stored.
pub fn set_startup_preview(result: crate::capture_runtime::CaptureResult) {
    *STARTUP.lock().expect("preview queue poisoned") = Some(Published {
        path: result.path,
        width: result.width,
        height: result.height,
        screen: result.screen,
        standalone: true,
    });
}

// --- QML bridge: the Preview singleton -----------------------------------

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qurl.h");
        type QUrl = cxx_qt_lib::QUrl;
    }

    extern "RustQt" {
        /// The Quick Access preview window QML renders after a capture.
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(bool, open)]
        #[qproperty(bool, standalone)]
        #[qproperty(QString, path)]
        #[qproperty(QUrl, content_url, cxx_name = "contentUrl")]
        #[qproperty(QString, title)]
        #[qproperty(QString, info)]
        #[qproperty(QString, message)]
        #[qproperty(bool, video)]
        #[qproperty(bool, collapsed)]
        #[qproperty(i32, x)]
        #[qproperty(i32, y)]
        #[qproperty(i32, window_width, cxx_name = "windowWidth")]
        #[qproperty(i32, window_height, cxx_name = "windowHeight")]
        #[qproperty(i32, auto_close_ms, cxx_name = "autoCloseMs")]
        #[qproperty(QString, auto_close_action, cxx_name = "autoCloseAction")]
        #[qproperty(bool, close_after_drag, cxx_name = "closeAfterDrag")]
        type Preview = super::PreviewRust;

        /// Register this object for captures that arrive before QML loads.
        #[qinvokable]
        fn bind(self: Pin<&mut Self>);

        /// Copy the capture to the clipboard.
        #[qinvokable]
        fn copy(self: Pin<&mut Self>);

        /// Save the capture again through the atomic store.
        #[qinvokable]
        fn save(self: Pin<&mut Self>);

        /// Open the capture in the annotate editor.
        #[qinvokable]
        fn annotate(self: Pin<&mut Self>);

        /// Pin the capture at its starting size.
        #[qinvokable]
        fn pin(self: Pin<&mut Self>);

        /// Close the preview.
        #[qinvokable]
        fn dismiss(self: Pin<&mut Self>);

        /// Collapse or restore the preview body.
        #[qinvokable]
        #[cxx_name = "collapseTo"]
        fn collapse(self: Pin<&mut Self>, collapsed: bool);
    }

    impl cxx_qt::Threading for Preview {}
}

use core::pin::Pin;
use cxx_qt::CxxQtType as _;
use cxx_qt::Threading as _;
use cxx_qt_lib::{QString, QUrl};

/// Rust state behind the QML `Preview` singleton.
pub struct PreviewRust {
    open: bool,
    standalone: bool,
    path: QString,
    content_url: QUrl,
    title: QString,
    info: QString,
    message: QString,
    video: bool,
    collapsed: bool,
    x: i32,
    y: i32,
    window_width: i32,
    window_height: i32,
    auto_close_ms: i32,
    auto_close_action: QString,
    close_after_drag: bool,
    window: Option<PreviewWindow>,
}

impl Default for PreviewRust {
    fn default() -> Self {
        // A command-line capture saved before the GUI started is applied here,
        // not only in `bind`: the preview window is constructed with its final
        // state, so it maps on the first commit instead of needing a runtime
        // show, which the Wayland backend refuses for an unparented frame.
        let (standalone, startup) = {
            let mut queue = STARTUP.lock().expect("preview queue poisoned");
            match queue.take() {
                Some(published) => (true, Some(published)),
                None => (false, None),
            }
        };
        let mut rust = Self {
            open: false,
            standalone,
            path: QString::from(""),
            content_url: QUrl::default(),
            title: QString::from(""),
            info: QString::from(""),
            message: QString::from(""),
            video: false,
            collapsed: false,
            x: 0,
            y: 0,
            window_width: WINDOW_WIDTH as i32,
            window_height: WINDOW_HEIGHT as i32,
            auto_close_ms: 0,
            auto_close_action: QString::from("close"),
            close_after_drag: true,
            window: None,
        };
        if let Some(published) = startup {
            rust.apply(published);
        }
        rust
    }
}

impl PreviewRust {
    fn apply(&mut self, published: Published) {
        let settings = crate::settings::preview_settings();
        let monitor = published.screen.map(MonitorScreen::new);
        let window = PreviewWindow::new(
            &published.path,
            published.width,
            published.height,
            settings,
            monitor,
        );
        self.path = QString::from(window.path().display().to_string());
        self.content_url =
            QUrl::from_local_file(&QString::from(window.path().display().to_string()));
        self.title = QString::from(window.title());
        self.info = QString::from(window.info());
        self.message = QString::from(monitor_hint(published.screen));
        self.video = window.is_video();
        self.collapsed = window.collapsed();
        let (x, y) = window.position();
        self.x = x.round() as i32;
        self.y = y.round() as i32;
        self.window_width = WINDOW_WIDTH as i32;
        self.window_height = window.window_height() as i32;
        self.auto_close_ms = window.auto_close_ms();
        self.auto_close_action = QString::from(window.auto_close_action());
        self.close_after_drag = window.close_after_drag();
        self.standalone = published.standalone;
        self.open = !window.dismissed();
        self.window = Some(window);
    }

    fn action_error(&mut self, result: Result<(), PreviewError>, done: &str) {
        self.message = match result {
            Ok(()) => QString::from(done),
            Err(error) => QString::from(error.to_string()),
        };
    }
}

impl qobject::Preview {
    /// Copy the Rust state onto the QObject properties.
    ///
    /// `PreviewRust` methods mutate plain fields; without going through the
    /// generated setters no change signal fires and QML bindings keep the
    /// stale value, so the window never maps even though `open` reads true.
    fn sync(mut self: Pin<&mut Self>) {
        let snapshot = {
            let rust = self.as_mut().rust_mut().get_mut();
            (
                rust.open,
                rust.standalone,
                rust.path.clone(),
                rust.content_url.clone(),
                rust.title.clone(),
                rust.info.clone(),
                rust.message.clone(),
                rust.video,
                rust.collapsed,
                rust.x,
                rust.y,
                rust.window_width,
                rust.window_height,
                rust.auto_close_ms,
                rust.auto_close_action.clone(),
                rust.close_after_drag,
            )
        };
        self.as_mut().set_open(snapshot.0);
        self.as_mut().set_standalone(snapshot.1);
        self.as_mut().set_path(snapshot.2);
        self.as_mut().set_content_url(snapshot.3);
        self.as_mut().set_title(snapshot.4);
        self.as_mut().set_info(snapshot.5);
        self.as_mut().set_message(snapshot.6);
        self.as_mut().set_video(snapshot.7);
        self.as_mut().set_collapsed(snapshot.8);
        self.as_mut().set_x(snapshot.9);
        self.as_mut().set_y(snapshot.10);
        self.as_mut().set_window_width(snapshot.11);
        self.as_mut().set_window_height(snapshot.12);
        self.as_mut().set_auto_close_ms(snapshot.13);
        self.as_mut().set_auto_close_action(snapshot.14);
        self.as_mut().set_close_after_drag(snapshot.15);
    }

    /// Register this object, then replay any capture or action that arrived
    /// before QML loaded.
    pub fn bind(mut self: Pin<&mut Self>) {
        let pending = PENDING.lock().expect("preview queue poisoned").take();
        let startup = STARTUP.lock().expect("preview queue poisoned").take();
        let actions = std::mem::take(&mut *ACTIONS.lock().expect("preview action queue poisoned"));
        let _ = BOUND.set(self.qt_thread());
        if let Some(published) = pending.or(startup) {
            self.as_mut().rust_mut().get_mut().apply(published);
            self.as_mut().sync();
        }
        for action in actions {
            run_action(self.as_mut(), action);
        }
    }

    /// Apply a capture published by the runtime.
    fn apply_published(mut self: Pin<&mut Self>, published: Published) {
        self.as_mut().rust_mut().get_mut().apply(published);
        self.as_mut().sync();
    }

    /// Copy the capture to the clipboard.
    pub fn copy(mut self: Pin<&mut Self>) {
        let path = PathBuf::from(self.path().to_string());
        let result = copy_capture(&crate::commands::SystemRunner, &path);
        self.as_mut()
            .rust_mut()
            .get_mut()
            .action_error(result, "Copied to the clipboard");
        self.as_mut().sync();
    }

    /// Save the capture again through the atomic store.
    pub fn save(mut self: Pin<&mut Self>) {
        let path = PathBuf::from(self.path().to_string());
        let result = save_capture(&path);
        self.as_mut()
            .rust_mut()
            .get_mut()
            .action_error(result, "Saved");
        self.as_mut().sync();
    }

    /// Open the capture in the annotate editor.
    pub fn annotate(mut self: Pin<&mut Self>) {
        let path = PathBuf::from(self.path().to_string());
        let result = annotate_capture(&path);
        if result.is_ok() {
            crate::editor_window::publish_open(path);
        }
        self.as_mut()
            .rust_mut()
            .get_mut()
            .action_error(result, "Opened in the editor");
        self.as_mut().sync();
    }

    /// Pin the capture at its starting size.
    ///
    /// No pin window exists yet, so the message names the missing window.
    pub fn pin(mut self: Pin<&mut Self>) {
        let path = PathBuf::from(self.path().to_string());
        let result = pin_capture(&path).map(|_| ());
        self.as_mut()
            .rust_mut()
            .get_mut()
            .action_error(result, "The pin window is not available yet");
        self.as_mut().sync();
    }

    /// Close the preview.
    pub fn dismiss(mut self: Pin<&mut Self>) {
        if let Some(window) = self.as_mut().rust_mut().get_mut().window.as_mut() {
            window.dismiss();
        }
        // Reset the delay as well: the QML timer stops being eligible once
        // the window is hidden, so a later save must not fire for it.
        self.as_mut().set_auto_close_ms(0);
        self.as_mut().set_open(false);
        self.as_mut().set_message(QString::from(""));
    }

    /// Collapse or restore the preview body.
    pub fn collapse(mut self: Pin<&mut Self>, collapsed: bool) {
        if let Some(window) = self.as_mut().rust_mut().get_mut().window.as_mut() {
            window.set_collapsed(collapsed);
            let (x, y) = window.position();
            let height = window.window_height();
            self.as_mut().set_x(x.round() as i32);
            self.as_mut().set_y(y.round() as i32);
            self.as_mut().set_window_height(height as i32);
        }
        self.as_mut().set_collapsed(collapsed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandError, CommandOutput, CommandSpec};
    use std::sync::Mutex;

    fn settings() -> PreviewSettings {
        PreviewSettings::default()
    }

    #[test]
    fn a_new_preview_sits_bottom_right_of_the_focused_monitor() {
        let monitor = MonitorScreen::new(ScreenRect::new(0.0, 0.0, 1920.0, 1080.0));
        let preview = PreviewWindow::new("/tmp/capture.png", 640, 480, settings(), Some(monitor));
        assert_eq!(preview.settings().corner, default_corner());
        assert_eq!(preview.position(), (1547.0, 762.0));
        assert_eq!(preview.auto_close_ms(), 0);
        assert!(preview.title().starts_with("TidyShot Preview"));
        assert_eq!(preview.info(), "640 × 480");
    }

    #[test]
    fn save_and_close_wait_for_the_preview_to_bind() {
        publish_dismiss();
        publish_save();
        let actions = std::mem::take(&mut *ACTIONS.lock().unwrap());
        assert_eq!(
            actions,
            [PendingAction::Dismiss, PendingAction::Save],
            "an action that arrives before the window is bound is replayed"
        );
    }

    #[test]
    fn a_command_line_capture_is_marked_standalone() {
        let result = crate::capture_runtime::CaptureResult {
            path: PathBuf::from("/tmp/capture.png"),
            width: 10,
            height: 10,
            rect: None,
            screen: Some(ScreenRect::new(0.0, 0.0, 1920.0, 1080.0)),
            display: None,
        };
        set_startup_preview(result);
        assert!(
            STARTUP.lock().unwrap().is_some(),
            "the startup preview is queued for the QML bind"
        );
        let _ = STARTUP.lock().unwrap().take();
    }

    #[test]
    fn a_configured_timeout_and_action_reach_the_window() {
        let preview = PreviewWindow::new(
            "/tmp/capture.png",
            10,
            10,
            PreviewSettings {
                timeout: 2.5,
                auto_action: crate::overlay::AutoAction::Save,
                ..settings()
            },
            None,
        );
        assert_eq!(preview.auto_close_ms(), 2500);
        assert_eq!(preview.auto_close_action(), "save");
    }

    #[test]
    fn collapsing_moves_the_window_to_the_collapsed_height() {
        let monitor = MonitorScreen::new(ScreenRect::new(0.0, 0.0, 1920.0, 1080.0));
        let mut preview = PreviewWindow::new("/tmp/capture.png", 10, 10, settings(), Some(monitor));
        let expanded = preview.position();
        preview.set_collapsed(true);
        let collapsed = preview.position();
        assert!(collapsed.1 > expanded.1);
        assert_eq!(preview.window_height(), COLLAPSED_HEIGHT);
    }

    #[test]
    fn focused_monitor_prefers_the_focused_output() {
        let monitors = crate::json::parse(
            r#"[
                {"name":"left","x":0,"y":0,"width":1920,"height":1080,"focused":false},
                {"name":"right","x":1920,"y":0,"width":2560,"height":1440,"focused":true}
            ]"#,
        )
        .unwrap();
        let monitor = focused_monitor(monitors.as_array().unwrap()).unwrap();
        assert_eq!(monitor.bounds, ScreenRect::new(1920.0, 0.0, 2560.0, 1440.0));
    }

    #[test]
    fn a_missing_monitor_list_falls_back_to_the_bottom_right_corner() {
        assert!(focused_monitor(&[]).is_none());
        let preview = PreviewWindow::new("/tmp/capture.png", 10, 10, settings(), None);
        assert_eq!(preview.position(), (1547.0, 762.0));
    }

    #[test]
    fn a_failed_monitor_query_is_reported_on_the_window() {
        assert_eq!(
            monitor_hint(None),
            "Monitor unavailable; using the default corner"
        );
        assert_eq!(
            monitor_hint(Some(ScreenRect::new(0.0, 0.0, 1920.0, 1080.0))),
            ""
        );
    }

    #[test]
    fn a_zero_margin_puts_the_window_flush_with_the_corner() {
        let monitor = MonitorScreen::new(ScreenRect::new(0.0, 0.0, 1920.0, 1080.0));
        let preview = PreviewWindow::new(
            "/tmp/capture.png",
            640,
            480,
            PreviewSettings {
                margin: 0.0,
                ..settings()
            },
            Some(monitor),
        );
        assert_eq!(preview.position(), (1565.0, 780.0));
    }

    #[derive(Default)]
    struct Fake {
        seen: Mutex<Vec<CommandSpec>>,
        fail: bool,
    }

    impl CommandRunner for Fake {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            self.seen.lock().unwrap().push(spec.clone());
            if self.fail {
                Err(CommandError::Failure {
                    program: spec.program.clone(),
                    output: CommandOutput {
                        status: 1,
                        stdout: Vec::new(),
                        stderr: b"clipboard unavailable".to_vec(),
                    },
                })
            } else {
                Ok(CommandOutput {
                    status: 0,
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                })
            }
        }
    }

    #[test]
    fn copy_sends_the_image_to_the_clipboard() {
        let dir = std::env::temp_dir().join(format!("tidyshot-preview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.png");
        let image = crate::pngzip::Image::new(4, 3, vec![7u8; 4 * 3 * 4]);
        std::fs::write(&path, image.to_png()).unwrap();
        let runner = Fake::default();
        copy_capture(&runner, &path).unwrap();
        let seen = runner.seen.lock().unwrap();
        assert_eq!(seen[0].program, "wl-copy");
        assert_eq!(seen[0].args, ["--type", "image/png"]);
        assert_eq!(seen[0].stdin.as_deref(), Some(image.to_png().as_slice()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copy_surfaces_a_clipboard_failure() {
        let dir =
            std::env::temp_dir().join(format!("tidyshot-preview-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.png");
        std::fs::write(
            &path,
            crate::pngzip::Image::new(1, 1, vec![0u8; 4]).to_png(),
        )
        .unwrap();
        let runner = Fake {
            fail: true,
            ..Fake::default()
        };
        let error = copy_capture(&runner, &path).unwrap_err();
        assert!(error.to_string().contains("clipboard unavailable"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_rewrites_the_capture_atomically_and_reports_errors() {
        let dir =
            std::env::temp_dir().join(format!("tidyshot-preview-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.png");
        let bytes = crate::pngzip::Image::new(2, 2, vec![9u8; 16]).to_png();
        std::fs::write(&path, &bytes).unwrap();
        save_capture(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let missing = dir.join("missing.png");
        assert!(save_capture(&missing).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn annotate_and_pin_use_the_capture_image() {
        let dir =
            std::env::temp_dir().join(format!("tidyshot-preview-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.png");
        std::fs::write(
            &path,
            crate::pngzip::Image::new(320, 200, vec![1u8; 320 * 200 * 4]).to_png(),
        )
        .unwrap();
        annotate_capture(&path).unwrap();
        let (width, height) = pin_capture(&path).unwrap();
        assert_eq!((width, height), (320.0, 200.0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
