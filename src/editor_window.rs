//! The annotate editor window's Rust side.
// Ported from omnishot/editor.py (EditorWindow state, open/save/copy, key
// handling) and omnishot/app.py (after-capture annotate).
//!
//! The document, renderer and undo stack are the existing [`Document`]; this
//! module only turns QML events into one document operation at a time and
//! publishes the rendered canvas through a QML image provider. There is no
//! second renderer: every visible pixel comes from `render_export`.

use std::path::{Path, PathBuf};

use crate::commands::{CommandRunner, SystemRunner, clipboard_spec};
use crate::editor::Document;
use crate::editor_ui::{
    EditorAction, EditorInteraction, Tool, clipboard_png, display_name, load_document,
    save_document, suggested_save_path, tool_ids, tool_labels, tool_shortcuts,
};

/// Why an editor operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorWindowError {
    message: String,
}

impl EditorWindowError {
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

impl std::fmt::Display for EditorWindowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EditorWindowError {}

/// Where an open request came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenRequest {
    /// A file or project to open.
    Path {
        path: PathBuf,
        /// True when this was the command-line request, so the editor is the
        /// only window and closing it ends the process.
        standalone: bool,
    },
    /// No path: the window asks the user for one.
    Chooser { standalone: bool },
}

impl OpenRequest {
    /// A file requested by the preview or an after-capture action.
    #[must_use]
    pub fn open(path: PathBuf) -> Self {
        Self::Path {
            path,
            standalone: false,
        }
    }

    #[must_use]
    pub fn standalone(&self) -> bool {
        match self {
            Self::Path { standalone, .. } | Self::Chooser { standalone } => *standalone,
        }
    }
}

/// The editor state QML renders.
#[derive(Debug, Default)]
pub struct EditorState {
    document: Option<Document>,
    path: Option<PathBuf>,
    interaction: EditorInteraction,
    dirty: bool,
    revision: u64,
    message: String,
}

impl EditorState {
    /// Open a file as a new document, replacing any unsaved work.
    pub fn open(&mut self, path: &Path) -> Result<(), EditorWindowError> {
        let document = load_document(path)
            .map_err(|error| EditorWindowError::new(error.message().to_owned()))?;
        self.document = Some(document);
        self.path = Some(path.to_path_buf());
        self.interaction = EditorInteraction::new();
        self.dirty = false;
        self.message.clear();
        self.revision += 1;
        Ok(())
    }

    /// Start an empty document over an image already in memory.
    pub fn start_with_image(&mut self, image: crate::pngzip::Image, path: Option<PathBuf>) {
        self.document = Some(Document::new(image, None, false));
        self.path = path;
        self.interaction = EditorInteraction::new();
        self.dirty = false;
        self.message.clear();
        self.revision += 1;
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.document.is_some()
    }

    #[must_use]
    pub fn document(&self) -> Option<&Document> {
        self.document.as_ref()
    }

    /// The document for an interaction the caller drives directly.
    pub fn document_mut(&mut self) -> &mut Document {
        self.document.as_mut().expect("a document is open")
    }

    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    #[must_use]
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub fn interaction(&self) -> &EditorInteraction {
        &self.interaction
    }

    pub fn interaction_mut(&mut self) -> &mut EditorInteraction {
        &mut self.interaction
    }

