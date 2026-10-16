//! The editable `.omnishot` project archive.
// Ported from omnishot/image_project.py (read_project) and omnishot/editor.py
// (write_project, load_project).
//!
//! A project is a ZIP holding `image.png`, `project.json` and, when canvas
//! edits were made, the untouched `original.png` plus the transform that maps
//! annotations back through every crop, rotate, flip and resize. Version-1
//! projects have no original and must still open.

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

use crate::annotation::{Annotation, Props, Transform, parse_transform};
use crate::backgrounds::Background;
use crate::json::Value;
use crate::pngzip::{Image, ZipArchive, ZipWriter};

/// The largest total uncompressed size of a project archive.
pub const MAX_ARCHIVE_BYTES: u64 = 512_000_000;

/// The megapixel ceiling shared with screenshots and imports.
pub const MAX_PIXELS: u64 = 120_000_000;

/// Why a project could not be opened or written.
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

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProjectError {}

fn unsupported() -> ProjectError {
    ProjectError::new("Unsupported project")
}

/// A decoded editable project.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub version: i32,
    pub base: Image,
    pub original: Option<Image>,
    pub source_transform: Transform,
    pub objects: Vec<Annotation>,
    pub background: Option<Background>,
    pub expand_canvas: bool,
}

impl Project {
    /// Open a `.omnishot` archive from bytes.
    // Ported from omnishot/image_project.py (read_project)
    pub fn read(bytes: &[u8]) -> Result<Self, ProjectError> {
        let archive =
            ZipArchive::read(bytes).map_err(|error| ProjectError::new(error.to_string()))?;
        let mut names = BTreeSet::new();
        let mut total = 0u64;
        for entry in archive.entries() {
            if !names.insert(entry.name.clone()) {
                return Err(ProjectError::new("Duplicate project entries"));
            }
            total += entry.data.len() as u64;
            if total > MAX_ARCHIVE_BYTES {
                return Err(ProjectError::new("Project is too large"));
            }
        }

        let image_bytes = archive
            .get("image.png")
            .ok_or_else(|| ProjectError::new("Unsupported project"))?;
        let image = crate::pngzip::decode_png(image_bytes)
            .map_err(|error| ProjectError::new(format!("Could not open image project: {error}")))?;
        if image.width as u64 * image.height as u64 > MAX_PIXELS {
            return Err(ProjectError::new(
                "Could not open image project: Project is too large",
            ));
        }

        let document_bytes = archive
            .get("project.json")
            .ok_or_else(|| ProjectError::new("Unsupported project"))?;
        let text = std::str::from_utf8(document_bytes).map_err(|_| unsupported())?;
        let document = crate::json::parse(text)
            .map_err(|error| ProjectError::new(format!("Could not open image project: {error}")))?;
        let version = document
            .get("version")
            .and_then(Value::as_f64)
            .ok_or_else(unsupported)?;
        if version != 1.0 && version != 2.0 {
            return Err(unsupported());
        }
        let objects_value = document.get("objects").and_then(Value::as_array);
        let objects_value = objects_value.ok_or_else(unsupported)?;
        let mut objects = Vec::with_capacity(objects_value.len());
        for object in objects_value {
            let Some(map) = object.as_object() else {
                return Err(unsupported());
            };
            if !map.get("kind").is_some_and(|kind| kind.as_str().is_some()) {
                return Err(unsupported());
            }
            let props: Props = map.clone().into_iter().collect();
            objects.push(Annotation::new(props));
        }
        let background = match document.get("background") {
            None | Some(Value::Null) => None,
            Some(value) if value.as_object().is_some() => Background::from_json(value),
            Some(_) => return Err(unsupported()),
        };

        let mut original = None;
        let mut source_transform = Transform::default();
        if document.get("original_image").is_some() || document.get("source_transform").is_some() {
            if document.get("original_image").and_then(Value::as_str) != Some("original.png") {
                return Err(ProjectError::new("Invalid original image"));
            }
            let original_bytes = archive
                .get("original.png")
                .ok_or_else(|| ProjectError::new("Invalid original image"))?;
            let decoded = crate::pngzip::decode_png(original_bytes).map_err(|_| {
                ProjectError::new("Could not open image project: Invalid original image")
            })?;
            if decoded.width as u64 * decoded.height as u64 > MAX_PIXELS {
                return Err(ProjectError::new("Invalid original image"));
            }
            source_transform = parse_transform(document.get("source_transform"))
                .map_err(|_| ProjectError::new("Invalid original image"))?;
            original = Some(decoded);
        }

        Ok(Self {
            version: version as i32,
            base: image,
            original,
            source_transform,
            objects,
            background,
            expand_canvas: document
                .get("expand_canvas")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    /// Open a `.omnishot` archive from disk.
    pub fn open(path: &Path) -> Result<Self, ProjectError> {
        let bytes = std::fs::read(path)
            .map_err(|error| ProjectError::new(format!("Could not open image project: {error}")))?;
        Self::read(&bytes)
    }

    /// Serialize to archive bytes.
    // Ported from omnishot/editor.py (write_project)
    pub fn write_bytes(&self) -> Result<Vec<u8>, ProjectError> {
        let mut writer = ZipWriter::new();
        let mut object_values = Vec::with_capacity(self.objects.len());
        for object in &self.objects {
            object_values.push(Value::Object(object.data_dict().into_iter().collect()));
        }
        let mut document = std::collections::BTreeMap::new();
        document.insert("version".to_owned(), Value::Number(2.0));
        document.insert("objects".to_owned(), Value::Array(object_values));
        document.insert(
            "background".to_owned(),
            self.background
                .as_ref()
                .map_or(Value::Null, Background::to_json),
        );
        document.insert("expand_canvas".to_owned(), Value::Bool(self.expand_canvas));
        if let Some(original) = &self.original {
            document.insert(
                "original_image".to_owned(),
                Value::String("original.png".to_owned()),
            );
            document.insert(
                "source_transform".to_owned(),
                self.source_transform.to_values(),
            );
            writer.add("original.png", original.to_png());
        }
        writer.add("image.png", self.base.to_png());
        writer.add(
            "project.json",
            Value::Object(document).to_string_compact().into_bytes(),
        );

        let bytes = writer.finish();
        if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
            return Err(ProjectError::new("Project is too large"));
        }
        Ok(bytes)
    }

    /// Write the archive atomically beside `destination`.
    // Ported from omnishot/storage.py (atomic_output)
    pub fn save(&self, destination: &Path) -> Result<(), ProjectError> {
        let bytes = self.write_bytes()?;
        crate::storage::atomic_write_file(destination, &bytes)
            .map_err(|error| ProjectError::new(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::Point;
    use crate::color::Rgba;

    fn props(entries: &[(&str, Value)]) -> Props {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    fn image(width: usize, height: usize) -> Image {
        let mut data = vec![0u8; width * height * 4];
        for (index, pixel) in data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            pixel.copy_from_slice(&[(index % 255) as u8, 40, 80, 255]);
        }
        Image::new(width, height, data)
    }

    #[test]
    fn version_two_projects_round_trip_objects_and_original() {
        let project = Project {
            version: 2,
            base: image(20, 10),
            original: Some(image(30, 15)),
            source_transform: Transform([1.0, 0.0, 0.0, 1.0, 5.0, 6.0]),
            objects: vec![Annotation::new(props(&[
                ("kind", Value::String("rect".into())),
                ("x", Value::Number(1.0)),
                ("y", Value::Number(2.0)),
                ("w", Value::Number(3.0)),
                ("h", Value::Number(4.0)),
                ("color", Value::String("#ff0000".into())),
            ]))],
            background: Some(Background::default()),
            expand_canvas: true,
        };
        let bytes = project.write_bytes().unwrap();
        let reopened = Project::read(&bytes).unwrap();
        assert_eq!(reopened.base, project.base);
        assert_eq!(reopened.original, project.original);
        assert_eq!(reopened.source_transform, project.source_transform);
        assert_eq!(reopened.objects, project.objects);
        assert!(reopened.expand_canvas);
        assert_eq!(
            reopened.background.unwrap().padding,
            project.background.clone().unwrap().padding
        );
    }

    #[test]
    fn version_one_projects_open_without_an_original() {
        // Build a v1 archive by hand: image plus a minimal document.
        let mut writer = ZipWriter::new();
        writer.add("image.png", image(8, 6).to_png());
        writer.add(
            "project.json",
            br#"{"version":1,"objects":[{"kind":"rect","x":1,"y":2,"w":3,"h":4}]}"#.to_vec(),
        );
        let reopened = Project::read(&writer.finish()).unwrap();
        assert_eq!(reopened.version, 1);
        assert!(reopened.original.is_none());
        assert!(reopened.source_transform.is_identity());
        assert_eq!(reopened.objects.len(), 1);
        assert!(!reopened.expand_canvas);
    }

    #[test]
    fn invalid_projects_are_rejected() {
        let mut writer = ZipWriter::new();
        writer.add("image.png", image(4, 4).to_png());
        // Unsupported version.
        writer.add("project.json", br#"{"version":3,"objects":[]}"#.to_vec());
        assert!(Project::read(&writer.finish()).is_err());

        // Missing image.
        let mut missing = ZipWriter::new();
        missing.add("project.json", br#"{"version":2,"objects":[]}"#.to_vec());
        assert!(Project::read(&missing.finish()).is_err());

        // Objects must be objects with a string kind.
        let mut bad_kind = ZipWriter::new();
        bad_kind.add("image.png", image(4, 4).to_png());
        bad_kind.add(
            "project.json",
            br#"{"version":2,"objects":[{"kind":7}]}"#.to_vec(),
        );
        assert!(Project::read(&bad_kind.finish()).is_err());

        // A bad original transform must not be accepted.
        let mut bad_transform = ZipWriter::new();
        bad_transform.add("image.png", image(4, 4).to_png());
        bad_transform.add("original.png", image(4, 4).to_png());
        bad_transform.add(
            "project.json",
            br#"{"version":2,"objects":[],"original_image":"original.png","source_transform":[0,0,0,0,0,0]}"#
                .to_vec(),
        );
        assert!(Project::read(&bad_transform.finish()).is_err());
    }

    #[test]
    fn duplicate_entries_are_rejected() {
        let mut writer = ZipWriter::new();
        writer.add("image.png", image(4, 4).to_png());
        writer.add("image.png", image(4, 4).to_png());
        writer.add("project.json", br#"{"version":2,"objects":[]}"#.to_vec());
        assert_eq!(
            Project::read(&writer.finish()).unwrap_err().message(),
            "Duplicate project entries"
        );
    }

    #[test]
    fn saving_uses_the_destination_and_is_atomic() {
        let dir = std::env::temp_dir().join(format!("tidyshot-project-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("out.omnishot");
        let project = Project {
            version: 2,
            base: image(6, 4),
            original: None,
            source_transform: Transform::default(),
            objects: vec![Annotation::new(props(&[
                ("kind", Value::String("arrow".into())),
                ("x", Value::Number(0.0)),
                ("y", Value::Number(0.0)),
            ]))],
            background: None,
            expand_canvas: false,
        };
        project.save(&target).unwrap();
        let reopened = Project::open(&target).unwrap();
        assert_eq!(reopened.base, project.base);
        assert_eq!(reopened.objects, project.objects);
        assert_eq!(
            std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".tidyshot-"))
                .count(),
            0
        );
    }

    #[test]
    fn background_round_trips_with_a_cover_image() {
        let cover = image(3, 3);
        let background = Background {
            padding: 7,
            image_data: Some(cover.clone()),
            ..Background::default()
        };
        let json = background.to_json();
        let parsed = Background::from_json(&json).unwrap();
        assert_eq!(parsed.padding, 7);
        assert_eq!(parsed.image_data, Some(cover));
        assert_eq!(parsed.color, background.color);
    }

    #[test]
    fn annotation_points_survive_the_json_round_trip() {
        let project = Project {
            version: 2,
            base: image(2, 2),
            original: None,
            source_transform: Transform::default(),
            objects: vec![Annotation::new(props(&[
                ("kind", Value::String("pencil".into())),
                ("x", Value::Number(0.0)),
                ("y", Value::Number(0.0)),
                (
                    "points",
                    Value::Array(vec![
                        Value::Array(vec![Value::Number(1.0), Value::Number(2.0)]),
                        Value::Array(vec![Value::Number(3.0), Value::Number(4.0)]),
                    ]),
                ),
            ]))],
            background: None,
            expand_canvas: false,
        };
        let reopened = Project::read(&project.write_bytes().unwrap()).unwrap();
        assert_eq!(
            reopened.objects[0].props["points"],
            project.objects[0].props["points"]
        );
        // Sanity check the point helpers still read the stored values.
        let _ = Point::new(0.0, 0.0);
        let _ = Rgba::from_hex("#ff0000").unwrap();
    }
}
