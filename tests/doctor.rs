// `tidyshot doctor` against a fake machine: no compositor, commands or files
// are touched.
// Ported from omnishot/test_doctor.py

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tidyshot::compat;
use tidyshot::doctor::{self, CheckResult, HyprlandInfo, System};

const ABI: &str = "efb50993780079460b0cbed1363e2166a2de1d9f_aq_0.15_hu_0.14_hg_0.5_hc_0.1_hlg_0.6";
const VERSION_H: &str = r#"#pragma once
#define GIT_COMMIT_HASH    "efb50993780079460b0cbed1363e2166a2de1d9f"
#define GIT_TAG            "v0.56.2"
#define AQUAMARINE_VERSION "0.15.0"
#define AQUAMARINE_VERSION_MAJOR 0
#define HYPRLANG_VERSION     "0.6.8"
#define HYPRUTILS_VERSION    "0.14.1"
#define HYPRCURSOR_VERSION   "0.1.13"
#define HYPRGRAPHICS_VERSION "0.5.1"
"#;
const PROBE: &[&str] = &[
    "hyprctl",
    "repl",
    "return tostring(hl.plugin ~= nil and hl.plugin.omnishot ~= nil)",
];

/// A healthy install; tests break one thing at a time.
struct Machine {
    env: Mutex<BTreeMap<String, String>>,
    home: PathBuf,
    config: PathBuf,
    native: PathBuf,
    headers: PathBuf,
    info: Mutex<Option<HyprlandInfo>>,
    omarchy: Mutex<String>,
    missing: Mutex<BTreeSet<String>>,
    widget: Mutex<bool>,
    registered: Mutex<bool>,
    accepts_plugin: Mutex<bool>,
    calls: Mutex<Vec<Vec<String>>>,
}

impl Machine {
    fn healthy(root: &Path) -> Self {
        let home = root.join("home");
        let config = home.join(".config");
        let native = root.join("app/native");
        std::fs::create_dir_all(&native).unwrap();
        for name in doctor::NATIVE_FILES {
            std::fs::write(native.join(name), b"built").unwrap();
        }
        std::fs::write(native.join("clean-mirror.abi"), format!("{ABI}\n")).unwrap();
        let hypr = config.join("hypr");
        std::fs::create_dir_all(&hypr).unwrap();
        std::fs::write(
            hypr.join("bindings.lua"),
            format!("-- mine\n\n{}\n", doctor::BINDINGS_MARKER),
        )
        .unwrap();
        std::fs::write(
            hypr.join("hyprland.lua"),
            format!("-- mine\n{}\n", doctor::RULES_REQUIRE),
        )
        .unwrap();
        std::fs::write(hypr.join("tidyshot.lua"), "-- rules\n").unwrap();
        std::fs::create_dir_all(config.join("omarchy")).unwrap();
        std::fs::write(config.join("omarchy/shell.json"), "{}").unwrap();
        let launcher = home.join(".local/bin/tidyshot");
        std::fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        std::fs::write(
            &launcher,
            format!(
                "#!/bin/sh\nexport LD_PRELOAD={}\nexec {}\n",
                native.join("drag-status.so").display(),
                root.join("app/build/tidyshot").display()
            ),
        )
        .unwrap();
        let headers = root.join("version.h");
        std::fs::write(&headers, VERSION_H).unwrap();

        let mut env = BTreeMap::new();
        env.insert("WAYLAND_DISPLAY".to_owned(), "wayland-1".to_owned());
        env.insert("HYPRLAND_INSTANCE_SIGNATURE".to_owned(), "x".to_owned());

        let machine = Self {
            env: Mutex::new(env),
            home,
            config,
            native,
            headers,
            info: Mutex::new(Some(HyprlandInfo {
                version: "0.56.2".to_owned(),
                abi_hash: ABI.to_owned(),
            })),
            omarchy: Mutex::new("4.0.2-1".to_owned()),
            missing: Mutex::new(BTreeSet::new()),
            widget: Mutex::new(true),
            registered: Mutex::new(false),
            accepts_plugin: Mutex::new(true),
            calls: Mutex::new(Vec::new()),
        };
        // The launcher check wants this installation's built binary path.
        std::fs::create_dir_all(root.join("app/build")).unwrap();
        machine
    }

    fn set_env(&self, env: &[(&str, &str)]) {
        *self.env.lock().unwrap() = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
    }

    fn set_info(&self, info: Option<HyprlandInfo>) {
        *self.info.lock().unwrap() = info;
    }

    fn set_omarchy(&self, version: &str) {
        *self.omarchy.lock().unwrap() = version.to_owned();
    }

    fn set_missing(&self, names: &[&str]) {
        *self.missing.lock().unwrap() = names.iter().map(|n| (*n).to_owned()).collect();
    }

