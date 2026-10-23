//! Capture history and the on-disk capture store.
// Ported from omnishot/backend.py (Store history rows)
//!
//! History is a newest-first JSON array beside the capture store. Every write
//! goes through [`crate::storage`], so a crash cannot leave a half-written
//! history file. A missing or malformed file reads as empty history: the
//! captures themselves are still on disk and must stay reachable.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::geometry::Rect;
use crate::json::Value;

/// The directory holding saved captures.
#[must_use]
pub fn captures_dir(root: &Path) -> PathBuf {
    root.join("captures")
}

/// The history file beside the capture store.
#[must_use]
pub fn history_path(root: &Path) -> PathBuf {
    root.join("history.json")
}

/// One capture's history row.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryRow {
    pub path: PathBuf,
    pub kind: String,
    pub created: f64,
    pub width: u32,
    pub height: u32,
    pub rect: Option<Rect>,
    pub capture_named: bool,
}

impl HistoryRow {
    /// A completed screenshot row.
    #[must_use]
    pub fn screenshot(
        path: impl Into<PathBuf>,
        width: u32,
        height: u32,
        rect: Option<Rect>,
    ) -> Self {
        Self {
            path: path.into(),
            kind: "image".to_owned(),
            created: now_seconds(),
            width,
            height,
            rect,
            capture_named: true,
        }
    }

    /// A finished scrolling capture; a scroll stitch is a screenshot too, so
    /// the annotate-last path still finds it.
    #[must_use]
    pub fn scroll(path: impl Into<PathBuf>, width: u32, height: u32, rect: Option<Rect>) -> Self {
        Self {
            kind: "scroll".to_owned(),
            ..Self::screenshot(path, width, height, rect)
        }
    }

    /// A finished recording.
    #[must_use]
    pub fn video(path: impl Into<PathBuf>, width: u32, height: u32) -> Self {
        Self {
            kind: "video".to_owned(),
            rect: None,
            ..Self::screenshot(path, width, height, None)
        }
    }

    /// Serialize the row for the history file.
    #[must_use]
    pub fn to_value(&self) -> Value {
        use std::collections::BTreeMap;
        let mut object = BTreeMap::new();
        object.insert(
            "path".to_owned(),
            Value::String(self.path.display().to_string()),
        );
        object.insert("kind".to_owned(), Value::String(self.kind.clone()));
        object.insert("created".to_owned(), Value::Number(self.created));
        object.insert("width".to_owned(), Value::Number(f64::from(self.width)));
        object.insert("height".to_owned(), Value::Number(f64::from(self.height)));
        object.insert("capture_named".to_owned(), Value::Bool(self.capture_named));
        if let Some(rect) = self.rect {
            object.insert(
                "rect".to_owned(),
                Value::Array(vec![
                    Value::Number(f64::from(rect.x)),
                    Value::Number(f64::from(rect.y)),
                    Value::Number(f64::from(rect.width)),
                    Value::Number(f64::from(rect.height)),
                ]),
            );
            object.insert(
                "geometry".to_owned(),
                Value::String(crate::geometry::geometry_text(rect)),
            );
        }
        Value::Object(object)
    }

    /// Parse one row, dropping anything without a usable path.
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let path = value.get("path").and_then(Value::as_str)?;
        if path.is_empty() {
            return None;
        }
        let number = |key: &str| value.get(key).and_then(Value::as_f64);
        Some(Self {
            path: PathBuf::from(path),
            kind: value
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("image")
                .to_owned(),
            created: number("created").unwrap_or(0.0),
            width: number("width").unwrap_or(0.0).max(0.0) as u32,
            height: number("height").unwrap_or(0.0).max(0.0) as u32,
            rect: value.get("rect").and_then(rect_from_value),
            capture_named: value
                .get("capture_named")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }
}

fn rect_from_value(value: &Value) -> Option<Rect> {
    let values = value.as_array()?;
    if values.len() != 4 {
        return None;
    }
    let mut parts = [0i32; 4];
    for (slot, value) in parts.iter_mut().zip(values) {
        *slot = value.as_f64()? as i32;
    }
    Some(Rect::new(parts[0], parts[1], parts[2], parts[3]))
}

/// Read every history row, newest first.
#[must_use]
pub fn read(root: &Path) -> Vec<HistoryRow> {
    read_at(&history_path(root))
}

/// Read history from an explicit file.
#[must_use]
pub fn read_at(path: &Path) -> Vec<HistoryRow> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = crate::json::parse(&text) else {
        return Vec::new();
    };
    value
        .as_array()
        .unwrap_or(&[])
        .iter()
        .filter_map(HistoryRow::from_value)
        .collect()
}

