//! setup.sh / uninstall.sh against a real git plugin checkout in a fake HOME.
//!
//! These are the tidyshot port of omnishot's `tests/test_setup.py`: they use a
//! throwaway HOME, stub the desktop commands on PATH, and never touch a live
//! session. The scripts refuse to run as root, so a root test process drops to
//! an unprivileged uid for every script and git call inside the fixture.
//!
//! Release tests use a fixture directory through `TIDYSHOT_RELEASE_DIR` and
//! fake the machine architecture through `TIDYSHOT_UNAME`, so they need no
//! network access. `TIDYSHOT_RELEASE_URL` is exercised through a stubbed curl
//! that serves a local tree; the real GitHub URL is used only by real installs.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const PLUGIN_ID: &str = "com.jasonrimmer.tidyshot";
const NOBODY: (u32, u32) = (65534, 65534);

/// The install stub the fixture uses in place of the real Rust build.
const INSTALL: &str = r#"printf 'install ran in %s\n' "$PWD" >>"$SETUP_LOG.install"
if [ -z "${TIDYSHOT_PREBUILT:-}" ]; then
  cargo build --release --offline
else
  printf 'prebuilt %s\n' "${TIDYSHOT_PREBUILT}" >>"$SETUP_LOG.prebuilt"
fi
mkdir -p native .venv backups/stamp
printf built >native/helper.so
printf venv >.venv/marker
exit "${INSTALL_EXIT:-0}"
"#;

/// Stub desktop commands; records each call and fakes the few behaviors the
/// scripts rely on.
const STUB: &str = r#"#!/usr/bin/env python3
import json, os, pathlib, shutil, sys
name = pathlib.Path(sys.argv[0]).name
with open(os.environ['SETUP_LOG'], 'a') as log:
    log.write(json.dumps([name, *sys.argv[1:]]) + '\n')
if name == 'pgrep':
    sys.exit(0 if os.environ.get('APP_RUNNING') else 1)
if name == 'omarchy' and sys.argv[1:3] == ['plugin', 'remove']:
    shutil.rmtree(pathlib.Path(os.environ['XDG_CONFIG_HOME']) / 'omarchy/plugins' / sys.argv[3])
if name == 'curl':
    # Serve the path below TIDYSHOT_RELEASE_URL from a local directory, so a
    # test exercises the URL setup builds without any network access.
    url = sys.argv[-1]
    base = os.environ['TIDYSHOT_RELEASE_URL'].rstrip('/')
    assert url.startswith(base + '/'), url
    output = sys.argv[sys.argv.index('--output') + 1]
    shutil.copy(os.path.join(base, url[len(base) + 1:]), output)
"#;

