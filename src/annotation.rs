//! The editable annotation model and its geometry.
// Ported from omnishot/editor.py (Annotation), omnishot/annotation_clipboard.py,
// omnishot/annotation_resize.py, omnishot/image_original.py,
// omnishot/spotlight.py, omnishot/arrow_geometry.py,
// omnishot/rounded_corners.py and omnishot/text_styles.py.
//!
//! An annotation is plain data: a property bag serialized into projects and
//! the annotation clipboard. Geometry is computed over borrowed values so the
//! renderer and the crop session share one definition of every handle.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::json::Value;

/// A point in image space.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// An axis-aligned rectangle in image space.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    #[must_use]
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[must_use]
    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    #[must_use]
    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }

    #[must_use]
    pub fn center(&self) -> Point {
        Point::new(self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    #[must_use]
    pub fn translated(&self, dx: f64, dy: f64) -> Self {
        Self::new(self.x + dx, self.y + dy, self.width, self.height)
    }

    /// The smallest rectangle covering both this and `other`.
    #[must_use]
    pub fn united(&self, other: &Rect) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Self::new(x, y, right - x, bottom - y)
    }

    #[must_use]
    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    #[must_use]
    pub fn to_aligned_rect(&self) -> Self {
        let (l, t) = (self.x.floor(), self.y.floor());
        let (r, b) = (self.right().ceil(), self.bottom().ceil());
        Self::new(l, t, r - l, b - t)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }
}

/// The annotation shapes a document can contain.
pub const KINDS: &[&str] = &[
    "arrow",
    "line",
    "ellipse",
    "rect",
    "fill",
    "pencil",
    "text",
    "counter",
    "highlight",
    "redact",
    "blur",
    "pixelate",
    "spotlight",
    "image",
];

pub const NUMBERS: &[&str] = &[
    "x",
    "y",
    "w",
    "h",
    "width",
    "ax",
    "ay",
    "bx",
    "by",
    "font_size",
    "strength",
    "z",
    "effect_seed",
];

pub const STRINGS: &[&str] = &[
    "color",
    "text",
    "font_family",
    "style",
    "text_style",
    "counter_style",
    "blur_mode",
    "pixelate_mode",
];

pub const FLAGS: &[&str] = &["bold", "italic", "underline", "smooth", "shadow"];

/// The 120 megapixel ceiling shared with screenshots and imports.
pub const MAX_PIXELS: u64 = 120_000_000;

/// The largest annotation clipboard or project document accepted.
pub const MAX_CLIPBOARD_BYTES: usize = 64_000_000;

/// The text presets the toolbar offers.
pub const TEXT_STYLES: &[&str] = &[
    "Standard",
    "Rounded",
    "Monospaced",
    "Outlined",
    "Boxed",
    "Rounded Boxed",
    "Monospaced Boxed",
];

/// A validation failure with the message the UI shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotationError {
    message: String,
}

impl AnnotationError {
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for AnnotationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AnnotationError {}

/// A property bag for one annotation. Property order is stable because the
/// JSON map keeps keys sorted.
pub type Props = BTreeMap<String, Value>;

/// One editable annotation.
#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    pub props: Props,
}

impl Annotation {
    #[must_use]
    pub fn new(props: Props) -> Self {
        Self { props }
    }

    #[must_use]
    pub fn kind(&self) -> Option<&str> {
        self.props.get("kind").and_then(Value::as_str)
    }

    #[must_use]
    pub fn x(&self) -> f64 {
        self.number("x", 0.0)
    }

    #[must_use]
    pub fn y(&self) -> f64 {
        self.number("y", 0.0)
    }

    #[must_use]
    pub fn width(&self) -> f64 {
        self.number("w", 100.0)
    }

    #[must_use]
    pub fn height(&self) -> f64 {
        self.number("h", 40.0)
    }

    #[must_use]
    pub fn z(&self) -> f64 {
        self.number("z", 1.0)
    }

    /// Read a numeric property with a fallback, ignoring wrong types.
    #[must_use]
    pub fn number(&self, key: &str, fallback: f64) -> f64 {
        self.props
            .get(key)
            .and_then(Value::as_f64)
            .unwrap_or(fallback)
    }

    #[must_use]
    pub fn string<'a>(&'a self, key: &str, fallback: &'a str) -> &'a str {
        self.props
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or(fallback)
    }

    /// The serialized form: the property bag with `x`/`y` folded in.
    #[must_use]
    pub fn data_dict(&self) -> Props {
        self.props.clone()
    }

    /// The annotation's local rectangle before any transform.
    #[must_use]
    pub fn rect(&self) -> Rect {
        Rect::new(0.0, 0.0, self.width(), self.height())
    }

