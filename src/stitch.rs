//! Conservative overlap stitching for scrolling captures.
// Ported from omnishot/stitch.py
//!
//! A scrolling capture grows one image by appending only the rows a new frame
//! reveals. Alignment is decided by independent textured template anchors; an
//! ambiguous or repeated layout is rejected instead of guessing a shift, so a
//! capture can never silently drop or duplicate page rows. Matching reads
//! borrowed slices of the two frames and never copies them; the accumulated
//! image is the only buffer that grows.

use std::collections::BTreeSet;
use std::fmt;

/// Fraction of the frame height where texture anchors are sampled.
const ANCHORS: [f64; 6] = [0.12, 0.27, 0.43, 0.60, 0.76, 0.87];

/// The megapixel ceiling shared with screenshots and imports.
pub const MAX_PIXELS: usize = 120_000_000;

/// Why a frame could not be stitched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StitchError {
    /// The new frame has different dimensions from the first one.
    DimensionsChanged,
    /// The selection is below the minimum matchable size.
    TooSmall,
    /// The accumulated image would exceed [`MAX_PIXELS`].
    TooLarge,
}

impl fmt::Display for StitchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StitchError::DimensionsChanged => {
                f.write_str("Capture dimensions changed; start a new scrolling capture.")
            }
            StitchError::TooSmall => f.write_str("Select an area at least 24 × 32 pixels."),
            StitchError::TooLarge => {
                f.write_str("Capture reached the 120 megapixel limit. Finish this capture.")
            }
        }
    }
}

impl std::error::Error for StitchError {}

/// An immutable view of one frame. Rows are contiguous and `channels` is 3
/// (RGB) or 4 (RGBA).
#[derive(Debug, Clone, Copy)]
pub struct FrameRef<'a> {
    data: &'a [u8],
    width: usize,
    height: usize,
    channels: usize,
}

impl<'a> FrameRef<'a> {
    #[must_use]
    pub fn new(data: &'a [u8], width: usize, height: usize, channels: usize) -> Self {
        debug_assert!(channels == 3 || channels == 4, "frames are RGB or RGBA");
        debug_assert!(data.len() >= width * height * channels);
        Self {
            data,
            width,
            height,
            channels,
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

    #[must_use]
    pub fn channels(&self) -> usize {
        self.channels
    }

    #[must_use]
    pub fn as_bytes(&self) -> &'a [u8] {
        self.data
    }

    fn row(&self, y: usize) -> &'a [u8] {
        let start = y * self.width * self.channels;
        &self.data[start..start + self.width * self.channels]
    }
}

/// A frame owned by the stitcher's accumulated image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameData {
    pub data: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub channels: usize,
}

impl FrameData {
    #[must_use]
    pub fn new(data: Vec<u8>, width: usize, height: usize, channels: usize) -> Self {
        debug_assert!(data.len() >= width * height * channels);
        Self {
            data,
            width,
            height,
            channels,
        }
    }

    /// Copy a borrowed view into an owned frame. The one copy is the frame the
    /// stitcher must retain; matching never copies.
    #[must_use]
    pub fn from_view(frame: FrameRef<'_>) -> Self {
        Self::new(
            frame.data.to_vec(),
            frame.width,
            frame.height,
            frame.channels,
        )
    }

    #[must_use]
    pub fn view(&self) -> FrameRef<'_> {
        FrameRef::new(&self.data, self.width, self.height, self.channels)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    fn rows(&self, range: std::ops::Range<usize>) -> &[u8] {
        let row = self.width * self.channels;
        &self.data[range.start * row..range.end * row]
    }
}

/// One candidate displacement between two frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Match {
    pub shift: i32,
    pub confidence: f32,
    pub error: f32,
}

/// A grayscale, width-bounded copy of one frame used for matching.
struct Gray {
    data: Vec<f32>,
    width: usize,
    height: usize,
}