struct Fixture {
    base: PathBuf,
    home: PathBuf,
    config: PathBuf,
    data: PathBuf,
    source: PathBuf,
    runtime: PathBuf,
    bin: PathBuf,
    log: PathBuf,
    install_log: PathBuf,
    prebuilt_log: PathBuf,
    head: String,
    /// Uid/gid to run scripts as, or `None` when the test user is unprivileged.
    drop: Option<(u32, u32)>,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let base = std::env::temp_dir().join(format!(
            "tidyshot-setup-{}-{nonce}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let home = base.join("home");
        let config = home.join(".config");
        let data = home.join(".local/share");
        fs::create_dir_all(config.join("hypr")).unwrap();
        fs::create_dir_all(config.join("omarchy")).unwrap();
        fs::write(config.join("hypr/hyprland.lua"), "-- mine\n").unwrap();
        fs::write(config.join("hypr/bindings.lua"), "-- mine\n").unwrap();
        fs::write(
            config.join("omarchy/shell.json"),
            format!(r#"{{"bar": {{"layout": {{"right": [{{"id": "{PLUGIN_ID}"}}]}}}}}}"#),
        )
        .unwrap();

        let source = config.join("omarchy/plugins").join(PLUGIN_ID);
        fs::create_dir_all(source.join("scripts")).unwrap();
        for script in [
            "setup.sh",
            "uninstall.sh",
            "runtime.sh",
            "manifest.sh",
            "uninstall_user.py",
        ] {
            fs::copy(
                repo.join("scripts").join(script),
                source.join("scripts").join(script),
            )
            .unwrap();
        }
        for file in ["manifest.json", ".gitignore"] {
            fs::copy(repo.join(file), source.join(file)).unwrap();
        }
        fs::create_dir_all(source.join("plugin")).unwrap();
        fs::copy(
            repo.join("plugin/BarWidget.qml"),
            source.join("plugin/BarWidget.qml"),
        )
        .unwrap();
        fs::write(source.join("install.sh"), INSTALL).unwrap();
        fs::write(source.join("removed-later.txt"), "old").unwrap();

        git(&source, &["init", "-q"]);
        git(&source, &["add", "-A"]);
        git(&source, &["commit", "-qm", "one"]);
        let head = git_stdout(&source, &["rev-parse", "HEAD"]);

        let bin = base.join("bin");
        fs::create_dir_all(&bin).unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        for stub in ["omarchy", "hyprctl", "pgrep", "setsid", "cargo", "curl"] {
            let path = bin.join(stub);
            fs::write(&path, STUB).unwrap();
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).unwrap();
        }

        let is_root = Command::new("id").arg("-u").output().unwrap().stdout == b"0\n";
        let drop = if is_root {
            chown_recursive(&base, NOBODY.0, NOBODY.1);
            Some(NOBODY)
        } else {
            None
        };

        Self {
            install_log: base.join("commands.jsonl.install"),
            prebuilt_log: base.join("commands.jsonl.prebuilt"),
            log: base.join("commands.jsonl"),
            runtime: data.join("tidyshot-app"),
            base,
            home,
            config,
            data,
            source,
            bin,
            head,
            drop,
        }
    }

    fn env(&self) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = std::env::vars().collect();
        env.retain(|(key, _)| key != "TIDYSHOT_INSTALL_DIR");
        env.push(("HOME".into(), self.home.display().to_string()));
        env.push(("XDG_CONFIG_HOME".into(), self.config.display().to_string()));
        env.push(("XDG_DATA_HOME".into(), self.data.display().to_string()));
        env.push((
            "PATH".into(),
            format!(
                "{}:{}",
                self.bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        ));
        env.push(("SETUP_LOG".into(), self.log.display().to_string()));
        // Keep every fixture off the network and on one architecture; release
        // tests override these with their own release directory.
        env.push((
            "TIDYSHOT_RELEASE_DIR".into(),
            self.base.join("no-release").display().to_string(),
        ));
        env.push(("TIDYSHOT_UNAME".into(), "x86_64".into()));
        env.push(("WAYLAND_DISPLAY".into(), "test".into()));
        env.push(("HYPRLAND_INSTANCE_SIGNATURE".into(), "test".into()));
        env
    }

    /// Run a script, allowing the caller to override environment variables.
    fn run(&self, script: &str, extra: &[(&str, &str)]) -> Output {
        let mut command = Command::new("bash");
        command.arg(self.source.join("scripts").join(script));
        command.stdin(Stdio::null());
        self.execute(command, extra)
    }

    fn run_git(&self, args: &[&str]) -> Output {
        let mut command = Command::new("git");
        command.args(["-C", &self.source.display().to_string()]);
        command.args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "safe.directory=*",
        ]);
        command.args(args);
        self.execute(command, &[])
    }

    fn execute(&self, mut command: Command, extra: &[(&str, &str)]) -> Output {
        let env = self.env();
        command.env_clear();
        for (key, value) in &env {
            command.env(key, value);
        }
        for (key, value) in extra {
            command.env(key, value);
        }
        match self.drop {
            None => command.output().unwrap(),
            Some((uid, gid)) => {
                let mut wrapped = Command::new("setpriv");
                wrapped.args([
                    "--reuid",
                    &uid.to_string(),
                    "--regid",
                    &gid.to_string(),
                    "--clear-groups",
                ]);
                wrapped.arg(command.get_program());
                wrapped.args(command.get_args());
                for (key, value) in &env {
                    wrapped.env(key, value);
                }
                for (key, value) in extra {
                    wrapped.env(key, value);
                }
                wrapped.output().unwrap()
            }
        }
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

    fn commit(&self, message: &str) -> String {
        assert!(self.run_git(&["add", "-A"]).status.success());
        assert!(self.run_git(&["commit", "-qm", message]).status.success());
        String::from_utf8(self.run_git(&["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_owned()
    }

    fn write(&self, name: &str, contents: &str) {
        let path = self.source.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn install_log(&self) -> String {
        fs::read_to_string(&self.install_log).unwrap_or_default()
    }

    fn prebuilt_log(&self) -> String {
        fs::read_to_string(&self.prebuilt_log).unwrap_or_default()
    }

    fn release(&self, commit: &str) -> Release {
        Release::write(self, commit, ReleaseFlaw::None)
    }

    fn release_with(&self, commit: &str, flaw: ReleaseFlaw) -> Release {
        Release::write(self, commit, flaw)
    }

    /// Give a path the same ownership the scripts will run with, so a test
    /// that stages files before running them is writable when uid is dropped.
    fn own(&self, path: &Path) {
        if let Some((uid, gid)) = self.drop {
            chown_recursive(path, uid, gid);
        }
    }
}

/// Absolute path of a command, so a test can keep it reachable after
/// replacing PATH.
fn command_path(name: &str) -> String {
    let output = Command::new("sh")
        .args(["-c", &format!("command -v {name}")])
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// The version in the repository's own `manifest.json`, read from the real
/// bytes that `scripts/setup.sh` parses. Parsing it as JSON here keeps this
/// side independent of the shell `sed`; the release-name coupling then proves
/// both readers agree when the fixture archive is named from this value.
fn manifest_version() -> String {
    let bytes = fs::read(repo_root().join("manifest.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    manifest["version"].as_str().unwrap().to_owned()
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn release_archive(version: &str) -> String {
    format!("tidyshot-{version}-x86_64.tar.gz")
}

#[derive(Clone, Copy)]
enum ReleaseFlaw {
    None,
    Checksum,
    OmitTarball,
    UnsafePath,
    Symlink,
    /// Links front-loaded ahead of a blob, so a check that stops reading the
    /// listing kills tar with SIGPIPE instead of seeing the links.
    ManySymlinks,
    Hardlink,
    Fifo,
    Setuid,
}

/// A fake release directory in the shape `bin/make-release-artifacts` writes:
/// a tarball rooted at the runtime layout (build/tidyshot plus the source
/// commit stamp) and a checksums.txt naming it.
struct Release {
    dir: PathBuf,
}

impl Release {
    fn write(fixture: &Fixture, commit: &str, flaw: ReleaseFlaw) -> Self {
        let version = manifest_version();
        let archive_name = release_archive(&version);
        let dir = fixture.base.join("release");
        let stage = fixture.base.join("release-stage");
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&stage);
        fs::create_dir_all(stage.join("build")).unwrap();
        fs::write(stage.join("build/tidyshot"), "#!/bin/sh\necho prebuilt\n").unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        let binary = stage.join("build/tidyshot");
        let mut perms = fs::metadata(&binary).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&binary, perms).unwrap();
        fs::write(stage.join(".tidyshot-source"), format!("{commit}\n")).unwrap();
        if matches!(flaw, ReleaseFlaw::Symlink) {
            std::os::unix::fs::symlink("/etc/passwd", stage.join("build/escape")).unwrap();
        }
        if matches!(flaw, ReleaseFlaw::Hardlink) {
            fs::hard_link(stage.join("build/tidyshot"), stage.join("build/escape")).unwrap();
        }
        if matches!(flaw, ReleaseFlaw::Fifo) {
            let status = Command::new("mkfifo")
                .arg(stage.join("build/fifo"))
                .status()
                .unwrap();
            assert!(status.success(), "could not create the fixture fifo");
        }
        if matches!(flaw, ReleaseFlaw::Setuid) {
            let mut perms = fs::metadata(&binary).unwrap().permissions();
            perms.set_mode(0o4755);
            fs::set_permissions(&binary, perms).unwrap();
        }
        if matches!(flaw, ReleaseFlaw::ManySymlinks) {
            // Front-load hundreds of links, then a multi-megabyte file: a
            // correctness check that reads the whole listing sees the links,
            // while one that stops early lets tar die of SIGPIPE.
            for index in 0..300 {
                std::os::unix::fs::symlink("/etc/passwd", stage.join(format!("link-{index}")))
                    .unwrap();
            }
            fs::write(stage.join("build/blob"), vec![b'x'; 4 * 1024 * 1024]).unwrap();
        }
        fs::create_dir_all(&dir).unwrap();
        if !matches!(flaw, ReleaseFlaw::OmitTarball) {
            let archive = dir.join(&archive_name);
            let mut command = Command::new("tar");
            command
                .args(["--owner=0", "--group=0", "--numeric-owner", "-czf"])
                .arg(&archive);
            if matches!(flaw, ReleaseFlaw::UnsafePath) {
                // Rewrite one member to climb out of the extraction directory.
                command.arg(r"--transform=s|^\./build/tidyshot$|../escaped|");
            }
            let status = command
                .args(["-C", &stage.display().to_string(), "."])
                .status()
                .unwrap();
            assert!(status.success(), "could not create the fixture tarball");
        }
        // The tarball is fetched before any comparison runs, so the digest for
        // a missing tarball is never read; it only has to be shaped like one.
        let digest = if matches!(flaw, ReleaseFlaw::Checksum) {
            "0".repeat(64)
        } else if matches!(flaw, ReleaseFlaw::OmitTarball) {
            "1".repeat(64)
        } else {
            let output = Command::new("sha256sum")
                .arg(dir.join(&archive_name))
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout)
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap()
                .to_owned()
        };
        fs::write(
            dir.join("checksums.txt"),
            format!("{digest}  {archive_name}\n"),
        )
        .unwrap();
        fixture.own(&dir);
        Self { dir }
    }

    fn env(&self) -> (&str, String) {
        ("TIDYSHOT_RELEASE_DIR", self.dir.display().to_string())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.drop.is_some() {
            let _ = Command::new("chown")
                .args(["-R", "0:0", &self.base.display().to_string()])
                .status();
        }
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn git(source: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-C", &source.display().to_string()])
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "safe.directory=*",
        ])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn git_stdout(source: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-C", &source.display().to_string()])
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "safe.directory=*",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn chown_recursive(path: &Path, uid: u32, gid: u32) {
    let status = Command::new("chown")
        .args(["-R", &format!("{uid}:{gid}"), &path.display().to_string()])
        .status()
        .unwrap();
    assert!(status.success(), "chown failed");
}

/// Minimal JSON array reader for the stubbed command log.
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

/// Snapshot every file in a directory tree except VCS metadata.
fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut map = BTreeMap::new();
    collect(root, root, &mut map);
    map
}

fn collect(root: &Path, path: &Path, map: &mut BTreeMap<String, Vec<u8>>) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let entry_path = entry.path();
        if entry_path.file_name().unwrap() == ".git" {
            continue;
        }
        if entry_path.is_dir() {
            collect(root, &entry_path, map);
        } else {
            let relative = entry_path.strip_prefix(root).unwrap().display().to_string();
            map.insert(relative, fs::read(&entry_path).unwrap());
        }
    }
}

#[test]
fn builds_a_copy_outside_the_plugin_checkout() {
    let fixture = Fixture::new("copy");
    let result = fixture.run("setup.sh", &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fixture.install_log().trim(),
        format!("install ran in {}", fixture.runtime.display())
    );
    assert_eq!(
        fs::read_to_string(fixture.runtime.join("removed-later.txt")).unwrap(),
        "old"
    );
    assert!(!fixture.runtime.join(".git").exists());
    assert_eq!(
        fs::read_to_string(fixture.runtime.join(".tidyshot-source"))
            .unwrap()
            .trim(),
        fixture.head
    );
    // Build outputs stay out of the plugin folder the validator checks.
    assert!(!fixture.source.join("native").exists());
    assert!(!fixture.source.join(".venv").exists());
    assert!(
        fixture
            .calls()
            .iter()
            .any(|call| call.iter().take(3).eq(["omarchy", "pkg", "add"]))
    );
}

#[test]
fn update_mirrors_source_but_keeps_build_outputs() {
    let fixture = Fixture::new("update");
    assert!(fixture.run("setup.sh", &[]).status.success());
    fs::remove_file(fixture.source.join("removed-later.txt")).unwrap();
    fixture.write("added.txt", "new");
    let head = fixture.commit("two");
    let result = fixture.run("setup.sh", &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!fixture.runtime.join("removed-later.txt").exists());
    assert_eq!(
        fs::read_to_string(fixture.runtime.join("added.txt")).unwrap(),
        "new"
    );
    assert!(fixture.runtime.join("native/helper.so").exists());
    assert!(fixture.runtime.join(".venv/marker").exists());
    assert!(fixture.runtime.join("backups/stamp").is_dir());
    assert_eq!(
        fs::read_to_string(fixture.runtime.join(".tidyshot-source"))
            .unwrap()
            .trim(),
        head
    );
}

#[test]
fn update_restarts_the_shell_only_when_the_widget_changed() {
    let fixture = Fixture::new("restart");
    let restart = vec![
        "omarchy".to_owned(),
        "restart".to_owned(),
        "shell".to_owned(),
    ];
    assert!(fixture.run("setup.sh", &[]).status.success());
    assert!(!fixture.calls().contains(&restart));
    fixture.write("added.txt", "new");
    fixture.commit("app only");
    assert!(fixture.run("setup.sh", &[]).status.success());
    assert!(!fixture.calls().contains(&restart));
    fixture.write("plugin/BarWidget.qml", "Item {}");
    fixture.commit("widget");
    assert!(fixture.run("setup.sh", &[]).status.success());
    assert_eq!(
        fixture
            .calls()
            .into_iter()
            .filter(|call| call == &restart)
            .count(),
        1
    );
}

#[test]
fn failed_build_is_left_unstamped() {
    let fixture = Fixture::new("failed");
    let result = fixture.run("setup.sh", &[("INSTALL_EXIT", "3")]);
    assert!(!result.status.success());
    assert!(!fixture.runtime.join(".tidyshot-source").exists());
}

#[test]
fn moves_a_pre_plugin_checkout_aside() {
    let fixture = Fixture::new("legacy");
    fs::create_dir_all(fixture.runtime.join(".git")).unwrap();
    fs::create_dir_all(fixture.runtime.join("backups")).unwrap();
    fs::write(fixture.runtime.join("backups/old.lua"), "backup").unwrap();
    fixture.own(&fixture.runtime);
    fixture.own(&fixture.data);
    let result = fixture.run("setup.sh", &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let moved: Vec<_> = fs::read_dir(&fixture.data)
        .unwrap()
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("tidyshot-app.pre-plugin-")
        })
        .collect();
    assert_eq!(moved.len(), 1);
    assert_eq!(
        fs::read_to_string(moved[0].path().join("backups/old.lua")).unwrap(),
        "backup"
    );
    assert!(!fixture.runtime.join(".git").exists());
}

#[test]
fn refuses_an_unknown_folder_before_installing_packages() {
    let fixture = Fixture::new("foreign");
    fs::create_dir_all(&fixture.runtime).unwrap();
    fs::write(fixture.runtime.join("mine.txt"), "keep").unwrap();
    fixture.own(&fixture.runtime);
    let result = fixture.run("setup.sh", &[]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("not created by TidyShot setup"), "{stderr}");
    assert_eq!(
        fs::read_to_string(fixture.runtime.join("mine.txt")).unwrap(),
        "keep"
    );
    assert!(
        !fixture
            .calls()
            .iter()
            .any(|call| call.first().map(String::as_str) == Some("omarchy"))
    );
}

#[test]
fn refuses_while_the_app_will_not_quit() {
    let fixture = Fixture::new("running");
    fs::create_dir_all(&fixture.runtime).unwrap();
    fixture.own(&fixture.runtime);
    let result = fixture.run(
        "setup.sh",
        &[
            ("APP_RUNNING", "1"),
            ("TIDYSHOT_SKIP_PACKAGES", "1"),
            ("TIDYSHOT_QUIT_WAIT", "0"),
        ],
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("still running"), "{stderr}");
    assert!(!fixture.install_log.exists());
}

#[test]
fn accepts_the_nested_omarchy_layout() {
    let fixture = Fixture::new("nestedguard");
    let nested = fixture.config.join("omarchy/hypr");
    fs::create_dir_all(&nested).unwrap();
    fs::rename(
        fixture.config.join("hypr/hyprland.lua"),
        nested.join("hyprland.lua"),
    )
    .unwrap();
    fs::rename(
        fixture.config.join("hypr/bindings.lua"),
        nested.join("bindings.lua"),
    )
    .unwrap();
    fs::remove_dir(fixture.config.join("hypr")).unwrap();
    fixture.own(&nested);
    fixture.own(&fixture.config.join("omarchy"));
    let result = fixture.run("setup.sh", &[]);
    assert!(
        result.status.success(),
        "a nested Hyprland layout must be accepted: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(fixture.install_log().contains("install ran"));
}

#[test]
fn refuses_when_neither_hyprland_layout_is_present() {
    let fixture = Fixture::new("nolayout");
    fs::remove_file(fixture.config.join("hypr/hyprland.lua")).unwrap();
    let result = fixture.run("setup.sh", &[]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("Lua Hyprland configuration"), "{stderr}");
    assert!(!fixture.install_log.exists());
}

/// The runtime helpers must detect the real built binary, not the Python
/// launcher the reference used. Drive the actual `pgrep` against a process
/// whose command line starts with the launcher's binary path.
#[test]
fn running_detects_and_stops_the_built_binary() {
    use std::os::unix::process::CommandExt as _;
    let fixture = Fixture::new("detect");
    let app_dir = fixture.runtime.join("build");
    fs::create_dir_all(&app_dir).unwrap();
    let app = app_dir.join("tidyshot");
    fixture.own(&fixture.runtime);

    // The PATH deliberately omits the fixture's `pgrep` stub so the real
    // process table is inspected, exactly as on a desktop.
    let real_path = std::env::var("PATH").unwrap_or_default();
    // `arg0` makes the command line begin with the launcher's binary path,
    // which is what `tidyshot_running` matches.
    let mut child = Command::new("sleep").arg("300").arg0(&app).spawn().unwrap();
    let check = |call: &str| {
        let mut command = Command::new("bash");
        command.arg("-c").arg(format!(
            "source '{0}/scripts/runtime.sh'; {call}",
            fixture.source.display()
        ));
        command.env_clear();
        command.env("PATH", &real_path);
        command.env("HOME", &fixture.home);
        command.env("TIDYSHOT_INSTALL_DIR", &fixture.runtime);
        command
    };
    let before = check("tidyshot_running && echo yes || echo no")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&before.stdout).trim(), "yes");

    let stopped = check("tidyshot_stop && echo stopped || echo alive")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&stopped.stdout).trim(), "stopped");
    let _ = child.kill();
    let status = child.wait().unwrap();
    assert!(!status.success(), "the app was terminated");
}