    /// Run one document edit without copying the canvas out of the state.
    ///
    /// The document is moved into the closure and back, so an image edit never
    /// clones the pixel buffer.
    fn with_document<F>(&mut self, edit: F) -> bool
    where
        F: FnOnce(&mut Document, &mut EditorInteraction) -> bool,
    {
        let Some(mut document) = self.document.take() else {
            return false;
        };
        let changed = edit(&mut document, &mut self.interaction);
        self.document = Some(document);
        if changed {
            self.dirty = true;
            self.revision += 1;
        }
        changed
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn canvas_size(&self) -> (u32, u32) {
        self.document.as_ref().map_or((0, 0), |document| {
            (document.base.width as u32, document.base.height as u32)
        })
    }

    #[must_use]
    pub fn title(&self) -> String {
        let name = self
            .path
            .as_deref()
            .map_or_else(|| "Untitled".to_owned(), display_name);
        if self.dirty {
            format!("TidyShot Editor — {name} *")
        } else {
            format!("TidyShot Editor — {name}")
        }
    }

    /// The rendered canvas the image provider serves.
    #[must_use]
    pub fn rendered(&self) -> Option<crate::pngzip::Image> {
        self.document.as_ref().map(Document::render_export)
    }

    /// Re-render after a document change.
    pub fn changed(&mut self, dirty: bool) {
        if dirty {
            self.dirty = true;
        }
        self.revision += 1;
    }

    /// Save over the opened path, or to `path` when given.
    pub fn save(&mut self, path: Option<&Path>) -> Result<PathBuf, EditorWindowError> {
        let Some(document) = self.document.as_ref() else {
            return Err(EditorWindowError::new("No image is open"));
        };
        let destination = path
            .map(Path::to_path_buf)
            .or_else(|| self.path.clone())
            .ok_or_else(|| EditorWindowError::new("Choose where to save the image"))?;
        save_document(document, &destination)
            .map_err(|error| EditorWindowError::new(error.message().to_owned()))?;
        self.path = Some(destination.clone());
        self.dirty = false;
        self.message = format!("Saved {}", display_name(&destination));
        self.revision += 1;
        Ok(destination)
    }

    /// The path a Save As dialog should start from.
    #[must_use]
    pub fn save_suggestion(&self, project: bool) -> String {
        suggested_save_path(self.path.as_deref(), project)
    }

    /// Render and write the canvas to the clipboard.
    pub fn copy<R: CommandRunner>(&mut self, runner: &R) -> Result<(), EditorWindowError> {
        let Some(document) = self.document.as_ref() else {
            return Err(EditorWindowError::new("No image is open"));
        };
        let png = clipboard_png(document);
        runner
            .run(&clipboard_spec("image/png", png))
            .map_err(|error| EditorWindowError::new(error.to_string()))?;
        self.message = "Copied to the clipboard".to_owned();
        Ok(())
    }

    /// Apply a key press the window receives; true when it was handled.
    pub fn key(&mut self, key: &str, ctrl: bool, shift: bool) -> bool {
        let Some(action) = crate::editor_ui::action_for_key(key, ctrl, shift) else {
            return false;
        };
        match action {
            EditorAction::Undo => self.undo(),
            EditorAction::Redo => self.redo(),
            EditorAction::Save => {
                if let Err(error) = self.save(None) {
                    self.message = error.to_string();
                }
            }
            EditorAction::Copy => {
                if let Err(error) = self.copy(&SystemRunner) {
                    self.message = error.to_string();
                }
            }
            EditorAction::Delete => self.delete_selection(),
            EditorAction::Cancel => {
                if !self.interaction.cancel_crop() && !self.interaction.cancel_text() {
                    self.interaction.cancel_drag();
                }
            }
            EditorAction::Apply => {
                let _ = self.apply_crop();
            }
            EditorAction::Tool(tool) => self.interaction.set_tool(tool),
        }
        true
    }

    /// Apply the provisional crop as one undo step.
    pub fn apply_crop(&mut self) -> Result<bool, EditorWindowError> {
        if self.interaction.crop().is_none() {
            return Err(EditorWindowError::new("Draw a crop before applying it"));
        }
        let Some(mut document) = self.document.take() else {
            return Err(EditorWindowError::new("No image is open"));
        };
        let result = self.interaction.apply_crop(&mut document, None);
        self.document = Some(document);
        match result {
            Ok(changed) => {
                if changed {
                    self.dirty = true;
                    self.revision += 1;
                }
                Ok(changed)
            }
            Err(error) => Err(EditorWindowError::new(error.to_string())),
        }
    }

    pub fn undo(&mut self) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        if document.can_undo() {
            document.undo();
            self.dirty = true;
            self.revision += 1;
        }
    }

    pub fn redo(&mut self) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        if document.can_redo() {
            document.redo();
            self.dirty = true;
            self.revision += 1;
        }
    }

    pub fn delete_selection(&mut self) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        if self.interaction.delete_selected(document) {
            self.dirty = true;
            self.revision += 1;
        }
    }
}

/// A published open request that waits for QML to bind.
static PENDING: std::sync::Mutex<Option<OpenRequest>> = std::sync::Mutex::new(None);
static BOUND: std::sync::OnceLock<cxx_qt::CxxQtThread<qobject::EditorApi>> =
    std::sync::OnceLock::new();

/// Ask the running editor (or the next one) to open a file.
///
/// A request that arrives before QML binds came from the command line, so the
/// editor is the only window and closing it ends the process.
pub fn publish_open(path: PathBuf) {
    publish(OpenRequest::Path {
        path,
        standalone: BOUND.get().is_none(),
    });
}

/// Ask the editor to show its file chooser.
pub fn publish_chooser() {
    publish(OpenRequest::Chooser {
        standalone: BOUND.get().is_none(),
    });
}

