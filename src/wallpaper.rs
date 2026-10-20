//! The desktop wallpaper as a capture background.
// Ported from omnishot/wallpaper.py
//!
//! Omarchy keeps the active wallpaper as a path in its state directory. It is
//! decoded once through ffmpeg into the RGBA8 buffer the compositor uses, and
//! cached beside the capture store when the user asked for the wallpaper to
//! follow changes. A custom wallpaper from settings wins over the desktop's.
//!
//! Nothing here modifies the wallpaper file: it is read, decoded and cached.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::commands::{CommandError, CommandRunner, CommandSpec};
use crate::json::Value;
use crate::pngzip::Image;

/// The largest wallpaper side, and pixel count, a background may decode to.
pub const MAX_SIDE: u32 = 16_384;
pub const MAX_PIXELS: u64 = 120_000_000;

/// Why a wallpaper could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WallpaperError {
    message: String,
}

impl WallpaperError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for WallpaperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for WallpaperError {}

impl From<std::io::Error> for WallpaperError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<CommandError> for WallpaperError {
    fn from(error: CommandError) -> Self {
        Self::new(error.to_string())
    }
}

/// The active desktop wallpaper path, when one is configured.
///
/// The Omarchy state directory is preferred; the older config location is
/// still checked so an upgraded install keeps its background.
// Ported from omnishot/wallpaper.py (desktop_path)
#[must_use]
pub fn desktop_path_in(home: &Path) -> Option<PathBuf> {
    for candidate in [
        home.join(".local/state/omarchy/current/background"),
        home.join(".config/omarchy/current/background"),
    ] {
        if candidate.is_file() {
            return std::fs::canonicalize(&candidate).ok().or(Some(candidate));
        }
    }
    None
}

/// The active desktop wallpaper path for the current user.
#[must_use]
pub fn desktop_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    desktop_path_in(&home)
}

/// The cached copy of the wallpaper inside a store root.
#[must_use]
pub fn cache_path(root: &Path) -> PathBuf {
    root.join("wallpaper-desktop.png")
}

/// Decode any image file to RGBA8 through ffmpeg.
///
/// The file is read, never written: the wallpaper belongs to the desktop and
/// stays untouched. Oversized or malformed images are rejected before the
/// buffer is allocated.
// Ported from omnishot/wallpaper.py (image_data)
pub fn decode<R: CommandRunner>(runner: &R, path: &Path) -> Result<Image, WallpaperError> {
    let probe = CommandSpec::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
        ])
        .arg(path.to_string_lossy().into_owned())
        .timeout(Duration::from_secs(30));
    let probe = runner.run(&probe)?;
    let text = probe.stdout_string();
    let mut parts = text.trim().split(',');
    let width: u32 = parts
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| WallpaperError::new("Could not read the wallpaper size"))?;
    let height: u32 = parts
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| WallpaperError::new("Could not read the wallpaper size"))?;
    if width == 0
        || height == 0
        || width > MAX_SIDE
        || height > MAX_SIDE
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err(WallpaperError::new("The wallpaper is too large"));
    }
    let decode = CommandSpec::new("ffmpeg")
        .args(["-v", "error", "-xerror", "-i"])
        .arg(path.to_string_lossy().into_owned())
        .args([
            "-map",
            "0:v:0",
            "-frames:v",
            "1",
            "-an",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-",
        ])
        .timeout(Duration::from_secs(30));
    let output = runner.run(&decode)?;
    let expected = width as usize * height as usize * 4;
    if output.stdout.len() < expected {
        return Err(WallpaperError::new("Could not decode the wallpaper"));
    }
    Ok(Image::new(
        width as usize,
        height as usize,
        output.stdout[..expected].to_vec(),
    ))
}

/// The wallpaper preferences from a settings map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WallpaperSettings {
    /// `desktop` or `custom`.
    pub source: String,
    /// The cached custom wallpaper, as base64 PNG, when one was chosen.
    pub data: String,
    /// Whether a changed desktop wallpaper is re-read every time.
    pub follow_changes: bool,
}

impl Default for WallpaperSettings {
    fn default() -> Self {
        Self {
            source: "desktop".to_owned(),
            data: String::new(),
            follow_changes: true,
        }
    }
}

impl WallpaperSettings {
    /// Read the wallpaper keys from a saved settings map.
    #[must_use]
    pub fn from_settings(settings: &std::collections::BTreeMap<String, Value>) -> Self {
        let text = |key: &str, fallback: &str| {
            settings
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or(fallback)
                .to_owned()
        };
        Self {
            source: text("wallpaper_source", "desktop"),
            data: text("wallpaper_data", ""),
            follow_changes: settings
                .get("wallpaper_follow_changes")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        }
    }

    /// Whether the background should use a custom image.
    #[must_use]
    pub fn is_custom(&self) -> bool {
        self.source == "custom"
    }
}

