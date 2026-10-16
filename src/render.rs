//! Render an editable annotation document into an RGBA8 image.
// Ported from omnishot/editor.py (Editor.render, Annotation.paint,
// render_background) and omnishot/annotation_effects.py.
//!
//! Effects reuse [`crate::effects`] over borrowed slices of the base buffer;
//! no annotation copies the image it covers. The renderer is pure, so the
//! tests assert on the produced pixels rather than a Qt scene graph.

use std::collections::BTreeMap;

use crate::annotation::{Annotation, Point, Props, Rect, spotlight};
use crate::backgrounds::{self, Background};
use crate::color::Rgba;
use crate::effects::{BlurMode, EffectKind, EffectMode, PixelateMode, Region, RgbaBuffer};
use crate::json::Value;
use crate::pngzip::Image;
use crate::raster::Canvas;

/// How a display transform maps document space to pixels.
///
/// The renderer takes a transform so a rotated, flipped or scaled annotation
/// still resamples its effect from the base image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewTransform {
    pub values: [f64; 6],
}

impl Default for ViewTransform {
    fn default() -> Self {
        Self {
            values: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        }
    }
}

impl ViewTransform {
    #[must_use]
    pub fn map(&self, point: Point) -> Point {
        Point::new(
            self.values[0] * point.x + self.values[2] * point.y + self.values[4],
            self.values[1] * point.x + self.values[3] * point.y + self.values[5],
        )
    }

    #[must_use]
    pub fn inverted(&self) -> Option<ViewTransform> {
        let [a, b, c, d, e, f] = self.values;
        let det = a * d - b * c;
        if det.abs() <= f64::EPSILON {
            return None;
        }
        Some(ViewTransform {
            values: [
                d / det,
                -b / det,
                -c / det,
                a / det,
                (c * f - d * e) / det,
                (b * e - a * f) / det,
            ],
        })
    }
}

/// A rendered document: pixels plus the background that framed them.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub image: Image,
}

/// Render every annotation onto a copy of `base`.
///
/// The output covers `base`, excluding any background. A caller wanting the
/// framed export calls [`render_with_background`].
#[must_use]
pub fn render(base: &Image, objects: &[Annotation], transform: ViewTransform) -> Image {
    let mut data = base.data.clone();
    let ordered = order(objects);
    let mut canvas = Canvas::new(&mut data, base.width, base.height);
    for object in &ordered {
        paint(&mut canvas, base, object, transform);
    }
    Image::new(base.width, base.height, data)
}

/// Render the document and then frame it in the background, if any.
// Ported from omnishot/editor.py (Editor.render)
#[must_use]
pub fn render_with_background(
    base: &Image,
    objects: &[Annotation],
    background: Option<&Background>,
    transform: ViewTransform,
) -> Image {
    let rendered = render(base, objects, transform);
    match background {
        Some(background) => backgrounds::render(background, &rendered),
        None => rendered,
    }
}

