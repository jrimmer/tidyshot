//! The selection overlay state machine, magnifier sampling and countdown UI.
// Ported from omnishot/selection.py, omnishot/selection_geometry.py and
// omnishot/capture_countdown.py
//!
//! QML draws the overlay and forwards gestures; this module owns the rectangle
//! arithmetic, the aspect lock, the exact-size fields and the magnifier's pixel
//! samples. The same state machine backs the Rust tests, so a drag, a handle
//! resize and a keyboard nudge are verified without a display server.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::geometry::{Driver, Rect, move_selection, resize_selection, size_selection};
use crate::json::Value;
use crate::pngzip::Image;

/// The smallest accepted selection, matching `parse_geometry`.
pub const MIN_SELECTION: i32 = 2;
/// The magnifier's default magnification.
pub const MAGNIFIER_ZOOM: i32 = 6;
/// The magnification bounds the overlay offers.
pub const MIN_ZOOM: i32 = 4;
pub const MAX_ZOOM: i32 = 8;
/// How close the pointer must be to an edge to grab it.
pub const HANDLE_SLACK: i32 = 8;
/// The magnifier inset's side, in logical pixels.
pub const MAGNIFIER_SIZE: usize = 140;

/// A resize handle or a body drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    Move,
}

impl Handle {
    /// The edge token [`resize_selection`] expects.
    #[must_use]
    pub const fn edge(self) -> &'static str {
        match self {
            Self::TopLeft => "lt",
            Self::Top => "t",
            Self::TopRight => "rt",
            Self::Right => "r",
            Self::BottomRight => "rb",
            Self::Bottom => "b",
            Self::BottomLeft => "lb",
            Self::Left => "l",
            Self::Move => "",
        }
    }

    /// The handle under a point, or the body when the point is inside.
    #[must_use]
    pub fn at(rect: Rect, x: i32, y: i32, slack: i32) -> Option<Self> {
        let slack = slack.max(0);
        let near_left = (x - rect.x).abs() <= slack;
        let near_right = (x - rect.right()).abs() <= slack;
        let near_top = (y - rect.y).abs() <= slack;
        let near_bottom = (y - rect.bottom()).abs() <= slack;
        let inside_x = x >= rect.x - slack && x <= rect.right() + slack;
        let inside_y = y >= rect.y - slack && y <= rect.bottom() + slack;
        if inside_x && inside_y {
            if near_left && near_top {
                return Some(Self::TopLeft);
            }
            if near_right && near_top {
                return Some(Self::TopRight);
            }
            if near_left && near_bottom {
                return Some(Self::BottomLeft);
            }
            if near_right && near_bottom {
                return Some(Self::BottomRight);
            }
            if near_top {
                return Some(Self::Top);
            }
            if near_bottom {
                return Some(Self::Bottom);
            }
            if near_left {
                return Some(Self::Left);
            }
            if near_right {
                return Some(Self::Right);
            }
        }
        if x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom() {
            return Some(Self::Move);
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum DragKind {
    New,
    Resize(Handle),
    Move,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    kind: DragKind,
    start_x: i32,
    start_y: i32,
    start_rect: Option<Rect>,
}

/// The overlay's selection state for one output.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    bounds: Rect,
    rect: Option<Rect>,
    drag: Option<Drag>,
    anchor: (i32, i32),
    ratio: Option<f64>,
    minimum: i32,
}

impl Selection {
    /// A selection clamped to one output's logical bounds.
    #[must_use]
    pub fn new(bounds: Rect, initial: Option<Rect>) -> Self {
        let mut selection = Self {
            bounds,
            rect: None,
            drag: None,
            anchor: (bounds.x, bounds.y),
            ratio: None,
            minimum: MIN_SELECTION,
        };
        if let Some(rect) = initial {
            selection.set_rect(rect);
        }
        selection
    }

    /// Build a selection with a smaller minimum, for tests of the geometry.
    #[must_use]
    pub fn with_minimum(bounds: Rect, initial: Option<Rect>, minimum: i32) -> Self {
        let mut selection = Self::new(bounds, initial);
        selection.minimum = minimum.max(1);
        selection
    }

    #[must_use]
    pub const fn bounds(&self) -> Rect {
        self.bounds
    }

    #[must_use]
    pub const fn rect(&self) -> Option<Rect> {
        self.rect
    }

    #[must_use]
    pub const fn ratio(&self) -> Option<f64> {
        self.ratio
    }

    #[must_use]
    pub const fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Replace the rectangle, clamped to the output.
    pub fn set_rect(&mut self, rect: Rect) -> bool {
        let x = rect.x.clamp(self.bounds.x, self.bounds.right() - 1);
        let y = rect.y.clamp(self.bounds.y, self.bounds.bottom() - 1);
        let width = rect.width.clamp(1, self.bounds.right() - x);
        let height = rect.height.clamp(1, self.bounds.bottom() - y);
        self.rect = Some(Rect::new(x, y, width, height));
        true
    }

    /// Lock or clear the aspect ratio.
    ///
    /// Enabling the lock keeps the ratio the rectangle already has; a rectangle
    /// with no area falls back to the output's own ratio.
    pub fn set_aspect(&mut self, enabled: bool) -> Option<f64> {
        if enabled {
            let ratio = self.rect.filter(|rect| rect.height > 0).map_or_else(
                || {
                    if self.bounds.height > 0 {
                        f64::from(self.bounds.width) / f64::from(self.bounds.height)
                    } else {
                        1.0
                    }
                },
                |rect| f64::from(rect.width) / f64::from(rect.height),
            );
            self.ratio = Some(ratio);
        } else {
            self.ratio = None;
        }
        self.ratio
    }

    /// Flip the aspect lock.
    pub fn toggle_aspect(&mut self) -> Option<f64> {
        self.set_aspect(self.ratio.is_none())
    }

    /// The ratio a gesture applies: the lock, or Shift's square constraint.
    fn effective_ratio(&self, shift: bool) -> Option<f64> {
        self.ratio.or(if shift { Some(1.0) } else { None })
    }

    fn clamp_point(&self, x: i32, y: i32) -> (i32, i32) {
        (
            x.clamp(self.bounds.x, self.bounds.right()),
            y.clamp(self.bounds.y, self.bounds.bottom()),
        )
    }

    /// Begin a gesture at a point local to the output.
    pub fn press(&mut self, x: i32, y: i32) -> Handle {
        let (x, y) = self.clamp_point(x, y);
        self.anchor = (x, y);
        if let Some(rect) = self.rect
            && let Some(handle) = Handle::at(rect, x, y, HANDLE_SLACK)
        {
            let kind = match handle {
                Handle::Move => DragKind::Move,
                other => DragKind::Resize(other),
            };
            self.drag = Some(Drag {
                kind,
                start_x: x,
                start_y: y,
                start_rect: Some(rect),
            });
            return handle;
        }
        self.rect = Some(Rect::new(x, y, 0, 0));
        self.drag = Some(Drag {
            kind: DragKind::New,
            start_x: x,
            start_y: y,
            start_rect: None,
        });
        Handle::BottomRight
    }

    /// Update the gesture. `shift` adds the square constraint while dragging.
    pub fn move_to(&mut self, x: i32, y: i32, shift: bool) -> Option<Rect> {
        let (x, y) = self.clamp_point(x, y);
        let ratio = self.effective_ratio(shift);
        let Some(drag) = self.drag else {
            return self.rect;
        };
        let rect = match drag.kind {
            DragKind::New => {
                let (anchor_x, anchor_y) = self.anchor;
                let raw_width = (x - anchor_x).abs().max(1);
                let raw_height = (y - anchor_y).abs().max(1);
                let max_width = if x >= anchor_x {
                    self.bounds.right() - anchor_x
                } else {
                    anchor_x - self.bounds.x
                };
                let max_height = if y >= anchor_y {
                    self.bounds.bottom() - anchor_y
                } else {
                    anchor_y - self.bounds.y
                };
                let (width, height) = crate::geometry::fit_size(
                    raw_width,
                    raw_height,
                    max_width.max(1),
                    max_height.max(1),
                    self.minimum,
                    ratio,
                    Driver::Width,
                );
                let rect_x = if x >= anchor_x {
                    anchor_x
                } else {
                    anchor_x - width
                };
                let rect_y = if y >= anchor_y {
                    anchor_y
                } else {
                    anchor_y - height
                };
                Rect::new(rect_x, rect_y, width, height)
            }
            DragKind::Resize(handle) => resize_selection(
                drag.start_rect
                    .unwrap_or(Rect::new(drag.start_x, drag.start_y, 1, 1)),
                handle.edge(),
                x - drag.start_x,
                y - drag.start_y,
                self.bounds,
                ratio,
                self.minimum,
            ),
            DragKind::Move => move_selection(
                drag.start_rect
                    .unwrap_or(Rect::new(drag.start_x, drag.start_y, 1, 1)),
                x - drag.start_x,
                y - drag.start_y,
                self.bounds,
            ),
        };
        self.rect = Some(rect);
        Some(rect)
    }

    /// Finish the gesture and keep the rectangle.
    pub fn release(&mut self) -> Option<Rect> {
        self.drag = None;
        self.rect
    }

    /// Move the selection by a delta, for arrow keys.
    pub fn nudge(&mut self, dx: i32, dy: i32) -> Option<Rect> {
        let rect = self.rect?;
        let moved = move_selection(rect, dx, dy, self.bounds);
        self.rect = Some(moved);
        Some(moved)
    }

    /// Grow or shrink the selection from its bottom-right corner, for
    /// Shift+arrow.
    pub fn resize_by(&mut self, dx: i32, dy: i32, shift: bool) -> Option<Rect> {
        let rect = self.rect?;
        let resized = resize_selection(
            rect,
            "rb",
            dx,
            dy,
            self.bounds,
            self.effective_ratio(shift),
            self.minimum,
        );
        self.rect = Some(resized);
        Some(resized)
    }

    /// Set an exact size, with the changed field driving the ratio.
    pub fn set_size(&mut self, width: i32, height: i32, driver: Driver) -> Option<Rect> {
        let rect = self.rect?;
        let sized = size_selection(
            rect,
            width.max(1),
            height.max(1),
            self.bounds,
            self.ratio,
            driver,
            self.minimum,
        );
        self.rect = Some(sized);
        Some(sized)
    }

    /// Accept the selection. A rectangle smaller than the minimum is refused.
    pub fn accept(&mut self) -> Option<Rect> {
        self.drag = None;
        self.rect
            .filter(|rect| rect.width >= self.minimum && rect.height >= self.minimum)
    }

    /// Cancel the gesture and clear the rectangle.
    pub fn cancel(&mut self) {
        self.drag = None;
        self.rect = None;
    }
}

/// The sample rectangle a magnifier inset shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MagnifierRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Map a pointer over an output to the source rectangle in the frame image.
#[must_use]
pub fn magnifier_rect(
    pointer_x: f64,
    pointer_y: f64,
    bounds: Rect,
    image_width: usize,
    image_height: usize,
    zoom: i32,
    size: usize,
) -> MagnifierRect {
    let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    let span = (size as f64 / f64::from(zoom)).max(1.0);
    let scale_x = if bounds.width > 0 {
        image_width as f64 / f64::from(bounds.width)
    } else {
        1.0
    };
    let scale_y = if bounds.height > 0 {
        image_height as f64 / f64::from(bounds.height)
    } else {
        1.0
    };
    let source_x = (pointer_x - f64::from(bounds.x)) * scale_x - span / 2.0;
    let source_y = (pointer_y - f64::from(bounds.y)) * scale_y - span / 2.0;
    MagnifierRect {
        x: source_x.clamp(0.0, (image_width as f64 - span).max(0.0)),
        y: source_y.clamp(0.0, (image_height as f64 - span).max(0.0)),
        width: span.min(image_width as f64),
        height: span.min(image_height as f64),
    }
}

/// Sample the frozen frame around a pointer into the magnifier's RGBA image.
///
/// Nearest-neighbour is deliberate: a magnifier must show exact pixels, not a
/// smoothed guess at them.
#[must_use]
pub fn magnifier_sample(
    image: &Image,
    bounds: Rect,
    pointer_x: f64,
    pointer_y: f64,
    zoom: i32,
    size: usize,
) -> Image {
    let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    let size = size.max(1);
    let span = (size as f64 / f64::from(zoom)).max(1.0);
    let scale_x = if bounds.width > 0 {
        image.width as f64 / f64::from(bounds.width)
    } else {
        1.0
    };
    let scale_y = if bounds.height > 0 {
        image.height as f64 / f64::from(bounds.height)
    } else {
        1.0
    };
    let source_x = (pointer_x - f64::from(bounds.x)) * scale_x - span / 2.0;
    let source_y = (pointer_y - f64::from(bounds.y)) * scale_y - span / 2.0;
    let mut out = vec![0u8; size * size * 4];
    for row in 0..size {
        let sy = source_y + row as f64 / f64::from(zoom);
        let image_y = (sy.round() as i64).clamp(0, image.height as i64 - 1) as usize;
        for column in 0..size {
            let sx = source_x + column as f64 / f64::from(zoom);
            let image_x = (sx.round() as i64).clamp(0, image.width as i64 - 1) as usize;
            let pixel = image.pixel(image_x, image_y);
            let offset = (row * size + column) * 4;
            out[offset..offset + 4].copy_from_slice(&pixel);
        }
    }
    Image::new(size, size, out)
}

/// What a selection request asks the overlay to show.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectionRequest {
    pub mode: String,
    pub frame: FrameInfo,
    /// Local window rows for window mode, as the picker projects them.
    pub rows: Vec<String>,
    pub accent: String,
    pub hint: String,
    pub live: bool,
}