fn publish(request: OpenRequest) {
    match BOUND.get() {
        Some(thread) => {
            let _ = thread.queue(move |mut editor| {
                editor.as_mut().apply_request(request);
            });
        }
        None => *PENDING.lock().expect("editor queue poisoned") = Some(request),
    }
}

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
        include!("cxx-qt-lib/qurl.h");
        type QUrl = cxx_qt_lib::QUrl;
    }

    extern "RustQt" {
        /// The annotate editor window's document and interaction state.
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(bool, open)]
        #[qproperty(bool, standalone)]
        #[qproperty(bool, chooser_open, cxx_name = "chooserOpen")]
        #[qproperty(QString, title)]
        #[qproperty(QString, path)]
        #[qproperty(QString, message)]
        #[qproperty(bool, dirty)]
        #[qproperty(i64, revision)]
        #[qproperty(i32, canvas_width, cxx_name = "canvasWidth")]
        #[qproperty(i32, canvas_height, cxx_name = "canvasHeight")]
        #[qproperty(QStringList, tool_ids, cxx_name = "toolIds")]
        #[qproperty(QStringList, tool_labels, cxx_name = "toolLabels")]
        #[qproperty(QStringList, tool_shortcuts, cxx_name = "toolShortcuts")]
        #[qproperty(QString, tool)]
        #[qproperty(QString, color)]
        #[qproperty(f64, stroke)]
        #[qproperty(bool, can_undo, cxx_name = "canUndo")]
        #[qproperty(bool, can_redo, cxx_name = "canRedo")]
        #[qproperty(bool, crop_active, cxx_name = "cropActive")]
        #[qproperty(f64, crop_x, cxx_name = "cropX")]
        #[qproperty(f64, crop_y, cxx_name = "cropY")]
        #[qproperty(f64, crop_w, cxx_name = "cropW")]
        #[qproperty(f64, crop_h, cxx_name = "cropH")]
        #[qproperty(bool, text_active, cxx_name = "textActive")]
        #[qproperty(bool, has_selection, cxx_name = "hasSelection")]
        #[qproperty(f64, selection_x, cxx_name = "selectionX")]
        #[qproperty(f64, selection_y, cxx_name = "selectionY")]
        #[qproperty(f64, selection_w, cxx_name = "selectionW")]
        #[qproperty(f64, selection_h, cxx_name = "selectionH")]
        #[qproperty(bool, provisional)]
        #[qproperty(QString, provisional_kind, cxx_name = "provisionalKind")]
        #[qproperty(f64, provisional_x, cxx_name = "provisionalX")]
        #[qproperty(f64, provisional_y, cxx_name = "provisionalY")]
        #[qproperty(f64, provisional_w, cxx_name = "provisionalW")]
        #[qproperty(f64, provisional_h, cxx_name = "provisionalH")]
        #[qproperty(f64, text_x, cxx_name = "textX")]
        #[qproperty(f64, text_y, cxx_name = "textY")]
        #[qproperty(f64, text_w, cxx_name = "textW")]
        #[qproperty(f64, text_h, cxx_name = "textH")]
        #[qproperty(QString, save_suggestion, cxx_name = "saveSuggestion")]
        #[qproperty(f64, ui_scale, cxx_name = "uiScale")]
        #[cxx_name = "EditorApi"]
        type EditorApi = super::EditorRust;

        /// Register this object and replay a queued open request.
        #[qinvokable]
        fn bind(self: Pin<&mut Self>);

        /// Open a file or `.omnishot` project.
        #[qinvokable]
        #[cxx_name = "openPath"]
        fn open_path(self: Pin<&mut Self>, path: &QString);

        /// Open the local file a dialog URL names.
        #[qinvokable]
        #[cxx_name = "openUrl"]
        fn open_url(self: Pin<&mut Self>, url: &QUrl);

        /// Show the file chooser.
        #[qinvokable]
        #[cxx_name = "openChooser"]
        fn open_chooser(self: Pin<&mut Self>);

        /// Close the chooser after a choice or cancellation.
        #[qinvokable]
        #[cxx_name = "closeChooser"]
        fn close_chooser(self: Pin<&mut Self>);

        /// Save over the opened path.
        #[qinvokable]
        fn save(self: Pin<&mut Self>);

        /// Save to a chosen path.
        #[qinvokable]
        #[cxx_name = "saveAs"]
        fn save_as(self: Pin<&mut Self>, path: &QString);

        /// Save to the local file a dialog URL names.
        #[qinvokable]
        #[cxx_name = "saveUrl"]
        fn save_url(self: Pin<&mut Self>, url: &QUrl);

        /// Copy the rendered canvas to the clipboard.
        #[qinvokable]
        fn copy(self: Pin<&mut Self>);

        /// Arm a tool by id.
        #[qinvokable]
        #[cxx_name = "chooseTool"]
        fn choose_tool(self: Pin<&mut Self>, tool: &QString);

        /// Set the annotation color.
        #[qinvokable]
        #[cxx_name = "chooseColor"]
        fn choose_color(self: Pin<&mut Self>, color: &QString);

        /// Set the stroke width.
        #[qinvokable]
        #[cxx_name = "chooseStroke"]
        fn choose_stroke(self: Pin<&mut Self>, stroke: f64);

        /// Begin a drag at canvas coordinates.
        #[qinvokable]
        fn press(self: Pin<&mut Self>, x: f64, y: f64);

        /// Update the drag.
        #[qinvokable]
        #[cxx_name = "moveTo"]
        fn move_to(self: Pin<&mut Self>, x: f64, y: f64);

        /// Finish the drag.
        #[qinvokable]
        fn release(self: Pin<&mut Self>, x: f64, y: f64);

        /// Finish a pending text annotation.
        #[qinvokable]
        #[cxx_name = "commitText"]
        fn commit_text(self: Pin<&mut Self>, text: &QString);

        /// Drop a pending text annotation.
        #[qinvokable]
        #[cxx_name = "cancelText"]
        fn cancel_text(self: Pin<&mut Self>);

        /// Apply the provisional crop.
        #[qinvokable]
        #[cxx_name = "applyCrop"]
        fn apply_crop(self: Pin<&mut Self>);

        /// Drop the provisional crop.
        #[qinvokable]
        #[cxx_name = "cancelCrop"]
        fn cancel_crop(self: Pin<&mut Self>);

        /// Undo the last document change.
        #[qinvokable]
        fn undo(self: Pin<&mut Self>);

        /// Redo the last undone change.
        #[qinvokable]
        fn redo(self: Pin<&mut Self>);

        /// Delete the selected annotation.
        #[qinvokable]
        #[cxx_name = "deleteSelected"]
        fn delete_selected(self: Pin<&mut Self>);

        /// Handle a key the window received; false when unbound.
        #[qinvokable]
        fn key(self: Pin<&mut Self>, key: &QString, ctrl: bool, shift: bool) -> bool;

        /// Close the window, discarding unsaved work.
        #[qinvokable]
        fn discard(self: Pin<&mut Self>);

        /// Clear the status message.
        #[qinvokable]
        #[cxx_name = "clearMessage"]
        fn clear_message(self: Pin<&mut Self>);
    }

    impl cxx_qt::Threading for EditorApi {}
}

