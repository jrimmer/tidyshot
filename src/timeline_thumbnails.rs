//! Bounded thumbnail decoding for the source-time timeline.
// Ported from omnishot/timeline_thumbnails.py
//!
//! The source decodes with OpenCV on a worker thread. Here the decode is an
//! `ffmpeg` command through [`CommandRunner`], and the cache is a plain
//! most-recently-used map capped at [`LIMIT`] entries so a long recording
//! cannot grow without bound. Selecting which tiles to request is the
//! timeline's job; this module only fetches and retains them.

use std::collections::VecDeque;
use std::path::Path;
use std::time::Duration;

use crate::commands::{CommandRunner, CommandSpec};

/// The most thumbnails the cache keeps.
pub const LIMIT: usize = 192;
/// The default thumbnail decode size, as `(width, height)`.
pub const MAXIMUM: (u32, u32) = (320, 128);

/// A most-recently-used thumbnail cache keyed by source milliseconds.
#[derive(Debug, Default)]
pub struct ThumbnailCache {
    entries: VecDeque<(u64, Vec<u8>)>,
    /// Set when a decode failed; further requests are skipped.
    error: bool,
}

impl ThumbnailCache {
    /// A new, empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The bitmap stored for `milliseconds`, if any; marks it most-recent.
    pub fn get(&mut self, milliseconds: u64) -> Option<&[u8]> {
        let position = self
            .entries
            .iter()
            .position(|(key, _)| *key == milliseconds)?;
        let entry = self.entries.remove(position)?;
        self.entries.push_back(entry);
        self.entries.back().map(|(_, data)| data.as_slice())
    }

    /// The number of cached thumbnails.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether a decode has failed.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.error
    }

    /// Insert decoded thumbnails, evicting the oldest above [`LIMIT`].
    pub fn extend<I>(&mut self, images: I)
    where
        I: IntoIterator<Item = (u64, Vec<u8>)>,
    {
        for (key, data) in images {
            if self.entries.iter().any(|(existing, _)| *existing == key) {
                continue;
            }
            self.entries.push_back((key, data));
        }
        while self.entries.len() > LIMIT {
            self.entries.pop_front();
        }
    }

    /// Record a decode failure.
    pub fn fail(&mut self) {
        self.error = true;
    }

    /// Drop every entry, as `close` does.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// The `ffmpeg` command that decodes one thumbnail as a PPM frame.
///
/// `milliseconds` seeks before the input so a long recording does not decode
/// from the start, and `-frames:v 1` bounds the output to one image.
// Ported from omnishot/timeline_thumbnails.py (decode)
#[must_use]
pub fn decode_spec(source: &Path, milliseconds: u64, maximum: (u32, u32)) -> CommandSpec {
    let seconds = format!("{:.3}", milliseconds as f64 / 1000.0);
    let scale = format!(
        "scale=w={}:h={}:force_original_aspect_ratio=decrease",
        maximum.0, maximum.1
    );
    CommandSpec::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error"])
        .args(["-ss"])
        .arg(seconds)
        .args(["-i"])
        .arg(source.to_string_lossy().into_owned())
        .args(["-frames:v", "1", "-vf"])
        .arg(scale)
        .args(["-f", "image2pipe", "-vcodec", "ppm", "-"])
        .timeout(Duration::from_secs(15))
}

/// Decode one thumbnail, returning its PPM bytes.
pub fn decode<R: CommandRunner>(
    runner: &R,
    source: &Path,
    milliseconds: u64,
) -> Result<Vec<u8>, String> {
    let output = runner
        .run(&decode_spec(source, milliseconds, MAXIMUM))
        .map_err(|error| error.to_string())?;
    if output.stdout.is_empty() {
        return Err("Cannot decode timeline thumbnails".to_owned());
    }
    Ok(output.stdout)
}

