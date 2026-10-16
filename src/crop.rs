//! Crop session state: aspect ratios, typed sizes and snapping.
// Ported from omnishot/crop.py (CropSession).
//!
//! The session is transactional: it owns a working rectangle and only changes
//! the document when `apply` is requested. Rotate and flip change the
//! document immediately but stay undoable as part of the same session.

use crate::annotation::Rect;
use crate::editor::{self, Document, EditorError};
use crate::pngzip::Image;

/// The aspect options the crop toolbar offers.
pub const ASPECTS: &[&str] = &[
    "Freeform", "Original", "1:1", "16:9", "9:16", "4:3", "3:2", "5:4", "Custom",
];

/// The alignment-free crop state.
#[derive(Debug, Clone)]
pub struct CropState {
    pub rect: Rect,
    pub aspect: String,
    pub ratio_width: f64,
    pub ratio_height: f64,
    pub fill: Option<crate::color::Rgba>,
    pub transformed: bool,
    pub snap: bool,
}

impl CropState {
    #[must_use]
    pub fn new(base: &Image, snap: bool) -> Self {
        Self {
            rect: Rect::new(0.0, 0.0, base.width as f64, base.height as f64),
            aspect: "Freeform".to_owned(),
            ratio_width: 1.0,
            ratio_height: 1.0,
            fill: None,
            transformed: false,
            snap,
        }
    }

