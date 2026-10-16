//! Editable recording metadata and deterministic local video composition.
// Ported from omnishot/studio.py and omnishot/video_camera.py
//!
//! Composition is shared by the preview and the final export, so the two never
//! disagree. Every external program runs through [`CommandRunner`]; the
//! pipeline decodes the source with one `ffmpeg` process, composites each
//! output frame in memory, and pipes RGB24 into a second `ffmpeg` process.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::annotation::{Point, Rect};
use crate::commands::{CommandRunner, CommandSpec};
use crate::json::Value;
use crate::pngzip::{Image, decode_png};
use crate::raster::Canvas;

use crate::motion_blur::blur_level;
use crate::video_camera;
use crate::video_clicks::press_scale;
use crate::video_keys::{key_colors, visible_keys};
use crate::video_motion::{Zoom, zoom_at};
use crate::video_position::camera_rect;

/// Video options, the stored `options` object.
pub type Options = BTreeMap<String, Value>;

/// A single cursor sample on the normalized source clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorSample {
    pub t: f64,
    pub x: f64,
    pub y: f64,
}

impl CursorSample {
    /// Read a cursor sample from a manifest object.
    #[must_use]
    pub fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            t: value.get("t").and_then(Value::as_f64)?,
            x: value.get("x").and_then(Value::as_f64).unwrap_or(0.5),
            y: value.get("y").and_then(Value::as_f64).unwrap_or(0.5),
        })
    }

    /// Read a cursor track from a manifest list.
    #[must_use]
    pub fn list_from_json(value: Option<&Value>) -> Vec<Self> {
        value
            .and_then(Value::as_array)
            .map(|rows| rows.iter().filter_map(Self::from_json).collect())
            .unwrap_or_default()
    }

    /// Serialize the sample back to a manifest object.
    #[must_use]
    pub fn to_json(&self) -> Value {
        Value::Object(BTreeMap::from([
            ("t".to_owned(), Value::Number(self.t)),
            ("x".to_owned(), Value::Number(self.x)),
            ("y".to_owned(), Value::Number(self.y)),
        ]))
    }
}

/// The retained source intervals after removing the cuts.
// Ported from omnishot/studio.py (kept_segments)
#[must_use]
pub fn kept_segments(start: f64, end: f64, cuts: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut sorted: Vec<(f64, f64)> = cuts.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mut segments = Vec::new();
    let mut cursor = start;
    for (a, b) in sorted {
        if b <= cursor || a >= end {
            continue;
        }
        if a > cursor {
            segments.push((cursor, a.min(end)));
        }
        cursor = cursor.max(b);
    }
    if cursor < end {
        segments.push((cursor, end));
    }
    segments
}

/// The number of output frames for each retained segment at `fps`/`speed`.
// Ported from omnishot/studio.py (export_studio segment counts)
#[must_use]
pub fn segment_frame_counts(
    segments: &[(f64, f64)],
    fps: f64,
    speed: f64,
) -> Vec<(f64, f64, usize)> {
    segments
        .iter()
        .map(|(a, b)| {
            let seconds = (b - a) * fps / speed;
            (*a, *b, if seconds <= 0.0 { 0 } else { seconds as usize })
        })
        .collect()
}

/// Smooth the recorded cursor track.
// Ported from omnishot/studio.py (smooth_cursor)
#[must_use]
pub fn smooth_cursor(samples: &[CursorSample], strength: f64) -> Vec<CursorSample> {
    let Some(first) = samples.first() else {
        return Vec::new();
    };
    let mut out = vec![*first];
    for sample in &samples[1..] {
        let previous = *out.last().unwrap();
        let dt = (sample.t - previous.t).max(0.001);
        let alpha = 1.0 - (-dt / (strength * 0.12).max(0.005)).exp();
        out.push(CursorSample {
            t: sample.t,
            x: previous.x + alpha * (sample.x - previous.x),
            y: previous.y + alpha * (sample.y - previous.y),
        });
    }
    out
}

/// The interpolated cursor at `t`, or `None` when the track is empty.
// Ported from omnishot/studio.py (cursor_at)
#[must_use]
pub fn cursor_at(samples: &[CursorSample], t: f64) -> Option<CursorSample> {
    let first = samples.first()?;
    let index = samples.partition_point(|sample| sample.t <= t);
    if index == 0 {
        return Some(*first);
    }
    if index >= samples.len() {
        return samples.last().copied();
    }
    let a = samples[index - 1];
    let b = samples[index];
    let f = (t - a.t) / (b.t - a.t).max(0.0001);
    Some(CursorSample {
        t,
        x: a.x + (b.x - a.x) * f,
        y: a.y + (b.y - a.y) * f,
    })
}

/// Plan zooms from the click track.
// Ported from omnishot/studio.py (smart_zooms)
#[must_use]
pub fn smart_zooms(events: &[Value], duration: f64, scale: f64, limit: Option<f64>) -> Vec<Zoom> {
    let mut result: Vec<Zoom> = Vec::new();
    for event in events {
        let Some(object) = event.as_object() else {
            continue;
        };
        if object.get("kind").and_then(Value::as_str) != Some("click") {
            continue;
        }
        if object.get("hidden").is_some_and(Value::is_truthy) {
            continue;
        }
        let Some(t) = object.get("t").and_then(Value::as_f64) else {
            continue;
        };
        let x = object.get("x").and_then(Value::as_f64).unwrap_or(-1.0);
        let y = object.get("y").and_then(Value::as_f64).unwrap_or(-1.0);
        if t < 0.0
            || !(0.0..=1.0).contains(&x)
            || !(0.0..=1.0).contains(&y)
            || limit.is_some_and(|limit| t >= limit)
        {
            continue;
        }
        if let Some(last) = result.last()
            && t < last.end - 0.3
        {
            continue;
        }
        let a = (t - 0.45).max(result.last().map_or(0.0, |last| last.end));
        let b = limit.map_or(t + duration, |limit| (t + duration).min(limit));
        if b <= a {
            continue;
        }
        result.push(Zoom {
            start: a,
            end: b,
            scale,
            x,
            y,
            follow: true,
        });
    }
    result
}

