//! Document operations behind the annotate editor.
// Ported from omnishot/editor.py (crop, resize_to, rotate, flip, combine,
// expand_to_annotations, commit, undo/redo, render) and omnishot/crop.py
// (CropSession geometry, background_color, resized).
//!
//! The document owns the base image, annotations, background and the original
//! source used to restore canvas edits. State changes are snapshots, so undo
//! and redo reuse the same project codec the files use.

use std::collections::BTreeMap;

use crate::annotation::{Annotation, Point, Props, Rect, Transform, restore_original};
use crate::backgrounds::Background;
use crate::color::Rgba;
use crate::json::Value;
use crate::pngzip::Image;
use crate::project::Project;
use crate::render::{self, ViewTransform};

/// The largest side and pixel count an expanded canvas may reach.
pub const MAX_SIDE: u32 = 100_000;
pub const MAX_PIXELS: u64 = 120_000_000;

/// Why a document edit could not be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorError {
    message: String,
}

impl EditorError {
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for EditorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EditorError {}

/// A saved snapshot of the whole document.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub base: Image,
    pub original: Option<Image>,
    pub source_transform: Transform,
    pub objects: Vec<Annotation>,
    pub background: Option<Background>,
    pub expand_canvas: bool,
}

impl Snapshot {
    #[must_use]
    pub fn to_project(&self) -> Project {
        Project {
            version: 2,
            base: self.base.clone(),
            original: self.original.clone(),
            source_transform: self.source_transform,
            objects: self.objects.clone(),
            background: self.background.clone(),
            expand_canvas: self.expand_canvas,
        }
    }

    #[must_use]
    pub fn from_project(project: Project) -> Self {
        Self {
            base: project.base,
            original: project.original,
            source_transform: project.source_transform,
            objects: project.objects,
            background: project.background,
            expand_canvas: project.expand_canvas,
        }
    }
}

/// The editable document.
#[derive(Debug, Clone)]
pub struct Document {
    pub base: Image,
    pub original: Option<Image>,
    pub source_transform: Transform,
    pub objects: Vec<Annotation>,
    pub background: Option<Background>,
    pub expand_canvas: bool,
    undo: Vec<Snapshot>,
    position: isize,
}

impl Document {
    /// Start a new document over a base image.
    #[must_use]
    pub fn new(base: Image, background: Option<Background>, expand_canvas: bool) -> Self {
        let mut document = Self {
            base,
            original: None,
            source_transform: Transform::default(),
            objects: Vec::new(),
            background,
            expand_canvas,
            undo: Vec::new(),
            position: -1,
        };
        document.commit();
        document
    }

    /// Open an editable project.
    pub fn open(project: Project) -> Self {
        let snapshot = Snapshot::from_project(project);
        let mut document = Self {
            base: snapshot.base.clone(),
            original: snapshot.original.clone(),
            source_transform: snapshot.source_transform,
            objects: snapshot.objects.clone(),
            background: snapshot.background.clone(),
            expand_canvas: snapshot.expand_canvas,
            undo: Vec::new(),
            position: -1,
        };
        document.commit();
        document
    }

