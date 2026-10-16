//! Background presets and the reversible image background editor.
// Ported from omnishot/backgrounds.py and omnishot/editor.py
// (background_layout, paint_background, render_background).
//!
//! A background is plain data: a two-stop gradient (or a cover image), padding,
//! corner rounding and an optional shadow. The layout is computed from the
//! base image size so the same options always frame the same capture.

use std::collections::BTreeMap;

use crate::annotation::Rect;
use crate::color::Rgba;
use crate::json::Value;
use crate::pngzip::Image;
use crate::raster::{Canvas, rounded_mask};

/// The named local gradient presets, in display order.
pub const PRESETS: &[(&str, &str, &str)] = &[
    ("Lavender", "#9f86ed", "#515dbb"),
    ("Ocean", "#46c7e7", "#184496"),
    ("Sunset", "#ffb56b", "#d65b94"),
    ("Forest", "#8ed7a4", "#245c60"),
    ("Slate", "#566176", "#202733"),
    ("Cream", "#f1e6d4", "#d6bca5"),
    ("Rose", "#f8c4d6", "#c56b91"),
    ("Peach", "#ffe0b6", "#f49891"),
    ("Mint", "#cef8dd", "#64afad"),
    ("Indigo", "#8994f0", "#33386d"),
    ("Glacier", "#e7f5ff", "#93b8d7"),
    ("Dusk", "#bc91c9", "#4b5a91"),
    ("Citrus", "#f6e5a0", "#d7af54"),
    ("Coral", "#ffaaa0", "#d56663"),
    ("Sage", "#d5ddc1", "#8a9d83"),
    ("Sand", "#eddbc0", "#ad927b"),
    ("Midnight", "#344465", "#111827"),
    ("Lilac", "#ebdfff", "#b7a4d9"),
    ("White", "#ffffff", "#ffffff"),
    ("Black", "#151515", "#151515"),
];

/// A resolved background configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Background {
    pub color: Rgba,
    pub color2: Rgba,
    pub padding: i32,
    pub radius: f64,
    pub radius_power: f64,
    pub shadow: bool,
    pub aspect: String,
    pub align: String,
    /// Decoded cover image, when the background uses one.
    pub image_data: Option<Image>,
}

impl Default for Background {
    fn default() -> Self {
        Self {
            color: Rgba::from_hex("#9f86ed").unwrap(),
            color2: Rgba::from_hex("#515dbb").unwrap(),
            padding: 64,
            radius: 12.0,
            radius_power: 2.0,
            shadow: true,
            aspect: "Auto".to_owned(),
            align: "Center".to_owned(),
            image_data: None,
        }
    }
}

impl Background {
    /// A named preset's colors with the default spacing.
    #[must_use]
    pub fn preset(name: &str) -> Option<Self> {
        PRESETS
            .iter()
            .find(|(preset, _, _)| *preset == name)
            .map(|(_, first, second)| Self {
                color: Rgba::from_hex(first).unwrap_or_default(),
                color2: Rgba::from_hex(second).unwrap_or(Rgba::new(0, 0, 0, 255)),
                ..Self::default()
            })
    }

    /// Parse a stored background object.
    // Ported from omnishot/editor.py (load_project background)
    #[must_use]
    pub fn from_json(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let base = Self::default();
        let number = |key: &str, fallback: f64| {
            object
                .get(key)
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite())
                .unwrap_or(fallback)
        };
        let text = |key: &str, fallback: &str| {
            object
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or(fallback)
                .to_owned()
        };
        let image_data = object
            .get("image_data")
            .and_then(Value::as_str)
            .and_then(crate::annotation::decode_base64)
            .and_then(|bytes| crate::pngzip::decode_png(&bytes).ok());
        Some(Self {
            color: object
                .get("color")
                .and_then(Value::as_str)
                .and_then(Rgba::from_hex)
                .unwrap_or(base.color),
            color2: object
                .get("color2")
                .and_then(Value::as_str)
                .and_then(Rgba::from_hex)
                .unwrap_or(base.color2),
            padding: number("padding", 64.0).clamp(0.0, 600.0) as i32,
            radius: number("radius", 12.0).clamp(0.0, 200.0),
            radius_power: number("radius_power", 2.0).clamp(1.0, 10.0),
            shadow: object
                .get("shadow")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            aspect: text("aspect", "Auto"),
            align: text("align", "Center"),
            image_data,
        })
    }

    /// Serialize to the stored background object.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut object = BTreeMap::new();
        object.insert(
            "color".to_owned(),
            Value::String(crate::color::color_text(self.color)),
        );
        object.insert(
            "color2".to_owned(),
            Value::String(crate::color::color_text(self.color2)),
        );
        object.insert("padding".to_owned(), Value::Number(f64::from(self.padding)));
        object.insert("radius".to_owned(), Value::Number(self.radius));
        object.insert("radius_power".to_owned(), Value::Number(self.radius_power));
        object.insert("shadow".to_owned(), Value::Bool(self.shadow));
        object.insert("aspect".to_owned(), Value::String(self.aspect.clone()));
        object.insert("align".to_owned(), Value::String(self.align.clone()));
        if let Some(image) = &self.image_data {
            object.insert(
                "image_data".to_owned(),
                Value::String(crate::annotation::encode_base64(&image.to_png())),
            );
        }
        Value::Object(object)
    }
}

