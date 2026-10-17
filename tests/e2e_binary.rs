//! End-to-end behavior of the real `tidyshot` binary.
//!
//! Flow 1: headless CLI subcommands with a throwaway HOME and their exit
//! codes. Flow 2: the interactive OCR capture path with stubbed `slurp`,
//! `grim` and `hyprctl` on a temp PATH, driven until the reachable prefix is
//! done. Flow 3: the single-instance socket, driven by real processes: launch
//! offscreen, wait for the socket, forward a command, signal the server, and
//! watch a relaunch take over. Flow 4: a `.omnishot` project created by the
//! library and accepted by the binary's headless surface.
//!
//! Every wait is bounded and a missing binary skips the test rather than
//! failing, so the file is safe to run on a machine without a build.
//!
//! Flows 2 and 4 stop at the GUI boundary. Flow 2 runs the real capture
//! prefix (slurp selection, grim grab, OCR) with stubbed commands and then
//! tears the dialog process down; it never asserts on the QML dialog, because
//! a headless e2e run cannot drive a real GUI. Flow 4 exercises the CLI
//! parse, route and forward of a library-created project; the editor window
//! that would open it is outside the headless surface.

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

    /// A marker file the stub commands append to, so one test can observe
    /// which real capture commands the binary ran, in order.
    fn marker(&self) -> PathBuf {
        self.root.join("commands.log")
    }

    /// The temp directory of stub `grim`/`slurp`/`hyprctl` executables.
    fn stub_bin(&self) -> PathBuf {
        self.root.join("stub-bin")
    }

    /// A native directory holding the mirror plugin, so a grab proceeds
    /// instead of failing the component check up front.
    fn native_dir(&self) -> PathBuf {
        self.root.join("native")
    }

    /// Install stubs for every command the interactive capture path reaches
    /// and return the directory to prepend to PATH.
    ///
    /// `hyprctl` reports the aarch64 degrade (the plugin cannot hook), so the
    /// clean mirror is unavailable and the grab still reaches grim. `slurp`
    /// returns a fixed region and `grim` returns placeholder PNG bytes; both
    /// append their argv to the marker. `tesseract` yields text and `zbarimg`
    /// declines, matching a screen with OCR text and no QR code.
    fn install_capture_stubs(&self) -> PathBuf {
        let bin = self.stub_bin();
        fs::create_dir_all(&bin).unwrap();
        let native = self.native_dir();
        fs::create_dir_all(&native).unwrap();
        fs::write(native.join("clean-mirror.so"), b"plugin").unwrap();

        write_stub(
            &bin,
            "slurp",
            "echo \"slurp $*\" >> \"$TIDYSHOT_E2E_MARKER\"\necho '10,20 100x200'",
        );
        write_stub(
            &bin,
            "grim",
            "echo \"grim $*\" >> \"$TIDYSHOT_E2E_MARKER\"\nprintf 'PNG'",
        );
        write_stub(
            &bin,
            "hyprctl",
            "if [ \"$1\" = \"-j\" ] && [ \"$2\" = \"plugin list\" ]; then\n  echo '[]'\nelif [ \"$1\" = \"plugin\" ] && [ \"$2\" = \"load\" ]; then\n  echo 'Could not hook IElementRenderer::preDrawSurface'\nelif [ \"$1\" = \"repl\" ]; then\n  echo 'false'\nelse\n  echo 'ok'\nfi",
        );
        write_stub(
            &bin,
            "tesseract",
            "echo \"tesseract $*\" >> \"$TIDYSHOT_E2E_MARKER\"\necho 'Recognized text'",
        );
        write_stub(
            &bin,
            "zbarimg",
            "echo \"zbarimg $*\" >> \"$TIDYSHOT_E2E_MARKER\"\nexit 4",
        );
        write_stub(
            &bin,
            "wl-copy",
            "cat >/dev/null\necho 'wl-copy' >> \"$TIDYSHOT_E2E_MARKER\"",
        );
        write_stub(
            &bin,
            "notify-send",
            "echo 'notify-send' >> \"$TIDYSHOT_E2E_MARKER\"",
        );
        bin
    }

    /// PATH with the stub directory in front of the inherited one.
    fn path_with_stubs(&self, bin: &Path) -> String {
        let inherited = std::env::var("PATH").unwrap_or_default();
        format!("{}:{inherited}", bin.display())
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

/// Write an executable stub script.
fn write_stub(dir: &Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt as _;
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
}

/// Poll until `path` contains `needle`, bounded. Returns the file text.
fn wait_for_marker(path: &Path, needle: &str, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(text) = fs::read_to_string(path)
            && text.contains(needle)
        {
            return Some(text);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(POLL);
    }
}

/// A background process a test starts and always reaps.
///
/// Stdin, stdout and stderr are null rather than piped: an offscreen GUI that
/// fills an undrained pipe would block before it reaches the code under test.
struct Background {
    child: Child,
}

impl Background {
    fn spawn(mut command: Command) -> Self {
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self { child }
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        kill_and_reap(&mut self.child);
    }
}

/// Kill and reap a child process, tolerating one that has already exited.
fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
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
    ///
    /// Stdio is null, not piped: an offscreen server that wrote more than a
    /// pipe buffer to an undrained pipe would block before it served, and a
    /// bounded poll cannot rescue a process stuck on write.
    fn launch(home: &Home) -> Self {
        Self::launch_with_stderr(home, None)
    }

    /// Start `tidyshot` with its stderr captured to `path`, so a server-side
    /// routing decision (an unknown command) is observable without a GUI.
    fn launch_with_stderr(home: &Home, stderr: Option<&Path>) -> Self {
        let mut command = home.command();
        command.stdin(Stdio::null()).stdout(Stdio::null());
        if let Some(path) = stderr {
            command.stderr(Stdio::from(fs::File::create(path).unwrap()));
        } else {
            command.stderr(Stdio::null());
        }
        let child = command.spawn().unwrap();
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
        kill_and_reap(&mut self.child);
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

/// Send one raw argument vector over the instance socket, bypassing the
/// sender's CLI validation, so the server-side router's decision is what the
/// test observes. Returns false when no instance is listening.
fn send_raw_command(home: &Home, argv: &[&str]) -> bool {
    use std::io::Write as _;
    use std::os::unix::net::UnixStream;
    let Some(socket) = home.socket() else {
        return false;
    };
    let Ok(mut stream) = UnixStream::connect(&socket) else {
        return false;
    };
    let argv: Vec<String> = argv.iter().map(|arg| (*arg).to_owned()).collect();
    stream.write_all(&tidyshot::instance::encode(&argv)).is_ok()
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

/// Flow 2: the interactive `ocr` capture runs slurp, grim and OCR through the
/// command runner before the result dialog.
///
/// The dialog itself is the GUI boundary: a headless run cannot dismiss it,
/// so the test stops after the reachable prefix and never claims the dialog
/// worked. The stubbed commands prove the binary reached the real capture
/// pipeline rather than merely validating the arguments.
#[test]
fn the_ocr_capture_reaches_stubbed_slurp_grim_and_ocr() {
    if missing_binary() {
        return;
    }
    let home = Home::new("capture");
    let bin = home.install_capture_stubs();
    let marker = home.marker();

    let process = Background::spawn({
        let mut command = home.command();
        command
            .env("PATH", home.path_with_stubs(&bin))
            .env("TIDYSHOT_E2E_MARKER", &marker)
            .env("TIDYSHOT_NATIVE_DIR", home.native_dir())
            .args(["ocr"]);
        command
    });

    // Poll for the recognition pass itself, not just the first `tesseract`
    // line: the OSD script probe (`-l osd --psm 0`) writes a tesseract marker
    // milliseconds before the `--psm 3` recognition pass, so matching on
    // "tesseract" would decide on whichever phase the first read landed in.
    let text = wait_for_marker(&marker, "--psm 3", Duration::from_secs(15))
        .expect("the capture prefix never reached the OCR recognition pass");
    drop(process);

    let slurp = text
        .lines()
        .position(|line| line.starts_with("slurp "))
        .unwrap_or_else(|| panic!("slurp did not run; marker was:\n{text}"));
    let grim = text
        .lines()
        .position(|line| line.starts_with("grim "))
        .unwrap_or_else(|| panic!("grim did not run; marker was:\n{text}"));
    assert!(slurp < grim, "slurp must select before grim grabs: {text}");

    // The grab used slurp's fixed region and grim's PNG output; tesseract
    // then ran its real recognition pass, distinct from the OSD script probe
    // that uses `-l osd --psm 0`. The QML dialog is where a real run
    // continues.
    assert!(
        text.contains("grim -g 10,20 100x200 -t png -"),
        "grim did not grab the selected region: {text}"
    );
    assert!(
        text.lines().any(|line| line.starts_with("tesseract ")
            && line.contains(" stdout ")
            && line.contains("--psm 3")),
        "tesseract ran no recognition pass over the captured frame: {text}"
    );
}

/// Flow 4: a `.omnishot` project written by the library is accepted by the
/// binary's headless surface.
///
/// The project is built with the same public API the app uses, so the fixture
/// is a genuine editable archive. The binary then parses the bare path, sees a
/// file, routes it to the editor command and forwards it to the running
/// instance; the sender is headless and its exit code is the observable. The
/// server's own stderr records the routing decision it made for the forwarded
/// payload, which pins that the instance, not just the sender, recognized the
/// project. The editor window that would open the project is outside this
/// surface.
#[test]
fn a_library_project_is_forwarded_by_the_headless_surface() {
    if missing_binary() {
        return;
    }
    let home = Home::new("project");
    let project = editable_project_path(&home);
    let project_arg = project.to_str().unwrap().to_owned();

    // A missing project path is not a file, so the CLI rejects it as an
    // unknown command before it ever reaches the GUI.
    let absent = home.root.join("absent.omnishot");
    let absent = run_bounded(&home, &[absent.to_str().unwrap()], EXIT_TIMEOUT);
    assert_eq!(
        absent.status.code(),
        Some(2),
        "a missing project must be rejected"
    );
    let stderr = String::from_utf8_lossy(&absent.stderr);
    assert!(stderr.contains("unknown command"), "stderr was: {stderr}");

    // A real archive is recognized as a local file and delivered to the
    // running instance; the sender exits 0 and the instance survives. The
    // server's stderr log is captured so its side of the routing is visible.
    let server_stderr = home.root.join("server.stderr");
    let mut server = Server::launch_with_stderr(&home, Some(&server_stderr));
    wait_for_listening(&home, START_TIMEOUT)
        .expect("the offscreen instance never bound its socket");
    let forwarded = run_bounded(&home, &[&project_arg], EXIT_TIMEOUT);
    assert_eq!(
        forwarded.status.code(),
        Some(0),
        "a library project must be forwarded, stderr: {}",
        String::from_utf8_lossy(&forwarded.stderr)
    );
    assert!(
        server.running(),
        "the instance stopped on the project command"
    );

    // The forwarded project must be routed to the editor, not merely
    // accepted. A project command is well understood, so the server stays
    // silent for it while logging no error that names the project. A raw
    // unknown payload makes the same server-side router log a rejection,
    // proving the instance runs the router over socket commands; the silent
    // project therefore means it routed cleanly.
    let written = send_raw_command(&home, &["frobnicate"]);
    assert!(
        written,
        "the raw payload could not be written to the instance socket"
    );
    let logged = wait_for_marker(&server_stderr, "unknown command", EXIT_TIMEOUT)
        .expect("the server never logged the raw unknown command");
    assert!(
        logged.contains("frobnicate"),
        "the raw rejection did not name the command: {logged}"
    );
    assert!(
        !logged.contains(&project_arg),
        "the server rejected the forwarded project: {logged}"
    );
    assert!(server.running(), "the instance stopped on the raw command");
    server.terminate();
}

/// Create a version-2 `.omnishot` archive with one annotation.
fn editable_project_path(home: &Home) -> PathBuf {
    use tidyshot::annotation::{Annotation, Props};
    use tidyshot::editor::Document;
    use tidyshot::json::Value;
    use tidyshot::pngzip::Image;

    let mut data = vec![0u8; 64 * 48 * 4];
    for (index, pixel) in data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        pixel.copy_from_slice(&[(index % 255) as u8, 200, 120, 255]);
    }
    let mut document = Document::new(Image::new(64, 48, data), None, false);
    let props: Props = [
        ("kind".to_owned(), Value::String("rect".to_owned())),
        ("x".to_owned(), Value::Number(4.0)),
        ("y".to_owned(), Value::Number(4.0)),
        ("w".to_owned(), Value::Number(20.0)),
        ("h".to_owned(), Value::Number(12.0)),
        ("color".to_owned(), Value::String("#ffd60a".to_owned())),
        ("width".to_owned(), Value::Number(3.0)),
    ]
    .into_iter()
    .collect();
    document.objects.push(Annotation::new(props));
    document.commit();

    let path = home.root.join("editable.omnishot");
    document.save(&path).unwrap();
    // The fixture must round-trip through the project codec, not just exist.
    let reopened = tidyshot::project::Project::open(&path).unwrap();
    assert_eq!(reopened.version, 2);
    assert_eq!(reopened.objects.len(), 1);
    path
}
