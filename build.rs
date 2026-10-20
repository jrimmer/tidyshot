use std::path::Path;
use std::process::Command;

fn main() {
    let mut builder = cxx_qt_build::CxxQtBuilder::new_qml_module(
        cxx_qt_build::QmlModule::new("tidyshot").qml_files([
            "qml/Main.qml",
            "qml/Panel.qml",
            "qml/KeyboardPanel.qml",
            "qml/PanelButton.qml",
            "qml/MenuPanel.qml",
            "qml/WindowPicker.qml",
            "qml/Overlay.qml",
            "qml/PreviewWindow.qml",
            "qml/TrayMenu.qml",
            "qml/TextResultDialog.qml",
            "qml/ScrollControls.qml",
            "qml/Editor.qml",
            "qml/EditorToolbar.qml",
        ]),
    )
    .qt_module("Quick")
    .qt_module("QuickControls2")
    .qt_module("QuickTest")
    .cpp_file("src/cpp/qml_load.cpp")
    .cpp_file("src/qt_shutdown.cpp")
    .cpp_file("src/cpp/editor_image.cpp")
    .files([
        "src/bridge.rs",
        "src/menu.rs",
        "src/preview.rs",
        "src/capture_runtime.rs",
        "src/qml_load.rs",
        "src/text_result.rs",
        "src/editor_window.rs",
        "src/editor_image.rs",
    ]);
    // The load probe's header is local to src/cpp; cxx generates the binding
    // that includes it, so that directory must be on the include path.
    unsafe {
        builder = builder.cc_builder(|cc| {
            cc.include("src/cpp");
        });
    }
    builder.build();

    build_scroll_helper();
    build_window_helper();
}

/// Build `native/scroll-helper`, the Wayland virtual-pointer injector that
/// auto-scroll uses.
///
/// The helper is a small standalone program that links `wayland-client`
/// directly; it is never linked into the app. Building it here keeps a
/// checkout runnable, while a missing scanner or headers only warns so the
/// rest of the app still builds.
fn build_scroll_helper() {
    let protocol = "native/wlr-virtual-pointer-unstable-v1.xml";
    let helper = "native/scroll-helper.c";
    let binary = "native/scroll-helper";
    println!("cargo:rerun-if-changed={protocol}");
    println!("cargo:rerun-if-changed={helper}");

    if !tool_available("wayland-scanner", &["--help"]) {
        println!(
            "cargo:warning=wayland-scanner not found; native/scroll-helper will not be built, so auto-scroll is unavailable"
        );
        return;
    }
    if !tool_available("pkg-config", &["--exists", "wayland-client"]) {
        println!(
            "cargo:warning=wayland-client development files not found; native/scroll-helper will not be built, so auto-scroll is unavailable"
        );
        return;
    }
    if !Path::new(helper).exists() || !Path::new(protocol).exists() {
        println!("cargo:warning=native scroll helper sources missing");
        return;
    }

    let out_dir = std::env::var("OUT_DIR").unwrap_or_else(|_| "target".to_owned());
    let build_dir = Path::new(&out_dir).join("scroll-helper-build");
    if std::fs::create_dir_all(&build_dir).is_err() {
        return;
    }
    let header = build_dir.join("virtual-pointer.h");
    let code = build_dir.join("virtual-pointer.c");

    let status = Command::new("wayland-scanner")
        .args(["client-header", protocol])
        .arg(&header)
        .status();
    if !matches!(status, Ok(status) if status.success()) {
        println!("cargo:warning=wayland-scanner client-header failed");
        return;
    }
    let status = Command::new("wayland-scanner")
        .args(["private-code", protocol])
        .arg(&code)
        .status();
    if !matches!(status, Ok(status) if status.success()) {
        println!("cargo:warning=wayland-scanner private-code failed");
        return;
    }

    // Ask pkg-config for the flags rather than hardcoding include paths.
    let flags = Command::new("pkg-config")
        .args(["--cflags", "--libs", "wayland-client"])
        .output();
    let Ok(flags) = flags else {
        println!("cargo:warning=pkg-config could not report wayland-client flags");
        return;
    };
    let flags = String::from_utf8_lossy(&flags.stdout);
    let mut command = Command::new("gcc");
    command
        .args(["-Wall", "-Wextra", "-O2"])
        .arg(format!("-I{}", build_dir.display()))
        .arg(helper)
        .arg(&code)
        .arg("-o")
        .arg(binary);
    command.args(flags.split_whitespace());
    match command.status() {
        Ok(status) if status.success() => {}
        Ok(_) => println!("cargo:warning=compiling native/scroll-helper failed"),
        Err(_) => println!("cargo:warning=gcc not found; native/scroll-helper will not be built"),
    }
}