/// One output's frozen frame and where it sits.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameInfo {
    pub name: String,
    pub bounds: Rect,
    pub image_width: usize,
    pub image_height: usize,
    pub url: String,
    pub focused: bool,
}

impl FrameInfo {
    /// The JSON object QML reads for the frame.
    #[must_use]
    pub fn json(&self) -> String {
        let mut object = std::collections::BTreeMap::new();
        object.insert(
            "index".to_owned(),
            Value::Number(if self.focused { 0.0 } else { 1.0 }),
        );
        object.insert("name".to_owned(), Value::String(self.name.clone()));
        object.insert("x".to_owned(), Value::Number(f64::from(self.bounds.x)));
        object.insert("y".to_owned(), Value::Number(f64::from(self.bounds.y)));
        object.insert("w".to_owned(), Value::Number(f64::from(self.bounds.width)));
        object.insert("h".to_owned(), Value::Number(f64::from(self.bounds.height)));
        object.insert("iw".to_owned(), Value::Number(self.image_width as f64));
        object.insert("ih".to_owned(), Value::Number(self.image_height as f64));
        object.insert("url".to_owned(), Value::String(self.url.clone()));
        object.insert("focused".to_owned(), Value::Bool(self.focused));
        Value::Object(object).to_string_compact()
    }
}

