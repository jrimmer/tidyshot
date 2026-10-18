//! Clean-capture mirror lifecycle for the Hyprland plugin.
// Ported from omnishot/clean_capture.py (MirrorLease, SelectionMirror, CursorMirror)
//!
//! The compositor plugin is loaded once and shared by every consumer. On CPUs
//! where Hyprland cannot hook functions (aarch64) the plugin load reports an
//! error while `hyprctl` still exits 0, so the caller degrades to a plain
//! capture instead of treating it as a fatal setup problem.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::commands::{CommandError, CommandRunner, CommandSpec, hypr};
use crate::json::Value;

/// File name of the compositor mirror plugin.
pub const MIRROR_PLUGIN: &str = "clean-mirror.so";
/// File name of the capture-source plugin.
pub const CAPTURE_PLUGIN: &str = "clean-capture.so";
/// The plugin name reported by `hyprctl plugin list`.
pub const PLUGIN_NAME: &str = "omnishot-clean-mirror";

/// The rebuild instructions shown when the plugin cannot be made to work.
pub const REBUILD: &str =
    "TidyShot's capture components need rebuilding. Run bin/build from the TidyShot folder.";

/// Hyprland cannot hook functions on this CPU, so the clean mirror cannot load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorUnavailable {
    message: String,
}

impl MirrorUnavailable {
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for MirrorUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MirrorUnavailable {}

/// Why a mirror lease could not be started.
#[derive(Debug)]
pub enum MirrorError {
    /// The compositor cannot load the plugin; degrade to a plain capture.
    Unavailable(MirrorUnavailable),
    /// The installed components are missing or built for a different ABI.
    Rebuild(String),
    /// A hyprctl command failed.
    Command(CommandError),
}

impl fmt::Display for MirrorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MirrorError::Unavailable(error) => error.fmt(f),
            MirrorError::Rebuild(message) => f.write_str(message),
            MirrorError::Command(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for MirrorError {}

impl From<CommandError> for MirrorError {
    fn from(error: CommandError) -> Self {
        MirrorError::Command(error)
    }
}

#[derive(Debug, Default)]
struct MirrorState {
    loaded_path: Option<PathBuf>,
    unavailable: Option<String>,
    roles: BTreeMap<&'static str, i64>,
}

/// Shared plugin state: the loaded plugin path, the cached degrade reason and
/// the per-role consumer counts.
#[derive(Debug)]
pub struct MirrorRegistry {
    state: Mutex<MirrorState>,
    arch: String,
}

impl Default for MirrorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MirrorRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::with_arch(std::env::consts::ARCH)
    }

    /// Construct a registry that reports `arch` as the host CPU, matching
    /// `platform.machine()` in the source.
    #[must_use]
    pub fn with_arch(arch: impl Into<String>) -> Self {
        Self {
            state: Mutex::new(MirrorState::default()),
            arch: arch.into(),
        }
    }

    /// A lease for one consumer role.
    #[must_use]
    pub fn lease(&self, role: &'static str) -> MirrorLease<'_> {
        MirrorLease {
            registry: self,
            role,
            enabled: false,
        }
    }

    /// The process-wide registry used by production code.
    #[must_use]
    pub fn global() -> &'static MirrorRegistry {
        static REGISTRY: OnceLock<MirrorRegistry> = OnceLock::new();
        REGISTRY.get_or_init(MirrorRegistry::new)
    }

    /// True when `hyprctl plugin list` reports the mirror plugin.
    // Ported from omnishot/clean_capture.py (MirrorLease.loaded)
    pub fn loaded<R: CommandRunner>(&self, runner: &R) -> Result<bool, CommandError> {
        let plugins = hypr(runner, "plugin list")?;
        Ok(plugins
            .as_array()
            .is_some_and(|items| items.iter().any(is_mirror)))
    }

