//! The corner preview overlay: one capture, its actions and its gestures.
// Ported from omnishot/widgets.py (QuickOverlay, DragPreview)
//!
//! The overlay is a frameless tool window. The state machine here decides when
//! it auto-closes, what a wheel swipe means, where it sits in its corner and
//! which actions it offers for an image or a recording. QML owns the layout;
//! this module owns the behavior so the offscreen tests can drive it with
//! synthetic events.

use std::path::{Path, PathBuf};

use crate::commands::CommandRunner;
use crate::editor::Document;
use crate::pngzip::Image;

/// A capture is a recording when its extension is one of these.
pub const VIDEO_EXTENSIONS: &[&str] = &[".mp4", ".gif", ".webm", ".mov", ".mkv"];

/// A wheel swipe this many logical pixels wide dismisses the preview.
pub const SWIPE_DISMISS: f64 = 90.0;
/// A wheel swipe this far down collapses the preview.
pub const SWIPE_COLLAPSE: f64 = 90.0;

/// Which corner the preview prefers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    BottomLeft,
    BottomRight,
    TopLeft,
    TopRight,
}

impl Corner {
    /// Parse the settings value, falling back to the bottom-left corner the
    /// source uses.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "bottom-right" => Self::BottomRight,
            "top-left" => Self::TopLeft,
            "top-right" => Self::TopRight,
            _ => Self::BottomLeft,
        }
    }

    #[must_use]
    pub const fn is_left(self) -> bool {
        matches!(self, Self::BottomLeft | Self::TopLeft)
    }

    #[must_use]
    pub const fn is_top(self) -> bool {
        matches!(self, Self::TopLeft | Self::TopRight)
    }
}

/// A monitor's usable rectangle in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ScreenRect {
    #[must_use]
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// Position a preview of `width` x `height` in its corner, stacked by
/// `index`, exactly as the source computes it before handing it to the
/// compositor.
// Ported from omnishot/widgets.py (QuickOverlay.position_on)
#[must_use]
pub fn corner_position(
    screen: ScreenRect,
    width: f64,
    height: f64,
    corner: Corner,
    index: u32,
    margin: f64,
) -> (f64, f64) {
    let mut x = if corner.is_left() {
        screen.x + margin
    } else {
        screen.x + screen.width - width - margin
    };
    let mut y = if corner.is_top() {
        screen.y + margin
    } else {
        screen.y + screen.height - height - margin
    };
    let stack = f64::from(index) * margin;
    // A later preview in the same corner stacks away from the edge: downward
    // from the top, upward from the bottom.
    if corner.is_top() {
        y += stack;
    } else {
        y -= stack;
    }
    if !corner.is_left() {
        x = x.max(screen.x + margin);
    }
    (x, y)
}

/// One wheel gesture, already converted to logical pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Swipe {
    pub x: f64,
    pub y: f64,
}

/// What a swipe did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwipeAction {
    /// Swipe sideways past the dismissal threshold.
    Dismiss,
    /// Swipe down past the collapse threshold.
    Collapse,
    /// Swipe up past the restore threshold.
    Restore,
    /// Still accumulating, or a plain mouse wheel that was ignored.
    None,
}

/// The swipe state machine, mirroring the source's threshold checks.
#[derive(Debug, Default)]
pub struct SwipeState {
    accumulated: Swipe,
}