use core::pin::Pin;
use cxx_qt::CxxQtType as _;
use cxx_qt::Threading as _;
use cxx_qt_lib::{QString, QStringList};

/// Rust state behind the QML `Editor` singleton.
pub struct EditorRust {
    open: bool,
    standalone: bool,
    chooser_open: bool,
    title: QString,
    path: QString,
    message: QString,
    dirty: bool,
    revision: i64,
    canvas_width: i32,
    canvas_height: i32,
    tool_ids: QStringList,
    tool_labels: QStringList,
    tool_shortcuts: QStringList,
    tool: QString,
    color: QString,
    stroke: f64,
    can_undo: bool,
    can_redo: bool,
    crop_active: bool,
    crop_x: f64,
    crop_y: f64,
    crop_w: f64,
    crop_h: f64,
    text_active: bool,
    has_selection: bool,
    selection_x: f64,
    selection_y: f64,
    selection_w: f64,
    selection_h: f64,
    provisional: bool,
    provisional_kind: QString,
    provisional_x: f64,
    provisional_y: f64,
    provisional_w: f64,
    provisional_h: f64,
    text_x: f64,
    text_y: f64,
    text_w: f64,
    text_h: f64,
    save_suggestion: QString,
    ui_scale: f64,
    published: i64,
    state: EditorState,
}

impl Default for EditorRust {
    fn default() -> Self {
        let mut rust = Self {
            open: false,
            standalone: false,
            chooser_open: false,
            title: QString::from("TidyShot Editor"),
            path: QString::from(""),
            message: QString::from(""),
            dirty: false,
            revision: 0,
            canvas_width: 0,
            canvas_height: 0,
            tool_ids: strings(tool_ids()),
            tool_labels: strings(tool_labels()),
            tool_shortcuts: strings(tool_shortcuts()),
            tool: QString::from("select"),
            color: QString::from("#ff5b61"),
            stroke: 4.0,
            can_undo: false,
            can_redo: false,
            crop_active: false,
            crop_x: 0.0,
            crop_y: 0.0,
            crop_w: 0.0,
            crop_h: 0.0,
            text_active: false,
            has_selection: false,
            selection_x: 0.0,
            selection_y: 0.0,
            selection_w: 0.0,
            selection_h: 0.0,
            provisional: false,
            provisional_kind: QString::from(""),
            provisional_x: 0.0,
            provisional_y: 0.0,
            provisional_w: 0.0,
            provisional_h: 0.0,
            text_x: 0.0,
            text_y: 0.0,
            text_w: 0.0,
            text_h: 0.0,
            save_suggestion: QString::from(""),
            ui_scale: crate::ui_scale::read(None)
                .unwrap_or_default()
                .factor()
                .max(0.5),
            published: -1,
            state: EditorState::default(),
        };
        // A command-line open arrives before QML loads; applying it here means
        // the window maps with its document already rendered.
        if let Some(request) = PENDING.lock().expect("editor queue poisoned").take() {
            apply_request(&mut rust, request);
        }
        rust
    }
}

/// The local path a dialog URL names, or `None` for a remote URL.
fn local_path(url: &cxx_qt_lib::QUrl) -> Option<String> {
    url.to_local_file().map(|path| path.to_string())
}

fn strings(values: Vec<String>) -> QStringList {
    let mut list = QStringList::default();
    for value in values {
        list.append(QString::from(value));
    }
    list
}

fn apply_request(rust: &mut EditorRust, request: OpenRequest) {
    // Opening another file must not silently discard unsaved annotations; the
    // open file stays until the user saves or closes it.
    if rust.state.dirty() && rust.state.is_open() {
        rust.open = true;
        rust.state.message =
            "Save or discard the current edits before opening another file".to_owned();
        rust.message = QString::from(rust.state.message.clone());
        return;
    }
    rust.standalone = request.standalone();
    match request {
        OpenRequest::Path { path, .. } => match rust.state.open(&path) {
            Ok(()) => {
                rust.open = true;
                rust.chooser_open = false;
            }
            Err(error) => {
                rust.open = true;
                rust.message = QString::from(error.to_string());
            }
        },
        OpenRequest::Chooser { .. } => {
            rust.open = true;
            rust.chooser_open = true;
        }
    }
}

