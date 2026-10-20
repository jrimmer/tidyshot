//! The frozen-frame window picker: hover, keyboard cycling and the fallback.
// Ported from omnishot/window_selection.py (WindowSelector) and
// omnishot/app.py (window_selection).
//!
//! A window capture starts by freezing every output and hit-testing the
//! pointer against the client rectangles, so the user picks the window they
//! see instead of dragging a rectangle over it. The overlay is one fullscreen
//! window per output; this module owns the state both the overlay and the
//! tests read: the focus-sorted rows, the hovered row, the Tab cycle and the
//! capture modifiers.
//!
//! When the frozen frames cannot be grabbed (no valid frame, offscreen), the
//! list picker stays available as the keyboard fallback.

use crate::geometry::Rect;
use crate::json::Value;
use crate::window::{WindowInfo, focus_order, visible_windows};

/// One output's frozen frame and where it sits in logical space.
#[derive(Debug, Clone, PartialEq)]
pub struct FrozenFrame {
    pub name: String,
    pub bounds: Rect,
    pub focused: bool,
    /// The PNG on disk the overlay shows, or empty when the grab failed.
    pub url: String,
}

impl FrozenFrame {
    /// A frame that could not be grabbed leaves the overlay without a picture.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        !self.url.is_empty() && self.bounds.width > 0 && self.bounds.height > 0
    }

    /// Whether an absolute point lies on this output.
    #[must_use]
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= f64::from(self.bounds.x)
            && x < f64::from(self.bounds.right())
            && y >= f64::from(self.bounds.y)
            && y < f64::from(self.bounds.bottom())
    }
}

/// One row the overlay hit-tests and the list fallback lists.
#[derive(Debug, Clone, PartialEq)]
pub struct PickerRow {
    pub window: WindowInfo,
    /// The output this row is drawn on, by index into the frame list.
    pub monitor: usize,
}

impl PickerRow {
    /// The row's rectangle relative to its output's top-left corner.
    #[must_use]
    pub fn local_rect(&self, frame: &FrozenFrame) -> Rect {
        Rect::new(
            self.window.rect.x - frame.bounds.x,
            self.window.rect.y - frame.bounds.y,
            self.window.rect.width,
            self.window.rect.height,
        )
    }

    /// The rectangle clipped to its output, so a partly off-screen window
    /// still highlights only the visible part.
    #[must_use]
    pub fn clipped_rect(&self, frame: &FrozenFrame) -> Rect {
        let local = self.local_rect(frame);
        let x = local.x.max(0);
        let y = local.y.max(0);
        Rect::new(
            x,
            y,
            (local.right().min(frame.bounds.width) - x).max(0),
            (local.bottom().min(frame.bounds.height) - y).max(0),
        )
    }
}

/// The complete picker model published to QML.
#[derive(Debug, Clone, PartialEq)]
pub struct PickerModel {
    pub frames: Vec<FrozenFrame>,
    /// Focus-sorted rows, so the first match at a point is the focused window.
    pub rows: Vec<PickerRow>,
    pub hovered: Option<usize>,
    pub accent: String,
}

/// The hint band the overlay draws above the desktop.
pub const HINT: &str =
    "Click a window · Shift for transparency · Tab switches windows · Esc cancels";

/// Freeze every output into a PNG the overlay can show.
///
/// One grab per output, written under `directory` with an atomic rename. A
/// failed output keeps its frame without a picture, so the picker can still
/// fall back to the list instead of hiding the whole desktop.
// Ported from omnishot/app.py (window_selection prepare)
pub fn freeze_outputs(
    monitors: &Value,
    directory: &std::path::Path,
    mut grab: impl FnMut(&crate::capture::Display) -> Result<Vec<u8>, String>,
) -> Vec<FrozenFrame> {
    let _ = std::fs::create_dir_all(directory);
    crate::capture_session::displays_from_monitors(monitors.as_array().unwrap_or(&[]))
        .into_iter()
        .map(|display| {
            let bounds = display.geometry.bounds();
            let name = display.name.clone();
            let focused = display.focused;
            let url = grab(&display)
                .ok()
                .and_then(|png| {
                    let path = directory.join(format!("frozen-{}.png", safe_name(&name)));
                    crate::storage::atomic_write_file(&path, &png)
                        .ok()
                        .map(|()| crate::overlay::local_file_url(&path))
                })
                .unwrap_or_default();
            FrozenFrame {
                name,
                bounds,
                focused,
                url,
            }
        })
        .collect()
}

