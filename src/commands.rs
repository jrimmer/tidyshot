//! External command execution behind a trait so tests can fake it.
// Ported from omnishot/backend.py (run)
//!
//! Every external program (`grim`, `slurp`, `wl-copy`, `ffmpeg`, `ffprobe`,
//! `gpu-screen-recorder`, `hyprctl`) goes through [`CommandRunner`]. Production
//! code uses [`SystemRunner`]; tests substitute a recorder so behavior is
//! asserted without spawning processes.

use std::collections::BTreeMap;
use std::fs::File;
use std::io;
#[cfg(unix)]
use std::os::fd::RawFd;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A captured command result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl CommandOutput {
    #[must_use]
    pub fn success(&self) -> bool {
        self.status == 0
    }

    #[must_use]
    pub fn stdout_string(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    #[must_use]
    pub fn stderr_string(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// The error a failing command reports, carrying stderr for the UI.
#[derive(Debug)]
pub enum CommandError {
    /// The program could not be started at all.
    Spawn { program: String, source: io::Error },
    /// The program ran but exited non-zero.
    Failure {
        program: String,
        output: CommandOutput,
    },
    /// The program exceeded its timeout.
    Timeout { program: String, timeout: Duration },
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::Spawn { program, source } => {
                write!(f, "could not start {program}: {source}")
            }
            CommandError::Failure { program, output } => {
                let stderr = output.stderr_string();
                let stdout = output.stdout_string();
                let detail = if stderr.trim().is_empty() {
                    stdout
                } else {
                    stderr
                };
                let detail = detail.trim();
                if detail.is_empty() {
                    write!(f, "{program} failed")
                } else {
                    write!(f, "{detail}")
                }
            }
            CommandError::Timeout { program, timeout } => {
                write!(f, "{program} timed out after {timeout:?}")
            }
        }
    }
}

impl std::error::Error for CommandError {}

/// A single command invocation.
#[derive(Debug, Clone, Default)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<Vec<u8>>,
    pub timeout: Option<Duration>,
    pub env: BTreeMap<String, String>,
    /// Capture stderr through a temporary file instead of a pipe, and send
    /// stdout to `/dev/null`.
    ///
    /// Some programs fork a long-lived owner that inherits our pipes
    /// (`wl-copy`); reading them to EOF blocks until that owner exits. A file
    /// never makes the caller wait for the owner.
    pub stderr_to_file: bool,
    /// Descriptors the spawned process must inherit across `execve`.
    ///
    /// A recorder inherits its journal lease so recovery can tell a live take
    /// from an abandoned one after the app crashes.
    pub inherit_fds: Vec<i32>,
}

impl CommandSpec {
    #[must_use]
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    #[must_use]
    pub fn stdin(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(data.into());
        self
    }

    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Capture stderr through a temporary file rather than a pipe.
    #[must_use]
    pub fn stderr_to_file(mut self, on: bool) -> Self {
        self.stderr_to_file = on;
        self
    }

    /// Make `fd` inheritable by the spawned process.
    #[must_use]
    pub fn inherit_fd(mut self, fd: i32) -> Self {
        self.inherit_fds.push(fd);
        self
    }
}

/// A long-lived child process the runtime can pause, resume and stop.
pub trait RunningCommand: Send {
    /// The child's process id.
    fn pid(&self) -> u32;

    /// Suspend the child's process group.
    fn pause(&mut self) -> Result<(), CommandError>;

    /// Resume a suspended child.
    fn resume(&mut self) -> Result<(), CommandError>;

    /// Wait for the child to exit and report its status.
    fn wait(&mut self) -> Result<CommandOutput, CommandError>;

    /// True while the child is still running.
    fn running(&mut self) -> Result<bool, CommandError>;

    /// Stop the child, escalating from SIGINT to SIGKILL without hanging.
    fn stop(&mut self) -> Result<CommandOutput, CommandError>;
}

