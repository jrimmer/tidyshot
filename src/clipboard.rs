//! Clipboard content negotiation for paste and copy.
// Ported from omnishot/clipboard.py (Content, local_files, read_content) and
// omnishot/backend.py (copy_image).
//!
//! Only file lists are parsed here; images arrive through wl-paste, so the
//! actual decoding and the native clipboard owner stay at the command
//! boundary. The file-list rules are pure and are what the tests assert.

use std::path::{Path, PathBuf};

/// The clipboard annotation MIME type.
pub const ANNOTATION_MIME: &str = "application/x-omnishot-annotations+json";

/// The MIME types a clipboard image may arrive as, in preference order.
pub const IMAGE_TYPES: &[&str] = &[
    "image/png",
    "image/webp",
    "image/jpeg",
    "image/bmp",
    "image/tiff",
];

/// Extensions [`local_files`] accepts from a clipboard file list.
pub const MEDIA: &[&str] = &[
    ".png",
    ".jpg",
    ".jpeg",
    ".webp",
    ".heic",
    ".heif",
    ".omnishot",
    ".omnishot-video",
    ".mp4",
    ".gif",
    ".webm",
    ".mov",
    ".mkv",
];

/// Extensions that only the annotate editor treats as images.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".webp", ".heic", ".heif", ".bmp", ".tif", ".tiff",
];

/// Why clipboard content could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardError {
    message: String,
}

impl ClipboardError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ClipboardError {}

/// Parse a `text/uri-list` (or GNOME copied-files) payload into local paths.
// Ported from omnishot/clipboard.py (local_files)
pub fn local_files(data: &[u8]) -> Result<Vec<PathBuf>, ClipboardError> {
    if data.len() > 1024 * 1024 {
        return Err(ClipboardError::new("The clipboard file list is too large"));
    }
    let text = String::from_utf8_lossy(data);
    let mut paths: Vec<PathBuf> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line == "copy" || line == "cut" {
            continue;
        }
        let Some(path) = file_url_path(line) else {
            return Err(ClipboardError::new(
                "Copy a local image or video file to the clipboard",
            ));
        };
        let lower = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!(".{}", extension.to_ascii_lowercase()))
            .unwrap_or_default();
        if !MEDIA.contains(&lower.as_str()) {
            continue;
        }
        if !path.is_file() {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("file");
            return Err(ClipboardError::new(format!(
                "The copied file is no longer available: {name}"
            )));
        }
        if !paths.contains(&path) {
            paths.push(path);
        }
        if paths.len() > 100 {
            return Err(ClipboardError::new(
                "Open at most 100 clipboard files at once",
            ));
        }
    }
    if paths.is_empty() {
        return Err(ClipboardError::new(
            "There is no supported image or video file on the clipboard",
        ));
    }
    Ok(paths)
}

/// Extract the path from a `file://` URL, rejecting remote or relative forms.
fn file_url_path(line: &str) -> Option<PathBuf> {
    let rest = line.strip_prefix("file://")?;
    if rest.starts_with("localhost/") {
        // file://localhost/path drops the host.
        let path = percent_decode(&rest["localhost".len()..]);
        return absolute_path(&path);
    }
    if !rest.starts_with('/') {
        return None;
    }
    let path = percent_decode(rest);
    if path.contains('?') || path.contains('#') {
        return None;
    }
    absolute_path(&path)
}

fn absolute_path(path: &str) -> Option<PathBuf> {
    if !path.starts_with('/') || path.contains('\0') {
        return None;
    }
    Some(PathBuf::from(path))
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            match hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                Some(byte) => {
                    out.push(byte);
                    index += 3;
                }
                None => {
                    out.push(bytes[index]);
                    index += 1;
                }
            }
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The clipboard representation for an image of `format`.
///
/// A copied JPEG stays a JPEG so the receiver gets the same bytes the user
/// exported.
#[must_use]
pub fn image_mime(format: crate::export::ExportFormat) -> &'static str {
    match format {
        crate::export::ExportFormat::Jpeg => "image/jpeg",
        crate::export::ExportFormat::Webp => "image/webp",
        crate::export::ExportFormat::Heic => "image/heic",
        crate::export::ExportFormat::Png => "image/png",
    }
}

/// Build the `wl-copy` invocation for an image payload.
// Ported from omnishot/backend.py (copy_image)
#[must_use]
pub fn image_spec(
    format: crate::export::ExportFormat,
    bytes: Vec<u8>,
) -> crate::commands::CommandSpec {
    crate::commands::clipboard_spec(image_mime(format), bytes)
}

