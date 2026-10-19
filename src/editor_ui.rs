//! Editor tools over the document, renderer and undo model.
// Ported from omnishot/editor.py (tool presses, provisional annotations) and
// omnishot/editor_toolbar.py (TOOLS).
//!
//! This module owns the interaction state a drag passes through: the selected
//! tool, the annotation or crop being drawn, the selected object and the text
//! waiting for input. Every finished drag mutates [`Document`] once and calls
//! `commit` once, so the undo stack matches what the user did.

use crate::annotation::{Annotation, Point, Props, Rect, text_height};
use crate::color::Rgba as Color;
use crate::editor::{Document, EditorError, background_color};
use crate::json::Value;

/// Every editor tool, in toolbar order.
pub const TOOLS: &[(&str, &str, &str)] = &[
    ("select", "Select", "V"),
    ("arrow", "Arrow", "A"),
    ("line", "Line", "L"),
    ("rect", "Rectangle", "R"),
    ("ellipse", "Ellipse", "E"),
    ("text", "Text", "T"),
    ("counter", "Counter", "N"),
    ("pencil", "Pencil", "P"),
    ("highlighter", "Highlighter", "H"),
    ("blur", "Blur", "B"),
    ("pixelate", "Pixelate", "X"),
    ("crop", "Crop", "C"),
];

/// The tool ids, for the QML toolbar model.
#[must_use]
pub fn tool_ids() -> Vec<String> {
    TOOLS.iter().map(|(id, _, _)| (*id).to_owned()).collect()
}

/// The tool labels, for the QML toolbar model.
#[must_use]
pub fn tool_labels() -> Vec<String> {
    TOOLS
        .iter()
        .map(|(_, label, _)| (*label).to_owned())
        .collect()
}

/// The single-key shortcuts, for the QML toolbar model.
#[must_use]
pub fn tool_shortcuts() -> Vec<String> {
    TOOLS.iter().map(|(_, _, key)| (*key).to_owned()).collect()
}

/// The editor tool currently armed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Text,
    Counter,
    Pencil,
    Highlighter,
    Blur,
    Pixelate,
    Crop,
}

impl Tool {
    /// Parse a toolbar id, defaulting to Select for an unknown id.
    #[must_use]
    pub fn from_id(id: &str) -> Self {
        match id {
            "arrow" => Self::Arrow,
            "line" => Self::Line,
            "rect" => Self::Rect,
            "ellipse" => Self::Ellipse,
            "text" => Self::Text,
            "counter" => Self::Counter,
            "pencil" => Self::Pencil,
            "highlighter" => Self::Highlighter,
            "blur" => Self::Blur,
            "pixelate" => Self::Pixelate,
            "crop" => Self::Crop,
            _ => Self::Select,
        }
    }

    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Arrow => "arrow",
            Self::Line => "line",
            Self::Rect => "rect",
            Self::Ellipse => "ellipse",
            Self::Text => "text",
            Self::Counter => "counter",
            Self::Pencil => "pencil",
            Self::Highlighter => "highlighter",
            Self::Blur => "blur",
            Self::Pixelate => "pixelate",
            Self::Crop => "crop",
        }
    }

    /// The tool a single key arms, ignoring case.
    #[must_use]
    pub fn from_shortcut(key: &str) -> Option<Self> {
        let key = key.to_ascii_uppercase();
        TOOLS
            .iter()
            .find(|(_, _, shortcut)| *shortcut == key)
            .map(|(id, _, _)| Self::from_id(id))
    }

    /// Whether the tool draws a rectangular annotation on release.
    #[must_use]
    pub const fn is_rect_shape(self) -> bool {
        matches!(
            self,
            Self::Rect | Self::Ellipse | Self::Blur | Self::Pixelate | Self::Highlighter
        )
    }

    /// Whether the tool follows the pointer and stores its path.
    #[must_use]
    pub const fn is_freehand(self) -> bool {
        matches!(self, Self::Pencil | Self::Highlighter)
    }
}

/// A provisional annotation that has not entered the document yet.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingText {
    pub annotation: Annotation,
    pub index: Option<usize>,
}

