//! User-level install/uninstall in an isolated home; no desktop services are
//! changed. Ported from omnishot/tests/test_install_user.py

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PLUGIN_ID: &str = "io.github.joshdaws.tidyshot";

/// Stub the desktop commands the installer probes; record every call.
const STUB: &str = r#"#!/usr/bin/env python3
import json, os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
with open(os.environ['SETUP_LOG'], 'a') as log:
    log.write(json.dumps([name, *sys.argv[1:]]) + '\n')
sys.exit(0)
"#;

struct Home {
    base: PathBuf,
    home: PathBuf,
    config: PathBuf,
    data: PathBuf,
    source: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

impl Home {
    fn new(name: &str) -> Self {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let base = std::env::temp_dir().join(format!(
            "tidyshot-install-{}-{nonce}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let source = base.join("checkout");
        fs::create_dir_all(source.join("plugin")).unwrap();
        fs::create_dir_all(source.join("packaging")).unwrap();
        fs::create_dir_all(source.join("build")).unwrap();
        fs::copy(repo.join("manifest.json"), source.join("manifest.json")).unwrap();
        fs::copy(
            repo.join("plugin/BarWidget.qml"),
            source.join("plugin/BarWidget.qml"),
        )
        .unwrap();
        fs::copy(
            repo.join("packaging/tidyshot.xml"),
            source.join("packaging/tidyshot.xml"),
        )
        .unwrap();
        fs::write(source.join("build/tidyshot"), b"binary").unwrap();

        let home = base.join("home");
        let config = base.join("config");
        let data = base.join("data");
        fs::create_dir_all(home.join(".local/bin")).unwrap();
        fs::create_dir_all(config.join("hypr")).unwrap();
        fs::create_dir_all(config.join("omarchy")).unwrap();
        fs::write(
            config.join("hypr/hyprland.lua"),
            "-- existing window config\n",
        )
        .unwrap();
        fs::write(
            config.join("hypr/bindings.lua"),
            "-- existing keybindings\n",
        )
        .unwrap();
        fs::write(
            config.join("omarchy/shell.json"),
            r#"{"idle": {"lock": 600}, "bar": {"layout": {"left": [{"id": "local.tidyshot"}], "right": [{"id": "omarchy.clock"}]}}}"#,
        )
        .unwrap();

        let bin = base.join("bin");
        fs::create_dir_all(&bin).unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        for stub in [
            "update-desktop-database",
            "update-mime-database",
            "xdg-mime",
            "omarchy-shell",
            "omarchy",
            "hyprctl",
        ] {
            let path = bin.join(stub);
            fs::write(&path, STUB).unwrap();
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
        }

        Self {
            log: base.join("commands.jsonl"),
            base,
            home,
            config,
            data,
            source,
            bin,
        }
    }

    fn env(&self) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = std::env::vars().collect();
        env.retain(|(key, _)| key != "TIDYSHOT_SOURCE");
        env.push(("HOME".into(), self.home.display().to_string()));
        env.push(("XDG_CONFIG_HOME".into(), self.config.display().to_string()));
        env.push(("XDG_DATA_HOME".into(), self.data.display().to_string()));
        env.push(("TIDYSHOT_SOURCE".into(), self.source.display().to_string()));
        env.push(("SETUP_LOG".into(), self.log.display().to_string()));
        env.push((
            "PATH".into(),
            format!(
                "{}:{}",
                self.bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        ));
        env
    }

    fn install(&self) -> Output {
        self.python("install_user.py")
    }

    fn uninstall(&self) -> Output {
        self.python("uninstall_user.py")
    }

    fn python(&self, script: &str) -> Output {
        let mut command = Command::new("python3");
        command.arg(self.source.join("scripts").join(script));
        command.env_clear();
        for (key, value) in self.env() {
            command.env(key, value);
        }
        command.output().unwrap()
    }

    fn calls(&self) -> Vec<Vec<String>> {
        let Ok(text) = fs::read_to_string(&self.log) else {
            return Vec::new();
        };
        text.lines()
            .filter(|line| !line.is_empty())
            .map(parse_json_array)
            .collect()
    }

    fn layout(&self) -> String {
        fs::read_to_string(self.config.join("omarchy/shell.json")).unwrap()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn parse_json_array(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'"' {
            index += 1;
            continue;
        }
        index += 1;
        let mut value = String::new();
        while index < bytes.len() {
            match bytes[index] {
                b'\\' if index + 1 < bytes.len() => {
                    value.push(bytes[index + 1] as char);
                    index += 2;
                }
                b'"' => {
                    index += 1;
                    break;
                }
                byte => {
                    value.push(byte as char);
                    index += 1;
                }
            }
        }
        out.push(value);
    }
    out
}

fn copy_scripts(home: &Home) {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    fs::create_dir_all(home.source.join("scripts")).unwrap();
    for script in ["install_user.py", "uninstall_user.py"] {
        fs::copy(
            repo.join("scripts").join(script),
            home.source.join("scripts").join(script),
        )
        .unwrap();
    }
}

#[test]
fn reinstall_preserves_widget_placement_and_user_config() {
    let home = Home::new("reinstall");
    copy_scripts(&home);
    let first = home.install();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    // Config files are stable across a rerun; the pre-plugin id is migrated in
    // place, keeping the user's bar section and unrelated keys.
    let before = snapshot(&home.config);
    let rerun = home.install();
    assert!(
        rerun.status.success(),
        "{}",
        String::from_utf8_lossy(&rerun.stderr)
    );
    assert_eq!(
        snapshot(&home.config),
        before,
        "a rerun must not churn config"
    );
    let layout = home.layout();
    assert!(layout.contains(&format!("\"id\": \"{PLUGIN_ID}\"")));
    assert!(layout.contains("\"id\": \"omarchy.clock\""));
    assert!(
        layout.contains("\"lock\": 600"),
        "unrelated config is kept: {layout}"
    );
    assert!(
        home.source.join("backups").is_dir(),
        "configuration is backed up"
    );
    // The first install changes the widget, so the shell is asked to restart;
    // the rerun left it untouched and must not restart it again.
    let restart = vec![
        "omarchy".to_owned(),
        "restart".to_owned(),
        "shell".to_owned(),
    ];
    assert_eq!(
        home.calls().iter().filter(|call| **call == restart).count(),
        1,
        "the shell restarts once, when the widget changes"
    );
}

#[test]
fn installs_desktop_files_into_xdg_data_home() {
    let home = Home::new("desktopfiles");
    copy_scripts(&home);
    assert!(home.install().status.success());
    assert!(
        home.data
            .join("applications/org.omarchy.TidyShot.desktop")
            .is_file()
    );
    assert!(home.data.join("mime/packages/tidyshot.xml").is_file());
    assert!(
        !home.home.join(".local/share").exists(),
        "XDG_DATA_HOME is respected"
    );
    let launcher = fs::read_to_string(home.home.join(".local/bin/tidyshot")).unwrap();
    assert!(
        launcher.contains("build/tidyshot"),
        "launcher points at the built binary: {launcher}"
    );
    assert!(home.calls().contains(&vec![
        "update-desktop-database".to_owned(),
        home.data.join("applications").display().to_string()
    ]));
    assert!(home.calls().contains(&vec![
        "update-mime-database".to_owned(),
        home.data.join("mime").display().to_string()
    ]));
}

#[test]
fn bad_shell_config_does_not_partially_install() {
    let home = Home::new("badconfig");
    copy_scripts(&home);
    fs::write(home.config.join("omarchy/shell.json"), "{broken").unwrap();
    let result = home.install();
    assert!(!result.status.success());
    assert!(!home.home.join(".local/bin/tidyshot").exists());
    assert!(!home.data.exists());
    assert!(!home.source.join("backups").exists());
    assert!(home.calls().is_empty());
}

#[test]
fn manual_install_copies_widget_under_plugin_id() {
    let home = Home::new("manual");
    copy_scripts(&home);
    assert!(home.install().status.success());
    let plugin = home.config.join("omarchy/plugins").join(PLUGIN_ID);
    let manifest = fs::read_to_string(plugin.join("manifest.json")).unwrap();
    assert!(manifest.contains(&format!("\"id\": \"{PLUGIN_ID}\"")));
    assert_eq!(
        fs::read(plugin.join("plugin/BarWidget.qml")).unwrap(),
        fs::read(home.source.join("plugin/BarWidget.qml")).unwrap()
    );
}

#[test]
fn migrates_legacy_widget_with_its_settings() {
    let home = Home::new("legacy");
    copy_scripts(&home);
    let legacy = home.config.join("omarchy/plugins/local.tidyshot");
    fs::create_dir_all(&legacy).unwrap();
    fs::write(legacy.join("BarWidget.qml"), "old widget").unwrap();
    fs::write(legacy.join("manifest.json"), "{}").unwrap();
    assert!(home.install().status.success());
    assert!(!legacy.exists());
    let layout = home.layout();
    assert!(
        layout.contains(&format!("\"id\": \"{PLUGIN_ID}\"")),
        "placement migrated: {layout}"
    );
    let backup = fs::read_dir(home.source.join("backups"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(
        fs::read_to_string(backup.path().join("local.tidyshot-BarWidget.qml")).unwrap(),
        "old widget"
    );
    // Removing the legacy widget changes what the bar serves, so the shell
    // must reload it.
    assert!(
        home.calls().contains(&vec![
            "omarchy".to_owned(),
            "restart".to_owned(),
            "shell".to_owned(),
        ]),
        "the shell restarts when the legacy widget is migrated"
    );
}

#[test]
fn uninstall_restores_configuration_and_keeps_captures() {
    let home = Home::new("uninstall");
    copy_scripts(&home);
    let tracked_bindings = fs::read_to_string(home.config.join("hypr/bindings.lua")).unwrap();
    let tracked: Vec<(String, String)> = ["hypr/hyprland.lua", "hypr/bindings.lua"]
        .iter()
        .map(|name| {
            (
                name.to_string(),
                fs::read_to_string(home.config.join(name)).unwrap(),
            )
        })
        .collect();
    let captures = home.data.join("tidyshot/captures");
    fs::create_dir_all(&captures).unwrap();
    fs::write(captures.join("shot.png"), b"capture").unwrap();
    let mimeapps = home.config.join("mimeapps.list");
    fs::write(
        &mimeapps,
        "[Default Applications]\nimage/png=viewer.desktop;\n",
    )
    .unwrap();
    assert!(home.install().status.success());
    let mut mime_text = fs::read_to_string(&mimeapps).unwrap();
    mime_text.push_str(
        "application/x-tidyshot=org.omarchy.TidyShot.desktop;\nimage/jpeg=org.omarchy.TidyShot.desktop;viewer.desktop;\n",
    );
    fs::write(&mimeapps, mime_text).unwrap();
    // A plugin managed by omarchy plugin add is left for `plugin remove`.
    let managed = home.config.join("omarchy/plugins/other.plugin");
    fs::create_dir_all(managed.join(".git")).unwrap();
    let result = home.uninstall();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    for (name, before) in tracked {
        assert_eq!(fs::read_to_string(home.config.join(&name)).unwrap(), before);
    }
    assert!(!home.home.join(".local/bin/tidyshot").exists());
    assert!(
        !home
            .data
            .join("applications/org.omarchy.TidyShot.desktop")
            .exists()
    );
    assert!(!home.config.join("hypr/tidyshot.lua").exists());
    assert!(!home.config.join("omarchy/plugins").join(PLUGIN_ID).exists());
    assert!(
        managed.exists(),
        "a managed plugin belongs to omarchy plugin remove"
    );
    assert_eq!(
        fs::read_to_string(&mimeapps).unwrap(),
        "[Default Applications]\nimage/png=viewer.desktop;\nimage/jpeg=viewer.desktop;\n"
    );
    assert_eq!(fs::read(captures.join("shot.png")).unwrap(), b"capture");
    // The pre-uninstall configuration is preserved under config-backups.
    let backup = fs::read_dir(home.data.join("tidyshot/config-backups"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    assert_ne!(
        fs::read_to_string(backup.path().join("config/hypr/bindings.lua")).unwrap(),
        tracked_bindings
    );
}

#[test]
fn uninstall_leaves_plugin_checkout_and_foreign_launcher() {
    let home = Home::new("foreign");
    copy_scripts(&home);
    let plugin = home.config.join("omarchy/plugins").join(PLUGIN_ID);
    fs::create_dir_all(plugin.join(".git")).unwrap();
    let launcher = home.home.join(".local/bin/tidyshot");
    fs::write(&launcher, "#!/bin/sh\necho something else\n").unwrap();
    assert!(home.uninstall().status.success());
    assert!(
        plugin.exists(),
        "a managed git checkout belongs to omarchy plugin remove"
    );
    assert_eq!(
        fs::read_to_string(launcher).unwrap(),
        "#!/bin/sh\necho something else\n"
    );
}

#[test]
fn plugin_managed_checkout_is_never_written() {
    let home = Home::new("managed");
    copy_scripts(&home);
    let plugin = home.config.join("omarchy/plugins").join(PLUGIN_ID);
    fs::create_dir_all(plugin.join(".git")).unwrap();
    fs::write(plugin.join("manifest.json"), "from git").unwrap();
    fs::write(
        home.config.join("omarchy/shell.json"),
        r#"{"bar": {"layout": {"right": [{"id": "local.tidyshot"}]}}}"#,
    )
    .unwrap();
    assert!(home.install().status.success());
    assert_eq!(
        fs::read_to_string(plugin.join("manifest.json")).unwrap(),
        "from git"
    );
    assert!(
        !plugin.join("plugin").exists(),
        "a git checkout owns its own files"
    );
    // `omarchy plugin add --enable` placed the widget; the stale legacy entry
    // is dropped rather than duplicated.
    let layout = home.layout();
    assert!(layout.contains(&format!("\"id\": \"{PLUGIN_ID}\"")));
    assert!(!layout.contains("local.tidyshot"));
}

#[test]
fn migration_keeps_legacy_position_over_default_placement() {
    // `omarchy plugin add --enable` places the new id before setup migrates.
    let home = Home::new("position");
    copy_scripts(&home);
    fs::write(
        home.config.join("omarchy/shell.json"),
        format!(
            r#"{{"bar": {{"layout": {{"right": [{{"id": "local.tidyshot", "custom": 1}}, {{"id": "omarchy.tray"}}, {{"id": "{PLUGIN_ID}"}}]}}}}}}"#
        ),
    )
    .unwrap();
    assert!(home.install().status.success());
    let layout = home.layout();
    // The migrated entry keeps its inline settings, and the duplicate the
    // plugin add already placed is dropped.
    assert_eq!(
        layout.matches(&format!("\"id\": \"{PLUGIN_ID}\"")).count(),
        1
    );
    assert!(
        layout.contains("\"custom\": 1"),
        "user settings are kept: {layout}"
    );
}

fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    collect(root, root, &mut out);
    out.sort();
    out
}

fn collect(root: &Path, path: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let entry_path = entry.path();
        if entry_path.is_dir() {
            collect(root, &entry_path, out);
        } else {
            out.push((
                entry_path.strip_prefix(root).unwrap().display().to_string(),
                fs::read(&entry_path).unwrap(),
            ));
        }
    }
}