#[test]
fn requires_a_plugin_checkout() {
    let fixture = Fixture::new("nocheckout");
    fs::remove_dir_all(fixture.source.join(".git")).unwrap();
    let result = fixture.run("setup.sh", &[]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("bash install.sh"), "{stderr}");
}

#[test]
fn uninstall_removes_app_files_and_plugin() {
    let fixture = Fixture::new("uninstall");
    assert!(fixture.run("setup.sh", &[]).status.success());
    let captures = fixture.data.join("tidyshot/captures");
    fs::create_dir_all(&captures).unwrap();
    fs::write(captures.join("shot.png"), b"keep").unwrap();
    fixture.own(&fixture.data.join("tidyshot"));
    let result = fixture.run("uninstall.sh", &[("TIDYSHOT_ASSUME_YES", "1")]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!fixture.runtime.exists());
    assert!(!fixture.source.exists());
    let expected = vec![
        "omarchy".to_owned(),
        "plugin".to_owned(),
        "remove".to_owned(),
        PLUGIN_ID.to_owned(),
        "--yes".to_owned(),
    ];
    assert!(fixture.calls().contains(&expected));
    assert_eq!(fs::read(captures.join("shot.png")).unwrap(), b"keep");
    let shell = fs::read_to_string(fixture.config.join("omarchy/shell.json")).unwrap();
    assert!(parse_layout(&shell, "right").is_empty());
}