impl Gray {
    fn from_frame(frame: FrameRef<'_>) -> Self {
        let target_width = frame.width.min(320);
        let mut data = Vec::with_capacity(target_width * frame.height);
        for y in 0..frame.height {
            let row = frame.row(y);
            for column in 0..target_width {
                let left = column * frame.width / target_width;
                let right = ((column + 1) * frame.width / target_width).max(left + 1);
                let mut sum = 0.0f64;
                for x in left..right {
                    let offset = x * frame.channels;
                    let r = f64::from(row[offset]);
                    let g = f64::from(row[offset + 1]);
                    let b = f64::from(row[offset + 2]);
                    sum += (r * 299.0 + g * 587.0 + b * 114.0) / 1000.0;
                }
                data.push((sum / (right - left) as f64) as f32);
            }
        }
        Self {
            data,
            width: target_width,
            height: frame.height,
        }
    }

    fn value(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }
}

/// The mean absolute per-row difference between two frames, used to find the
/// stationary header and footer rows.
fn same_rows(previous: FrameRef<'_>, current: FrameRef<'_>) -> Vec<bool> {
    (0..previous.height)
        .map(|y| {
            let a = previous.row(y);
            let b = current.row(y);
            let sum: f64 = a
                .iter()
                .zip(b)
                .map(|(x, y)| f64::from(x.abs_diff(*y)))
                .sum();
            (sum / a.len() as f64) < 1.2
        })
        .collect()
}

/// Transpose a frame through an owned copy of its pixels.
///
/// Used only at the horizontal boundary: once per incoming frame and once for
/// the finished output, never inside the matching loop.
fn transpose(frame: FrameRef<'_>) -> FrameData {
    let (width, height, channels) = (frame.height, frame.width, frame.channels);
    let mut data = vec![0u8; frame.data.len()];
    for y in 0..frame.height {
        for x in 0..frame.width {
            let source = (y * frame.width + x) * channels;
            let target = (x * width + y) * channels;
            data[target..target + channels].copy_from_slice(&frame.data[source..source + channels]);
        }
    }
    FrameData::new(data, width, height, channels)
}

/// `matchTemplate` with `TM_CCOEFF_NORMED`, sliding only vertically because
/// the anchor already spans the bordered width. `template` is indexed
/// `[relative_row * width + x]`.
fn correlate(image: &Gray, template: &[f32], left: usize, right: usize, top: usize) -> Vec<f32> {
    let template_w = right - left;
    let template_h = template.len() / template_w;
    let template_mean = template.iter().sum::<f32>() / template.len() as f32;
    let template_norm = template
        .iter()
        .map(|v| (v - template_mean) * (v - template_mean))
        .sum::<f32>()
        .sqrt();

    let max_location = image.height - template_h;
    // Sliding row sums keep the per-location mean and variance cheap; only the
    // weighted correlation below needs the inner template loop.
    let mut row_sums = vec![0.0f32; image.width];
    let mut row_sq = vec![0.0f32; image.width];
    for y in top..top + template_h {
        for x in left..right {
            let v = image.value(x, y);
            row_sums[x] += v;
            row_sq[x] += v * v;
        }
    }

    let mut scores = Vec::with_capacity(max_location + 1);
    for location in 0..=max_location {
        let mut sum = 0.0f32;
        let mut sum_sq = 0.0f32;
        for x in left..right {
            sum += row_sums[x];
            sum_sq += row_sq[x];
        }
        let count = (template_h * template_w) as f32;
        let mean = sum / count;
        let variance = (sum_sq - sum * sum / count).max(0.0);
        let norm = variance.sqrt();
        let score = if template_norm <= f32::EPSILON || norm <= f32::EPSILON {
            0.0
        } else {
            let mut dot = 0.0f32;
            for y in 0..template_h {
                for x in left..right {
                    dot += image.value(x, location + y) * template[y * template_w + (x - left)];
                }
            }
            (dot - mean * template.iter().sum::<f32>()) / (template_norm * norm)
        };
        scores.push(score);
        if location < max_location {
            for x in left..right {
                let leaving = image.value(x, location);
                let entering = image.value(x, location + template_h);
                row_sums[x] += entering - leaving;
                row_sq[x] += entering * entering - leaving * leaving;
            }
        }
    }
    scores
}

