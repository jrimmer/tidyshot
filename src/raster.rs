//! Raster drawing over borrowed RGBA8 buffers.
// Ported from the QPainter primitives omniShot draws annotations with in
// omnishot/editor.py, omnishot/backgrounds.py and omnishot/rounded_corners.py.
//!
//! Everything here works on straight (non-premultiplied) RGBA8 and blends
//! with `SourceOver`, so annotation output matches the image codec boundary.

use crate::annotation::{Point, Rect};

/// A mutable RGBA8 canvas with a tight stride.
pub struct Canvas<'a> {
    data: &'a mut [u8],
    width: usize,
    height: usize,
}

impl<'a> Canvas<'a> {
    #[must_use]
    pub fn new(data: &'a mut [u8], width: usize, height: usize) -> Self {
        debug_assert!(data.len() >= width * height * 4);
        Self {
            data,
            width,
            height,
        }
    }

    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> usize {
        self.height
    }

    /// Source-over blend one pixel. `color` is straight RGBA8.
    pub fn blend(&mut self, x: usize, y: usize, color: [u8; 4]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = (y * self.width + x) * 4;
        let source = [
            f64::from(color[0]),
            f64::from(color[1]),
            f64::from(color[2]),
            f64::from(color[3]) / 255.0,
        ];
        for channel in 0..3 {
            let destination = f64::from(self.data[offset + channel]);
            let value = source[channel] * source[3] + destination * (1.0 - source[3]);
            self.data[offset + channel] = value.round().clamp(0.0, 255.0) as u8;
        }
        let destination_alpha = f64::from(self.data[offset + 3]) / 255.0;
        let alpha = source[3] + destination_alpha * (1.0 - source[3]);
        self.data[offset + 3] = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
    }

    /// Replace a pixel, ignoring the previous value.
    pub fn set(&mut self, x: usize, y: usize, color: [u8; 4]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = (y * self.width + x) * 4;
        self.data[offset..offset + 4].copy_from_slice(&color);
    }