/// Apply the stored event edits and return the events in time order.
// Ported from omnishot/studio.py (edited_events)
#[must_use]
pub fn edited_events(metadata: &Value, options: &Options) -> Vec<Value> {
    let events: Vec<Value> = metadata
        .get("events")
        .and_then(Value::as_array)
        .map(<[Value]>::to_vec)
        .unwrap_or_default();
    let Some(edits) = options.get("event_edits").and_then(Value::as_object) else {
        return events;
    };
    if edits.is_empty() {
        return events;
    }
    let mut result: Vec<Value> = events
        .iter()
        .enumerate()
        .map(|(index, event)| {
            let mut merged = event.as_object().cloned().unwrap_or_default();
            if let Some(edit) = edits.get(&index.to_string()).and_then(Value::as_object) {
                for key in ["t", "label", "hidden"] {
                    if let Some(value) = edit.get(key) {
                        merged.insert(key.to_owned(), value.clone());
                    }
                }
            }
            Value::Object(merged)
        })
        .collect();
    result.sort_by(|a, b| {
        let ta = a.get("t").and_then(Value::as_f64).unwrap_or(0.0);
        let tb = b.get("t").and_then(Value::as_f64).unwrap_or(0.0);
        ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
    });
    result
}

/// Decode a base64 PNG background image.
// Ported from omnishot/studio.py (background_image)
#[must_use]
pub fn background_image(data: &str) -> Option<Image> {
    let bytes = crate::annotation::decode_base64(data)?;
    decode_png(&bytes).ok()
}

/// The number of output frames the edit produces, or `None` when empty.
#[must_use]
pub fn total_frames(segments: &[(f64, f64)], fps: f64, speed: f64) -> Option<usize> {
    let counts = segment_frame_counts(segments, fps, speed);
    let total: usize = counts.iter().map(|(_, _, count)| count).sum();
    (total > 0).then_some(total)
}

/// The source-time iterator for the exported frames.
// Ported from omnishot/studio.py (export_studio times generator)
#[must_use]
pub fn frame_times(segments: &[(f64, f64)], fps: f64, speed: f64) -> Vec<f64> {
    let mut times = Vec::new();
    for (a, b, count) in segment_frame_counts(segments, fps, speed) {
        let _ = b;
        for index in 0..count {
            times.push(a + index as f64 * speed / fps);
        }
    }
    times
}

/// Paint a cursor at `position` with `size` onto the canvas.
// Ported from omnishot/cursor_shape.py (paint_cursor)
pub fn paint_cursor(
    canvas: &mut Canvas<'_>,
    position: Point,
    size: f64,
    style: &str,
    fill: [u8; 4],
    outline: [u8; 4],
) {
    match style {
        "Dot" => {
            let radius = crate::video_cursor::dot_radius(size);
            let rect = Rect::new(
                position.x - radius,
                position.y - radius,
                radius * 2.0,
                radius * 2.0,
            );
            canvas.fill_ellipse(rect, outline);
            canvas.fill_ellipse(
                Rect::new(
                    position.x - radius * 0.6,
                    position.y - radius * 0.6,
                    radius * 1.2,
                    radius * 1.2,
                ),
                fill,
            );
        }
        "Crosshair" => {
            for (center, half_width, half_height) in
                crate::video_cursor::crosshair_bars(position, size)
            {
                canvas.fill_rounded_rect(
                    Rect::new(
                        center.x - half_width,
                        center.y - half_height,
                        half_width * 2.0,
                        half_height * 2.0,
                    ),
                    size * 0.06,
                    fill,
                );
            }
        }
        _ => {
            let points = crate::video_cursor::arrow_points(position, size);
            canvas.fill_polygon(&points, fill);
        }
    }
}

/// One composed frame from the source and optional camera.
///
/// Motion blur, when enabled, averages several sharp compositions sampled
/// inside the frame's shutter window before returning a single image.
// Ported from omnishot/studio.py (compose_frame) and
// omnishot/motion_blur.py (compose_motion)
#[must_use]
pub fn compose_frame(
    frame: &Image,
    t: f64,
    metadata: &Value,
    options: &Options,
    camera: Option<&Image>,
) -> Image {
    let times = crate::motion_blur::shutter_times(t, options);
    if times.len() == 1 || blur_level(options) == 0 {
        return compose_sharp_frame(frame, t, metadata, options, camera);
    }
    let mut accumulation = vec![0u32; frame.width * frame.height * 4];
    for seconds in &times {
        let rendered = compose_sharp_frame(frame, *seconds, metadata, options, camera);
        for (target, source) in accumulation.iter_mut().zip(rendered.data.iter()) {
            *target += u32::from(*source);
        }
    }
    let count = times.len() as u32;
    let data: Vec<u8> = accumulation
        .into_iter()
        .map(|value| ((value + count / 2) / count).min(255) as u8)
        .collect();
    Image::new(frame.width, frame.height, data)
}

/// One sharp composition, without motion blur.
// Ported from omnishot/studio.py (compose_screen / compose_scene)
#[must_use]
pub fn compose_sharp_frame(
    frame: &Image,
    t: f64,
    metadata: &Value,
    options: &Options,
    camera: Option<&Image>,
) -> Image {
    let (canvas, zoom) = compose_screen(frame, t, metadata, options);
    compose_scene(&canvas, t, metadata, options, camera, zoom)
}