/// A file-name-safe output name.
fn safe_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

/// Build the model from the live compositor state.
///
/// Rows on an output without a frozen frame are dropped: the overlay draws
/// them on their output, so a missing frame leaves nothing to hover.
#[must_use]
pub fn build(
    clients: &Value,
    monitors: &Value,
    frames: Vec<FrozenFrame>,
    accent: String,
) -> PickerModel {
    let windows = focus_order(&visible_windows(clients, monitors));
    let rows = windows
        .into_iter()
        // TidyShot's own previews must never be offered: they are always on
        // top, so the picker would capture itself.
        .filter(|window| !window.is_own_preview())
        .filter_map(|window| {
            let monitor = frames.iter().position(|frame| {
                frame.contains(
                    f64::from(window.rect.x) + f64::from(window.rect.width) / 2.0,
                    f64::from(window.rect.y) + f64::from(window.rect.height) / 2.0,
                )
            })?;
            Some(PickerRow { window, monitor })
        })
        .collect();
    PickerModel {
        frames,
        rows,
        hovered: None,
        accent,
    }
}

impl PickerModel {
    /// The row under an absolute point, or none.
    ///
    /// The rows are already in focus order, so the topmost focused window wins
    /// when windows overlap, exactly as the source's first-match search does.
    #[must_use]
    pub fn hover_at(&self, x: f64, y: f64) -> Option<usize> {
        self.rows.iter().position(|row| row.window.contains(x, y))
    }

    /// Move the highlight to a row, or clear it.
    pub fn set_hovered(&mut self, index: Option<usize>) {
        self.hovered = index.filter(|index| *index < self.rows.len());
    }

    /// The highlighted row, when any.
    #[must_use]
    pub fn current(&self) -> Option<&PickerRow> {
        self.hovered.and_then(|index| self.rows.get(index))
    }

    /// The highlighted window's title, or an empty line.
    #[must_use]
    pub fn current_title(&self) -> String {
        self.current()
            .map_or_else(String::new, |row| row.window.title.clone())
    }

    /// Cycle the highlight by `step`, wrapping at both ends.
    ///
    /// A picker with no highlight starts at the first row for a forward step
    /// and at the last row for a backward one.
    pub fn cycle(&mut self, step: i32) -> Option<usize> {
        if self.rows.is_empty() {
            self.hovered = None;
            return None;
        }
        let count = self.rows.len() as i32;
        let next = match self.hovered {
            Some(index) => (index as i32 + step).rem_euclid(count),
            None if step < 0 => count - 1,
            None => 0,
        };
        self.hovered = Some(next as usize);
        self.hovered
    }

    /// Whether the overlay can run at all.
    #[must_use]
    pub fn frozen_usable(&self) -> bool {
        self.frames.iter().any(FrozenFrame::is_usable)
    }

    /// The chosen capture.
    #[must_use]
    pub fn choose(&self, transparent: bool, force_copy: bool) -> Option<Choice> {
        self.current().map(|_row| Choice {
            index: self.hovered.expect("a current row is hovered"),
            transparent,
            force_copy,
        })
    }
}

/// The result of a picker gesture.
///
/// `transparent` keeps the isolated window's alpha instead of compositing the
/// shadow and wallpaper; `force_copy` is the Ctrl modifier the source carries
/// through to the after-capture actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice {
    pub index: usize,
    pub transparent: bool,
    pub force_copy: bool,
}

/// The JSON row the overlay reads for one output.
#[must_use]
pub fn frame_json(frame: &FrozenFrame) -> Value {
    let mut object = std::collections::BTreeMap::new();
    object.insert("name".to_owned(), Value::String(frame.name.clone()));
    object.insert("x".to_owned(), Value::Number(f64::from(frame.bounds.x)));
    object.insert("y".to_owned(), Value::Number(f64::from(frame.bounds.y)));
    object.insert("w".to_owned(), Value::Number(f64::from(frame.bounds.width)));
    object.insert(
        "h".to_owned(),
        Value::Number(f64::from(frame.bounds.height)),
    );
    object.insert("url".to_owned(), Value::String(frame.url.clone()));
    object.insert("focused".to_owned(), Value::Bool(frame.focused));
    Value::Object(object)
}

