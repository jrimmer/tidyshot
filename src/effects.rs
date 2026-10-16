//! Annotation pixel effects over borrowed RGBA8 buffers.
// Ported from omnishot/annotation_effects.py
//!
//! The annotator passes a mutable view of one region into these functions;
//! nothing is copied in or out beyond the pixels the effect itself replaces.
//! Secure blur never samples the concealed interior, so the covered content
//! cannot leak through the replacement.

/// How the blur replacement is derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlurMode {
    /// An ordinary Gaussian-like blur of the region's own pixels.
    Smooth,
    /// An opaque texture built only from the boundary and a seed.
    Secure,
}

/// How pixelation arranges its blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelateMode {
    /// One averaged color per block.
    Classic,
    /// A seeded shuffle and color perturbation of the blocks.
    Randomized,
}

/// A window into an RGBA8 buffer with an explicit row stride.
#[derive(Debug)]
pub struct RgbaBuffer<'a> {
    data: &'a mut [u8],
    /// Total image width in pixels, not the region width.
    width: usize,
    /// Total image height in pixels, not the region height.
    height: usize,
    /// Bytes between the start of one row and the next.
    stride: usize,
}

/// The alpha-weighted average of the ring of pixels just outside `region`.
/// Each boundary pixel is sampled once; the four corners belong to the top
/// and bottom rows alone.
fn boundary_average(buffer: &RgbaBuffer<'_>, region: Region) -> [f64; 3] {
    let (mut sum_r, mut sum_g, mut sum_b, mut weight) = (0.0f64, 0.0, 0.0, 0.0);
    let mut accumulate = |x: usize, y: usize| {
        // Only pixels strictly outside the region may feed the replacement,
        // so the concealed content cannot leak into its own texture.
        if x >= region.x
            && x < region.x + region.width
            && y >= region.y
            && y < region.y + region.height
        {
            return;
        }
        let Some(pixel) = buffer.pixel(x, y) else {
            return;
        };
        let alpha = f64::from(pixel[3]) / 255.0;
        sum_r += f64::from(pixel[0]) * alpha;
        sum_g += f64::from(pixel[1]) * alpha;
        sum_b += f64::from(pixel[2]) * alpha;
        weight += alpha;
    };
    let left = region.x.saturating_sub(1);
    let right = (region.x + region.width).min(buffer.width.saturating_sub(1));
    let top = region.y.saturating_sub(1);
    let bottom = (region.y + region.height).min(buffer.height.saturating_sub(1));
    for x in left..=right {
        accumulate(x, top);
        accumulate(x, bottom);
    }
    // The side columns exclude the corner rows already counted above.
    for y in (top + 1)..bottom {
        accumulate(left, y);
        accumulate(right, y);
    }
    if weight > 0.0 {
        [sum_r / weight, sum_g / weight, sum_b / weight]
    } else {
        [216.0, 220.0, 228.0]
    }
}