    /// The width/height ratio for the current aspect, or `None` when free.
    // Ported from omnishot/crop.py (CropSession.ratio)
    #[must_use]
    pub fn ratio(&self, base: &Image) -> Option<f64> {
        match self.aspect.as_str() {
            "Freeform" => None,
            "Custom" => {
                if self.ratio_height == 0.0 {
                    None
                } else {
                    Some(self.ratio_width / self.ratio_height)
                }
            }
            "Original" => Some(base.width as f64 / base.height.max(1) as f64),
            value => value
                .split_once(':')
                .and_then(|(a, b)| Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok()?)),
        }
    }

    /// Switch to `aspect` and re-fit the working rectangle.
    // Ported from omnishot/crop.py (CropSession.aspect_changed)
    pub fn set_aspect(&mut self, aspect: &str, base: &Image) {
        self.aspect = aspect.to_owned();
        if aspect == "Custom" {
            let (w, h) = (
                self.rect.width.round() as u64,
                self.rect.height.round() as u64,
            );
            let divisor = gcd(w.max(2), h.max(2));
            self.ratio_width = (w.max(2) / divisor) as f64;
            self.ratio_height = (h.max(2) / divisor) as f64;
        }
        if let Some(ratio) = self.ratio(base) {
            let center = self.rect.center();
            let width = self.rect.width.min(self.rect.height * ratio);
            self.rect.width = width;
            self.rect.height = width / ratio;
            self.rect.x = center.x - width / 2.0;
            self.rect.y = center.y - self.rect.height / 2.0;
        }
        self.sync();
    }

    /// Set the width or height explicitly (index 0 is width).
    // Ported from omnishot/crop.py (CropSession.size_changed)
    pub fn size_changed(&mut self, index: usize, value: f64, base: &Image) {
        let value = value.clamp(2.0, 100_000.0);
        let ratio = self.ratio(base);
        if index == 0 {
            self.rect.width = value;
            if let Some(ratio) = ratio {
                self.rect.height = value / ratio;
            }
        } else {
            self.rect.height = value;
            if let Some(ratio) = ratio {
                self.rect.width = value * ratio;
            }
        }
        self.sync();
    }

    /// Swap width and height, transposing the ratio.
    // Ported from omnishot/crop.py (CropSession.swap)
    pub fn swap(&mut self) {
        let center = self.rect.center();
        std::mem::swap(&mut self.rect.width, &mut self.rect.height);
        self.rect.x = center.x - self.rect.width / 2.0;
        self.rect.y = center.y - self.rect.height / 2.0;
        self.transpose_ratio();
        if self.aspect == "Original" {
            self.aspect = "Freeform".to_owned();
        }
        self.sync();
    }

    fn transpose_ratio(&mut self) {
        match self.aspect.as_str() {
            "Custom" => std::mem::swap(&mut self.ratio_width, &mut self.ratio_height),
            value if value.contains(':') => {
                let swapped = value
                    .split_once(':')
                    .map(|(a, b)| format!("{b}:{a}"))
                    .unwrap_or_else(|| value.to_owned());
                self.aspect = swapped;
                self.sync_ratio_fields();
            }
            _ => {}
        }
    }

    fn sync_ratio_fields(&mut self) {
        if let Some((a, b)) = self.aspect.split_once(':')
            && let (Ok(a), Ok(b)) = (a.parse::<f64>(), b.parse::<f64>())
        {
            self.ratio_width = a;
            self.ratio_height = b;
        }
    }

    /// Round the working rectangle to whole pixels.
    pub fn sync(&mut self) {
        self.rect.x = self.rect.x.round();
        self.rect.y = self.rect.y.round();
        self.rect.width = self.rect.width.round().max(1.0);
        self.rect.height = self.rect.height.round().max(1.0);
    }

    /// True when the crop fits the documented limits.
    // Ported from omnishot/crop.py (CropSession.sync valid)
    #[must_use]
    pub fn valid(&self) -> bool {
        let width = self.rect.width;
        let height = self.rect.height;
        (2.0..=100_000.0).contains(&width)
            && (2.0..=100_000.0).contains(&height)
            && width * height <= crate::editor::MAX_PIXELS as f64
    }

    /// Set a custom fill color, or `None` for automatic.
    pub fn set_fill(&mut self, fill: Option<crate::color::Rgba>) {
        self.fill = fill;
    }

    /// Resolve the fill for expansion: explicit, else the base's border.
    // Ported from omnishot/crop.py (CropSession.resolved_fill)
    #[must_use]
    pub fn resolved_fill(&self, base: &Image) -> crate::color::Rgba {
        self.fill.unwrap_or_else(|| editor::background_color(base))
    }

    /// Apply the crop to the document, if valid.
    pub fn apply(&self, document: &mut Document) -> Result<bool, EditorError> {
        if !self.valid() {
            return Err(EditorError::new(
                "Crop is too large. The limit is 120 megapixels.",
            ));
        }
        let changed = self.transformed
            || self.rect
                != Rect::new(
                    0.0,
                    0.0,
                    document.base.width as f64,
                    document.base.height as f64,
                );
        if self.rect
            != Rect::new(
                0.0,
                0.0,
                document.base.width as f64,
                document.base.height as f64,
            )
        {
            document.crop(self.rect, true, Some(self.resolved_fill(&document.base)))?;
        }
        Ok(changed)
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    let (mut a, mut b) = (a, b);
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Image {
        Image::new(100, 50, vec![255u8; 100 * 50 * 4])
    }

    #[test]
    fn original_and_named_ratios_resolve() {
        let image = base();
        let mut state = CropState::new(&image, false);
        assert_eq!(state.ratio(&image), None);
        state.set_aspect("Original", &image);
        assert_eq!(state.ratio(&image), Some(2.0));
        state.set_aspect("16:9", &image);
        assert!((state.ratio(&image).unwrap() - 16.0 / 9.0).abs() < 1e-9);
    }

    #[test]
    fn typed_size_applies_the_ratio_precisely() {
        let image = base();
        let mut state = CropState::new(&image, false);
        state.set_aspect("16:9", &image);
        state.size_changed(0, 640.0, &image);
        assert_eq!((state.rect.width, state.rect.height), (640.0, 360.0));
        state.swap();
        assert_eq!(state.aspect, "9:16");
        assert_eq!((state.rect.width, state.rect.height), (360.0, 640.0));
    }

    #[test]
    fn custom_ratio_drives_the_height() {
        let image = base();
        let mut state = CropState::new(&image, false);
        state.aspect = "Custom".to_owned();
        state.ratio_width = 7.0;
        state.ratio_height = 4.0;
        state.size_changed(0, 840.0, &image);
        assert_eq!((state.rect.width, state.rect.height), (840.0, 480.0));
    }

    #[test]
    fn the_megapixel_limit_disables_apply() {
        let image = base();
        let mut state = CropState::new(&image, false);
        state.rect = Rect::new(0.0, 0.0, 100_000.0, 100_000.0);
        assert!(!state.valid());
    }

    #[test]
    fn applying_a_noop_crop_changes_nothing() {
        let image = base();
        let mut document = Document::new(image.clone(), None, false);
        let state = CropState::new(&image, false);
        assert!(!state.apply(&mut document).unwrap());
        assert_eq!(document.base, image);
    }

    #[test]
    fn applying_a_crop_is_one_undo_step() {
        let image = base();
        let mut document = Document::new(image, None, false);
        let index = document.render().data.len();
        let mut state = CropState::new(&document.base.clone(), false);
        state.rect = Rect::new(10.0, 5.0, 40.0, 20.0);
        assert!(state.apply(&mut document).unwrap());
        assert_eq!((document.base.width, document.base.height), (40, 20));
        document.undo();
        assert_eq!(document.base.data.len(), index);
    }
}