    /// Unload the plugin once no role is registered and nothing is active.
    // Ported from omnishot/clean_capture.py (MirrorLease.unload_unused)
    fn unload_unused<R: CommandRunner>(
        &self,
        runner: &R,
        state: &mut MirrorState,
    ) -> Result<(), CommandError> {
        if state.roles.values().any(|count| *count > 0) {
            return Ok(());
        }
        let Some(path) = state.loaded_path.clone() else {
            return Ok(());
        };
        let spec = CommandSpec::new("hyprctl").args([
            "repl",
            "return tostring(hl.plugin.omnishot.capture_active())",
        ]);
        let active = runner.run(&spec)?.stdout_string();
        if active.trim() == "false" {
            let path = path.to_string_lossy().into_owned();
            let spec = CommandSpec::new("hyprctl").args(["plugin", "unload", &path]);
            runner.run(&spec)?;
            state.loaded_path = None;
        }
        Ok(())
    }

    /// Unload the plugin if it is no longer needed.
    pub fn release_unused<R: CommandRunner>(&self, runner: &R) -> Result<(), CommandError> {
        let mut state = self.state.lock().expect("mirror state poisoned");
        self.unload_unused(runner, &mut state)
    }

    /// Acquire the mirror for `role`, loading the plugin when needed.
    ///
    /// The role count is only incremented on success, so a failed attempt
    /// leaves no phantom consumer behind.
    // Ported from omnishot/clean_capture.py (MirrorLease.start)
    pub fn acquire<R: CommandRunner>(
        &self,
        role: &'static str,
        runner: &R,
        native: &Path,
    ) -> Result<(), MirrorError> {
        let mut state = self.state.lock().expect("mirror state poisoned");
        if state.roles.get(role).copied().unwrap_or(0) > 0 {
            *state.roles.entry(role).or_insert(0) += 1;
            return Ok(());
        }
        if let Some(message) = &state.unavailable {
            return Err(MirrorError::Unavailable(MirrorUnavailable {
                message: message.clone(),
            }));
        }
        let plugin = native.join(MIRROR_PLUGIN);
        if !plugin.is_file() {
            // TidyShot has no native clean-mirror build, so an absent plugin
            // is the normal case: degrade once and cache it, exactly like a
            // compositor that cannot hook the function.
            let message =
                "The clean-capture compositor plugin is not installed; using the plain capture path"
                    .to_owned();
            state.unavailable = Some(message.clone());
            return Err(MirrorError::Unavailable(MirrorUnavailable { message }));
        }
        if !self.loaded(runner)? {
            let path = std::fs::canonicalize(&plugin)
                .map_err(|error| MirrorError::Rebuild(format!("{REBUILD}\n\n{error}")))?;
            // hyprctl exits 0 even when the plugin fails to initialize.
            let path_text = path.to_string_lossy().into_owned();
            let spec = CommandSpec::new("hyprctl").args(["plugin", "load", &path_text]);
            let output = runner.run(&spec)?.stdout_string();
            if !self.loaded(runner)? {
                let output = output.trim().to_owned();
                if !output.contains("Could not hook") && self.arch == "x86_64" {
                    // e.g. an ABI mismatch after a Hyprland upgrade: fixable.
                    return Err(MirrorError::Rebuild(format!("{REBUILD}\n\n{output}")));
                }
                let message = if output.is_empty() {
                    "The TidyShot compositor plugin could not be loaded".to_owned()
                } else {
                    output
                };
                state.unavailable = Some(message.clone());
                return Err(MirrorError::Unavailable(MirrorUnavailable { message }));
            }
            state.loaded_path = Some(path);
        }
        let spec = CommandSpec::new("hyprctl").args([
            "eval",
            &format!("hl.plugin.omnishot.{role}({})", std::process::id()),
        ]);
        if let Err(error) = runner.run(&spec) {
            let _ = self.unload_unused(runner, &mut state);
            return Err(MirrorError::Command(error));
        }
        *state.roles.entry(role).or_insert(0) += 1;
        Ok(())
    }