    /// The scene-space bounds an annotation contributes to canvas expansion.
    ///
    /// Spotlights never expand the canvas, and image annotations contribute
    /// their full box; every other kind gains a stroke margin.
    // Ported from omnishot/editor.py (Annotation.content_bounds)
    #[must_use]
    pub fn content_bounds(&self) -> Rect {
        let kind = self.kind().unwrap_or("");
        if kind == "spotlight" {
            return self.rect().translated(self.x(), self.y());
        }
        let width = self.number("width", 4.0);
        let margin = width / 2.0 + 1.0;
        let local = if matches!(kind, "arrow" | "line") {
            let (a, b) = self.endpoints();
            let mut bounds = segment_bounds(a, b).adjusted(-margin, -margin, margin, margin);
            if kind == "arrow" {
                let size = 13.0f64.max(width * 4.0);
                bounds = bounds.united(&polygon_bounds(&arrow_head_points(a, b, size)));
                if self.string("style", "standard") == "double" {
                    bounds = bounds.united(&polygon_bounds(&arrow_head_points(b, a, size)));
                }
            }
            bounds
        } else if matches!(kind, "ellipse" | "rect" | "fill" | "pencil") {
            self.rect().adjusted(-margin, -margin, margin, margin)
        } else {
            self.rect()
        };
        local.translated(self.x(), self.y())
    }

    /// The two endpoints of a line or arrow in local coordinates.
    #[must_use]
    pub fn endpoints(&self) -> (Point, Point) {
        let w = self.width();
        let h = self.height();
        (
            Point::new(self.number("ax", 0.0) * w, self.number("ay", 0.0) * h),
            Point::new(self.number("bx", 1.0) * w, self.number("by", 1.0) * h),
        )
    }

    /// The eight resize handles, in local coordinates.
    #[must_use]
    pub fn handles(&self) -> BTreeMap<&'static str, Point> {
        let w = self.width();
        let h = self.height();
        if matches!(self.kind(), Some("line" | "arrow")) {
            let (a, b) = self.endpoints();
            return BTreeMap::from([("a", a), ("b", b)]);
        }
        BTreeMap::from([
            ("lt", Point::new(0.0, 0.0)),
            ("t", Point::new(w / 2.0, 0.0)),
            ("rt", Point::new(w, 0.0)),
            ("r", Point::new(w, h / 2.0)),
            ("rb", Point::new(w, h)),
            ("b", Point::new(w / 2.0, h)),
            ("lb", Point::new(0.0, h)),
            ("l", Point::new(0.0, h / 2.0)),
        ])
    }

    /// True when the annotation carries a drop shadow.
    #[must_use]
    pub fn has_shadow(&self) -> bool {
        self.props
            .get("shadow")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && matches!(
                self.kind(),
                Some(
                    "arrow" | "line" | "ellipse" | "rect" | "fill" | "pencil" | "text" | "counter"
                )
            )
    }

    /// Paint the text into the `h` property when it is too short. Returns the
    /// height that must be stored, or `None` for non-text annotations.
    // Ported from omnishot/editor.py (Annotation.__init__ and text_height)
    #[must_use]
    pub fn normalized_height(&self) -> Option<f64> {
        if self.kind() == Some("text") {
            Some(self.height().max(text_height(&self.props)))
        } else {
            None
        }
    }
}

impl Rect {
    #[must_use]
    pub fn adjusted(&self, dl: f64, dt: f64, dr: f64, db: f64) -> Self {
        Self::new(
            self.x + dl,
            self.y + dt,
            (self.width - dl + dr).max(0.0),
            (self.height - dt + db).max(0.0),
        )
    }
}

/// The bounding box of two points.
fn segment_bounds(a: Point, b: Point) -> Rect {
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    Rect::new(x, y, (a.x - b.x).abs(), (a.y - b.y).abs())
}

fn polygon_bounds(points: &[Point]) -> Rect {
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

/// The three points of an arrow head at `tip`, pointing away from `from`.
// Ported from omnishot/editor.py (Annotation.paint)
#[must_use]
pub fn arrow_head_points(from: Point, tip: Point, size: f64) -> Vec<Point> {
    let angle = (tip.y - from.y).atan2(tip.x - from.x);
    vec![
        tip,
        Point::new(
            tip.x - (angle - 0.5).cos() * size,
            tip.y - (angle - 0.5).sin() * size,
        ),
        Point::new(
            tip.x - (angle + 0.5).cos() * size,
            tip.y - (angle + 0.5).sin() * size,
        ),
    ]
}

/// The six-point tapered arrow outline. Returns an empty polygon when the
/// endpoints coincide.
// Ported from omnishot/arrow_geometry.py (tapered_arrow)
#[must_use]
pub fn tapered_arrow(a: Point, b: Point, width: f64) -> Vec<Point> {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length = (dx * dx + dy * dy).sqrt();
    if length < 0.001 {
        return Vec::new();
    }
    let dir = Point::new(dx / length, dy / length);
    let normal = Point::new(-dir.y, dir.x);
    let head = (13.0f64.max(width * 4.0)).min(length * 0.7);
    let base = Point::new(
        b.x - dir.x * head * 0.5f64.cos(),
        b.y - dir.y * head * 0.5f64.cos(),
    );
    let wing = Point::new(
        normal.x * head * 0.5f64.sin(),
        normal.y * head * 0.5f64.sin(),
    );
    let neck = (width / 2.0).min(head * 0.15);
    let tail = (width * 0.125).min(length * 0.025);
    vec![
        Point::new(a.x - normal.x * tail, a.y - normal.y * tail),
        Point::new(base.x - normal.x * neck, base.y - normal.y * neck),
        Point::new(base.x - wing.x, base.y - wing.y),
        Point::new(b.x, b.y),
        Point::new(base.x + wing.x, base.y + wing.y),
        Point::new(base.x + normal.x * neck, base.y + normal.y * neck),
        Point::new(a.x + normal.x * tail, a.y + normal.y * tail),
    ]
}

/// The spotlight aperture shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotlightShape {
    Rectangle,
    Ellipse,
    RoundedRectangle,
}