/// Draw the input-layer effects (cursor and clicks) and apply the zoom crop.
// Ported from omnishot/studio.py (compose_screen)
#[must_use]
pub fn compose_screen(frame: &Image, t: f64, metadata: &Value, options: &Options) -> (Image, f64) {
    let width = frame.width;
    let height = frame.height;
    let mut data = frame.data.clone();
    let cursor = cursor_at(&CursorSample::list_from_json(metadata.get("cursor")), t);
    let events = edited_events(metadata, options);
    {
        let mut canvas = Canvas::new(&mut data, width, height);
        if let Some(cursor) = cursor
            && options.get("cursor").is_none_or(Value::is_truthy)
            && (0.0..=1.0).contains(&cursor.x)
            && (0.0..=1.0).contains(&cursor.y)
        {
            let size = options
                .get("cursor_size")
                .and_then(Value::as_f64)
                .unwrap_or(28.0)
                * if options.get("click_press").is_some_and(Value::is_truthy) {
                    press_scale(&events, t)
                } else {
                    1.0
                };
            let style = options
                .get("cursor_style")
                .and_then(Value::as_str)
                .unwrap_or("Arrow");
            let fill = color(options.get("cursor_color"), [0x16, 0x18, 0x21, 255]);
            let outline = color(options.get("cursor_outline"), [255, 255, 255, 255]);
            paint_cursor(
                &mut canvas,
                Point::new(cursor.x * width as f64, cursor.y * height as f64),
                size,
                style,
                fill,
                outline,
            );
        }
        if options.get("clicks").is_none_or(Value::is_truthy) {
            let duration = options
                .get("click_duration")
                .and_then(Value::as_f64)
                .unwrap_or(0.5)
                .max(0.1);
            let base = color(options.get("click_color"), [0xff, 0x65, 0x65, 255]);
            for event in &events {
                let Some(object) = event.as_object() else {
                    continue;
                };
                if object.get("kind").and_then(Value::as_str) != Some("click")
                    || object.get("hidden").is_some_and(Value::is_truthy)
                {
                    continue;
                }
                let Some(instant) = object.get("t").and_then(Value::as_f64) else {
                    continue;
                };
                let age = t - instant;
                if !(0.0..duration).contains(&age) {
                    continue;
                }
                let x = object.get("x").and_then(Value::as_f64).unwrap_or(0.5);
                let y = object.get("y").and_then(Value::as_f64).unwrap_or(0.5);
                let alpha = (base[3] as f64 * (1.0 - age / duration) * 0.8) as u8;
                let radius = options
                    .get("click_size")
                    .and_then(Value::as_f64)
                    .unwrap_or(24.0)
                    * (1.0 + age / duration * 0.5);
                let ring = [base[0], base[1], base[2], alpha];
                let ellipse = Rect::new(
                    x * width as f64 - radius,
                    y * height as f64 - radius,
                    radius * 2.0,
                    radius * 2.0,
                );
                if options.get("click_style").and_then(Value::as_str) == Some("filled") {
                    canvas.fill_ellipse(ellipse, ring);
                } else {
                    canvas.stroke_ellipse(ellipse, 3.0, ring);
                }
            }
        }
    }
    let cursor_value = cursor.map(|c| (c.x, c.y));
    let zooms = Zoom::list_from_json(options.get("zooms"));
    let animation = options
        .get("zoom_animation")
        .and_then(Value::as_str)
        .unwrap_or("Smooth");
    let (zoom, zx, zy) = zoom_at(&zooms, t, cursor_value, animation);
    if zoom > 1.0 {
        let canvas = Image::new(width, height, data);
        return (zoom_crop(&canvas, zoom, zx, zy), zoom);
    }
    (Image::new(width, height, data), zoom)
}

/// Crop the zoomed window and scale it back to the source size.
fn zoom_crop(frame: &Image, zoom: f64, zx: f64, zy: f64) -> Image {
    let width = frame.width;
    let height = frame.height;
    let crop_width = width as f64 / zoom;
    let crop_height = height as f64 / zoom;
    let x = (zx * width as f64 - crop_width / 2.0).clamp(0.0, width as f64 - crop_width);
    let y = (zy * height as f64 - crop_height / 2.0).clamp(0.0, height as f64 - crop_height);
    let mut data = vec![0u8; width * height * 4];
    for target_y in 0..height {
        let source_y = y + (target_y as f64 + 0.5) * crop_height / height as f64;
        let source_y = source_y.floor().clamp(0.0, height as f64 - 1.0) as usize;
        for target_x in 0..width {
            let source_x = x + (target_x as f64 + 0.5) * crop_width / width as f64;
            let source_x = source_x.floor().clamp(0.0, width as f64 - 1.0) as usize;
            let source = (source_y * width + source_x) * 4;
            let target = (target_y * width + target_x) * 4;
            data[target..target + 4].copy_from_slice(&frame.data[source..source + 4]);
        }
    }
    Image::new(width, height, data)
}

