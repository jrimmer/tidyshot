//! The `gpu-screen-recorder` invocation and its options.
// Ported from omnishot/recording.py (recorder_command)
// Ported from omnishot/video_audio_tracks.py (capture_audio_sources)
//!
//! The recorder is a command line the Qt layer launches through
//! [`crate::commands`]. These functions build its argument vector only, so
//! the transport stays behind the runner trait.

use std::path::Path;

use super::display::{CleanCapture, Monitor};
use super::options::{recording_monitors, resolution_limit};

/// The audio sources a take records, in the order they are mixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSource {
    System,
    Microphone,
}

impl AudioSource {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            AudioSource::System => "system",
            AudioSource::Microphone => "microphone",
        }
    }
}

/// The per-track editing defaults a finished take carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackEdit {
    pub source: AudioSource,
    pub enabled: bool,
    pub volume: u32,
}

/// Which audio tracks the initial video keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioTracks {
    #[default]
    Single,
    Separate,
}

/// Options that decide what and how a take records.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordOptions {
    pub mode: CaptureMode,
    pub format: CaptureFormat,
    pub fps: u32,
    pub quality: String,
    pub size: String,
    pub scale_video: bool,
    pub system: bool,
    pub microphone: bool,
    pub system_device: Option<String>,
    pub microphone_device: Option<String>,
    pub separate_audio: bool,
    pub mono: bool,
    pub cursor: bool,
    pub studio: bool,
    pub clicks: bool,
    pub keys: bool,
    pub commands_only: bool,
    pub dnd: bool,
    pub camera: bool,
    pub camera_device: Option<String>,
    pub camera_shape: String,
    pub camera_size: f64,
    pub camera_mirror: bool,
    pub delay: i64,
    pub dim_screen: bool,
    pub show_controls: bool,
    pub show_time: bool,
    pub remember_selection: bool,
    pub gif_width: u32,
    pub gif_quality: u32,
    pub gif_optimize: bool,
    pub capture_output: Option<String>,
}

impl Default for RecordOptions {
    fn default() -> Self {
        Self {
            mode: CaptureMode::Area,
            format: CaptureFormat::Mp4,
            fps: 30,
            quality: "very_high".to_owned(),
            size: "Native".to_owned(),
            scale_video: false,
            system: false,
            microphone: false,
            system_device: None,
            microphone_device: None,
            separate_audio: false,
            mono: false,
            cursor: true,
            studio: false,
            clicks: false,
            keys: false,
            commands_only: true,
            dnd: true,
            camera: false,
            camera_device: None,
            camera_shape: "Circle".to_owned(),
            camera_size: 0.22,
            camera_mirror: false,
            delay: 3,
            dim_screen: true,
            show_controls: true,
            show_time: true,
            remember_selection: true,
            gif_width: 800,
            gif_quality: 80,
            gif_optimize: true,
            capture_output: None,
        }
    }
}

/// The capture the setup dialog selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptureMode {
    #[default]
    Area,
    Window,
    Fullscreen,
}

/// The output container a take is exported to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptureFormat {
    #[default]
    Mp4,
    Gif,
}

impl AudioSource {
    /// The source list the encoder mixes when a take records audio.
    ///
    /// Studio and per-track takes keep each source separate; an ordinary
    /// take with two sources joins them into one mixed stream.
    #[must_use]
    pub fn sources(options: &RecordOptions) -> Vec<AudioSource> {
        let mut sources = Vec::new();
        if options.system {
            sources.push(AudioSource::System);
        }
        if options.microphone {
            sources.push(AudioSource::Microphone);
        }
        sources
    }

    /// The audio-source names a take exposes for later per-track editing.
    #[must_use]
    pub fn track_names(options: &RecordOptions) -> Vec<String> {
        let sources = Self::sources(options);
        if options.studio || options.separate_audio || sources.len() < 2 {
            sources.iter().map(|s| s.label().to_owned()).collect()
        } else {
            vec!["mixed".to_owned()]
        }
    }
}

/// The editing defaults for the initial video's audio tracks.
#[must_use]
pub fn default_track_edits(sources: &[AudioSource]) -> Vec<TrackEdit> {
    sources
        .iter()
        .map(|source| TrackEdit {
            source: *source,
            enabled: true,
            volume: 100,
        })
        .collect()
}

