//! Visual source framing for an editable recording zoom.
// Ported from omnishot/zoom_focus.py
//!
//! The widget is a preview image with a draggable focus frame. The geometry is
//! pure: given a viewport, the image ratio and the zoom scale, the frame and
//! its corners are deterministic, so the same model backs the inspector and the
//! exported crop.

use crate::annotation::{Point, Rect};

/// The smallest and largest zoom factor the inspector allows.
pub const MIN_SCALE: f64 = 1.0;
/// The largest zoom factor the inspector allows.
pub const MAX_SCALE: f64 = 5.0;
/// The inset the image is drawn with inside the widget.
pub const INSET: f64 = 10.0;

/// A zoom framing model.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoomFocus {
    pub scale: f64,
    pub cx: f64,
    pub cy: f64,
    pub width: f64,
    pub height: f64,
    /// The source image's width/height ratio; `16/9` when unknown.
    pub aspect: f64,
    drag: Option<Drag>,
    before: Option<(f64, f64, f64)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    Move { origin: Point, x: f64, y: f64 },
    Resize { anchor: Point, sx: f64, sy: f64 },
}

impl Default for ZoomFocus {
    fn default() -> Self {
        Self {
            scale: 2.0,
            cx: 0.5,
            cy: 0.5,
            width: 600.0,
            height: 320.0,
            aspect: 16.0 / 9.0,
            drag: None,
            before: None,
        }
    }
}

impl ZoomFocus {
    /// Create a model for a viewport and source image ratio.
    #[must_use]
    pub fn new(width: f64, height: f64, aspect: f64) -> Self {
        Self {
            width,
            height,
            aspect: if aspect.is_finite() && aspect > 0.0 {
                aspect
            } else {
                16.0 / 9.0
            },
            ..Self::default()
        }
    }

    /// Set the scale and center, clamped so the frame stays inside the image.
    ///
    /// The source clamps the scale to `1..=5` and each center to half the
    /// frame's normalized size from the edges.
    // Ported from omnishot/zoom_focus.py (set_values)
    pub fn set_values(&mut self, scale: f64, x: f64, y: f64) {
        self.scale = scale.clamp(MIN_SCALE, MAX_SCALE);
        let half = 0.5 / self.scale;
        self.cx = x.clamp(half, 1.0 - half);
        self.cy = y.clamp(half, 1.0 - half);
    }

    /// The rectangle the image is drawn in.
    #[must_use]
    pub fn image_rect(&self) -> Rect {
        let area = Rect::new(
            INSET,
            INSET,
            (self.width - INSET * 2.0).max(0.0),
            (self.height - INSET * 2.0).max(0.0),
        );
        let width = area.width.min(area.height * self.aspect);
        let height = width / self.aspect;
        Rect::new(
            area.center().x - width / 2.0,
            area.center().y - height / 2.0,
            width,
            height,
        )
    }

    /// The focus frame, in widget coordinates.
    #[must_use]
    pub fn focus_rect(&self) -> Rect {
        let rect = self.image_rect();
        let width = rect.width / self.scale;
        let height = rect.height / self.scale;
        Rect::new(
            rect.x + rect.width * self.cx - width / 2.0,
            rect.y + rect.height * self.cy - height / 2.0,
            width,
            height,
        )
    }

    /// The four corner handles, clockwise from the top-left.
    #[must_use]
    pub fn corners(&self) -> [Point; 4] {
        let rect = self.focus_rect();
        [
            Point::new(rect.x, rect.y),
            Point::new(rect.right(), rect.y),
            Point::new(rect.right(), rect.bottom()),
            Point::new(rect.x, rect.bottom()),
        ]
    }

    /// Map a widget point into normalized image coordinates.
    #[must_use]
    pub fn normalized(&self, point: Point) -> Point {
        let rect = self.image_rect();
        Point::new(
            (point.x - rect.x) / rect.width,
            (point.y - rect.y) / rect.height,
        )
    }

    /// Begin a move or resize gesture.
    ///
    /// A press on a corner starts a resize anchored at the opposite corner; a
    /// press inside the image but outside the frame recenters the frame first.
    // Ported from omnishot/zoom_focus.py (mousePressEvent)
    pub fn press(&mut self, point: Point) {
        self.before = Some((self.scale, self.cx, self.cy));
        let normalized = self.normalized(point);
        for (index, corner) in self.corners().iter().enumerate() {
            if (point.x - corner.x).abs() + (point.y - corner.y).abs() < 15.0 {
                let anchor = self.normalized(self.corners()[(index + 2) % 4]);
                self.drag = Some(Drag::Resize {
                    anchor,
                    sx: if matches!(index, 0 | 3) { -1.0 } else { 1.0 },
                    sy: if matches!(index, 0 | 1) { -1.0 } else { 1.0 },
                });
                return;
            }
        }
        if !self.image_rect().contains(point) {
            return;
        }
        if !self.focus_rect().contains(point) {
            self.set_values(self.scale, normalized.x, normalized.y);
        }
        self.drag = Some(Drag::Move {
            origin: normalized,
            x: self.cx,
            y: self.cy,
        });
    }