    fn set_widget(&self, value: bool) {
        *self.widget.lock().unwrap() = value;
    }

    fn set_accepts_plugin(&self, value: bool) {
        *self.accepts_plugin.lock().unwrap() = value;
    }

    fn set_registered(&self, value: bool) {
        *self.registered.lock().unwrap() = value;
    }

    fn registered(&self) -> bool {
        *self.registered.lock().unwrap()
    }

    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }

    fn plugin_calls(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|call| {
                call.first().is_some_and(|p| p == "hyprctl")
                    && call.get(1).is_some_and(|a| a == "plugin")
            })
            .filter_map(|call| call.get(2).cloned())
            .collect()
    }
}

impl System for Machine {
    fn env(&self, key: &str) -> Option<String> {
        self.env.lock().unwrap().get(key).cloned()
    }

    fn run(&self, args: &[&str]) -> Option<String> {
        let call: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        self.calls.lock().unwrap().push(call.clone());
        if args == ["omarchy", "version"] {
            return Some(self.omarchy.lock().unwrap().clone());
        }
        if args == PROBE {
            return Some(if self.registered() {
                "true".to_owned()
            } else {
                "false".to_owned()
            });
        }
        if args.len() >= 3 && args[0] == "hyprctl" && args[1] == "plugin" && args[2] == "load" {
            *self.registered.lock().unwrap() = *self.accepts_plugin.lock().unwrap();
            return Some("ok".to_owned());
        }
        if args.len() >= 3 && args[0] == "hyprctl" && args[1] == "plugin" && args[2] == "unload" {
            *self.registered.lock().unwrap() = false;
            return Some("ok".to_owned());
        }
        None
    }

    fn which(&self, name: &str) -> Option<PathBuf> {
        if self.missing.lock().unwrap().contains(name) {
            None
        } else {
            Some(PathBuf::from(format!("/usr/bin/{name}")))
        }
    }

    fn hyprland(&self) -> Option<HyprlandInfo> {
        self.info.lock().unwrap().clone()
    }

    fn header_version_file(&self) -> Option<PathBuf> {
        self.headers.exists().then(|| self.headers.clone())
    }

    fn shell_widget(&self) -> bool {
        *self.widget.lock().unwrap()
    }

    fn home(&self) -> PathBuf {
        self.home.clone()
    }

    fn config(&self) -> PathBuf {
        self.config.clone()
    }

    fn native(&self) -> PathBuf {
        self.native.clone()
    }
}

fn results(machine: &Machine, load: bool) -> BTreeMap<String, CheckResult> {
    doctor::diagnose(machine, load)
        .into_iter()
        .map(|result| (result.id.clone(), result))
        .collect()
}

fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("tidyshot-doctor-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn healthy_machine_passes_every_check() {
    let root = temp_root("healthy");
    let machine = Machine::healthy(&root);
    let statuses: BTreeSet<&str> = results(&machine, false)
        .values()
        .map(|r| r.status.id())
        .collect();
    assert_eq!(statuses, BTreeSet::from(["ok"]));
    let mut out = Vec::new();
    assert_eq!(doctor::main_writer(&[], &machine, &mut out), 0);
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("Everything TidyShot needs looks good."));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn header_hash_mirrors_hyprland_plugin_api() {
    let root = temp_root("hash");
    let path = root.join("version.h");
    std::fs::write(&path, VERSION_H).unwrap();
    assert_eq!(compat::header_abi_hash(&path).as_deref(), Some(ABI));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn outside_a_session_fails_first() {
    let root = temp_root("session");
    let machine = Machine::healthy(&root);
    machine.set_env(&[]);
    let session = results(&machine, false)["session"].clone();
    assert_eq!(session.status.id(), "fail");
    assert!(session.fix.contains("Omarchy desktop session"));

    machine.set_env(&[
        ("WAYLAND_DISPLAY", "w"),
        ("HYPRLAND_INSTANCE_SIGNATURE", "x"),
    ]);
    machine.set_info(None);
    assert_eq!(results(&machine, false)["session"].status.id(), "fail");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn untested_versions_warn_without_failing() {
    let root = temp_root("versions");
    let machine = Machine::healthy(&root);
    machine.set_omarchy("4.0.3rc4-1");
    machine.set_info(Some(HyprlandInfo {
        version: "0.57.0".to_owned(),
        abi_hash: ABI.to_owned(),
    }));
    let versions = results(&machine, false)["versions"].clone();
    assert_eq!(versions.status.id(), "warn");
    assert!(
        versions
            .summary
            .contains("Omarchy 4.0.3rc4-1 and Hyprland 0.57.0"),
        "{}",
        versions.summary
    );
    let mut out = Vec::new();
    assert_eq!(doctor::main_writer(&[], &machine, &mut out), 0);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn upgraded_hyprland_asks_for_a_session_restart() {
    let root = temp_root("upgraded");
    let machine = Machine::healthy(&root);
    std::fs::write(&machine.headers, VERSION_H.replace("0.15.0", "0.16.0")).unwrap();
    let headers = results(&machine, false)["headers"].clone();
    assert_eq!(headers.status.id(), "fail");
    assert!(headers.fix.contains("Restart your desktop session"));
    std::fs::remove_file(&machine.headers).unwrap();
    assert!(
        results(&machine, false)["headers"]
            .fix
            .contains("omarchy pkg add hyprland")
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn stale_or_unrecorded_build_abi() {
    let root = temp_root("abi");
    let machine = Machine::healthy(&root);
    std::fs::write(machine.native.join("clean-mirror.abi"), "older-hyprland\n").unwrap();
    let stale = results(&machine, false)["build-abi"].clone();
    assert_eq!(stale.status.id(), "fail");
    assert!(stale.fix.contains("install.sh"));
    std::fs::remove_file(machine.native.join("clean-mirror.abi")).unwrap();
    assert_eq!(results(&machine, false)["build-abi"].status.id(), "warn");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn missing_helpers_and_commands_name_the_fix() {
    let root = temp_root("missing");
    let machine = Machine::healthy(&root);
    std::fs::remove_file(machine.native.join("live-selector")).unwrap();
    machine.set_missing(&["ffmpeg", "ffprobe", "wl-copy"]);
    let found = results(&machine, false);
    assert_eq!(found["native"].status.id(), "fail");
    assert!(found["native"].summary.contains("live-selector"));
    assert_eq!(
        found["commands"].fix,
        "Install them with: omarchy pkg add wl-clipboard ffmpeg"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn launcher_and_desktop_integration() {
    let root = temp_root("launcher");
    let machine = Machine::healthy(&root);
    let launcher = machine.home.join(".local/bin/tidyshot");
    std::fs::write(&launcher, "#!/bin/sh\nexec /elsewhere/build/tidyshot\n").unwrap();
    assert_eq!(results(&machine, false)["launcher"].status.id(), "warn");
    std::fs::remove_file(&launcher).unwrap();
    assert_eq!(results(&machine, false)["launcher"].status.id(), "fail");

    std::fs::write(machine.config.join("hypr/bindings.lua"), "-- mine\n").unwrap();
    std::fs::remove_file(machine.config.join("hypr/tidyshot.lua")).unwrap();
    let config = results(&machine, false)["hyprland-config"].clone();
    assert_eq!(config.status.id(), "warn");
    assert!(
        config
            .summary
            .contains("keyboard shortcuts and window rules"),
        "{}",
        config.summary
    );
    machine.set_widget(false);
    assert_eq!(results(&machine, false)["shell"].status.id(), "warn");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn load_test_only_runs_when_asked() {
    let root = temp_root("optin");
    let machine = Machine::healthy(&root);
    let _ = results(&machine, false);
    assert!(machine.plugin_calls().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn load_test_loads_probes_and_unloads() {
    let root = temp_root("load");
    let machine = Machine::healthy(&root);
    assert_eq!(results(&machine, true)["load-test"].status.id(), "ok");
    assert_eq!(machine.plugin_calls(), ["load", "unload"]);
    assert!(!machine.registered());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn load_test_reports_a_rejected_plugin_and_still_unloads() {
    let root = temp_root("rejected");
    let machine = Machine::healthy(&root);
    machine.set_accepts_plugin(false);
    let result = results(&machine, true)["load-test"].clone();
    assert_eq!(result.status.id(), "fail");
    assert!(result.fix.contains("aarch64"));
    assert_eq!(machine.plugin_calls(), ["load", "unload"]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn load_test_leaves_an_already_loaded_plugin_alone() {
    let root = temp_root("already");
    let machine = Machine::healthy(&root);
    machine.set_registered(true);
    assert_eq!(results(&machine, true)["load-test"].status.id(), "ok");
    assert!(machine.plugin_calls().is_empty());
    assert!(machine.registered());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn json_output_and_failure_exit_code() {
    let root = temp_root("json");
    let machine = Machine::healthy(&root);
    machine.set_missing(&["grim"]);
    let mut out = Vec::new();
    assert_eq!(
        doctor::main_writer(&["--json".to_owned()], &machine, &mut out),
        1
    );
    let report = tidyshot::json::parse(std::str::from_utf8(&out).unwrap().trim()).unwrap();
    assert_eq!(
        report.get("hyprland").and_then(|v| v.as_str()),
        Some("0.56.2")
    );
    let results = report.get("results").and_then(|v| v.as_array()).unwrap();
    let commands = results
        .iter()
        .find(|r| r.get("id").and_then(|v| v.as_str()) == Some("commands"))
        .unwrap();
    assert_eq!(
        commands.get("status").and_then(|v| v.as_str()),
        Some("fail")
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn nested_hyprland_layout_passes_config_and_rules() {
    let root = temp_root("nested");
    let machine = Machine::healthy(&root);
    let top = machine.config.join("hypr");
    let nested = machine.config.join("omarchy/hypr");
    std::fs::create_dir_all(&nested).unwrap();
    for name in ["hyprland.lua", "bindings.lua", "tidyshot.lua"] {
        std::fs::rename(top.join(name), nested.join(name)).unwrap();
    }
    std::fs::remove_dir(&top).unwrap();
    let found = results(&machine, false);
    assert_eq!(found["config"].status.id(), "ok");
    assert_eq!(found["hyprland-config"].status.id(), "ok");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn top_level_layout_wins_when_both_are_present() {
    let root = temp_root("both");
    let machine = Machine::healthy(&root);
    // A stale nested copy missing the rules must not make doctor report a
    // problem while the preferred top-level layout is healthy.
    let nested = machine.config.join("omarchy/hypr");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("hyprland.lua"), "-- stale\n").unwrap();
    std::fs::write(nested.join("bindings.lua"), "-- stale\n").unwrap();
    let found = results(&machine, false);
    assert_eq!(found["config"].status.id(), "ok");
    assert_eq!(found["hyprland-config"].status.id(), "ok");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn missing_hyprland_config_fails_the_config_check() {
    let root = temp_root("nohypr");
    let machine = Machine::healthy(&root);
    std::fs::remove_file(machine.config.join("hypr/hyprland.lua")).unwrap();
    let config = results(&machine, false)["config"].clone();
    assert_eq!(config.status.id(), "fail");
    assert!(config.summary.contains("hypr/hyprland.lua"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn untested_helper_strips_the_package_release() {
    assert!(compat::untested("4.0.2-1", "0.56.2").is_empty());
    assert_eq!(compat::untested("4.0.3rc4-1", ""), ["Omarchy 4.0.3rc4-1"]);
}

#[test]
fn supported_versions_have_one_source() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let compat = std::fs::read_to_string(manifest.join("src/compat.rs")).unwrap();
    for version in compat::TESTED_OMARCHY.iter().chain(compat::TESTED_HYPRLAND) {
        assert!(
            compat.contains(&format!("\"{version}\"")),
            "src/compat.rs does not list tested version {version}"
        );
    }
    // README.md states the versions a user should match.
    let stated = std::fs::read_to_string(manifest.join("README.md"))
        .unwrap()
        .lines()
        .find(|line| line.starts_with("Developed and tested on"))
        .expect("README.md has no test-compatibility line")
        .to_owned();
    for version in compat::TESTED_OMARCHY.iter().chain(compat::TESTED_HYPRLAND) {
        assert!(
            stated.contains(version),
            "README.md does not list tested version {version}; update it to match src/compat.rs"
        );
    }
    // install.sh runs the same table before building; that integration is
    // wired by the installer unit, which owns install.sh.
    // No other module may redeclare the tested versions.
    let mut sources = Vec::new();
    collect_rs(&manifest.join("src"), &mut sources);
    for path in sources {
        if path.ends_with("compat.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for version in compat::TESTED_OMARCHY.iter().chain(compat::TESTED_HYPRLAND) {
            assert!(
                !text.contains(&format!("\"{version}\"")),
                "{} redeclares tested version {version}",
                path.display()
            );
        }
    }
}

fn collect_rs(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_rs(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn compositor_checks_are_skipped_outside_a_session() {
    let root = temp_root("skip");
    let machine = Machine::healthy(&root);
    machine.set_env(&[]);
    machine.set_widget(false);
    let found = results(&machine, true);
    let skipped: BTreeSet<&str> = found
        .iter()
        .filter(|(_, r)| r.status.id() == "skip")
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(
        skipped,
        BTreeSet::from(["headers", "build-abi", "shell", "load-test"])
    );
    assert!(machine.plugin_calls().is_empty());
    let mut out = Vec::new();
    doctor::main_writer(&[], &machine, &mut out);
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("1 problem, 0 warnings")
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn headers_are_found_through_the_compiler_include_paths() {
    let root = temp_root("includes");
    let nested = root.join("hyprland/src");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("version.h"), VERSION_H).unwrap();
    let flags = format!(
        "-I{} -I{}/hyprland/protocols -I{}/hyprland -I{}/hyprland/src -I/usr/include/pixman-1\n",
        root.display(),
        root.display(),
        root.display(),
        root.display()
    );
    assert_eq!(
        compat::version_file_from_flags(&flags),
        Some(nested.join("version.h"))
    );
    assert_eq!(compat::version_file_from_flags(""), None);
    let _ = std::fs::remove_dir_all(&root);
}