    /// Read a pixel.
    #[must_use]
    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let offset = (y * self.width + x) * 4;
        [
            self.data[offset],
            self.data[offset + 1],
            self.data[offset + 2],
            self.data[offset + 3],
        ]
    }

    /// Copy an axis-aligned rectangle of `source` into this canvas, replacing
    /// the destination pixels.
    pub fn copy_region(
        &mut self,
        source: &[u8],
        source_width: usize,
        source_height: usize,
        rect: Rect,
    ) {
        let x0 = rect.x.floor().max(0.0) as usize;
        let y0 = rect.y.floor().max(0.0) as usize;
        let x1 = (rect.right().ceil().max(0.0) as usize).min(self.width);
        let y1 = (rect.bottom().ceil().max(0.0) as usize).min(self.height);
        for y in y0..y1 {
            for x in x0..x1 {
                if x >= source_width || y >= source_height {
                    continue;
                }
                let offset = (y * source_width + x) * 4;
                self.set(
                    x,
                    y,
                    [
                        source[offset],
                        source[offset + 1],
                        source[offset + 2],
                        source[offset + 3],
                    ],
                );
            }
        }
    }

    /// Fill a rectangle with rounded corners.
    pub fn fill_rounded_rect(&mut self, rect: Rect, radius: f64, color: [u8; 4]) {
        let mask = rounded_mask(self.width, self.height, rect, radius, 2.0);
        let x0 = rect.x.floor().max(0.0) as usize;
        let y0 = rect.y.floor().max(0.0) as usize;
        let x1 = (rect.right().ceil().max(0.0) as usize).min(self.width);
        let y1 = (rect.bottom().ceil().max(0.0) as usize).min(self.height);
        for y in y0..y1 {
            for x in x0..x1 {
                if mask[y * self.width + x] != 0 {
                    self.blend(x, y, color);
                }
            }
        }
    }

    /// Source-over composite an image at `(dx, dy)`.
    pub fn draw_image(
        &mut self,
        source: &[u8],
        source_width: usize,
        source_height: usize,
        dx: i64,
        dy: i64,
    ) {
        for row in 0..source_height {
            let target_y = dy + row as i64;
            if target_y < 0 || target_y >= self.height as i64 {
                continue;
            }
            for column in 0..source_width {
                let target_x = dx + column as i64;
                if target_x < 0 || target_x >= self.width as i64 {
                    continue;
                }
                let offset = (row * source_width + column) * 4;
                self.blend(
                    target_x as usize,
                    target_y as usize,
                    [
                        source[offset],
                        source[offset + 1],
                        source[offset + 2],
                        source[offset + 3],
                    ],
                );
            }
        }
    }

    /// Draw `source` scaled to cover `dest`, cropping the overflow.
    // Ported from omnishot/editor.py (paint_background image branch)
    pub fn draw_image_cover(
        &mut self,
        source: &[u8],
        source_width: usize,
        source_height: usize,
        dest: Rect,
    ) {
        if source_width == 0 || source_height == 0 || dest.width <= 0.0 || dest.height <= 0.0 {
            return;
        }
        let ratio = (dest.width / source_width as f64).max(dest.height / source_height as f64);
        let sample_width = dest.width / ratio;
        let sample_height = dest.height / ratio;
        let offset_x = (source_width as f64 - sample_width) / 2.0;
        let offset_y = (source_height as f64 - sample_height) / 2.0;
        let target_x0 = dest.x.floor() as i64;
        let target_y0 = dest.y.floor() as i64;
        let target_x1 = dest.right().ceil() as i64;
        let target_y1 = dest.bottom().ceil() as i64;
        for target_y in target_y0..target_y1 {
            if target_y < 0 || target_y >= self.height as i64 {
                continue;
            }
            let v = (target_y as f64 + 0.5 - dest.y) / dest.height;
            let source_y = (offset_y + v * sample_height).floor() as i64;
            for target_x in target_x0..target_x1 {
                if target_x < 0 || target_x >= self.width as i64 {
                    continue;
                }
                let u = (target_x as f64 + 0.5 - dest.x) / dest.width;
                let source_x = (offset_x + u * sample_width).floor() as i64;
                if source_x < 0
                    || source_y < 0
                    || source_x >= source_width as i64
                    || source_y >= source_height as i64
                {
                    continue;
                }
                let offset = (source_y as usize * source_width + source_x as usize) * 4;
                self.blend(
                    target_x as usize,
                    target_y as usize,
                    [
                        source[offset],
                        source[offset + 1],
                        source[offset + 2],
                        source[offset + 3],
                    ],
                );
            }
        }
    }

    /// Fill a rectangle.
    pub fn fill_rect(&mut self, rect: Rect, color: [u8; 4]) {
        let x0 = rect.x.floor().max(0.0) as usize;
        let y0 = rect.y.floor().max(0.0) as usize;
        let x1 = (rect.right().ceil().max(0.0) as usize).min(self.width);
        let y1 = (rect.bottom().ceil().max(0.0) as usize).min(self.height);
        for y in y0..y1 {
            for x in x0..x1 {
                self.blend(x, y, color);
            }
        }
    }

    /// Fill an axis-aligned ellipse inscribed in `rect`.
    pub fn fill_ellipse(&mut self, rect: Rect, color: [u8; 4]) {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }
        let cx = rect.center().x;
        let cy = rect.center().y;
        let rx = rect.width / 2.0;
        let ry = rect.height / 2.0;
        let y0 = rect.y.floor().max(0.0) as usize;
        let y1 = (rect.bottom().ceil().max(0.0) as usize).min(self.height);
        for y in y0..y1 {
            let dy = (y as f64 + 0.5 - cy) / ry;
            if dy.abs() > 1.0 {
                continue;
            }
            let span = (1.0 - dy * dy).sqrt() * rx;
            let x0 = ((cx - span).floor().max(0.0) as usize).min(self.width);
            let x1 = ((cx + span).ceil().max(0.0) as usize).min(self.width);
            for x in x0..x1 {
                self.blend(x, y, color);
            }
        }
    }

    /// Fill a closed polygon using the even-odd rule.
    pub fn fill_polygon(&mut self, points: &[Point], color: [u8; 4]) {
        if points.len() < 3 {
            return;
        }
        let mut top = f64::INFINITY;
        let mut bottom = f64::NEG_INFINITY;
        for point in points {
            top = top.min(point.y);
            bottom = bottom.max(point.y);
        }
        let y0 = top.floor().max(0.0) as usize;
        let y1 = (bottom.ceil().max(0.0) as usize).min(self.height);
        let mut crossings: Vec<f64> = Vec::new();
        for y in y0..y1 {
            let scan = y as f64 + 0.5;
            crossings.clear();
            for index in 0..points.len() {
                let a = points[index];
                let b = points[(index + 1) % points.len()];
                if (a.y <= scan && b.y > scan) || (b.y <= scan && a.y > scan) {
                    let t = (scan - a.y) / (b.y - a.y);
                    crossings.push(a.x + t * (b.x - a.x));
                }
            }
            crossings.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            for pair in crossings.as_chunks::<2>().0 {
                let x0 = pair[0].floor().max(0.0) as usize;
                let x1 = (pair[1].ceil().max(0.0) as usize).min(self.width);
                for x in x0..x1 {
                    if x as f64 + 0.5 >= pair[0] && (x as f64 + 0.5) <= pair[1] {
                        self.blend(x, y, color);
                    }
                }
            }
        }
    }

    /// Stroke a polyline with a round-capped, `width`-pixel pen.
    pub fn stroke_polyline(&mut self, points: &[Point], width: f64, color: [u8; 4]) {
        if points.is_empty() {
            return;
        }
        if points.len() == 1 {
            self.fill_ellipse(
                Rect::new(
                    points[0].x - width / 2.0,
                    points[0].y - width / 2.0,
                    width,
                    width,
                ),
                color,
            );
            return;
        }
        let radius = (width / 2.0).max(0.5);
        for segment in points.windows(2) {
            self.stroke_segment(segment[0], segment[1], radius, color);
        }
        for point in points {
            self.fill_ellipse(
                Rect::new(
                    point.x - radius,
                    point.y - radius,
                    radius * 2.0,
                    radius * 2.0,
                ),
                color,
            );
        }
    }

    fn stroke_segment(&mut self, a: Point, b: Point, radius: f64, color: [u8; 4]) {
        let x0 = (a.x.min(b.x) - radius - 1.0).floor().max(0.0) as usize;
        let y0 = (a.y.min(b.y) - radius - 1.0).floor().max(0.0) as usize;
        let x1 = ((a.x.max(b.x) + radius + 1.0).ceil().max(0.0) as usize).min(self.width);
        let y1 = ((a.y.max(b.y) + radius + 1.0).ceil().max(0.0) as usize).min(self.height);
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let length_squared = dx * dx + dy * dy;
        for y in y0..y1 {
            for x in x0..x1 {
                let px = x as f64 + 0.5;
                let py = y as f64 + 0.5;
                let t = if length_squared == 0.0 {
                    0.0
                } else {
                    (((px - a.x) * dx + (py - a.y) * dy) / length_squared).clamp(0.0, 1.0)
                };
                let nearest_x = a.x + t * dx;
                let nearest_y = a.y + t * dy;
                let distance = ((px - nearest_x).powi(2) + (py - nearest_y).powi(2)).sqrt();
                if distance <= radius {
                    self.blend(x, y, color);
                }
            }
        }
    }

    /// Outline a closed polygon.
    pub fn stroke_polygon(&mut self, points: &[Point], width: f64, color: [u8; 4]) {
        if points.is_empty() {
            return;
        }
        let mut closed = points.to_vec();
        closed.push(points[0]);
        self.stroke_polyline(&closed, width, color);
    }

    /// Draw the superellipse-rounded outline of `rect`.
    // Ported from omnishot/rounded_corners.py (rounded_path)
    pub fn stroke_rounded_rect(
        &mut self,
        rect: Rect,
        radius: f64,
        power: f64,
        width: f64,
        color: [u8; 4],
    ) {
        let points = crate::annotation::rounded_corner_points(rect, radius, power, 24);
        self.stroke_polygon(&points, width, color);
    }

    /// Scale every channel (and alpha) by `factor`, for shadow layers.
    pub fn scale_alpha(&mut self, factor: f64) {
        for offset in (3..self.data.len()).step_by(4) {
            let value = (f64::from(self.data[offset]) * factor).round();
            self.data[offset] = value.clamp(0.0, 255.0) as u8;
        }
    }
}