/// Resolve a spotlight's shape and corner radius.
// Ported from omnishot/spotlight.py (aperture)
#[must_use]
pub fn spotlight(rect: Rect, props: &Props) -> (SpotlightShape, f64) {
    let shape = props
        .get("spotlight_shape")
        .and_then(Value::as_str)
        .unwrap_or("Rounded rectangle");
    let radius = props
        .get("spotlight_radius")
        .and_then(Value::as_f64)
        .unwrap_or(8.0)
        .max(0.0);
    match shape {
        "Ellipse" => (SpotlightShape::Ellipse, radius),
        "Rectangle" => (SpotlightShape::Rectangle, 0.0),
        _ => (
            SpotlightShape::RoundedRectangle,
            radius.min(rect.width / 2.0).min(rect.height / 2.0),
        ),
    }
}

/// Sample the superellipse outline of a rounded rectangle.
///
/// A power of two is a plain rounded rectangle; other powers trace the
/// Hyprland corner contour. Points are emitted once per corner and the
/// polygon is left open for the caller to close.
// Ported from omnishot/rounded_corners.py (rounded_path)
#[must_use]
pub fn rounded_corner_points(rect: Rect, radius: f64, power: f64, segments: usize) -> Vec<Point> {
    use std::f64::consts::PI;
    let radius = radius.max(0.0).min(rect.width / 2.0).min(rect.height / 2.0);
    let power = power.clamp(1.0, 10.0);
    if radius == 0.0 {
        return vec![
            Point::new(rect.x, rect.y),
            Point::new(rect.right(), rect.y),
            Point::new(rect.right(), rect.bottom()),
            Point::new(rect.x, rect.bottom()),
        ];
    }
    if (power - 2.0).abs() < f64::EPSILON {
        let mut points = Vec::with_capacity(segments * 4 + 4);
        for (cx, cy, start) in [
            (rect.right() - radius, rect.y + radius, -PI / 2.0),
            (rect.right() - radius, rect.bottom() - radius, 0.0),
            (rect.x + radius, rect.bottom() - radius, PI / 2.0),
            (rect.x + radius, rect.y + radius, PI),
        ] {
            for step in 0..=segments {
                let angle = start + step as f64 * PI / (2.0 * segments as f64);
                points.push(Point::new(
                    cx + radius * angle.cos(),
                    cy + radius * angle.sin(),
                ));
            }
        }
        return points;
    }
    let mut points = Vec::with_capacity(segments * 4 + 1);
    for (cx, cy, start) in [
        (rect.right() - radius, rect.y + radius, -PI / 2.0),
        (rect.right() - radius, rect.bottom() - radius, 0.0),
        (rect.x + radius, rect.bottom() - radius, PI / 2.0),
        (rect.x + radius, rect.y + radius, PI),
    ] {
        for step in 0..=segments {
            let angle = start + step as f64 * PI / (2.0 * segments as f64);
            let c = angle.cos();
            let s = angle.sin();
            let cx = cx + radius * c.abs().powf(2.0 / power).copysign(c);
            let cy = cy + radius * s.abs().powf(2.0 / power).copysign(s);
            points.push(Point::new(cx, cy));
        }
    }
    points
}

/// Resize a local rectangle while preserving its opposite anchor.
// Ported from omnishot/annotation_resize.py (resize_rect)
#[must_use]
pub fn resize_rect(width: f64, height: f64, edge: &str, point: Point, proportional: bool) -> Rect {
    let horizontal = edge.contains('l') || edge.contains('r');
    let vertical = edge.contains('t') || edge.contains('b');
    let fixed_x = if edge.contains('l') { width } else { 0.0 };
    let fixed_y = if edge.contains('t') { height } else { 0.0 };
    let dx = if horizontal { point.x - fixed_x } else { width };
    let dy = if vertical { point.y - fixed_y } else { height };
    let sx = if dx < 0.0 || (dx == 0.0 && edge.contains('l')) {
        -1.0
    } else {
        1.0
    };
    let sy = if dy < 0.0 || (dy == 0.0 && edge.contains('t')) {
        -1.0
    } else {
        1.0
    };
    let (w, h) = if proportional {
        let scale = if horizontal && vertical {
            (dx.abs() / width).max(dy.abs() / height)
        } else if horizontal {
            dx.abs() / width
        } else {
            dy.abs() / height
        };
        let scale = scale.max(8.0 / width).max(8.0 / height);
        (width * scale, height * scale)
    } else {
        (
            if horizontal { dx.abs().max(8.0) } else { width },
            if vertical { dy.abs().max(8.0) } else { height },
        )
    };
    let x = if horizontal {
        if sx < 0.0 { fixed_x - w } else { fixed_x }
    } else {
        (width - w) / 2.0
    };
    let y = if vertical {
        if sy < 0.0 { fixed_y - h } else { fixed_y }
    } else {
        (height - h) / 2.0
    };
    Rect::new(x, y, w, h)
}