#[test]
fn uninstall_needs_confirmation_and_keeps_development_checkouts() {
    let fixture = Fixture::new("keep");
    let result = fixture.run("uninstall.sh", &[]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("confirmation"), "{stderr}");
    fs::create_dir_all(fixture.runtime.join(".git")).unwrap();
    fs::write(fixture.runtime.join(".tidyshot-source"), "x").unwrap();
    fixture.own(&fixture.runtime);
    fixture.own(&fixture.home.join(".local"));
    let output = fixture.run("uninstall.sh", &[("TIDYSHOT_ASSUME_YES", "1")]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fixture.runtime.join(".git").exists());
}

/// The mirror recreates a source directory that holds no file, matching
/// `rsync -a` rather than losing empty tracked trees.
#[test]
fn mirror_recreates_empty_source_directories() {
    let fixture = Fixture::new("emptydir");
    assert!(fixture.run("setup.sh", &[]).status.success());
    fs::create_dir_all(fixture.source.join("assets/icons")).unwrap();
    fixture.own(&fixture.source.join("assets"));
    let result = fixture.run("setup.sh", &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        fixture.runtime.join("assets/icons").is_dir(),
        "an empty source directory is mirrored"
    );
}

#[test]
fn update_removes_files_and_empty_directories_the_source_dropped() {
    let fixture = Fixture::new("prune");
    assert!(fixture.run("setup.sh", &[]).status.success());
    // A tracked directory the next revision removes must disappear from the
    // runtime, not just its files.
    fs::create_dir_all(fixture.source.join("stale/deep")).unwrap();
    fs::write(fixture.source.join("stale/deep/keep.txt"), "gone").unwrap();
    fixture.commit("add stale");
    assert!(fixture.run("setup.sh", &[]).status.success());
    assert!(fixture.runtime.join("stale/deep/keep.txt").is_file());
    fs::remove_dir_all(fixture.source.join("stale")).unwrap();
    fixture.commit("drop stale");
    assert!(fixture.run("setup.sh", &[]).status.success());
    assert!(
        !fixture.runtime.join("stale").exists(),
        "stale tree is pruned"
    );
    // The build outputs the source never tracked still survive.
    assert!(fixture.runtime.join("native/helper.so").exists());
}