impl<'a> RgbaBuffer<'a> {
    /// Borrow an RGBA8 buffer. `stride` is bytes per row and must be at least
    /// `width * 4`; `height` is the number of addressable rows.
    #[must_use]
    pub fn new(data: &'a mut [u8], width: usize, height: usize, stride: usize) -> Self {
        debug_assert!(
            stride >= width.saturating_mul(4),
            "stride must cover a full row"
        );
        Self {
            data,
            width,
            height,
            stride,
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

    fn offset(&self, x: usize, y: usize) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = y * self.stride + x * 4;
        if offset + 4 <= self.data.len() {
            Some(offset)
        } else {
            None
        }
    }

    #[must_use]
    pub fn pixel(&self, x: usize, y: usize) -> Option<[u8; 4]> {
        let offset = self.offset(x, y)?;
        Some([
            self.data[offset],
            self.data[offset + 1],
            self.data[offset + 2],
            self.data[offset + 3],
        ])
    }

    fn set_pixel(&mut self, x: usize, y: usize, pixel: [u8; 4]) {
        if let Some(offset) = self.offset(x, y) {
            self.data[offset..offset + 4].copy_from_slice(&pixel);
        }
    }
}

/// A half-open region, clipped to the buffer where needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl Region {
    #[must_use]
    pub const fn new(x: usize, y: usize, width: usize, height: usize) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// Apply the requested effect to `region` of `buffer`.
///
/// `strength` is the effect radius or block factor. Blur runs at a radius of
/// at least two; pixelate lifts its block factor to at least five.
// Ported from omnishot/annotation_effects.py (render_effect)
pub fn apply(
    buffer: &mut RgbaBuffer<'_>,
    region: Region,
    kind: EffectKind,
    strength: u32,
    mode: EffectMode,
    seed: u64,
) {
    let strength = strength.max(2);
    match kind {
        EffectKind::Blur => match mode {
            EffectMode::Blur(BlurMode::Smooth) => smooth_blur(buffer, region, strength),
            EffectMode::Blur(BlurMode::Secure) => secure_blur(buffer, region, strength, seed),
            EffectMode::Pixelate(_) => {}
        },
        EffectKind::Pixelate => {
            let mode = match mode {
                EffectMode::Pixelate(mode) => mode,
                EffectMode::Blur(_) => PixelateMode::Classic,
            };
            pixelate(buffer, region, strength, mode, seed);
        }
    }
}

/// The effect family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectKind {
    Blur,
    Pixelate,
}

/// The per-family configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectMode {
    Blur(BlurMode),
    Pixelate(PixelateMode),
}

/// A rectangular box blur, applied separably over a copy of the clipped region.
// Ported from omnishot/annotation_effects.py (Smooth)
pub fn smooth_blur(buffer: &mut RgbaBuffer<'_>, region: Region, radius: u32) {
    let Some(region) = clip(buffer, region) else {
        return;
    };
    let radius = radius.max(1) as usize;
    let width = region.width;
    let height = region.height;
    let mut source = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let pixel = buffer
                .pixel(region.x + x, region.y + y)
                .unwrap_or([0, 0, 0, 0]);
            source.extend_from_slice(&pixel);
        }
    }
    let mut horizontal = vec![0u8; source.len()];
    box_blur_rows(&source, &mut horizontal, width, height, radius);
    let mut vertical = vec![0u8; source.len()];
    box_blur_columns(&horizontal, &mut vertical, width, height, radius);
    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * 4;
            let pixel = [
                vertical[offset],
                vertical[offset + 1],
                vertical[offset + 2],
                vertical[offset + 3],
            ];
            buffer.set_pixel(region.x + x, region.y + y, pixel);
        }
    }
}

/// Replace the region with an opaque texture derived only from its boundary.
///
/// The visible result is identical for any two images that share a boundary,
/// so a concealed password or face cannot influence its own replacement.
// Ported from omnishot/annotation_effects.py (secure_texture)
pub fn secure_blur(buffer: &mut RgbaBuffer<'_>, region: Region, strength: u32, seed: u64) {
    let Some(region) = clip(buffer, region) else {
        return;
    };
    let strength = strength.max(2) as f64;
    let base = boundary_average(buffer, region);

    // One lattice cell per (spacing) pixels, smoothly interpolated, so the
    // replacement looks like a texture rather than per-pixel noise.
    let spacing = (strength as usize).max(8);
    let cells_x = region.width.div_ceil(spacing) + 1;
    let cells_y = region.height.div_ceil(spacing) + 1;
    let mut lattice = Vec::with_capacity(cells_x * cells_y);
    let mut rng = SplitMix64::new(seed);
    for _ in 0..cells_x * cells_y {
        lattice.push(rng.next_f64() * 72.0 - 36.0);
    }
    for y in 0..region.height {
        let gy = y as f64 / spacing as f64;
        let y0 = gy.floor() as usize;
        let fy = gy - y0 as f64;
        for x in 0..region.width {
            let gx = x as f64 / spacing as f64;
            let x0 = gx.floor() as usize;
            let fx = gx - x0 as f64;
            let value = bilerp(&lattice, cells_x, x0, y0, fx, fy);
            let pixel = [
                clamp_channel(base[0] + value),
                clamp_channel(base[1] + value),
                clamp_channel(base[2] + value),
                255,
            ];
            buffer.set_pixel(region.x + x, region.y + y, pixel);
        }
    }
}