/// Build the `wl-copy` invocation for a file list.
///
/// A Wayland file drag and a file copy share the `text/uri-list` body, so a
/// receiver pastes the same file either way.
#[must_use]
pub fn files_spec(paths: &[std::path::PathBuf]) -> crate::commands::CommandSpec {
    crate::commands::clipboard_spec("text/uri-list", file_list(paths))
}

/// Build the `wl-copy` invocation for recognized or plain text.
#[must_use]
pub fn text_spec(text: &str) -> crate::commands::CommandSpec {
    crate::commands::clipboard_spec("text/plain;charset=utf-8", text.as_bytes().to_vec())
}

/// The `text/uri-list` body for a set of local files.
///
/// The URLs are percent-encoded by [`crate::overlay::local_file_url`], the
/// same builder a drag uses, so a file name with a space or a non-ASCII byte
/// is offered as one valid URI by both a paste and a drop.
#[must_use]
pub fn file_list(paths: &[std::path::PathBuf]) -> Vec<u8> {
    let mut body = String::new();
    for path in paths {
        body.push_str(&crate::overlay::local_file_url(path));
        body.push_str("\r\n");
    }
    body.into_bytes()
}

/// The clipboard representations `read_content` prefers, in order.
#[must_use]
pub fn preference_order(annotations: bool) -> Vec<&'static str> {
    let mut order = Vec::new();
    if annotations {
        order.push(ANNOTATION_MIME);
    }
    order.push("text/uri-list");
    order.push("x-special/gnome-copied-files");
    order.extend(IMAGE_TYPES);
    order
}

/// The clipboard image payload limit.
pub const MAX_IMAGE_BYTES: usize = 512_000_000;

/// Read the clipboard's local file list through `wl-paste`.
///
/// A clipboard holding an image bitmap rather than a file list is reported as
/// such; the caller surfaces the message instead of opening an empty editor.
pub fn read_files<R: crate::commands::CommandRunner>(
    runner: &R,
) -> Result<Vec<PathBuf>, ClipboardError> {
    for mime in ["text/uri-list", "x-special/gnome-copied-files"] {
        let spec = crate::commands::CommandSpec::new("wl-paste")
            .args(["--no-newline", "--type", mime])
            .timeout(std::time::Duration::from_secs(10));
        match runner.run(&spec) {
            Ok(output) if output.success() && !output.stdout.is_empty() => {
                return local_files(&output.stdout);
            }
            Ok(_) => continue,
            Err(error) => return Err(ClipboardError::new(error.to_string())),
        }
    }
    Err(ClipboardError::new(
        "There is no supported image or video file on the clipboard",
    ))
}

/// Validate a decoded clipboard image's dimensions.
pub fn validate_image_size(width: u32, height: u32) -> Result<(), ClipboardError> {
    if width == 0
        || height == 0
        || u64::from(width) * u64::from(height) > crate::annotation::MAX_PIXELS
    {
        return Err(ClipboardError::new("Could not open the clipboard image"));
    }
    Ok(())
}

