//! `tidyshot doctor`: check that this machine can run TidyShot, and say how to fix what can't.
// Ported from omnishot/doctor.py
//!
//! Checks are read-only.

use crate::commands::{CommandRunner, CommandSpec, SystemRunner};
use crate::json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// The status of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Fail,
    Skip,
}

impl Status {
    /// The machine-readable status string.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "fail",
            Status::Skip => "skip",
        }
    }
}

/// The mark shown in the human-readable report.
#[must_use]
pub const fn mark(status: Status) -> &'static str {
    match status {
        Status::Ok => "✓",
        Status::Warn => "!",
        Status::Fail => "✗",
        Status::Skip => "-",
    }
}

/// One check's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub id: String,
    pub status: Status,
    pub summary: String,
    pub fix: String,
}

impl CheckResult {
    fn new(id: &str, status: Status, summary: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            id: id.to_owned(),
            status,
            summary: summary.into(),
            fix: fix.into(),
        }
    }
}

/// The native helpers TidyShot builds into `native/` and loads or runs.
///
/// `build.rs` and `bin/native` build `scroll-helper`; nothing else in this
/// tree produces a native binary. The TidyShot plugin libraries have no build
/// here, so they are not checked.
pub const NATIVE_FILES: &[&str] = &["scroll-helper"];

/// Commands the app runs, with the Omarchy package that provides each.
pub const RUNTIME_COMMANDS: &[(&str, &str)] = &[
    ("grim", "grim"),
    ("slurp", "slurp"),
    ("wl-copy", "wl-clipboard"),
    ("ffmpeg", "ffmpeg"),
    ("ffprobe", "ffmpeg"),
    ("tesseract", "tesseract"),
    ("zbarimg", "zbar"),
    ("gpu-screen-recorder", "gpu-screen-recorder"),
    ("parec", "libpulse"),
    ("notify-send", "libnotify"),
];

/// The marker TidyShot writes into the user's Hyprland bindings.
pub const BINDINGS_MARKER: &str = "-- TidyShot managed bindings";
/// The plugin id this build registers with the Omarchy shell.
pub const PLUGIN_ID: &str = "com.jasonrimmer.tidyshot";
/// The plugin id used before the rename, kept so doctor still finds an
/// existing install until the user updates the plugin.
pub const LEGACY_PLUGIN_ID: &str = "io.github.joshdaws.tidyshot";
/// The module the user's Hyprland configuration requires for window rules.
pub const RULES_REQUIRE: &str = "require(\"hypr.tidyshot\")";
/// The Hyprland Lua directory under an Omarchy config directory.
///
/// Omarchy 4 keeps `hyprland.lua` and `bindings.lua` at the top of
/// `~/.config/hypr`; the older layout nests them under
/// `~/.config/omarchy/hypr`. Prefer the top-level layout and fall back to the
/// nested one, matching `tidyshot_hypr_dir` in scripts/runtime.sh and the
/// installer that writes bindings.
#[must_use]
pub fn hypr_dir(config: &Path) -> Option<PathBuf> {
    let top = config.join("hypr");
    if top.join("hyprland.lua").is_file() && top.join("bindings.lua").is_file() {
        return Some(top);
    }
    let nested = config.join("omarchy/hypr");
    if nested.join("hyprland.lua").is_file() && nested.join("bindings.lua").is_file() {
        return Some(nested);
    }
    None
}

/// The shell plugin API settings file, which lives directly under
/// `~/.config/omarchy` in both layouts.
#[must_use]
pub fn shell_settings(config: &Path) -> PathBuf {
    config.join("omarchy/shell.json")
}

/// The version and plugin ABI of the running compositor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HyprlandInfo {
    pub version: String,
    pub abi_hash: String,
}

/// The machine, as the checks see it. Tests substitute their own.
pub trait System {
    /// Look up one environment variable.
    fn env(&self, key: &str) -> Option<String>;
    /// Stdout of a successful command, or None.
    fn run(&self, args: &[&str]) -> Option<String>;
    /// The path of an executable on PATH, or None.
    fn which(&self, name: &str) -> Option<PathBuf>;
    /// `hyprctl -j version` of the running compositor, or None.
    fn hyprland(&self) -> Option<HyprlandInfo>;
    /// The version.h a plugin build would compile against, or None.
    fn header_version_file(&self) -> Option<PathBuf>;
    /// Whether the TidyShot bar widget is responding.
    fn shell_widget(&self) -> bool;
    /// The user's home directory.
    fn home(&self) -> PathBuf;
    /// The XDG config directory.
    fn config(&self) -> PathBuf;
    /// The native helpers directory.
    fn native(&self) -> PathBuf;
}