/// The drag currently in progress.
#[derive(Debug, Clone, PartialEq)]
struct Drag {
    tool: Tool,
    start: Point,
    current: Point,
    points: Vec<Point>,
    /// The object being moved by the select tool, and its start position.
    moving: Option<(usize, f64, f64)>,
    moved: bool,
}

/// The editor's interaction state.
#[derive(Debug, Clone)]
pub struct EditorInteraction {
    tool: Tool,
    color: Color,
    stroke: f64,
    drag: Option<Drag>,
    crop: Option<Rect>,
    selected: Option<usize>,
    pending_text: Option<PendingText>,
    counter: i64,
}

impl Default for EditorInteraction {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorInteraction {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tool: Tool::Select,
            color: Color::new(0xff, 0x5b, 0x61, 255),
            stroke: 4.0,
            drag: None,
            crop: None,
            selected: None,
            pending_text: None,
            counter: 1,
        }
    }

    #[must_use]
    pub const fn tool(&self) -> Tool {
        self.tool
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.drag = None;
        self.pending_text = None;
        if tool != Tool::Select {
            self.selected = None;
        }
    }

    #[must_use]
    pub const fn color(&self) -> Color {
        self.color
    }

    pub fn set_color(&mut self, color: Color) {
        self.color = color;
    }

    #[must_use]
    pub const fn stroke(&self) -> f64 {
        self.stroke
    }

    pub fn set_stroke(&mut self, stroke: f64) {
        self.stroke = stroke.clamp(1.0, 40.0);
    }

    #[must_use]
    pub const fn selected(&self) -> Option<usize> {
        self.selected
    }

    #[must_use]
    pub const fn crop(&self) -> Option<Rect> {
        self.crop
    }

    #[must_use]
    pub fn pending_text(&self) -> Option<&PendingText> {
        self.pending_text.as_ref()
    }

    #[must_use]
    pub const fn counter(&self) -> i64 {
        self.counter
    }

    /// Begin a drag or a selection at `point`.
    pub fn press(&mut self, document: &mut Document, point: Point) {
        self.crop = None;
        self.pending_text = None;
        let point = clamp_to_canvas(document, point);
        if self.tool == Tool::Select {
            self.selected = topmost_at(document, point);
            let moving = self
                .selected
                .and_then(|index| document.objects.get(index))
                .map(|object| (object.x(), object.y()));
            self.drag = Some(Drag {
                tool: self.tool,
                start: point,
                current: point,
                points: vec![point],
                moving: self
                    .selected
                    .zip(moving)
                    .map(|(index, (x, y))| (index, x, y)),
                moved: false,
            });
            return;
        }
        self.drag = Some(Drag {
            tool: self.tool,
            start: point,
            current: point,
            points: vec![point],
            moving: None,
            moved: false,
        });
    }

    /// Update the drag to `point`; the document is only mutated for a move.
    pub fn move_to(&mut self, document: &mut Document, point: Point) {
        let point = clamp_to_canvas(document, point);
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        if drag.current != point {
            drag.moved = true;
        }
        drag.current = point;
        if drag.points.last() != Some(&point) && drag.points.len() < 4096 {
            drag.points.push(point);
        }
        if let Some((index, x, y)) = drag.moving {
            let dx = point.x - drag.start.x;
            let dy = point.y - drag.start.y;
            if let Some(object) = document.objects.get_mut(index) {
                object.props.insert("x".to_owned(), Value::Number(x + dx));
                object.props.insert("y".to_owned(), Value::Number(y + dy));
            }
        }
    }

    /// Finish the drag. Returns true when the document changed.
    pub fn release(&mut self, document: &mut Document, point: Point) -> bool {
        self.move_to(document, point);
        let Some(drag) = self.drag.take() else {
            return false;
        };
        match drag.tool {
            Tool::Select => {
                if drag.moving.is_some() && drag.moved {
                    document.commit();
                    true
                } else {
                    false
                }
            }
            Tool::Crop => {
                let rect = rect_from_points(drag.start, drag.current);
                if rect.width < 2.0 || rect.height < 2.0 {
                    self.crop = None;
                    false
                } else {
                    self.crop = Some(rect);
                    false
                }
            }
            Tool::Text => {
                let rect = text_rect(drag.start, drag.current, self.stroke);
                let mut annotation = self.base_annotation("text", rect, document);
                annotation
                    .props
                    .insert("text".to_owned(), Value::String(String::new()));
                annotation.props.insert(
                    "font_size".to_owned(),
                    Value::Number(font_size(self.stroke)),
                );
                annotation
                    .props
                    .insert("text_style".to_owned(), Value::String("plain".to_owned()));
                annotation.props.insert(
                    "h".to_owned(),
                    Value::Number(annotation.height().max(text_height(&annotation.props))),
                );
                self.pending_text = Some(PendingText {
                    annotation,
                    index: None,
                });
                false
            }
            Tool::Pencil | Tool::Highlighter => {
                let bounds = points_bounds(&drag.points);
                if bounds.width < 1.0 && bounds.height < 1.0 {
                    return false;
                }
                let kind = if drag.tool == Tool::Pencil {
                    "pencil"
                } else {
                    "highlight"
                };
                let mut annotation = self.base_annotation(kind, bounds, document);
                let points: Vec<Value> = drag
                    .points
                    .iter()
                    .map(|point| {
                        Value::Array(vec![
                            Value::Number(if bounds.width > 0.0 {
                                (point.x - bounds.x) / bounds.width
                            } else {
                                0.0
                            }),
                            Value::Number(if bounds.height > 0.0 {
                                (point.y - bounds.y) / bounds.height
                            } else {
                                0.0
                            }),
                        ])
                    })
                    .collect();
                annotation
                    .props
                    .insert("points".to_owned(), Value::Array(points));
                annotation
                    .props
                    .insert("smooth".to_owned(), Value::Bool(true));
                push(document, annotation);
                true
            }
            tool => {
                let rect = rect_from_points(drag.start, drag.current);
                if rect.width < 1.0 || rect.height < 1.0 {
                    return false;
                }
                let kind = match tool {
                    Tool::Arrow => "arrow",
                    Tool::Line => "line",
                    Tool::Ellipse => "ellipse",
                    Tool::Blur => "blur",
                    Tool::Pixelate => "pixelate",
                    Tool::Counter => "counter",
                    _ => "rect",
                };
                let mut annotation = self.base_annotation(kind, rect, document);
                if matches!(tool, Tool::Arrow | Tool::Line) {
                    let (ax, ay, bx, by) = line_endpoints(rect, drag.start, drag.current);
                    annotation.props.insert("ax".to_owned(), Value::Number(ax));
                    annotation.props.insert("ay".to_owned(), Value::Number(ay));
                    annotation.props.insert("bx".to_owned(), Value::Number(bx));
                    annotation.props.insert("by".to_owned(), Value::Number(by));
                    annotation
                        .props
                        .insert("style".to_owned(), Value::String("standard".to_owned()));
                }
                if tool == Tool::Counter {
                    annotation
                        .props
                        .insert("text".to_owned(), Value::String(self.counter.to_string()));
                    annotation.props.insert(
                        "counter_style".to_owned(),
                        Value::String("Circle".to_owned()),
                    );
                    annotation
                        .props
                        .insert("font_size".to_owned(), Value::Number(20.0));
                    self.counter += 1;
                }
                if tool == Tool::Blur {
                    annotation
                        .props
                        .insert("strength".to_owned(), Value::Number(18.0));
                    annotation
                        .props
                        .insert("blur_mode".to_owned(), Value::String("Smooth".to_owned()));
                }
                if tool == Tool::Pixelate {
                    annotation
                        .props
                        .insert("strength".to_owned(), Value::Number(12.0));
                    annotation.props.insert(
                        "pixelate_mode".to_owned(),
                        Value::String("Classic".to_owned()),
                    );
                }
                push(document, annotation);
                true
            }
        }
    }

    /// Finish a text annotation with the user's text; returns true when added.
    pub fn commit_text(&mut self, document: &mut Document, text: &str) -> bool {
        let Some(pending) = self.pending_text.take() else {
            return false;
        };
        if text.is_empty() {
            return false;
        }
        let mut annotation = pending.annotation;
        annotation
            .props
            .insert("text".to_owned(), Value::String(text.to_owned()));
        let height = annotation.height().max(text_height(&annotation.props));
        annotation
            .props
            .insert("h".to_owned(), Value::Number(height));
        push(document, annotation);
        true
    }

    /// Drop a provisional text without touching the document.
    pub fn cancel_text(&mut self) -> bool {
        self.pending_text.take().is_some()
    }

    /// Apply the provisional crop, if one is drawn.
    pub fn apply_crop(
        &mut self,
        document: &mut Document,
        fill: Option<Color>,
    ) -> Result<bool, EditorError> {
        let Some(rect) = self.crop.take() else {
            return Ok(false);
        };
        let expand = rect.x < 0.0
            || rect.y < 0.0
            || rect.right() > document.base.width as f64
            || rect.bottom() > document.base.height as f64;
        let fill = fill.or_else(|| expand.then(|| background_color(&document.base)));
        document.crop(rect, expand, fill)?;
        self.selected = None;
        Ok(true)
    }

    /// Escape drops a provisional crop without changing the document.
    pub fn cancel_crop(&mut self) -> bool {
        self.crop.take().is_some()
    }

    /// Delete the selected object as one undo step.
    pub fn delete_selected(&mut self, document: &mut Document) -> bool {
        let Some(index) = self.selected.take() else {
            return false;
        };
        if index >= document.objects.len() {
            return false;
        }
        document.delete(&[index]);
        true
    }

    /// The selected object's rectangle, for a QML selection outline.
    #[must_use]
    pub fn selection_rect(&self, document: &Document) -> Option<Rect> {
        self.selected
            .and_then(|index| document.objects.get(index))
            .map(Annotation::content_bounds)
    }

    /// The annotation a finished text drag would add, for the interaction test.
    #[must_use]
    pub fn pending_annotation(&self) -> Option<&Annotation> {
        self.pending_text
            .as_ref()
            .map(|pending| &pending.annotation)
    }

    fn base_annotation(&self, kind: &str, rect: Rect, document: &Document) -> Annotation {
        let mut props = Props::new();
        props.insert("kind".to_owned(), Value::String(kind.to_owned()));
        props.insert("x".to_owned(), Value::Number(rect.x));
        props.insert("y".to_owned(), Value::Number(rect.y));
        props.insert("w".to_owned(), Value::Number(rect.width.max(1.0)));
        props.insert("h".to_owned(), Value::Number(rect.height.max(1.0)));
        props.insert(
            "color".to_owned(),
            Value::String(crate::color::color_text(self.color)),
        );
        props.insert("width".to_owned(), Value::Number(self.stroke));
        let z = document
            .objects
            .iter()
            .map(Annotation::z)
            .fold(0.0, f64::max)
            + 1.0;
        props.insert("z".to_owned(), Value::Number(z));
        if kind == "highlight" {
            props.insert("highlight_alpha".to_owned(), Value::Number(100.0));
        }
        Annotation::new(props)
    }
}

