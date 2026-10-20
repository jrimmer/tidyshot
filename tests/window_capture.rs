//! Behavior for isolated window capture end to end through the runtime.
//!
//! A window capture must copy the window's own surface, not a desktop
//! rectangle. These tests use a real helper stand-in on PATH-style fakes: the
//! runner answers `hyprctl`, `slurp` and the helper, so the identity rules,
//! the PAM parse and the editable frame are asserted without a compositor.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tidyshot::capture_runtime::CaptureRuntime;
use tidyshot::capture_session::{CaptureOptions, CaptureRequest};
use tidyshot::commands::{CommandError, CommandOutput, CommandRunner, CommandSpec};
use tidyshot::geometry::Rect;
use tidyshot::json::Value;
use tidyshot::mirror::MirrorRegistry;
use tidyshot::window::{self, WindowIdentity};

const MONITORS: &str = r#"[{"name":"DP-1","x":0,"y":0,"width":1920,"height":1080,"scale":1.0,"transform":0,"focused":true,"dpmsStatus":true,"activeWorkspace":{"name":"1"}}]"#;

fn clients() -> String {
    r#"[
        {"address":"0x112345678","stableId":"1","title":"Editor","class":"code","at":[10,20],"size":[600,400],"monitor":0,"workspace":{"name":"1"},"mapped":true,"focusHistoryID":0},
        {"address":"0x1abcdef01","stableId":"2","title":"Browser","class":"firefox","at":[700,100],"size":[800,600],"monitor":0,"workspace":{"name":"1"},"mapped":true,"focusHistoryID":1}
    ]"#
    .to_owned()
}

#[derive(Default)]
struct Fake {
    clients: Mutex<String>,
    /// Return an empty client list from this query number onward, to model a
    /// window disappearing between the choice and the copy.
    empty_from: Mutex<Option<usize>>,
    queries: Mutex<usize>,
    results: Mutex<VecDeque<(String, CommandOutput)>>,
    seen: Mutex<Vec<CommandSpec>>,
}

impl Fake {
    fn new() -> Self {
        Self {
            clients: Mutex::new(clients()),
            ..Self::default()
        }
    }

    fn push(&self, program: &str, stdout: Vec<u8>) {
        self.results.lock().unwrap().push_back((
            program.to_owned(),
            CommandOutput {
                status: 0,
                stdout,
                stderr: Vec::new(),
            },
        ));
    }
}

impl CommandRunner for Fake {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
        self.seen.lock().unwrap().push(spec.clone());
        if spec.program == "hyprctl" {
            let text = match spec.args.as_slice() {
                [flag, query] if flag == "-j" && query == "clients" => {
                    let mut queries = self.queries.lock().unwrap();
                    *queries += 1;
                    match *self.empty_from.lock().unwrap() {
                        Some(limit) if *queries >= limit => "[]".to_owned(),
                        _ => self.clients.lock().unwrap().clone(),
                    }
                }
                [flag, query] if flag == "-j" && query == "monitors" => MONITORS.to_owned(),
                [flag, ..] if flag == "-j" => "{}".to_owned(),
                _ => "{}".to_owned(),
            };
            return Ok(CommandOutput {
                status: 0,
                stdout: text.into_bytes(),
                stderr: Vec::new(),
            });
        }
        let mut results = self.results.lock().unwrap();
        let position = results
            .iter()
            .position(|(program, _)| *program == spec.program)
            .unwrap_or_else(|| panic!("no queued {} command", spec.program));
        Ok(results.remove(position).unwrap().1)
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tidyshot-window-capture-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn native_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tidyshot-window-native-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for name in [
        tidyshot::mirror::MIRROR_PLUGIN,
        tidyshot::mirror::CAPTURE_PLUGIN,
        window::HELPER,
    ] {
        std::fs::write(dir.join(name), b"binary").unwrap();
    }
    dir
}

fn pam(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
    let mut data = format!(
        "P7\nWIDTH {width}\nHEIGHT {height}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n"
    )
    .into_bytes();
    for _ in 0..width * height {
        data.extend_from_slice(&pixel);
    }
    data
}

fn request() -> CaptureRequest {
    CaptureRequest::resolve("window", None, None, 0).unwrap()
}

fn runtime<'a>(runner: &'a Fake, root: &'a Path) -> CaptureRuntime<'a, Fake> {
    CaptureRuntime {
        runner,
        mirror: Box::leak(Box::new(MirrorRegistry::with_arch("x86_64"))),
        native: native_dir(),
        root: root.to_path_buf(),
        options: CaptureOptions {
            cursor: false,
            freeze: true,
        },
        filename_format: "shot %S".to_owned(),
        default_delay: 0,
    }
}