impl EditorRust {
    /// Pull the state fields from [`EditorState`] before publishing them.
    fn refresh(&mut self) {
        self.title = QString::from(self.state.title());
        self.path = QString::from(
            self.state
                .path()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        );
        self.message = QString::from(self.state.message());
        self.dirty = self.state.dirty();
        self.revision = self.state.revision() as i64;
        let (width, height) = self.state.canvas_size();
        self.canvas_width = width as i32;
        self.canvas_height = height as i32;
        let interaction = self.state.interaction();
        self.tool = QString::from(interaction.tool().id());
        self.color = QString::from(crate::color::color_text(interaction.color()));
        self.stroke = interaction.stroke();
        self.crop_active = interaction.crop().is_some();
        if let Some(rect) = interaction.crop() {
            self.crop_x = rect.x;
            self.crop_y = rect.y;
            self.crop_w = rect.width;
            self.crop_h = rect.height;
        }
        self.text_active = interaction.pending_text().is_some();
        if let Some(pending) = interaction.pending_text() {
            self.text_x = pending.annotation.x();
            self.text_y = pending.annotation.y();
            self.text_w = pending.annotation.width();
            self.text_h = pending.annotation.height();
        }
        self.can_undo = self
            .state
            .document()
            .is_some_and(crate::editor::Document::can_undo);
        self.can_redo = self
            .state
            .document()
            .is_some_and(crate::editor::Document::can_redo);
        let selection = self
            .state
            .document()
            .and_then(|document| interaction.selection_rect(document));
        self.has_selection = selection.is_some();
        if let Some(rect) = selection {
            self.selection_x = rect.x;
            self.selection_y = rect.y;
            self.selection_w = rect.width;
            self.selection_h = rect.height;
        }
        let provisional = interaction.provisional();
        self.provisional = provisional.is_some();
        if let Some(shape) = provisional {
            self.provisional_kind = QString::from(shape.kind);
            self.provisional_x = shape.rect.x;
            self.provisional_y = shape.rect.y;
            self.provisional_w = shape.rect.width;
            self.provisional_h = shape.rect.height;
        } else {
            self.provisional_kind = QString::from("");
        }
        self.save_suggestion = QString::from(self.state.save_suggestion(false));
    }

    /// Publish the rendered canvas when the document changed.
    ///
    /// A drag only moves an overlay in QML, so the full render is skipped
    /// until the document revision advances.
    fn publish_image(&mut self) {
        if !self.state.is_open() || self.published == self.state.revision() as i64 {
            return;
        }
        if let Some(image) = self.state.rendered() {
            crate::editor_image::set_image(&image.data, image.width as i32, image.height as i32);
            self.published = self.state.revision() as i64;
        }
    }
}

impl qobject::EditorApi {
    /// Copy every field onto the QObject properties.
    fn sync(mut self: Pin<&mut Self>) {
        let snapshot = {
            let rust = self.as_mut().rust_mut().get_mut();
            rust.refresh();
            rust.publish_image();
            (
                rust.open,
                rust.standalone,
                rust.chooser_open,
                rust.title.clone(),
                rust.path.clone(),
                rust.message.clone(),
                rust.dirty,
                rust.revision,
                rust.canvas_width,
                rust.canvas_height,
                rust.tool.clone(),
                rust.color.clone(),
                rust.stroke,
                rust.can_undo,
                rust.can_redo,
                rust.crop_active,
                rust.crop_x,
                rust.crop_y,
                rust.crop_w,
                rust.crop_h,
                rust.text_active,
                rust.has_selection,
                rust.selection_x,
                rust.selection_y,
                rust.selection_w,
                rust.selection_h,
                rust.provisional,
                rust.provisional_kind.clone(),
                rust.provisional_x,
                rust.provisional_y,
                rust.provisional_w,
                rust.provisional_h,
                rust.text_x,
                rust.text_y,
                rust.text_w,
                rust.text_h,
                rust.save_suggestion.clone(),
                rust.ui_scale,
            )
        };
        self.as_mut().set_open(snapshot.0);
        self.as_mut().set_standalone(snapshot.1);
        self.as_mut().set_chooser_open(snapshot.2);
        self.as_mut().set_title(snapshot.3);
        self.as_mut().set_path(snapshot.4);
        self.as_mut().set_message(snapshot.5);
        self.as_mut().set_dirty(snapshot.6);
        self.as_mut().set_revision(snapshot.7);
        self.as_mut().set_canvas_width(snapshot.8);
        self.as_mut().set_canvas_height(snapshot.9);
        self.as_mut().set_tool(snapshot.10);
        self.as_mut().set_color(snapshot.11);
        self.as_mut().set_stroke(snapshot.12);
        self.as_mut().set_can_undo(snapshot.13);
        self.as_mut().set_can_redo(snapshot.14);
        self.as_mut().set_crop_active(snapshot.15);
        self.as_mut().set_crop_x(snapshot.16);
        self.as_mut().set_crop_y(snapshot.17);
        self.as_mut().set_crop_w(snapshot.18);
        self.as_mut().set_crop_h(snapshot.19);
        self.as_mut().set_text_active(snapshot.20);
        self.as_mut().set_has_selection(snapshot.21);
        self.as_mut().set_selection_x(snapshot.22);
        self.as_mut().set_selection_y(snapshot.23);
        self.as_mut().set_selection_w(snapshot.24);
        self.as_mut().set_selection_h(snapshot.25);
        self.as_mut().set_provisional(snapshot.26);
        self.as_mut().set_provisional_kind(snapshot.27);
        self.as_mut().set_provisional_x(snapshot.28);
        self.as_mut().set_provisional_y(snapshot.29);
        self.as_mut().set_provisional_w(snapshot.30);
        self.as_mut().set_provisional_h(snapshot.31);
        self.as_mut().set_text_x(snapshot.32);
        self.as_mut().set_text_y(snapshot.33);
        self.as_mut().set_text_w(snapshot.34);
        self.as_mut().set_text_h(snapshot.35);
        self.as_mut().set_save_suggestion(snapshot.36);
        self.as_mut().set_ui_scale(snapshot.37);
    }