#[test]
fn install_round_trip_is_byte_exact() {
    let fixture = Fixture::new("roundtrip");
    assert!(fixture.run("setup.sh", &[]).status.success());
    let first = snapshot(&fixture.runtime);
    // A no-op rerun must not change a single byte of the mirrored checkout.
    assert!(fixture.run("setup.sh", &[]).status.success());
    assert_eq!(snapshot(&fixture.runtime), first);
    // A tracked source file changing flows through byte for byte.
    fixture.write("added.txt", "payload");
    assert!(fixture.run("setup.sh", &[]).status.success());
    assert_eq!(
        fs::read_to_string(fixture.runtime.join("added.txt")).unwrap(),
        "payload"
    );
    assert_eq!(
        snapshot(&fixture.source.join("plugin")),
        snapshot(&fixture.runtime.join("plugin"))
    );
}

/// The mirror preserves a symlink the way `rsync -a` does, instead of
/// dereferencing it by content, and never aborts on a broken one.
#[test]
fn mirror_preserves_symlinks_without_following_them() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new("symlink");
    assert!(fixture.run("setup.sh", &[]).status.success());
    // A good link points at a tracked file; a broken link points at nothing.
    symlink("manifest.json", fixture.source.join("good-link")).unwrap();
    symlink("does-not-exist", fixture.source.join("broken-link")).unwrap();
    let result = fixture.run("setup.sh", &[]);
    assert!(
        result.status.success(),
        "a broken symlink must not abort setup: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let good = fixture.runtime.join("good-link");
    let broken = fixture.runtime.join("broken-link");
    assert!(
        fs::symlink_metadata(&good)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_link(&good).unwrap(), Path::new("manifest.json"));
    assert!(
        fs::symlink_metadata(&broken)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_link(&broken).unwrap(), Path::new("does-not-exist"));
}

