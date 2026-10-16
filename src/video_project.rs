//! Portable local video projects, with bounded extraction and atomic saves.
// Ported from omnishot/video_project.py
//!
//! A project is a ZIP archive holding a `manifest.json` and the source (and
//! optional camera) track. The manifest is validated strictly before anything
//! is extracted, so a malicious archive cannot write outside its folder or
//! exhaust memory. Saving writes to a sibling temp file and renames it, so a
//! failed save never replaces the destination.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::json::Value;

use super::pngzip::{ImageError, ZipArchive, ZipWriter};

/// The project extension.
pub const EXTENSION: &str = ".omnishot-video";
/// The largest media payload a project may carry (50 GiB).
pub const MAX_MEDIA_BYTES: u64 = 50 * 1024 * 1024 * 1024;
/// The largest manifest a project may carry (20 MiB).
pub const MAX_MANIFEST_BYTES: u64 = 20 * 1024 * 1024;

/// A project that failed validation or archive reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectError {
    message: String,
}

impl ProjectError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The message the UI shows.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProjectError {}

impl From<ImageError> for ProjectError {
    fn from(error: ImageError) -> Self {
        Self::new(error.to_string())
    }
}

/// A read-and-validated project.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoProject {
    /// The extracted source path.
    pub source: PathBuf,
    /// The extracted camera path, when the project has one.
    pub camera: Option<PathBuf>,
    /// The manifest's metadata object.
    pub metadata: Value,
    /// The manifest's options object.
    pub options: BTreeMap<String, Value>,
    /// The archive's manifest bytes.
    pub manifest: Vec<u8>,
}

