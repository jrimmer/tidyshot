//! Full-screen playback geometry for the editor's preview.
// Ported from omnishot/video_preview.py
//!
//! The QML layer owns the window and controls. The pure part is letterboxing
//! the composited image and the control-bar placement, which must agree with
//! the on-screen preview.

use crate::annotation::Rect;

/// The letterboxed rectangle for `image` inside a `width` x `height` viewport.
///
/// A null image or an empty viewport yields an empty rectangle.
// Ported from omnishot/video_preview.py (FullscreenPreview.image_rect)
#[must_use]
pub fn image_rect(
    image_width: usize,
    image_height: usize,
    viewport_width: f64,
    viewport_height: f64,
) -> Rect {
    if image_width == 0 || image_height == 0 || viewport_width <= 0.0 || viewport_height <= 0.0 {
        return Rect::new(0.0, 0.0, 0.0, 0.0);
    }
    let scale = (viewport_width / image_width as f64).min(viewport_height / image_height as f64);
    let width = image_width as f64 * scale;
    let height = image_height as f64 * scale;
    Rect::new(
        (viewport_width - width) / 2.0,
        (viewport_height - height) / 2.0,
        width,
        height,
    )
}

/// The playback controls' rectangle at the bottom of the viewport.
///
/// The bar is between 320 and 900 px wide, centered, and its height is the
/// controls' size hint.
// Ported from omnishot/video_preview.py (FullscreenPreview.resizeEvent)
#[must_use]
pub fn controls_rect(viewport_width: f64, viewport_height: f64, controls_height: f64) -> Rect {
    let width = 900.0_f64.min((viewport_width - 48.0).max(320.0));
    let x = (viewport_width - width) / 2.0;
    let y = (viewport_height - controls_height - 24.0).max(0.0);
    Rect::new(x, y, width, controls_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_is_letterboxed_and_centered() {
        let rect = image_rect(1920, 1080, 1280.0, 720.0);
        assert!((rect.width - 1280.0).abs() < 1e-9);
        assert!((rect.height - 720.0).abs() < 1e-9);
        assert_eq!((rect.x, rect.y), (0.0, 0.0));

        let tall = image_rect(1080, 1920, 1280.0, 720.0);
        assert!((tall.height - 720.0).abs() < 1e-9);
        assert!(tall.width < 1280.0);
        assert!((tall.x - (1280.0 - tall.width) / 2.0).abs() < 1e-9);
    }

    #[test]
    fn a_null_image_or_empty_viewport_is_empty() {
        assert!(image_rect(0, 0, 1280.0, 720.0).is_empty());
        assert!(image_rect(100, 100, 0.0, 720.0).is_empty());
    }

    #[test]
    fn controls_are_centered_and_clamped_to_the_viewport() {
        let rect = controls_rect(1600.0, 900.0, 60.0);
        assert!((rect.width - 900.0).abs() < 1e-9);
        assert!((rect.x - 350.0).abs() < 1e-9);
        assert!((rect.bottom() - (900.0 - 24.0)).abs() < 1e-9);

        let narrow = controls_rect(320.0, 200.0, 80.0);
        assert!((narrow.width - 320.0).abs() < 1e-9);
        assert_eq!(narrow.y, 96.0);
    }
}