    /// Release the mirror for `role`.
    // Ported from omnishot/clean_capture.py (MirrorLease.stop)
    pub fn release<R: CommandRunner>(
        &self,
        role: &'static str,
        enabled: bool,
        runner: &R,
    ) -> Result<(), CommandError> {
        let mut state = self.state.lock().expect("mirror state poisoned");
        if !enabled {
            return self.unload_unused(runner, &mut state);
        }
        let current = state.roles.get(role).copied().unwrap_or(0);
        let remaining = current.saturating_sub(1);
        if remaining == 0 {
            let spec = CommandSpec::new("hyprctl")
                .args(["eval", &format!("hl.plugin.omnishot.{role}(0)")]);
            runner.run(&spec)?;
        }
        state.roles.insert(role, remaining);
        self.unload_unused(runner, &mut state)
    }
}

fn is_mirror(plugin: &Value) -> bool {
    plugin.get("name").and_then(Value::as_str) == Some(PLUGIN_NAME)
}

/// One consumer's hold on the shared compositor mirror.
#[derive(Debug)]
pub struct MirrorLease<'a> {
    registry: &'a MirrorRegistry,
    role: &'static str,
    enabled: bool,
}

impl MirrorLease<'_> {
    /// Whether this consumer currently holds the mirror.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Start the mirror for this role, loading the plugin when needed.
    // Ported from omnishot/clean_capture.py (MirrorLease.start)
    pub fn start<R: CommandRunner>(
        &mut self,
        runner: &R,
        native: &Path,
    ) -> Result<(), MirrorError> {
        if self.enabled {
            return Ok(());
        }
        self.registry.acquire(self.role, runner, native)?;
        self.enabled = true;
        Ok(())
    }

    /// Start the mirror, tolerating a degraded compositor.
    ///
    /// Without the plugin grim's own cursor flag still applies, so a cursor
    /// mirror failure must not stop the capture.
    pub fn start_best_effort<R: CommandRunner>(
        &mut self,
        runner: &R,
        native: &Path,
    ) -> Result<(), MirrorError> {
        match self.start(runner, native) {
            Err(MirrorError::Unavailable(_)) => Ok(()),
            other => other,
        }
    }

    /// Release this consumer's hold on the mirror.
    // Ported from omnishot/clean_capture.py (MirrorLease.stop)
    pub fn stop<R: CommandRunner>(&mut self, runner: &R) -> Result<(), CommandError> {
        if !self.enabled {
            return self.registry.release(self.role, false, runner);
        }
        self.registry.release(self.role, true, runner)?;
        self.enabled = false;
        Ok(())
    }
}

/// A selection mirror, used while a live overlay is on screen.
#[derive(Debug)]
pub struct SelectionMirror {
    role: &'static str,
}

impl SelectionMirror {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            role: "selection_capture",
        }
    }

    #[must_use]
    pub const fn role(&self) -> &'static str {
        self.role
    }
}

impl Default for SelectionMirror {
    fn default() -> Self {
        Self::new()
    }
}

/// A cursor mirror, best effort: without the plugin grim's cursor flag applies.
#[derive(Debug)]
pub struct CursorMirror {
    role: &'static str,
}

impl CursorMirror {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            role: "cursor_capture",
        }
    }

    #[must_use]
    pub const fn role(&self) -> &'static str {
        self.role
    }
}

impl Default for CursorMirror {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CommandOutput;
    use std::sync::Mutex as StdMutex;

