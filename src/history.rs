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