    /// Continue a gesture to `point`.
    // Ported from omnishot/zoom_focus.py (mouseMoveEvent)
    pub fn move_to(&mut self, point: Point) {
        match self.drag {
            Some(Drag::Move { origin, x, y }) => {
                let normalized = self.normalized(point);
                self.set_values(
                    self.scale,
                    x + normalized.x - origin.x,
                    y + normalized.y - origin.y,
                );
            }
            Some(Drag::Resize { anchor, sx, sy }) => {
                let normalized = self.normalized(point);
                let size = ((normalized.x - anchor.x) * sx).max((normalized.y - anchor.y) * sy);
                let limit_x = if sx < 0.0 { anchor.x } else { 1.0 - anchor.x };
                let limit_y = if sy < 0.0 { anchor.y } else { 1.0 - anchor.y };
                let size = size.clamp(0.2, limit_x.min(limit_y).max(0.2));
                self.set_values(
                    1.0 / size,
                    anchor.x + sx * size / 2.0,
                    anchor.y + sy * size / 2.0,
                );
            }
            None => {}
        }
    }

    /// End the gesture.
    pub fn release(&mut self) {
        self.drag = None;
    }

    /// Cancel an in-progress gesture, restoring the pre-drag values.
    ///
    /// Returns `true` when a gesture was cancelled.
    // Ported from omnishot/zoom_focus.py (keyPressEvent Escape)
    pub fn cancel(&mut self) -> bool {
        if self.drag.is_none() {
            return false;
        }
        self.drag = None;
        if let Some((scale, x, y)) = self.before {
            self.set_values(scale, x, y);
        }
        true
    }

    /// Nudge the frame by an arrow key. `far` is the Shift modifier.
    // Ported from omnishot/zoom_focus.py (keyPressEvent arrows)
    pub fn nudge(&mut self, dx: f64, dy: f64, far: bool) {
        let step = if far { 0.01 } else { 0.001 };
        self.set_values(self.scale, self.cx + dx * step, self.cy + dy * step);
    }

    /// Whether a gesture is in progress.
    #[must_use]
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_values_clamps_the_frame_inside_the_image() {
        let mut focus = ZoomFocus::new(600.0, 320.0, 2.0);
        focus.set_values(3.0, 1.0, 1.0);
        let image = focus.image_rect();
        let frame = focus.focus_rect();
        assert!(image.x - 0.001 <= frame.x && frame.right() <= image.right() + 0.001);
        assert!(image.y - 0.001 <= frame.y && frame.bottom() <= image.bottom() + 0.001);
        assert_eq!(focus.scale, 3.0);
    }

    #[test]
    fn pressing_inside_recenters_then_drags() {
        let mut focus = ZoomFocus::new(600.0, 320.0, 2.0);
        // Press at the frame center and drag right/down.
        let center = focus.focus_rect().center();
        focus.press(center);
        let before = (focus.cx, focus.cy);
        focus.move_to(Point::new(center.x + 30.0, center.y + 15.0));
        assert!(focus.cx > before.0 && focus.cy > before.1);
        focus.release();

        // A press outside the frame recenters it on the press.
        let mut focus = ZoomFocus::new(600.0, 320.0, 2.0);
        let frame = focus.focus_rect();
        let outside = Point::new(20.0, frame.center().y);
        focus.press(outside);
        assert!(focus.cx < 0.5);
    }

    #[test]
    fn resizing_a_corner_changes_the_scale_and_cancel_restores_it() {
        let mut focus = ZoomFocus::new(600.0, 320.0, 2.0);
        let corner = focus.corners()[2];
        let before = focus.scale;
        focus.press(corner);
        focus.move_to(Point::new(corner.x + 25.0, corner.y + 12.0));
        assert!(focus.scale < before);
        assert!(focus.cancel());
        assert!((focus.scale - before).abs() < 1e-9);
    }

    #[test]
    fn nudge_steps_by_keyboard_and_shift_moves_farther() {
        let mut focus = ZoomFocus::new(600.0, 320.0, 2.0);
        let before = focus.cx;
        focus.nudge(-1.0, 0.0, true);
        assert!((focus.cx - (before - 0.01)).abs() < 1e-6);
        focus.nudge(1.0, 0.0, false);
        assert!((focus.cx - (before - 0.01 + 0.001)).abs() < 1e-6);
    }
}