/// Request a bounded batch of thumbnails, skipping any already cached.
///
/// At most 40 tiles are decoded per call, matching the source's limit, and a
/// decode error stops the batch and marks the cache failed.
pub fn request<R: CommandRunner>(
    runner: &R,
    cache: &mut ThumbnailCache,
    source: &Path,
    times: &[u64],
) -> Result<usize, String> {
    if cache.failed() {
        return Ok(0);
    }
    let missing: Vec<u64> = times
        .iter()
        .copied()
        .filter(|time| cache.get(*time).is_none())
        .take(40)
        .collect();
    let mut decoded = 0;
    for time in missing {
        match decode(runner, source, time) {
            Ok(bytes) => {
                cache.extend(std::iter::once((time, bytes)));
                decoded += 1;
            }
            Err(message) => {
                cache.fail();
                return Err(message);
            }
        }
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandError, CommandOutput};
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRunner {
        seen: Mutex<Vec<CommandSpec>>,
        fail: bool,
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            self.seen.lock().unwrap().push(spec.clone());
            if self.fail {
                return Err(CommandError::Failure {
                    program: spec.program.clone(),
                    output: CommandOutput {
                        status: 1,
                        stdout: Vec::new(),
                        stderr: b"decode failed".to_vec(),
                    },
                });
            }
            Ok(CommandOutput {
                status: 0,
                stdout: b"P6 thumbnail".to_vec(),
                stderr: Vec::new(),
            })
        }
    }

    #[test]
    fn the_decode_command_seeks_scales_and_bounds_the_frame() {
        let spec = decode_spec(Path::new("/tmp/source.mp4"), 1500, (320, 128));
        assert_eq!(spec.program, "ffmpeg");
        assert!(spec.args.windows(2).any(|w| w == ["-ss", "1.500"]));
        assert!(spec.args.windows(2).any(|w| w == ["-frames:v", "1"]));
        assert!(
            spec.args
                .iter()
                .any(|a| a == "scale=w=320:h=128:force_original_aspect_ratio=decrease")
        );
        assert_eq!(spec.args.last().map(String::as_str), Some("-"));
    }

    #[test]
    fn the_cache_is_most_recently_used_and_bounded() {
        let mut cache = ThumbnailCache::new();
        cache.extend((0..LIMIT as u64 + 20).map(|i| (i, vec![i as u8])));
        assert_eq!(cache.len(), LIMIT);
        // The oldest 20 entries evicted.
        assert!(cache.get(0).is_none());
        assert_eq!(cache.get(LIMIT as u64 + 19), Some(&[211u8][..]));
        // A hit marks it most-recent, so it survives the next eviction.
        assert!(cache.get(LIMIT as u64).is_some());
        cache.extend(std::iter::once((9999, b"new".to_vec())));
        assert!(cache.get(LIMIT as u64).is_some());
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn a_batch_skips_cached_times_and_a_failure_stops_it() {
        let runner = FakeRunner::default();
        let mut cache = ThumbnailCache::new();
        let decoded = request(
            &runner,
            &mut cache,
            Path::new("/tmp/source.mp4"),
            &[0, 1000, 2000],
        )
        .unwrap();
        assert_eq!(decoded, 3);
        assert_eq!(cache.len(), 3);
        let decoded = request(
            &runner,
            &mut cache,
            Path::new("/tmp/source.mp4"),
            &[0, 1000, 3000],
        )
        .unwrap();
        assert_eq!(decoded, 1);
        assert_eq!(runner.seen.lock().unwrap().len(), 4);

        let failing = FakeRunner {
            fail: true,
            ..FakeRunner::default()
        };
        let mut cache = ThumbnailCache::new();
        assert!(request(&failing, &mut cache, Path::new("/tmp/source.mp4"), &[0]).is_err());
        assert!(cache.failed());
        assert!(request(&failing, &mut cache, Path::new("/tmp/source.mp4"), &[1000]).is_ok());
        // A failed cache never invokes the runner again.
        assert_eq!(failing.seen.lock().unwrap().len(), 1);
    }
}