/// Quantize the region into blocks, optionally shuffling and tinting them.
// Ported from omnishot/annotation_effects.py (pixelate)
pub fn pixelate(
    buffer: &mut RgbaBuffer<'_>,
    region: Region,
    factor: u32,
    mode: PixelateMode,
    seed: u64,
) {
    let Some(region) = clip(buffer, region) else {
        return;
    };
    let factor = factor.max(5) as usize;
    let blocks_x = region.width.div_ceil(factor);
    let blocks_y = region.height.div_ceil(factor);
    let mut blocks: Vec<[f32; 4]> = Vec::with_capacity(blocks_x * blocks_y);
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let (mut r, mut g, mut b, mut a, mut count) = (0.0f32, 0.0, 0.0, 0.0, 0.0);
            for y in by * factor..((by + 1) * factor).min(region.height) {
                for x in bx * factor..((bx + 1) * factor).min(region.width) {
                    let pixel = buffer
                        .pixel(region.x + x, region.y + y)
                        .unwrap_or([0, 0, 0, 0]);
                    r += f32::from(pixel[0]);
                    g += f32::from(pixel[1]);
                    b += f32::from(pixel[2]);
                    a += f32::from(pixel[3]);
                    count += 1.0;
                }
            }
            let scale = if count > 0.0 { 1.0 / count } else { 0.0 };
            blocks.push([r * scale, g * scale, b * scale, a * scale]);
        }
    }

    let mut order: Vec<usize> = (0..blocks.len()).collect();
    if mode == PixelateMode::Randomized {
        let mut rng = SplitMix64::new(seed);
        for index in (1..order.len()).rev() {
            order.swap(index, rng.next_bounded(index + 1));
        }
        for block in &mut blocks {
            for channel in block.iter_mut().take(3) {
                let jitter = (rng.next_f64() * 24.0 - 12.0) as f32;
                *channel = (*channel + jitter).clamp(0.0, 255.0);
            }
        }
    }

    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let index = by * blocks_x + bx;
            let block = blocks[order[index]];
            let pixel = [
                block[0].round() as u8,
                block[1].round() as u8,
                block[2].round() as u8,
                block[3].round() as u8,
            ];
            for y in by * factor..((by + 1) * factor).min(region.height) {
                for x in bx * factor..((bx + 1) * factor).min(region.width) {
                    buffer.set_pixel(region.x + x, region.y + y, pixel);
                }
            }
        }
    }
}

fn clip(buffer: &RgbaBuffer<'_>, region: Region) -> Option<Region> {
    let x = region.x.min(buffer.width);
    let y = region.y.min(buffer.height);
    let width = region.width.min(buffer.width - x);
    let height = region.height.min(buffer.height - y);
    if width == 0 || height == 0 {
        return None;
    }
    Some(Region::new(x, y, width, height))
}

fn box_blur_rows(source: &[u8], out: &mut [u8], width: usize, height: usize, radius: usize) {
    // A prefix sum per row turns each window mean into two reads instead of a
    // rescan, which is what the radius-sized blur would otherwise pay per pixel.
    let mut prefix = vec![0u32; width + 1];
    for y in 0..height {
        for channel in 0..4 {
            prefix[0] = 0;
            for x in 0..width {
                prefix[x + 1] = prefix[x] + u32::from(source[(y * width + x) * 4 + channel]);
            }
            for x in 0..width {
                let start = x.saturating_sub(radius);
                let end = (x + radius).min(width - 1);
                let sum = prefix[end + 1] - prefix[start];
                out[(y * width + x) * 4 + channel] = (sum / (end - start + 1) as u32) as u8;
            }
        }
    }
}