/// The production machine, backed by the shared command runner.
pub struct RealSystem {
    env: BTreeMap<String, String>,
    home: PathBuf,
    config: PathBuf,
    native: PathBuf,
    runner: SystemRunner,
    hyprland: Mutex<Option<Option<HyprlandInfo>>>,
}

impl RealSystem {
    #[must_use]
    pub fn new() -> Self {
        let env: BTreeMap<String, String> = std::env::vars().collect();
        let home = env
            .get("HOME")
            .map_or_else(|| PathBuf::from("/"), PathBuf::from);
        let config = env
            .get("XDG_CONFIG_HOME")
            .map_or_else(|| home.join(".config"), PathBuf::from);
        let native = env
            .get("TIDYSHOT_NATIVE")
            .map_or_else(|| repo_root().join("native"), PathBuf::from);
        Self {
            env,
            home,
            config,
            native,
            runner: SystemRunner,
            hyprland: Mutex::new(None),
        }
    }
}

impl Default for RealSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl System for RealSystem {
    fn env(&self, key: &str) -> Option<String> {
        self.env.get(key).cloned()
    }

    fn run(&self, args: &[&str]) -> Option<String> {
        let (program, rest) = args.split_first()?;
        let spec = CommandSpec::new(*program)
            .args(rest.iter().copied())
            .timeout(Duration::from_secs(10));
        let output = self.runner.run(&spec).ok()?;
        Some(output.stdout_string().trim().to_owned())
    }

    fn which(&self, name: &str) -> Option<PathBuf> {
        which_on_path(name, self.env.get("PATH").map(String::as_str))
    }

    fn hyprland(&self) -> Option<HyprlandInfo> {
        let mut cache = self.hyprland.lock().unwrap();
        if cache.is_none() {
            *cache = Some(parse_hyprland_info(self.run(&["hyprctl", "-j", "version"])));
        }
        cache.clone().flatten()
    }

    fn header_version_file(&self) -> Option<PathBuf> {
        crate::compat::header_version_file()
    }