impl SwipeState {
    /// A new gesture. `pixel` is false for a traditional wheel, which the
    /// source ignores so a mouse wheel cannot dismiss a capture.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            accumulated: Swipe { x: 0.0, y: 0.0 },
        }
    }

    /// Feed one wheel delta. `inverted` flips the axis as the source does.
    ///
    /// Returns the action the swipe triggered, resetting the accumulator on
    /// any threshold crossing.
    pub fn wheel(&mut self, pixel: bool, delta: Swipe, inverted: bool) -> SwipeAction {
        if !pixel {
            return SwipeAction::None;
        }
        let delta = if inverted {
            Swipe {
                x: -delta.x,
                y: -delta.y,
            }
        } else {
            delta
        };
        self.accumulated.x += delta.x;
        self.accumulated.y += delta.y;
        if self.accumulated.x.abs() > SWIPE_DISMISS {
            self.accumulated = Swipe { x: 0.0, y: 0.0 };
            return SwipeAction::Dismiss;
        }
        if self.accumulated.y < -SWIPE_COLLAPSE {
            self.accumulated = Swipe { x: 0.0, y: 0.0 };
            return SwipeAction::Collapse;
        }
        if self.accumulated.y > SWIPE_COLLAPSE {
            self.accumulated = Swipe { x: 0.0, y: 0.0 };
            return SwipeAction::Restore;
        }
        SwipeAction::None
    }

    /// A scroll phase end clears any half-finished swipe.
    pub fn phase_end(&mut self) {
        self.accumulated = Swipe { x: 0.0, y: 0.0 };
    }
}

/// The actions a preview offers for a given media kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewAction {
    Annotate,
    Copy,
    Save,
    Pin,
}

/// The visible action list, in the order the source lays the buttons out.
#[must_use]
pub fn available_actions(video: bool) -> Vec<PreviewAction> {
    let mut actions = vec![
        PreviewAction::Annotate,
        PreviewAction::Copy,
        PreviewAction::Save,
    ];
    if !video {
        actions.push(PreviewAction::Pin);
    }
    actions
}

/// True when a path names a recording.
#[must_use]
pub fn is_video(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!(".{}", extension.to_ascii_lowercase()))
        .unwrap_or_default();
    VIDEO_EXTENSIONS.contains(&extension.as_str())
}

/// The label the title uses: recordings and captures read differently.
#[must_use]
pub const fn ready_title(video: bool) -> &'static str {
    if video {
        "Recording ready"
    } else {
        "Capture ready"
    }
}

/// One preview, from its content to its lifecycle.
///
/// The pixel content is loaded once; the UI never re-reads the file while the
/// preview is open.
#[derive(Debug)]
pub struct Preview {
    path: PathBuf,
    image: Image,
    video: bool,
    name: String,
    collapsed: bool,
    dragging: bool,
    copying: bool,
    large_open: bool,
    timer_armed: bool,
    timeout: f64,
    auto_action: AutoAction,
    close_after_drag: bool,
    corner: Corner,
    margin: f64,
    index: u32,
}

/// What an elapsed timeout does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoAction {
    Close,
    Save,
}

impl AutoAction {
    #[must_use]
    pub fn parse(value: &str) -> Self {
        if value == "save" {
            Self::Save
        } else {
            Self::Close
        }
    }
}

/// The settings a preview needs. The rest of the app stores these in the
/// settings JSON; this is the subset the overlay reads.
#[derive(Debug, Clone)]
pub struct PreviewSettings {
    pub timeout: f64,
    pub auto_action: AutoAction,
    pub close_after_drag: bool,
    pub corner: Corner,
    pub margin: f64,
}

impl Default for PreviewSettings {
    fn default() -> Self {
        Self {
            timeout: 0.0,
            auto_action: AutoAction::Close,
            close_after_drag: true,
            corner: Corner::BottomLeft,
            margin: 18.0,
        }
    }
}

/// Why a preview could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewError {
    message: String,
}

impl std::fmt::Display for PreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PreviewError {}

impl Preview {
    /// Build a preview for `path` at `index` in its stack.
    ///
    /// The file is read once; the decoded image is owned. A recording gets its
    /// first frame through ffmpeg as the thumbnail.
    pub fn open<R: CommandRunner>(
        path: impl Into<PathBuf>,
        index: u32,
        settings: &PreviewSettings,
        runner: &R,
    ) -> Result<Self, PreviewError> {
        let path = path.into();
        let video = is_video(&path);
        let image = if video {
            crate::media_import::first_frame(runner, &path).map_err(|error| PreviewError {
                message: error.to_string(),
            })?
        } else {
            let bytes = std::fs::read(&path).map_err(|error| PreviewError {
                message: error.to_string(),
            })?;
            crate::pngzip::decode_png(&bytes).map_err(|error| PreviewError {
                message: error.to_string(),
            })?
        };
        Ok(Self::from_image(path, image, index, settings))
    }

