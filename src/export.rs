//! Image export formats, quality controls and external opening.
// Ported from omnishot/backend.py (save_image, copy_image) and
// omnishot/image_project.py (write_image).
//!
//! PNG is encoded in-process. JPEG, WebP and HEIC go through the `ffmpeg`
//! command line, the same boundary every other media path in the app uses;
//! raw RGBA8 frames are streamed to the encoder and the encoded bytes are
//! written with [`crate::storage::atomic_write_file`]. A format the local
//! ffmpeg cannot encode reports its stderr instead of writing a broken file.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::commands::{CommandError, CommandRunner, CommandSpec};
use crate::json::Value;
use crate::pngzip::Image;

/// An output image format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Png,
    Jpeg,
    Webp,
    Heic,
}

impl ExportFormat {
    /// Every format the export menu offers, in menu order.
    pub const ALL: &'static [Self] = &[Self::Png, Self::Jpeg, Self::Webp, Self::Heic];

    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Webp => "webp",
            Self::Heic => "heic",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Webp => "WebP",
            Self::Heic => "HEIC",
        }
    }

    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Heic => "heic",
        }
    }

    #[must_use]
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Heic => "image/heic",
        }
    }

    /// The settings key holding this format's quality.
    #[must_use]
    pub const fn quality_key(self) -> Option<&'static str> {
        match self {
            Self::Png => None,
            Self::Jpeg => Some("jpeg_quality"),
            Self::Webp => Some("webp_quality"),
            Self::Heic => Some("heic_quality"),
        }
    }

    /// Parse a format id or a file extension, case-insensitively.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpeg" | "jpg" | "jpe" => Some(Self::Jpeg),
            "webp" => Some(Self::Webp),
            "heic" | "heif" => Some(Self::Heic),
            _ => None,
        }
    }

    /// The format a saved path names, defaulting to PNG.
    #[must_use]
    pub fn from_path(path: &Path) -> Self {
        path.extension()
            .and_then(|extension| extension.to_str())
            .and_then(Self::parse)
            .unwrap_or(Self::Png)
    }

    /// The stored default export format.
    #[must_use]
    pub fn from_settings(settings: &std::collections::BTreeMap<String, Value>) -> Self {
        settings
            .get("export_format")
            .and_then(Value::as_str)
            .and_then(Self::parse)
            .unwrap_or(Self::Png)
    }

    /// Whether the format keeps the alpha channel.
    #[must_use]
    pub const fn keeps_alpha(self) -> bool {
        !matches!(self, Self::Jpeg)
    }
}

/// The quality of a lossy export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportOptions {
    pub format: ExportFormat,
    /// 1 to 100, higher keeps more detail. PNG ignores it.
    pub quality: u32,
}

impl ExportOptions {
    /// The stored options for a format.
    #[must_use]
    pub fn from_settings(
        settings: &std::collections::BTreeMap<String, Value>,
        format: ExportFormat,
    ) -> Self {
        let quality = format
            .quality_key()
            .and_then(|key| settings.get(key))
            .and_then(Value::as_f64)
            .map_or(90.0, |value| value.clamp(1.0, 100.0)) as u32;
        Self { format, quality }
    }

    /// The stored default format with its configured quality.
    #[must_use]
    pub fn default_from_settings(settings: &std::collections::BTreeMap<String, Value>) -> Self {
        Self::from_settings(settings, ExportFormat::from_settings(settings))
    }
}