/// The default background for a newly opened image, from stored settings.
// Ported from omnishot/backgrounds.py (default_background)
#[must_use]
pub fn default_background(settings: &BTreeMap<String, Value>) -> Option<Background> {
    let name = settings
        .get("background_preset")
        .and_then(Value::as_str)
        .unwrap_or("None");
    if let Some(background) = Background::preset(name) {
        return Some(background);
    }
    settings
        .get("background_presets")
        .and_then(Value::as_object)
        .and_then(|presets| presets.get(name))
        .and_then(Background::from_json)
}

/// The output size and the content rectangle inside it.
// Ported from omnishot/editor.py (background_layout)
#[must_use]
pub fn layout(
    background: &Background,
    base_width: usize,
    base_height: usize,
) -> (usize, usize, Rect) {
    let pad = background.padding.max(0);
    let mut width = base_width as f64 + f64::from(pad) * 2.0;
    let mut height = base_height as f64 + f64::from(pad) * 2.0;
    if background.aspect != "Auto"
        && let Some((a, b)) = background.aspect.split_once(':')
        && let (Ok(a), Ok(b)) = (a.parse::<f64>(), b.parse::<f64>())
        && a > 0.0
        && b > 0.0
    {
        width = width.max(height * a / b);
        height = height.max(width * b / a);
    }
    let align = &background.align;
    let mut x = (width - base_width as f64) / 2.0;
    let mut y = (height - base_height as f64) / 2.0;
    if align.contains("Left") {
        x = f64::from(pad);
    }
    if align.contains("Right") {
        x = width - base_width as f64 - f64::from(pad);
    }
    if align.contains("Top") {
        y = f64::from(pad);
    }
    if align.contains("Bottom") {
        y = height - base_height as f64 - f64::from(pad);
    }
    (
        width.round().max(1.0) as usize,
        height.round().max(1.0) as usize,
        Rect::new(x, y, base_width as f64, base_height as f64),
    )
}

/// Render the base image onto a background canvas.
// Ported from omnishot/editor.py (render_background)
#[must_use]
pub fn render(background: &Background, base: &Image) -> Image {
    let (width, height, content) = layout(background, base.width, base.height);
    let mut data = vec![0u8; width * height * 4];
    // Fill the whole canvas with the first stop, then the gradient body.
    fill_canvas(&mut data, width, height, background.color);
    if background.image_data.is_none() {
        paint_gradient(&mut data, width, height, background);
    }
    if let Some(cover) = &background.image_data {
        let mut canvas = Canvas::new(&mut data, width, height);
        canvas.draw_image_cover(
            &cover.data,
            cover.width,
            cover.height,
            Rect::new(0.0, 0.0, width as f64, height as f64),
        );
    }
    if background.shadow {
        paint_shadow(
            &mut data,
            width,
            height,
            content,
            background.radius,
            background.radius_power,
        );
    }
    {
        let mut canvas = Canvas::new(&mut data, width, height);
        // The content is clipped to the rounded rectangle by the mask.
        let mask = rounded_mask(
            width,
            height,
            content,
            background.radius,
            background.radius_power,
        );
        for y in 0..base.height {
            for x in 0..base.width {
                let target_x = (content.x as usize) + x;
                let target_y = (content.y as usize) + y;
                if target_x >= width || target_y >= height {
                    continue;
                }
                if mask[target_y * width + target_x] == 0 {
                    continue;
                }
                let offset = (y * base.width + x) * 4;
                canvas.set_raw(
                    target_x,
                    target_y,
                    [
                        base.data[offset],
                        base.data[offset + 1],
                        base.data[offset + 2],
                        base.data[offset + 3],
                    ],
                );
            }
        }
    }
    Image::new(width, height, data)
}

fn fill_canvas(data: &mut [u8], width: usize, height: usize, color: Rgba) {
    let pixel = [color.red, color.green, color.blue, color.alpha];
    for chunk in data.as_chunks_mut::<4>().0 {
        chunk.copy_from_slice(&pixel);
    }
    let _ = (width, height);
}

fn paint_gradient(data: &mut [u8], width: usize, height: usize, background: &Background) {
    let first = [
        f64::from(background.color.red),
        f64::from(background.color.green),
        f64::from(background.color.blue),
    ];
    let second = [
        f64::from(background.color2.red),
        f64::from(background.color2.green),
        f64::from(background.color2.blue),
    ];
    let denominator = (width + height) as f64;
    for y in 0..height {
        for x in 0..width {
            let t = if denominator == 0.0 {
                0.0
            } else {
                ((x + y) as f64 / denominator).clamp(0.0, 1.0)
            };
            let offset = (y * width + x) * 4;
            for channel in 0..3 {
                let value = first[channel] * (1.0 - t) + second[channel] * t;
                data[offset + channel] = value.round().clamp(0.0, 255.0) as u8;
            }
            data[offset + 3] = background.color.alpha;
        }
    }
}