/// Frame the composed canvas in its background and place the camera and keys.
// Ported from omnishot/studio.py (compose_scene)
#[must_use]
pub fn compose_scene(
    canvas: &Image,
    t: f64,
    metadata: &Value,
    options: &Options,
    camera: Option<&Image>,
    zoom: f64,
) -> Image {
    let options = video_camera::options_at(metadata, options, t);
    let width = canvas.width;
    let height = canvas.height;
    let output_width = options
        .get("width")
        .and_then(Value::as_f64)
        .unwrap_or(width as f64);
    let output_width = if output_width <= 0.0 {
        width as f64
    } else {
        output_width
    };
    let content_height = (height as f64 * output_width / width as f64).round();
    let padding = options
        .get("padding")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let mut out_width = output_width + 2.0 * padding;
    let mut out_height = content_height + 2.0 * padding;
    if let Some(aspect) = options.get("aspect").and_then(Value::as_str)
        && aspect != "Original"
        && let Some((a, b)) = aspect.split_once(':')
        && let (Ok(a), Ok(b)) = (a.parse::<f64>(), b.parse::<f64>())
        && a > 0.0
        && b > 0.0
    {
        out_width = out_width.max((out_height * a / b).round());
        out_height = out_height.max((out_width * b / a).round());
    }
    out_width += out_width % 2.0;
    out_height += out_height % 2.0;
    let (out_width, out_height) = (out_width.max(2.0) as usize, out_height.max(2.0) as usize);

    let background = color(options.get("background"), [0x25, 0x32, 0x4b, 255]);
    let mut data = vec![0u8; out_width * out_height * 4];
    for pixel in data.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&background);
    }
    if let Some(second) = options.get("background2").and_then(Value::as_str) {
        let second = color(Some(&Value::String(second.to_owned())), [0, 0, 0, 255]);
        paint_gradient(&mut data, out_width, out_height, background, second);
    }
    if let Some(encoded) = options.get("background_image_data").and_then(Value::as_str)
        && let Some(image) = background_image(encoded)
    {
        let mut cover = Canvas::new(&mut data, out_width, out_height);
        cover.draw_image_cover(
            &image.data,
            image.width,
            image.height,
            Rect::new(0.0, 0.0, out_width as f64, out_height as f64),
        );
    }

    // Scale the composed source into the content rectangle.
    let scaled = scale_to(canvas, output_width as usize, content_height as usize);
    let align = options
        .get("align")
        .and_then(Value::as_str)
        .unwrap_or("Center");
    let x = if align.contains("Left") {
        padding
    } else if align.contains("Right") {
        out_width as f64 - output_width - padding
    } else {
        (out_width as f64 - output_width) / 2.0
    };
    let y = if align.contains("Top") {
        padding
    } else if align.contains("Bottom") {
        out_height as f64 - content_height - padding
    } else {
        (out_height as f64 - content_height) / 2.0
    };
    {
        let mut target = Canvas::new(&mut data, out_width, out_height);
        let radius = if padding > 0.0 {
            options
                .get("radius")
                .and_then(Value::as_f64)
                .unwrap_or(12.0)
        } else {
            0.0
        };
        let content_rect = Rect::new(x, y, output_width, content_height);
        if padding > 0.0 && options.get("shadow").is_none_or(Value::is_truthy) {
            paint_content_shadow(&mut target, content_rect, radius);
        }
        if radius > 0.0 {
            let mask = crate::raster::rounded_mask(
                out_width,
                out_height,
                Rect::new(x, y, output_width, content_height),
                radius,
                2.0,
            );
            blend_masked(
                &mut target,
                &scaled,
                x as i64,
                y as i64,
                output_width as usize,
                content_height as usize,
                &mask,
            );
        } else {
            target.draw_image(
                &scaled.data,
                scaled.width,
                scaled.height,
                x as i64,
                y as i64,
            );
        }
    }

    if let Some(camera) = camera
        && options.get("camera").is_none_or(Value::is_truthy)
    {
        let aspect = camera.width as f64 / camera.height.max(1) as f64;
        let rect = camera_rect(out_width as f64, out_height as f64, aspect, &options, zoom);
        let shape = options
            .get("camera_shape")
            .and_then(Value::as_str)
            .unwrap_or("Circle");
        let fullscreen = options
            .get("camera_fullscreen")
            .is_some_and(Value::is_truthy);
        let mut fitted = cover_fit(camera, rect);
        if options.get("camera_mirror").is_some_and(Value::is_truthy) {
            fitted = mirror_horizontal(&fitted);
        }
        let mut target = Canvas::new(&mut data, out_width, out_height);
        if options.get("camera_shadow").is_none_or(Value::is_truthy) && !fullscreen {
            let camera_radius = options
                .get("camera_radius")
                .and_then(Value::as_f64)
                .unwrap_or(18.0);
            let shadow_radius = if matches!(shape, "Rounded" | "Square") {
                camera_radius
            } else {
                0.0
            };
            paint_camera_shadow(&mut target, rect, shadow_radius, shape == "Circle");
        }
        let mask = if shape == "Circle" && !fullscreen {
            ellipse_mask(out_width, out_height, rect)
        } else {
            let radius = if matches!(shape, "Rounded" | "Square") && !fullscreen {
                options
                    .get("camera_radius")
                    .and_then(Value::as_f64)
                    .unwrap_or(18.0)
            } else {
                0.0
            };
            crate::raster::rounded_mask(out_width, out_height, rect, radius, 2.0)
        };
        blend_masked(
            &mut target,
            &fitted,
            rect.x as i64,
            rect.y as i64,
            rect.width.max(1.0) as usize,
            rect.height.max(1.0) as usize,
            &mask,
        );
    }

    if options.get("keys").is_none_or(Value::is_truthy) {
        let keys = visible_keys(&edited_events(metadata, &options), t, &options, metadata);
        if !keys.is_empty() {
            let (chip_background, chip_foreground) = key_colors(&options);
            let chip = color(
                Some(&Value::String(chip_background)),
                [0xd7, 0x14, 0x17, 0x20],
            );
            let foreground = color(Some(&Value::String(chip_foreground)), [255, 255, 255, 255]);
            let label = keys[keys.len().saturating_sub(5)..].join("  ");
            let font_size = options
                .get("key_size")
                .and_then(Value::as_f64)
                .unwrap_or(20.0);
            let chip_width = (label.chars().count() as f64 * font_size * 0.6 + 36.0)
                .min(out_width as f64 - 40.0)
                .max(1.0);
            let chip_height = font_size + 20.0;
            let position = options
                .get("key_position")
                .and_then(Value::as_str)
                .unwrap_or("Bottom Center");
            let cx = if position.contains("Left") {
                20.0
            } else if position.contains("Right") {
                out_width as f64 - chip_width - 20.0
            } else {
                (out_width as f64 - chip_width) / 2.0
            };
            let cy = if position.contains("Top") {
                24.0
            } else if position.contains("Bottom") {
                out_height as f64 - chip_height - 24.0
            } else {
                (out_height as f64 - chip_height) / 2.0
            };
            let chip_rect = Rect::new(cx, cy, chip_width, chip_height);
            let mut target = Canvas::new(&mut data, out_width, out_height);
            target.fill_rounded_rect(chip_rect, 12.0, chip);
            let count = label.chars().count();
            let text_left = chip_rect.center().x - count as f64 * font_size * 0.3;
            let baseline = chip_rect.center().y - font_size * 0.35;
            draw_ui_text(
                &mut target,
                text_left,
                baseline,
                &label,
                font_size,
                foreground,
            );
        }
    }

    Image::new(out_width, out_height, data)
}

/// Compose a frame with motion blur, when the options ask for it.
// Ported from omnishot/motion_blur.py (compose_motion)
#[must_use]
pub fn compose_motion(frame: &Image, t: f64, metadata: &Value, options: &Options) -> Image {
    compose_frame(frame, t, metadata, options, None)
}

/// The ffmpeg input options that accept raw RGB frames on stdin, including
/// the VAAPI device when hardware encoding is requested. The device option is
/// global and must precede the input.
// Ported from omnishot/studio.py (export_studio encoder selection)
#[must_use]
pub fn encoder_input_arguments(
    options: &Options,
    width: usize,
    height: usize,
    fps: f64,
) -> Vec<String> {
    let mut args = Vec::new();
    if options.get("hardware").is_some_and(Value::is_truthy) {
        let device = options
            .get("render_device")
            .and_then(Value::as_str)
            .unwrap_or("/dev/dri/renderD128");
        args.extend(["-vaapi_device".to_owned(), device.to_owned()]);
    }
    args.extend([
        "-f".to_owned(),
        "rawvideo".to_owned(),
        "-pixel_format".to_owned(),
        "rgb24".to_owned(),
        "-video_size".to_owned(),
        format!("{width}x{height}"),
        "-framerate".to_owned(),
        format!("{fps}"),
        "-i".to_owned(),
        "pipe:0".to_owned(),
    ]);
    args
}