/// The result the worker waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionOutcome {
    Accepted(Rect),
    Cancelled,
}

#[derive(Debug, Default)]
struct Shared {
    open: bool,
    result: Option<SelectionOutcome>,
    closed: bool,
}

/// The shared request between the worker capture and the Qt overlay.
#[derive(Debug, Clone, Default)]
pub struct SelectionControl {
    inner: Arc<(Mutex<Shared>, Condvar)>,
}

impl SelectionControl {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark the overlay open and clear any previous result.
    pub fn open(&self) {
        let (lock, wake) = &*self.inner;
        let mut shared = lock.lock().expect("selection control poisoned");
        shared.open = true;
        shared.result = None;
        shared.closed = false;
        wake.notify_all();
    }

    /// The user accepted a rectangle.
    pub fn accept(&self, rect: Rect) {
        if rect.width < MIN_SELECTION || rect.height < MIN_SELECTION {
            return;
        }
        self.finish(SelectionOutcome::Accepted(rect));
    }

    /// The user dismissed the overlay.
    pub fn cancel(&self) {
        self.finish(SelectionOutcome::Cancelled);
    }

    fn finish(&self, outcome: SelectionOutcome) {
        let (lock, wake) = &*self.inner;
        let mut shared = lock.lock().expect("selection control poisoned");
        if shared.closed {
            return;
        }
        shared.open = false;
        shared.result = Some(outcome);
        wake.notify_all();
    }