fn push(document: &mut Document, annotation: Annotation) {
    document.objects.push(annotation);
    document.commit();
}

fn clamp_to_canvas(document: &Document, point: Point) -> Point {
    Point::new(
        point.x.clamp(0.0, document.base.width as f64),
        point.y.clamp(0.0, document.base.height as f64),
    )
}

fn topmost_at(document: &Document, point: Point) -> Option<usize> {
    document
        .objects
        .iter()
        .enumerate()
        .rev()
        .find(|(_, object)| object.content_bounds().contains(point))
        .map(|(index, _)| index)
}

/// The normalized corner rectangle between two points.
#[must_use]
pub fn rect_from_points(a: Point, b: Point) -> Rect {
    Rect::new(
        a.x.min(b.x),
        a.y.min(b.y),
        (a.x - b.x).abs(),
        (a.y - b.y).abs(),
    )
}

fn text_rect(a: Point, b: Point, stroke: f64) -> Rect {
    let rect = rect_from_points(a, b);
    Rect::new(
        rect.x,
        rect.y,
        rect.width.max(120.0 + stroke),
        rect.height.max(font_size(stroke) * 1.25),
    )
}

fn font_size(stroke: f64) -> f64 {
    (stroke * 5.0).clamp(18.0, 72.0)
}