/// Build the rounded-rectangle clip mask for `rect` at `power`.
#[must_use]
pub fn rounded_mask(width: usize, height: usize, rect: Rect, radius: f64, power: f64) -> Vec<u8> {
    let points = crate::annotation::rounded_corner_points(rect, radius, power, 24);
    let mut mask = vec![0u8; width * height];
    fill_polygon_mask(&mut mask, width, height, &points);
    mask
}

fn fill_polygon_mask(mask: &mut [u8], width: usize, height: usize, points: &[Point]) {
    if points.len() < 3 {
        return;
    }
    let mut top = f64::INFINITY;
    let mut bottom = f64::NEG_INFINITY;
    for point in points {
        top = top.min(point.y);
        bottom = bottom.max(point.y);
    }
    let y0 = top.floor().max(0.0) as usize;
    let y1 = (bottom.ceil().max(0.0) as usize).min(height);
    let mut crossings: Vec<f64> = Vec::new();
    for y in y0..y1 {
        let scan = y as f64 + 0.5;
        crossings.clear();
        for index in 0..points.len() {
            let a = points[index];
            let b = points[(index + 1) % points.len()];
            if (a.y <= scan && b.y > scan) || (b.y <= scan && a.y > scan) {
                let t = (scan - a.y) / (b.y - a.y);
                crossings.push(a.x + t * (b.x - a.x));
            }
        }
        crossings.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        for pair in crossings.as_chunks::<2>().0 {
            let x0 = pair[0].floor().max(0.0) as usize;
            let x1 = (pair[1].ceil().max(0.0) as usize).min(width);
            for x in x0..x1 {
                if x as f64 + 0.5 >= pair[0] && (x as f64 + 0.5) <= pair[1] {
                    mask[y * width + x] = 255;
                }
            }
        }
    }
}