    /// Wait for a result, or `None` when `timeout` elapses.
    #[must_use]
    pub fn wait(&self, timeout: Duration) -> Option<SelectionOutcome> {
        let (lock, wake) = &*self.inner;
        let mut shared = lock.lock().expect("selection control poisoned");
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(result) = shared.result {
                return Some(result);
            }
            if shared.closed {
                return Some(SelectionOutcome::Cancelled);
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let (next, _) = wake
                .wait_timeout(shared, remaining)
                .expect("selection control poisoned");
            shared = next;
        }
    }

    /// Stop waiting and drop any queued result.
    pub fn close(&self) {
        let (lock, wake) = &*self.inner;
        let mut shared = lock.lock().expect("selection control poisoned");
        shared.closed = true;
        shared.open = false;
        wake.notify_all();
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        let (lock, _) = &*self.inner;
        lock.lock().expect("selection control poisoned").open
    }
}

/// The countdown label's cancel flag and current text.
#[derive(Debug, Clone, Default)]
pub struct CountdownControl {
    cancelled: Arc<AtomicBool>,
}

impl CountdownControl {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The Cancel button or Escape.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Arm a fresh countdown.
    pub fn reset(&self) {
        self.cancelled.store(false, Ordering::SeqCst);
    }
}

/// The seconds a countdown label shows, parsed from its text.
#[must_use]
pub fn seconds_from_label(label: &str) -> i64 {
    label
        .split(|character: char| !character.is_ascii_digit())
        .find_map(|part| part.parse::<i64>().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Rect {
        Rect::new(0, 0, 1000, 800)
    }

    #[test]
    fn a_drag_builds_the_rectangle_from_its_anchor() {
        let mut selection = Selection::new(bounds(), None);
        selection.press(100, 120);
        let rect = selection.move_to(300, 420, false).unwrap();
        assert_eq!(rect, Rect::new(100, 120, 200, 300));
        assert_eq!(selection.release(), Some(rect));
        // A drag in the opposite direction still yields a positive rectangle.
        selection.press(500, 500);
        assert_eq!(
            selection.move_to(400, 350, false).unwrap(),
            Rect::new(400, 350, 100, 150)
        );
    }

    #[test]
    fn a_drag_never_leaves_the_output() {
        let mut selection = Selection::new(bounds(), None);
        selection.press(900, 700);
        let rect = selection.move_to(5000, 5000, false).unwrap();
        assert_eq!(rect, Rect::new(900, 700, 100, 100));
        selection.press(50, 60);
        let rect = selection.move_to(-500, -500, false).unwrap();
        assert_eq!(rect, Rect::new(0, 0, 50, 60));
    }

    #[test]
    fn corners_resize_after_release_and_keep_the_opposite_edge() {
        let mut selection = Selection::new(bounds(), Some(Rect::new(100, 100, 200, 100)));
        assert_eq!(selection.press(300, 200), Handle::BottomRight);
        let resized = selection.move_to(400, 250, false).unwrap();
        assert_eq!(resized.right(), 400);
        assert_eq!(resized.bottom(), 250);
        assert_eq!(resized.x, 100);
        assert_eq!(resized.y, 100);
        assert_eq!(selection.release(), Some(resized));
        // After a release the handle is grabbable again.
        assert_eq!(selection.press(100, 100), Handle::TopLeft);
        let resized = selection.move_to(150, 140, false).unwrap();
        assert_eq!(resized.right(), 400);
        assert_eq!(resized.bottom(), 250);
        assert_eq!(resized.x, 150);
        assert_eq!(resized.y, 140);
    }

    #[test]
    fn a_body_drag_moves_without_resizing() {
        let mut selection = Selection::new(bounds(), Some(Rect::new(100, 100, 200, 100)));
        assert_eq!(selection.press(200, 150), Handle::Move);
        let moved = selection.move_to(260, 190, false).unwrap();
        assert_eq!(moved, Rect::new(160, 140, 200, 100));
        // The clamp stops it at the output edge.
        let moved = selection.move_to(5000, 5000, false).unwrap();
        assert_eq!(moved, Rect::new(800, 700, 200, 100));
    }

    #[test]
    fn shift_squares_a_drag_and_the_lock_keeps_a_ratio() {
        let mut selection = Selection::new(bounds(), None);
        selection.press(100, 100);
        let square = selection.move_to(400, 200, true).unwrap();
        assert_eq!(square.width, square.height);
        selection.release();
        selection.set_rect(Rect::new(100, 100, 400, 200));
        assert_eq!(selection.set_aspect(true), Some(2.0));
        let resized = selection.move_to(500, 500, false).unwrap();
        assert!((f64::from(resized.width) - f64::from(resized.height) * 2.0).abs() <= 2.0);
        selection.release();
        assert_eq!(selection.set_aspect(false), None);
    }

    #[test]
    fn typed_sizes_drive_the_ratio_and_nudge_keeps_the_size() {
        let mut selection = Selection::with_minimum(
            Rect::new(0, 0, 1800, 1125),
            Some(Rect::new(120, 140, 640, 400)),
            2,
        );
        selection.ratio = Some(16.0 / 9.0);
        assert_eq!(
            selection.set_size(800, 400, Driver::Width),
            Some(Rect::new(120, 140, 800, 450))
        );
        assert_eq!(
            selection.set_size(800, 360, Driver::Height),
            Some(Rect::new(120, 140, 640, 360))
        );
        let moved = selection.nudge(5000, 5000).unwrap();
        assert_eq!((moved.width, moved.height), (640, 360));
        assert_eq!(moved.x, 1160);
    }

    #[test]
    fn arrows_resize_from_the_bottom_right_and_stay_inside() {
        let mut selection = Selection::new(bounds(), Some(Rect::new(100, 100, 200, 100)));
        assert_eq!(
            selection.resize_by(50, 25, false),
            Some(Rect::new(100, 100, 250, 125))
        );
        assert_eq!(
            selection.resize_by(-5000, -5000, false),
            Some(Rect::new(100, 100, 2, 2))
        );
        let mut fixed = Selection::new(bounds(), Some(Rect::new(100, 100, 200, 100)));
        let square = fixed.resize_by(100, 10, true).unwrap();
        assert_eq!(square.width, square.height);
    }

    #[test]
    fn accept_refuses_an_empty_rectangle_and_cancel_clears_it() {
        let mut selection = Selection::new(bounds(), Some(Rect::new(10, 10, 100, 100)));
        assert_eq!(selection.accept(), Some(Rect::new(10, 10, 100, 100)));
        selection.set_rect(Rect::new(10, 10, 1, 1));
        assert_eq!(selection.accept(), None);
        selection.set_rect(Rect::new(10, 10, 100, 100));
        selection.cancel();
        assert_eq!(selection.rect(), None);
        assert!(!selection.dragging());
    }

    #[test]
    fn geometry_matches_the_selection_geometry_cases() {
        // The same assertions the selection_geometry suite makes, now through
        // the overlay state machine.
        let bounds = Rect::new(-1800, -200, 3720, 1325);
        let original = Rect::new(-300, 100, 640, 400);
        for edge in ["lt", "t", "rt", "r", "rb", "b", "lb", "l"] {
            let handle = match edge {
                "lt" => Handle::TopLeft,
                "t" => Handle::Top,
                "rt" => Handle::TopRight,
                "r" => Handle::Right,
                "rb" => Handle::BottomRight,
                "b" => Handle::Bottom,
                "lb" => Handle::BottomLeft,
                _ => Handle::Left,
            };
            for (dx, dy) in [(130, 80), (-130, -80), (5000, 5000), (-5000, -5000)] {
                // The overlay clamps the pointer to the output before it
                // computes the delta, so the reference uses the same point.
                let target_x = (original.x + dx).clamp(bounds.x, bounds.right());
                let target_y = (original.y + dy).clamp(bounds.y, bounds.bottom());
                let (dx, dy) = (target_x - original.x, target_y - original.y);
                let expected = crate::geometry::resize_selection(
                    original,
                    edge,
                    dx,
                    dy,
                    bounds,
                    Some(1.6),
                    40,
                );
                let mut selection = Selection::with_minimum(bounds, Some(original), 40);
                selection.ratio = Some(1.6);
                selection.drag = Some(Drag {
                    kind: DragKind::Resize(handle),
                    start_x: original.x,
                    start_y: original.y,
                    start_rect: Some(original),
                });
                let result = selection
                    .move_to(original.x + dx, original.y + dy, false)
                    .unwrap();
                assert_eq!(result, expected, "{edge} {dx},{dy}");
            }
        }
        // The typed-size and move cases go through the same helpers.
        let sized = crate::geometry::size_selection(
            original,
            800,
            400,
            bounds,
            Some(16.0 / 9.0),
            Driver::Width,
            2,
        );
        let mut selection = Selection::with_minimum(bounds, Some(original), 2);
        selection.set_aspect(true);
        selection.ratio = Some(16.0 / 9.0);
        assert_eq!(selection.set_size(800, 400, Driver::Width), Some(sized));
        assert_eq!(
            selection.nudge(5000, 5000),
            Some(crate::geometry::move_selection(sized, 5000, 5000, bounds))
        );
    }

    #[test]
    fn the_magnifier_samples_exact_pixels_and_clamps_at_the_edges() {
        let mut data = vec![0u8; 40 * 40 * 4];
        for y in 0..40 {
            for x in 0..40 {
                let offset = (y * 40 + x) * 4;
                data[offset] = (x * 6) as u8;
                data[offset + 1] = (y * 6) as u8;
                data[offset + 2] = 7;
                data[offset + 3] = 255;
            }
        }
        let image = Image::new(40, 40, data);
        let sample = magnifier_sample(
            &image,
            Rect::new(0, 0, 100, 100),
            50.0,
            50.0,
            MAGNIFIER_ZOOM,
            30,
        );
        assert_eq!(sample.width, 30);
        assert_eq!(sample.height, 30);
        // The centre of the sample is the pixel under the pointer.
        assert_eq!(sample.pixel(15, 15), image.pixel(20, 20));
        // Sampling past the frame clamps instead of wrapping.
        let edge = magnifier_sample(&image, Rect::new(0, 0, 100, 100), -50.0, -50.0, 4, 16);
        assert_eq!(edge.pixel(0, 0), image.pixel(0, 0));
        let rect = magnifier_rect(50.0, 50.0, Rect::new(0, 0, 100, 100), 40, 40, 4, 16);
        assert!(rect.x >= 0.0 && rect.y >= 0.0);
        assert!(rect.x + rect.width <= 40.0 && rect.y + rect.height <= 40.0);
    }

    #[test]
    fn the_selection_control_accepts_cancels_and_times_out() {
        let control = SelectionControl::new();
        control.open();
        assert!(control.is_open());
        control.accept(Rect::new(10, 10, 100, 100));
        assert_eq!(
            control.wait(Duration::from_millis(1)),
            Some(SelectionOutcome::Accepted(Rect::new(10, 10, 100, 100)))
        );
        control.open();
        control.cancel();
        assert_eq!(
            control.wait(Duration::from_millis(1)),
            Some(SelectionOutcome::Cancelled)
        );
        // Too small an area is not a result.
        control.open();
        control.accept(Rect::new(0, 0, 1, 1));
        assert_eq!(control.wait(Duration::from_millis(1)), None);
        control.close();
        assert_eq!(
            control.wait(Duration::from_millis(1)),
            Some(SelectionOutcome::Cancelled)
        );
    }

    #[test]
    fn the_countdown_control_reports_and_resets_cancel() {
        let control = CountdownControl::new();
        assert!(!control.cancelled());
        control.cancel();
        assert!(control.cancelled());
        control.reset();
        assert!(!control.cancelled());
        assert_eq!(seconds_from_label("Capturing in 3…"), 3);
        assert_eq!(seconds_from_label(""), 0);
    }

    #[test]
    fn a_frame_projects_the_json_the_overlay_reads() {
        let frame = FrameInfo {
            name: "DP-1".to_owned(),
            bounds: Rect::new(0, 0, 1920, 1080),
            image_width: 3840,
            image_height: 2160,
            url: "file:///tmp/frozen.png".to_owned(),
            focused: true,
        };
        let json = frame.json();
        assert!(json.contains("\"name\":\"DP-1\""), "{json}");
        assert!(json.contains("\"iw\":3840"), "{json}");
        assert!(json.contains("\"focused\":true"), "{json}");
    }
}