/// The ffmpeg output options that choose the video encoder.
// Ported from omnishot/studio.py (export_studio encoder selection)
#[must_use]
pub fn encoder_selector_arguments(options: &Options) -> Vec<String> {
    if options.get("hardware").is_some_and(Value::is_truthy) {
        [
            "-vf",
            "format=nv12,hwupload",
            "-c:v",
            "h264_vaapi",
            "-qp",
            "22",
        ]
        .map(str::to_owned)
        .to_vec()
    } else {
        [
            "-c:v", "libx264", "-crf", "20", "-preset", "fast", "-pix_fmt", "yuv420p",
        ]
        .map(str::to_owned)
        .to_vec()
    }
}

/// Build the standalone `ffmpeg` encoder arguments for a stdin-only export.
///
/// Software x264 is the default; hardware encoding is opt-in and uses VAAPI
/// only when the caller asks and supplies a render device.
// Ported from omnishot/studio.py (export_studio encoder selection)
#[must_use]
pub fn encoder_arguments(options: &Options, width: usize, height: usize, fps: f64) -> Vec<String> {
    let mut args = encoder_input_arguments(options, width, height, fps);
    args.extend(encoder_selector_arguments(options));
    args.push("-movflags".to_owned());
    args.push("+faststart".to_owned());
    args
}

/// The audio filters that follow a selected/trimmed source stream.
///
/// Cut intervals are removed on the post-seek source clock and a non-unit
/// speed is compressed with `atempo`, matching the recorded edit.
// Ported from omnishot/studio.py (export_studio audio arguments)
#[must_use]
pub fn audio_edit_filters(start: f64, speed: f64, cuts: &[(f64, f64)]) -> Vec<String> {
    let mut filters = Vec::new();
    let terms: Vec<String> = cuts
        .iter()
        .filter(|(_, b)| *b > start)
        .map(|(a, b)| format!("between(t,{},{})", (a - start).max(0.0), b - start))
        .collect();
    if !terms.is_empty() {
        filters.push(format!("aselect='not({})'", terms.join("+")));
        filters.push("asetpts=N/SR/TB".to_owned());
    }
    if (speed - 1.0).abs() > f64::EPSILON {
        filters.push(format!("atempo={speed}"));
    }
    filters
}

/// Export an edited recording through the ffmpeg CLI.
///
/// `decoder` and `encoder` are the runner and command builder; tests pass a
/// runner that records the invocation. The source is decoded with `ffmpeg`,
/// each output frame is composited, and the result is piped into the encoder.
pub struct ExportRequest<'a> {
    pub source: &'a Path,
    pub destination: &'a Path,
    pub metadata: &'a Value,
    pub options: &'a Options,
    pub fps: f64,
    pub speed: f64,
    pub start: f64,
    pub end: f64,
    pub cuts: &'a [(f64, f64)],
}

/// Resolve the export's frame size, or `None` when there is no frame.
#[must_use]
pub fn export_plan(request: &ExportRequest<'_>) -> Option<ExportPlan> {
    let segments = kept_segments(request.start, request.end, request.cuts);
    let total = total_frames(&segments, request.fps, request.speed)?;
    let times = frame_times(&segments, request.fps, request.speed);
    Some(ExportPlan {
        segments,
        times,
        total,
    })
}

/// A resolved export: retained intervals, frame times and total count.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportPlan {
    pub segments: Vec<(f64, f64)>,
    pub times: Vec<f64>,
    pub total: usize,
}

/// The partial path a failed export cleans up.
#[must_use]
pub fn partial_path(destination: &Path) -> PathBuf {
    let stem = destination
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("export");
    let suffix = destination
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("mp4");
    destination.with_file_name(format!("{stem}.partial.{suffix}"))
}