/// A parsed affine transform `[m11, m12, m21, m22, dx, dy]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform(pub [f64; 6]);

impl Default for Transform {
    fn default() -> Self {
        Self([1.0, 0.0, 0.0, 1.0, 0.0, 0.0])
    }
}

impl Transform {
    #[must_use]
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }

    #[must_use]
    pub fn determinant(&self) -> f64 {
        self.0[0] * self.0[3] - self.0[1] * self.0[2]
    }

    #[must_use]
    pub fn is_invertible(&self) -> bool {
        self.determinant().abs() > f64::EPSILON
    }

    /// Map a point through the transform.
    #[must_use]
    pub fn map(&self, point: Point) -> Point {
        Point::new(
            self.0[0] * point.x + self.0[2] * point.y + self.0[4],
            self.0[1] * point.x + self.0[3] * point.y + self.0[5],
        )
    }

    /// The inverse, or `None` when singular.
    #[must_use]
    pub fn inverted(&self) -> Option<Transform> {
        let det = self.determinant();
        if det.abs() <= f64::EPSILON {
            return None;
        }
        let [a, b, c, d, e, f] = self.0;
        Some(Transform([
            d / det,
            -b / det,
            -c / det,
            a / det,
            (c * f - d * e) / det,
            (b * e - a * f) / det,
        ]))
    }

    /// Compose `self * other`.
    #[must_use]
    pub fn multiply(&self, other: &Transform) -> Transform {
        let [a1, b1, c1, d1, e1, f1] = self.0;
        let [a2, b2, c2, d2, e2, f2] = other.0;
        Transform([
            a1 * a2 + c1 * b2,
            b1 * a2 + d1 * b2,
            a1 * c2 + c1 * d2,
            b1 * c2 + d1 * d2,
            a1 * e2 + c1 * f2 + e1,
            b1 * e2 + d1 * f2 + f1,
        ])
    }

    #[must_use]
    pub fn to_values(&self) -> Value {
        Value::Array(self.0.iter().map(|value| Value::Number(*value)).collect())
    }
}

/// Validate the six-number `source_transform` stored beside an original.
// Ported from omnishot/image_original.py (parse_transform)
pub fn parse_transform(raw: Option<&Value>) -> Result<Transform, AnnotationError> {
    let Some(Value::Array(items)) = raw else {
        return Err(AnnotationError::new("Invalid original image geometry"));
    };
    if items.len() != 6 {
        return Err(AnnotationError::new("Invalid original image geometry"));
    }
    let mut values = [0.0f64; 6];
    for (index, item) in items.iter().enumerate() {
        let Value::Number(number) = item else {
            return Err(AnnotationError::new("Invalid original image geometry"));
        };
        if !number.is_finite() || number.abs() > 1e12 {
            return Err(AnnotationError::new("Invalid original image geometry"));
        }
        values[index] = *number;
    }
    let transform = Transform(values);
    if !transform.is_invertible() {
        return Err(AnnotationError::new("Invalid original image geometry"));
    }
    Ok(transform)
}

/// Map annotations back through canvas edits to the remembered source.
///
/// Returns the original-relative properties. The caller swaps the base image
/// and transform; this is the pure part of `restore_original`.
// Ported from omnishot/image_original.py (restore_original)
pub fn restore_original(
    objects: &[Annotation],
    source: &Transform,
) -> Result<Vec<Props>, AnnotationError> {
    let inverse = source
        .inverted()
        .ok_or_else(|| AnnotationError::new("Cannot restore invalid source geometry"))?;
    let linear = Transform([
        inverse.0[0],
        inverse.0[1],
        inverse.0[2],
        inverse.0[3],
        0.0,
        0.0,
    ]);
    let mut restored = Vec::with_capacity(objects.len());
    for object in objects {
        let mut props = object.data_dict();
        let point = inverse.map(Point::new(object.x(), object.y()));
        props.insert("x".to_owned(), Value::Number(point.x));
        props.insert("y".to_owned(), Value::Number(point.y));
        let existing = object
            .props
            .get("transform")
            .and_then(Value::as_array)
            .and_then(|items| {
                let values: Option<Vec<f64>> = items.iter().map(Value::as_f64).collect();
                values.and_then(|values| <[f64; 6]>::try_from(values).ok())
            })
            .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        props.insert(
            "transform".to_owned(),
            Transform(existing).multiply(&linear).to_values(),
        );
        restored.push(props);
    }
    Ok(restored)
}