/// A `wl-copy` invocation that does not wait on the clipboard owner.
///
/// The owner inherits our stdout/stderr, so both are kept off pipes: stdout is
/// discarded and stderr goes to a temporary file, matching the source's
/// clipboard write. The timeout is the source's ten seconds.
// Ported from omnishot/backend.py (clipboard_write)
pub fn clipboard_spec(mime: &str, data: Vec<u8>) -> CommandSpec {
    CommandSpec::new("wl-copy")
        .args(["--type", mime])
        .stdin(data)
        .stderr_to_file(true)
        .timeout(Duration::from_secs(10))
}

/// Fire a desktop notification and do not wait for it.
///
/// The recognized-text path reports a copied result and an empty read this
/// way, matching the source's `notify`; a missing `notify-send` is silent
/// because the dialog already carries the status.
// Ported from omnishot/app.py (notify)
pub fn notify_send(title: &str, message: &str) {
    let _ = Command::new("notify-send")
        .args(["--app-name=tidyshot", title, message])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// Run a command and return its output, or a [`CommandError`] on failure.
///
/// Implementations must never swallow a non-zero exit: the error text is what
/// the UI surfaces.
pub trait CommandRunner: Send + Sync {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError>;

    /// Run a command, streaming `input` to its stdin instead of buffering the
    /// whole payload into the command spec. The export path uses this so a
    /// long recording is never held in memory all at once.
    fn run_streaming(
        &self,
        spec: &CommandSpec,
        input: &mut (dyn std::io::Read + Send),
    ) -> Result<CommandOutput, CommandError> {
        let mut data = Vec::new();
        input
            .read_to_end(&mut data)
            .map_err(|source| CommandError::Spawn {
                program: spec.program.clone(),
                source,
            })?;
        self.run(&spec.clone().stdin(data))
    }

    /// Convenience wrapper that requires success and returns stdout.
    fn run_stdout(&self, spec: &CommandSpec) -> Result<Vec<u8>, CommandError> {
        Ok(self.run(spec)?.stdout)
    }

    /// Start a long-lived process for a recorder or another interactive tool.
    ///
    /// The default refuses, so a test runner only implements the paths it
    /// actually exercises and a recorder never starts by accident.
    fn spawn(&self, spec: &CommandSpec) -> Result<Box<dyn RunningCommand>, CommandError> {
        Err(CommandError::Spawn {
            program: spec.program.clone(),
            source: io::Error::new(
                io::ErrorKind::Unsupported,
                "this command runner cannot start a long-lived process",
            ),
        })
    }
}

/// Production runner backed by [`std::process::Command`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
        let mut command = Command::new(&spec.program);
        command.args(&spec.args);
        command.stdin(if spec.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut stderr_file = None;
        if spec.stderr_to_file {
            command.stdout(Stdio::null());
            let file = temp_file()?;
            stderr_file = Some(file.try_clone().map_err(|source| CommandError::Spawn {
                program: spec.program.clone(),
                source,
            })?);
            command.stderr(Stdio::from(file));
        } else {
            command.stdout(Stdio::piped());
            command.stderr(Stdio::piped());
        }
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        // Give every child its own process group so a timeout can reap the
        // encoder it spawned as well as the child itself.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }

        let mut child = command.spawn().map_err(|source| CommandError::Spawn {
            program: spec.program.clone(),
            source,
        })?;
        if let Some(data) = &spec.stdin {
            use std::io::Write as _;
            let mut stdin = child.stdin.take().ok_or_else(|| CommandError::Spawn {
                program: spec.program.clone(),
                source: io::Error::other("stdin pipe unavailable"),
            })?;
            if let Err(source) = stdin.write_all(data) {
                // A broken pipe means the child exited early; reap it so it
                // does not linger as a zombie.
                drop(stdin);
                let _ = child.kill();
                let _ = child.wait();
                return Err(CommandError::Spawn {
                    program: spec.program.clone(),
                    source,
                });
            }
        }

        // Python's backend.run defaults to a 30 second timeout.
        let timeout = spec.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let output = wait_with_timeout(child, timeout, &spec.program)?;
        let status = output.status.code().unwrap_or(-1);
        let mut captured = CommandOutput {
            status,
            stdout: output.stdout,
            stderr: output.stderr,
        };
        if let Some(mut file) = stderr_file {
            use std::io::{Read as _, Seek as _, SeekFrom};
            let _ = file.seek(SeekFrom::Start(0));
            let _ = file.read_to_end(&mut captured.stderr);
        }
        if captured.success() {
            Ok(captured)
        } else {
            Err(CommandError::Failure {
                program: spec.program.clone(),
                output: captured,
            })
        }
    }

    fn spawn(&self, spec: &CommandSpec) -> Result<Box<dyn RunningCommand>, CommandError> {
        let mut command = Command::new(&spec.program);
        command.args(&spec.args);
        command.stdin(Stdio::null());
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }
        #[cfg(unix)]
        let _restore =
            make_inheritable(&spec.inherit_fds).map_err(|source| CommandError::Spawn {
                program: spec.program.clone(),
                source,
            })?;
        let child = command.spawn().map_err(|source| CommandError::Spawn {
            program: spec.program.clone(),
            source,
        })?;
        Ok(Box::new(SystemRunningCommand {
            child,
            program: spec.program.clone(),
        }))
    }

    fn run_streaming(
        &self,
        spec: &CommandSpec,
        input: &mut (dyn std::io::Read + Send),
    ) -> Result<CommandOutput, CommandError> {
        let mut command = Command::new(&spec.program);
        command.args(&spec.args);
        command.stdin(Stdio::piped());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        for (key, value) in &spec.env {
            command.env(key, value);
        }
        // Give every child its own process group so a timeout can reap the
        // encoder it spawned as well as the child itself.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }

        let mut child = command.spawn().map_err(|source| CommandError::Spawn {
            program: spec.program.clone(),
            source,
        })?;
        let mut stdin = child.stdin.take().ok_or_else(|| CommandError::Spawn {
            program: spec.program.clone(),
            source: io::Error::other("stdin pipe unavailable"),
        })?;
        let timeout = spec.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let program = spec.program.clone();
        std::thread::scope(|scope| {
            // A writer thread drains the frames into the encoder while the
            // main thread reads its stdout/stderr, so neither side deadlocks.
            let writer = scope.spawn(move || {
                let result = std::io::copy(input, &mut stdin);
                drop(stdin);
                result
            });
            let output = wait_with_timeout(child, timeout, &program);
            let write = writer.join();
            match output {
                // A failing encoder reports its own stderr; only surface a
                // write error when the encoder itself succeeded.
                Err(error) => Err(error),
                Ok(output) => {
                    let write = write.map_err(|_| CommandError::Spawn {
                        program: program.clone(),
                        source: io::Error::other("stdin writer panicked"),
                    })?;
                    write.map_err(|source| CommandError::Spawn {
                        program: program.clone(),
                        source,
                    })?;
                    let status = output.status.code().unwrap_or(-1);
                    let captured = CommandOutput {
                        status,
                        stdout: output.stdout,
                        stderr: output.stderr,
                    };
                    if captured.success() {
                        Ok(captured)
                    } else {
                        Err(CommandError::Failure {
                            program: program.clone(),
                            output: captured,
                        })
                    }
                }
            }
        })
    }
}