/// The JSON row the overlay reads for one window.
#[must_use]
pub fn row_json(row: &PickerRow, frame: &FrozenFrame) -> Value {
    let clipped = row.clipped_rect(frame);
    let mut object = std::collections::BTreeMap::new();
    object.insert("title".to_owned(), Value::String(row.window.title.clone()));
    object.insert("app".to_owned(), Value::String(row.window.class.clone()));
    object.insert("x".to_owned(), Value::Number(f64::from(clipped.x)));
    object.insert("y".to_owned(), Value::Number(f64::from(clipped.y)));
    object.insert("w".to_owned(), Value::Number(f64::from(clipped.width)));
    object.insert("h".to_owned(), Value::Number(f64::from(clipped.height)));
    Value::Object(object)
}

/// Project the model into the two flat JSON lists QML binds.
#[must_use]
pub fn projections(model: &PickerModel) -> (Vec<String>, Vec<String>) {
    let frames = model.frames.iter().enumerate().map(|(index, frame)| {
        let mut value = frame_json(frame);
        if let Value::Object(object) = &mut value {
            object.insert("index".to_owned(), Value::Number(index as f64));
        }
        value.to_string_compact()
    });
    let rows = model.rows.iter().flat_map(|row| {
        let frame = model.frames.get(row.monitor)?;
        let mut value = row_json(row, frame);
        if let Value::Object(object) = &mut value {
            object.insert("monitor".to_owned(), Value::Number(row.monitor as f64));
        }
        Some(value.to_string_compact())
    });
    (frames.collect(), rows.collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(address: &str, title: &str, x: i32, y: i32, focus: i32) -> String {
        format!(
            r#"{{"address":"{address}","title":"{title}","class":"app","at":[{x},{y}],"size":[100,80],"workspace":{{"name":"1"}},"mapped":true,"focusHistoryID":{focus}}}"#
        )
    }

    fn frames() -> Vec<FrozenFrame> {
        vec![
            FrozenFrame {
                name: "DP-1".to_owned(),
                bounds: Rect::new(0, 0, 1920, 1080),
                focused: true,
                url: "file:///tmp/frozen-0.png".to_owned(),
            },
            FrozenFrame {
                name: "DP-2".to_owned(),
                bounds: Rect::new(1920, 0, 1280, 1024),
                focused: false,
                url: "file:///tmp/frozen-1.png".to_owned(),
            },
        ]
    }

    fn monitors() -> Value {
        crate::json::parse(
            r#"[{"name":"DP-1","x":0,"y":0,"width":1920,"height":1080,"focused":true,"activeWorkspace":{"name":"1"}},{"name":"DP-2","x":1920,"y":0,"width":1280,"height":1024,"focused":false,"activeWorkspace":{"name":"1"}}]"#,
        )
        .unwrap()
    }

    fn model(clients: &str) -> PickerModel {
        build(
            &crate::json::parse(clients).unwrap(),
            &monitors(),
            frames(),
            "#ff0000".to_owned(),
        )
    }

    #[test]
    fn hover_picks_the_topmost_focused_window_at_a_point() {
        let mut picker = model(&format!(
            "[{},{},{}]",
            window("0x1", "Back", 10, 10, 2),
            window("0x2", "Front", 50, 40, 0),
            window("0x3", "Other", 2000, 10, 1)
        ));
        // The focused window sorts first, so it wins an overlap.
        let hit = picker.hover_at(60.0, 50.0);
        assert_eq!(hit, Some(0));
        picker.set_hovered(hit);
        assert_eq!(picker.current_title(), "Front");
        assert_eq!(picker.hover_at(12.0, 12.0), Some(2));
        assert_eq!(picker.hover_at(2100.0, 12.0), Some(1));
        assert_eq!(picker.hover_at(1500.0, 800.0), None);
    }

    #[test]
    fn tab_cycles_by_focus_order_and_wraps() {
        let mut picker = model(&format!(
            "[{},{},{}]",
            window("0x1", "One", 10, 10, 0),
            window("0x2", "Two", 20, 20, 1),
            window("0x3", "Three", 30, 30, 2)
        ));
        assert_eq!(picker.cycle(1), Some(0));
        assert_eq!(picker.cycle(1), Some(1));
        assert_eq!(picker.cycle(1), Some(2));
        assert_eq!(picker.cycle(1), Some(0));
        assert_eq!(picker.cycle(-1), Some(2));
        assert_eq!(picker.current_title(), "Three");
    }

    #[test]
    fn a_choice_carries_the_modifiers() {
        let mut picker = model(&format!("[{}]", window("0x1", "One", 10, 10, 0)));
        assert!(picker.choose(true, false).is_none(), "nothing hovered");
        picker.set_hovered(Some(0));
        let choice = picker.choose(true, true).unwrap();
        assert_eq!(
            choice,
            Choice {
                index: 0,
                transparent: true,
                force_copy: true
            }
        );
    }

    #[test]
    fn rows_are_clipped_to_their_output_and_carry_absolute_rects() {
        let picker = model(&format!("[{}]", window("0x1", "Wide", -40, 20, 0)));
        let frame = &picker.frames[0];
        let row = &picker.rows[0];
        assert_eq!(row.local_rect(frame), Rect::new(-40, 20, 100, 80));
        assert_eq!(row.clipped_rect(frame), Rect::new(0, 20, 60, 80));
        let (frames, rows) = projections(&picker);
        assert_eq!(frames.len(), 2);
        assert!(rows[0].contains("\"monitor\":0"), "{}", rows[0]);
        assert!(frames[0].contains("\"index\":0"), "{}", frames[0]);
        assert!(rows[0].contains("\"x\":0"), "{}", rows[0]);
    }

    #[test]
    fn tidy_shot_previews_are_never_offered() {
        let clients = crate::json::parse(&format!(
            "[{},{}]",
            window("0x1", "One", 10, 10, 0),
            r#"{"address":"0x2","title":"TidyShot Preview","class":"tidyshot","at":[500,500],"size":[100,80],"workspace":{"name":"1"},"mapped":true,"focusHistoryID":1}"#
        ))
        .unwrap();
        let picker = build(&clients, &monitors(), frames(), "#ff0000".to_owned());
        assert_eq!(picker.rows.len(), 1);
        assert_eq!(picker.rows[0].window.title, "One");
    }

    #[test]
    fn windows_without_an_output_frame_are_dropped() {
        let picker = build(
            &crate::json::parse(&format!("[{}]", window("0x1", "One", 10, 10, 0))).unwrap(),
            &monitors(),
            vec![FrozenFrame {
                name: "DP-1".to_owned(),
                bounds: Rect::new(0, 0, 100, 100),
                focused: true,
                url: String::new(),
            }],
            "#ffffff".to_owned(),
        );
        assert_eq!(picker.rows.len(), 1);
        assert!(!picker.frozen_usable());
    }

    #[test]
    fn freezing_writes_one_png_per_output_and_tolerates_a_failed_grab() {
        let dir = std::env::temp_dir().join(format!("tidyshot-freeze-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let png = crate::pngzip::Image::new(2, 2, vec![1u8; 16]).to_png();
        let mut calls = 0;
        let frozen = freeze_outputs(&monitors(), &dir, |display| {
            calls += 1;
            if display.name == "DP-2" {
                return Err("output asleep".to_owned());
            }
            Ok(png.clone())
        });
        assert_eq!(calls, 2);
        assert_eq!(frozen.len(), 2);
        assert!(frozen[0].is_usable());
        assert!(frozen[0].url.starts_with("file://"));
        assert!(
            std::fs::read(dir.join("frozen-DP-1.png"))
                .unwrap()
                .starts_with(b"\x89PNG")
        );
        assert!(!frozen[1].is_usable(), "a failed grab keeps an empty frame");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn frames_without_a_picture_are_not_usable() {
        let mut frame = frames().remove(0);
        assert!(frame.is_usable());
        frame.url.clear();
        assert!(!frame.is_usable());
    }
}