/// Build `native/window-helper`, the Hyprland toplevel-export client that
/// captures one window as a PAM RGBA stream.
///
/// Like the scroll helper it is a standalone Wayland program, never linked
/// into the app. A missing scanner, headers or compiler only warns: the rest
/// of the app still builds, and the window capture path reports why it cannot
/// take an isolated window.
fn build_window_helper() {
    let protocols = [
        "native/hyprland-toplevel-export-v1.xml",
        "native/wlr-foreign-toplevel-management-unstable-v1.xml",
    ];
    let helper = "native/window-helper.c";
    let binary = "native/window-helper";
    for protocol in protocols {
        println!("cargo:rerun-if-changed={protocol}");
    }
    println!("cargo:rerun-if-changed={helper}");

    if !tool_available("wayland-scanner", &["--help"]) {
        println!(
            "cargo:warning=wayland-scanner not found; native/window-helper will not be built, so isolated window capture is unavailable"
        );
        return;
    }
    if !tool_available("pkg-config", &["--exists", "wayland-client"]) {
        println!(
            "cargo:warning=wayland-client development files not found; native/window-helper will not be built, so isolated window capture is unavailable"
        );
        return;
    }
    if !Path::new(helper).exists()
        || protocols
            .iter()
            .any(|protocol| !Path::new(protocol).exists())
    {
        println!("cargo:warning=native window helper sources missing");
        return;
    }

    let out_dir = std::env::var("OUT_DIR").unwrap_or_else(|_| "target".to_owned());
    let build_dir = Path::new(&out_dir).join("window-helper-build");
    if std::fs::create_dir_all(&build_dir).is_err() {
        println!("cargo:warning=could not create the window helper build directory");
        return;
    }
    let mut sources = Vec::new();
    for (protocol, stem) in [
        ("native/hyprland-toplevel-export-v1.xml", "toplevel-export"),
        (
            "native/wlr-foreign-toplevel-management-unstable-v1.xml",
            "foreign-toplevel",
        ),
    ] {
        let header = build_dir.join(format!("{stem}.h"));
        let code = build_dir.join(format!("{stem}.c"));
        let status = Command::new("wayland-scanner")
            .args(["client-header", protocol])
            .arg(&header)
            .status();
        if !matches!(status, Ok(status) if status.success()) {
            println!("cargo:warning=wayland-scanner client-header failed for {protocol}");
            return;
        }
        let status = Command::new("wayland-scanner")
            .args(["private-code", protocol])
            .arg(&code)
            .status();
        if !matches!(status, Ok(status) if status.success()) {
            println!("cargo:warning=wayland-scanner private-code failed for {protocol}");
            return;
        }
        sources.push(code);
    }

    let flags = Command::new("pkg-config")
        .args(["--cflags", "--libs", "wayland-client"])
        .output();
    let Ok(flags) = flags else {
        println!("cargo:warning=pkg-config could not report wayland-client flags");
        return;
    };
    let flags = String::from_utf8_lossy(&flags.stdout);
    let mut command = Command::new("gcc");
    command
        .args(["-Wall", "-Wextra", "-O2"])
        .arg(format!("-I{}", build_dir.display()))
        .arg(helper)
        .args(&sources)
        .arg("-o")
        .arg(binary);
    command.args(flags.split_whitespace());
    match command.status() {
        Ok(status) if status.success() => {}
        Ok(_) => println!("cargo:warning=compiling native/window-helper failed"),
        Err(_) => println!("cargo:warning=gcc not found; native/window-helper will not be built"),
    }
}

/// True when `program` runs and exits successfully.
fn tool_available(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