/// Validate a manifest, returning the source and camera track names.
// Ported from omnishot/video_project.py (validate_manifest)
pub fn validate_manifest(manifest: &Value) -> Result<(String, Option<String>), ProjectError> {
    let object = manifest
        .as_object()
        .ok_or_else(|| ProjectError::new("Unsupported OmniShot video project"))?;
    if object.get("format").and_then(Value::as_str) != Some("omnishot-video")
        || object.get("version").and_then(Value::as_f64) != Some(1.0)
    {
        return Err(ProjectError::new("Unsupported OmniShot video project"));
    }
    let source = object.get("source").and_then(Value::as_str).unwrap_or("");
    if !valid_source_name(source) {
        return Err(ProjectError::new("Invalid source track"));
    }
    let camera = object
        .get("camera")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(camera) = &camera
        && camera != "camera.mp4"
    {
        return Err(ProjectError::new("Invalid camera track"));
    }
    let metadata = object.get("metadata").unwrap_or(&Value::Null);
    let options = object.get("options").unwrap_or(&Value::Null);
    if metadata.as_object().is_none() || options.as_object().is_none() {
        return Err(ProjectError::new("Invalid project settings"));
    }
    let options = options.as_object().unwrap();

    if let Some(blur) = options.get("motion_blur")
        && !matches!(blur, Value::Number(n) if n.fract() == 0.0 && (0.0..=3.0).contains(n))
    {
        return Err(ProjectError::new("Invalid motion blur intensity"));
    }
    if let Some(value) = options.get("key_commands_only")
        && value.as_bool().is_none()
    {
        return Err(ProjectError::new("Invalid keystroke filter"));
    }
    if let Some(value) = options.get("key_style").and_then(Value::as_str)
        && !matches!(value, "Dark" | "Light" | "Custom")
    {
        return Err(ProjectError::new("Invalid keystroke style"));
    }

    let metadata_object = metadata.as_object().unwrap();
    let framing = metadata_object
        .get("camera_framing")
        .and_then(Value::as_array)
        .map(<[Value]>::to_vec)
        .unwrap_or_default();
    if framing.len() > 100_000 {
        return Err(ProjectError::new("Invalid camera framing changes"));
    }
    let mut previous = -1.0_f64;
    for event in &framing {
        let Some(event) = event.as_object() else {
            return Err(ProjectError::new("Invalid camera framing changes"));
        };
        let Some(t) = finite_number(event.get("t")) else {
            return Err(ProjectError::new("Invalid camera framing changes"));
        };
        if t < previous || t < 0.0 || event.get("fullscreen").and_then(Value::as_bool).is_none() {
            return Err(ProjectError::new("Invalid camera framing changes"));
        }
        if let Some(shape) = event.get("shape").and_then(Value::as_str)
            && !matches!(shape, "Circle" | "Square" | "Rounded" | "Rectangle")
        {
            return Err(ProjectError::new("Invalid camera framing shape"));
        }
        if let Some(placement) = event.get("placement") {
            let Some(values) = placement.as_array() else {
                return Err(ProjectError::new("Invalid camera framing placement"));
            };
            if values.len() != 3
                || values.iter().any(|v| finite_number_bool(v).is_none())
                || !(0.001..=1.0).contains(&values[2].as_f64().unwrap_or(-1.0))
                || values[..2]
                    .iter()
                    .any(|v| !(-1.0..=1.0).contains(&v.as_f64().unwrap_or(2.0)))
            {
                return Err(ProjectError::new("Invalid camera framing placement"));
            }
        }
        previous = t;
    }
    if let Some(value) = options.get("camera_recorded_framing")
        && value.as_bool().is_none()
    {
        return Err(ProjectError::new("Invalid camera framing preference"));
    }

    for key in ["cursor", "events"] {
        let rows = metadata_object
            .get(key)
            .and_then(Value::as_array)
            .map(<[Value]>::to_vec)
            .unwrap_or_default();
        if rows.len() > 2_000_000 {
            return Err(ProjectError::new("Invalid input track"));
        }
        for row in &rows {
            let Some(row) = row.as_object() else {
                return Err(ProjectError::new("Invalid input timestamp"));
            };
            if row.get("t").and_then(Value::as_f64).is_none() {
                return Err(ProjectError::new("Invalid input timestamp"));
            }
            if (key == "cursor" || row.get("kind").and_then(Value::as_str) == Some("click"))
                && !all_numbers(row, &["x", "y"])
            {
                return Err(ProjectError::new("Invalid cursor position"));
            }
            if row.get("kind").and_then(Value::as_str) == Some("key") {
                let label = row.get("label").and_then(Value::as_str).unwrap_or("");
                if label.len() > 4096 {
                    return Err(ProjectError::new("Invalid keystroke label"));
                }
            }
            if row.get("kind").and_then(Value::as_str) == Some("key")
                && row.get("command").is_some()
                && row.get("command").and_then(Value::as_bool).is_none()
            {
                return Err(ProjectError::new("Invalid command classification"));
            }
        }
    }

    let zooms = options
        .get("zooms")
        .and_then(Value::as_array)
        .map(<[Value]>::to_vec)
        .unwrap_or_default();
    for zoom in &zooms {
        let Some(zoom) = zoom.as_object() else {
            return Err(ProjectError::new("Invalid zoom region"));
        };
        if !all_numbers(zoom, &["start", "end", "scale"]) {
            return Err(ProjectError::new("Invalid zoom region"));
        }
        let (start, end, scale) = (
            zoom["start"].as_f64().unwrap(),
            zoom["end"].as_f64().unwrap(),
            zoom["scale"].as_f64().unwrap(),
        );
        if start < 0.0 || end <= start || !(1.0..=5.0).contains(&scale) {
            return Err(ProjectError::new("Invalid zoom region"));
        }
        for axis in ["x", "y"] {
            let value = zoom.get(axis).and_then(Value::as_f64).unwrap_or(0.5);
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(ProjectError::new("Invalid zoom focus"));
            }
        }
    }

    let cuts = options
        .get("cuts")
        .and_then(Value::as_array)
        .map(<[Value]>::to_vec)
        .unwrap_or_default();
    for cut in &cuts {
        let Some(values) = cut.as_array() else {
            return Err(ProjectError::new("Invalid cut region"));
        };
        if values.len() != 2 || values.iter().any(|v| finite_number_bool(v).is_none()) {
            return Err(ProjectError::new("Invalid cut region"));
        }
        let (a, b) = (values[0].as_f64().unwrap(), values[1].as_f64().unwrap());
        if !(0.0..b).contains(&a) || a >= b {
            return Err(ProjectError::new("Invalid cut region"));
        }
    }

    let splits = options
        .get("splits")
        .and_then(Value::as_array)
        .map(<[Value]>::to_vec)
        .unwrap_or_default();
    if !splits
        .iter()
        .all(|value| matches!(value, Value::Number(n) if n.is_finite() && *n > 0.0 && *n < 86400.0))
    {
        return Err(ProjectError::new("Invalid clip boundary"));
    }

    if let Some(tracks) = options.get("audio_tracks") {
        let Some(rows) = tracks.as_array() else {
            return Err(ProjectError::new("Invalid audio edits"));
        };
        if rows.len() > 256 {
            return Err(ProjectError::new("Invalid audio edits"));
        }
        for track in rows {
            let Some(track) = track.as_object() else {
                return Err(ProjectError::new("Invalid audio edits"));
            };
            if track.get("enabled").is_some_and(|v| v.as_bool().is_none()) {
                return Err(ProjectError::new("Invalid audio edits"));
            }
            let volume = track.get("volume").and_then(Value::as_f64).unwrap_or(100.0);
            if !volume.is_finite() || !(0.0..=200.0).contains(&volume) {
                return Err(ProjectError::new("Invalid audio edits"));
            }
        }
    }

    let edits = options.get("event_edits");
    if let Some(edits) = edits {
        let Some(edits) = edits.as_object() else {
            return Err(ProjectError::new("Invalid input edits"));
        };
        let events = metadata_object
            .get("events")
            .and_then(Value::as_array)
            .map(<[Value]>::len)
            .unwrap_or(0);
        for (key, edit) in edits {
            let index: usize = key
                .parse()
                .map_err(|_| ProjectError::new("Invalid input edit"))?;
            if index >= events {
                return Err(ProjectError::new("Invalid input edit"));
            }
            let Some(edit) = edit.as_object() else {
                return Err(ProjectError::new("Invalid input edit"));
            };
            if let Some(t) = edit.get("t")
                && (finite_number_bool(t).is_none() || t.as_f64().unwrap_or(-1.0) < 0.0)
            {
                return Err(ProjectError::new("Invalid edited timestamp"));
            }
            if let Some(label) = edit.get("label")
                && label.as_str().map(str::len).unwrap_or(usize::MAX) > 4096
            {
                return Err(ProjectError::new("Invalid edited label"));
            }
            if let Some(hidden) = edit.get("hidden")
                && hidden.as_bool().is_none()
            {
                return Err(ProjectError::new("Invalid event visibility"));
            }
        }
    }
    Ok((source.to_owned(), camera))
}