fn box_blur_columns(source: &[u8], out: &mut [u8], width: usize, height: usize, radius: usize) {
    let mut prefix = vec![0u32; height + 1];
    for x in 0..width {
        for channel in 0..4 {
            prefix[0] = 0;
            for y in 0..height {
                prefix[y + 1] = prefix[y] + u32::from(source[(y * width + x) * 4 + channel]);
            }
            for y in 0..height {
                let start = y.saturating_sub(radius);
                let end = (y + radius).min(height - 1);
                let sum = prefix[end + 1] - prefix[start];
                out[(y * width + x) * 4 + channel] = (sum / (end - start + 1) as u32) as u8;
            }
        }
    }
}

fn bilerp(lattice: &[f64], cells_x: usize, x0: usize, y0: usize, fx: f64, fy: f64) -> f64 {
    let at = |x: usize, y: usize| lattice.get(y * cells_x + x).copied().unwrap_or(0.0);
    let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
    let bottom = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
    top * (1.0 - fy) + bottom * fy
}

fn clamp_channel(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

/// A small deterministic RNG, so effects are reproducible across exports.
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_add(0x9e37_79b9_7f4a_7c15),
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn next_bounded(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() % bound as u64) as usize
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: usize, height: usize) -> Vec<u8> {
        vec![0u8; width * height * 4]
    }

    fn pixel(buffer: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
        let offset = (y * width + x) * 4;
        [
            buffer[offset],
            buffer[offset + 1],
            buffer[offset + 2],
            buffer[offset + 3],
        ]
    }

    #[test]
    fn pixelate_uses_the_documented_minimum_block_size() {
        // Upstream pixelate clamps the block factor to at least five.
        let mut data = image(20, 1);
        for x in 0..20 {
            let offset = x * 4;
            data[offset] = (x * 12) as u8;
        }
        let mut buffer = RgbaBuffer::new(&mut data, 20, 1, 20 * 4);
        pixelate(
            &mut buffer,
            Region::new(0, 0, 20, 1),
            2,
            PixelateMode::Classic,
            0,
        );
        // With a factor of five the first block spans x = 0..4.
        assert_eq!(buffer.pixel(0, 0).unwrap(), buffer.pixel(4, 0).unwrap());
        assert_ne!(buffer.pixel(0, 0).unwrap(), buffer.pixel(5, 0).unwrap());
    }

    #[test]
    fn boundary_average_weights_each_ring_pixel_once() {
        // A single bright corner pixel must contribute exactly one sample, not
        // once per row and once per column.
        let mut data = image(5, 5);
        for offset in (0..5 * 5 * 4).step_by(4) {
            data[offset..offset + 4].copy_from_slice(&[0, 0, 0, 255]);
        }
        // Region is the center 3x3 (1,1)..(3,3); its ring is the border.
        // Put 100 at corner (0,0) and 200 at top edge (1,0).
        data[0..4].copy_from_slice(&[100, 100, 100, 255]);
        let offset = 4;
        data[offset..offset + 4].copy_from_slice(&[200, 200, 200, 255]);
        let buffer = RgbaBuffer::new(&mut data, 5, 5, 5 * 4);
        let base = boundary_average(&buffer, Region::new(1, 1, 3, 3));
        // 16 ring pixels: 100 once + 200 once + 14 zeros.
        let expected = (100.0 + 200.0) / 16.0;
        assert!((base[0] - expected).abs() < 1e-9, "{base:?}");
    }

    #[test]
    fn secure_blur_matches_for_different_interiors() {
        let mut first = image(40, 30);
        let mut second = image(40, 30);
        for data in [&mut first, &mut second] {
            for offset in (0..40 * 30 * 4).step_by(4) {
                data[offset..offset + 4].copy_from_slice(&[0xe8, 0xed, 0xf4, 255]);
            }
        }
        for y in 5..25 {
            for x in 5..35 {
                let red = (y * 40 + x) * 4;
                first[red..red + 4].copy_from_slice(&[255, 0, 0, 255]);
                second[red..red + 4].copy_from_slice(&[0, 0, 255, 255]);
            }
        }
        let region = Region::new(5, 5, 30, 20);
        {
            let mut buffer = RgbaBuffer::new(&mut first, 40, 30, 40 * 4);
            secure_blur(&mut buffer, region, 18, 123);
        }
        {
            let mut buffer = RgbaBuffer::new(&mut second, 40, 30, 40 * 4);
            secure_blur(&mut buffer, region, 18, 123);
        }
        assert_eq!(first, second);
    }

    #[test]
    fn secure_blur_texture_changes_with_the_seed() {
        let make = |seed| {
            let mut data = image(20, 20);
            for offset in (0..20 * 20 * 4).step_by(4) {
                data[offset..offset + 4].copy_from_slice(&[100, 140, 180, 255]);
            }
            let mut buffer = RgbaBuffer::new(&mut data, 20, 20, 20 * 4);
            secure_blur(&mut buffer, Region::new(0, 0, 20, 20), 10, seed);
            data
        };
        assert_ne!(make(1), make(2));
        assert_eq!(make(7), make(7));
    }

    #[test]
    fn randomized_pixelation_is_stable_and_seed_sensitive() {
        let mut source = image(64, 48);
        for y in 0..48 {
            for x in 0..64 {
                let offset = (y * 64 + x) * 4;
                source[offset..offset + 4].copy_from_slice(&[
                    ((x * 3) % 256) as u8,
                    ((y * 5) % 256) as u8,
                    90,
                    255,
                ]);
            }
        }
        let render = |mode, seed| {
            let mut data = source.clone();
            let mut buffer = RgbaBuffer::new(&mut data, 64, 48, 64 * 4);
            pixelate(&mut buffer, Region::new(0, 0, 64, 48), 8, mode, seed);
            data
        };
        let first = render(PixelateMode::Randomized, 123);
        assert_eq!(first, render(PixelateMode::Randomized, 123));
        assert_ne!(first, render(PixelateMode::Randomized, 124));
    }

    #[test]
    fn classic_pixelation_makes_uniform_blocks_and_keeps_alpha() {
        let mut data = image(9, 9);
        for y in 0..9 {
            for x in 0..9 {
                let offset = (y * 9 + x) * 4;
                data[offset..offset + 4].copy_from_slice(&[10, 20, 30, 200]);
            }
        }
        let mut buffer = RgbaBuffer::new(&mut data, 9, 9, 9 * 4);
        pixelate(
            &mut buffer,
            Region::new(0, 0, 9, 9),
            3,
            PixelateMode::Classic,
            0,
        );
        // Every pixel in a 3x3 block shares one value including its alpha.
        let corner = pixel(&data, 9, 0, 0);
        assert_eq!(corner, [10, 20, 30, 200]);
        assert_eq!(pixel(&data, 9, 2, 2), corner);
        assert_eq!(pixel(&data, 9, 3, 3), corner);
    }

    #[test]
    fn smooth_blur_spreads_a_central_peak() {
        let mut data = image(21, 21);
        for offset in (0..21 * 21 * 4).step_by(4) {
            data[offset..offset + 4].copy_from_slice(&[0, 0, 0, 255]);
        }
        let offset = (10 * 21 + 10) * 4;
        data[offset..offset + 4].copy_from_slice(&[255, 255, 255, 255]);
        let mut buffer = RgbaBuffer::new(&mut data, 21, 21, 21 * 4);
        smooth_blur(&mut buffer, Region::new(0, 0, 21, 21), 3);
        let center = buffer.pixel(10, 10).unwrap()[0];
        let adjacent = buffer.pixel(9, 10).unwrap()[0];
        let far = buffer.pixel(10, 4).unwrap()[0];
        assert!(center > 0, "blur should keep the peak's brightness");
        assert!(adjacent > 0, "the peak should spread to nearby pixels");
        assert_eq!(far, 0, "pixels outside the radius stay untouched");
    }

    #[test]
    fn region_is_clipped_to_the_buffer() {
        let mut data = image(4, 4);
        let mut buffer = RgbaBuffer::new(&mut data, 4, 4, 4 * 4);
        // A region larger than the buffer must not panic.
        secure_blur(&mut buffer, Region::new(2, 2, 100, 100), 8, 1);
        pixelate(
            &mut buffer,
            Region::new(0, 9, 4, 4),
            4,
            PixelateMode::Classic,
            1,
        );
    }
}