/// Load the background image a window capture should sit on.
///
/// A custom wallpaper is decoded from the stored base64 PNG. Otherwise the
/// desktop wallpaper is decoded and, unless changes are followed, the cached
/// copy is used so the capture matches the background it was framed against.
// Ported from omnishot/wallpaper.py (capture_wallpaper)
pub fn capture_wallpaper<R: CommandRunner>(
    runner: &R,
    settings: &WallpaperSettings,
    root: &Path,
) -> Result<Option<Image>, WallpaperError> {
    if settings.is_custom() {
        if settings.data.is_empty() {
            return Ok(None);
        }
        return crate::annotation::decode_base64(&settings.data)
            .ok_or_else(|| WallpaperError::new("The custom wallpaper is not valid PNG data"))
            .and_then(|bytes| {
                crate::pngzip::decode_png(&bytes)
                    .map_err(|error| WallpaperError::new(error.to_string()))
            })
            .map(Some);
    }
    let cached = cache_path(root);
    if !settings.follow_changes && cached.is_file() {
        let bytes = std::fs::read(&cached)?;
        return crate::pngzip::decode_png(&bytes)
            .map(Some)
            .map_err(|error| WallpaperError::new(error.to_string()));
    }
    let Some(path) = desktop_path() else {
        return Ok(None);
    };
    let image = decode(runner, &path)?;
    // The cache is an optimisation only: a failed write still yields the
    // decoded background for this capture.
    let _ = crate::storage::atomic_write_file(&cached, &image.to_png());
    Ok(Some(image))
}

/// A custom wallpaper encoded for the settings file.
#[must_use]
pub fn encode_custom(image: &Image) -> String {
    crate::annotation::encode_base64(&image.to_png())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandOutput, CommandSpec};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        results: Mutex<VecDeque<CommandOutput>>,
        seen: Mutex<Vec<CommandSpec>>,
    }

    impl Fake {
        fn push(&self, stdout: Vec<u8>) {
            self.results.lock().unwrap().push_back(CommandOutput {
                status: 0,
                stdout,
                stderr: Vec::new(),
            });
        }
    }

    impl CommandRunner for Fake {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            self.seen.lock().unwrap().push(spec.clone());
            self.results
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| CommandError::Spawn {
                    program: spec.program.clone(),
                    source: std::io::Error::other("no queued output"),
                })
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tidyshot-wallpaper-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_omarchy_state_background_is_preferred() {
        let home = scratch("desktop");
        std::fs::create_dir_all(home.join(".config/omarchy/current")).unwrap();
        std::fs::write(home.join(".config/omarchy/current/background"), b"old").unwrap();
        assert_eq!(
            desktop_path_in(&home),
            Some(home.join(".config/omarchy/current/background"))
        );
        std::fs::create_dir_all(home.join(".local/state/omarchy/current")).unwrap();
        std::fs::write(home.join(".local/state/omarchy/current/background"), b"new").unwrap();
        assert!(
            desktop_path_in(&home)
                .unwrap()
                .ends_with(".local/state/omarchy/current/background")
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_background_is_decoded_through_ffmpeg_once_and_cached() {
        let home = scratch("decode");
        let wallpaper = home.join("wallpaper.png");
        std::fs::write(&wallpaper, b"not really an image").unwrap();
        let runner = Fake::default();
        runner.push(b"4,2".to_vec());
        let pixels: Vec<u8> = (0..4 * 2 * 4).map(|index| index as u8).collect();
        runner.push(pixels.clone());
        let image = decode(&runner, &wallpaper).unwrap();
        assert_eq!((image.width, image.height), (4, 2));
        assert_eq!(image.data, pixels);
        let seen = runner.seen.lock().unwrap().clone();
        assert!(seen[0].program == "ffprobe");
        assert!(seen[1].program == "ffmpeg");
        assert!(seen[1].args.iter().any(|arg| arg == "rgba"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_truncated_decode_is_reported() {
        let home = scratch("truncated");
        let wallpaper = home.join("wallpaper.png");
        std::fs::write(&wallpaper, b"x").unwrap();
        let runner = Fake::default();
        runner.push(b"4,2".to_vec());
        runner.push(vec![0u8; 8]);
        let error = decode(&runner, &wallpaper).unwrap_err();
        assert!(error.to_string().contains("decode"), "{error}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_custom_wallpaper_wins_over_the_desktop() {
        let root = scratch("custom");
        let image = Image::new(2, 2, vec![7u8; 16]);
        let settings = WallpaperSettings {
            source: "custom".to_owned(),
            data: encode_custom(&image),
            follow_changes: true,
        };
        let runner = Fake::default();
        let loaded = capture_wallpaper(&runner, &settings, &root).unwrap().unwrap();
        assert_eq!(loaded.pixel(0, 0), [7, 7, 7, 7]);
        assert!(
            runner.seen.lock().unwrap().is_empty(),
            "a custom wallpaper needs no ffmpeg"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn settings_round_trip_the_wallpaper_keys() {
        let value = crate::json::parse(
            r#"{"wallpaper_source":"custom","wallpaper_data":"abc","wallpaper_follow_changes":false}"#,
        )
        .unwrap();
        let settings = WallpaperSettings::from_settings(value.as_object().unwrap());
        assert!(settings.is_custom());
        assert_eq!(settings.data, "abc");
        assert!(!settings.follow_changes);
        assert_eq!(WallpaperSettings::default().source, "desktop");
    }
}