fn valid_source_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("source.") else {
        return false;
    };
    matches!(rest, "mp4" | "mov" | "mkv" | "webm" | "gif")
}

fn finite_number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|n| n.is_finite())
}

fn finite_number_bool(value: &Value) -> Option<f64> {
    if matches!(value, Value::Bool(_)) {
        return None;
    }
    value.as_f64().filter(|n| n.is_finite())
}

fn all_numbers(object: &BTreeMap<String, Value>, keys: &[&str]) -> bool {
    if keys.is_empty() {
        return true;
    }
    keys.iter().all(|key| {
        object
            .get(*key)
            .is_some_and(|v| finite_number_bool(v).is_some())
    })
}

/// Serialize a project archive's manifest and media entries.
///
/// Only the manifest half of [`write_project`] is modeled; the caller supplies
/// the source bytes so the archive stays a pure function and tests do not touch
/// the filesystem.
pub fn build_archive(
    source_name: &str,
    source_bytes: &[u8],
    camera: Option<(&str, &[u8])>,
    metadata: &Value,
    options: &BTreeMap<String, Value>,
) -> Result<Vec<u8>, ProjectError> {
    let manifest = manifest_value(source_name, camera.map(|c| c.0), metadata, options);
    validate_manifest(&manifest)?;
    let manifest_bytes = manifest.to_string_compact().into_bytes();
    if manifest_bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(ProjectError::new("Project metadata is too large"));
    }
    let total = source_bytes.len() as u64 + camera.map_or(0, |(_, bytes)| bytes.len() as u64);
    if total > MAX_MEDIA_BYTES {
        return Err(ProjectError::new("Project media exceeds the 50 GB limit"));
    }
    let mut writer = ZipWriter::new();
    writer.add("manifest.json", manifest_bytes);
    writer.add(source_name.to_owned(), source_bytes.to_vec());
    if let Some((name, bytes)) = camera {
        writer.add(name.to_owned(), bytes.to_vec());
    }
    Ok(writer.finish())
}