/// Run the export, returning the destination on success.
///
/// The source is probed with `ffprobe` for its frame rate and size, decoded
/// with `ffmpeg`, composited, and encoded with [`encoder_arguments`]. A failing
/// command surfaces its stderr; the partial file is removed.
pub fn export_studio<R: CommandRunner>(
    runner: &R,
    request: &ExportRequest<'_>,
) -> Result<(), String> {
    let plan = export_plan(request).ok_or_else(|| "The edit contains no frames".to_owned())?;
    let probe = runner
        .run(
            &CommandSpec::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=width,height,avg_frame_rate",
                    "-of",
                    "json",
                ])
                .arg(request.source.to_string_lossy().into_owned())
                .timeout(std::time::Duration::from_secs(15)),
        )
        .map_err(|error| error.to_string())?;
    let probe = crate::json::parse(&probe.stdout_string())
        .map_err(|_| "Could not read the source recording".to_owned())?;
    let stream = probe
        .get("streams")
        .and_then(Value::as_array)
        .and_then(|streams| streams.first())
        .ok_or_else(|| "Cannot open source recording".to_owned())?;
    let width = stream.get("width").and_then(Value::as_f64).unwrap_or(0.0) as usize;
    let height = stream.get("height").and_then(Value::as_f64).unwrap_or(0.0) as usize;
    if width == 0 || height == 0 {
        return Err("Cannot open source recording".to_owned());
    }
    let source_fps = stream
        .get("avg_frame_rate")
        .and_then(Value::as_str)
        .and_then(parse_frame_rate)
        .filter(|fps| fps.is_finite() && *fps > 0.0)
        .unwrap_or(30.0);

    let metadata = request.metadata;
    let options = request.options;
    let per_frame = width * height * 3;

    // The first composed frame decides the output size, so decode its whole
    // segment once and feed that buffer to the streaming producer rather than
    // decoding the first segment twice.
    let first_index = plan
        .segments
        .iter()
        .position(|(a, b)| segment_frame_counts(&[(*a, *b)], request.fps, request.speed)[0].2 > 0)
        .ok_or_else(|| "The edit contains no frames".to_owned())?;
    let (first_start, first_end) = plan.segments[first_index];
    let first_decoded = decode_segment(runner, request.source, first_start, first_end)?;
    let first_available = first_decoded.len() / per_frame;
    if first_available == 0 {
        return Err("Could not decode recording".to_owned());
    }
    let first_time = first_start;
    let first_image = rgb_to_image(&first_decoded[..per_frame], width, height);
    let first_composed = compose_frame(&first_image, first_time, metadata, options, None);
    let (out_width, out_height) = (first_composed.width, first_composed.height);

    let camera_path = metadata
        .get("camera_path")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let camera_offset = metadata
        .get("camera_offset")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let camera_size = camera_path
        .as_deref()
        .and_then(|path| probe_video_size(runner, path));

    let segments = plan.segments.clone();
    let source = request.source.to_path_buf();
    let metadata = metadata.clone();
    let options = options.clone();
    let fps = request.fps;
    let speed = request.speed;
    let error_slot = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let (sender, reader) = std::sync::mpsc::sync_channel::<Vec<u8>>(4);

    let partial = partial_path(request.destination);
    let mut command = CommandSpec::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(encoder_input_arguments(
            &options, out_width, out_height, fps,
        ))
        .args(["-ss"])
        .arg(request.start.to_string())
        .args(["-i"])
        .arg(source.to_string_lossy().into_owned())
        .args(["-map", "0:v"])
        .args(encoder_selector_arguments(&options));
    let audio_filters = audio_edit_filters(request.start, speed, request.cuts);
    command = command.args(crate::video_audio_tracks::export_audio_args(
        &options,
        1,
        &audio_filters,
    ));
    command = command
        .args(["-c:a", "aac", "-shortest", "-movflags", "+faststart"])
        .arg(partial.to_string_lossy().into_owned());
    command.timeout = Some(std::time::Duration::from_secs(3600));

    let result = std::thread::scope(|scope| {
        let runner_ref = runner;
        let error = std::sync::Arc::clone(&error_slot);
        let first_decoded = first_decoded;
        let first_composed = first_composed;
        let mut first_composed = Some(first_composed);
        let camera_path = camera_path.clone();
        scope.spawn(move || {
            let emit = |bytes: &[u8]| sender.send(bytes.to_vec()).is_ok();
            // Compose the retained intervals in order and stream each frame.
            for (segment_index, (a, b)) in segments.iter().enumerate() {
                let count = segment_frame_counts(&[(*a, *b)], fps, speed)[0].2;
                if count == 0 {
                    continue;
                }
                let decoded = if segment_index == first_index {
                    first_decoded.clone()
                } else {
                    match decode_segment(runner_ref, &source, *a, *b) {
                        Ok(bytes) => bytes,
                        Err(message) => {
                            *error.lock().unwrap() = Some(message);
                            return;
                        }
                    }
                };
                let available = decoded.len() / per_frame;
                if available == 0 {
                    *error.lock().unwrap() = Some("Could not decode recording".to_owned());
                    return;
                }
                for index in 0..count {
                    let t = a + index as f64 * speed / fps;
                    let composed = if segment_index == first_index && index == 0 {
                        first_composed.take().unwrap()
                    } else {
                        let local = (((t - a) * source_fps).round() as usize).min(available - 1);
                        let offset = local * per_frame;
                        let image =
                            rgb_to_image(&decoded[offset..offset + per_frame], width, height);
                        let camera = camera_path.as_deref().and_then(|path| {
                            let size = camera_size?;
                            decode_camera_frame(
                                runner_ref,
                                path,
                                (t - camera_offset).max(0.0),
                                size,
                            )
                        });
                        compose_frame(&image, t, &metadata, &options, camera.as_ref())
                    };
                    if composed.width != out_width || composed.height != out_height {
                        *error.lock().unwrap() =
                            Some("The composition changed size mid-export".to_owned());
                        return;
                    }
                    if !emit(&pixels_to_rgb(&composed)) {
                        // The encoder closed its stdin; stop producing.
                        return;
                    }
                }
            }
        });
        let mut reader = ChannelReader::new(reader);
        let streaming = runner_ref.run_streaming(&command, &mut reader);
        (streaming, ())
    });
    let (streaming, ()) = result;
    if let Some(message) = error_slot.lock().unwrap().take() {
        let _ = std::fs::remove_file(&partial);
        return Err(message);
    }
    match streaming {
        Ok(_) => std::fs::rename(&partial, request.destination).map_err(|error| error.to_string()),
        Err(error) => {
            let _ = std::fs::remove_file(&partial);
            Err(error.to_string())
        }
    }
}

/// Decode one retained interval to raw RGB24 bytes.
fn decode_segment<R: CommandRunner>(
    runner: &R,
    source: &Path,
    start: f64,
    end: f64,
) -> Result<Vec<u8>, String> {
    let decoded = runner
        .run(
            &CommandSpec::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error"])
                .args(["-ss"])
                .arg(start.to_string())
                .args(["-t"])
                .arg((end - start).to_string())
                .args(["-i"])
                .arg(source.to_string_lossy().into_owned())
                .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
                .timeout(std::time::Duration::from_secs(30)),
        )
        .map_err(|error| error.to_string())?;
    Ok(decoded.stdout)
}

/// Probe a video file's frame size, or `None` when it is unreadable.
fn probe_video_size<R: CommandRunner>(runner: &R, path: &Path) -> Option<(usize, usize)> {
    let output = runner
        .run(
            &CommandSpec::new("ffprobe")
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=width,height",
                    "-of",
                    "json",
                ])
                .arg(path.to_string_lossy().into_owned())
                .timeout(std::time::Duration::from_secs(15)),
        )
        .ok()?;
    let probe = crate::json::parse(&output.stdout_string()).ok()?;
    let stream = probe
        .get("streams")
        .and_then(Value::as_array)
        .and_then(|streams| streams.first())?;
    let width = stream.get("width").and_then(Value::as_f64)? as usize;
    let height = stream.get("height").and_then(Value::as_f64)? as usize;
    (width > 0 && height > 0).then_some((width, height))
}

/// Decode a single camera frame at `time`, or `None` when it is unavailable.
fn decode_camera_frame<R: CommandRunner>(
    runner: &R,
    path: &Path,
    time: f64,
    size: (usize, usize),
) -> Option<Image> {
    let (width, height) = size;
    let decoded = runner
        .run(
            &CommandSpec::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error"])
                .args(["-ss"])
                .arg(time.to_string())
                .args(["-i"])
                .arg(path.to_string_lossy().into_owned())
                .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
                .timeout(std::time::Duration::from_secs(15)),
        )
        .ok()?;
    if decoded.stdout.len() < width * height * 3 {
        return None;
    }
    Some(rgb_to_image(&decoded.stdout, width, height))
}

/// A `Read` adapter over framed byte chunks, for streaming encoder input.
struct ChannelReader {
    receiver: std::sync::mpsc::Receiver<Vec<u8>>,
    current: Vec<u8>,
    offset: usize,
}

impl ChannelReader {
    fn new(receiver: std::sync::mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            receiver,
            current: Vec::new(),
            offset: 0,
        }
    }
}