    fn shell_widget(&self) -> bool {
        shell_widget_state(|id| self.run(&["omarchy-shell", id, "state"]))
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

/// Whether an `omarchy-shell <id> state` reply comes from a compatible widget.
fn widget_responds(state: Option<String>) -> bool {
    state
        .and_then(|text| crate::json::parse(&text).ok())
        .and_then(|value| value.get("panelApi").and_then(Value::as_f64))
        .is_some_and(|api| api == 1.0)
}

/// Probe the widget by its current id, then the pre-rename id so an install
/// whose plugin has not been updated yet still reports a live bar widget.
fn shell_widget_state(mut probe: impl FnMut(&str) -> Option<String>) -> bool {
    [PLUGIN_ID, LEGACY_PLUGIN_ID]
        .iter()
        .any(|id| widget_responds(probe(id)))
}

/// Find the checkout or installation root from the source env or executable.
fn repo_root() -> PathBuf {
    if let Some(source) = std::env::var_os("TIDYSHOT_SOURCE") {
        return PathBuf::from(source);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(build) = exe.parent()
        && build.file_name().is_some_and(|name| name == "build")
        && let Some(root) = build.parent()
    {
        return root.to_owned();
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Search PATH for an executable, as `shutil.which` does.
fn which_on_path(name: &str, path: Option<&str>) -> Option<PathBuf> {
    let path = path?;
    for directory in std::env::split_paths(path) {
        let candidate = directory.join(name);
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Parse `hyprctl -j version` into the version and ABI hash.
fn parse_hyprland_info(text: Option<String>) -> Option<HyprlandInfo> {
    let value = crate::json::parse(text.as_deref()?).ok()?;
    // Python treats `json.loads(...) or {}` truthiness as reachability: an
    // empty object means the compositor did not answer.
    if !value.is_truthy() {
        return None;
    }
    Some(HyprlandInfo {
        version: value
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        abi_hash: value
            .get("abiHash")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    })
}

fn installer(system: &dyn System) -> String {
    let root = system
        .native()
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    format!("bash {}", root.join("install.sh").display())
}

/// The order the checks run in; compositor checks are marked separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Session,
    Config,
    Versions,
    Headers,
    Native,
    Commands,
    Launcher,
    HyprlandConfig,
    Shell,
}

impl Check {
    /// The result id.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Check::Session => "session",
            Check::Config => "config",
            Check::Versions => "versions",
            Check::Headers => "headers",
            Check::Native => "native",
            Check::Commands => "commands",
            Check::Launcher => "launcher",
            Check::HyprlandConfig => "hyprland-config",
            Check::Shell => "shell",
        }
    }

    /// The label shown when a compositor check is skipped.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Check::Session => "Session check",
            Check::Config => "Configuration check",
            Check::Versions => "Version check",
            Check::Headers => "Hyprland header check",
            Check::Native => "Native helper check",
            Check::Commands => "Command check",
            Check::Launcher => "Launcher check",
            Check::HyprlandConfig => "Hyprland configuration check",
            Check::Shell => "Bar widget check",
        }
    }
}

type CheckFn = fn(&dyn System) -> CheckResult;

const CHECKS: &[(Check, CheckFn)] = &[
    (Check::Session, check_session),
    (Check::Config, check_config),
    (Check::Versions, check_versions),
    (Check::Headers, check_headers),
    (Check::Native, check_native),
    (Check::Commands, check_commands),
    (Check::Launcher, check_launcher),
    (Check::HyprlandConfig, check_hyprland_config),
    (Check::Shell, check_shell),
];

/// These compare against or talk to the running compositor and shell.
const NEEDS_SESSION: &[Check] = &[Check::Headers, Check::Shell];

// Ported from omnishot/doctor.py (check_session)
fn check_session(system: &dyn System) -> CheckResult {
    if system.env("WAYLAND_DISPLAY").is_none()
        || system.env("HYPRLAND_INSTANCE_SIGNATURE").is_none()
    {
        return CheckResult::new(
            "session",
            Status::Fail,
            "Not running inside a Hyprland session",
            "Run this in a terminal inside your Omarchy desktop session.",
        );
    }
    let Some(info) = system.hyprland() else {
        return CheckResult::new(
            "session",
            Status::Fail,
            "hyprctl cannot reach the running compositor",
            "Run this in a terminal inside your Omarchy desktop session.",
        );
    };
    let version = if info.version.is_empty() {
        "?"
    } else {
        &info.version
    };
    CheckResult::new(
        "session",
        Status::Ok,
        format!("Hyprland {version} session"),
        "",
    )
}

// Ported from omnishot/doctor.py (check_config)
fn check_config(system: &dyn System) -> CheckResult {
    let config = system.config();
    let mut missing = Vec::new();
    if hypr_dir(&config).is_none() {
        missing.push("hypr/hyprland.lua");
        missing.push("hypr/bindings.lua");
    }
    if !shell_settings(&config).is_file() {
        missing.push("omarchy/shell.json");
    }
    if !missing.is_empty() {
        return CheckResult::new(
            "config",
            Status::Fail,
            format!("Missing Omarchy configuration: {}", missing.join(", ")),
            "TidyShot needs Omarchy with Lua Hyprland configuration and the shell plugin API.",
        );
    }
    CheckResult::new(
        "config",
        Status::Ok,
        "Omarchy Lua configuration and shell settings found",
        "",
    )
}

// Ported from omnishot/doctor.py (check_versions)
fn check_versions(system: &dyn System) -> CheckResult {
    let info = system.hyprland().unwrap_or_default();
    let omarchy = system.run(&["omarchy", "version"]).unwrap_or_default();
    let hyprland = info.version;
    if omarchy.is_empty() && hyprland.is_empty() {
        return CheckResult::new(
            "versions",
            Status::Warn,
            "Could not read the Omarchy or Hyprland version",
            "",
        );
    }
    let found = crate::compat::untested(&omarchy, &hyprland);
    if !found.is_empty() {
        return CheckResult::new(
            "versions",
            Status::Warn,
            format!(
                "Untested: {} (tested with {})",
                found.join(" and "),
                crate::compat::tested_summary()
            ),
            "Untested versions may work. Report problems with the output of: tidyshot doctor --json",
        );
    }
    CheckResult::new(
        "versions",
        Status::Ok,
        format!("Omarchy {omarchy} with Hyprland {hyprland} (tested)"),
        "",
    )
}

// Ported from omnishot/doctor.py (check_headers)
fn check_headers(system: &dyn System) -> CheckResult {
    let Some(path) = system.header_version_file() else {
        return CheckResult::new(
            "headers",
            Status::Fail,
            "Hyprland development headers not found",
            "Install them with: omarchy pkg add hyprland",
        );
    };
    let Some(info) = system.hyprland() else {
        return CheckResult::new(
            "headers",
            Status::Warn,
            "Headers found, but the running compositor's ABI is unknown",
            "",
        );
    };
    if info.abi_hash.is_empty() {
        return CheckResult::new(
            "headers",
            Status::Warn,
            "Headers found, but the running compositor's ABI is unknown",
            "",
        );
    }
    if crate::compat::header_abi_hash(&path).as_deref() != Some(info.abi_hash.as_str()) {
        return CheckResult::new(
            "headers",
            Status::Fail,
            "Installed Hyprland headers do not match the running compositor",
            format!(
                "Hyprland was updated after this session started. Restart your desktop session, then rebuild TidyShot: {}",
                installer(system)
            ),
        );
    }
    CheckResult::new(
        "headers",
        Status::Ok,
        "Installed Hyprland headers match the running compositor",
        "",
    )
}

// Ported from omnishot/doctor.py (check_native)
fn check_native(system: &dyn System) -> CheckResult {
    let native = system.native();
    let missing: Vec<&str> = NATIVE_FILES
        .iter()
        .copied()
        .filter(|name| !native.join(name).exists())
        .collect();
    if !missing.is_empty() {
        return CheckResult::new(
            "native",
            Status::Fail,
            format!("Native helpers not built: {}", missing.join(", ")),
            format!("Rebuild TidyShot: {}", installer(system)),
        );
    }
    CheckResult::new(
        "native",
        Status::Ok,
        format!("Native helpers built in {}", native.display()),
        "",
    )
}

// Ported from omnishot/doctor.py (check_commands)
fn check_commands(system: &dyn System) -> CheckResult {
    let missing: Vec<&str> = RUNTIME_COMMANDS
        .iter()
        .filter(|(name, _)| system.which(name).is_none())
        .map(|(name, _)| *name)
        .collect();
    if !missing.is_empty() {
        let mut packages: Vec<&str> = Vec::new();
        for name in &missing {
            let package = RUNTIME_COMMANDS
                .iter()
                .find(|(command, _)| command == name)
                .map_or(name, |(_, package)| package);
            if !packages.contains(package) {
                packages.push(package);
            }
        }
        return CheckResult::new(
            "commands",
            Status::Fail,
            format!("Missing commands: {}", missing.join(", ")),
            format!("Install them with: omarchy pkg add {}", packages.join(" ")),
        );
    }
    CheckResult::new(
        "commands",
        Status::Ok,
        "Required commands are installed",
        "",
    )
}

// Ported from omnishot/doctor.py (check_launcher)
fn check_launcher(system: &dyn System) -> CheckResult {
    let launcher = system.home().join(".local/bin/tidyshot");
    if !launcher.is_file() {
        return CheckResult::new(
            "launcher",
            Status::Fail,
            format!("{} is missing", launcher.display()),
            format!("Reinstall TidyShot: {}", installer(system)),
        );
    }
    // The launcher execs this installation's built binary.
    let root = system
        .native()
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let expected = root.join("build/tidyshot");
    let text = std::fs::read_to_string(&launcher).unwrap_or_default();
    if !text.contains(expected.to_string_lossy().as_ref()) {
        return CheckResult::new(
            "launcher",
            Status::Warn,
            format!(
                "{} starts a different TidyShot installation than {}",
                launcher.display(),
                root.display()
            ),
            "Expected if you ran doctor from a development checkout. Otherwise rerun the installer in the installation you use.",
        );
    }
    CheckResult::new(
        "launcher",
        Status::Ok,
        format!("{} starts this installation", launcher.display()),
        "",
    )
}

// Ported from omnishot/doctor.py (check_hyprland_config)
fn check_hyprland_config(system: &dyn System) -> CheckResult {
    let Some(directory) = hypr_dir(&system.config()) else {
        return CheckResult::new(
            "hyprland-config",
            Status::Warn,
            "TidyShot keyboard shortcuts and window rules are not installed",
            format!("Reinstall TidyShot: {}", installer(system)),
        );
    };
    let bindings = directory.join("bindings.lua");
    let hypr = directory.join("hyprland.lua");
    let rules = directory.join("tidyshot.lua");
    let mut missing = Vec::new();
    let bindings_text = std::fs::read_to_string(&bindings).unwrap_or_default();
    if !bindings_text.contains(BINDINGS_MARKER) {
        missing.push("keyboard shortcuts");
    }
    let hypr_text = std::fs::read_to_string(&hypr).unwrap_or_default();
    if !rules.is_file() || !hypr.is_file() || !hypr_text.contains(RULES_REQUIRE) {
        missing.push("window rules");
    }
    if !missing.is_empty() {
        return CheckResult::new(
            "hyprland-config",
            Status::Warn,
            format!("TidyShot {} are not installed", missing.join(" and ")),
            format!("Reinstall TidyShot: {}", installer(system)),
        );
    }
    CheckResult::new(
        "hyprland-config",
        Status::Ok,
        "Keyboard shortcuts and window rules are installed",
        "",
    )
}

// Ported from omnishot/doctor.py (check_shell)
fn check_shell(system: &dyn System) -> CheckResult {
    if !system.shell_widget() {
        return CheckResult::new(
            "shell",
            Status::Warn,
            "The TidyShot bar widget is not responding; TidyShot falls back to its tray icon",
            "Check that it is listed by `omarchy plugin list`, or rerun the installer.",
        );
    }
    CheckResult::new("shell", Status::Ok, "The TidyShot bar widget responds", "")
}

/// Run every check, skipping compositor checks outside a session.
// Ported from omnishot/doctor.py (diagnose)
#[must_use]
pub fn diagnose(system: &dyn System) -> Vec<CheckResult> {
    let mut results = vec![check_session(system)];
    let in_session = results[0].status == Status::Ok;
    for (check, function) in CHECKS.iter().skip(1) {
        if !in_session && NEEDS_SESSION.contains(check) {
            results.push(CheckResult::new(
                check.id(),
                Status::Skip,
                format!("{} skipped: needs the running compositor", check.label()),
                "",
            ));
        } else {
            results.push(function(system));
        }
    }
    results
}

/// Render the human-readable report.
// Ported from omnishot/doctor.py (report)
#[must_use]
pub fn report(results: &[CheckResult]) -> String {
    let mut lines = Vec::new();
    for result in results {
        lines.push(format!("{} {}", mark(result.status), result.summary));
        if !result.fix.is_empty() && result.status != Status::Ok {
            lines.push(format!("  → {}", result.fix));
        }
    }
    let failed = results.iter().filter(|r| r.status == Status::Fail).count();
    let warned = results.iter().filter(|r| r.status == Status::Warn).count();
    lines.push(String::new());
    if failed > 0 || warned > 0 {
        let problems = if failed == 1 { "problem" } else { "problems" };
        let warnings = if warned == 1 { "warning" } else { "warnings" };
        lines.push(format!("{failed} {problems}, {warned} {warnings}"));
    } else {
        lines.push("Everything TidyShot needs looks good.".to_owned());
    }
    lines.join("\n")
}

/// Serialize the results as the `--json` bug-report payload.
// Ported from omnishot/doctor.py (main)
#[must_use]
pub fn json_report(system: &dyn System, results: &[CheckResult]) -> String {
    let mut report = BTreeMap::new();
    report.insert(
        "tidyshot".to_owned(),
        Value::String(env!("CARGO_PKG_VERSION").to_owned()),
    );
    report.insert(
        "hyprland".to_owned(),
        system
            .hyprland()
            .filter(|info| !info.version.is_empty())
            .map_or(Value::Null, |info| Value::String(info.version)),
    );
    let entries = results
        .iter()
        .map(|result| {
            let mut entry = BTreeMap::new();
            entry.insert("id".to_owned(), Value::String(result.id.clone()));
            entry.insert(
                "status".to_owned(),
                Value::String(result.status.id().to_owned()),
            );
            entry.insert("summary".to_owned(), Value::String(result.summary.clone()));
            entry.insert("fix".to_owned(), Value::String(result.fix.clone()));
            Value::Object(entry)
        })
        .collect();
    report.insert("results".to_owned(), Value::Array(entries));
    Value::Object(report).to_string_pretty()
}

/// Parse `--json`, run the checks, and return the exit code.
// Ported from omnishot/doctor.py (main)
pub fn main(argv: &[String], system: &dyn System) -> i32 {
    main_writer(argv, system, &mut std::io::stdout())
}

/// The same, writing the report to `out` so callers can capture it.
pub fn main_writer(argv: &[String], system: &dyn System, out: &mut dyn std::io::Write) -> i32 {
    let mut json = false;
    for argument in argv {
        match argument.as_str() {
            "--json" => json = true,
            other => {
                eprintln!("tidyshot doctor: unrecognized argument: {other}");
                return 2;
            }
        }
    }
    let results = diagnose(system);
    let rendered = if json {
        json_report(system, &results)
    } else {
        report(&results)
    };
    writeln!(out, "{rendered}").ok();
    if results.iter().any(|r| r.status == Status::Fail) {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipped_check_labels_are_stable() {
        assert_eq!(Check::Headers.id(), "headers");
        assert_eq!(Check::Shell.label(), "Bar widget check");
        assert_eq!(mark(Status::Fail), "✗");
    }

    #[test]
    fn the_shell_probe_tries_the_current_id_before_the_legacy_id() {
        assert_eq!(PLUGIN_ID, "com.jasonrimmer.tidyshot");
        assert_eq!(LEGACY_PLUGIN_ID, "io.github.joshdaws.tidyshot");

        let mut asked = Vec::new();
        let found = shell_widget_state(|id| {
            asked.push(id.to_owned());
            (id == LEGACY_PLUGIN_ID).then(|| r#"{"panelApi": 1}"#.to_owned())
        });
        assert!(found, "the legacy widget answers for an existing install");
        assert_eq!(asked, [PLUGIN_ID, LEGACY_PLUGIN_ID]);

        let mut asked = Vec::new();
        let found = shell_widget_state(|id| {
            asked.push(id.to_owned());
            (id == PLUGIN_ID).then(|| r#"{"panelApi": 1}"#.to_owned())
        });
        assert!(found);
        assert_eq!(
            asked,
            [PLUGIN_ID],
            "a current widget stops after the first probe"
        );
    }

    #[test]
    fn which_finds_an_executable_under_path() {
        let dir = std::env::temp_dir().join(format!("tidyshot-which-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = dir.join("grim");
        std::fs::write(&tool, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let found = which_on_path("grim", Some(dir.to_string_lossy().as_ref()));
        assert_eq!(found.as_deref(), Some(tool.as_path()));
        assert!(which_on_path("missing", Some(dir.to_string_lossy().as_ref())).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn config_with(root: &Path, hypr: &Path) {
        std::fs::create_dir_all(hypr).unwrap();
        std::fs::write(hypr.join("hyprland.lua"), b"-- mine\n").unwrap();
        std::fs::write(hypr.join("bindings.lua"), b"-- mine\n").unwrap();
        std::fs::create_dir_all(root.join("omarchy")).unwrap();
        std::fs::write(root.join("omarchy/shell.json"), b"{}\n").unwrap();
    }

    #[test]
    fn finds_the_top_level_hyprland_layout() {
        let root = std::env::temp_dir().join(format!("tidyshot-top-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        config_with(&root, &root.join("hypr"));
        assert_eq!(
            hypr_dir(&root).as_deref(),
            Some(root.join("hypr").as_path())
        );
        assert!(shell_settings(&root).is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn falls_back_to_the_nested_hyprland_layout() {
        let root = std::env::temp_dir().join(format!("tidyshot-nested-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        config_with(&root, &root.join("omarchy/hypr"));
        assert_eq!(
            hypr_dir(&root).as_deref(),
            Some(root.join("omarchy/hypr").as_path())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prefers_the_top_level_layout_when_both_exist() {
        let root = std::env::temp_dir().join(format!("tidyshot-both-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        config_with(&root, &root.join("hypr"));
        config_with(&root, &root.join("omarchy/hypr"));
        assert_eq!(
            hypr_dir(&root).as_deref(),
            Some(root.join("hypr").as_path())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_hyprland_config_is_reported() {
        let root = std::env::temp_dir().join(format!("tidyshot-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert!(hypr_dir(&root).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