    /// Register this object and replay a queued open request.
    pub fn bind(mut self: Pin<&mut Self>) {
        let pending = PENDING.lock().expect("editor queue poisoned").take();
        let _ = BOUND.set(self.qt_thread());
        if let Some(request) = pending {
            self.as_mut().apply_request(request);
        }
        self.as_mut().sync();
    }

    /// Open a file or project, reporting a failure on the window.
    pub fn open_path(mut self: Pin<&mut Self>, path: &QString) {
        self.as_mut()
            .apply_request(OpenRequest::open(PathBuf::from(path.to_string())));
    }

    /// Open the local file a dialog URL names.
    pub fn open_url(mut self: Pin<&mut Self>, url: &cxx_qt_lib::QUrl) {
        let Some(path) = local_path(url) else {
            self.as_mut().rust_mut().get_mut().state.message =
                "Choose a local image or project".to_owned();
            self.as_mut().sync();
            return;
        };
        self.as_mut()
            .apply_request(OpenRequest::open(PathBuf::from(path)));
    }

    /// Show the file chooser.
    pub fn open_chooser(mut self: Pin<&mut Self>) {
        self.as_mut().set_open(true);
        self.as_mut().set_chooser_open(true);
    }

    /// Close the chooser after a choice or cancellation.
    pub fn close_chooser(mut self: Pin<&mut Self>) {
        self.as_mut().set_chooser_open(false);
    }

    /// Save over the opened path.
    pub fn save(mut self: Pin<&mut Self>) {
        let result = self.as_mut().rust_mut().get_mut().state.save(None);
        if let Err(error) = result {
            self.as_mut().rust_mut().get_mut().state.message = error.to_string();
        }
        self.as_mut().sync();
    }

    /// Save to a chosen path.
    pub fn save_as(mut self: Pin<&mut Self>, path: &QString) {
        let path = PathBuf::from(path.to_string());
        let result = self.as_mut().rust_mut().get_mut().state.save(Some(&path));
        if let Err(error) = result {
            self.as_mut().rust_mut().get_mut().state.message = error.to_string();
        }
        self.as_mut().sync();
    }

    /// Save to the local file a dialog URL names.
    pub fn save_url(mut self: Pin<&mut Self>, url: &cxx_qt_lib::QUrl) {
        let Some(path) = local_path(url) else {
            self.as_mut().rust_mut().get_mut().state.message =
                "Choose a local file to save".to_owned();
            self.as_mut().sync();
            return;
        };
        self.as_mut().save_as(&QString::from(path));
    }

    /// Copy the rendered canvas to the clipboard.
    pub fn copy(mut self: Pin<&mut Self>) {
        let result = self.as_mut().rust_mut().get_mut().state.copy(&SystemRunner);
        if let Err(error) = result {
            self.as_mut().rust_mut().get_mut().state.message = error.to_string();
        }
        self.as_mut().sync();
    }

    /// Arm a tool by id.
    pub fn choose_tool(mut self: Pin<&mut Self>, tool: &QString) {
        self.as_mut()
            .rust_mut()
            .get_mut()
            .state
            .interaction_mut()
            .set_tool(Tool::from_id(&tool.to_string()));
        self.as_mut().sync();
    }

    /// Set the annotation color.
    pub fn choose_color(mut self: Pin<&mut Self>, color: &QString) {
        if let Some(color) = crate::color::Rgba::from_hex(&color.to_string()) {
            self.as_mut()
                .rust_mut()
                .get_mut()
                .state
                .interaction_mut()
                .set_color(color);
        }
        self.as_mut().sync();
    }