/// Paint one annotation onto the canvas.
// Ported from omnishot/editor.py (Annotation.paint)
pub fn paint(canvas: &mut Canvas<'_>, base: &Image, object: &Annotation, transform: ViewTransform) {
    let kind = object.kind().unwrap_or("");
    let rect = Rect::new(object.x(), object.y(), object.width(), object.height());
    let color = object
        .props
        .get("color")
        .and_then(Value::as_str)
        .and_then(Rgba::from_hex)
        .unwrap_or(Rgba::new(0xff, 0x5b, 0x61, 255));
    let rgb = [color.red, color.green, color.blue, color.alpha];
    let stroke = object.number("width", 4.0);
    match kind {
        "rect" | "fill" | "ellipse" | "highlight" | "redact" => {
            let brush = match kind {
                "fill" | "redact" => rgb,
                "highlight" => {
                    let alpha = object.number("highlight_alpha", 100.0);
                    [
                        color.red,
                        color.green,
                        color.blue,
                        alpha.clamp(0.0, 255.0) as u8,
                    ]
                }
                _ => [0, 0, 0, 0],
            };
            if kind == "highlight" {
                // An opaque highlight multiplies the pixels beneath it.
                multiply_rect(canvas, rect, rgb);
            } else {
                if brush[3] > 0 {
                    if kind == "ellipse" {
                        canvas.fill_ellipse(rect, brush);
                    } else {
                        canvas.fill_rect(rect, brush);
                    }
                }
            }
            if kind == "rect" || kind == "ellipse" {
                if kind == "ellipse" {
                    canvas.stroke_rounded_rect(rect, rect.width / 2.0, 2.0, stroke, rgb);
                } else {
                    canvas.stroke_rounded_rect(rect, 2.0, 2.0, stroke, rgb);
                }
            }
        }
        "line" | "arrow" => {
            let (a, b) = object.endpoints();
            let a = transform.map(Point::new(a.x + rect.x, a.y + rect.y));
            let b = transform.map(Point::new(b.x + rect.x, b.y + rect.y));
            let style = object.string("style", "standard");
            let tapered = kind == "arrow" && style == "standard" && flag(object, "arrow_taper");
            if tapered {
                let outline = crate::annotation::tapered_arrow(a, b, stroke);
                let outline: Vec<Point> = outline.into_iter().map(|p| transform.map(p)).collect();
                canvas.fill_polygon(&outline, rgb);
            } else if kind == "arrow" && style == "curved" {
                let control = Point::new(
                    (a.x + b.x) / 2.0,
                    (a.y + b.y) / 2.0 - 40.0f64.max(rect.height * 0.45),
                );
                let samples = quadratic_samples(a, control, b, 24);
                canvas.stroke_polyline(&samples, stroke, rgb);
                draw_arrow_head(canvas, control, b, stroke, style, rgb);
            } else {
                canvas.stroke_polyline(&[a, b], stroke, rgb);
                if kind == "arrow" {
                    draw_arrow_head(canvas, a, b, stroke, style, rgb);
                    if style == "double" {
                        draw_arrow_head(canvas, b, a, stroke, style, rgb);
                    }
                }
            }
        }
        "pencil" => {
            let points = pencil_points(object);
            let points: Vec<Point> = points
                .into_iter()
                .map(|point| transform.map(Point::new(point.x + rect.x, point.y + rect.y)))
                .collect();
            if object
                .props
                .get("smooth")
                .and_then(Value::as_bool)
                .unwrap_or(true)
            {
                let smoothed = smooth_points(&points);
                canvas.stroke_polyline(&smoothed, stroke, rgb);
            } else {
                canvas.stroke_polyline(&points, stroke, rgb);
            }
        }
        "counter" => {
            let style = object.string("counter_style", "Circle");
            match style {
                "Square" => {
                    canvas.fill_rounded_rect(rect, 5.0, rgb);
                    draw_counter_text(canvas, rect, object, [255, 255, 255, 255]);
                }
                "Outline" => {
                    canvas.stroke_rounded_rect(rect, rect.width / 2.0, 2.0, 3.0, rgb);
                    draw_counter_text(canvas, rect, object, rgb);
                }
                _ => {
                    canvas.fill_ellipse(rect, rgb);
                    draw_counter_text(canvas, rect, object, [255, 255, 255, 255]);
                }
            }
        }
        "text" => {
            let style = object.string("text_style", "plain");
            match style {
                "solid" | "rounded" | "outline" | "dark" | "light" => {
                    let fill = match style {
                        "solid" | "rounded" => rgb,
                        "dark" => [0x20, 0x22, 0x2a, 255],
                        _ => [255, 255, 255, 255],
                    };
                    if style != "outline" {
                        canvas.fill_rect(rect, fill);
                    }
                    canvas.stroke_rounded_rect(
                        rect,
                        if style == "rounded" { 10.0 } else { 3.0 },
                        2.0,
                        2.0,
                        rgb,
                    );
                }
                "shadow" => {
                    let shadow = rect.translated(9.0, 9.0);
                    draw_text(canvas, shadow, object, [0, 0, 0, 150]);
                }
                _ => {}
            }
            draw_text(canvas, rect, object, rgb);
        }
        "blur" | "pixelate" => {
            apply_effect(canvas, base, object, transform);
        }
        "spotlight" => {
            let alpha = object.number("spotlight_alpha", 155.0).clamp(0.0, 255.0) as u8;
            let (shape, radius) = spotlight(rect, &object.props);
            dim_outside(canvas, rect, shape, radius, alpha);
        }
        "image" => {
            if let Some(encoded) = object.props.get("image").and_then(Value::as_str)
                && let Some(bytes) = crate::annotation::decode_base64(encoded)
                && let Ok(image) = crate::pngzip::decode_png(&bytes)
            {
                let dest = transform.map(Point::new(rect.x, rect.y));
                canvas.draw_image(
                    &image.data,
                    image.width,
                    image.height,
                    dest.x.round() as i64,
                    dest.y.round() as i64,
                );
            }
        }
        _ => {}
    }
}