    fn output(stdout: &str) -> CommandOutput {
        CommandOutput {
            status: 0,
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    /// A runner that models hyprctl's plugin registry and records calls.
    struct Fake {
        arch_load_output: StdMutex<String>,
        plugins: StdMutex<Vec<String>>,
        calls: StdMutex<Vec<Vec<String>>>,
        registered: StdMutex<BTreeMap<String, bool>>,
        fail_registration: StdMutex<bool>,
    }

    impl Fake {
        fn new(load_output: &str) -> Self {
            Self {
                arch_load_output: StdMutex::new(load_output.to_owned()),
                plugins: StdMutex::new(Vec::new()),
                calls: StdMutex::new(Vec::new()),
                registered: StdMutex::new(BTreeMap::new()),
                fail_registration: StdMutex::new(false),
            }
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }

        fn set_plugins(&self, names: &[&str]) {
            *self.plugins.lock().unwrap() = names.iter().map(|n| (*n).to_owned()).collect();
        }

        fn plugin_calls(&self) -> Vec<String> {
            self.calls()
                .into_iter()
                .filter(|args| args.first().map(String::as_str) == Some("plugin"))
                .filter_map(|args| args.get(1).cloned())
                .collect()
        }
    }

    impl CommandRunner for Fake {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            let args = spec.args.clone();
            self.calls.lock().unwrap().push(args.clone());
            match args.as_slice() {
                [flag, query]
                    if spec.program == "hyprctl" && flag == "-j" && query == "plugin list" =>
                {
                    let names = self.plugins.lock().unwrap().clone();
                    let rows: Vec<String> = names
                        .iter()
                        .map(|name| format!("{{\"name\":\"{name}\"}}"))
                        .collect();
                    Ok(output(&format!("[{}]", rows.join(","))))
                }
                [action, sub, ..]
                    if spec.program == "hyprctl" && action == "plugin" && sub == "load" =>
                {
                    let text = self.arch_load_output.lock().unwrap().clone();
                    if !text.contains("Could not hook") && !text.contains("ABI mismatch") {
                        self.plugins.lock().unwrap().push(PLUGIN_NAME.to_owned());
                    } else {
                        return Ok(output(&text));
                    }
                    Ok(output(&text))
                }
                [action, sub, ..]
                    if spec.program == "hyprctl" && action == "plugin" && sub == "unload" =>
                {
                    self.plugins.lock().unwrap().clear();
                    Ok(output("ok"))
                }
                [action, ..] if spec.program == "hyprctl" && action == "repl" => {
                    let active = self.registered.lock().unwrap().values().any(|on| *on);
                    Ok(output(if active { "true" } else { "false" }))
                }
                [action, expression] if spec.program == "hyprctl" && action == "eval" => {
                    if *self.fail_registration.lock().unwrap() {
                        return Err(CommandError::Failure {
                            program: "hyprctl".to_owned(),
                            output: CommandOutput {
                                status: 1,
                                stdout: Vec::new(),
                                stderr: b"registration failed".to_vec(),
                            },
                        });
                    }
                    let role = expression
                        .split("omnishot.")
                        .nth(1)
                        .and_then(|rest| rest.split('(').next())
                        .unwrap_or("")
                        .to_owned();
                    let on = !expression.ends_with("(0)");
                    self.registered.lock().unwrap().insert(role, on);
                    Ok(output("ok"))
                }
                _ => Ok(output("ok")),
            }
        }
    }

    fn native_dir() -> (tempdir::TempDir, PathBuf) {
        let dir = tempdir::TempDir::new();
        let native = dir.path().to_path_buf();
        for name in [MIRROR_PLUGIN, CAPTURE_PLUGIN] {
            std::fs::write(native.join(name), b"plugin").unwrap();
        }
        (dir, native)
    }

    #[test]
    fn rejected_plugin_degrades_to_plain_capture() {
        // aarch64 Hyprland cannot hook functions; hyprctl still exits 0.
        let (_dir, native) = native_dir();
        let registry = MirrorRegistry::with_arch("aarch64");
        let runner = Fake::new(
            "Plugin x could not be loaded: plugin crashed/threw in main: Could not hook IElementRenderer::preDrawSurface",
        );

        let mut cursor = registry.lease(CursorMirror::new().role());
        cursor.start_best_effort(&runner, &native).unwrap();
        assert!(!cursor.enabled());

        let mut selection = registry.lease(SelectionMirror::new().role());
        let error = selection.start(&runner, &native).unwrap_err();
        assert!(matches!(error, MirrorError::Unavailable(_)), "{error:?}");
        assert!(error.to_string().contains("preDrawSurface"), "{error}");
        assert_eq!(runner.plugin_calls(), ["load"], "load attempted once");
    }

    #[test]
    fn x86_load_failure_asks_for_rebuild_without_caching() {
        let (_dir, native) = native_dir();
        let registry = MirrorRegistry::with_arch("x86_64");
        let runner = Fake::new(
            "Plugin x could not be loaded: plugin crashed/threw in main: OmniShot compositor ABI mismatch",
        );

        for _ in 0..2 {
            let mut selection = registry.lease(SelectionMirror::new().role());
            let error = selection.start(&runner, &native).unwrap_err();
            let text = error.to_string();
            assert!(matches!(error, MirrorError::Rebuild(_)), "{error:?}");
            assert!(text.contains("rebuilding"), "{text}");
            assert!(text.contains("ABI mismatch"), "{text}");
        }
        // A fixable failure is not cached, so a rebuild can recover.
        assert_eq!(runner.plugin_calls(), ["load", "load"]);
    }

    #[test]
    fn consumers_share_one_plugin_and_release_the_last_hold() {
        let (_dir, native) = native_dir();
        let registry = MirrorRegistry::new();
        let runner = Fake::new("ok");

        let mut first = registry.lease(SelectionMirror::new().role());
        let mut second = registry.lease(SelectionMirror::new().role());
        let mut recording = registry.lease("clean_capture");
        let mut scrolling = registry.lease(CursorMirror::new().role());
        first.start(&runner, &native).unwrap();
        second.start(&runner, &native).unwrap();
        recording.start(&runner, &native).unwrap();
        scrolling.start(&runner, &native).unwrap();

        first.stop(&runner).unwrap();
        assert!(runner.registered.lock().unwrap()["selection_capture"]);
        assert!(!runner.plugins.lock().unwrap().is_empty());

        recording.stop(&runner).unwrap();
        second.stop(&runner).unwrap();
        assert!(!runner.plugins.lock().unwrap().is_empty());
        scrolling.stop(&runner).unwrap();
        assert!(runner.plugins.lock().unwrap().is_empty());
        assert_eq!(runner.plugin_calls(), ["load", "unload"]);
    }

    #[test]
    fn failed_registration_unloads_the_plugin() {
        let (_dir, native) = native_dir();
        let registry = MirrorRegistry::new();
        let runner = Fake::new("ok");
        *runner.fail_registration.lock().unwrap() = true;

        let mut selection = registry.lease(SelectionMirror::new().role());
        let error = selection.start(&runner, &native).unwrap_err();
        assert!(error.to_string().contains("registration"), "{error}");
        assert!(runner.plugins.lock().unwrap().is_empty());
        assert!(!selection.enabled());
    }

    #[test]
    fn missing_components_degrade() {
        let dir = tempdir::TempDir::new();
        let registry = MirrorRegistry::new();
        let runner = Fake::new("ok");
        let mut selection = registry.lease(SelectionMirror::new().role());
        let error = selection
            .start(&runner, dir.path())
            .expect_err("missing plugin is reported");
        assert!(matches!(error, MirrorError::Unavailable(_)), "{error:?}");
        assert!(
            runner.calls().is_empty(),
            "nothing is run when files are missing"
        );
    }

    #[test]
    fn an_already_loaded_plugin_is_not_reloaded() {
        let (_dir, native) = native_dir();
        let registry = MirrorRegistry::new();
        let runner = Fake::new("ok");
        runner.set_plugins(&[PLUGIN_NAME]);

        let mut selection = registry.lease(SelectionMirror::new().role());
        selection.start(&runner, &native).unwrap();
        assert!(selection.enabled());
        assert_eq!(runner.plugin_calls(), Vec::<String>::new());
        selection.stop(&runner).unwrap();
    }

    /// Minimal scratch directory that removes itself.
    mod tempdir {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);

        pub struct TempDir(PathBuf);

        impl TempDir {
            pub fn new() -> Self {
                let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir()
                    .join(format!("tidyshot-mirror-{}-{nonce}", std::process::id()));
                std::fs::create_dir_all(&path).unwrap();
                Self(path)
            }

            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