fn points_bounds(points: &[Point]) -> Rect {
    let mut x0 = f64::INFINITY;
    let mut y0 = f64::INFINITY;
    let mut x1 = f64::NEG_INFINITY;
    let mut y1 = f64::NEG_INFINITY;
    for point in points {
        x0 = x0.min(point.x);
        y0 = y0.min(point.y);
        x1 = x1.max(point.x);
        y1 = y1.max(point.y);
    }
    if !x0.is_finite() {
        return Rect::default();
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

fn line_endpoints(rect: Rect, start: Point, end: Point) -> (f64, f64, f64, f64) {
    let width = rect.width.max(1.0);
    let height = rect.height.max(1.0);
    (
        (start.x - rect.x) / width,
        (start.y - rect.y) / height,
        (end.x - rect.x) / width,
        (end.y - rect.y) / height,
    )
}

/// The provisional shape drawn during a drag, for the QML preview.
#[derive(Debug, Clone, PartialEq)]
pub struct Provisional {
    pub kind: &'static str,
    pub rect: Rect,
    pub points: Vec<Point>,
    pub crop: bool,
}

impl EditorInteraction {
    /// The shape the current drag would add, or the crop being drawn.
    #[must_use]
    pub fn provisional(&self) -> Option<Provisional> {
        let drag = self.drag.as_ref()?;
        if drag.tool == Tool::Crop {
            let rect = rect_from_points(drag.start, drag.current);
            if rect.width < 1.0 || rect.height < 1.0 {
                return None;
            }
            return Some(Provisional {
                kind: "crop",
                rect,
                points: Vec::new(),
                crop: true,
            });
        }
        if drag.tool.is_freehand() {
            let bounds = points_bounds(&drag.points);
            return Some(Provisional {
                kind: drag.tool.id(),
                rect: bounds,
                points: drag.points.clone(),
                crop: false,
            });
        }
        let rect = rect_from_points(drag.start, drag.current);
        if rect.width < 1.0 || rect.height < 1.0 {
            return None;
        }
        Some(Provisional {
            kind: drag.tool.id(),
            rect,
            points: vec![drag.start, drag.current],
            crop: false,
        })
    }

    /// Drop an unfinished drag without touching the document.
    pub fn cancel_drag(&mut self) -> bool {
        self.drag.take().is_some()
    }
}
/// The action a key press performs in the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorAction {
    Undo,
    Redo,
    Save,
    Copy,
    Delete,
    Cancel,
    Apply,
    Tool(Tool),
}

/// Map a key press to its editor action, or none when it is unbound.
///
/// Ctrl+Shift+Z redoes, Ctrl+Z undoes and a plain letter arms its tool. A
/// modifier other than Ctrl or Shift with a letter is left to the text editor.
#[must_use]
pub fn action_for_key(key: &str, ctrl: bool, shift: bool) -> Option<EditorAction> {
    if ctrl && key.eq_ignore_ascii_case("z") {
        return Some(if shift {
            EditorAction::Redo
        } else {
            EditorAction::Undo
        });
    }
    if ctrl && key.eq_ignore_ascii_case("y") {
        return Some(EditorAction::Redo);
    }
    if ctrl && key.eq_ignore_ascii_case("s") {
        return Some(EditorAction::Save);
    }
    if ctrl && key.eq_ignore_ascii_case("c") {
        return Some(EditorAction::Copy);
    }
    if !ctrl && !shift {
        if key == "Escape" {
            return Some(EditorAction::Cancel);
        }
        if key == "Return" || key == "Enter" {
            return Some(EditorAction::Apply);
        }
        if key == "Delete" || key == "Backspace" {
            return Some(EditorAction::Delete);
        }
        return Tool::from_shortcut(key).map(EditorAction::Tool);
    }
    None
}

/// The rendered clipboard PNG for a document.
pub fn clipboard_png(document: &Document) -> Vec<u8> {
    document.render_export().to_png()
}

/// True when a path is an editable project archive.
#[must_use]
pub fn is_project(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("omnishot"))
}