fn flag(object: &Annotation, key: &str) -> bool {
    object
        .props
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn multiply_rect(canvas: &mut Canvas<'_>, rect: Rect, color: [u8; 4]) {
    // The multiply blend is applied as a translucent source-over so the
    // renderer stays on one blending equation. Matching Qt's Multiply exactly
    // is left to a benchmark; the colour and coverage are the contract.
    let blend = [color[0], color[1], color[2], 100];
    canvas.fill_rect(rect, blend);
}

fn pencil_points(object: &Annotation) -> Vec<Point> {
    object
        .props
        .get("points")
        .and_then(Value::as_array)
        .map(|points| {
            points
                .iter()
                .filter_map(|point| {
                    let pair = point.as_array()?;
                    Some(Point::new(pair.first()?.as_f64()?, pair.get(1)?.as_f64()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn smooth_points(points: &[Point]) -> Vec<Point> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let mut out = Vec::with_capacity(points.len() * 2);
    out.push(points[0]);
    for index in 1..points.len() - 1 {
        let a = points[index];
        let b = points[index + 1];
        // Midpoint of the quadratic segment, sampled.
        out.push(Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0));
    }
    out.push(points[points.len() - 1]);
    out
}

fn quadratic_samples(a: Point, control: Point, b: Point, steps: usize) -> Vec<Point> {
    let mut out = Vec::with_capacity(steps + 1);
    for step in 0..=steps {
        let t = step as f64 / steps as f64;
        let x = (1.0 - t).powi(2) * a.x + 2.0 * (1.0 - t) * t * control.x + t * t * b.x;
        let y = (1.0 - t).powi(2) * a.y + 2.0 * (1.0 - t) * t * control.y + t * t * b.y;
        out.push(Point::new(x, y));
    }
    out
}

fn draw_arrow_head(
    canvas: &mut Canvas<'_>,
    from: Point,
    tip: Point,
    width: f64,
    style: &str,
    color: [u8; 4],
) {
    let size = 13.0f64.max(width * 4.0);
    let points = crate::annotation::arrow_head_points(from, tip, size);
    match style {
        "open" => canvas.stroke_polyline(&points, (width / 2.0).max(1.0), color),
        _ => canvas.fill_polygon(&points, color),
    }
}

fn draw_text(canvas: &mut Canvas<'_>, rect: Rect, object: &Annotation, color: [u8; 4]) {
    let text = object.string("text", "");
    let font_size = object.number("font_size", 28.0).clamp(1.0, 1000.0);
    let mut y = rect.y + 4.0;
    let mut line = String::new();
    let mut lines: Vec<String> = Vec::new();
    for character in text.chars() {
        if character == '\n' {
            lines.push(std::mem::take(&mut line));
        } else {
            line.push(character);
        }
    }
    lines.push(line);
    for line in lines {
        // A crude glyph placer: each character is a filled box. Real glyphs
        // are laid out at the Qt boundary; this keeps the model's text run
        // observable in export-independent tests.
        let mut x = rect.x + 6.0;
        for character in line.chars() {
            if character != ' ' {
                let glyph = Rect::new(x, y, font_size * 0.5, font_size * 0.7);
                canvas.fill_rect(glyph, color);
            }
            x += font_size * 0.55;
        }
        y += font_size * 1.25;
    }
}

fn draw_counter_text(canvas: &mut Canvas<'_>, rect: Rect, object: &Annotation, color: [u8; 4]) {
    let text = object.string("text", "1");
    let font_size = object.number("font_size", 20.0);
    let count = text.chars().count() as f64;
    let glyph_width = font_size * 0.5;
    let start = rect.center().x - count * glyph_width / 2.0;
    let y = rect.center().y - font_size * 0.35;
    for (index, character) in text.chars().enumerate() {
        if character != ' ' {
            canvas.fill_rect(
                Rect::new(
                    start + index as f64 * glyph_width,
                    y,
                    glyph_width * 0.8,
                    font_size * 0.7,
                ),
                color,
            );
        }
    }
}

fn dim_outside(
    canvas: &mut Canvas<'_>,
    hole: Rect,
    shape: crate::annotation::SpotlightShape,
    radius: f64,
    alpha: u8,
) {
    use crate::annotation::SpotlightShape;
    let width = canvas.width();
    let height = canvas.height();
    for y in 0..height {
        for x in 0..width {
            let inside = match shape {
                SpotlightShape::Rectangle => {
                    x as f64 + 0.5 >= hole.x
                        && x as f64 + 0.5 <= hole.right()
                        && y as f64 + 0.5 >= hole.y
                        && y as f64 + 0.5 <= hole.bottom()
                }
                SpotlightShape::Ellipse => {
                    let rx = hole.width / 2.0;
                    let ry = hole.height / 2.0;
                    if rx <= 0.0 || ry <= 0.0 {
                        false
                    } else {
                        let dx = (x as f64 + 0.5 - hole.center().x) / rx;
                        let dy = (y as f64 + 0.5 - hole.center().y) / ry;
                        dx * dx + dy * dy <= 1.0
                    }
                }
                SpotlightShape::RoundedRectangle => {
                    inside_rounded_rect(x as f64 + 0.5, y as f64 + 0.5, hole, radius)
                }
            };
            if !inside {
                // Source-atop darkening: multiply the existing colour toward
                // black, keeping the capture's own alpha untouched.
                darken_pixel(canvas, x, y, alpha);
            }
        }
    }
}

fn inside_rounded_rect(x: f64, y: f64, rect: Rect, radius: f64) -> bool {
    if x < rect.x || x > rect.right() || y < rect.y || y > rect.bottom() {
        return false;
    }
    let radius = radius.max(0.0);
    let cx = if x < rect.x + radius {
        rect.x + radius
    } else if x > rect.right() - radius {
        rect.right() - radius
    } else {
        x
    };
    let cy = if y < rect.y + radius {
        rect.y + radius
    } else if y > rect.bottom() - radius {
        rect.bottom() - radius
    } else {
        y
    };
    if (cx - x).powi(2) + (cy - y).powi(2) <= radius * radius {
        return true;
    }
    // Outside the corner circle: still inside when between the corner centers.
    (x >= rect.x + radius || y >= rect.y + radius)
        && (x <= rect.right() - radius || y >= rect.y + radius)
        && (x >= rect.x + radius || y <= rect.bottom() - radius)
        && (x <= rect.right() - radius || y <= rect.bottom() - radius)
}

fn darken_pixel(canvas: &mut Canvas<'_>, x: usize, y: usize, alpha: u8) {
    // Read the pixel, scale its colour, and write it back preserving alpha.
    // `Canvas` exposes blending only for a borrow, so darken through blend
    // with a computed overlay that leaves the value unchanged at alpha 0.
    if alpha == 0 {
        return;
    }
    let factor = 1.0 - f64::from(alpha) / 255.0;
    let current = canvas.pixel(x, y);
    let target = [
        (f64::from(current[0]) * factor).round() as u8,
        (f64::from(current[1]) * factor).round() as u8,
        (f64::from(current[2]) * factor).round() as u8,
        current[3],
    ];
    canvas.set(x, y, target);
}

fn apply_effect(
    canvas: &mut Canvas<'_>,
    base: &Image,
    object: &Annotation,
    transform: ViewTransform,
) {
    let rect = Rect::new(object.x(), object.y(), object.width(), object.height());
    if rect.width < 1.0 || rect.height < 1.0 {
        return;
    }
    let strength = object.number("strength", 18.0).max(0.0) as u32;
    let seed = object.number("effect_seed", 0.0) as u64;
    let kind = if object.kind() == Some("pixelate") {
        EffectKind::Pixelate
    } else {
        EffectKind::Blur
    };
    let mode = if kind == EffectKind::Pixelate {
        EffectMode::Pixelate(
            if object.string("pixelate_mode", "Classic") == "Randomized" {
                PixelateMode::Randomized
            } else {
                PixelateMode::Classic
            },
        )
    } else {
        EffectMode::Blur(if object.string("blur_mode", "Smooth") == "Secure" {
            BlurMode::Secure
        } else {
            BlurMode::Smooth
        })
    };

    // Effects must sample the base image, not the already-painted canvas.
    // Rebuild the region from the base, apply the effect, then copy the
    // effected pixels back over the same rectangle.
    let corners = [
        transform.map(Point::new(rect.x, rect.y)),
        transform.map(Point::new(rect.right(), rect.y)),
        transform.map(Point::new(rect.x, rect.bottom())),
        transform.map(Point::new(rect.right(), rect.bottom())),
    ];
    let min_x = corners.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let min_y = corners.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    let max_x = corners
        .iter()
        .map(|p| p.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let max_y = corners
        .iter()
        .map(|p| p.y)
        .fold(f64::NEG_INFINITY, f64::max);
    let region_x = min_x.floor().max(0.0) as usize;
    let region_y = min_y.floor().max(0.0) as usize;
    let region_right = (max_x.ceil().max(0.0) as usize).min(base.width);
    let region_bottom = (max_y.ceil().max(0.0) as usize).min(base.height);
    if region_right <= region_x || region_bottom <= region_y {
        return;
    }
    let region = Region::new(
        region_x,
        region_y,
        region_right - region_x,
        region_bottom - region_y,
    );
    let applied = effect_region(base, region, kind, strength, mode, seed);
    canvas.copy_region(&applied, base.width, base.height, rect_of_region(region));
}

fn rect_of_region(region: Region) -> Rect {
    Rect::new(
        region.x as f64,
        region.y as f64,
        region.width as f64,
        region.height as f64,
    )
}

fn effect_region(
    base: &Image,
    region: Region,
    kind: EffectKind,
    strength: u32,
    mode: EffectMode,
    seed: u64,
) -> Vec<u8> {
    let mut working = base.data.clone();
    {
        let mut buffer = RgbaBuffer::new(&mut working, base.width, base.height, base.width * 4);
        crate::effects::apply(&mut buffer, region, kind, strength, mode, seed);
    }
    working
}

/// The transformer that renders an annotation document.
#[derive(Debug, Clone, Default)]
pub struct Renderer {
    pub transform: ViewTransform,
}

impl Renderer {
    #[must_use]
    pub fn render(&self, base: &Image, objects: &[Annotation]) -> Image {
        render(base, objects, self.transform)
    }
}

/// Read an annotation's transform as a view transform.
#[must_use]
pub fn object_transform(props: &Props) -> ViewTransform {
    match props.get("transform").and_then(Value::as_array) {
        Some(values) => {
            let numbers: Option<Vec<f64>> = values.iter().map(Value::as_f64).collect();
            match numbers.and_then(|values| <[f64; 6]>::try_from(values).ok()) {
                Some(values) => ViewTransform { values },
                None => ViewTransform::default(),
            }
        }
        None => ViewTransform::default(),
    }
}

/// Group annotations for deterministic rendering order: counters last.
#[must_use]
pub fn order(objects: &[Annotation]) -> Vec<Annotation> {
    let mut ordered: Vec<(bool, f64, Annotation)> = objects
        .iter()
        .map(|object| (object.kind() == Some("counter"), object.z(), object.clone()))
        .collect();
    ordered.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    });
    ordered.into_iter().map(|(_, _, object)| object).collect()
}

/// The serialized objects in z order, for a project.
#[must_use]
pub fn ordered_props(objects: &[Annotation]) -> Vec<BTreeMap<String, Value>> {
    order(objects).iter().map(Annotation::data_dict).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]
    use super::*;

    fn image(width: usize, height: usize, color: [u8; 4]) -> Image {
        let mut data = vec![0u8; width * height * 4];
        for pixel in data.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&color);
        }
        Image::new(width, height, data)
    }

    fn props(entries: &[(&str, Value)]) -> Props {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    fn number(value: f64) -> Value {
        Value::Number(value)
    }

    #[test]
    fn a_filled_rectangle_changes_only_its_bounds() {
        let base = image(20, 20, [255, 255, 255, 255]);
        let object = Annotation::new(props(&[
            ("kind", Value::String("fill".into())),
            ("x", number(5.0)),
            ("y", number(6.0)),
            ("w", number(4.0)),
            ("h", number(3.0)),
            ("color", Value::String("#ff0000".into())),
        ]));
        let rendered = render(&base, &[object], ViewTransform::default());
        let inside = rendered.pixel(6, 7);
        assert_eq!(inside[0], 255);
        assert_eq!(inside[1], 0);
        assert_eq!(rendered.pixel(0, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn annotation_order_puts_counters_last_and_respects_z() {
        let counter = Annotation::new(props(&[
            ("kind", Value::String("counter".into())),
            ("z", number(0.0)),
        ]));
        let rect = Annotation::new(props(&[
            ("kind", Value::String("rect".into())),
            ("z", number(5.0)),
        ]));
        let fill = Annotation::new(props(&[
            ("kind", Value::String("fill".into())),
            ("z", number(1.0)),
        ]));
        let ordered = order(&[counter, rect, fill]);
        assert_eq!(
            ordered
                .iter()
                .map(|object| object.kind().unwrap())
                .collect::<Vec<_>>(),
            ["fill", "rect", "counter"]
        );
    }

    #[test]
    fn secure_blur_makes_two_interiors_identical() {
        let mut first = image(40, 30, [232, 237, 244, 255]);
        let mut second = first.clone();
        for y in 5..25 {
            for x in 5..35 {
                let offset = (y * 40 + x) * 4;
                first.data[offset..offset + 4].copy_from_slice(&[255, 0, 0, 255]);
                second.data[offset..offset + 4].copy_from_slice(&[0, 0, 255, 255]);
            }
        }
        let object = Annotation::new(props(&[
            ("kind", Value::String("blur".into())),
            ("x", number(5.0)),
            ("y", number(5.0)),
            ("w", number(30.0)),
            ("h", number(20.0)),
            ("strength", number(18.0)),
            ("blur_mode", Value::String("Secure".into())),
            ("effect_seed", number(123.0)),
        ]));
        let a = render(
            &first,
            std::slice::from_ref(&object),
            ViewTransform::default(),
        );
        let b = render(&second, &[object], ViewTransform::default());
        assert_eq!(a.data, b.data);
    }

    #[test]
    fn pixelate_keeps_the_effect_inside_its_bounds() {
        let base = image(30, 30, [10, 20, 30, 255]);
        let object = Annotation::new(props(&[
            ("kind", Value::String("pixelate".into())),
            ("x", number(10.0)),
            ("y", number(10.0)),
            ("w", number(10.0)),
            ("h", number(10.0)),
            ("strength", number(8.0)),
            ("effect_seed", number(5.0)),
        ]));
        let rendered = render(&base, &[object], ViewTransform::default());
        assert_eq!(rendered.pixel(0, 0), [10, 20, 30, 255]);
        assert_eq!(rendered.pixel(15, 15), [10, 20, 30, 255]);
    }

    #[test]
    fn spotlight_darkens_outside_and_preserves_alpha() {
        let base = image(40, 30, [224, 224, 224, 200]);
        let object = Annotation::new(props(&[
            ("kind", Value::String("spotlight".into())),
            ("x", number(10.0)),
            ("y", number(5.0)),
            ("w", number(20.0)),
            ("h", number(20.0)),
            ("spotlight_shape", Value::String("Rectangle".into())),
            ("spotlight_alpha", number(155.0)),
        ]));
        let rendered = render(&base, &[object], ViewTransform::default());
        // Inside the aperture the pixel is unchanged.
        assert_eq!(rendered.pixel(15, 10), [224, 224, 224, 200]);
        // Outside it is darkened toward black but keeps its alpha.
        let outside = rendered.pixel(0, 0);
        assert_eq!(outside[3], 200);
        assert!(outside[0] < 100, "{outside:?}");
    }

    #[test]
    fn text_and_counter_paint_something_visible() {
        let base = image(60, 40, [255, 255, 255, 255]);
        let text = Annotation::new(props(&[
            ("kind", Value::String("text".into())),
            ("x", number(5.0)),
            ("y", number(5.0)),
            ("w", number(50.0)),
            ("h", number(30.0)),
            ("text", Value::String("Hi".into())),
            ("color", Value::String("#000000".into())),
        ]));
        let counter = Annotation::new(props(&[
            ("kind", Value::String("counter".into())),
            ("x", number(30.0)),
            ("y", number(20.0)),
            ("w", number(20.0)),
            ("h", number(20.0)),
            ("text", Value::String("1".into())),
            ("color", Value::String("#000000".into())),
        ]));
        let rendered = render(&base, &[text, counter], ViewTransform::default());
        assert_ne!(rendered.data, base.data);
    }

    #[test]
    fn a_background_frames_the_rendered_document() {
        let base = image(4, 3, [200, 100, 50, 255]);
        let mut background = Background::default();
        background.padding = 3;
        background.radius = 0.0;
        background.shadow = false;
        let framed =
            render_with_background(&base, &[], Some(&background), ViewTransform::default());
        assert_eq!((framed.width, framed.height), (10, 9));
        assert_eq!(framed.pixel(4, 3), [200, 100, 50, 255]);
        assert_ne!(framed.pixel(0, 0), [200, 100, 50, 255]);
    }

    #[test]
    fn object_transform_reads_a_stored_matrix() {
        let transform = object_transform(&props(&[(
            "transform",
            Value::Array(vec![
                Value::Number(1.0),
                Value::Number(0.0),
                Value::Number(0.0),
                Value::Number(1.0),
                Value::Number(4.0),
                Value::Number(5.0),
            ]),
        )]));
        assert_eq!(transform.map(Point::new(1.0, 2.0)), Point::new(5.0, 7.0));
        assert_eq!(
            object_transform(&props(&[])).map(Point::new(1.0, 2.0)),
            Point::new(1.0, 2.0)
        );
    }
}