/// The encoder argument vector for one take.
///
/// `rect` is the selected region for an area take, or `None` for a
/// fullscreen take, which records the output in `monitors` chosen by
/// `options.capture_output` (or the focused one). `clean_plugin` is the
/// `clean-capture.so` path when the clean mirror is active; `None` leaves
/// the compositor's own pointer handling in place.
pub fn recorder_command(
    path: &Path,
    rect: Option<(i64, i64, i64, i64)>,
    options: &RecordOptions,
    clean_plugin: Option<&Path>,
    monitors: &[Monitor],
) -> Result<Vec<String>, super::display::CaptureError> {
    let mut command = vec!["gpu-screen-recorder".to_owned()];
    let selected = if rect.is_none() {
        recording_monitors(monitors, options.capture_output.as_deref())?
    } else {
        // A region source never queries a fullscreen target.
        Vec::new()
    };
    if let Some(rect) = rect {
        let (x, y, w, h) = rect;
        command.extend([
            "-w".to_owned(),
            "region".to_owned(),
            "-region".to_owned(),
            format!("{w}x{h}+{x}+{y}"),
        ]);
    } else {
        let monitor = selected
            .iter()
            .find(|monitor| monitor.focused)
            .or_else(|| selected.first())
            .ok_or(super::display::CaptureError::NoDisplay)?;
        command.extend(["-w".to_owned(), monitor.name.clone()]);
    }
    let cursor = if options.cursor && !options.studio {
        "yes"
    } else {
        "no"
    };
    command.extend([
        "-k".to_owned(),
        "h264".to_owned(),
        "-f".to_owned(),
        options.fps.to_string(),
        "-fm".to_owned(),
        "cfr".to_owned(),
        "-q".to_owned(),
        options.quality.clone(),
        "-cursor".to_owned(),
        cursor.to_owned(),
        "-fallback-cpu-encoding".to_owned(),
        "yes".to_owned(),
        "-write-first-frame-ts".to_owned(),
        "yes".to_owned(),
        "-o".to_owned(),
        path.to_string_lossy().into_owned(),
    ]);
    // Commit recoverable MP4 fragments every second: a killed encoder keeps
    // the committed fragments instead of losing a buffered recording.
    command.extend([
        "-ffmpeg-opts".to_owned(),
        "movflags=+hybrid_fragmented;frag_duration=1000000;flush_packets=1".to_owned(),
    ]);
    if let Some(plugin) = clean_plugin {
        command.push("-p".to_owned());
        command.push(plugin.to_string_lossy().into_owned());
    }
    if let Some(limit) = resolution_limit(&options.size, options.scale_video, &selected, rect) {
        command.push("-s".to_owned());
        command.push(limit);
    }
    let devices = audio_devices(options);
    if !devices.is_empty() {
        command.extend(["-a".to_owned()]);
        // A separate or Studio take asks for one `-a` per source; an ordinary
        // two-source take joins them so the encoder mixes them itself.
        for device in &devices {
            command.push(device.clone());
            command.push("-a".to_owned());
        }
        command.pop();
        command.extend(["-ac".to_owned(), "aac".to_owned()]);
    }
    Ok(command)
}

/// The device names to pass, one per `-a`, honouring the separate-track flag.
fn audio_devices(options: &RecordOptions) -> Vec<String> {
    let mut devices = Vec::new();
    if options.system {
        devices.push(
            options
                .system_device
                .clone()
                .unwrap_or_else(|| "default_output".to_owned()),
        );
    }
    if options.microphone {
        devices.push(
            options
                .microphone_device
                .clone()
                .unwrap_or_else(|| "default_input".to_owned()),
        );
    }
    if devices.is_empty() {
        return devices;
    }
    if options.separate_audio || options.studio {
        devices
    } else {
        vec![devices.join("|")]
    }
}

/// Whether a take needs the clean mirror's source layout environment.
#[must_use]
pub fn needs_clean_capture(options: &RecordOptions) -> bool {
    options.cursor && !options.studio
}