    /// Set the stroke width.
    pub fn choose_stroke(mut self: Pin<&mut Self>, stroke: f64) {
        self.as_mut()
            .rust_mut()
            .get_mut()
            .state
            .interaction_mut()
            .set_stroke(stroke);
        self.as_mut().sync();
    }

    /// Begin a drag at canvas coordinates.
    pub fn press(mut self: Pin<&mut Self>, x: f64, y: f64) {
        self.as_mut()
            .rust_mut()
            .get_mut()
            .state
            .with_document(|document, interaction| {
                interaction.press(document, crate::annotation::Point::new(x, y));
                false
            });
        self.as_mut().sync();
    }

    /// Update the drag.
    pub fn move_to(mut self: Pin<&mut Self>, x: f64, y: f64) {
        self.as_mut()
            .rust_mut()
            .get_mut()
            .state
            .with_document(|document, interaction| {
                interaction.move_to(document, crate::annotation::Point::new(x, y));
                false
            });
        self.as_mut().sync();
    }

    /// Finish the drag and commit its one document change.
    pub fn release(mut self: Pin<&mut Self>, x: f64, y: f64) {
        self.as_mut()
            .rust_mut()
            .get_mut()
            .state
            .with_document(|document, interaction| {
                interaction.release(document, crate::annotation::Point::new(x, y))
            });
        self.as_mut().sync();
    }

    /// Finish a pending text annotation.
    pub fn commit_text(mut self: Pin<&mut Self>, text: &QString) {
        self.as_mut()
            .rust_mut()
            .get_mut()
            .state
            .with_document(|document, interaction| {
                interaction.commit_text(document, &text.to_string())
            });
        self.as_mut().sync();
    }

    /// Drop a pending text annotation.
    pub fn cancel_text(mut self: Pin<&mut Self>) {
        self.as_mut()
            .rust_mut()
            .get_mut()
            .state
            .interaction_mut()
            .cancel_text();
        self.as_mut().sync();
    }

    /// Apply the provisional crop.
    pub fn apply_crop(mut self: Pin<&mut Self>) {
        let error = {
            let state = &mut self.as_mut().rust_mut().get_mut().state;
            state.apply_crop().err().map(|error| error.to_string())
        };
        if let Some(error) = error {
            self.as_mut().rust_mut().get_mut().state.message = error;
        }
        self.as_mut().sync();
    }

    /// Drop the provisional crop without changing the document.
    pub fn cancel_crop(mut self: Pin<&mut Self>) {
        self.as_mut()
            .rust_mut()
            .get_mut()
            .state
            .interaction_mut()
            .cancel_crop();
        self.as_mut().sync();
    }

    /// Undo the last document change.
    pub fn undo(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().get_mut().state.undo();
        self.as_mut().sync();
    }

    /// Redo the last undone change.
    pub fn redo(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().get_mut().state.redo();
        self.as_mut().sync();
    }

    /// Delete the selected annotation.
    pub fn delete_selected(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().get_mut().state.delete_selection();
        self.as_mut().sync();
    }

    /// Handle a key the window received.
    pub fn key(mut self: Pin<&mut Self>, key: &QString, ctrl: bool, shift: bool) -> bool {
        let handled = self
            .as_mut()
            .rust_mut()
            .get_mut()
            .state
            .key(&key.to_string(), ctrl, shift);
        if handled {
            self.as_mut().sync();
        }
        handled
    }

    /// Close the window, discarding unsaved work.
    pub fn discard(mut self: Pin<&mut Self>) {
        self.as_mut().set_open(false);
        self.as_mut().set_chooser_open(false);
        self.as_mut().set_dirty(false);
    }

    /// Clear the status message.
    pub fn clear_message(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().get_mut().state.message.clear();
        self.as_mut().sync();
    }