/// The package step must cover the tools its own build hard-requires:
/// `install.sh` exits when cargo or qmake6 is missing.
#[test]
fn installs_the_build_toolchain_it_requires() {
    let fixture = Fixture::new("packages");
    assert!(fixture.run("setup.sh", &[]).status.success());
    let package_call = fixture
        .calls()
        .into_iter()
        .find(|call| call.iter().take(3).eq(["omarchy", "pkg", "add"]))
        .expect("setup installs missing packages");
    for package in [
        "git",
        "rust",
        "qt6-base",
        "qt6-declarative",
        "ffmpeg",
        "tesseract",
        "zbar",
    ] {
        assert!(
            package_call.iter().any(|arg| arg == package),
            "the package list must include {package}: {package_call:?}"
        );
    }
}

/// Extract the ids from one `shell.json` layout section without a JSON dep.
fn parse_layout(text: &str, section: &str) -> Vec<String> {
    let needle = format!("\"{section}\"");
    let Some(start) = text.find(&needle) else {
        return Vec::new();
    };
    let rest = &text[start..];
    let Some(open) = rest.find('[') else {
        return Vec::new();
    };
    let Some(close) = rest[open..].find(']') else {
        return Vec::new();
    };
    let body = &rest[open..open + close];
    body.match_indices("\"id\"")
        .map(|(index, _)| {
            let after = &body[index..];
            let colon = after.find(':').unwrap();
            let after = &after[colon + 1..];
            let quote = after.find('"').unwrap();
            let after = &after[quote + 1..];
            let end = after.find('"').unwrap();
            after[..end].to_owned()
        })
        .collect()
}

fn run_release(fixture: &Fixture, release: &Release, extra: &[(&str, &str)]) -> Output {
    let (key, value) = release.env();
    let mut env: Vec<(&str, &str)> = vec![(key, value.as_str()), ("TIDYSHOT_SKIP_PACKAGES", "1")];
    env.extend_from_slice(extra);
    fixture.run("setup.sh", &env)
}

fn cargo_called(fixture: &Fixture) -> bool {
    fixture
        .calls()
        .iter()
        .any(|call| call.first().map(String::as_str) == Some("cargo"))
}

#[test]
fn head_match_installs_the_prebuilt_release_without_cargo() {
    let fixture = Fixture::new("release-match");
    let release = fixture.release(&fixture.head);
    let result = run_release(&fixture, &release, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains(&format!(
            "installing the prebuilt {}",
            release_archive(&manifest_version())
        )),
        "{stdout}"
    );
    assert_eq!(
        fs::read_to_string(fixture.runtime.join("build/tidyshot")).unwrap(),
        "#!/bin/sh\necho prebuilt\n"
    );
    assert!(!fixture.prebuilt_log().is_empty());
    assert!(!cargo_called(&fixture), "the source build must not run");
    assert_eq!(
        fs::read_to_string(fixture.runtime.join(".tidyshot-source"))
            .unwrap()
            .trim(),
        fixture.head
    );
}

#[test]
fn head_mismatch_falls_back_to_the_source_build() {
    let fixture = Fixture::new("release-mismatch");
    let release = fixture.release(&"d".repeat(40));
    let result = run_release(&fixture, &release, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("not the checkout HEAD"), "{stderr}");
    assert!(cargo_called(&fixture));
    assert!(fixture.prebuilt_log().is_empty());
}