/// The standard deviation of a block, used to skip flat anchors.
fn block_std(image: &Gray, left: usize, right: usize, top: usize, bottom: usize) -> f32 {
    let mut sum = 0.0f32;
    let mut sum_sq = 0.0f32;
    for y in top..bottom {
        for x in left..right {
            let value = image.value(x, y);
            sum += value;
            sum_sq += value * value;
        }
    }
    let count = ((right - left) * (bottom - top)) as f32;
    let mean = sum / count;
    (sum_sq / count - mean * mean).max(0.0).sqrt()
}

/// A 5×5 box blur approximating the reference Gaussian, used to isolate fine
/// detail for the overlap error.
fn blur(value: &Gray) -> Vec<f32> {
    let mut out = vec![0.0f32; value.data.len()];
    for y in 0..value.height {
        for x in 0..value.width {
            let mut sum = 0.0f32;
            let mut count = 0.0f32;
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= value.width as i32 || ny >= value.height as i32 {
                        continue;
                    }
                    sum += value.value(nx as usize, ny as usize);
                    count += 1.0;
                }
            }
            out[y * value.width + x] = sum / count;
        }
    }
    out
}

/// One candidate anchor peak.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    shift: i32,
    score: f32,
    anchor: usize,
}

/// Find vertical displacement using independent textured strip anchors.
///
/// Positive displacement means forward/down. Static sidebars and scrollbar
/// edges are excluded by the border. Anchor consensus prevents a single
/// animated area or a repeated line from deciding the whole stitch.
// Ported from omnishot/stitch.py (match_translation)
pub fn match_translation(
    previous: FrameRef<'_>,
    current: FrameRef<'_>,
) -> Result<Match, StitchError> {
    if previous.width != current.width
        || previous.height != current.height
        || previous.channels != current.channels
    {
        return Err(StitchError::DimensionsChanged);
    }
    let (h, w) = (previous.height, previous.width);
    if h < 32 || w < 24 {
        return Err(StitchError::TooSmall);
    }
    let old = Gray::from_frame(previous);
    let new = Gray::from_frame(current);
    let tw = old.width;

    let mean_diff: f64 = old
        .data
        .iter()
        .zip(&new.data)
        .map(|(a, b)| f64::from((a - b).abs()))
        .sum::<f64>()
        / old.data.len() as f64;
    if mean_diff < 0.7 {
        return Ok(Match {
            shift: 0,
            confidence: 1.0,
            error: 0.0,
        });
    }

    let border = 3.max(tw / 12);
    let sh = 56.min(16.max(h / 10));
    let left = border;
    let right = tw.saturating_sub(border);
    if right <= left {
        return Ok(Match {
            shift: 0,
            confidence: 0.0,
            error: 255.0,
        });
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    for (anchor_index, fraction) in ANCHORS.iter().enumerate() {
        let y = ((h as f64 * fraction) as usize).min(h - sh);
        if block_std(&new, left, right, y, y + sh) < 4.0 {
            continue;
        }
        let mut template = Vec::with_capacity(sh * (right - left));
        for row in 0..sh {
            for x in left..right {
                template.push(new.value(x, y + row));
            }
        }
        let mut scores = correlate(&old, &template, left, right, 0);
        for _ in 0..4 {
            let mut best_location = 0usize;
            let mut best_score = f32::MIN;
            for (location, score) in scores.iter().enumerate() {
                if *score > best_score {
                    best_score = *score;
                    best_location = location;
                }
            }
            if best_score < 0.83 {
                break;
            }
            let shift = best_location as i32 - y as i32;
            if (shift.unsigned_abs() as usize) < h - sh.max(h / 8) {
                candidates.push(Candidate {
                    shift,
                    score: best_score,
                    anchor: anchor_index,
                });
            }
            let start = best_location.saturating_sub(4);
            let end = (best_location + 5).min(scores.len());
            for score in &mut scores[start..end] {
                *score = -1.0;
            }
        }
    }
    if candidates.is_empty() {
        return Ok(Match {
            shift: 0,
            confidence: 0.0,
            error: 255.0,
        });
    }

    let rows = same_rows(previous, current);
    let mut top = 0usize;
    for value in rows.iter() {
        if !value || top >= h / 5 {
            break;
        }
        top += 1;
    }
    let mut bottom = 0usize;
    for value in rows.iter().rev() {
        if !value || bottom >= h / 5 {
            break;
        }
        bottom += 1;
    }

    let detail_old = blur(&old);
    let detail_new = blur(&new);
    let mut shifts: Vec<i32> = candidates.iter().map(|c| c.shift).collect();
    shifts.sort_unstable();
    shifts.dedup();

    let mut evaluated: Vec<Match> = Vec::new();
    for shift in shifts {
        let group: Vec<&Candidate> = candidates
            .iter()
            .filter(|c| (c.shift - shift).abs() <= 1)
            .collect();
        let anchors: BTreeSet<usize> = group.iter().map(|c| c.anchor).collect();
        if anchors.len() < 2 {
            continue;
        }
        // The reference clamps each overlap bound toward the middle; keeping
        // the raw difference means a large negative shift can yield a range
        // that fails the minimum-length check and is skipped, instead of being
        // rounded up to a spurious window.
        let lo = (top as i32).max(top as i32 - shift);
        let hi = ((h - bottom) as i32).min(h as i32 - bottom as i32 - shift);
        if hi - lo < 48i32.max((h / 8) as i32) {
            continue;
        }
        let (lo, hi) = (lo as usize, hi as usize);
        let mut detail_count = 0usize;
        let mut error_sum = 0.0f64;
        for y in lo..hi {
            for x in left..right {
                let shifted = (y as i32 + shift) as usize;
                let a = old.value(x, shifted);
                let b = new.value(x, y);
                let da = (a - detail_old[shifted * tw + x]).abs();
                let db = (b - detail_new[y * tw + x]).abs();
                if da > 5.0 || db > 5.0 {
                    detail_count += 1;
                    error_sum += f64::from((a - b).abs());
                }
            }
        }
        if detail_count < 30 {
            continue;
        }
        let error = (error_sum / detail_count as f64) as f32;
        let confidence = anchors
            .iter()
            .map(|anchor| {
                group
                    .iter()
                    .filter(|c| c.anchor == *anchor)
                    .map(|c| c.score)
                    .fold(f32::MIN, f32::max)
            })
            .sum::<f32>()
            / anchors.len() as f32;
        evaluated.push(Match {
            shift,
            confidence,
            error,
        });
    }
    if evaluated.is_empty() {
        return Ok(Match {
            shift: 0,
            confidence: 0.0,
            error: 255.0,
        });
    }

    evaluated.sort_by(|a, b| {
        a.error
            .partial_cmp(&b.error)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    let best = evaluated[0];
    // Fully indistinguishable repeated patterns cannot establish a safe shift.
    if let Some(alternate) = evaluated.iter().find(|m| (m.shift - best.shift).abs() > 2)
        && (alternate.error - best.error).abs() < 0.08
        && alternate.confidence > 0.95
    {
        return Ok(Match {
            confidence: 0.0,
            ..best
        });
    }
    Ok(best)
}

/// The growing image plus the displacement bookkeeping for its rows.
// Ported from omnishot/stitch.py (ScrollStitcher)
#[derive(Debug)]
pub struct ScrollStitcher {
    horizontal: bool,
    max_pixels: usize,
    previous: Option<FrameData>,
    image: Option<FrameData>,
    position: i32,
    origin: i32,
    accepted: u32,
}

impl Default for ScrollStitcher {
    fn default() -> Self {
        Self::new(false)
    }
}

impl ScrollStitcher {
    #[must_use]
    pub fn new(horizontal: bool) -> Self {
        Self {
            horizontal,
            max_pixels: MAX_PIXELS,
            previous: None,
            image: None,
            position: 0,
            origin: 0,
            accepted: 0,
        }
    }

    /// Build a stitcher with a tighter pixel budget, for tests.
    #[must_use]
    pub fn with_limit(horizontal: bool, max_pixels: usize) -> Self {
        Self {
            max_pixels,
            ..Self::new(horizontal)
        }
    }

    #[must_use]
    pub fn horizontal(&self) -> bool {
        self.horizontal
    }

    #[must_use]
    pub fn position(&self) -> i32 {
        self.position
    }

    #[must_use]
    pub fn origin(&self) -> i32 {
        self.origin
    }

    #[must_use]
    pub fn accepted(&self) -> u32 {
        self.accepted
    }

    #[must_use]
    pub fn previous(&self) -> Option<FrameRef<'_>> {
        self.previous.as_ref().map(FrameData::view)
    }

    /// The stitched image in output orientation, or `None` before the first
    /// frame. Horizontal captures are transposed back here, once, outside the
    /// matching loop.
    #[must_use]
    pub fn output(&self) -> Option<FrameData> {
        let image = self.image.as_ref()?;
        if !self.horizontal {
            return Some(image.clone());
        }
        Some(transpose(image.view()))
    }

    /// A borrowed view of the stitched image in internal orientation.
    ///
    /// A vertical capture's internal orientation is already the output one, so
    /// the preview can render straight from this borrow with no copy.
    #[must_use]
    pub fn image_view(&self) -> Option<FrameRef<'_>> {
        self.image.as_ref().map(FrameData::view)
    }

    /// Place a frame into the internal output orientation.
    ///
    /// Horizontal captures are transposed once here, at the boundary, so the
    /// growing image and every later match work in output orientation. A
    /// vertical frame is taken by value, with no copy at all.
    fn orient(&self, frame: FrameData) -> FrameData {
        if !self.horizontal {
            return frame;
        }
        transpose(frame.view())
    }

    /// Append the rows a frame reveals.
    ///
    /// Takes the frame by value so a vertical capture never copies it. Matching
    /// reads borrowed slices of the two frames; only the growing image is
    /// replaced. Returns whether the image grew and the status message for the
    /// UI. A frame that cannot be aligned is rejected without changing state.
    // Ported from omnishot/stitch.py (ScrollStitcher.push)
    pub fn push(&mut self, frame: FrameData) -> Result<(bool, String), StitchError> {
        let frame = self.orient(frame);
        let Some(previous) = self.previous.as_ref() else {
            if frame.width * frame.height > self.max_pixels {
                return Err(StitchError::TooLarge);
            }
            self.previous = Some(frame.clone());
            self.image = Some(frame);
            self.accepted = 1;
            return Ok((
                true,
                "Ready — scroll slowly inside the selected area.".to_owned(),
            ));
        };

        let match_result = match_translation(previous.view(), frame.view())?;
        // Repeated labels can correlate well while disagreeing on the unique
        // digits in every row. Accept residual image changes only when the
        // independent template anchors agree very strongly on the movement.
        if match_result.confidence < 0.83
            || match_result.error > 16.0
            || (match_result.error > 4.0 && match_result.confidence < 0.97)
        {
            return Ok((
                false,
                "Could not align. Scroll back slightly or slow down.".to_owned(),
            ));
        }
        if match_result.shift.abs() <= 1 {
            return Ok((false, "No new content.".to_owned()));
        }

        let shift = match_result.shift;
        let previous_frame = previous.view();
        let current_image = self.image.take().expect("image exists once previous does");
        let new_position = self.position + shift;
        let bottom = new_position + frame.height as i32;
        let image_bottom = self.origin + current_image.height as i32;
        let extra_bottom = (bottom - image_bottom).max(0) as usize;
        let extra_top = (self.origin - new_position).max(0) as usize;
        let new_height = current_image.height + extra_bottom + extra_top;
        if new_height * frame.width > self.max_pixels {
            self.image = Some(current_image);
            return Err(StitchError::TooLarge);
        }

        let mut image = current_image;
        if extra_bottom > 0 || extra_top > 0 {
            let same = same_rows(previous_frame, frame.view());
            if extra_bottom > 0 {
                // Detect stationary footer rows and replace them before appending.
                let mut footer = 0usize;
                for value in same.iter().rev() {
                    if !value || footer >= frame.height / 5 {
                        break;
                    }
                    footer += 1;
                }
                let cut = footer
                    .min(image.height.saturating_sub(1))
                    .min(frame.height - extra_bottom);
                let mut data = Vec::with_capacity(
                    (image.height + extra_bottom) * frame.width * frame.channels,
                );
                data.extend_from_slice(image.rows(0..image.height - cut));
                data.extend_from_slice(frame.rows(frame.height - extra_bottom - cut..frame.height));
                let rows = data.len() / (frame.width * frame.channels);
                image = FrameData::new(data, frame.width, rows, frame.channels);
            }
            if extra_top > 0 {
                // Scrolling above the initial viewport must replace its
                // stationary header too; otherwise that header becomes a seam.
                let mut header = 0usize;
                for value in same.iter() {
                    if !value || header >= frame.height / 5 {
                        break;
                    }
                    header += 1;
                }
                let cut = header
                    .min(image.height.saturating_sub(1))
                    .min(frame.height - extra_top);
                let mut data =
                    Vec::with_capacity((image.height + extra_top) * frame.width * frame.channels);
                data.extend_from_slice(frame.rows(0..extra_top + cut));
                data.extend_from_slice(image.rows(cut..image.height));
                let rows = data.len() / (frame.width * frame.channels);
                image = FrameData::new(data, frame.width, rows, frame.channels);
                self.origin = new_position;
            }
        }

        self.image = Some(image);
        self.position = new_position;
        self.previous = Some(frame);
        self.accepted += 1;
        let image = self.image.as_ref().expect("image exists");
        // A horizontal capture keeps its accumulating image transposed, so the
        // internal dimensions are swapped against the image the user sees.
        let (display_width, display_height) = if self.horizontal {
            (image.height, image.width)
        } else {
            (image.width, image.height)
        };
        Ok((
            extra_top > 0 || extra_bottom > 0,
            format!(
                "{} × {} pixels · {} frames",
                display_width, display_height, self.accepted
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random source so synthetic pages are stable.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }

        fn below(&mut self, bound: u32) -> u32 {
            self.next() % bound
        }
    }

    /// A tall, unique-textured page like the reference fixture generator.
    fn page(height: usize, width: usize) -> Vec<u8> {
        let mut rng = Rng(12);
        let mut data = vec![245u8; height * width * 3];
        for y in (0..height).step_by(37) {
            for x in 20..width.saturating_sub(20) {
                if (x + y) % 7 < 3 {
                    let offset = (y * width + x) * 3;
                    data[offset] = 30;
                    data[offset + 1] = 40;
                    data[offset + 2] = 55;
                }
            }
            let block = if width > 80 { width - 80 } else { width / 2 };
            for x in block..(block + 50).min(width) {
                let offset = (y * width + x) * 3;
                data[offset] = rng.below(200) as u8;
                data[offset + 1] = rng.below(200) as u8;
                data[offset + 2] = rng.below(200) as u8;
            }
        }
        data
    }

    fn frame<'a>(data: &'a [u8], width: usize, height: usize) -> FrameRef<'a> {
        FrameRef::new(data, width, height, 3)
    }

    fn owned(data: &[u8], width: usize, height: usize) -> FrameData {
        FrameData::from_view(frame(data, width, height))
    }

    fn transpose_square(data: &[u8], side: usize) -> Vec<u8> {
        let mut out = vec![0u8; side * side * 3];
        for y in 0..side {
            for x in 0..side {
                let s = (y * side + x) * 3;
                let d = (x * side + y) * 3;
                out[d..d + 3].copy_from_slice(&data[s..s + 3]);
            }
        }
        out
    }

    fn with_sticky(frame_data: &mut [u8], width: usize, header: usize, footer: usize) {
        for row in 0..header {
            for x in 0..width {
                let offset = (row * width + x) * 3;
                frame_data[offset] = 80;
                frame_data[offset + 1] = 70;
                frame_data[offset + 2] = 110;
            }
        }
        let height = frame_data.len() / (width * 3);
        for row in (height - footer)..height {
            for x in 0..width {
                let offset = (row * width + x) * 3;
                frame_data[offset] = 130;
                frame_data[offset + 1] = 150;
                frame_data[offset + 2] = 170;
            }
        }
    }

    #[test]
    fn exact_vertical_forward_and_reverse() {
        let source = page(3200, 600);
        let mut stitch = ScrollStitcher::new(false);
        for y in [0usize, 180, 360, 180, 360, 540, 720, 900, 1000] {
            let start = y * 600 * 3;
            stitch
                .push(owned(&source[start..start + 600 * 600 * 3], 600, 600))
                .unwrap();
        }
        let output = stitch.output().unwrap();
        assert_eq!(output.width, 600);
        assert_eq!(output.height, 1600);
        assert!(output.as_bytes() == &source[..1600 * 600 * 3]);
    }

    #[test]
    fn horizontal_capture_transposes_once() {
        // Transpose the tall synthetic page into a wide one, exactly as the
        // reference fixtures do: each column now carries the unique texture.
        let tall = page(3200, 600);
        let (wide_height, wide_width) = (600usize, 3200usize);
        let mut wide = vec![0u8; wide_width * wide_height * 3];
        for y in 0..wide_height {
            for x in 0..wide_width {
                let s = (x * 600 + y) * 3;
                let t = (y * wide_width + x) * 3;
                wide[t..t + 3].copy_from_slice(&tall[s..s + 3]);
            }
        }
        let mut stitch = ScrollStitcher::new(true);
        let mut message = String::new();
        for x in [0usize, 180, 360, 540] {
            let mut frame_data = vec![0u8; 600 * 600 * 3];
            for row in 0..600 {
                let source = (row * wide_width + x) * 3;
                let target = row * 600 * 3;
                frame_data[target..target + 600 * 3]
                    .copy_from_slice(&wide[source..source + 600 * 3]);
            }
            (_, message) = stitch.push(owned(&frame_data, 600, 600)).unwrap();
        }
        let output = stitch.output().unwrap();
        assert_eq!(output.width, 1140);
        assert_eq!(output.height, 600);
        // The status reports the image the user sees, not the internal
        // transposed buffer.
        assert!(message.starts_with("1140 × 600"), "{message}");
        // The stitched image is the first 1140 columns of the wide page.
        for row in 0..600 {
            let expected = (row * wide_width) * 3;
            let actual = row * 1140 * 3;
            assert_eq!(
                &output.as_bytes()[actual..actual + 1140 * 3],
                &wide[expected..expected + 1140 * 3],
                "row {row}"
            );
        }
    }

    #[test]
    fn repeated_identical_frames_do_not_grow() {
        let source = page(600, 600);
        let mut stitch = ScrollStitcher::new(false);
        stitch.push(owned(&source, 600, 600)).unwrap();
        for _ in 0..10 {
            let (changed, _) = stitch.push(owned(&source, 600, 600)).unwrap();
            assert!(!changed);
        }
        assert!(stitch.output().unwrap().as_bytes() == source.as_slice());
    }

    #[test]
    fn unrelated_frame_is_rejected_and_keeps_state() {
        let source = page(600, 600);
        let mut stitch = ScrollStitcher::new(false);
        stitch.push(owned(&source, 600, 600)).unwrap();
        let mut rng = Rng(44);
        let mut noise = vec![0u8; 600 * 600 * 3];
        for byte in &mut noise {
            *byte = rng.below(255) as u8;
        }
        let (changed, message) = stitch.push(owned(&noise, 600, 600)).unwrap();
        assert!(!changed, "{message}");
        assert_eq!(stitch.position(), 0);
        assert_eq!(stitch.accepted(), 1);
        assert!(stitch.output().unwrap().as_bytes() == source.as_slice());
    }

    #[test]
    fn sticky_header_and_footer_are_replaced() {
        let source = page(1200, 600);
        let mut stitch = ScrollStitcher::new(false);
        for y in [0usize, 170, 340, 510] {
            let start = y * 600 * 3;
            let mut frame_data = source[start..start + 600 * 600 * 3].to_vec();
            with_sticky(&mut frame_data, 600, 45, 30);
            stitch.push(owned(&frame_data, 600, 600)).unwrap();
        }
        let output = stitch.output().unwrap();
        assert_eq!(output.height, 1110);
        // The first 45 and last 30 rows are the one fixed header/footer; the
        // interior carries the real page content.
        for row in [0usize, 44, 1080, 1109] {
            let offset = row * 600 * 3;
            assert!(output.as_bytes()[offset] < 200, "row {row}");
        }
        let interior = &output.as_bytes()[45 * 600 * 3..1080 * 600 * 3];
        assert!(interior.iter().any(|&b| b < 100));
    }

    #[test]
    fn reverse_past_start_keeps_one_fixed_leading_edge() {
        let source = page(1200, 600);
        for horizontal in [false, true] {
            let mut stitch = ScrollStitcher::new(horizontal);
            for position in [510usize, 340, 510, 340, 340, 170, 0] {
                let start = position * 600 * 3;
                let mut frame_data = source[start..start + 600 * 600 * 3].to_vec();
                with_sticky(&mut frame_data, 600, 45, 30);
                if horizontal {
                    let transposed = transpose_square(&frame_data, 600);
                    stitch.push(owned(&transposed, 600, 600)).unwrap();
                } else {
                    stitch.push(owned(&frame_data, 600, 600)).unwrap();
                }
            }
            // Expected: the first 1110 rows of the page with one fixed header
            // and footer, transposed for a horizontal capture.
            let mut expected = source[..1110 * 600 * 3].to_vec();
            with_sticky(&mut expected, 600, 45, 30);
            let expected = if horizontal {
                let (rows, cols) = (1110usize, 600usize);
                let mut out = vec![0u8; rows * cols * 3];
                for r in 0..rows {
                    for c in 0..cols {
                        let s = (r * cols + c) * 3;
                        let d = (c * rows + r) * 3;
                        out[d..d + 3].copy_from_slice(&expected[s..s + 3]);
                    }
                }
                out
            } else {
                expected
            };
            let output = stitch.output().unwrap();
            assert_eq!(output.data, expected, "horizontal={horizontal}");
        }
    }

    #[test]
    fn limits_reject_oversize_and_resize() {
        let source = page(1000, 600);
        let mut stitch = ScrollStitcher::with_limit(false, 600 * 700);
        stitch.push(owned(&source, 600, 600)).unwrap();
        let grown = owned(&source[200 * 600 * 3..], 600, 600);
        assert_eq!(stitch.push(grown), Err(StitchError::TooLarge));
        let resized = owned(&source[..500 * 600 * 3], 600, 500);
        assert_eq!(stitch.push(resized), Err(StitchError::DimensionsChanged));
    }

    #[test]
    fn match_reports_error_on_dimension_mismatch_and_tiny_frames() {
        let source = page(100, 100);
        let big = frame(&source, 100, 100);
        assert_eq!(
            match_translation(big, frame(&source[..50 * 100 * 3], 100, 50)),
            Err(StitchError::DimensionsChanged)
        );
        let tiny = page(16, 16);
        assert_eq!(
            match_translation(frame(&tiny, 16, 16), frame(&tiny, 16, 16)),
            Err(StitchError::TooSmall)
        );
    }

    #[test]
    fn brightness_and_noise_changes_with_valid_motion_are_accepted() {
        let source = page(800, 600);
        for change in ["brightness", "noise"] {
            let before = &source[..600 * 600 * 3];
            let after_start = 170 * 600 * 3;
            let after: Vec<u8> = source[after_start..after_start + 600 * 600 * 3]
                .iter()
                .enumerate()
                .map(|(index, &b)| {
                    if change == "brightness" {
                        b.saturating_add(12)
                    } else {
                        let jitter = ((index * 2654435761usize) % 11) as i32 - 5;
                        (i32::from(b) + jitter).clamp(0, 255) as u8
                    }
                })
                .collect();
            let mut stitch = ScrollStitcher::new(false);
            stitch.push(owned(before, 600, 600)).unwrap();
            let (changed, message) = stitch.push(owned(&after, 600, 600)).unwrap();
            assert!(changed, "{change}: {message}");
            assert_eq!(stitch.position(), 170, "{change}");
        }
    }
}