    /// The current state as a project.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            base: self.base.clone(),
            original: self.original.clone(),
            source_transform: self.source_transform,
            objects: self.objects.clone(),
            background: self.background.clone(),
            expand_canvas: self.expand_canvas,
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.base = snapshot.base;
        self.original = snapshot.original;
        self.source_transform = snapshot.source_transform;
        self.objects = snapshot.objects;
        self.background = snapshot.background;
        self.expand_canvas = snapshot.expand_canvas;
    }

    /// Record the current state as an undo step.
    // Ported from omnishot/editor.py (commit)
    pub fn commit(&mut self) {
        if self.expand_canvas {
            // Expansion failure leaves the canvas unchanged and is reported
            // by the caller, which can still show the megapixel message.
            let _ = self.expand_to_annotations();
        }
        self.undo.truncate((self.position + 1).max(0) as usize);
        self.undo.push(self.snapshot());
        if self.undo.len() > 35 {
            self.undo.remove(0);
        }
        self.position = self.undo.len() as isize - 1;
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.position > 0
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        (self.position as usize + 1) < self.undo.len()
    }

    pub fn undo(&mut self) {
        if self.can_undo() {
            self.position -= 1;
            let snapshot = self.undo[self.position as usize].clone();
            self.restore(snapshot);
        }
    }

    pub fn redo(&mut self) {
        if self.can_redo() {
            self.position += 1;
            let snapshot = self.undo[self.position as usize].clone();
            self.restore(snapshot);
        }
    }

    /// Delete the selected annotations.
    // Ported from omnishot/editor.py (delete_selected)
    pub fn delete(&mut self, indices: &[usize]) {
        let mut keep = vec![true; self.objects.len()];
        for index in indices {
            if let Some(slot) = keep.get_mut(*index) {
                *slot = false;
            }
        }
        self.objects = self
            .objects
            .drain(..)
            .zip(keep)
            .filter_map(|(object, keep)| keep.then_some(object))
            .collect();
        self.commit();
    }

    /// Render the document without a background.
    #[must_use]
    pub fn render(&self) -> Image {
        render::render(&self.base, &self.objects, ViewTransform::default())
    }

    /// Render the document, framing it in the background when present.
    #[must_use]
    pub fn render_export(&self) -> Image {
        render::render_with_background(
            &self.base,
            &self.objects,
            self.background.as_ref(),
            ViewTransform::default(),
        )
    }

    /// Grow the canvas to include every non-spotlight annotation.
    // Ported from omnishot/editor.py (expand_to_annotations)
    pub fn expand_to_annotations(&mut self) -> Result<bool, EditorError> {
        let mut bounds = Rect::new(0.0, 0.0, self.base.width as f64, self.base.height as f64);
        for object in &self.objects {
            if object.kind() == Some("spotlight") {
                continue;
            }
            bounds = bounds.united(&object.content_bounds());
        }
        let bounds = bounds.to_aligned_rect();
        if bounds == Rect::new(0.0, 0.0, self.base.width as f64, self.base.height as f64) {
            return Ok(false);
        }
        let width = bounds.width.round() as u64;
        let height = bounds.height.round() as u64;
        if width == 0
            || height == 0
            || width * height > MAX_PIXELS
            || width.max(height) > u64::from(MAX_SIDE)
        {
            return Err(EditorError::new(
                "Could not expand the canvas. The limit is 120 megapixels; move the annotation closer to the image.",
            ));
        }
        let mut canvas = Image::transparent(width as usize, height as usize);
        let dx = -bounds.x.round() as i64;
        let dy = -bounds.y.round() as i64;
        for y in 0..self.base.height {
            for x in 0..self.base.width {
                let target_x = x as i64 + dx;
                let target_y = y as i64 + dy;
                if target_x < 0
                    || target_y < 0
                    || target_x >= canvas.width as i64
                    || target_y >= canvas.height as i64
                {
                    continue;
                }
                let source = (y * self.base.width + x) * 4;
                let target = (target_y as usize * canvas.width + target_x as usize) * 4;
                canvas.data[target..target + 4]
                    .copy_from_slice(&self.base.data[source..source + 4]);
            }
        }
        self.remember(Transform([
            1.0,
            0.0,
            0.0,
            1.0,
            -bounds.x.round(),
            -bounds.y.round(),
        ]));
        self.base = canvas;
        for object in &mut self.objects {
            object
                .props
                .insert("x".to_owned(), Value::Number(object.x() - bounds.x.round()));
            object
                .props
                .insert("y".to_owned(), Value::Number(object.y() - bounds.y.round()));
        }
        Ok(true)
    }

    /// Remember the operation that produced a canvas edit, for restore.
    fn remember(&mut self, operation: Transform) {
        if self.original.is_none() {
            self.original = Some(self.base.clone());
        }
        self.source_transform = self.source_transform.multiply(&operation);
    }

    /// Crop or expand the canvas.
    // Ported from omnishot/editor.py (crop)
    pub fn crop(
        &mut self,
        rect: Rect,
        expand: bool,
        fill: Option<Rgba>,
    ) -> Result<(), EditorError> {
        let width = rect.width.round().max(0.0) as u64;
        let height = rect.height.round().max(0.0) as u64;
        if width < 2 || height < 2 {
            return Err(EditorError::new("Crop is too small."));
        }
        if width.max(height) > u64::from(MAX_SIDE) || width * height > MAX_PIXELS {
            return Err(EditorError::new(
                "Crop is too large. The limit is 120 megapixels.",
            ));
        }
        let mut canvas = if expand {
            let fill = fill.unwrap_or(Rgba::new(0, 0, 0, 0));
            let pixel = [fill.red, fill.green, fill.blue, fill.alpha];
            let mut data = vec![0u8; (width * height * 4) as usize];
            for chunk in data.as_chunks_mut::<4>().0 {
                chunk.copy_from_slice(&pixel);
            }
            Image::new(width as usize, height as usize, data)
        } else {
            Image::transparent(width as usize, height as usize)
        };
        let dx = -rect.x.round() as i64;
        let dy = -rect.y.round() as i64;
        for y in 0..self.base.height {
            for x in 0..self.base.width {
                let target_x = x as i64 + dx;
                let target_y = y as i64 + dy;
                if target_x < 0
                    || target_y < 0
                    || target_x >= canvas.width as i64
                    || target_y >= canvas.height as i64
                {
                    continue;
                }
                let source = (y * self.base.width + x) * 4;
                let target = (target_y as usize * canvas.width + target_x as usize) * 4;
                canvas.data[target..target + 4]
                    .copy_from_slice(&self.base.data[source..source + 4]);
            }
        }
        self.remember(Transform([
            1.0,
            0.0,
            0.0,
            1.0,
            -rect.x.round(),
            -rect.y.round(),
        ]));
        self.base = canvas;
        // A deliberate crop fixes the canvas; objects outside must not grow it.
        let expanding = self.expand_canvas;
        self.expand_canvas = false;
        for object in &mut self.objects {
            object
                .props
                .insert("x".to_owned(), Value::Number(object.x() - rect.x.round()));
            object
                .props
                .insert("y".to_owned(), Value::Number(object.y() - rect.y.round()));
        }
        self.commit();
        if expanding {
            self.expand_canvas = false;
        }
        Ok(())
    }

    /// Resize the canvas to `width`, scaling annotations and effects.
    // Ported from omnishot/editor.py (resize_to)
    pub fn resize_to(&mut self, width: i32) -> Result<(), EditorError> {
        let width = width.clamp(2, 30_000) as u64;
        let factor = width as f64 / self.base.width.max(1) as f64;
        let new_height = ((self.base.height as f64 * factor).round() as u64).max(1);
        if width * new_height > MAX_PIXELS {
            return Err(EditorError::new(
                "Resize is too large. The limit is 120 megapixels.",
            ));
        }
        self.remember(Transform([factor, 0.0, 0.0, factor, 0.0, 0.0]));
        self.base = scale_image(&self.base, width as usize, new_height as usize);
        for object in &mut self.objects {
            for key in ["x", "y", "w", "h", "width", "font_size"] {
                if let Some(value) = object.props.get(key).and_then(Value::as_f64) {
                    object
                        .props
                        .insert(key.to_owned(), Value::Number(value * factor));
                }
            }
            if let Some(Value::Array(points)) = object.props.get("points").cloned() {
                let scaled: Vec<Value> = points
                    .iter()
                    .map(|point| {
                        let pair = point.as_array().unwrap_or(&[]);
                        Value::Array(vec![
                            Value::Number(
                                pair.first().and_then(Value::as_f64).unwrap_or(0.0) * factor,
                            ),
                            Value::Number(
                                pair.get(1).and_then(Value::as_f64).unwrap_or(0.0) * factor,
                            ),
                        ])
                    })
                    .collect();
                object
                    .props
                    .insert("points".to_owned(), Value::Array(scaled));
            }
        }
        self.commit();
        Ok(())
    }

    /// Rotate the canvas clockwise and rotate each annotation with it.
    // Ported from omnishot/editor.py (rotate)
    pub fn rotate(&mut self) {
        let height = self.base.height;
        self.remember(Transform([0.0, 1.0, -1.0, 0.0, height as f64, 0.0]));
        let rotation = Transform([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);
        for object in &mut self.objects {
            let x = object.x();
            let y = object.y();
            object
                .props
                .insert("x".to_owned(), Value::Number(height as f64 - y));
            object.props.insert("y".to_owned(), Value::Number(x));
            let existing = object_transform(object);
            object.props.insert(
                "transform".to_owned(),
                existing.multiply(&rotation).to_values(),
            );
        }
        self.base = rotate_image(&self.base);
        self.commit();
    }

    /// Flip the canvas horizontally or vertically.
    // Ported from omnishot/editor.py (flip)
    pub fn flip(&mut self, vertical: bool) {
        let width = self.base.width;
        let height = self.base.height;
        let operation = if vertical {
            Transform([1.0, 0.0, 0.0, -1.0, 0.0, height as f64])
        } else {
            Transform([-1.0, 0.0, 0.0, 1.0, width as f64, 0.0])
        };
        self.remember(operation);
        let flip = if vertical {
            Transform([1.0, 0.0, 0.0, -1.0, 0.0, 0.0])
        } else {
            Transform([-1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
        };
        for object in &mut self.objects {
            if vertical {
                object
                    .props
                    .insert("y".to_owned(), Value::Number(height as f64 - object.y()));
            } else {
                object
                    .props
                    .insert("x".to_owned(), Value::Number(width as f64 - object.x()));
            }
            let existing = object_transform(object);
            object
                .props
                .insert("transform".to_owned(), existing.multiply(&flip).to_values());
        }
        self.base = flip_image(&self.base, vertical);
        self.commit();
    }

    /// Restore the remembered original and map annotations back through edits.
    // Ported from omnishot/image_original.py (restore_original)
    pub fn restore_original(&mut self) -> Result<bool, EditorError> {
        let Some(original) = self.original.clone() else {
            return Ok(false);
        };
        let props = restore_original(&self.objects, &self.source_transform)
            .map_err(|error| EditorError::new(error.message()))?;
        self.objects = props.into_iter().map(Annotation::new).collect();
        self.base = original;
        self.source_transform = Transform::default();
        self.expand_canvas = false;
        self.commit();
        Ok(true)
    }

    /// Combine another image beside this one.
    // Ported from omnishot/editor.py (combine)
    pub fn combine(&mut self, image: &Image, direction: &str) {
        let (w, h) = (self.base.width, self.base.height);
        let (iw, ih) = (image.width, image.height);
        let (nw, nh) = if matches!(direction, "Above" | "Below") {
            (w.max(iw), h + ih)
        } else {
            (w + iw, h.max(ih))
        };
        let (dx, dy) = match direction {
            "Left" => (iw as i64, 0),
            "Above" => (0, ih as i64),
            _ => (0, 0),
        };
        self.remember(Transform([1.0, 0.0, 0.0, 1.0, dx as f64, dy as f64]));
        let mut canvas = Image::transparent(nw, nh);
        blit(&mut canvas, &self.base, dx, dy);
        self.base = canvas;
        for object in &mut self.objects {
            if dx != 0 {
                object
                    .props
                    .insert("x".to_owned(), Value::Number(object.x() + dx as f64));
            }
            if dy != 0 {
                object
                    .props
                    .insert("y".to_owned(), Value::Number(object.y() + dy as f64));
            }
        }
        let position = match direction {
            "Below" => Point::new(0.0, h as f64),
            "Right" => Point::new(w as f64, 0.0),
            _ => Point::new(0.0, 0.0),
        };
        self.insert_image(image, position);
    }

    /// Insert an image annotation at `position`.
    // Ported from omnishot/editor.py (insert_qimage)
    pub fn insert_image(&mut self, image: &Image, position: Point) {
        let mut props = Props::new();
        props.insert("kind".to_owned(), Value::String("image".to_owned()));
        props.insert("x".to_owned(), Value::Number(position.x));
        props.insert("y".to_owned(), Value::Number(position.y));
        props.insert("w".to_owned(), Value::Number(image.width as f64));
        props.insert("h".to_owned(), Value::Number(image.height as f64));
        props.insert(
            "image".to_owned(),
            Value::String(crate::annotation::encode_base64(&image.to_png())),
        );
        props.insert(
            "z".to_owned(),
            Value::Number(self.objects.len() as f64 + 1.0),
        );
        self.objects.push(Annotation::new(props));
        self.commit();
    }

    /// Paste decoded annotations, offset by 20 pixels and kept on canvas.
    // Ported from omnishot/editor.py (paste_content)
    pub fn paste_annotations(&mut self, objects: Vec<Annotation>) -> Result<(), EditorError> {
        let mut first = self
            .objects
            .iter()
            .map(|object| object.z())
            .fold(0.0, f64::max)
            + 1.0;
        let mut prepared = Vec::with_capacity(objects.len());
        for mut object in objects {
            object
                .props
                .insert("x".to_owned(), Value::Number(object.x() + 20.0));
            object
                .props
                .insert("y".to_owned(), Value::Number(object.y() + 20.0));
            object.props.insert("z".to_owned(), Value::Number(first));
            first += 1.0;
            prepared.push(object);
        }
        // A pasted group that lands entirely off-canvas is centred on it.
        if !prepared.is_empty() {
            let mut bounds = prepared[0].content_bounds();
            for object in &prepared {
                bounds = bounds.united(&object.content_bounds());
            }
            let canvas = Rect::new(0.0, 0.0, self.base.width as f64, self.base.height as f64);
            if !bounds.intersects(&canvas) {
                let offset = Point::new(
                    canvas.center().x - bounds.center().x,
                    canvas.center().y - bounds.center().y,
                );
                for object in &mut prepared {
                    object
                        .props
                        .insert("x".to_owned(), Value::Number(object.x() + offset.x));
                    object
                        .props
                        .insert("y".to_owned(), Value::Number(object.y() + offset.y));
                }
            }
        }
        self.objects.extend(prepared);
        self.commit();
        Ok(())
    }

    /// Zoom to a percentage of the base size, for the render transform.
    #[must_use]
    pub fn zoom(&self, percent: f64) -> ViewTransform {
        let scale = percent / 100.0;
        ViewTransform {
            values: [scale, 0.0, 0.0, scale, 0.0, 0.0],
        }
    }

    /// Serialize the document as a `.omnishot` archive.
    // Ported from omnishot/editor.py (write_project)
    pub fn to_project(&self) -> Project {
        self.snapshot().to_project()
    }

    /// Write the document atomically as a `.omnishot` archive.
    pub fn save(&self, destination: &std::path::Path) -> Result<(), EditorError> {
        self.to_project()
            .save(destination)
            .map_err(|error| EditorError::new(error.message()))
    }
}

fn object_transform(object: &Annotation) -> Transform {
    let values = object
        .props
        .get("transform")
        .and_then(Value::as_array)
        .and_then(|items| {
            let numbers: Option<Vec<f64>> = items.iter().map(Value::as_f64).collect();
            numbers.and_then(|values| <[f64; 6]>::try_from(values).ok())
        })
        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    Transform(values)
}

fn scale_image(image: &Image, width: usize, height: usize) -> Image {
    let mut data = vec![0u8; width * height * 4];
    let x_ratio = image.width as f64 / width as f64;
    let y_ratio = image.height as f64 / height as f64;
    for y in 0..height {
        let source_y = ((y as f64 + 0.5) * y_ratio)
            .floor()
            .clamp(0.0, image.height.saturating_sub(1) as f64) as usize;
        for x in 0..width {
            let source_x = ((x as f64 + 0.5) * x_ratio)
                .floor()
                .clamp(0.0, image.width.saturating_sub(1) as f64)
                as usize;
            let source = (source_y * image.width + source_x) * 4;
            let target = (y * width + x) * 4;
            data[target..target + 4].copy_from_slice(&image.data[source..source + 4]);
        }
    }
    Image::new(width, height, data)
}

fn rotate_image(image: &Image) -> Image {
    let mut data = vec![0u8; image.width * image.height * 4];
    let new_width = image.height;
    for y in 0..image.height {
        for x in 0..image.width {
            let source = (y * image.width + x) * 4;
            let target_x = image.height - 1 - y;
            let target_y = x;
            let target = (target_y * new_width + target_x) * 4;
            data[target..target + 4].copy_from_slice(&image.data[source..source + 4]);
        }
    }
    Image::new(image.height, image.width, data)
}

fn flip_image(image: &Image, vertical: bool) -> Image {
    let mut data = vec![0u8; image.data.len()];
    for y in 0..image.height {
        for x in 0..image.width {
            let source = (y * image.width + x) * 4;
            let target_x = if vertical { x } else { image.width - 1 - x };
            let target_y = if vertical { image.height - 1 - y } else { y };
            let target = (target_y * image.width + target_x) * 4;
            data[target..target + 4].copy_from_slice(&image.data[source..source + 4]);
        }
    }
    Image::new(image.width, image.height, data)
}

fn blit(canvas: &mut Image, source: &Image, dx: i64, dy: i64) {
    for y in 0..source.height {
        for x in 0..source.width {
            let target_x = x as i64 + dx;
            let target_y = y as i64 + dy;
            if target_x < 0
                || target_y < 0
                || target_x >= canvas.width as i64
                || target_y >= canvas.height as i64
            {
                continue;
            }
            let source_offset = (y * source.width + x) * 4;
            let target = (target_y as usize * canvas.width + target_x as usize) * 4;
            canvas.data[target..target + 4]
                .copy_from_slice(&source.data[source_offset..source_offset + 4]);
        }
    }
}

/// The dominant uniform border color, or transparent when there is none.
// Ported from omnishot/crop.py (background_color)
#[must_use]
pub fn background_color(image: &Image) -> Rgba {
    let mut points: Vec<[u8; 4]> = Vec::with_capacity(64);
    for index in 0..16 {
        if image.width == 0 || image.height == 0 {
            break;
        }
        let x = (index * (image.width.saturating_sub(1))) / 15;
        let y = (index * (image.height.saturating_sub(1))) / 15;
        points.push(image.pixel(x, 0));
        points.push(image.pixel(x, image.height - 1));
        points.push(image.pixel(0, y));
        points.push(image.pixel(image.width - 1, y));
    }
    if points.is_empty() {
        return Rgba::new(0, 0, 0, 0);
    }
    let mut counts: BTreeMap<[u8; 4], usize> = BTreeMap::new();
    for point in &points {
        *counts.entry(*point).or_insert(0) += 1;
    }
    let (value, count) = counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .unwrap_or(([0, 0, 0, 0], 0));
    if count * 5 >= points.len() * 4 {
        Rgba::new(value[0], value[1], value[2], value[3])
    } else {
        Rgba::new(0, 0, 0, 0)
    }
}

/// Resize a crop rectangle from a handle, optionally keeping a ratio.
// Ported from omnishot/crop.py (resized)
#[must_use]
pub fn resized(rect: Rect, handle: &str, point: Point, ratio: Option<f64>) -> Rect {
    let mut left = rect.x;
    let mut top = rect.y;
    let mut right = rect.right();
    let mut bottom = rect.bottom();
    if handle.contains('l') {
        left = point.x.min(right - 2.0);
    }
    if handle.contains('r') {
        right = point.x.max(left + 2.0);
    }
    if handle.contains('t') {
        top = point.y.min(bottom - 2.0);
    }
    if handle.contains('b') {
        bottom = point.y.max(top + 2.0);
    }
    if let Some(ratio) = ratio {
        let mut width = right - left;
        let mut height = bottom - top;
        if handle == "l" || handle == "r" {
            height = width / ratio;
            top = rect.center().y - height / 2.0;
            bottom = top + height;
        } else if handle == "t" || handle == "b" {
            width = height * ratio;
            left = rect.center().x - width / 2.0;
            right = left + width;
        } else {
            width = width.max(height * ratio);
            height = width / ratio;
            if handle.contains('l') {
                left = right - width;
            } else {
                right = left + width;
            }
            if handle.contains('t') {
                top = bottom - height;
            } else {
                bottom = top + height;
            }
        }
    }
    Rect::new(left, top, right - left, bottom - top)
}

/// Read an annotation and its snap targets from the base and image objects.
// Ported from omnishot/crop.py (CropSession.targets/snap_point)
#[must_use]
pub fn snap_targets(image: &Image, objects: &[Annotation]) -> (Vec<f64>, Vec<f64>) {
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    let rects = std::iter::once(Rect::new(0.0, 0.0, image.width as f64, image.height as f64))
        .chain(
            objects
                .iter()
                .filter(|object| object.kind() == Some("image"))
                .map(Annotation::content_bounds),
        );
    for rect in rects {
        xs.push(rect.x);
        xs.push(rect.right());
        ys.push(rect.y);
        ys.push(rect.bottom());
    }
    (xs, ys)
}

/// Snap a point to the nearest target within `threshold`.
// Ported from omnishot/crop.py (snap_point)
#[must_use]
pub fn snap_point(point: Point, targets: &(Vec<f64>, Vec<f64>), threshold: f64) -> Point {
    let nearest = |value: f64, values: &[f64]| {
        values
            .iter()
            .copied()
            .min_by(|a, b| {
                (a - value)
                    .abs()
                    .partial_cmp(&(b - value).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .filter(|best| (best - value).abs() <= threshold)
            .unwrap_or(value)
    };
    Point::new(nearest(point.x, &targets.0), nearest(point.y, &targets.1))
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn crop_is_one_undo_step_and_offsets_annotations() {
        let mut document = Document::new(image(100, 80, [230, 230, 230, 255]), None, false);
        document.objects.push(Annotation::new(props(&[
            ("kind", Value::String("rect".into())),
            ("x", Value::Number(20.0)),
            ("y", Value::Number(30.0)),
        ])));
        document.commit();
        let index = document.position;
        document
            .crop(Rect::new(10.0, 5.0, 50.0, 40.0), false, None)
            .unwrap();
        assert_eq!((document.base.width, document.base.height), (50, 40));
        assert_eq!(document.objects[0].x(), 10.0);
        assert_eq!(document.objects[0].y(), 25.0);
        assert_eq!(document.position, index + 1);
        document.undo();
        assert_eq!((document.base.width, document.base.height), (100, 80));
        assert_eq!(document.objects[0].x(), 20.0);
        assert_eq!(document.objects[0].y(), 30.0);
    }

    #[test]
    fn expanding_a_crop_fills_with_the_requested_color() {
        let mut document = Document::new(image(20, 20, [10, 20, 30, 255]), None, false);
        document
            .crop(
                Rect::new(-5.0, -5.0, 30.0, 30.0),
                true,
                Some(Rgba::new(200, 100, 50, 255)),
            )
            .unwrap();
        assert_eq!((document.base.width, document.base.height), (30, 30));
        assert_eq!(document.base.pixel(0, 0), [200, 100, 50, 255]);
        assert_eq!(document.base.pixel(5, 5), [10, 20, 30, 255]);
    }

    #[test]
    fn rotate_and_flip_move_annotations_with_the_canvas() {
        let mut document = Document::new(image(40, 20, [255, 255, 255, 255]), None, false);
        document.objects.push(Annotation::new(props(&[
            ("kind", Value::String("rect".into())),
            ("x", Value::Number(4.0)),
            ("y", Value::Number(6.0)),
        ])));
        document.commit();
        document.rotate();
        assert_eq!((document.base.width, document.base.height), (20, 40));
        assert_eq!(
            (document.objects[0].x(), document.objects[0].y()),
            (14.0, 4.0)
        );
        document.flip(true);
        assert_eq!(document.objects[0].y(), 36.0);
    }

    #[test]
    fn restore_original_maps_annotations_back() {
        let mut document = Document::new(image(20, 20, [200, 200, 200, 255]), None, false);
        document
            .crop(Rect::new(5.0, 5.0, 10.0, 10.0), false, None)
            .unwrap();
        document.objects.push(Annotation::new(props(&[
            ("kind", Value::String("rect".into())),
            ("x", Value::Number(2.0)),
            ("y", Value::Number(3.0)),
        ])));
        document.commit();
        assert!(document.original.is_some());
        document.restore_original().unwrap();
        assert_eq!((document.base.width, document.base.height), (20, 20));
        assert_eq!(
            (document.objects[0].x(), document.objects[0].y()),
            (7.0, 8.0)
        );
        assert!(document.source_transform.is_identity());
    }

    #[test]
    fn expand_to_annotations_grows_the_canvas() {
        let mut document = Document::new(image(40, 30, [255, 255, 255, 255]), None, false);
        document.objects.push(Annotation::new(props(&[
            ("kind", Value::String("rect".into())),
            ("x", Value::Number(-10.0)),
            ("y", Value::Number(0.0)),
            ("w", Value::Number(20.0)),
            ("h", Value::Number(10.0)),
        ])));
        let changed = document.expand_to_annotations().unwrap();
        assert!(changed);
        assert!(document.base.width > 40);
        assert!(document.objects[0].x() >= 0.0);
    }

    #[test]
    fn resize_scales_annotation_geometry() {
        let mut document = Document::new(image(100, 50, [255, 255, 255, 255]), None, false);
        document.objects.push(Annotation::new(props(&[
            ("kind", Value::String("rect".into())),
            ("x", Value::Number(10.0)),
            ("y", Value::Number(10.0)),
            ("w", Value::Number(20.0)),
            ("h", Value::Number(10.0)),
        ])));
        document.commit();
        document.resize_to(200).unwrap();
        assert_eq!((document.base.width, document.base.height), (200, 100));
        assert_eq!(document.objects[0].x(), 20.0);
        assert_eq!(document.objects[0].width(), 40.0);
    }

    #[test]
    fn pasting_keeps_a_group_visible_and_offsets_it() {
        let mut document = Document::new(image(100, 100, [255, 255, 255, 255]), None, false);
        let far = vec![Annotation::new(props(&[
            ("kind", Value::String("rect".into())),
            ("x", Value::Number(5000.0)),
            ("y", Value::Number(8000.0)),
            ("w", Value::Number(100.0)),
            ("h", Value::Number(80.0)),
        ]))];
        document.paste_annotations(far).unwrap();
        let object = &document.objects[0];
        assert!(
            object
                .content_bounds()
                .intersects(&Rect::new(0.0, 0.0, 100.0, 100.0))
        );
    }

    #[test]
    fn background_color_reads_a_uniform_border() {
        let uniform = image(10, 10, [230, 230, 230, 255]);
        assert_eq!(background_color(&uniform), Rgba::new(230, 230, 230, 255));
        let mut mixed = uniform.clone();
        for y in 0..10 {
            for x in 0..10 {
                let offset = (y * 10 + x) * 4;
                mixed.data[offset..offset + 4].copy_from_slice(&[(x * 25) as u8, 0, 0, 255]);
            }
        }
        assert_eq!(background_color(&mixed).alpha, 0);
    }

    #[test]
    fn crop_ratio_resize_anchors_the_opposite_edge() {
        let rect = Rect::new(40.0, 30.0, 160.0, 90.0);
        for handle in ["lt", "t", "rt", "r", "rb", "b", "lb", "l"] {
            let result = resized(rect, handle, Point::new(210.0, 150.0), Some(16.0 / 9.0));
            assert!(
                (result.width / result.height - 16.0 / 9.0).abs() < 0.00001,
                "{handle} {result:?}"
            );
        }
    }

    #[test]
    fn snapping_chooses_the_nearest_edge_within_threshold() {
        let base = image(300, 200, [0, 0, 0, 255]);
        let inserted = Annotation::new(props(&[
            ("kind", Value::String("image".into())),
            ("x", Value::Number(70.0)),
            ("y", Value::Number(80.0)),
            ("w", Value::Number(300.0)),
            ("h", Value::Number(200.0)),
        ]));
        let targets = snap_targets(&base, std::slice::from_ref(&inserted));
        assert_eq!(
            snap_point(Point::new(369.0, 279.0), &targets, 8.0),
            Point::new(370.0, 280.0)
        );
        assert_eq!(
            snap_point(Point::new(100.0, 100.0), &targets, 8.0),
            Point::new(100.0, 100.0)
        );
    }

    #[test]
    fn combined_images_place_the_new_one_beside() {
        let mut document = Document::new(image(40, 30, [255, 0, 0, 255]), None, false);
        document.combine(&image(20, 10, [0, 0, 255, 255]), "Below");
        assert_eq!((document.base.width, document.base.height), (40, 40));
        assert_eq!(document.base.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(document.objects.len(), 1);
        assert_eq!(document.objects[0].kind(), Some("image"));
    }
}