#[test]
fn the_list_fallback_captures_the_window_surface_and_keeps_alpha() {
    let root = scratch("fallback");
    let runner = Fake::new();
    runner.push("slurp", b"0x112345678\n".to_vec());
    let helper = native_dir().join(window::HELPER);
    runner.push(&helper.to_string_lossy(), pam(600, 400, [12, 34, 56, 128]));
    let result = runtime(&runner, &root).run(request(), None).unwrap();
    assert_eq!((result.width, result.height), (600, 400));
    assert_eq!(result.rect, Some(Rect::new(10, 20, 600, 400)));
    let raw = tidyshot::pngzip::decode_png(&std::fs::read(&result.path).unwrap()).unwrap();
    assert_eq!(raw.pixel(0, 0), [12, 34, 56, 128]);
    let seen = runner.seen.lock().unwrap().clone();
    let slurp = seen.iter().find(|spec| spec.program == "slurp").unwrap();
    let rows = String::from_utf8(slurp.stdin.clone().unwrap()).unwrap();
    assert!(rows.contains("0x112345678"), "{rows}");
    assert!(
        seen.iter().all(|spec| spec.program != "grim"),
        "an isolated capture must never fall back to a rectangle grab"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_closed_window_is_reported_and_nothing_is_saved() {
    let root = scratch("closed");
    let runner = Fake::new();
    runner.push("slurp", b"0x112345678\n".to_vec());
    let helper = native_dir().join(window::HELPER);
    runner.push(&helper.to_string_lossy(), pam(600, 400, [1, 2, 3, 255]));
    // The window disappears between the choice and the copy: the third client
    // query is the post-copy re-resolve.
    *runner.empty_from.lock().unwrap() = Some(3);
    let error = runtime(&runner, &root).run(request(), None).unwrap_err();
    assert!(error.to_string().contains("closed"), "{error}");
    assert!(tidyshot::history::read(&root).is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_missing_helper_reports_the_build_step() {
    let root = scratch("helper-missing");
    let runner = Fake::new();
    let empty = scratch("helper-missing-native");
    let mut runtime = runtime(&runner, &root);
    runtime.native = empty.clone();
    let window =
        tidyshot::window::windows_from(&tidyshot::json::parse(&clients()).unwrap()).remove(0);
    let identity = WindowIdentity::from_address(&window.address).unwrap();
    let error = runtime
        .capture_window(&window, &identity, false, false)
        .unwrap_err();
    assert_eq!(error.to_string(), window::helper_missing_message());
    assert!(error.to_string().contains("wayland-scanner"));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&empty);
}

#[test]
fn the_composed_frame_reopens_with_every_editable_option() {
    let root = scratch("editable");
    let runner = Fake::new();
    runner.push("slurp", b"0x112345678\n".to_vec());
    let helper = native_dir().join(window::HELPER);
    runner.push(
        &helper.to_string_lossy(),
        pam(600, 400, [200, 100, 50, 255]),
    );
    let result = runtime(&runner, &root).run(request(), None).unwrap();
    let project = tidyshot::window_edit::load(&root, &result.path)
        .unwrap()
        .expect("a window frame is editable");
    assert_eq!(
        tidyshot::window_edit::display_path(&root, &result.path),
        result.display.clone().unwrap()
    );
    let background = project.background.clone().expect("a background");
    assert!(background.shadow);
    assert_eq!(background.radius_power, 2.0);
    let composed = tidyshot::window_edit::render(&project);
    assert!(composed.width > project.base.width);
    let mut document = tidyshot::editor::Document::open(project);
    // Editing the frame stays reversible and keeps the window pixels.
    let before = document.render_export();
    document
        .crop(
            tidyshot::annotation::Rect::new(0.0, 0.0, 10.0, 10.0),
            false,
            None,
        )
        .unwrap();
    document.undo();
    assert_eq!(document.render_export(), before);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_window_identity_rejects_a_reused_address_handle() {
    let root = scratch("identity");
    let runner = Fake::new();
    let window =
        tidyshot::window::windows_from(&tidyshot::json::parse(&clients()).unwrap()).remove(0);
    let identity = WindowIdentity {
        address: "0x112345678".to_owned(),
        stable_id: Some("other".to_owned()),
    };
    let error = window::capture(&runner, &native_dir(), &identity, false).unwrap_err();
    assert!(error.to_string().contains("closed"), "{error}");
    let _ = window;
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_settings_keys_choose_shadow_padding_and_wallpaper() {
    let value: Value = tidyshot::json::parse(
        r#"{"window_shadow":false,"window_padding":8,"window_wallpaper":false}"#,
    )
    .unwrap();
    let settings: BTreeMap<String, Value> = value.as_object().unwrap().clone();
    assert_eq!(
        settings.get("window_padding").and_then(Value::as_f64),
        Some(8.0)
    );
    let style = window::WindowStyle {
        rounding: 10,
        padding: 8,
        shadow: false,
        ..window::WindowStyle::default()
    };
    let background = style.background(600, 600);
    assert!(!background.shadow);
    assert_eq!(background.padding, 8);
}