/// A child of [`SystemRunner::spawn`].
struct SystemRunningCommand {
    child: Child,
    program: String,
}

impl RunningCommand for SystemRunningCommand {
    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn pause(&mut self) -> Result<(), CommandError> {
        self.signal(libc::SIGSTOP)
    }

    fn resume(&mut self) -> Result<(), CommandError> {
        self.signal(libc::SIGCONT)
    }

    fn running(&mut self) -> Result<bool, CommandError> {
        self.exited().map(|exited| !exited)
    }

    fn wait(&mut self) -> Result<CommandOutput, CommandError> {
        let status = self.child.wait().map_err(|source| CommandError::Spawn {
            program: self.program.clone(),
            source,
        })?;
        Ok(CommandOutput {
            status: status.code().unwrap_or(-1),
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }

    fn stop(&mut self) -> Result<CommandOutput, CommandError> {
        for (signal, wait) in [
            (libc::SIGINT, Duration::from_secs(3)),
            (libc::SIGTERM, Duration::from_secs(2)),
            (libc::SIGKILL, Duration::from_secs(2)),
        ] {
            if self.exited()? {
                break;
            }
            // A paused process ignores SIGINT until it is continued.
            if signal != libc::SIGCONT {
                let _ = self.signal(libc::SIGCONT);
            }
            self.signal(signal)?;
            let exited =
                wait_for_exit(&mut self.child, wait).map_err(|source| CommandError::Spawn {
                    program: self.program.clone(),
                    source,
                })?;
            if exited {
                break;
            }
        }
        self.wait()
    }
}

impl SystemRunningCommand {
    fn exited(&mut self) -> Result<bool, CommandError> {
        self.child
            .try_wait()
            .map(|status| status.is_some())
            .map_err(|source| CommandError::Spawn {
                program: self.program.clone(),
                source,
            })
    }

    fn signal(&self, signal: i32) -> Result<(), CommandError> {
        let group = -(self.child.id() as i32);
        // SAFETY: `kill` only sends a signal to this child's process group.
        if unsafe { libc::kill(group, signal) } < 0 {
            let error = io::Error::last_os_error();
            // ESRCH means the child is already gone; the caller's wait then
            // reports the real exit status.
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(CommandError::Spawn {
                    program: self.program.clone(),
                    source: error,
                });
            }
        }
        Ok(())
    }
}

/// Wait up to `limit` for `child` to exit, polling so a stop never hangs.
fn wait_for_exit(child: &mut Child, limit: Duration) -> io::Result<bool> {
    let deadline = Instant::now() + limit;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn fd_flags(fd: RawFd) -> io::Result<i32> {
    // SAFETY: `fcntl` with `F_GETFD` only reads this descriptor's flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(flags)
}

#[cfg(unix)]
fn set_fd_flags(fd: RawFd, flags: i32) -> io::Result<()> {
    // SAFETY: `fcntl` with `F_SETFD` only changes this descriptor's flags.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Clear `FD_CLOEXEC` for the spawn and put it back afterwards.
#[cfg(unix)]
struct RestoreCloexec(Vec<(RawFd, i32)>);

#[cfg(unix)]
impl Drop for RestoreCloexec {
    fn drop(&mut self) {
        for (fd, flags) in &self.0 {
            let _ = set_fd_flags(*fd, *flags);
        }
    }
}

#[cfg(unix)]
fn make_inheritable(fds: &[i32]) -> io::Result<RestoreCloexec> {
    let mut saved = Vec::with_capacity(fds.len());
    for fd in fds {
        saved.push((*fd, fd_flags(*fd)?));
    }
    let restore = RestoreCloexec(saved);
    for fd in fds {
        set_fd_flags(*fd, fd_flags(*fd)? & !libc::FD_CLOEXEC)?;
    }
    Ok(restore)
}

/// Wait for `child` in a helper thread; kill it if `timeout` elapses.
fn wait_with_timeout(
    child: std::process::Child,
    timeout: Duration,
    program: &str,
) -> Result<Output, CommandError> {
    let pid = child.id();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(child.wait_with_output());
    });
    match receiver.recv_timeout(timeout) {
        Ok(result) => result.map_err(|source| CommandError::Spawn {
            program: program.to_owned(),
            source,
        }),
        Err(_) => {
            // SIGKILL the whole process group, so children (encoders) die too.
            unsafe_kill(pid);
            Err(CommandError::Timeout {
                program: program.to_owned(),
                timeout,
            })
        }
    }
}

/// The timeout applied when a caller does not request one, matching Python's
/// `backend.run` default.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// An unlinked temporary file for capturing a child's stderr.
///
/// The name is removed as soon as the file is open, so nothing is left behind
/// even if the process dies; the open descriptor keeps the file alive.
#[cfg(unix)]
fn temp_file() -> Result<File, CommandError> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        ".tidyshot-command-{}-{nonce:x}",
        std::process::id()
    ));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|source| CommandError::Spawn {
            program: "temporary stderr file".to_owned(),
            source,
        })?;
    let _ = std::fs::remove_file(&path);
    Ok(file)
}

