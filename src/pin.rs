//! Pinned screenshot geometry and interaction rules.
// Ported from omnishot/widgets.py (Pin), omnishot/pin_controls.py,
// omnishot/pin_shadow.py and omnishot/crop_controls.py.
//!
//! The pin is a frameless top-level window. Everything here is the part that
//! can be decided without a compositor: scale, opacity, the wheel gestures,
//! and where each hover control sits for a given pin size.

use crate::annotation::Rect;

/// The starting maximum side of a new pin.
pub const MAX_START_SIDE: f64 = 520.0;

/// The pin window size for an image, preserving aspect ratio.
// Ported from omnishot/widgets.py (Pin.__init__)
#[must_use]
pub fn initial_size(image_width: usize, image_height: usize) -> (f64, f64) {
    if image_width == 0 || image_height == 0 {
        return (1.0, 1.0);
    }
    let ratio = (MAX_START_SIDE / image_width as f64).min(MAX_START_SIDE / image_height as f64);
    let ratio = ratio.min(1.0);
    let width = (image_width as f64 * ratio).round().max(1.0);
    let height = (image_height as f64 * ratio).round().max(1.0);
    (width, height)
}

/// The magnification of a pin at a given width.
#[must_use]
pub fn display_scale(width: f64, image_width: usize) -> f64 {
    if image_width == 0 {
        1.0
    } else {
        width / image_width as f64
    }
}

/// A pin's interactive state.
#[derive(Debug, Clone)]
pub struct PinState {
    pub image_width: usize,
    pub image_height: usize,
    pub width: f64,
    pub height: f64,
    pub opacity: f64,
    pub locked: bool,
    pub hovered: bool,
    pub dragging: bool,
    pub rounded: bool,
    pub border: bool,
    pub shadow: bool,
}

impl PinState {
    #[must_use]
    pub fn new(image_width: usize, image_height: usize) -> Self {
        let (width, height) = initial_size(image_width, image_height);
        Self {
            image_width,
            image_height,
            width,
            height,
            opacity: 1.0,
            locked: false,
            hovered: false,
            dragging: false,
            rounded: true,
            border: false,
            shadow: false,
        }
    }

    #[must_use]
    pub fn display_scale(&self) -> f64 {
        display_scale(self.width, self.image_width)
    }

    /// Handle a wheel event. `pixel` is the trackpad delta; `angle` is the
    /// traditional wheel delta; `alt` flips to opacity.
    ///
    /// Returns true when the event changed something.
    // Ported from omnishot/widgets.py (Pin.wheelEvent)
    pub fn wheel(
        &mut self,
        pixel_y: f64,
        angle_x: f64,
        angle_y: f64,
        alt: bool,
        trackpad: bool,
    ) -> bool {
        if self.locked {
            return false;
        }
        let delta = if trackpad {
            pixel_y
        } else if angle_y != 0.0 {
            angle_y
        } else if alt {
            angle_x
        } else {
            0.0
        };
        if delta == 0.0 {
            return false;
        }
        if alt || trackpad {
            let amount = if trackpad {
                delta / 500.0
            } else {
                delta / 120.0 * 0.08
            };
            self.opacity = (self.opacity + amount).clamp(0.1, 1.0);
        } else {
            let factor = 1.1f64.powf((delta / 120.0).clamp(-10.0, 10.0));
            let width = (self.width * factor).round().clamp(50.0, 3000.0);
            self.width = width;
            self.height = (width * self.image_height as f64 / self.image_width.max(1) as f64)
                .round()
                .max(1.0);
        }
        true
    }

    /// The control visibility rules for the current state.
    // Ported from omnishot/pin_controls.py (PinControls.sync)
    #[must_use]
    pub fn controls_visible(&self) -> ControlsVisible {
        let normal = self.hovered && !self.locked && !self.dragging;
        ControlsVisible {
            close: normal && self.width >= 50.0,
            scale: normal && self.width >= 120.0,
            drag: normal && self.width >= 64.0 && self.height >= 58.0,
            lock: normal,
            unlock: self.locked,
        }
    }
}

/// Which hover controls a pin shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlsVisible {
    pub close: bool,
    pub scale: bool,
    pub drag: bool,
    pub lock: bool,
    pub unlock: bool,
}