/// Prepend `row` to the history, then write it atomically.
pub fn prepend(root: &Path, row: &HistoryRow) -> std::io::Result<()> {
    let mut rows = read(root);
    rows.insert(0, row.clone());
    write(root, &rows)
}

/// Write a complete history listing atomically.
pub fn write(root: &Path, rows: &[HistoryRow]) -> std::io::Result<()> {
    let values: Vec<Value> = rows.iter().map(HistoryRow::to_value).collect();
    let text = Value::Array(values).to_string_pretty();
    std::fs::create_dir_all(root)?;
    crate::storage::atomic_write_file(&history_path(root), text.as_bytes())
}

/// The area most recently captured, for the `previous` command.
#[must_use]
pub fn last_area(root: &Path) -> Option<Rect> {
    read(root).into_iter().find_map(|row| row.rect)
}

/// The newest capture path, when any history exists.
#[must_use]
pub fn newest_path(root: &Path) -> Option<PathBuf> {
    read(root).into_iter().next().map(|row| row.path)
}

fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |duration| duration.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tidyshot-history-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rows_round_trip_newest_first() {
        let root = scratch("round-trip");
        let first = HistoryRow::screenshot("/tmp/a.png", 640, 400, Some(Rect::new(1, 2, 3, 4)));
        let second = HistoryRow::screenshot("/tmp/b.png", 800, 600, None);
        write(&root, &[first.clone(), second.clone()]).unwrap();
        let rows = read(&root);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].path, first.path);
        assert_eq!(rows[0].rect, first.rect);
        assert_eq!(rows[1].path, second.path);
        assert_eq!(last_area(&root), Some(Rect::new(1, 2, 3, 4)));
        assert_eq!(newest_path(&root).as_deref(), Some(Path::new("/tmp/a.png")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prepend_keeps_the_new_row_first_and_tolerates_a_bad_file() {
        let root = scratch("prepend");
        prepend(&root, &HistoryRow::screenshot("/tmp/a.png", 10, 20, None)).unwrap();
        prepend(&root, &HistoryRow::screenshot("/tmp/b.png", 30, 40, None)).unwrap();
        let rows = read(&root);
        assert_eq!(rows[0].path, PathBuf::from("/tmp/b.png"));
        assert_eq!(rows[1].path, PathBuf::from("/tmp/a.png"));
        // A malformed history must read as empty rather than fail.
        std::fs::write(history_path(&root), "not json").unwrap();
        assert!(read(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn filters_and_search_keep_newest_first() {
        let root = scratch("filter");
        let rows = [
            HistoryRow::screenshot("/tmp/Browser.png", 1, 1, None),
            HistoryRow::scroll("/tmp/long-page.png", 1, 1, None),
            HistoryRow::video("/tmp/take.mp4", 1, 1),
        ];
        write(&root, &rows).unwrap();
        assert_eq!(filtered(&root, KindFilter::All, "").len(), 3);
        assert_eq!(filtered(&root, KindFilter::Video, "").len(), 1);
        assert_eq!(filtered(&root, KindFilter::Scroll, "").len(), 1);
        // The images tab includes screenshots and scroll stitches.
        assert_eq!(filtered(&root, KindFilter::Image, "").len(), 2);
        let search = filtered(&root, KindFilter::All, "BROW");
        assert_eq!(search.len(), 1);
        assert_eq!(search[0].path, PathBuf::from("/tmp/Browser.png"));
        assert!(filtered(&root, KindFilter::All, "missing").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deleting_moves_the_capture_to_trash_and_drops_its_row() {
        let root = scratch("delete");
        std::fs::create_dir_all(captures_dir(&root)).unwrap();
        let shot = captures_dir(&root).join("shot.png");
        let keep = captures_dir(&root).join("keep.png");
        std::fs::write(&shot, b"capture").unwrap();
        std::fs::write(&keep, b"another").unwrap();
        write(
            &root,
            &[
                HistoryRow::screenshot(&shot, 1, 1, None),
                HistoryRow::screenshot(&keep, 1, 1, None),
            ],
        )
        .unwrap();
        let trashed = delete(&root, &shot).unwrap();
        assert!(!shot.exists(), "the capture left its old path");
        assert_eq!(std::fs::read(&trashed).unwrap(), b"capture");
        assert!(trashed.starts_with(trash_dir(&root)));
        assert!(keep.exists(), "a sibling capture is untouched");
        let rows = read(&root);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, keep);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_capture_is_reported_without_touching_history() {
        let root = scratch("missing-delete");
        let path = root.join("absent.png");
        write(&root, &[HistoryRow::screenshot(&path, 1, 1, None)]).unwrap();
        let error = delete(&root, &path).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(read(&root).len(), 1, "the row stays when the move fails");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clear_moves_every_capture_to_trash() {
        let root = scratch("clear");
        std::fs::create_dir_all(captures_dir(&root)).unwrap();
        let first = captures_dir(&root).join("first.png");
        let second = captures_dir(&root).join("second.png");
        std::fs::write(&first, b"one").unwrap();
        std::fs::write(&second, b"two").unwrap();
        write(
            &root,
            &[
                HistoryRow::screenshot(&first, 1, 1, None),
                HistoryRow::screenshot(&second, 1, 1, None),
            ],
        )
        .unwrap();
        assert_eq!(clear(&root).unwrap(), 2);
        assert!(read(&root).is_empty());
        assert!(!first.exists() && !second.exists());
        let trashed = std::fs::read_dir(trash_dir(&root)).unwrap().count();
        assert_eq!(trashed, 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn retention_prunes_old_rows_and_keeps_recent_ones() {
        let root = scratch("retention");
        std::fs::create_dir_all(captures_dir(&root)).unwrap();
        let old = captures_dir(&root).join("old.png");
        let recent = captures_dir(&root).join("recent.png");
        std::fs::write(&old, b"old").unwrap();
        std::fs::write(&recent, b"new").unwrap();
        let mut old_row = HistoryRow::screenshot(&old, 1, 1, None);
        old_row.created = 1_000.0;
        let mut recent_row = HistoryRow::screenshot(&recent, 1, 1, None);
        recent_row.created = 990_000.0;
        write(&root, &[old_row, recent_row.clone()]).unwrap();
        // One day at a time base of a million seconds keeps the new row.
        assert_eq!(prune(&root, 1, 1_000_000.0).unwrap(), 1);
        let rows = read(&root);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, recent_row.path);
        assert!(!old.exists());
        // Zero days keeps everything by design.
        assert_eq!(prune(&root, 0, 2_000_000.0).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn newest_lookups_skip_rows_whose_file_is_gone() {
        let root = scratch("newest");
        let missing = root.join("missing.png");
        let present = root.join("present.png");
        std::fs::write(&present, b"ok").unwrap();
        write(
            &root,
            &[
                HistoryRow::video(&missing, 1, 1),
                HistoryRow::screenshot(&present, 1, 1, None),
            ],
        )
        .unwrap();
        assert_eq!(newest_existing(&root).unwrap().path, present);
        assert!(newest_matching(&root, KindFilter::Video).is_none());
    }

    #[test]
    fn capture_rows_without_a_path_are_dropped() {
        let value = crate::json::parse(r#"{"kind":"image"}"#).unwrap();
        assert!(HistoryRow::from_value(&value).is_none());
        let value =
            crate::json::parse(r#"{"path":"/tmp/x.png","rect":[1,2,3,4],"width":5}"#).unwrap();
        let row = HistoryRow::from_value(&value).unwrap();
        assert_eq!(row.rect, Some(Rect::new(1, 2, 3, 4)));
        assert_eq!(row.width, 5);
        assert!(!row.capture_named);
    }
}

/// The directory captures move to when deleted.
#[must_use]
pub fn trash_dir(root: &Path) -> PathBuf {
    root.join("trash")
}

/// Which type tab a history view is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KindFilter {
    #[default]
    All,
    Image,
    Scroll,
    Video,
}

impl KindFilter {
    /// Parse a tab id; an unknown tab shows everything.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "image" => Self::Image,
            "scroll" => Self::Scroll,
            "video" => Self::Video,
            _ => Self::All,
        }
    }

    /// The tab id this filter came from.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Image => "image",
            Self::Scroll => "scroll",
            Self::Video => "video",
        }
    }

    /// Whether a row belongs to this tab. Screenshots and scroll stitches are
    /// both still images, so the image tab shows both.
    #[must_use]
    pub fn matches(self, row: &HistoryRow) -> bool {
        match self {
            Self::All => true,
            Self::Image => row.kind == "image" || row.kind == "scroll",
            Self::Scroll => row.kind == "scroll",
            Self::Video => row.kind == "video",
        }
    }
}

/// Whether a row's file name contains `query`, ignoring case.
#[must_use]
pub fn name_matches(row: &HistoryRow, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    row.path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_lowercase().contains(&query))
}

/// The history rows for one tab and search query, newest first.
#[must_use]
pub fn filtered(root: &Path, filter: KindFilter, query: &str) -> Vec<HistoryRow> {
    read(root)
        .into_iter()
        .filter(|row| filter.matches(row) && name_matches(row, query))
        .collect()
}

/// A unique sibling path inside the trash directory.
fn trash_target(root: &Path, source: &Path) -> PathBuf {
    let directory = trash_dir(root);
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("capture");
    let mut candidate = directory.join(name);
    let mut sequence = 2;
    while candidate.exists() {
        let stem = source
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("capture");
        let extension = source
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!(".{extension}"))
            .unwrap_or_default();
        candidate = directory.join(format!("{stem} ({sequence}){extension}"));
        sequence += 1;
    }
    candidate
}

/// Move one file into the trash directory without ever unlinking it first.
///
/// A rename keeps the move atomic on one filesystem. Across filesystems the
/// bytes are copied to a temporary file in the trash, fsynced and renamed into
/// place before the source is removed, so a failed copy leaves the capture
/// untouched. The returned path is the file inside the trash.
pub fn move_to_trash(root: &Path, source: &Path) -> std::io::Result<PathBuf> {
    if !source.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("the capture is no longer available: {}", source.display()),
        ));
    }
    std::fs::create_dir_all(trash_dir(root))?;
    let target = trash_target(root, source);
    match std::fs::rename(source, &target) {
        Ok(()) => Ok(target),
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            let bytes = std::fs::read(source)?;
            crate::storage::atomic_write_file(&target, &bytes)?;
            std::fs::remove_file(source)?;
            Ok(target)
        }
        Err(error) => Err(error),
    }
}

/// Delete one history row by moving its capture to the trash.
///
/// The capture is moved first; only then is the row dropped from the history.
/// A move failure leaves both the file and its row in place and is reported.
pub fn delete(root: &Path, path: &Path) -> std::io::Result<PathBuf> {
    let trashed = move_to_trash(root, path)?;
    let mut rows = read(root);
    rows.retain(|row| row.path != path);
    write(root, &rows)?;
    Ok(trashed)
}

/// Move every capture in the history to the trash and clear the listing.
pub fn clear(root: &Path) -> std::io::Result<usize> {
    let rows = read(root);
    let mut moved = 0;
    for row in &rows {
        // A row whose file is already gone is dropped without touching disk.
        if row.path.is_file() {
            move_to_trash(root, &row.path)?;
            moved += 1;
        }
    }
    write(root, &[])?;
    Ok(moved)
}

/// Drop rows older than `days`, moving their captures to the trash.
///
/// `now` is unix seconds, so the caller can pin a clock. A retention of zero
/// keeps everything. A missing file is dropped without moving anything.
pub fn prune(root: &Path, days: u32, now: f64) -> std::io::Result<usize> {
    if days == 0 {
        return Ok(0);
    }
    let cutoff = now - f64::from(days) * 86_400.0;
    // A row without a timestamp cannot be shown to be old, so it is kept.
    let (old, keep): (Vec<HistoryRow>, Vec<HistoryRow>) = read(root)
        .into_iter()
        .partition(|row| row.created > 0.0 && row.created < cutoff);
    let mut pruned = 0;
    for row in &old {
        if row.path.is_file() {
            move_to_trash(root, &row.path)?;
            pruned += 1;
        }
    }
    if !old.is_empty() {
        write(root, &keep)?;
    }
    Ok(pruned)
}

/// The newest row whose path still exists.
#[must_use]
pub fn newest_existing(root: &Path) -> Option<HistoryRow> {
    read(root).into_iter().find(|row| row.path.is_file())
}

/// The newest row matching a filter, skipping rows whose file is missing.
#[must_use]
pub fn newest_matching(root: &Path, filter: KindFilter) -> Option<HistoryRow> {
    read(root)
        .into_iter()
        .find(|row| filter.matches(row) && row.path.is_file())
}