/// A 5x7 uppercase/digit glyph set for the label chips Qt would draw with a
/// font. Each glyph is seven rows of five bits, most significant bit left.
/// Ported from the omnishot/video_keys.py chip text boundary.
const GLYPHS: [[u8; 7]; 58] = [
    [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E], // 0
    [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E], // 1
    [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F], // 2
    [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E], // 3
    [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02], // 4
    [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E], // 5
    [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E], // 6
    [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08], // 7
    [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E], // 8
    [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C], // 9
    [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11], // A
    [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E], // B
    [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E], // C
    [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C], // D
    [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F], // E
    [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10], // F
    [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F], // G
    [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11], // H
    [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E], // I
    [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C], // J
    [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11], // K
    [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F], // L
    [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11], // M
    [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11], // N
    [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E], // O
    [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10], // P
    [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D], // Q
    [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11], // R
    [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E], // S
    [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04], // T
    [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E], // U
    [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04], // V
    [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11], // W
    [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11], // X
    [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04], // Y
    [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F], // Z
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C], // .
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04], // ,
    [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10], // /
    [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E], // @ (placeholder)
    [0x04, 0x00, 0x04, 0x04, 0x04, 0x04, 0x04], // !
    [0x00, 0x04, 0x04, 0x04, 0x04, 0x04, 0x00], // |
    [0x0E, 0x11, 0x01, 0x06, 0x04, 0x00, 0x04], // ?
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F], // _
    [0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04], // -
    [0x04, 0x0A, 0x11, 0x11, 0x1F, 0x11, 0x11], // A-ish (unused)
    [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E], // (unused)
    [0x00, 0x00, 0x0E, 0x00, 0x0E, 0x00, 0x00], // =
    [0x00, 0x00, 0x0E, 0x00, 0x00, 0x00, 0x00], // ' (placeholder)
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // space
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
];

/// The 5x7 glyph rows for a character, or `None` for an unknown glyph.
fn glyph_rows(character: char) -> Option<[u8; 7]> {
    let index = match character {
        '0'..='9' => character as usize - '0' as usize,
        'A'..='Z' => 10 + character as usize - 'A' as usize,
        'a'..='z' => 10 + character as usize - 'a' as usize,
        ' ' => 50,
        _ => return None,
    };
    Some(GLYPHS[index])
}