#[test]
fn checksum_mismatch_falls_back_and_does_not_stage_the_release() {
    let fixture = Fixture::new("release-checksum");
    let release = fixture.release_with(&fixture.head, ReleaseFlaw::Checksum);
    let result = run_release(&fixture, &release, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("checksum mismatch"), "{stderr}");
    assert!(cargo_called(&fixture));
    assert!(fixture.prebuilt_log().is_empty());
}

#[test]
fn a_missing_tarball_falls_back_to_the_source_build() {
    let fixture = Fixture::new("release-missing");
    let release = fixture.release_with(&fixture.head, ReleaseFlaw::OmitTarball);
    let result = run_release(&fixture, &release, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains(&release_archive(&manifest_version())),
        "{stderr}"
    );
    assert!(stderr.contains("unavailable"), "{stderr}");
    assert!(cargo_called(&fixture));
    assert!(fixture.prebuilt_log().is_empty());
}

/// The plugin manifest is the only source of the release version, so setup
/// must read it from the real bytes. A fixture release named for the version
/// in the repository's own manifest only installs when that read works.
#[test]
fn version_comes_from_the_real_manifest_bytes() {
    let fixture = Fixture::new("release-version");
    let release = fixture.release(&fixture.head);
    let result = run_release(&fixture, &release, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let version = manifest_version();
    assert!(!version.is_empty());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains(&release_archive(&version)),
        "setup must name the manifest version's archive: {stdout}"
    );
    assert_eq!(
        fs::read_to_string(fixture.runtime.join("build/tidyshot")).unwrap(),
        "#!/bin/sh\necho prebuilt\n"
    );
    assert!(!cargo_called(&fixture), "the source build must not run");
}

#[test]
fn a_tarball_member_commanding_a_parent_path_falls_back() {
    let fixture = Fixture::new("release-traversal");
    let release = fixture.release_with(&fixture.head, ReleaseFlaw::UnsafePath);
    let result = run_release(&fixture, &release, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("unsafe member"), "{stderr}");
    assert!(cargo_called(&fixture));
    assert!(fixture.prebuilt_log().is_empty());
    assert!(!fixture.base.join("escaped").exists());
}

#[test]
fn another_architecture_falls_back_to_the_source_build() {
    let fixture = Fixture::new("release-arch");
    let release = fixture.release(&fixture.head);
    let result = run_release(&fixture, &release, &[("TIDYSHOT_UNAME", "aarch64")]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("no prebuilt binary for aarch64"),
        "{stderr}"
    );
    assert!(cargo_called(&fixture));
    assert!(fixture.prebuilt_log().is_empty());
}

/// A PATH with the stubs `install.sh` needs and the system utilities it calls,
/// but no Rust or Qt toolchain, so a test proves the prebuilt branch never
/// reaches for one.
fn toolchain_free_path(fixture: &Fixture) -> PathBuf {
    let dir = fixture.base.join("release-bin");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    // A root test process spawns through setpriv, so it must stay reachable
    // inside the stripped PATH.
    for name in [
        "bash", "cat", "chmod", "cp", "dirname", "find", "grep", "head", "id", "mkdir", "mktemp",
        "pgrep", "python3", "rm", "sed", "setpriv", "sleep", "sort", "tar", "tr", "uname", "wc",
    ] {
        let target = command_path(name);
        symlink(&target, dir.join(name)).unwrap();
    }
    for name in ["omarchy", "hyprctl", "setsid"] {
        symlink(fixture.bin.join(name), dir.join(name)).unwrap();
    }
    for name in [
        "omarchy-shell",
        "grim",
        "slurp",
        "wl-copy",
        "ffmpeg",
        "ffprobe",
        "tesseract",
        "zbarimg",
        "gpu-screen-recorder",
        "update-desktop-database",
        "update-mime-database",
        "xdg-mime",
        "notify-send",
        "gcc",
        "pkg-config",
        "wayland-scanner",
    ] {
        symlink("/bin/true", dir.join(name)).unwrap();
    }
    let mut perms = fs::metadata(&dir).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&dir, perms).unwrap();
    fixture.own(&dir);
    dir
}

