//! End-to-end behavior of the real `tidyshot` binary.
//!
//! Flow 1: headless CLI subcommands with a throwaway HOME and their exit
//! codes. Flow 3: the single-instance socket, driven by real processes:
//! launch offscreen, wait for the socket, forward a command, signal the
//! server, and watch a relaunch take over.
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

/// A GUI process started offscreen that owns the instance socket.
///
/// Dropping the guard kills the process, so a failing assertion cannot leave
/// a server behind.
struct Server {
    child: Child,
}

impl Server {
    /// Start `tidyshot` with no arguments; it binds the socket instead of
    /// exiting as a client.
    fn launch(home: &Home) -> Self {
        let child = home
            .command()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Self { child }
    }

    fn pid(&self) -> i32 {
        self.child.id() as i32
    }

    fn running(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }

    /// Wait for the process to exit, bounded; returns false on timeout.
    fn wait_exit(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.child.try_wait().unwrap().is_some() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(POLL);
        }
    }

    /// Ask the process to stop, then reap it within the bound.
    fn terminate(&mut self) {
        // SAFETY: the pid belongs to this child and SIGTERM is catchable.
        unsafe {
            libc::kill(self.pid(), libc::SIGTERM);
        }
        if !self.wait_exit(EXIT_TIMEOUT) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            panic!("the server ignored SIGTERM");
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if self.running() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// Poll until the instance socket accepts a connection, bounded.
///
/// A stale socket file from a signalled run exists but refuses connections,
/// so the file alone is not enough to know the relaunched server is ready.
fn wait_for_listening(home: &Home, timeout: Duration) -> Option<PathBuf> {
    use std::os::unix::net::UnixStream;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(socket) = home.socket()
            && UnixStream::connect(&socket).is_ok()
        {
            return Some(socket);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(POLL);
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
    assert_eq!(
        unknown.status.code(),
        Some(2),
        "unknown command must exit 2"
    );
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

#[test]
fn a_second_invocation_forwards_to_the_running_instance() {
    if missing_binary() {
        return;
    }
    let home = Home::new("forward");
    let mut server = Server::launch(&home);
    let sock = wait_for_listening(&home, START_TIMEOUT)
        .expect("the offscreen instance never bound its socket");
    assert!(server.running(), "the instance exited before the socket");

    // The sender is routed and delivered, then exits 0. The server keeps
    // accepting, so it is still alive afterwards.
    let forwarded = run_bounded(&home, &["close-pins"], EXIT_TIMEOUT);
    assert_eq!(
        forwarded.status.code(),
        Some(0),
        "a forwarded command must exit 0"
    );
    assert!(
        sock.exists(),
        "the delivered command must not remove the socket"
    );
    assert!(
        server.running(),
        "a non-quit command must not stop the server"
    );

    server.terminate();
    // SIGTERM terminates the process without running destructors, so the
    // socket file is left behind; the next launch must replace it.
}

#[test]
fn a_relaunch_replaces_the_socket_and_serves_again() {
    if missing_binary() {
        return;
    }
    let home = Home::new("relaunch");
    let mut first = Server::launch(&home);
    wait_for_listening(&home, START_TIMEOUT).expect("the first instance never bound its socket");
    first.terminate();
    assert!(
        home.socket().is_some(),
        "a signalled instance leaves a stale socket for the relaunch"
    );

    // The relaunch removes the stale socket, binds a fresh one, and serves a
    // forwarded command itself rather than starting a third process.
    let mut second = Server::launch(&home);
    wait_for_listening(&home, START_TIMEOUT)
        .expect("the relaunched instance never bound its socket");
    assert!(second.running(), "the relaunched instance exited");

    let forwarded = run_bounded(&home, &["close-pins"], EXIT_TIMEOUT);
    assert_eq!(forwarded.status.code(), Some(0));
    assert!(second.running(), "the relaunched server stopped early");

    second.terminate();
}