/// Why an export failed.
#[derive(Debug)]
pub enum ExportError {
    /// The format has no encoder in this build of ffmpeg.
    Unsupported(String),
    Command(CommandError),
    Io(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(message) => f.write_str(message),
            Self::Command(error) => error.fmt(f),
            Self::Io(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<CommandError> for ExportError {
    fn from(error: CommandError) -> Self {
        Self::Command(error)
    }
}

impl From<std::io::Error> for ExportError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

/// The JPEG quantizer for a 1..100 quality, on ffmpeg's 2..31 scale.
#[must_use]
pub fn jpeg_qscale(quality: u32) -> u32 {
    let quality = quality.clamp(1, 100);
    // quality 100 -> q 2, quality 1 -> q 31.
    (31.0 - (f64::from(quality) - 1.0) * 29.0 / 99.0).round() as u32
}

/// The encoder command for a format fed raw RGBA8 on stdin.
///
/// PNG is encoded in-process and has no command, so it returns `None`. HEIC
/// has no separate encoder crate; the local ffmpeg writes HEVC into the HEIF
/// muxer, falling back to the QuickTime/ISO-BMFF muxer on a build without the
/// HEIF one. [`encode_specs`] returns every attempt.
#[must_use]
pub fn encode_spec(
    format: ExportFormat,
    width: u32,
    height: u32,
    quality: u32,
) -> Option<CommandSpec> {
    if format == ExportFormat::Png {
        return None;
    }
    encode_specs(format, width, height, quality).into_iter().next()
}

/// Every encoder attempt for a format, in order.
///
/// JPEG and WebP have one. HEIC has the HEIF muxer first and the ISO-BMFF
/// `mov` muxer second, because a distribution may ship libx265 without the
/// HEIF muxer.
#[must_use]
pub fn encode_specs(
    format: ExportFormat,
    width: u32,
    height: u32,
    quality: u32,
) -> Vec<CommandSpec> {
    let raw = || {
        CommandSpec::new("ffmpeg").args([
            "-v",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-s",
            &format!("{width}x{height}"),
            "-i",
            "-",
            "-frames:v",
            "1",
        ])
    };
    let specs = match format {
        ExportFormat::Png => Vec::new(),
        ExportFormat::Jpeg => vec![raw().args([
            "-c:v",
            "mjpeg",
            "-q:v",
            &jpeg_qscale(quality).to_string(),
            "-pix_fmt",
            "yuvj444p",
            "-f",
            "image2pipe",
            "-",
        ])],
        ExportFormat::Webp => vec![raw().args([
            "-c:v",
            "libwebp",
            "-quality",
            &quality.clamp(1, 100).to_string(),
            "-f",
            "webp",
            "-",
        ])],
        // HEIC is written through a temporary file: the HEVC muxers refuse a
        // non-seekable pipe, so the output path is filled in by `encode`.
        ExportFormat::Heic => {
            let crf = heic_crf(quality).to_string();
            vec![
                raw().args([
                    "-c:v",
                    "libx265",
                    "-preset",
                    "medium",
                    "-crf",
                    &crf,
                    "-pix_fmt",
                    "yuv420p",
                    "-f",
                    "heif",
                ]),
                raw().args([
                    "-c:v",
                    "libx265",
                    "-preset",
                    "medium",
                    "-crf",
                    &crf,
                    "-pix_fmt",
                    "yuv420p",
                    "-tag:v",
                    "hvc1",
                    "-f",
                    "mov",
                ]),
            ]
        }
    };
    specs
        .into_iter()
        .map(|spec| spec.timeout(Duration::from_secs(120)))
        .collect()
}

/// The x265 CRF for a 1..100 quality, on the 51..18 scale.
#[must_use]
pub fn heic_crf(quality: u32) -> u32 {
    let quality = quality.clamp(1, 100);
    (51.0 - (f64::from(quality) - 1.0) * 33.0 / 99.0).round() as u32
}

/// Encode an image in the requested format.
///
/// PNG never spawns a process; every lossy format streams its RGBA8 buffer to
/// one ffmpeg process and returns the encoded bytes.
pub fn encode<R: CommandRunner>(
    runner: &R,
    image: &Image,
    options: ExportOptions,
) -> Result<Vec<u8>, ExportError> {
    if options.format == ExportFormat::Png {
        return Ok(image.to_png());
    }
    let specs = encode_specs(
        options.format,
        image.width as u32,
        image.height as u32,
        options.quality,
    );
    if specs.is_empty() {
        return Err(ExportError::Unsupported(
            "no encoder for this format".to_owned(),
        ));
    }
    let mut last_detail = String::new();
    for spec in specs {
        let attempt = if options.format == ExportFormat::Heic {
            // The muxer needs a seekable destination, so the raw frame still
            // arrives on stdin but the container lands in a sibling temp file.
            let destination = temp_path(options.format);
            let mut file_spec = spec.clone();
            file_spec.args.push(destination.to_string_lossy().into_owned());
            let result = runner.run(&file_spec.clone().stdin(image.data.clone()));
            let bytes = std::fs::read(&destination).unwrap_or_default();
            let _ = std::fs::remove_file(&destination);
            match result {
                Ok(_) if !bytes.is_empty() => Ok(bytes),
                Ok(output) => Err(CommandError::Failure {
                    program: file_spec.program.clone(),
                    output,
                }),
                Err(error) => Err(error),
            }
        } else {
            runner
                .run(&spec.clone().stdin(image.data.clone()))
                .map(|output| output.stdout)
        };
        match attempt {
            Ok(bytes) if !bytes.is_empty() => return Ok(bytes),
            Ok(_) => continue,
            Err(CommandError::Failure { output, .. }) => {
                last_detail = output.stderr_string();
            }
            Err(error) => return Err(ExportError::Command(error)),
        }
    }
    let detail = last_detail.trim();
    Err(ExportError::Unsupported(if detail.is_empty() {
        format!("{} export produced no data", options.format.label())
    } else {
        format!("{} export failed: {detail}", options.format.label())
    }))
}

/// A unique temp path for an encoder that needs a seekable destination.
fn temp_path(format: ExportFormat) -> std::path::PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!(
        ".tidyshot-export-{}-{nonce:x}.{}",
        std::process::id(),
        format.extension()
    ))
}

/// Write an encoded image to `path` atomically.
pub fn write<R: CommandRunner>(
    runner: &R,
    image: &Image,
    path: &Path,
    options: ExportOptions,
) -> Result<(), ExportError> {
    let bytes = encode(runner, image, options)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::storage::atomic_write_file(path, &bytes)?;
    Ok(())
}

/// The export path for a capture in another format.
///
/// The stem is kept so the export sits beside the capture, and an existing
/// file is never replaced.
#[must_use]
pub fn export_path(capture: &Path, format: ExportFormat) -> PathBuf {
    let parent = capture.parent().unwrap_or_else(|| Path::new("."));
    let stem = capture
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("capture");
    let mut candidate = parent.join(format!("{stem}.{}", format.extension()));
    let mut sequence = 2;
    while candidate.exists() {
        candidate = parent.join(format!(
            "{stem} ({sequence}).{}",
            format.extension()
        ));
        sequence += 1;
    }
    candidate
}

/// Decode an encoded image back to RGBA8 through ffmpeg.
///
/// The export tests use this to prove a written file is a real image with the
/// expected dimensions; nothing is guessed from headers.
pub fn decode<R: CommandRunner>(
    runner: &R,
    bytes: &[u8],
    format: ExportFormat,
) -> Result<Image, ExportError> {
    if format == ExportFormat::Png {
        return crate::pngzip::decode_png(bytes)
            .map_err(|error| ExportError::Unsupported(error.to_string()));
    }
    // A container needs a seekable input, so the bytes are written to a temp
    // file that both probes read.
    let temporary = temp_path(format);
    std::fs::write(&temporary, bytes)?;
    let result: Result<_, ExportError> = (|| {
        let probe = CommandSpec::new("ffprobe")
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
            .arg(temporary.to_string_lossy().into_owned())
            .timeout(Duration::from_secs(30));
        let info = runner.run(&probe)?;
        let (width, height) = parse_dimensions(&info.stdout_string()).ok_or_else(|| {
            ExportError::Unsupported("the encoded image has no readable dimensions".to_owned())
        })?;
        let decode = CommandSpec::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(temporary.to_string_lossy().into_owned())
            .args([
                "-frames:v",
                "1",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-",
            ])
            .timeout(Duration::from_secs(30));
        let raw = runner.run(&decode)?;
        Ok((width, height, raw))
    })();
    let _ = std::fs::remove_file(&temporary);
    let (width, height, raw) = result?;
    let expected = width as usize * height as usize * 4;
    if raw.stdout.len() < expected {
        return Err(ExportError::Unsupported(
            "the encoded image did not decode".to_owned(),
        ));
    }
    Ok(Image::new(
        width as usize,
        height as usize,
        raw.stdout[..expected].to_vec(),
    ))
}

fn parse_dimensions(json: &str) -> Option<(u32, u32)> {
    let value = crate::json::parse(json).ok()?;
    let stream = value.get("streams")?.as_array()?.first()?;
    let width = stream.get("width")?.as_f64()? as u32;
    let height = stream.get("height")?.as_f64()? as u32;
    if width == 0 || height == 0 {
        return None;
    }
    Some((width, height))
}

/// The command that opens a file in the user's default application.
// Ported from omnishot/backend.py (open_path)
#[must_use]
pub fn external_open_spec(path: &Path) -> CommandSpec {
    CommandSpec::new("xdg-open")
        .arg(path.to_string_lossy().into_owned())
        .timeout(Duration::from_secs(10))
}

/// Open a file or folder with the desktop's default application.
pub fn open_external<R: CommandRunner>(runner: &R, path: &Path) -> Result<(), ExportError> {
    if !path.exists() {
        return Err(ExportError::Io(format!(
            "the file is no longer available: {}",
            path.display()
        )));
    }
    runner.run(&external_open_spec(path))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_round_trip_through_ids_extensions_and_mimes() {
        for format in ExportFormat::ALL {
            assert_eq!(ExportFormat::parse(format.id()), Some(*format));
            assert_eq!(
                ExportFormat::from_path(Path::new(&format!("a.{}", format.extension()))),
                *format
            );
            assert!(format.mime().starts_with("image/"));
        }
        assert_eq!(ExportFormat::parse("JPG"), Some(ExportFormat::Jpeg));
        assert_eq!(ExportFormat::parse("heif"), Some(ExportFormat::Heic));
        assert_eq!(ExportFormat::from_path(Path::new("a.unknown")), ExportFormat::Png);
        assert!(!ExportFormat::Jpeg.keeps_alpha());
        assert!(ExportFormat::Webp.keeps_alpha());
    }

    #[test]
    fn the_stored_format_and_quality_are_read_from_settings() {
        let settings = std::collections::BTreeMap::from([
            (
                "export_format".to_owned(),
                Value::String("webp".to_owned()),
            ),
            ("webp_quality".to_owned(), Value::Number(63.0)),
        ]);
        let options = ExportOptions::default_from_settings(&settings);
        assert_eq!(options.format, ExportFormat::Webp);
        assert_eq!(options.quality, 63);
        assert_eq!(
            ExportOptions::from_settings(&settings, ExportFormat::Jpeg).quality,
            90,
            "a missing quality keeps the source default"
        );
        let clamped = std::collections::BTreeMap::from([(
            "jpeg_quality".to_owned(),
            Value::Number(500.0),
        )]);
        assert_eq!(
            ExportOptions::from_settings(&clamped, ExportFormat::Jpeg).quality,
            100
        );
    }

    #[test]
    fn quality_maps_onto_the_encoder_scales() {
        assert_eq!(jpeg_qscale(100), 2);
        assert_eq!(jpeg_qscale(1), 31);
        assert!(jpeg_qscale(90) < jpeg_qscale(50));
        assert_eq!(heic_crf(100), 18);
        assert_eq!(heic_crf(1), 51);
    }

    #[test]
    fn png_has_no_encoder_command_and_lossy_formats_do() {
        assert!(encode_spec(ExportFormat::Png, 4, 3, 90).is_none());
        let webp = encode_spec(ExportFormat::Webp, 4, 3, 80).unwrap();
        assert_eq!(webp.program, "ffmpeg");
        assert!(webp.args.contains(&"libwebp".to_owned()));
        assert!(webp.args.contains(&"80".to_owned()));
        let jpeg = encode_spec(ExportFormat::Jpeg, 4, 3, 90).unwrap();
        assert!(jpeg.args.contains(&"mjpeg".to_owned()));
        let heic = encode_spec(ExportFormat::Heic, 4, 3, 90).unwrap();
        assert!(heic.args.contains(&"libx265".to_owned()));
        assert!(heic.args.contains(&"heif".to_owned()));
        assert!(webp.args.contains(&"4x3".to_owned()));
    }

    #[test]
    fn an_export_path_never_replaces_an_existing_file() {
        let dir = std::env::temp_dir().join(format!("tidyshot-export-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let capture = dir.join("shot.png");
        std::fs::write(&capture, b"png").unwrap();
        let first = export_path(&capture, ExportFormat::Webp);
        assert_eq!(first, dir.join("shot.webp"));
        std::fs::write(&first, b"webp").unwrap();
        let second = export_path(&capture, ExportFormat::Webp);
        assert_eq!(second, dir.join("shot (2).webp"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_external_open_spec_names_the_file() {
        let spec = external_open_spec(Path::new("/tmp/shot.webp"));
        assert_eq!(spec.program, "xdg-open");
        assert_eq!(spec.args, vec!["/tmp/shot.webp".to_owned()]);
    }
}