/// Approximate the drop shadow with concentric dark rings outside `content`.
// Ported from omnishot/editor.py (paint_background shadow loop)
fn paint_shadow(
    data: &mut [u8],
    width: usize,
    height: usize,
    content: Rect,
    radius: f64,
    power: f64,
) {
    let mut canvas = Canvas::new(data, width, height);
    for spread in (1..=22i32).rev() {
        let ring = content.adjusted(
            -f64::from(spread),
            -f64::from(spread) / 2.0,
            f64::from(spread),
            f64::from(spread),
        );
        let mask = rounded_mask(width, height, ring, radius + f64::from(spread), power);
        let inner = rounded_mask(width, height, content, radius, power);
        for y in 0..height {
            for x in 0..width {
                if mask[y * width + x] != 0 && inner[y * width + x] == 0 {
                    canvas.blend(x, y, [0, 0, 0, 2]);
                }
            }
        }
    }
}

impl<'a> Canvas<'a> {
    /// Replace a pixel, used when compositing an opaque content layer.
    pub fn set_raw(&mut self, x: usize, y: usize, color: [u8; 4]) {
        self.set(x, y, color);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]
    use super::*;

    #[test]
    fn presets_are_complete_and_named() {
        assert!(PRESETS.len() >= 20);
        let ocean = Background::preset("Ocean").unwrap();
        assert_eq!(ocean.color, Rgba::from_hex("#46c7e7").unwrap());
        assert_eq!(ocean.padding, 64);
        assert!(Background::preset("Missing").is_none());
    }

    #[test]
    fn default_background_resolves_a_named_preset() {
        let settings: BTreeMap<String, Value> = [(
            "background_preset".to_owned(),
            Value::String("Sunset".to_owned()),
        )]
        .into_iter()
        .collect();
        let background = default_background(&settings).unwrap();
        assert_eq!(background.color, Rgba::from_hex("#ffb56b").unwrap());
        assert_eq!(background.color2, Rgba::from_hex("#d65b94").unwrap());
    }

    #[test]
    fn layout_pads_and_aligns_the_content() {
        let mut background = Background::default();
        background.padding = 20;
        background.radius = 0.0;
        background.shadow = false;
        background.aspect = "Auto".to_owned();
        background.align = "Center".to_owned();
        let (w, h, content) = layout(&background, 200, 100);
        assert_eq!((w, h), (240, 140));
        assert_eq!(content, Rect::new(20.0, 20.0, 200.0, 100.0));
        background.align = "Top Left".to_owned();
        let (_, _, content) = layout(&background, 200, 100);
        assert_eq!(content, Rect::new(20.0, 20.0, 200.0, 100.0));
    }

    #[test]
    fn layout_applies_an_aspect_ratio() {
        let mut background = Background::default();
        background.padding = 0;
        background.aspect = "1:1".to_owned();
        let (w, h, _) = layout(&background, 300, 100);
        assert_eq!(w, h);
        assert_eq!(w, 300);
    }

    #[test]
    fn rendering_wraps_the_image_and_round_trips_options() {
        let mut base = Image::new(4, 3, vec![200u8; 4 * 3 * 4]);
        for pixel in base.data.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&[200, 100, 50, 255]);
        }
        let mut background = Background::default();
        background.padding = 4;
        background.radius = 0.0;
        background.shadow = false;
        let rendered = render(&background, &base);
        assert_eq!((rendered.width, rendered.height), (12, 11));
        assert_eq!(rendered.pixel(6, 5), [200, 100, 50, 255]);
        // The padding carries the gradient, not the image.
        assert_ne!(rendered.pixel(0, 0), [200, 100, 50, 255]);

        let json = background.to_json();
        let parsed = Background::from_json(&json).unwrap();
        assert_eq!(parsed.color, background.color);
        assert_eq!(parsed.padding, 4);
    }

    #[test]
    fn a_cover_image_fills_the_canvas() {
        let mut base = Image::new(2, 2, vec![0u8; 16]);
        for pixel in base.data.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&[10, 20, 30, 255]);
        }
        let mut cover = Image::new(2, 2, vec![0u8; 16]);
        for pixel in cover.data.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&[250, 0, 0, 255]);
        }
        let mut background = Background::default();
        background.padding = 2;
        background.radius = 0.0;
        background.shadow = false;
        background.image_data = Some(cover);
        let rendered = render(&background, &base);
        assert_eq!(rendered.pixel(0, 0), [250, 0, 0, 255]);
    }
}