/// The clean-capture layout for a take, if the clean mirror is active.
pub fn clean_capture(
    monitors: &[Monitor],
    rect: Option<(i64, i64, i64, i64)>,
    options: &RecordOptions,
) -> Result<Option<CleanCapture>, super::display::CaptureError> {
    if !needs_clean_capture(options) {
        return Ok(None);
    }
    CleanCapture::new(monitors, rect, true).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::display::{CaptureError, Monitor};

    fn monitor(name: &str, x: i64, y: i64, width: i64, height: i64, scale: f64) -> Monitor {
        let mut monitor = Monitor::new(name, x, y, width, height);
        monitor.scale = scale;
        monitor
    }

    fn first() -> Monitor {
        let mut m = monitor("first", 0, 0, 2880, 1800, 1.6);
        m.focused = false;
        m
    }

    fn second() -> Monitor {
        let mut m = monitor("second", 1800, 0, 1920, 1080, 1.0);
        m.focused = true;
        m
    }

    fn options() -> RecordOptions {
        RecordOptions {
            mode: CaptureMode::Fullscreen,
            fps: 10,
            quality: "high".to_owned(),
            size: "Native".to_owned(),
            scale_video: true,
            cursor: false,
            delay: 600,
            ..RecordOptions::default()
        }
    }

    #[test]
    fn region_capture_uses_region_source_and_limit() {
        let command = recorder_command(
            Path::new("/tmp/out.mp4"),
            Some((-1800, 740, 320, 180)),
            &options(),
            None,
            &[first(), second()],
        )
        .unwrap();
        let index = |flag: &str| command.iter().position(|arg| arg == flag).unwrap();
        assert_eq!(command[index("-w") + 1], "region");
        assert_eq!(command[index("-region") + 1], "320x180+-1800+740");
        assert_eq!(command[index("-s") + 1], "320x180");
    }

    #[test]
    fn fullscreen_keeps_the_display_chosen_before_the_countdown() {
        let command = recorder_command(
            Path::new("/tmp/out.mp4"),
            None,
            &options(),
            None,
            &[first(), second()],
        )
        .unwrap();
        let index = |flag: &str| command.iter().position(|arg| arg == flag).unwrap();
        assert_eq!(command[index("-w") + 1], "second");
        assert_eq!(command[index("-s") + 1], "1920x1080");
    }

    #[test]
    fn explicit_output_survives_a_monitor_layout_change() {
        let mut rotated = monitor("second", -1080, 0, 1920, 1080, 1.0);
        rotated.transform = 1;
        let opts = RecordOptions {
            capture_output: Some("second".to_owned()),
            ..options()
        };
        let command = recorder_command(
            Path::new("/tmp/out.mp4"),
            None,
            &opts,
            None,
            &[first(), rotated],
        )
        .unwrap();
        let index = |flag: &str| command.iter().position(|arg| arg == flag).unwrap();
        assert_eq!(command[index("-w") + 1], "second");
        assert_eq!(command[index("-s") + 1], "1080x1920");
    }

    #[test]
    fn disconnected_fullscreen_output_is_not_replaced() {
        let opts = RecordOptions {
            capture_output: Some("second".to_owned()),
            ..options()
        };
        assert_eq!(
            recorder_command(Path::new("/tmp/out.mp4"), None, &opts, None, &[first()]).unwrap_err(),
            CaptureError::NoDisplay
        );
    }

    #[test]
    fn clean_plugin_is_added_only_when_requested() {
        let command = recorder_command(
            Path::new("/tmp/out.mp4"),
            Some((0, 0, 900, 600)),
            &options(),
            Some(Path::new("/opt/omnishot/clean-capture.so")),
            &[],
        )
        .unwrap();
        let index = command.iter().position(|arg| arg == "-p").unwrap();
        assert_eq!(command[index + 1], "/opt/omnishot/clean-capture.so");
    }

    #[test]
    fn separate_audio_tracks_get_one_flag_per_source() {
        let opts = RecordOptions {
            system: true,
            microphone: true,
            separate_audio: true,
            ..options()
        };
        let command = recorder_command(
            Path::new("/tmp/out.mp4"),
            Some((0, 0, 900, 600)),
            &opts,
            None,
            &[],
        )
        .unwrap();
        let positions: Vec<_> = command
            .iter()
            .enumerate()
            .filter(|(_, arg)| *arg == "-a")
            .map(|(index, _)| index)
            .collect();
        assert_eq!(positions.len(), 2);
        assert_eq!(command[positions[0] + 1], "default_output");
        assert_eq!(command[positions[1] + 1], "default_input");
    }

    #[test]
    fn a_mixed_take_joins_its_two_sources() {
        let opts = RecordOptions {
            system: true,
            microphone: true,
            ..options()
        };
        let command = recorder_command(
            Path::new("/tmp/out.mp4"),
            Some((0, 0, 900, 600)),
            &opts,
            None,
            &[],
        )
        .unwrap();
        let index = command.iter().position(|arg| arg == "-a").unwrap();
        assert_eq!(command[index + 1], "default_output|default_input");
    }

    #[test]
    fn studio_takes_never_show_the_cursor_in_the_video() {
        let opts = RecordOptions {
            cursor: true,
            studio: true,
            ..options()
        };
        let command = recorder_command(
            Path::new("/tmp/out.mp4"),
            Some((0, 0, 900, 600)),
            &opts,
            None,
            &[],
        )
        .unwrap();
        let index = command.iter().position(|arg| arg == "-cursor").unwrap();
        assert_eq!(command[index + 1], "no");
    }

    #[test]
    fn track_names_keep_sources_separate_only_when_asked() {
        let mixed = RecordOptions {
            system: true,
            microphone: true,
            ..RecordOptions::default()
        };
        assert_eq!(AudioSource::track_names(&mixed), ["mixed"]);
        let separate = RecordOptions {
            separate_audio: true,
            ..mixed.clone()
        };
        assert_eq!(
            AudioSource::track_names(&separate),
            ["system", "microphone"]
        );
    }
}
