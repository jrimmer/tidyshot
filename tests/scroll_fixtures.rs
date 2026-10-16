//! Real screenshot fixtures exercise repeated-row rejection and recovery.
//!
//! The reference fixtures are decoded through ffmpeg, the same media path the
//! app uses, so the tests assert on the actual browser content that produced
//! the source's regressions.

use std::process::Command;

use tidyshot::stitch::{FrameData, FrameRef, ScrollStitcher, match_translation};

/// Decode a PNG fixture into contiguous RGB24 rows.
fn decode(path: &str) -> (usize, usize, Vec<u8>) {
    let output = Command::new("ffmpeg")
        .args([
            "-v", "error", "-i", path, "-f", "rawvideo", "-pix_fmt", "rgb24", "-",
        ])
        .output()
        .expect("ffmpeg runs");
    assert!(
        output.status.success(),
        "decode {path}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let width = 0;
    let _ = width;
    // Dimensions come from ffprobe so the test does not hardcode them.
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
        .expect("ffprobe runs");
    let text = String::from_utf8_lossy(&probe.stdout);
    let mut parts = text.trim().split(',');
    let width: usize = parts.next().unwrap().parse().unwrap();
    let height: usize = parts.next().unwrap().parse().unwrap();
    assert_eq!(output.stdout.len(), width * height * 3);
    (width, height, output.stdout)
}

fn fixture(name: &str) -> (usize, usize, Vec<u8>) {
    decode(&format!("tests/fixtures/{name}"))
}

#[test]
fn repeated_browser_rows_choose_the_real_overlap() {
    let (width, height, before) = fixture("browser-overlap-before.png");
    let (width_after, height_after, after) = fixture("browser-overlap-after.png");
    assert_eq!((width, height), (width_after, height_after));
    let before = FrameData::new(before, width, height, 3);
    let after = FrameData::new(after, width, height, 3);
    // At 1.45 browser scale, 500 CSS pixels are 725 image pixels. A majority
    // vote previously selected -29 and silently omitted four complete rows.
    let matched = match_translation(before.view(), after.view()).unwrap();
    assert_eq!(matched.shift, 725, "match {matched:?}");
    let mut stitch = ScrollStitcher::new(false);
    stitch.push(before).unwrap();
    let (changed, message) = stitch.push(after).unwrap();
    assert!(changed, "{message}");
    let output = stitch.output().unwrap();
    assert_eq!((output.height, output.width), (2323, 1000));
}

#[test]
fn large_jump_with_repeated_rows_is_rejected_without_changing_state() {
    let (width, height, before) = fixture("scroll-jump-before.png");
    let (width_after, height_after, after) = fixture("scroll-jump-after.png");
    assert_eq!((width, height), (width_after, height_after));
    let before = FrameData::new(before, width, height, 3);
    let after_data = FrameData::new(after, width, height, 3);
    // The native page moved up 480 logical pixels (768 captured pixels).
    // Repeated row labels previously matched a +192-pixel downward movement.
    let mut stitch = ScrollStitcher::new(false);
    stitch.push(before.clone()).unwrap();
    let (changed, message) = stitch.push(after_data).unwrap();
    assert!(!changed, "{message}");
    assert!(message.contains("Could not align"), "{message}");
    assert_eq!(stitch.position(), 0);
    assert_eq!(stitch.origin(), 0);
    assert_eq!(stitch.accepted(), 1);
    assert_eq!(stitch.output().unwrap().data, before.data);
    assert_eq!(
        stitch.previous().unwrap().as_bytes(),
        before.data.as_slice()
    );
}

#[test]
fn the_fixture_matcher_reads_borrowed_slices() {
    // A compile-level guard: matching takes `FrameRef` borrows, so it cannot
    // clone its inputs into owned buffers.
    let (width, height, before) = fixture("browser-overlap-before.png");
    let (_, _, after) = fixture("browser-overlap-after.png");
    let before = FrameData::new(before, width, height, 3);
    let after = FrameData::new(after, width, height, 3);
    let borrowed_before: FrameRef<'_> = before.view();
    let borrowed_after: FrameRef<'_> = after.view();
    assert_eq!(
        borrowed_before.as_bytes().as_ptr(),
        before.data.as_ptr(),
        "the view aliases the stored buffer"
    );
    assert!(match_translation(borrowed_before, borrowed_after).is_ok());
}