/// The real `install.sh`, not the fixture stub: a staged release must install
/// with the auto-scroll helper built locally and no cargo or qmake on PATH.
#[test]
fn the_real_install_script_takes_a_prebuilt_release_without_the_toolchain() {
    let fixture = Fixture::new("prebuilt-install");
    let repo = repo_root();
    fs::copy(repo.join("install.sh"), fixture.source.join("install.sh")).unwrap();
    fs::create_dir_all(fixture.source.join("bin")).unwrap();
    fs::write(
        fixture.source.join("bin/native"),
        "#!/bin/sh\nmkdir -p native\nprintf helper >native/scroll-helper\nprintf helper >native/window-helper\nchmod +x native/scroll-helper native/window-helper\n",
    )
    .unwrap();
    fs::write(
        fixture.source.join("scripts/install_user.py"),
        "open(__import__('os').environ['SETUP_LOG'] + '.user', 'a').write('installed\\n')\n",
    )
    .unwrap();
    fs::create_dir_all(fixture.source.join("build")).unwrap();
    fs::write(
        fixture.source.join("build/tidyshot"),
        "#!/bin/sh\necho prebuilt\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    for path in [
        fixture.source.join("bin/native"),
        fixture.source.join("build/tidyshot"),
    ] {
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
    }
    fixture.own(&fixture.source);

    let path = toolchain_free_path(&fixture);
    let bash = command_path("bash");
    let mut command = Command::new(&bash);
    command.arg(fixture.source.join("install.sh"));
    command.stdin(Stdio::null());
    let result = fixture.execute(
        command,
        &[("PATH", path.to_str().unwrap()), ("TIDYSHOT_PREBUILT", "1")],
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{stderr}");
    assert!(stderr.contains("prebuilt"), "{stderr}");
    assert!(fixture.source.join("native/scroll-helper").exists());
    assert!(fixture.source.join("native/window-helper").exists());
    assert_eq!(
        fs::read_to_string(fixture.source.join("build/native/scroll-helper")).unwrap(),
        "helper"
    );
    assert_eq!(
        fs::read_to_string(fixture.source.join("build/native/window-helper")).unwrap(),
        "helper"
    );
    assert!(!cargo_called(&fixture), "the source build must not run");
    assert_eq!(
        fs::read_to_string(fixture.base.join("commands.jsonl.user")).unwrap(),
        "installed\n"
    );
}

#[test]
fn a_tarball_member_that_is_a_symlink_falls_back() {
    let fixture = Fixture::new("release-symlink");
    let release = fixture.release_with(&fixture.head, ReleaseFlaw::Symlink);
    let result = run_release(&fixture, &release, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("unsafe member"), "{stderr}");
    assert!(cargo_called(&fixture));
    assert!(fixture.prebuilt_log().is_empty());
}

/// A link check that stops reading the listing lets tar die of SIGPIPE; under
/// pipefail that pipeline failure must not read as "safe". The archive holds
/// hundreds of links ahead of a multi-megabyte blob, which overflows the pipe.
#[test]
fn links_ahead_of_a_large_member_are_still_rejected() {
    let fixture = Fixture::new("release-many-links");
    let release = fixture.release_with(&fixture.head, ReleaseFlaw::ManySymlinks);
    let result = run_release(&fixture, &release, &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("unsafe member"), "{stderr}");
    assert!(cargo_called(&fixture));
    assert!(fixture.prebuilt_log().is_empty());
    assert!(!fixture.runtime.join("link-0").exists());
}

#[test]
fn a_hardlink_member_falls_back() {
    let fixture = Fixture::new("release-hardlink");
    let release = fixture.release_with(&fixture.head, ReleaseFlaw::Hardlink);
    let result = run_release(&fixture, &release, &[]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{stderr}");
    assert!(stderr.contains("unsafe member"), "{stderr}");
    assert!(cargo_called(&fixture));
    assert!(fixture.prebuilt_log().is_empty());
}

#[test]
fn a_fifo_member_falls_back() {
    let fixture = Fixture::new("release-fifo");
    let release = fixture.release_with(&fixture.head, ReleaseFlaw::Fifo);
    let result = run_release(&fixture, &release, &[]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{stderr}");
    assert!(stderr.contains("unsafe member"), "{stderr}");
    assert!(cargo_called(&fixture));
    assert!(fixture.prebuilt_log().is_empty());
    assert!(!fixture.runtime.join("build/fifo").exists());
}

#[test]
fn a_setuid_member_falls_back() {
    let fixture = Fixture::new("release-setuid");
    let release = fixture.release_with(&fixture.head, ReleaseFlaw::Setuid);
    let result = run_release(&fixture, &release, &[]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{stderr}");
    assert!(stderr.contains("unsafe member"), "{stderr}");
    assert!(cargo_called(&fixture));
    assert!(fixture.prebuilt_log().is_empty());
}

/// `TIDYSHOT_RELEASE_URL` overrides the release download root. Until now only
/// the local-dir override was covered, so the URL setup builds (including the
/// `v<version>` path segment) was never exercised.
#[test]
fn the_release_url_override_supplies_the_download_root() {
    let fixture = Fixture::new("release-url");
    let release = fixture.release(&fixture.head);
    // Arrange the local directory as a release host: v<version>/<file>.
    let root = fixture.base.join("release-host");
    let versioned = root.join(format!("v{}", manifest_version()));
    fs::create_dir_all(&versioned).unwrap();
    for name in [
        "checksums.txt".to_owned(),
        release_archive(&manifest_version()),
    ] {
        fs::copy(release.dir.join(&name), versioned.join(name)).unwrap();
    }
    fixture.own(&root);
    let url = root.display().to_string();
    let result = run_release(
        &fixture,
        &release,
        &[("TIDYSHOT_RELEASE_URL", &url), ("TIDYSHOT_RELEASE_DIR", "")],
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{stderr}");
    assert!(
        String::from_utf8_lossy(&result.stdout).contains(&format!(
            "installing the prebuilt {}",
            release_archive(&manifest_version())
        )),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    assert!(!cargo_called(&fixture), "the source build must not run");
    assert!(!fixture.prebuilt_log().is_empty());
}

/// The extraction itself, not just its effect on the archive name: run the
/// shared shell parser against the repository's own manifest bytes and compare
/// it with an independent JSON read. This is the regression for the sed that
/// carried literal double backslashes and returned nothing.
#[test]
fn the_shell_parser_reads_the_real_manifest_version() {
    let script = format!(
        "source '{}'; tidyshot_manifest_version '{}'",
        repo_root().join("scripts/manifest.sh").display(),
        repo_root().join("manifest.json").display()
    );
    let output = Command::new("bash").args(["-c", &script]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let printed = String::from_utf8(output.stdout).unwrap();
    assert_eq!(printed.trim(), manifest_version());
    assert!(!printed.trim().is_empty());
}
