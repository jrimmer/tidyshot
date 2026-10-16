//! A fixed-workload timing harness for the stitcher and the annotation effects.
//!
//! Run with `cargo run --release --bin bench`, optionally passing the number of
//! repetitions as the first argument. The workloads are deterministic, so the
//! reported numbers are comparable across revisions. This is not a gate: the
//! unit tests assert behaviour, this binary only reports wall-clock time.
//!
//! The Python baseline is not measured: `omnishot/stitch.py` imports `cv2` and
//! `numpy` for `matchTemplate` and its array maths, and neither is installed on
//! the lease, so the harness reports absolute Rust numbers instead.

use std::process::Command;
use std::time::{Duration, Instant};

use tidyshot::effects::{
    BlurMode, EffectKind, EffectMode, PixelateMode, Region, RgbaBuffer, apply,
};
use tidyshot::stitch::{FrameData, ScrollStitcher, match_translation};

/// Deterministic pseudo-random source, shared by every generated fixture.
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

/// A tall, unique-textured page with the same shape the stitcher tests use.
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

/// Decode a PNG into contiguous RGB24 rows through ffmpeg, the app's media path.
fn decode(path: &str) -> Option<(usize, usize, Vec<u8>)> {
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
            path,
        ])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&probe.stdout);
    let mut parts = text.trim().split(',');
    let width: usize = parts.next()?.parse().ok()?;
    let height: usize = parts.next()?.parse().ok()?;
    let output = Command::new("ffmpeg")
        .args([
            "-v", "error", "-i", path, "-f", "rawvideo", "-pix_fmt", "rgb24", "-",
        ])
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.len() != width * height * 3 {
        return None;
    }
    Some((width, height, output.stdout))
}

/// The best and mean of `reps` timed runs, in milliseconds.
fn timed(reps: usize, mut run: impl FnMut()) -> (f64, f64) {
    let mut best = Duration::MAX;
    let mut total = Duration::ZERO;
    for _ in 0..reps {
        let start = Instant::now();
        run();
        let elapsed = start.elapsed();
        best = best.min(elapsed);
        total += elapsed;
    }
    (
        best.as_secs_f64() * 1e3,
        total.as_secs_f64() * 1e3 / reps as f64,
    )
}

fn report(name: &str, reps: usize, best: f64, mean: f64) {
    println!("{name:<38} reps={reps:<3} best={best:>8.2} ms  mean={mean:>8.2} ms");
}

/// Time a full vertical stitch over a synthetic page: 10 frames, 600×600 each,
/// moving forward 170 rows per accepted frame.
fn bench_vertical_stitch(reps: usize) {
    let width = 600usize;
    let stride = 170usize;
    let page = page(stride * 9 + 600, width);
    let frames: Vec<Vec<u8>> = (0..10)
        .map(|i| page[i * stride * width * 3..(i * stride + 600) * width * 3].to_vec())
        .collect();
    let (best, mean) = timed(reps, || {
        let mut stitch = ScrollStitcher::new(false);
        for bytes in &frames {
            stitch
                .push(FrameData::new(bytes.clone(), width, 600, 3))
                .expect("frame aligns");
        }
        std::hint::black_box(stitch.output());
    });
    report("stitch/vertical-10x600x600", reps, best, mean);
}

/// Time a single `match_translation` over the two real browser PNG fixtures, so
/// the anchor search runs against realistic repeated rows and a large overlap.
fn bench_real_match(reps: usize) {
    let Some((width, height, before)) = decode("tests/fixtures/browser-overlap-before.png") else {
        println!("stitch/match-browser-overlap            skipped: ffmpeg/ffprobe unavailable");
        return;
    };
    let Some((width_after, height_after, after)) =
        decode("tests/fixtures/browser-overlap-after.png")
    else {
        println!("stitch/match-browser-overlap            skipped: ffmpeg/ffprobe unavailable");
        return;
    };
    if (width, height) != (width_after, height_after) {
        println!("stitch/match-browser-overlap            skipped: fixture dimensions differ");
        return;
    }
    let before = FrameData::new(before, width, height, 3);
    let after = FrameData::new(after, width, height, 3);
    let (best, mean) = timed(reps, || {
        let matched = match_translation(before.view(), after.view()).expect("match");
        std::hint::black_box(matched);
    });
    report("stitch/match-browser-overlap", reps, best, mean);
}

/// Build a deterministic RGBA fixture with enough detail for the effects.
fn rgba_fixture(width: usize, height: usize) -> Vec<u8> {
    let mut rng = Rng(7);
    let mut data = vec![0u8; width * height * 4];
    for pixel in data.chunks_mut(4) {
        pixel[0] = rng.below(256) as u8;
        pixel[1] = rng.below(256) as u8;
        pixel[2] = rng.below(256) as u8;
        pixel[3] = 255;
    }
    data
}

/// Time each effect over a fixed region of a 1024×768 RGBA frame.
fn bench_effects(reps: usize, width: usize, height: usize) {
    let fixture = rgba_fixture(width, height);
    let region = Region::new(64, 32, 896, 704);
    let cases: [(&str, EffectKind, EffectMode); 3] = [
        (
            "effects/smooth-blur-896x704-r18",
            EffectKind::Blur,
            EffectMode::Blur(BlurMode::Smooth),
        ),
        (
            "effects/secure-blur-896x704-r18",
            EffectKind::Blur,
            EffectMode::Blur(BlurMode::Secure),
        ),
        (
            "effects/pixelate-896x704-f12",
            EffectKind::Pixelate,
            EffectMode::Pixelate(PixelateMode::Classic),
        ),
    ];
    for (name, kind, mode) in cases {
        let (best, mean) = timed(reps, || {
            let mut data = fixture.clone();
            let mut buffer = RgbaBuffer::new(&mut data, width, height, width * 4);
            apply(&mut buffer, region, kind, 18, mode, 12345);
            std::hint::black_box(data);
        });
        report(name, reps, best, mean);
    }
}

fn main() {
    let reps: usize = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(20);
    println!("tidyshot bench, {reps} repetitions per workload");
    bench_vertical_stitch(reps);
    bench_real_match(reps);
    bench_effects(reps, 1024, 768);
}