/// Draw `text` with box glyphs, scaled to `font_size`, with the top-left at
/// `(x, y)`. This keeps the chip's label observable without a font engine.
/// Ported from the omnishot/studio.py key-chip text boundary.
#[must_use]
pub fn text_width(text: &str, font_size: f64) -> f64 {
    text.chars().count() as f64 * font_size * 0.6
}

/// Draw `text` into `canvas` with the 5x7 bitmap face.
pub fn draw_bitmap_text(
    canvas: &mut Canvas<'_>,
    x: f64,
    y: f64,
    text: &str,
    font_size: f64,
    color: [u8; 4],
) {
    let cell = (font_size / 7.0).max(1.0);
    let mut pen_x = x;
    for character in text.chars() {
        if character == ' ' {
            pen_x += font_size * 0.6;
            continue;
        }
        if let Some(rows) = glyph_rows(character) {
            for (row, bits) in rows.iter().enumerate() {
                for column in 0..5 {
                    if bits & (1 << (4 - column)) != 0 {
                        canvas.fill_rect(
                            Rect::new(
                                pen_x + column as f64 * cell,
                                y + row as f64 * cell,
                                cell.ceil(),
                                cell.ceil(),
                            ),
                            color,
                        );
                    }
                }
            }
        }
        pen_x += font_size * 0.6;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(data: &mut [u8], width: usize, height: usize) -> Canvas<'_> {
        Canvas::new(data, width, height)
    }

    #[test]
    fn source_over_blends_opaque_and_translucent_pixels() {
        let mut data = vec![0u8; 4];
        canvas(&mut data, 1, 1).blend(0, 0, [100, 0, 0, 255]);
        assert_eq!(&data[0..4], &[100, 0, 0, 255]);
        canvas(&mut data, 1, 1).blend(0, 0, [0, 100, 0, 128]);
        // Green over opaque red: half green, half red, still opaque.
        assert_eq!(data[3], 255);
        assert!(data[0] > 40 && data[0] < 60, "{data:?}");
        assert!(data[1] > 40 && data[1] < 60, "{data:?}");
    }

    #[test]
    fn transparent_blend_keeps_alpha_untouched() {
        let mut data = vec![0u8; 4];
        canvas(&mut data, 1, 1).blend(0, 0, [10, 20, 30, 0]);
        assert_eq!(&data[0..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn fill_polygon_covers_a_triangle() {
        let mut data = vec![0u8; 8 * 8 * 4];
        canvas(&mut data, 8, 8).fill_polygon(
            &[
                Point::new(1.0, 1.0),
                Point::new(7.0, 1.0),
                Point::new(1.0, 7.0),
            ],
            [255, 0, 0, 255],
        );
        assert_eq!(image_pixel(&data, 8, 2, 2), [255, 0, 0, 255]);
        assert_eq!(image_pixel(&data, 8, 6, 6), [0, 0, 0, 0]);
    }

    #[test]
    fn stroke_polyline_covers_a_thick_horizontal_line() {
        let mut data = vec![0u8; 16 * 4 * 4];
        canvas(&mut data, 16, 4).stroke_polyline(
            &[Point::new(1.0, 2.0), Point::new(14.0, 2.0)],
            2.0,
            [0, 0, 255, 255],
        );
        assert_eq!(image_pixel(&data, 16, 8, 2), [0, 0, 255, 255]);
        assert_eq!(image_pixel(&data, 16, 8, 1), [0, 0, 255, 255]);
        assert_eq!(image_pixel(&data, 16, 8, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn rounded_mask_excludes_the_corner() {
        let mask = rounded_mask(20, 20, Rect::new(0.0, 0.0, 20.0, 20.0), 8.0, 2.0);
        assert_eq!(mask[10 * 20 + 10], 255);
        // The exact corner is outside a rounded rectangle.
        assert_eq!(mask[0], 0);
    }

    fn image_pixel(data: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
        let offset = (y * width + x) * 4;
        [
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]
    }
}