    fn apply_request(mut self: Pin<&mut Self>, request: OpenRequest) {
        let rust = self.as_mut().rust_mut().get_mut();
        apply_request(rust, request);
        self.as_mut().sync();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor_ui::Tool;
    use crate::pngzip::Image;

    fn state(width: usize, height: usize) -> EditorState {
        let mut state = EditorState::default();
        let data = vec![180u8; width * height * 4];
        state.start_with_image(Image::new(width, height, data), None);
        state
    }

    #[test]
    fn a_new_document_is_open_clean_and_untitled() {
        let state = state(80, 60);
        assert!(state.is_open());
        assert!(!state.dirty());
        assert_eq!(state.canvas_size(), (80, 60));
        assert_eq!(state.title(), "TidyShot Editor — Untitled");
        assert!(state.path().is_none());
    }

    #[test]
    fn one_drag_marks_dirty_and_undo_clears_the_pixels() {
        let mut state = state(200, 200);
        let before = state.rendered().unwrap();
        state.interaction_mut().set_tool(Tool::Rect);
        state.with_document(|document, interaction| {
            interaction.press(document, crate::annotation::Point::new(10.0, 10.0));
            interaction.move_to(document, crate::annotation::Point::new(80.0, 60.0));
            interaction.release(document, crate::annotation::Point::new(80.0, 60.0))
        });
        assert!(state.dirty());
        assert_ne!(state.rendered().unwrap(), before);
        state.undo();
        assert_eq!(state.rendered().unwrap(), before);
        state.redo();
        assert_ne!(state.rendered().unwrap(), before);
    }

    #[test]
    fn escape_cancels_a_crop_then_a_drag() {
        let mut state = state(200, 200);
        state.interaction_mut().set_tool(Tool::Crop);
        state.with_document(|document, interaction| {
            interaction.press(document, crate::annotation::Point::new(10.0, 10.0));
            interaction.move_to(document, crate::annotation::Point::new(100.0, 100.0));
            interaction.release(document, crate::annotation::Point::new(100.0, 100.0))
        });
        assert!(state.interaction().crop().is_some());
        assert!(state.key("Escape", false, false));
        assert!(state.interaction().crop().is_none());
        assert_eq!(state.canvas_size(), (200, 200));
    }

    #[test]
    fn applying_a_crop_is_one_undo_step_and_escape_leaves_no_change() {
        let mut state = state(200, 160);
        state.interaction_mut().set_tool(Tool::Crop);
        state.with_document(|document, interaction| {
            interaction.press(document, crate::annotation::Point::new(20.0, 20.0));
            interaction.move_to(document, crate::annotation::Point::new(120.0, 100.0));
            interaction.release(document, crate::annotation::Point::new(120.0, 100.0))
        });
        assert!(state.apply_crop().unwrap());
        assert_eq!(state.canvas_size(), (100, 80));
        state.undo();
        assert_eq!(state.canvas_size(), (200, 160));
        assert!(state.apply_crop().is_err(), "no provisional crop is left");
    }

    #[test]
    fn saving_writes_and_clears_dirty() {
        let dir = std::env::temp_dir().join(format!("tidyshot-editor-win-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut state = state(40, 30);
        state.changed(true);
        let target = dir.join("shot.png");
        let saved = state.save(Some(&target)).unwrap();
        assert_eq!(saved, target);
        assert!(!state.dirty());
        assert!(state.message().starts_with("Saved"));
        assert!(target.is_file());
        assert_eq!(state.title(), "TidyShot Editor — shot.png");
        let project = dir.join("shot.omnishot");
        state.save(Some(&project)).unwrap();
        assert_eq!(state.path(), Some(project.as_path()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_cover_undo_redo_and_tool_shortcuts() {
        let mut state = state(50, 50);
        assert!(state.key("r", false, false));
        assert_eq!(state.interaction().tool(), Tool::Rect);
        assert!(state.key("v", false, false));
        assert_eq!(state.interaction().tool(), Tool::Select);
        assert!(!state.key("s", false, false), "an unbound plain key");
        assert!(state.key("s", true, false));
        assert!(state.key("z", true, false));
        assert!(state.key("Z", true, true));
        assert!(state.key("c", true, false));
    }

    #[test]
    fn a_queued_open_request_is_applied_once() {
        let dir =
            std::env::temp_dir().join(format!("tidyshot-editor-queue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("queued.png");
        std::fs::write(&path, Image::new(12, 9, vec![0u8; 12 * 9 * 4]).to_png()).unwrap();
        let mut rust = EditorRust::default();
        apply_request(
            &mut rust,
            OpenRequest::Path {
                path: path.clone(),
                standalone: true,
            },
        );
        assert!(rust.open);
        rust.refresh();
        assert_eq!((rust.canvas_width, rust.canvas_height), (12, 9));
        assert!(rust.state.path().is_some());
        assert!(rust.title.to_string().contains("queued.png"));
        assert!(rust.standalone, "a startup request owns the window");
        apply_request(&mut rust, OpenRequest::Chooser { standalone: false });
        assert!(rust.open && rust.chooser_open);
        assert!(!rust.standalone);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_open_request_does_not_replace_unsaved_edits() {
        let dir =
            std::env::temp_dir().join(format!("tidyshot-editor-dirty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first.png");
        let second = dir.join("second.png");
        std::fs::write(&first, Image::new(20, 20, vec![0u8; 20 * 20 * 4]).to_png()).unwrap();
        std::fs::write(&second, Image::new(30, 30, vec![0u8; 30 * 30 * 4]).to_png()).unwrap();
        let mut rust = EditorRust::default();
        apply_request(
            &mut rust,
            OpenRequest::Path {
                path: first.clone(),
                standalone: true,
            },
        );
        rust.state.changed(true);
        apply_request(&mut rust, OpenRequest::open(second));
        rust.refresh();
        assert_eq!(rust.state.path(), Some(first.as_path()));
        assert_eq!((rust.canvas_width, rust.canvas_height), (20, 20));
        assert!(
            rust.message.to_string().contains("Save or discard"),
            "message was {}",
            rust.message.to_string()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_reports_an_error_and_keeps_the_window_open() {
        let mut rust = EditorRust::default();
        apply_request(
            &mut rust,
            OpenRequest::Path {
                path: PathBuf::from("/tmp/absent-12345.png"),
                standalone: true,
            },
        );
        assert!(rust.open);
        assert!(!rust.message.to_string().is_empty());
        assert!(!rust.state.is_open());
    }
}