/// Open an image or `.omnishot` project as a document.
pub fn load_document(path: &std::path::Path) -> Result<Document, EditorError> {
    let bytes = std::fs::read(path)
        .map_err(|error| EditorError::new(format!("Could not open {}: {error}", path.display())))?;
    if is_project(path) {
        let project = crate::project::Project::read(&bytes)
            .map_err(|error| EditorError::new(error.to_string()))?;
        Ok(Document::open(project))
    } else {
        let image = crate::pngzip::decode_png(&bytes).map_err(|error| {
            EditorError::new(format!("Could not open {}: {error}", path.display()))
        })?;
        Ok(Document::new(image, None, false))
    }
}

/// Write the document to `path`, atomically, in the format the path names.
///
/// A `.omnishot` path keeps the editable project; any other path flattens the
/// rendered image to PNG.
pub fn save_document(document: &Document, path: &std::path::Path) -> Result<(), EditorError> {
    if is_project(path) {
        document.save(path)
    } else {
        let bytes = document.render_export().to_png();
        crate::storage::atomic_write_file(path, &bytes)
            .map_err(|error| EditorError::new(error.to_string()))
    }
}

/// The document's filename for the window title.
#[must_use]
pub fn display_name(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Untitled")
        .to_owned()
}

/// A default save path beside the opened file, changing the extension only.
#[must_use]
pub fn suggested_save_path(path: Option<&std::path::Path>, project: bool) -> String {
    let extension = if project { "omnishot" } else { "png" };
    match path {
        Some(path) => path.with_extension(extension).display().to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pngzip::Image;

    fn document(width: usize, height: usize) -> Document {
        Document::new(
            Image::new(width, height, vec![200u8; width * height * 4]),
            None,
            false,
        )
    }

    fn drag(
        interaction: &mut EditorInteraction,
        document: &mut Document,
        a: (f64, f64),
        b: (f64, f64),
    ) {
        interaction.press(document, Point::new(a.0, a.1));
        interaction.move_to(document, Point::new(b.0, b.1));
        interaction.release(document, Point::new(b.0, b.1));
    }

    fn draw_shape(
        interaction: &mut EditorInteraction,
        document: &mut Document,
        a: (f64, f64),
        b: (f64, f64),
    ) {
        drag(interaction, document, a, b);
        assert_eq!(document.objects.len(), 1, "the drag added one annotation");
    }

    #[test]
    fn every_tool_has_a_label_and_a_unique_shortcut() {
        assert_eq!(TOOLS.len(), 12);
        let mut keys: Vec<&str> = TOOLS.iter().map(|(_, _, key)| *key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), TOOLS.len());
        assert_eq!(Tool::from_shortcut("v"), Some(Tool::Select));
        assert_eq!(Tool::from_shortcut("X"), Some(Tool::Pixelate));
        assert_eq!(Tool::from_shortcut("?"), None);
        assert_eq!(tool_ids()[0], "select");
        assert_eq!(tool_labels().len(), tool_ids().len());
        assert_eq!(tool_shortcuts()[1], "A");
    }

    #[test]
    fn a_shape_drag_is_one_undo_step() {
        let mut document = document(400, 300);
        let mut interaction = EditorInteraction::new();
        interaction.set_tool(Tool::Rect);
        interaction.set_color(Color::new(0, 255, 0, 255));
        assert!(!document.can_undo());
        draw_shape(&mut interaction, &mut document, (10.0, 20.0), (110.0, 80.0));
        assert!(document.can_undo());
        let object = &document.objects[0];
        assert_eq!(object.kind(), Some("rect"));
        assert_eq!(
            (object.x(), object.y(), object.width(), object.height()),
            (10.0, 20.0, 100.0, 60.0)
        );
        assert_eq!(object.string("color", ""), "#00ff00");
        document.undo();
        assert!(document.objects.is_empty(), "one drag is one undo step");
        document.redo();
        assert_eq!(document.objects.len(), 1);
    }

    #[test]
    fn arrows_point_from_the_press_to_the_release() {
        let mut document = document(200, 200);
        let mut interaction = EditorInteraction::new();
        interaction.set_tool(Tool::Arrow);
        drag(
            &mut interaction,
            &mut document,
            (150.0, 150.0),
            (50.0, 60.0),
        );
        let object = &document.objects[0];
        assert_eq!(object.kind(), Some("arrow"));
        assert_eq!(object.x(), 50.0);
        assert_eq!(object.y(), 60.0);
        assert_eq!(object.string("style", ""), "standard");
        let (a, b) = object.endpoints();
        assert!(a.x > b.x, "the head is at the release point");
    }

    #[test]
    fn freehand_tools_store_normalized_points() {
        let mut document = document(300, 300);
        let mut interaction = EditorInteraction::new();
        interaction.set_tool(Tool::Pencil);
        interaction.press(&mut document, Point::new(10.0, 10.0));
        interaction.move_to(&mut document, Point::new(30.0, 50.0));
        interaction.move_to(&mut document, Point::new(60.0, 90.0));
        assert!(interaction.release(&mut document, Point::new(60.0, 90.0)));
        let object = &document.objects[0];
        assert_eq!(object.kind(), Some("pencil"));
        assert_eq!((object.x(), object.y()), (10.0, 10.0));
        let points = object.props["points"].as_array().unwrap();
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].as_array().unwrap()[0].as_f64(), Some(0.0));
        assert_eq!(points[2].as_array().unwrap()[1].as_f64(), Some(1.0));
    }

    #[test]
    fn counters_number_in_drag_order() {
        let mut document = document(400, 400);
        let mut interaction = EditorInteraction::new();
        interaction.set_tool(Tool::Counter);
        drag(&mut interaction, &mut document, (10.0, 10.0), (40.0, 40.0));
        drag(
            &mut interaction,
            &mut document,
            (100.0, 10.0),
            (130.0, 40.0),
        );
        assert_eq!(document.objects[0].string("text", ""), "1");
        assert_eq!(document.objects[1].string("text", ""), "2");
        assert_eq!(interaction.counter(), 3);
    }

    #[test]
    fn text_waits_for_input_and_commits_once() {
        let mut document = document(400, 300);
        let mut interaction = EditorInteraction::new();
        interaction.set_tool(Tool::Text);
        interaction.press(&mut document, Point::new(20.0, 30.0));
        interaction.move_to(&mut document, Point::new(220.0, 90.0));
        assert!(!interaction.release(&mut document, Point::new(220.0, 90.0)));
        assert!(document.objects.is_empty(), "no empty text annotation");
        assert!(interaction.pending_text().is_some());
        assert!(interaction.commit_text(&mut document, "Hello"));
        assert_eq!(document.objects.len(), 1);
        assert_eq!(document.objects[0].string("text", ""), "Hello");
        document.undo();
        assert!(document.objects.is_empty());
    }

    #[test]
    fn escape_cancels_a_provisional_crop_without_changing_the_document() {
        let mut document = document(400, 300);
        let before = document.render();
        let mut interaction = EditorInteraction::new();
        interaction.set_tool(Tool::Crop);
        drag(
            &mut interaction,
            &mut document,
            (10.0, 10.0),
            (210.0, 160.0),
        );
        assert!(interaction.crop().is_some());
        assert!(interaction.cancel_crop());
        assert!(interaction.crop().is_none());
        assert_eq!(document.render(), before);
        assert!(!document.can_undo() || document.objects.is_empty());
    }

    #[test]
    fn applying_a_crop_is_one_undo_step() {
        let mut document = document(400, 300);
        let mut interaction = EditorInteraction::new();
        interaction.set_tool(Tool::Crop);
        drag(
            &mut interaction,
            &mut document,
            (50.0, 40.0),
            (250.0, 200.0),
        );
        assert!(interaction.apply_crop(&mut document, None).unwrap());
        assert_eq!((document.base.width, document.base.height), (200, 160));
        document.undo();
        assert_eq!((document.base.width, document.base.height), (400, 300));
    }

    #[test]
    fn selection_moves_an_object_as_one_step() {
        let mut document = document(400, 300);
        let mut interaction = EditorInteraction::new();
        interaction.set_tool(Tool::Rect);
        draw_shape(&mut interaction, &mut document, (10.0, 20.0), (110.0, 80.0));
        interaction.set_tool(Tool::Select);
        interaction.press(&mut document, Point::new(30.0, 40.0));
        assert_eq!(interaction.selected(), Some(0));
        interaction.move_to(&mut document, Point::new(60.0, 70.0));
        assert!(interaction.release(&mut document, Point::new(60.0, 70.0)));
        assert_eq!(
            (document.objects[0].x(), document.objects[0].y()),
            (40.0, 50.0)
        );
        document.undo();
        assert_eq!(
            (document.objects[0].x(), document.objects[0].y()),
            (10.0, 20.0)
        );
    }

    #[test]
    fn deleting_the_selection_commits_one_step_and_a_click_misses_otherwise() {
        let mut document = document(100, 100);
        let mut interaction = EditorInteraction::new();
        interaction.set_tool(Tool::Rect);
        draw_shape(&mut interaction, &mut document, (10.0, 10.0), (50.0, 50.0));
        interaction.set_tool(Tool::Select);
        interaction.press(&mut document, Point::new(90.0, 90.0));
        assert_eq!(interaction.selected(), None);
        assert!(!interaction.delete_selected(&mut document));
        interaction.press(&mut document, Point::new(20.0, 20.0));
        assert!(interaction.delete_selected(&mut document));
        assert!(document.objects.is_empty());
        document.undo();
        assert_eq!(document.objects.len(), 1);
    }

    #[test]
    fn key_actions_cover_undo_redo_save_copy_and_tools() {
        assert_eq!(action_for_key("z", true, false), Some(EditorAction::Undo));
        assert_eq!(action_for_key("Z", true, true), Some(EditorAction::Redo));
        assert_eq!(action_for_key("y", true, false), Some(EditorAction::Redo));
        assert_eq!(action_for_key("s", true, false), Some(EditorAction::Save));
        assert_eq!(action_for_key("c", true, false), Some(EditorAction::Copy));
        assert_eq!(
            action_for_key("Delete", false, false),
            Some(EditorAction::Delete)
        );
        assert_eq!(
            action_for_key("Escape", false, false),
            Some(EditorAction::Cancel)
        );
        assert_eq!(
            action_for_key("r", false, false),
            Some(EditorAction::Tool(Tool::Rect))
        );
        assert_eq!(action_for_key("a", true, false), None);
    }

    #[test]
    fn saving_writes_atomically_and_projects_round_trip() {
        let dir = std::env::temp_dir().join(format!("tidyshot-editor-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut document = document(80, 60);
        let mut interaction = EditorInteraction::new();
        interaction.set_tool(Tool::Arrow);
        draw_shape(&mut interaction, &mut document, (5.0, 5.0), (60.0, 40.0));

        let png = dir.join("shot.png");
        save_document(&document, &png).unwrap();
        let reopened = load_document(&png).unwrap();
        assert_eq!((reopened.base.width, reopened.base.height), (80, 60));

        let project = dir.join("shot.omnishot");
        save_document(&document, &project).unwrap();
        let reopened = load_document(&project).unwrap();
        assert_eq!(reopened.objects.len(), 1);
        assert!(is_project(&project));
        assert!(!is_project(&png));
        assert_eq!(display_name(&project), "shot.omnishot");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_suggestion_keeps_the_stem_and_changes_the_extension() {
        let path = std::path::Path::new("/tmp/capture.png");
        assert_eq!(suggested_save_path(Some(path), false), "/tmp/capture.png");
        assert_eq!(
            suggested_save_path(Some(path), true),
            "/tmp/capture.omnishot"
        );
        assert_eq!(suggested_save_path(None, true), "");
    }

    #[test]
    fn a_malformed_project_is_reported_not_opened() {
        let dir = std::env::temp_dir().join(format!("tidyshot-editor-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.omnishot");
        std::fs::write(&path, b"not a zip").unwrap();
        let error = load_document(&path).unwrap_err();
        assert!(!error.message().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