#[cfg(not(unix))]
fn temp_file() -> Result<File, CommandError> {
    let path = std::env::temp_dir().join(format!(".tidyshot-command-{}", std::process::id()));
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .map_err(|source| CommandError::Spawn {
            program: "temporary stderr file".to_owned(),
            source,
        })
}

#[cfg(unix)]
fn unsafe_kill(pid: u32) {
    // The child leads its own process group (see `process_group(0)` above), so
    // a negative pid signals the group and takes any encoder with it.
    let _ = Command::new("kill")
        .args(["-9", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(unix))]
fn unsafe_kill(_pid: u32) {}

/// Parse `hyprctl -j <query>` output into a JSON value.
///
/// Kept beside the runner because every caller reaches Hyprland the same way.
pub fn hypr<R: CommandRunner>(runner: &R, query: &str) -> Result<super::json::Value, CommandError> {
    let spec = CommandSpec::new("hyprctl").args(["-j", query]);
    let output = runner.run(&spec)?;
    let text = output.stdout_string();
    super::json::parse(&text).map_err(|_| CommandError::Failure {
        program: spec.program.clone(),
        output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A runner that returns queued results and records the specs it saw.
    #[derive(Default)]
    struct FakeRunner {
        results: Mutex<Vec<Result<CommandOutput, CommandError>>>,
        seen: Mutex<Vec<CommandSpec>>,
    }

    impl FakeRunner {
        fn push(&self, output: CommandOutput) {
            self.results.lock().unwrap().push(Ok(output));
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            self.seen.lock().unwrap().push(spec.clone());
            let output = self.results.lock().unwrap().remove(0)?;
            if output.success() {
                Ok(output)
            } else {
                Err(CommandError::Failure {
                    program: spec.program.clone(),
                    output,
                })
            }
        }
    }

    fn output(status: i32, stdout: &str, stderr: &str) -> CommandOutput {
        CommandOutput {
            status,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn successful_command_returns_stdout() {
        let runner = FakeRunner::default();
        runner.push(output(0, "hello", ""));
        let spec = CommandSpec::new("grim");
        let result = runner.run_stdout(&spec).unwrap();
        assert_eq!(result, b"hello");
    }

    #[test]
    fn failure_surfaces_stderr() {
        let runner = FakeRunner::default();
        runner.push(output(1, "", "no such output"));
        let spec = CommandSpec::new("slurp");
        let error = runner.run(&spec).unwrap_err();
        assert_eq!(error.to_string(), "no such output");
    }

    #[test]
    fn spec_records_arguments_in_order() {
        let runner = FakeRunner::default();
        runner.push(output(0, "", ""));
        let spec = CommandSpec::new("ffprobe")
            .arg("-v")
            .arg("error")
            .args(["-show_entries", "stream=width,height"]);
        runner.run(&spec).unwrap();
        let seen = runner.seen.lock().unwrap();
        assert_eq!(seen[0].program, "ffprobe");
        assert_eq!(
            seen[0].args,
            ["-v", "error", "-show_entries", "stream=width,height"]
        );
    }

    // A `wl-copy`-style program forks an owner that inherits our pipes and
    // outlives the parent; piping stderr would block the caller until the
    // owner exits. The temp-file path must return as soon as the parent exits.
    #[cfg(unix)]
    #[test]
    fn stderr_file_does_not_wait_for_a_pipe_holding_child() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = std::env::temp_dir().join(format!("tidyshot-holder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("holder");
        std::fs::write(&script, "#!/bin/sh\n( sleep 5 ) &\nexit 0\n").unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).unwrap();
        let program = script.to_string_lossy().into_owned();

        let piped = CommandSpec::new(&program).timeout(Duration::from_secs(1));
        assert!(matches!(
            SystemRunner.run(&piped),
            Err(CommandError::Timeout { .. })
        ));

        let spec = CommandSpec::new(&program)
            .stderr_to_file(true)
            .timeout(Duration::from_secs(2));
        let started = std::time::Instant::now();
        SystemRunner.run(&spec).unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