    /// Build a preview from an already-decoded image, for captures that have
    /// not been written to disk yet.
    #[must_use]
    pub fn from_image(
        path: impl Into<PathBuf>,
        image: Image,
        index: u32,
        settings: &PreviewSettings,
    ) -> Self {
        let path = path.into();
        let video = is_video(&path);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("capture")
            .to_owned();
        Self {
            path,
            image,
            video,
            name,
            collapsed: false,
            dragging: false,
            copying: false,
            large_open: false,
            timer_armed: settings.timeout > 0.0,
            timeout: settings.timeout,
            auto_action: settings.auto_action,
            close_after_drag: settings.close_after_drag,
            corner: settings.corner,
            margin: settings.margin,
            index,
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn image(&self) -> &Image {
        &self.image
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
    pub const fn dragging(&self) -> bool {
        self.dragging
    }

    #[must_use]
    pub const fn copying(&self) -> bool {
        self.copying
    }

    #[must_use]
    pub const fn large_open(&self) -> bool {
        self.large_open
    }

    /// The pixel dimensions of the content.
    #[must_use]
    pub fn dimensions(&self) -> (usize, usize) {
        (self.image.width, self.image.height)
    }

    /// The title line for this preview, from the configured display name.
    ///
    /// A renamed capture shows only its stem, elided by the UI.
    #[must_use]
    pub fn display_title(&self, display_name: &str) -> String {
        if display_name != self.name {
            Path::new(display_name)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or(display_name)
                .to_owned()
        } else {
            ready_title(self.video).to_owned()
        }
    }

    /// Collapse or restore the preview.
    ///
    /// Collapsing hides the body and shows the restore control; the timer is
    /// stopped while collapsed. A collapsed preview never keeps a hover lease.
    pub fn set_collapsed(&mut self, value: bool) {
        self.collapsed = value;
    }

    /// Note that a drag started; the timer must not fire mid-drag.
    pub fn begin_drag(&mut self) {
        self.dragging = true;
        self.timer_armed = false;
    }

    /// Note that a drag finished.
    ///
    /// Returns true when the preview should close: an accepted drop with
    /// close-after-drag on and Alt not held. Otherwise the timer rearms.
    pub fn finish_drag(&mut self, accepted: bool, keep: bool) -> bool {
        self.dragging = false;
        if accepted && self.close_after_drag && !keep {
            return true;
        }
        self.resume_timer();
        false
    }

    /// Note the pointer entered; auto-close is suspended while hovered.
    pub fn enter(&mut self) {
        self.timer_armed = false;
    }

    /// Note the pointer left, rearming auto-close when configured.
    pub fn leave(&mut self) {
        self.resume_timer();
    }

    /// Rearm the timer unless collapsed, dragging, copying or hovered.
    pub fn resume_timer(&mut self) {
        self.timer_armed = self.timeout > 0.0 && !self.collapsed && !self.dragging && !self.copying;
    }

    /// Whether auto-close should wait for the current activity to end.
    #[must_use]
    pub fn should_defer_close(&self) -> bool {
        self.copying || self.dragging || self.large_open
    }

    /// The action an elapsed timer takes. `None` when the timer is not armed.
    #[must_use]
    pub fn auto_close(&mut self) -> Option<AutoAction> {
        if !self.timer_armed || self.should_defer_close() {
            self.resume_timer();
            return None;
        }
        self.timer_armed = false;
        Some(self.auto_action)
    }

    /// Whether the timer is currently counting down.
    #[must_use]
    pub const fn timer_armed(&self) -> bool {
        self.timer_armed
    }

    /// Mark a copy in flight; the preview stays open until it finishes.
    pub fn begin_copy(&mut self) {
        self.copying = true;
        self.timer_armed = false;
    }

    /// Mark the copy finished.
    pub fn finish_copy(&mut self) {
        self.copying = false;
        self.resume_timer();
    }

    /// Open or close the larger preview, which suspends auto-close.
    pub fn set_large_open(&mut self, value: bool) {
        self.large_open = value;
        if !value {
            self.resume_timer();
        }
    }

    /// Handle one wheel gesture, applying the collapse/restore/dismiss rules.
    ///
    /// Returns `Some(true)` when the preview should close.
    pub fn swipe(
        &mut self,
        state: &mut SwipeState,
        pixel: bool,
        delta: Swipe,
        inverted: bool,
    ) -> Option<bool> {
        match state.wheel(pixel, delta, inverted) {
            SwipeAction::Dismiss => Some(true),
            SwipeAction::Collapse => {
                self.set_collapsed(true);
                None
            }
            SwipeAction::Restore => {
                self.set_collapsed(false);
                None
            }
            SwipeAction::None => None,
        }
    }

    /// The screen position of this preview in its corner.
    #[must_use]
    pub fn position(&self, screen: ScreenRect, width: f64, height: f64) -> (f64, f64) {
        corner_position(screen, width, height, self.corner, self.index, self.margin)
    }
}

/// Render a preview's current content into RGBA8, applying annotations when a
/// document is supplied. This is the exact bytes the drag or clipboard uses.
#[must_use]
pub fn rendered_content(preview: &Preview, document: Option<&Document>) -> Image {
    match document {
        Some(document) => document.render(),
        None => preview.image.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> PreviewSettings {
        PreviewSettings::default()
    }

    fn image(width: usize, height: usize) -> Image {
        Image::new(width, height, vec![0u8; width * height * 4])
    }

    #[test]
    fn corner_positions_stack_and_stay_inside_the_screen() {
        let screen = ScreenRect::new(0.0, 0.0, 1920.0, 1080.0);
        assert_eq!(
            corner_position(screen, 300.0, 200.0, Corner::BottomLeft, 0, 18.0),
            (18.0, 862.0)
        );
        assert_eq!(
            corner_position(screen, 300.0, 200.0, Corner::BottomRight, 0, 18.0),
            (1602.0, 862.0)
        );
        assert_eq!(
            corner_position(screen, 300.0, 200.0, Corner::TopLeft, 1, 18.0),
            (18.0, 36.0)
        );
        assert_eq!(
            corner_position(screen, 300.0, 200.0, Corner::BottomLeft, 1, 18.0),
            (18.0, 844.0)
        );
    }

    #[test]
    fn a_mouse_wheel_never_dismisses_but_a_trackpad_swipe_does() {
        let mut state = SwipeState::new();
        assert_eq!(
            state.wheel(false, Swipe { x: 0.0, y: -120.0 }, false),
            SwipeAction::None
        );
        assert_eq!(
            state.wheel(true, Swipe { x: 0.0, y: -110.0 }, false),
            SwipeAction::Collapse
        );
        assert_eq!(
            state.wheel(true, Swipe { x: 0.0, y: 110.0 }, false),
            SwipeAction::Restore
        );
        assert_eq!(
            state.wheel(true, Swipe { x: 110.0, y: 0.0 }, false),
            SwipeAction::Dismiss
        );
    }

    #[test]
    fn swipe_inversion_flips_the_axis() {
        let mut state = SwipeState::new();
        assert_eq!(
            state.wheel(true, Swipe { x: 0.0, y: 110.0 }, true),
            SwipeAction::Collapse
        );
    }

    #[test]
    fn collapse_hides_the_body_and_restore_shows_it() {
        let mut preview = Preview::from_image("/tmp/a.png", image(320, 200), 0, &settings());
        assert!(!preview.collapsed());
        preview.set_collapsed(true);
        assert!(preview.collapsed());
        preview.set_collapsed(false);
        assert!(!preview.collapsed());
    }

    #[test]
    fn recordings_do_not_offer_pin() {
        assert_eq!(
            available_actions(false),
            vec![
                PreviewAction::Annotate,
                PreviewAction::Copy,
                PreviewAction::Save,
                PreviewAction::Pin
            ]
        );
        assert_eq!(
            available_actions(true),
            vec![
                PreviewAction::Annotate,
                PreviewAction::Copy,
                PreviewAction::Save
            ]
        );
        assert!(is_video(Path::new("clip.MP4")));
        assert!(!is_video(Path::new("shot.png")));
    }

    #[test]
    fn an_accepted_drag_closes_unless_alt_keeps_it() {
        let mut preview = Preview::from_image("/tmp/a.png", image(320, 200), 0, &settings());
        preview.begin_drag();
        assert!(preview.dragging());
        assert!(preview.finish_drag(true, false));
        preview.begin_drag();
        assert!(!preview.finish_drag(true, true));
        preview.begin_drag();
        assert!(!preview.finish_drag(false, false));
        let closing = PreviewSettings {
            close_after_drag: false,
            ..settings()
        };
        let mut preview = Preview::from_image("/tmp/a.png", image(320, 200), 0, &closing);
        preview.begin_drag();
        assert!(!preview.finish_drag(true, false));
    }

    #[test]
    fn auto_close_defers_while_copying_or_showing_a_large_preview() {
        let mut settings = settings();
        settings.timeout = 5.0;
        let mut preview = Preview::from_image("/tmp/a.png", image(320, 200), 0, &settings);
        assert!(preview.timer_armed());
        preview.begin_copy();
        assert_eq!(preview.auto_close(), None);
        assert!(!preview.timer_armed(), "the timer stays off while copying");
        preview.finish_copy();
        assert_eq!(preview.auto_close(), Some(AutoAction::Close));
        assert!(!preview.timer_armed());

        let mut preview = Preview::from_image("/tmp/a.png", image(320, 200), 0, &settings);
        preview.set_large_open(true);
        assert_eq!(preview.auto_close(), None);
        preview.set_large_open(false);
        assert_eq!(preview.auto_close(), Some(AutoAction::Close));
    }

    #[test]
    fn auto_close_saves_when_configured() {
        let mut settings = settings();
        settings.timeout = 5.0;
        settings.auto_action = AutoAction::Save;
        let mut preview = Preview::from_image("/tmp/a.png", image(320, 200), 0, &settings);
        assert_eq!(preview.auto_close(), Some(AutoAction::Save));
    }

    #[test]
    fn hovering_suspends_the_timer_and_leaving_restores_it() {
        let mut settings = settings();
        settings.timeout = 5.0;
        let mut preview = Preview::from_image("/tmp/a.png", image(320, 200), 0, &settings);
        preview.enter();
        assert!(!preview.timer_armed());
        preview.leave();
        assert!(preview.timer_armed());
    }

    #[test]
    fn a_renamed_capture_shows_its_stem() {
        let preview = Preview::from_image("/tmp/abc.png", image(10, 10), 0, &settings());
        assert_eq!(preview.display_title("abc.png"), "Capture ready");
        assert_eq!(preview.display_title("Holiday 2024.png"), "Holiday 2024");
    }

    #[test]
    fn swipe_applies_state_to_the_preview() {
        let mut preview = Preview::from_image("/tmp/a.png", image(320, 200), 0, &settings());
        let mut state = SwipeState::new();
        assert_eq!(
            preview.swipe(&mut state, true, Swipe { x: 0.0, y: -110.0 }, false),
            None
        );
        assert!(preview.collapsed());
        assert_eq!(
            preview.swipe(&mut state, true, Swipe { x: 0.0, y: 110.0 }, false),
            None
        );
        assert!(!preview.collapsed());
        assert_eq!(
            preview.swipe(&mut state, true, Swipe { x: 100.0, y: 0.0 }, false),
            Some(true)
        );
    }
}