fn manifest_value(
    source_name: &str,
    camera_name: Option<&str>,
    metadata: &Value,
    options: &BTreeMap<String, Value>,
) -> Value {
    let mut metadata = metadata.as_object().cloned().unwrap_or_default();
    metadata.remove("source_path");
    metadata.remove("camera_path");
    Value::Object(BTreeMap::from([
        (
            "format".to_owned(),
            Value::String("omnishot-video".to_owned()),
        ),
        ("version".to_owned(), Value::Number(1.0)),
        ("source".to_owned(), Value::String(source_name.to_owned())),
        (
            "camera".to_owned(),
            camera_name.map_or(Value::Null, |name| Value::String(name.to_owned())),
        ),
        ("metadata".to_owned(), Value::Object(metadata)),
        ("options".to_owned(), Value::Object(options.clone())),
    ]))
}

/// Validate and read a project archive's manifest and media entries.
///
/// The archive's entries are checked against the manifest before any extraction
/// happens; this function never writes to disk.
pub fn read_archive(bytes: &[u8]) -> Result<VideoProject, ProjectError> {
    // Check the declared entry sizes before inflating anything, so a crafted
    // archive cannot force unbounded memory use ahead of the limits.
    let infos =
        ZipArchive::entry_infos(bytes).map_err(|error| ProjectError::new(error.to_string()))?;
    let names: Vec<&str> = infos
        .entries()
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    let unique: std::collections::BTreeSet<&str> = names.iter().copied().collect();
    if unique.len() != names.len() || infos.entries().len() > 3 {
        return Err(ProjectError::new("Unexpected project entries"));
    }
    let total: u64 = infos
        .entries()
        .iter()
        .map(|entry| entry.uncompressed_size as u64)
        .sum();
    if total > MAX_MEDIA_BYTES + MAX_MANIFEST_BYTES {
        return Err(ProjectError::new("Project media exceeds the 50 GB limit"));
    }
    let manifest_info = infos
        .entries()
        .iter()
        .find(|entry| entry.name == "manifest.json")
        .ok_or_else(|| ProjectError::new("Unexpected or missing project tracks"))?;
    if manifest_info.uncompressed_size as u64 > MAX_MANIFEST_BYTES {
        return Err(ProjectError::new("Project metadata is too large"));
    }
    let archive = ZipArchive::read(bytes)?;
    let manifest_bytes = archive
        .get("manifest.json")
        .ok_or_else(|| ProjectError::new("Unexpected or missing project tracks"))?;
    let manifest: Value = crate::json::parse(
        std::str::from_utf8(manifest_bytes).map_err(|_| ProjectError::new("Invalid manifest"))?,
    )
    .map_err(|_| ProjectError::new("Invalid manifest"))?;
    let (source, camera) = validate_manifest(&manifest)?;
    let mut expected: std::collections::BTreeSet<&str> =
        ["manifest.json", source.as_str()].into_iter().collect();
    if let Some(camera) = &camera {
        expected.insert(camera);
    }
    if unique != expected {
        return Err(ProjectError::new("Unexpected or missing project tracks"));
    }
    let options = manifest
        .get("options")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let metadata = manifest
        .get("metadata")
        .cloned()
        .unwrap_or(Value::Object(BTreeMap::new()));
    Ok(VideoProject {
        source: PathBuf::from(source),
        camera: camera.map(PathBuf::from),
        metadata,
        options,
        manifest: manifest_bytes.to_vec(),
    })
}

