//! End-to-end behavior for the annotate editor window.
//!
//! These drive the editor state the QML window calls: opening images and
//! `.omnishot` projects, one undo step per drag, copy/save, the keyboard map
//! and the after-capture annotate request. They assert observable results,
//! never the bridge's generated internals.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tidyshot::annotation::{Annotation, Point, Props};
use tidyshot::commands::{CommandError, CommandOutput, CommandRunner, CommandSpec};
use tidyshot::editor::Document;
use tidyshot::editor_ui::{EditorAction, EditorInteraction, Tool, action_for_key};
use tidyshot::editor_window::EditorState;
use tidyshot::json::Value;
use tidyshot::pngzip::Image;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tidyshot-editor-{}-{name}", std::process::id()));
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

fn project_at(path: &Path) {
    let mut document = Document::new(image(300, 200, [240, 240, 240, 255]), None, false);
    let props: Props = [
        ("kind".to_owned(), Value::String("rect".to_owned())),
        ("x".to_owned(), Value::Number(20.0)),
        ("y".to_owned(), Value::Number(30.0)),
        ("w".to_owned(), Value::Number(80.0)),
        ("h".to_owned(), Value::Number(50.0)),
        ("color".to_owned(), Value::String("#ff5b61".to_owned())),
    ]
    .into_iter()
    .collect();
    document.objects.push(Annotation::new(props));
    document.commit();
    document.save(path).unwrap();
}

fn drag(interaction: &mut EditorInteraction, document: &mut Document, a: Point, b: Point) -> bool {
    interaction.press(document, a);
    interaction.move_to(document, b);
    interaction.release(document, b)
}

#[test]
fn a_png_opens_and_a_project_reopens_with_its_annotations() {
    let dir = scratch("open");
    let png = dir.join("shot.png");
    std::fs::write(&png, image(120, 90, [10, 20, 30, 255]).to_png()).unwrap();
    let mut state = EditorState::default();
    state.open(&png).unwrap();
    assert!(state.is_open());
    assert_eq!(state.canvas_size(), (120, 90));
    assert_eq!(state.path(), Some(png.as_path()));
    assert_eq!(state.title(), "TidyShot Editor — shot.png");

    let project = dir.join("shot.omnishot");
    project_at(&project);
    let mut state = EditorState::default();
    state.open(&project).unwrap();
    assert_eq!(state.document().unwrap().objects.len(), 1);
    assert_eq!(state.document().unwrap().objects[0].x(), 20.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn one_drag_is_one_undo_and_redo_restores_it() {
    let dir = scratch("undo");
    let png = dir.join("shot.png");
    std::fs::write(&png, image(200, 150, [255, 255, 255, 255]).to_png()).unwrap();
    let mut state = EditorState::default();
    state.open(&png).unwrap();
    let before = state.rendered().unwrap();

    state.interaction_mut().set_tool(Tool::Arrow);
    let mut document = state.document().unwrap().clone();
    assert!(drag(
        state.interaction_mut(),
        &mut document,
        Point::new(20.0, 20.0),
        Point::new(150.0, 100.0)
    ));
    *state.document_mut() = document;
    state.changed(true);
    assert!(state.dirty());
    let after = state.rendered().unwrap();
    assert_ne!(after, before);
    assert_eq!(state.document().unwrap().objects.len(), 1);

    state.undo();
    assert_eq!(state.rendered().unwrap(), before);
    assert!(state.document().unwrap().objects.is_empty());
    state.redo();
    assert_eq!(state.rendered().unwrap(), after);
    assert_eq!(state.document().unwrap().objects.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ctrl_z_shift_z_and_escape_follow_the_keyboard_map() {
    assert_eq!(action_for_key("z", true, false), Some(EditorAction::Undo));
    assert_eq!(action_for_key("Z", true, true), Some(EditorAction::Redo));
    assert_eq!(
        action_for_key("Escape", false, false),
        Some(EditorAction::Cancel)
    );

    let dir = scratch("keys");
    let png = dir.join("shot.png");
    std::fs::write(&png, image(100, 100, [255, 255, 255, 255]).to_png()).unwrap();
    let mut state = EditorState::default();
    state.open(&png).unwrap();
    state.interaction_mut().set_tool(Tool::Crop);
    let mut document = state.document().unwrap().clone();
    drag(
        state.interaction_mut(),
        &mut document,
        Point::new(10.0, 10.0),
        Point::new(70.0, 60.0),
    );
    *state.document_mut() = document;
    assert!(state.interaction().crop().is_some());
    assert!(state.key("Escape", false, false));
    assert!(state.interaction().crop().is_none());
    assert_eq!(state.canvas_size(), (100, 100), "escape leaves the canvas");
}

#[test]
fn ctrl_s_saves_atomically_and_a_project_path_keeps_it_editable() {
    let dir = scratch("save");
    let source = dir.join("source.png");
    std::fs::write(&source, image(80, 60, [1, 2, 3, 255]).to_png()).unwrap();
    let mut state = EditorState::default();
    state.open(&source).unwrap();
    state.interaction_mut().set_tool(Tool::Ellipse);
    let mut document = state.document().unwrap().clone();
    drag(
        state.interaction_mut(),
        &mut document,
        Point::new(10.0, 10.0),
        Point::new(60.0, 50.0),
    );
    *state.document_mut() = document;
    state.changed(true);

    let target = dir.join("annotated.png");
    state.save(Some(&target)).unwrap();
    assert!(!state.dirty());
    let bytes = std::fs::read(&target).unwrap();
    assert_eq!(bytes, state.rendered().unwrap().to_png());
    assert_eq!(
        std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".tidyshot-"))
            .count(),
        0,
        "the atomic save leaves no temp file"
    );

    let project = dir.join("annotated.omnishot");
    state.save(Some(&project)).unwrap();
    let mut reopened = EditorState::default();
    reopened.open(&project).unwrap();
    assert_eq!(reopened.document().unwrap().objects.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<CommandSpec>>,
}

impl CommandRunner for Recorder {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
        self.seen.lock().unwrap().push(spec.clone());
        Ok(CommandOutput {
            status: 0,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }
}

#[test]
fn ctrl_c_copies_the_rendered_canvas_not_the_original() {
    let dir = scratch("copy");
    let source = dir.join("source.png");
    std::fs::write(&source, image(60, 40, [255, 255, 255, 255]).to_png()).unwrap();
    let mut state = EditorState::default();
    state.open(&source).unwrap();
    state.interaction_mut().set_tool(Tool::Rect);
    let mut document = state.document().unwrap().clone();
    drag(
        state.interaction_mut(),
        &mut document,
        Point::new(5.0, 5.0),
        Point::new(40.0, 30.0),
    );
    *state.document_mut() = document;
    state.changed(true);
    let expected = state.rendered().unwrap().to_png();
    let runner = Recorder::default();
    state.copy(&runner).unwrap();
    let seen = runner.seen.lock().unwrap();
    assert_eq!(seen[0].program, "wl-copy");
    assert_eq!(seen[0].args, ["--type", "image/png"]);
    assert_eq!(seen[0].stdin.as_deref(), Some(expected.as_slice()));
    let _ = std::fs::remove_dir_all(&dir);
}
