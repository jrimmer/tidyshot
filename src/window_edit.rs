//! Editable window frames: the composite, its project and its preview.
// Ported from omnishot/backend.py (image_edit_path, display_image) and
// omnishot/app.py (capture_window, finished_capture).
//!
//! A window capture keeps the raw, isolated window as the history capture and
//! composes the shadow, wallpaper and rounded frame into an editable project
//! beside it. The project holds the untouched window pixels and the background
//! options, so reopening it in the annotator restores every editable control
//! instead of a flattened picture. The preview PNG is a derived cache: losing
//! it never loses an edit.

use std::path::{Path, PathBuf};

use crate::backgrounds::Background;
use crate::pngzip::Image;
use crate::project::Project;

/// The folder holding editable window frames, beside the capture store.
#[must_use]
pub fn edits_dir(root: &Path) -> PathBuf {
    root.join("image-edits")
}

/// The stable identity for a capture path.
///
/// A 64-bit FNV-1a digest keeps the project name short and stable without a
/// hashing dependency; two different captures sharing a name would have to
/// collide in the capture store itself first.
// Ported from omnishot/backend.py (image_edit_path)
#[must_use]
pub fn edit_key(capture: &Path) -> String {
    let resolved = std::fs::canonicalize(capture).unwrap_or_else(|_| capture.to_path_buf());
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in resolved.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The project and preview paths for a capture.
#[must_use]
pub fn edit_paths(root: &Path, capture: &Path) -> (PathBuf, PathBuf) {
    let key = edit_key(capture);
    let directory = edits_dir(root);
    (
        directory.join(format!("{key}.omnishot")),
        directory.join(format!("{key}.png")),
    )
}

/// The picture a preview should show for a capture.
///
/// A window capture shows its composed preview when one exists; every other
/// capture shows the file itself.
// Ported from omnishot/backend.py (display_image)
#[must_use]
pub fn display_path(root: &Path, capture: &Path) -> PathBuf {
    let (_, preview) = edit_paths(root, capture);
    if preview.is_file() {
        return preview;
    }
    capture.to_path_buf()
}

/// Compose the editable project for a window frame.
#[must_use]
pub fn project_for(base: Image, background: Option<Background>) -> Project {
    Project {
        version: 2,
        base,
        original: None,
        source_transform: crate::annotation::Transform::default(),
        objects: Vec::new(),
        background,
        expand_canvas: false,
    }
}

/// Render the project's export image.
#[must_use]
pub fn render(project: &Project) -> Image {
    match &project.background {
        Some(background) => crate::backgrounds::render(background, &project.base),
        None => project.base.clone(),
    }
}

/// Write a window's editable project and preview beside the capture.
///
/// The project is written atomically first: the preview is derived, so a
/// crash between the two leaves an editable capture with a stale preview
/// rather than a saved picture with no edits. The capture file itself is
/// never touched.
pub fn save_editable(
    root: &Path,
    capture: &Path,
    project: &Project,
) -> Result<PathBuf, crate::project::ProjectError> {
    let (project_path, preview_path) = edit_paths(root, capture);
    if let Some(parent) = project_path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            crate::project::ProjectError::new(format!(
                "Could not write the window project: {error}"
            ))
        })?;
    }
    project.save(&project_path)?;
    let preview = render(project).to_png();
    crate::storage::atomic_write_file(&preview_path, &preview).map_err(|error| {
        crate::project::ProjectError::new(format!("Could not write the window preview: {error}"))
    })?;
    Ok(preview_path)
}

/// Reopen a capture's editable project, when it has one.
pub fn load(root: &Path, capture: &Path) -> Result<Option<Project>, crate::project::ProjectError> {
    let (project_path, _) = edit_paths(root, capture);
    if !project_path.is_file() {
        return Ok(None);
    }
    Project::open(&project_path).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tidyshot-window-edit-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn image(width: usize, height: usize, color: [u8; 4]) -> Image {
        let mut data = vec![0u8; width * height * 4];
        for pixel in data.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&color);
        }
        Image::new(width, height, data)
    }

    #[test]
    fn a_window_project_round_trips_with_its_background() {
        let root = scratch("round-trip");
        let capture = root.join("captures/window.png");
        std::fs::create_dir_all(capture.parent().unwrap()).unwrap();
        std::fs::write(&capture, image(4, 3, [10, 20, 30, 255]).to_png()).unwrap();
        let background = Background {
            color: Rgba::new(0, 0, 0, 0),
            color2: Rgba::new(0, 0, 0, 0),
            padding: 6,
            radius: 4.0,
            radius_power: 3.0,
            shadow: true,
            ..Background::default()
        };
        let project = project_for(image(4, 3, [10, 20, 30, 255]), Some(background.clone()));
        let preview_path = save_editable(&root, &capture, &project).unwrap();
        assert!(preview_path.is_file());
        assert_eq!(display_path(&root, &capture), preview_path);
        let preview = crate::pngzip::decode_png(&std::fs::read(&preview_path).unwrap()).unwrap();
        assert_eq!((preview.width, preview.height), (16, 15));
        let reopened = load(&root, &capture).unwrap().unwrap();
        assert_eq!(reopened.base, image(4, 3, [10, 20, 30, 255]));
        let restored = reopened.background.clone().unwrap();
        assert_eq!(restored.padding, 6);
        assert_eq!(restored.radius_power, 3.0);
        assert!(restored.shadow);
        let document = crate::editor::Document::open(reopened);
        assert_eq!(document.background.unwrap().radius, 4.0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_capture_without_a_project_shows_itself() {
        let root = scratch("plain");
        let capture = root.join("capture.png");
        std::fs::write(&capture, b"png").unwrap();
        assert_eq!(display_path(&root, &capture), capture);
        assert!(load(&root, &capture).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_edit_key_is_stable_and_distinct_per_capture() {
        let first = edit_key(Path::new("/tmp/a.png"));
        let again = edit_key(Path::new("/tmp/a.png"));
        let second = edit_key(Path::new("/tmp/b.png"));
        assert_eq!(first, again);
        assert_ne!(first, second);
        assert_eq!(first.len(), 16);
    }

    #[test]
    fn a_reopened_project_keeps_its_alpha() {
        let root = scratch("alpha");
        let capture = root.join("captures/window.png");
        std::fs::create_dir_all(capture.parent().unwrap()).unwrap();
        std::fs::write(&capture, image(2, 2, [1, 2, 3, 0]).to_png()).unwrap();
        let project = project_for(image(2, 2, [1, 2, 3, 0]), None);
        save_editable(&root, &capture, &project).unwrap();
        let reopened = load(&root, &capture).unwrap().unwrap();
        assert_eq!(reopened.base.pixel(0, 0)[3], 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}