/// Save a project to `destination` atomically, cleaning up on failure.
pub fn write_archive(
    destination: &Path,
    source_name: &str,
    source_bytes: &[u8],
    camera: Option<(&str, &[u8])>,
    metadata: &Value,
    options: &BTreeMap<String, Value>,
) -> Result<(), ProjectError> {
    let bytes = build_archive(source_name, source_bytes, camera, metadata, options)?;
    crate::storage::atomic_write_file(destination, &bytes)
        .map_err(|error| ProjectError::new(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(source: &str, options: BTreeMap<String, Value>) -> Value {
        Value::Object(BTreeMap::from([
            (
                "format".to_owned(),
                Value::String("omnishot-video".to_owned()),
            ),
            ("version".to_owned(), Value::Number(1.0)),
            ("source".to_owned(), Value::String(source.to_owned())),
            ("metadata".to_owned(), Value::Object(BTreeMap::new())),
            ("options".to_owned(), Value::Object(options)),
        ]))
    }

    #[test]
    fn manifest_requires_the_format_version_and_source_name() {
        assert!(validate_manifest(&manifest("source.mp4", BTreeMap::new())).is_ok());
        for bad in ["../outside.mp4", "raw.mp4", "source.txt", ""] {
            assert!(
                validate_manifest(&manifest(bad, BTreeMap::new())).is_err(),
                "{bad}"
            );
        }
        let mut wrong = manifest("source.mp4", BTreeMap::new());
        wrong
            .as_object_mut()
            .unwrap()
            .insert("version".to_owned(), Value::Number(2.0));
        assert!(validate_manifest(&wrong).is_err());
    }

    #[test]
    fn split_boundaries_reject_booleans_and_out_of_range() {
        let base = manifest("source.mp4", BTreeMap::new());
        for invalid in [
            Value::Bool(true),
            Value::Number(f64::NAN),
            Value::Number(-1.0),
            Value::String("3".to_owned()),
        ] {
            let mut candidate = base.clone();
            candidate
                .as_object_mut()
                .unwrap()
                .get_mut("options")
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("splits".to_owned(), Value::Array(vec![invalid.clone()]));
            assert!(
                validate_manifest(&candidate).is_err(),
                "accepted {invalid:?}"
            );
        }
        let mut valid = base;
        valid
            .as_object_mut()
            .unwrap()
            .get_mut("options")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(
                "splits".to_owned(),
                Value::Array(vec![Value::Number(2.0), Value::Number(3.0)]),
            );
        assert!(validate_manifest(&valid).is_ok());
    }

    #[test]
    fn motion_blur_and_keystroke_options_are_range_checked() {
        let mut base = manifest("source.mp4", BTreeMap::new());
        for invalid in [
            Value::Number(-1.0),
            Value::Number(4.0),
            Value::Bool(true),
            Value::String("Medium".to_owned()),
            Value::Number(f64::NAN),
        ] {
            base.as_object_mut()
                .unwrap()
                .get_mut("options")
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("motion_blur".to_owned(), invalid.clone());
            assert!(validate_manifest(&base).is_err(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn an_archive_round_trips_and_rejects_unexpected_tracks() {
        let metadata = Value::Object(BTreeMap::new());
        let options = BTreeMap::new();
        let bytes = build_archive("source.mp4", b"media", None, &metadata, &options).unwrap();
        let project = read_archive(&bytes).unwrap();
        assert_eq!(project.source, PathBuf::from("source.mp4"));
        assert!(project.camera.is_none());

        let mut writer = ZipWriter::new();
        writer.add("manifest.json", {
            let mut candidate = manifest("source.mp4", BTreeMap::new());
            candidate
                .as_object_mut()
                .unwrap()
                .insert("source".to_owned(), Value::String("source.mp4".to_owned()));
            candidate.to_string_compact().into_bytes()
        });
        writer.add("../outside.mp4", b"data".to_vec());
        assert!(read_archive(&writer.finish()).is_err());
    }

    #[test]
    fn a_camera_track_must_be_named_camera_mp4() {
        let metadata = Value::Object(BTreeMap::new());
        let options = BTreeMap::new();
        let bytes = build_archive(
            "source.mp4",
            b"media",
            Some(("camera.mp4", b"cam")),
            &metadata,
            &options,
        )
        .unwrap();
        let project = read_archive(&bytes).unwrap();
        assert_eq!(project.camera, Some(PathBuf::from("camera.mp4")));
        let mut candidate = manifest("source.mp4", BTreeMap::new());
        candidate
            .as_object_mut()
            .unwrap()
            .insert("camera".to_owned(), Value::String("other.mp4".to_owned()));
        assert!(validate_manifest(&candidate).is_err());
    }
}