/// Approximate the pixel height a wrapped text annotation needs.
///
/// The export path lays glyphs out with Qt; this estimate keeps the model's
/// height field sane before the renderer runs.
// Ported from omnishot/editor.py (text_height) and text_styles.py (height)
#[must_use]
pub fn text_height(props: &Props) -> f64 {
    let font_size = props
        .get("font_size")
        .and_then(Value::as_f64)
        .unwrap_or(28.0)
        .clamp(1.0, 1000.0);
    let width = props
        .get("w")
        .and_then(Value::as_f64)
        .unwrap_or(300.0)
        .max(12.0);
    let text = props.get("text").and_then(Value::as_str).unwrap_or("");
    let average = font_size * 0.6;
    let usable = (width - 12.0).max(average);
    let per_line = (usable / average).floor().max(1.0);
    let mut lines = 1usize;
    for logical in text.split('\n') {
        let characters = logical.chars().count() as f64;
        lines += (characters / per_line).ceil().max(1.0) as usize - 1;
    }
    ((lines as f64 * font_size * 1.25) + 8.0).ceil()
}

/// Validate one annotation property bag.
// Ported from omnishot/annotation_clipboard.py (decode)
pub fn validate_props(props: &Props) -> Result<(), AnnotationError> {
    let Some(kind) = props.get("kind").and_then(Value::as_str) else {
        return Err(AnnotationError::new(
            "Could not paste editable annotations: invalid clipboard data",
        ));
    };
    if !KINDS.contains(&kind) {
        return Err(AnnotationError::new(
            "Could not paste editable annotations: invalid clipboard data",
        ));
    }
    let allowed: BTreeSet<&str> = NUMBERS
        .iter()
        .chain(STRINGS)
        .chain(FLAGS)
        .copied()
        .chain(["kind", "points", "transform", "image"])
        .collect();
    for (key, value) in props {
        if !allowed.contains(key.as_str()) {
            return Err(invalid_clipboard());
        }
        if NUMBERS.contains(&key.as_str()) {
            let limit = if key == "effect_seed" {
                18_446_744_073_709_551_616.0
            } else {
                1_000_000.0
            };
            match value.as_f64() {
                Some(number) if number.is_finite() && number.abs() <= limit => {}
                _ => return Err(invalid_clipboard()),
            }
        } else if STRINGS.contains(&key.as_str()) {
            match value.as_str() {
                Some(text) if text.len() <= 100_000 => {}
                _ => return Err(invalid_clipboard()),
            }
        } else if FLAGS.contains(&key.as_str()) && value.as_bool().is_none() {
            return Err(invalid_clipboard());
        }
    }

    let color = props
        .get("color")
        .and_then(Value::as_str)
        .unwrap_or("#ff5b61");
    if crate::color::Rgba::from_hex(color).is_none() {
        return Err(invalid_clipboard());
    }
    for key in ["w", "h", "width"] {
        if props.get(key).and_then(Value::as_f64).unwrap_or(1.0) < 0.0 {
            return Err(invalid_clipboard());
        }
    }
    for key in ["font_size", "strength"] {
        if props.get(key).and_then(Value::as_f64).unwrap_or(1.0) <= 0.0 {
            return Err(invalid_clipboard());
        }
    }
    let w = props.get("w").and_then(Value::as_f64).unwrap_or(100.0);
    let h = props.get("h").and_then(Value::as_f64).unwrap_or(40.0);
    if w * h > MAX_PIXELS as f64 {
        return Err(invalid_clipboard());
    }
    if props
        .get("font_size")
        .and_then(Value::as_f64)
        .unwrap_or(28.0)
        > 1000.0
        || props.get("width").and_then(Value::as_f64).unwrap_or(4.0) > 1000.0
    {
        return Err(invalid_clipboard());
    }
    if let Some(Value::Array(matrix)) = props.get("transform") {
        if matrix.len() != 6 {
            return Err(invalid_clipboard());
        }
        let values: Option<Vec<f64>> = matrix.iter().map(Value::as_f64).collect();
        let Some(values) = values else {
            return Err(invalid_clipboard());
        };
        let Some(values) = <[f64; 6]>::try_from(values).ok() else {
            return Err(invalid_clipboard());
        };
        if !Transform(values).is_invertible() {
            return Err(invalid_clipboard());
        }
    } else if props.contains_key("transform") {
        return Err(invalid_clipboard());
    }

    if let Some(Value::Array(points)) = props.get("points") {
        if points.len() > 100_000 {
            return Err(invalid_clipboard());
        }
        for point in points {
            match point {
                Value::Array(pair)
                    if pair.len() == 2 && pair.iter().all(|v| v.as_f64().is_some()) => {}
                _ => return Err(invalid_clipboard()),
            }
        }
    } else if props.contains_key("points") {
        return Err(invalid_clipboard());
    }

    if kind == "image" {
        let Some(encoded) = props.get("image").and_then(Value::as_str) else {
            return Err(invalid_clipboard());
        };
        let Some(bytes) = decode_base64(encoded) else {
            return Err(invalid_clipboard());
        };
        let Some((width, height)) = png_dimensions(&bytes) else {
            return Err(invalid_clipboard());
        };
        if u64::from(width) * u64::from(height) > MAX_PIXELS {
            return Err(invalid_clipboard());
        }
    }
    Ok(())
}

fn invalid_clipboard() -> AnnotationError {
    AnnotationError::new("Could not paste editable annotations: invalid clipboard data")
}

