//! The repository root is the Omarchy plugin `omarchy plugin add` clones.
//! Ported from omnishot/tests/test_plugin_manifest.py

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const PLUGIN_ID: &str = "io.github.joshdaws.tidyshot";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn manifest_matches_the_shell_schema_and_ids() {
    let root = repo();
    let text = fs::read_to_string(root.join("manifest.json")).unwrap();
    assert!(
        text.contains("\"schemaVersion\": 1"),
        "schemaVersion must be 1: {text}"
    );
    assert!(
        text.contains(&format!("\"id\": \"{PLUGIN_ID}\"")),
        "id must be {PLUGIN_ID}"
    );
    assert!(
        text.contains("\"kinds\": [\n    \"bar-widget\"\n  ]"),
        "kinds must be bar-widget"
    );
    assert!(
        text.contains("\"barWidget\": \"plugin/BarWidget.qml\""),
        "entry point missing"
    );
    assert!(
        !PLUGIN_ID.starts_with("omarchy."),
        "the shell reserves the omarchy. namespace"
    );
    // The plugin id must be a safe path segment.
    assert!(
        PLUGIN_ID
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
        "plugin id has invalid characters"
    );
    // The entry point is a relative path that exists and names the id.
    let widget_rel = "plugin/BarWidget.qml";
    assert!(!widget_rel.starts_with('/') && !widget_rel.contains(".."));
    let widget = fs::read_to_string(root.join(widget_rel)).unwrap();
    assert!(widget.contains(&format!("moduleName: \"{PLUGIN_ID}\"")));
    assert!(widget.contains(&format!("ipcTarget: \"{PLUGIN_ID}\"")));
}

#[test]
fn repository_tracks_no_symlinks() {
    // `omarchy plugin validate` rejects any symlink in the plugin folder.
    let root = repo();
    let output = Command::new("git")
        .args(["-C", &root.display().to_string(), "ls-files", "-s"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let listing = String::from_utf8(output.stdout).unwrap();
    let symlinks: Vec<&str> = listing
        .lines()
        .filter(|line| line.starts_with("120000"))
        .collect();
    assert!(symlinks.is_empty(), "tracked symlinks: {symlinks:?}");
}

#[test]
fn widget_qml_parses() {
    let root = repo();
    let lint = find_qmllint();
    let Some(lint) = lint else {
        eprintln!("qmllint is not installed; skipping");
        return;
    };
    let widget = root.join("plugin/BarWidget.qml");
    let output = Command::new(lint).arg(&widget).output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!text.contains("[syntax]"), "{text}");
}

#[test]
fn app_qml_parses() {
    // The app's QML is packaged into the binary, so a syntax error only shows
    // up when the GUI loads; lint every file here instead.
    let root = repo();
    let lint = find_qmllint();
    let Some(lint) = lint else {
        eprintln!("qmllint is not installed; skipping");
        return;
    };
    let mut files: Vec<PathBuf> = fs::read_dir(root.join("qml"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "qml"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no qml files found");
    for file in files {
        let output = Command::new(&lint).arg(&file).output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!text.contains("[syntax]"), "{}: {text}", file.display());
    }
}

/// Locate a Qt 6 qmllint on PATH or in the usual Qt bin directory.
fn find_qmllint() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("QMAKE6") {
        let bin = Path::new(&path).with_file_name("qmllint");
        if bin.is_file() {
            return Some(bin);
        }
    }
    for dir in ["/usr/lib/qt6/bin", "/usr/bin"] {
        let candidate = Path::new(dir).join("qmllint");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    which("qmllint")
}

fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(program);
        candidate.is_file().then_some(candidate)
    })
}