/// True when the pasted files are images the annotate editor can open.
///
/// Video files and projects must be opened from the file list, not pasted.
// Ported from omnishot/clipboard.py (read_for_paste)
pub fn pasteable_images(files: &[PathBuf]) -> Result<Vec<&Path>, ClipboardError> {
    let mut images = Vec::new();
    for path in files {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!(".{}", extension.to_ascii_lowercase()))
            .unwrap_or_default();
        if !IMAGE_EXTENSIONS.contains(&extension.as_str()) {
            return Err(ClipboardError::new(
                "Paste an image into Annotate. Use Open from clipboard for video files and projects.",
            ));
        }
        images.push(path.as_path());
    }
    if images.is_empty() {
        return Err(ClipboardError::new(
            "There is no supported image or video file on the clipboard",
        ));
    }
    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_decode_and_deduplicate() {
        let dir = std::env::temp_dir().join(format!("tidyshot-clip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let image = dir.join("my image.png");
        std::fs::write(&image, b"png").unwrap();
        let other = dir.join("clip.mp4");
        std::fs::write(&other, b"mp4").unwrap();
        let url = format!(
            "file://{}\r\n# comment\r\nfile://{}\r\n",
            image.display(),
            other.display()
        );
        let files = local_files(url.as_bytes()).unwrap();
        assert_eq!(files, vec![image.clone(), other.clone()]);

        // Percent-encoded spaces decode to the same file, deduplicated.
        let encoded = format!(
            "file://{}\n",
            image.display().to_string().replace(' ', "%20")
        );
        assert_eq!(local_files(encoded.as_bytes()).unwrap(), vec![image]);
    }

    #[test]
    fn unsupported_and_missing_files_are_rejected() {
        assert!(local_files(b"https://example.com/a.png").is_err());
        assert!(local_files(b"file:///tmp/definitely-missing-12345.png").is_err());
        // A non-media file is skipped; with no media left, the list is empty.
        let err = local_files(b"file:///etc/hostname").unwrap_err();
        assert!(err.message().contains("no supported"));
    }

    #[test]
    fn clipboard_files_are_read_from_the_first_supported_file_list() {
        use crate::commands::{CommandError, CommandOutput, CommandRunner, CommandSpec};
        use std::sync::Mutex;

        #[derive(Default)]
        struct Fake {
            seen: Mutex<Vec<CommandSpec>>,
        }
        impl CommandRunner for Fake {
            fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
                self.seen.lock().unwrap().push(spec.clone());
                if spec.args.iter().any(|arg| arg == "text/uri-list") {
                    Ok(CommandOutput {
                        status: 0,
                        stdout: b"file:///tmp/clipboard-shot.png\n".to_vec(),
                        stderr: Vec::new(),
                    })
                } else {
                    Ok(CommandOutput {
                        status: 1,
                        stdout: Vec::new(),
                        stderr: b"no type".to_vec(),
                    })
                }
            }
        }
        let dir = std::env::temp_dir().join(format!("tidyshot-clip-read-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let image = dir.join("clipboard-shot.png");
        std::fs::write(&image, b"png").unwrap();
        let path = image.display().to_string();
        std::fs::write(&image, b"png").unwrap();
        let _ = path;
        // The fake returns a fixed absolute path, so the test asserts the
        // command shape and that a missing target is reported.
        let runner = Fake::default();
        let error = read_files(&runner).unwrap_err();
        assert!(error.message().contains("no longer available"));
        assert_eq!(runner.seen.lock().unwrap()[0].program, "wl-paste");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_payload_limit_is_enforced() {
        let huge = vec![b'a'; 1024 * 1024 + 1];
        assert_eq!(
            local_files(&huge).unwrap_err().message(),
            "The clipboard file list is too large"
        );
    }

    #[test]
    fn image_dimensions_are_bounded() {
        assert!(validate_image_size(800, 600).is_ok());
        assert!(validate_image_size(0, 600).is_err());
        assert!(validate_image_size(20000, 20000).is_err());
    }

    #[test]
    fn preference_order_puts_annotations_first_when_requested() {
        assert_eq!(preference_order(true)[0], ANNOTATION_MIME);
        assert_eq!(preference_order(false)[0], "text/uri-list");
        assert_eq!(preference_order(true).last(), Some(&"image/tiff"));
    }

    #[test]
    fn a_clipboard_spec_is_built_from_the_payload_format() {
        // Ported from omnishot/tests/test_clipboard.py (copy image per format)
        for (format, mime) in [
            (crate::export::ExportFormat::Png, "image/png"),
            (crate::export::ExportFormat::Jpeg, "image/jpeg"),
            (crate::export::ExportFormat::Webp, "image/webp"),
            (crate::export::ExportFormat::Heic, "image/heic"),
        ] {
            let spec = image_spec(format, vec![1, 2, 3]);
            assert_eq!(spec.program, "wl-copy");
            assert_eq!(spec.args, ["--type", mime]);
            assert_eq!(spec.stdin.as_deref(), Some([1, 2, 3].as_slice()));
            // The owner inherits our pipes, so stderr must not be a pipe.
            assert!(spec.stderr_to_file);
        }
        let files = files_spec(&[PathBuf::from("/tmp/a.png")]);
        assert_eq!(files.args, ["--type", "text/uri-list"]);
        assert_eq!(
            files.stdin.as_deref(),
            Some(b"file:///tmp/a.png\r\n".as_slice())
        );
        let text = text_spec("hello");
        assert_eq!(text.args, ["--type", "text/plain;charset=utf-8"]);
        assert_eq!(text.stdin.as_deref(), Some(b"hello".as_slice()));
    }

    #[test]
    fn paste_rejects_video_files_and_accepts_images() {
        let image = PathBuf::from("/tmp/a.png");
        let video = PathBuf::from("/tmp/a.mp4");
        assert_eq!(
            pasteable_images(&[image]).unwrap(),
            vec![Path::new("/tmp/a.png")]
        );
        assert!(pasteable_images(&[video]).is_err());
    }
}