/// Decode a version-1 annotation clipboard document.
// Ported from omnishot/annotation_clipboard.py (decode)
pub fn decode_clipboard(data: &[u8]) -> Result<Vec<Annotation>, AnnotationError> {
    if data.len() > MAX_CLIPBOARD_BYTES {
        return Err(invalid_clipboard());
    }
    let text = std::str::from_utf8(data).map_err(|_| invalid_clipboard())?;
    let document = crate::json::parse(text).map_err(|_| invalid_clipboard())?;
    if document.get("version").and_then(Value::as_f64) != Some(1.0) {
        return Err(invalid_clipboard());
    }
    let Some(objects) = document.get("objects").and_then(Value::as_array) else {
        return Err(invalid_clipboard());
    };
    if objects.is_empty() || objects.len() > 1000 {
        return Err(invalid_clipboard());
    }
    let mut parsed = Vec::with_capacity(objects.len());
    for object in objects {
        let Some(map) = object.as_object() else {
            return Err(invalid_clipboard());
        };
        let props: Props = map.clone().into_iter().collect();
        validate_props(&props)?;
        parsed.push(Annotation::new(props));
    }
    Ok(parsed)
}

/// Encode annotations as a version-1 clipboard document.
// Ported from omnishot/annotation_clipboard.py (encode)
pub fn encode_clipboard(objects: &[Annotation]) -> Result<Vec<u8>, AnnotationError> {
    let value = Value::Object(
        [
            ("version".to_owned(), Value::Number(1.0)),
            (
                "objects".to_owned(),
                Value::Array(
                    objects
                        .iter()
                        .map(|object| Value::Object(object.props.clone().into_iter().collect()))
                        .collect(),
                ),
            ),
        ]
        .into_iter()
        .collect(),
    );
    let text = value.to_string_compact();
    let bytes = text.into_bytes();
    // Round-trip so a caller cannot publish a document `decode` rejects.
    decode_clipboard(&bytes)?;
    Ok(bytes)
}