/// The geometry of the hover controls.
// Ported from omnishot/pin_controls.py (PinControls.layout)
#[must_use]
pub fn control_layout(width: f64, height: f64, scale_px: f64) -> ControlLayout {
    let small = width.min(height);
    let margin = if small >= scale_px * 60.0 {
        scale_px * 6.0
    } else {
        2.0
    };
    let size = (scale_px * 22.0).min((small - 4.0) / 2.0).max(8.0);
    ControlLayout {
        close: Rect::new(margin, margin, size, size),
        lock: Rect::new(width - margin - size, margin, size, size),
        unlock_size: size,
        drag: Rect::new((width - 52.0) / 2.0, height - margin - 22.0, 52.0, 22.0),
    }
}

/// Where the hover controls sit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlLayout {
    pub close: Rect,
    pub lock: Rect,
    pub unlock_size: f64,
    pub drag: Rect,
}

/// The rounded-clip radius for a pin.
// Ported from omnishot/widgets.py (Pin.paintEvent)
#[must_use]
pub fn clip_radius(state: &PinState) -> f64 {
    if state.rounded { 8.0 } else { 0.0 }
}

/// The shadow geometry: the pin grown by 12 pixels on every side.
// Ported from omnishot/pin_shadow.py (PinShadow.sync)
#[must_use]
pub fn shadow_bounds(width: f64, height: f64) -> Rect {
    Rect::new(-12.0, -12.0, width + 24.0, height + 24.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_large_image_starts_at_the_cap_and_keeps_aspect() {
        let (width, height) = initial_size(1920, 1080);
        assert_eq!(width, 520.0);
        assert!((width / height - 1920.0 / 1080.0).abs() < 0.01);
    }

    #[test]
    fn small_images_are_not_scaled_up() {
        let (width, height) = initial_size(200, 100);
        assert_eq!((width, height), (200.0, 100.0));
    }

    #[test]
    fn wheel_resizes_but_trackpad_and_alt_change_opacity() {
        let mut pin = PinState::new(320, 200);
        let original = (pin.width, pin.height);
        assert!(pin.wheel(0.0, 0.0, 120.0, false, false));
        assert_ne!((pin.width, pin.height), original);
        assert_eq!(pin.opacity, 1.0);

        let size = (pin.width, pin.height);
        assert!(!pin.wheel(0.0, 120.0, 0.0, false, false));
        assert_eq!((pin.width, pin.height), size);

        // Trackpad vertical delta changes opacity, not size.
        assert!(pin.wheel(-50.0, 0.0, 0.0, false, true));
        assert!(pin.opacity < 1.0);
        assert_eq!((pin.width, pin.height), size);

        pin.locked = true;
        let locked = (pin.width, pin.height, pin.opacity);
        assert!(!pin.wheel(0.0, 0.0, 120.0, false, false));
        assert_eq!((pin.width, pin.height, pin.opacity), locked);
    }

    #[test]
    fn opacity_clamps_between_ten_and_one_hundred_percent() {
        let mut pin = PinState::new(100, 100);
        for _ in 0..100 {
            pin.wheel(0.0, 0.0, -120.0, true, false);
        }
        assert_eq!(pin.opacity, 0.1);
        for _ in 0..100 {
            pin.wheel(0.0, 0.0, 120.0, true, false);
        }
        assert_eq!(pin.opacity, 1.0);
    }

    #[test]
    fn controls_fit_small_pins_and_lock_switches_to_unlock() {
        let mut pin = PinState::new(320, 200);
        pin.hovered = true;
        assert!(pin.controls_visible().drag);
        pin.locked = true;
        let visible = pin.controls_visible();
        assert!(visible.unlock);
        assert!(!visible.lock);
        assert!(!visible.close);
        assert!(!visible.drag);
        for (width, height) in [(50.0, 30.0), (10.0, 520.0), (520.0, 10.0)] {
            let layout = control_layout(width, height, 1.0);
            assert!(layout.close.x >= 0.0 && layout.close.right() <= width.max(1.0));
            assert!(layout.lock.x >= 0.0 && layout.lock.right() <= width.max(1.0));
        }
    }

    #[test]
    fn shadow_bounds_grow_the_pin() {
        assert_eq!(
            shadow_bounds(320.0, 200.0),
            Rect::new(-12.0, -12.0, 344.0, 224.0)
        );
    }
}
