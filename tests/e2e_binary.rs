//! End-to-end behavior of the real `tidyshot` binary.
//!
//! Flow 1: headless CLI subcommands with a throwaway HOME and their exit
//! codes.
//!
//! Every wait is bounded and a missing binary skips the test rather than
//! failing, so the file is safe to run on a machine without a build.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_tidyshot");
const START_TIMEOUT: Duration = Duration::from_secs(10);
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(20);

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A throwaway HOME and runtime directory for one test.
///
/// Both are removed on drop, so an e2e run never touches the real user's
/// settings or instance socket.
struct Home {
    root: PathBuf,
}

impl Home {
    fn new(name: &str) -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "tidyshot-e2e-{}-{name}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home")).unwrap();
        fs::create_dir_all(root.join("run")).unwrap();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::create_dir_all(root.join("data")).unwrap();
        Self { root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn run(&self) -> PathBuf {
        self.root.join("run")
    }

    fn state_dir(&self) -> PathBuf {
        self.run().join("tidyshot")
    }

    /// The instance socket this HOME/XDG pair would create, if present.
    fn socket(&self) -> Option<PathBuf> {
        for entry in fs::read_dir(self.state_dir()).ok()?.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|name| name.to_str());
            if name.is_some_and(|name| name.starts_with("tidyshot-") && name.ends_with(".sock")) {
                return Some(path);
            }
        }
        None
    }

    /// A command for this throwaway environment, never the real one.
    fn command(&self) -> Command {
        let mut command = Command::new(BIN);
        command
            .env("HOME", self.home())
            .env("XDG_RUNTIME_DIR", self.run())
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("QT_QPA_PLATFORM", "offscreen");
        command
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Run a command and wait for it with a bound, so a client that turns into a
/// server can never hang the test.
fn run_bounded(home: &Home, args: &[&str], timeout: Duration) -> Output {
    let mut child = home
        .command()
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("tidyshot {args:?} did not exit within {timeout:?}");
        }
        std::thread::sleep(POLL);
    }
}

fn missing_binary() -> bool {
    if Path::new(BIN).is_file() {
        false
    } else {
        eprintln!("skipping e2e: tidyshot binary not built at {BIN}");
        true
    }
}

#[test]
fn headless_subcommands_use_the_throwaway_home_and_exit_codes() {
    if missing_binary() {
        return;
    }
    let home = Home::new("cli");

    // A listing is a success and produces output.
    let menu = run_bounded(&home, &["menu"], EXIT_TIMEOUT);
    assert_eq!(menu.status.code(), Some(0), "menu must exit 0");
    assert!(!menu.stdout.is_empty(), "menu must list actions");

    // An unknowable command is a usage error, reported on stderr.
    let unknown = run_bounded(&home, &["frobnicate"], EXIT_TIMEOUT);
    assert_eq!(unknown.status.code(), Some(2), "unknown command must exit 2");
    let stderr = String::from_utf8_lossy(&unknown.stderr);
    assert!(stderr.contains("unknown command"), "stderr was: {stderr}");

    // A compat invocation with no subcommand is a usage error.
    let usage = run_bounded(&home, &["compat"], EXIT_TIMEOUT);
    assert_eq!(usage.status.code(), Some(2), "compat usage must exit 2");

    // Doctor is headless; its report is JSON whether it finds a healthy
    // machine (0) or reports problems (1).
    let doctor = run_bounded(&home, &["doctor", "--json"], EXIT_TIMEOUT);
    let code = doctor.status.code();
    assert!(
        matches!(code, Some(0) | Some(1)),
        "doctor --json exited {code:?}"
    );
    let stdout = String::from_utf8_lossy(&doctor.stdout);
    let report = tidyshot::json::parse(&stdout).expect("doctor --json must print JSON");
    assert!(
        report.get("results").is_some(),
        "doctor report needs results: {stdout}"
    );

    // None of these start the GUI or the single-instance socket.
    assert!(
        home.socket().is_none(),
        "headless subcommands must not bind the instance socket"
    );
}