/// Decode standard base64, rejecting non-canonical input.
// Ported from omnishot/annotation_clipboard.py (base64.b64decode validate=True)
#[must_use]
pub fn decode_base64(value: &str) -> Option<Vec<u8>> {
    let bytes = value.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let digit = |b: u8| -> Option<u32> {
        match b {
            b'A'..=b'Z' => Some(u32::from(b - b'A')),
            b'a'..=b'z' => Some(u32::from(b - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(b - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    // Padding is only allowed at the very end, and its count must match the
    // number of data characters the final group carries.
    let first_pad = bytes.iter().position(|b| *b == b'=').unwrap_or(bytes.len());
    if bytes[first_pad..].iter().any(|b| *b != b'=') {
        return None;
    }
    let padding = bytes.len() - first_pad;
    let data = first_pad;
    if data % 4 == 1 || padding != (4 - data % 4) % 4 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut index = 0;
    while index < bytes.len() {
        let chunk = &bytes[index..index + 4];
        let mut values = [0u32; 4];
        for (slot, byte) in chunk.iter().enumerate() {
            values[slot] = if *byte == b'=' { 0 } else { digit(*byte)? };
        }
        out.push(((values[0] << 2) | (values[1] >> 4)) as u8);
        if chunk[2] != b'=' {
            out.push((((values[1] & 0x0f) << 4) | (values[2] >> 2)) as u8);
        }
        if chunk[3] != b'=' {
            out.push((((values[2] & 0x03) << 6) | values[3]) as u8);
        }
        index += 4;
    }
    Some(out)
}

/// Encode bytes as standard base64 with padding.
#[must_use]
pub fn encode_base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let triple = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((triple >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(triple & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Read a PNG's dimensions from its IHDR chunk, when it is a PNG.
#[must_use]
pub fn png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if data.len() < 24 || &data[..8] != SIGNATURE || &data[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(data[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(data[20..24].try_into().ok()?);
    if width == 0 || height == 0 {
        return None;
    }
    Some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(entries: &[(&str, Value)]) -> Props {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    #[test]
    fn proportional_resize_preserves_ratio_and_anchor() {
        // The upstream property test drives every handle from several points.
        for edge in ["lt", "t", "rt", "r", "rb", "b", "lb", "l"] {
            for point in [
                Point::new(-40.0, -30.0),
                Point::new(180.0, 120.0),
                Point::new(30.0, 20.0),
                Point::new(120.0, 80.0),
            ] {
                let r = resize_rect(120.0, 80.0, edge, point, true);
                assert!(r.width >= 8.0 && r.height >= 8.0, "{edge} {point:?}");
                assert!((r.width / r.height - 1.5).abs() < 1e-9, "{edge}");
                if edge.contains('l') {
                    let anchor = if point.x <= 120.0 { r.right() } else { r.x };
                    assert!((anchor - 120.0).abs() < 1e-9, "{edge} {point:?} {r:?}");
                }
                if edge.contains('r') {
                    let anchor = if point.x >= 0.0 { r.x } else { r.right() };
                    assert!(anchor.abs() < 1e-9, "{edge} {point:?} {r:?}");
                }
                if edge.contains('t') {
                    let anchor = if point.y <= 80.0 { r.bottom() } else { r.y };
                    assert!((anchor - 80.0).abs() < 1e-9, "{edge} {point:?} {r:?}");
                }
                if edge.contains('b') {
                    let anchor = if point.y >= 0.0 { r.y } else { r.bottom() };
                    assert!(anchor.abs() < 1e-9, "{edge} {point:?} {r:?}");
                }
                if edge == "t" || edge == "b" {
                    assert!((r.center().x - 60.0).abs() < 1e-9, "{edge}");
                }
                if edge == "l" || edge == "r" {
                    assert!((r.center().y - 40.0).abs() < 1e-9, "{edge}");
                }
            }
        }
    }

    #[test]
    fn shift_vertical_edge_changes_size_instead_of_translating() {
        let top = resize_rect(120.0, 80.0, "t", Point::new(60.0, -40.0), true);
        assert_eq!(
            (top.x, top.y, top.width, top.height),
            (-30.0, -40.0, 180.0, 120.0)
        );
        let bottom = resize_rect(120.0, 80.0, "b", Point::new(60.0, 120.0), true);
        assert_eq!(
            (bottom.x, bottom.y, bottom.width, bottom.height),
            (-30.0, 0.0, 180.0, 120.0)
        );
        let free = resize_rect(120.0, 80.0, "t", Point::new(60.0, -40.0), false);
        assert_eq!(
            (free.x, free.y, free.width, free.height),
            (0.0, -40.0, 120.0, 120.0)
        );
    }

    #[test]
    fn clipboard_round_trip_preserves_geometry_and_styles() {
        let objects = vec![
            Annotation::new(props(&[
                ("kind", Value::String("text".into())),
                ("x", Value::Number(20.0)),
                ("y", Value::Number(25.0)),
                ("w", Value::Number(210.0)),
                ("h", Value::Number(80.0)),
                ("text", Value::String("Editable é\nsecond line".into())),
                ("font_size", Value::Number(22.0)),
                ("bold", Value::Bool(false)),
                ("italic", Value::Bool(true)),
                ("color", Value::String("#ff0033".into())),
                (
                    "transform",
                    Value::Array(vec![
                        Value::Number(1.0),
                        Value::Number(0.2),
                        Value::Number(0.1),
                        Value::Number(1.0),
                        Value::Number(0.0),
                        Value::Number(0.0),
                    ]),
                ),
            ])),
            Annotation::new(props(&[
                ("kind", Value::String("pencil".into())),
                ("x", Value::Number(50.0)),
                (
                    "points",
                    Value::Array(vec![
                        Value::Array(vec![Value::Number(0.0), Value::Number(0.0)]),
                        Value::Array(vec![Value::Number(30.0), Value::Number(0.0)]),
                    ]),
                ),
                ("smooth", Value::Bool(false)),
            ])),
        ];
        let encoded = encode_clipboard(&objects).unwrap();
        let decoded = decode_clipboard(&encoded).unwrap();
        assert_eq!(decoded, objects);
    }

    #[test]
    fn malformed_clipboard_documents_are_rejected() {
        for bad in [
            r#"{"version":1,"objects":[]}"#,
            r#"{"version":2,"objects":[{"kind":"rect"}]}"#,
            r#"{"version":1,"objects":[{"kind":"exec"}]}"#,
            r#"{"version":1,"objects":[{"kind":"rect","w":1e309}]}"#,
            r#"{"version":1,"objects":[{"kind":"text","font_size":"20"}]}"#,
            r#"{"version":1,"objects":[{"kind":"text","bold":"yes"}]}"#,
            r#"{"version":1,"objects":[{"kind":"image","image":"!!!"}]}"#,
            r#"{"version":1,"objects":[{"kind":"pencil","points":[[null,2]]}]}"#,
            r#"{"version":1,"objects":[{"kind":"rect","transform":[0,0,0,0,0,0]}]}"#,
            r#"{"version":1,"objects":[{"kind":"rect","w":20000,"h":20000}]}"#,
            r#"{"version":1,"objects":[{"kind":"rect","color":[]}]}"#,
            r#"{"version":1,"objects":[{"kind":"rect","strength":0}]}"#,
            r#"{"version":1,"objects":[{"kind":"rect","extra":1}]}"#,
        ] {
            assert!(decode_clipboard(bad.as_bytes()).is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn base64_round_trips_and_rejects_non_canonical_input() {
        for data in [
            b"".as_slice(),
            b"f",
            b"fo",
            b"foo",
            b"foob",
            b"fooba",
            b"foobar",
        ] {
            let encoded = encode_base64(data);
            assert_eq!(decode_base64(&encoded).unwrap(), data, "{data:?}");
        }
        assert!(decode_base64("!!!").is_none());
        assert!(decode_base64("AA=A").is_none());
    }

    #[test]
    fn png_dimensions_read_the_ihdr_chunk() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&800u32.to_be_bytes());
        png.extend_from_slice(&600u32.to_be_bytes());
        assert_eq!(png_dimensions(&png), Some((800, 600)));
        assert_eq!(png_dimensions(b"not a png at all........"), None);
    }

    #[test]
    fn transform_parse_and_restore_map_annotations_back() {
        let transform = parse_transform(Some(&Value::Array(vec![
            Value::Number(1.0),
            Value::Number(0.0),
            Value::Number(0.0),
            Value::Number(1.0),
            Value::Number(100.0),
            Value::Number(50.0),
        ])))
        .unwrap();
        let object = Annotation::new(props(&[
            ("kind", Value::String("rect".into())),
            ("x", Value::Number(120.0)),
            ("y", Value::Number(90.0)),
            ("w", Value::Number(10.0)),
            ("h", Value::Number(10.0)),
        ]));
        let restored = restore_original(&[object], &transform).unwrap();
        assert_eq!(restored[0]["x"], Value::Number(20.0));
        assert_eq!(restored[0]["y"], Value::Number(40.0));
        assert!(parse_transform(Some(&Value::Array(vec![Value::Number(0.0); 6]))).is_err());
        assert!(parse_transform(Some(&Value::Number(1.0))).is_err());
    }

    #[test]
    fn tapered_arrow_collapses_for_zero_length() {
        assert!(tapered_arrow(Point::new(1.0, 1.0), Point::new(1.0, 1.0), 4.0).is_empty());
        let arrow = tapered_arrow(Point::new(0.0, 0.0), Point::new(100.0, 0.0), 4.0);
        assert_eq!(arrow.len(), 7);
        assert_eq!(arrow[3], Point::new(100.0, 0.0));
    }

    #[test]
    fn spotlight_reports_shape_and_clamped_radius() {
        let rect = Rect::new(0.0, 0.0, 100.0, 40.0);
        let (shape, radius) = spotlight(rect, &props(&[("spotlight_radius", Value::Number(80.0))]));
        assert_eq!(shape, SpotlightShape::RoundedRectangle);
        assert_eq!(radius, 20.0);
        let (shape, _) = spotlight(
            rect,
            &props(&[("spotlight_shape", Value::String("Ellipse".into()))]),
        );
        assert_eq!(shape, SpotlightShape::Ellipse);
        let (shape, radius) = spotlight(
            rect,
            &props(&[("spotlight_shape", Value::String("Rectangle".into()))]),
        );
        assert_eq!(shape, SpotlightShape::Rectangle);
        assert_eq!(radius, 0.0);
    }

    #[test]
    fn rounded_corners_match_the_superellipse_inside_test() {
        // Points just inside a power-4 corner must be enclosed, points outside
        // must not. The sampled outline stands in for Qt's QPainterPath.
        let power = 4.0f64;
        let radius = 60.0f64;
        let points = rounded_corner_points(Rect::new(10.0, 20.0, 500.0, 300.0), radius, power, 64);
        for (dx, dy) in [(3.0, 3.0), (30.0, 30.0), (50.0, 50.0), (58.0, 1.0)] {
            let distance =
                ((radius - dx).powf(power) + (radius - dy).powf(power)).powf(1.0 / power);
            for (cx, cy) in [
                (10.0 + dx, 20.0 + dy),
                (510.0 - dx, 20.0 + dy),
                (10.0 + dx, 320.0 - dy),
                (510.0 - dx, 320.0 - dy),
            ] {
                let enclosed = point_in_polygon(Point::new(cx, cy), &points);
                assert_eq!(enclosed, distance < radius, "({cx},{cy}) d={distance}");
            }
        }
    }

    fn point_in_polygon(point: Point, polygon: &[Point]) -> bool {
        let mut inside = false;
        let mut j = polygon.len() - 1;
        for i in 0..polygon.len() {
            let (pi, pj) = (polygon[i], polygon[j]);
            if (pi.y > point.y) != (pj.y > point.y)
                && point.x < (pj.x - pi.x) * (point.y - pi.y) / (pj.y - pi.y) + pi.x
            {
                inside = !inside;
            }
            j = i;
        }
        inside
    }

    #[test]
    fn text_height_grows_with_wrapping() {
        let short = text_height(&props(&[
            ("text", Value::String("Hi".into())),
            ("font_size", Value::Number(20.0)),
            ("w", Value::Number(300.0)),
        ]));
        let wrapped = text_height(&props(&[
            ("text", Value::String("word ".repeat(80))),
            ("font_size", Value::Number(20.0)),
            ("w", Value::Number(120.0)),
        ]));
        assert!(wrapped > short);
    }

    #[test]
    fn content_bounds_expand_only_the_expected_kinds() {
        let rect = Annotation::new(props(&[
            ("kind", Value::String("rect".into())),
            ("x", Value::Number(10.0)),
            ("y", Value::Number(20.0)),
            ("w", Value::Number(30.0)),
            ("h", Value::Number(40.0)),
            ("width", Value::Number(4.0)),
        ]));
        assert_eq!(rect.content_bounds(), Rect::new(7.0, 17.0, 36.0, 46.0));
        let spotlight = Annotation::new(props(&[
            ("kind", Value::String("spotlight".into())),
            ("x", Value::Number(10.0)),
            ("y", Value::Number(20.0)),
            ("w", Value::Number(30.0)),
            ("h", Value::Number(40.0)),
        ]));
        assert_eq!(
            spotlight.content_bounds(),
            Rect::new(10.0, 20.0, 30.0, 40.0)
        );
    }
}