impl std::io::Read for ChannelReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        while self.offset >= self.current.len() {
            match self.receiver.recv() {
                Ok(next) => {
                    self.current = next;
                    self.offset = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let available = self.current.len() - self.offset;
        let count = available.min(buffer.len());
        buffer[..count].copy_from_slice(&self.current[self.offset..self.offset + count]);
        self.offset += count;
        Ok(count)
    }
}

fn parse_frame_rate(value: &str) -> Option<f64> {
    let (numerator, denominator) = value.split_once('/')?;
    let numerator: f64 = numerator.parse().ok()?;
    let denominator: f64 = denominator.parse().ok()?;
    if denominator == 0.0 {
        return None;
    }
    Some(numerator / denominator)
}

fn rgb_to_image(bytes: &[u8], width: usize, height: usize) -> Image {
    let mut data = vec![255u8; width * height * 4];
    for (pixel, rgb) in data
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(bytes.as_chunks::<3>().0)
    {
        pixel[0] = rgb[0];
        pixel[1] = rgb[1];
        pixel[2] = rgb[2];
    }
    Image::new(width, height, data)
}

fn pixels_to_rgb(image: &Image) -> Vec<u8> {
    let mut out = Vec::with_capacity(image.width * image.height * 3);
    for pixel in image.data.as_chunks::<4>().0 {
        out.extend_from_slice(&pixel[..3]);
    }
    out
}

fn color(value: Option<&Value>, fallback: [u8; 4]) -> [u8; 4] {
    value
        .and_then(Value::as_str)
        .and_then(crate::color::Rgba::from_hex)
        .map(|rgba| [rgba.red, rgba.green, rgba.blue, rgba.alpha])
        .unwrap_or(fallback)
}

fn paint_gradient(data: &mut [u8], width: usize, height: usize, first: [u8; 4], second: [u8; 4]) {
    let denominator = (width + height).max(1) as f64;
    for y in 0..height {
        for x in 0..width {
            let t = (x + y) as f64 / denominator;
            let offset = (y * width + x) * 4;
            for channel in 0..4 {
                let value = f64::from(first[channel])
                    + (f64::from(second[channel]) - f64::from(first[channel])) * t;
                data[offset + channel] = value.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn scale_to(image: &Image, width: usize, height: usize) -> Image {
    if width == 0 || height == 0 {
        return Image::transparent(width.max(1), height.max(1));
    }
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

/// Crop `source` to the aspect of `dest`, then scale it to `dest`'s size.
fn cover_fit(source: &Image, dest: Rect) -> Image {
    let width = dest.width.max(1.0) as usize;
    let height = dest.height.max(1.0) as usize;
    scale_to(source, width, height)
}

/// Mirror an image left-to-right, as the camera's `camera_mirror` option asks.
// Ported from omnishot/studio.py (compose_scene camera flip)
fn mirror_horizontal(image: &Image) -> Image {
    let width = image.width;
    let height = image.height;
    let mut data = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let source = (y * width + x) * 4;
            let target = (y * width + (width - 1 - x)) * 4;
            data[target..target + 4].copy_from_slice(&image.data[source..source + 4]);
        }
    }
    Image::new(width, height, data)
}

/// The soft drop shadow the source camera draws under a framed picture.
// Ported from omnishot/studio.py (compose_scene shadow loop)
fn paint_content_shadow(canvas: &mut Canvas<'_>, rect: Rect, radius: f64) {
    for step in (1..=15).rev() {
        let spread = step as f64;
        let shadow = Rect::new(
            rect.x - spread,
            rect.y - spread * 0.4,
            rect.width + 2.0 * spread,
            rect.height + 1.4 * spread,
        );
        canvas.fill_rounded_rect(shadow, radius + spread, [0, 0, 0, 3]);
    }
}

/// The soft shadow behind the camera bubble or card.
// Ported from omnishot/studio.py (compose_scene camera shadow loop)
fn paint_camera_shadow(canvas: &mut Canvas<'_>, rect: Rect, shape_radius: f64, circular: bool) {
    for step in (1..=12).rev() {
        let spread = step as f64;
        let shadow = Rect::new(
            rect.x - spread,
            rect.y - spread / 2.0,
            rect.width + 2.0 * spread,
            rect.height + 1.5 * spread,
        );
        if circular {
            canvas.fill_ellipse(shadow, [0, 0, 0, 4]);
        } else {
            canvas.fill_rounded_rect(shadow, shape_radius + spread, [0, 0, 0, 4]);
        }
    }
}

/// Draw chip text through the raster bitmap face.
// Ported from the omnishot/studio.py key-chip text boundary
fn draw_ui_text(
    canvas: &mut Canvas<'_>,
    x: f64,
    y: f64,
    text: &str,
    font_size: f64,
    color: [u8; 4],
) {
    crate::raster::draw_bitmap_text(canvas, x, y, text, font_size, color);
}

fn blend_masked(
    target: &mut Canvas<'_>,
    source: &Image,
    dx: i64,
    dy: i64,
    width: usize,
    height: usize,
    mask: &[u8],
) {
    let canvas_width = target.width() as i64;
    let canvas_height = target.height() as i64;
    for row in 0..height {
        let y = dy + row as i64;
        if y < 0 || y >= canvas_height {
            continue;
        }
        for column in 0..width {
            let x = dx + column as i64;
            if x < 0 || x >= canvas_width {
                continue;
            }
            if mask[y as usize * target.width() + x as usize] == 0 {
                continue;
            }
            if column >= source.width || row >= source.height {
                continue;
            }
            let offset = (row * source.width + column) * 4;
            target.blend(
                x as usize,
                y as usize,
                [
                    source.data[offset],
                    source.data[offset + 1],
                    source.data[offset + 2],
                    source.data[offset + 3],
                ],
            );
        }
    }
}

fn ellipse_mask(width: usize, height: usize, rect: Rect) -> Vec<u8> {
    let mut mask = vec![0u8; width * height];
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return mask;
    }
    let cx = rect.center().x;
    let cy = rect.center().y;
    let rx = rect.width / 2.0;
    let ry = rect.height / 2.0;
    for y in 0..height {
        let dy = (y as f64 + 0.5 - cy) / ry;
        if dy.abs() > 1.0 {
            continue;
        }
        let span = (1.0 - dy * dy).sqrt() * rx;
        let x0 = ((cx - span).floor().max(0.0) as usize).min(width);
        let x1 = ((cx + span).ceil().max(0.0) as usize).min(width);
        mask[y * width + x0..y * width + x1].fill(255);
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(entries: &[(&str, Value)]) -> Options {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    fn click(t: f64, x: f64, y: f64) -> Value {
        Value::Object(BTreeMap::from([
            ("kind".to_owned(), Value::String("click".to_owned())),
            ("t".to_owned(), Value::Number(t)),
            ("x".to_owned(), Value::Number(x)),
            ("y".to_owned(), Value::Number(y)),
        ]))
    }

    #[test]
    fn kept_segments_clip_and_union_cuts() {
        assert_eq!(
            kept_segments(0.0, 10.0, &[(2.0, 4.0), (3.0, 5.0), (8.0, 12.0)]),
            vec![(0.0, 2.0), (5.0, 8.0)]
        );
        assert_eq!(kept_segments(0.0, 10.0, &[]), vec![(0.0, 10.0)]);
    }

    #[test]
    fn frame_times_walk_each_retained_segment_at_the_output_rate() {
        let segments = [(0.0, 1.0)];
        let times = frame_times(&segments, 10.0, 1.0);
        assert_eq!(times.len(), 10);
        assert!((times[0] - 0.0).abs() < 1e-9);
        assert!((times[9] - 0.9).abs() < 1e-9);
        // Doubling the speed halves the frame count.
        assert_eq!(frame_times(&segments, 10.0, 2.0).len(), 5);
        assert_eq!(total_frames(&[(0.0, 1.0)], 10.0, 1.0), Some(10));
        assert_eq!(total_frames(&[], 10.0, 1.0), None);
    }

    #[test]
    fn cursor_smoothing_pulls_toward_the_recorded_path() {
        let samples = vec![
            CursorSample {
                t: 0.0,
                x: 0.2,
                y: 0.2,
            },
            CursorSample {
                t: 0.05,
                x: 0.8,
                y: 0.8,
            },
        ];
        let smoothed = smooth_cursor(&samples, 0.75);
        assert!(smoothed[1].x > 0.2 && smoothed[1].x < 0.8);
        let at = cursor_at(&samples, 0.025).unwrap();
        assert!((at.x - 0.5).abs() < 1e-9);
        assert!(cursor_at(&[], 0.0).is_none());
    }

    #[test]
    fn smart_zooms_respect_limit_spacing_and_invalid_positions() {
        let events = vec![
            click(1.0, 0.3, 0.4),
            click(3.8, 0.8, 0.5),
            click(3.9, 2.0, 0.5),
        ];
        let zooms = smart_zooms(&events, 2.8, 1.8, Some(4.0));
        assert_eq!(zooms.len(), 2);
        assert!(zooms.iter().all(|z| z.end <= 4.0));
        assert!(zooms[0].follow);
        // The click outside 0..1 is skipped.
        assert_eq!(
            smart_zooms(&[click(1.0, 2.0, 0.5)], 2.8, 1.8, None).len(),
            0
        );
    }

    #[test]
    fn edited_events_apply_edits_then_sort() {
        let metadata = Value::Object(BTreeMap::from([(
            "events".to_owned(),
            Value::Array(vec![click(1.0, 0.1, 0.1), click(2.0, 0.2, 0.2)]),
        )]));
        let options = options(&[(
            "event_edits",
            Value::Object(BTreeMap::from([(
                "0".to_owned(),
                Value::Object(BTreeMap::from([("t".to_owned(), Value::Number(3.0))])),
            )])),
        )]);
        let edited = edited_events(&metadata, &options);
        assert!((edited[1].get("t").and_then(Value::as_f64).unwrap() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn encoder_defaults_to_software_x264_and_hardware_is_opt_in() {
        let software = encoder_arguments(&Options::new(), 320, 180, 30.0);
        assert!(software.contains(&"libx264".to_owned()));
        assert!(!software.contains(&"h264_vaapi".to_owned()));
        assert!(software.windows(2).any(|w| w == ["-video_size", "320x180"]));
        let hardware =
            encoder_arguments(&options(&[("hardware", Value::Bool(true))]), 320, 180, 30.0);
        assert!(hardware.contains(&"h264_vaapi".to_owned()));
        assert!(hardware.contains(&"/dev/dri/renderD128".to_owned()));
    }

    #[test]
    fn export_plan_reports_the_retained_frames() {
        let request = ExportRequest {
            source: Path::new("/tmp/source.mp4"),
            destination: Path::new("/tmp/export.mp4"),
            metadata: &Value::Null,
            options: &Options::new(),
            fps: 10.0,
            speed: 1.0,
            start: 0.0,
            end: 1.0,
            cuts: &[(0.4, 0.6)],
        };
        let plan = export_plan(&request).unwrap();
        assert_eq!(plan.segments, vec![(0.0, 0.4), (0.6, 1.0)]);
        assert_eq!(plan.total, 8);
        assert_eq!(
            partial_path(request.destination),
            PathBuf::from("/tmp/export.partial.mp4")
        );
    }

    #[test]
    fn composition_frames_a_cover_background_and_keeps_the_centre() {
        let mut frame = Image::new(20, 10, vec![0u8; 20 * 10 * 4]);
        for pixel in frame.data.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&[10, 20, 30, 255]);
        }
        let metadata = Value::Object(BTreeMap::new());
        let options = options(&[
            ("padding", Value::Number(4.0)),
            ("width", Value::Number(20.0)),
            ("aspect", Value::String("1:1".to_owned())),
            ("background", Value::String("#25304f".to_owned())),
            ("cursor", Value::Bool(false)),
            ("clicks", Value::Bool(false)),
            ("keys", Value::Bool(false)),
        ]);
        let result = compose_frame(&frame, 0.0, &metadata, &options, None);
        assert_eq!(result.width, result.height);
        assert_eq!(result.pixel(1, 1), [0x25, 0x30, 0x4f, 255]);
    }

    #[test]
    fn motion_blur_averages_available_shutter_samples() {
        let mut frame = Image::new(8, 8, vec![0u8; 8 * 8 * 4]);
        for pixel in frame.data.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&[200, 200, 200, 255]);
        }
        let metadata = Value::Object(BTreeMap::new());
        let options = options(&[
            ("motion_blur", Value::Number(3.0)),
            ("fps", Value::Number(10.0)),
            ("cursor", Value::Bool(false)),
            ("clicks", Value::Bool(false)),
            ("keys", Value::Bool(false)),
        ]);
        let blurred = compose_motion(&frame, 0.6, &metadata, &options);
        assert_eq!((blurred.width, blurred.height), (8, 8));
    }
}
