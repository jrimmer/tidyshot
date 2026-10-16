//! Bounded shutdown for encoders the user chose to discard.
// Ported from omnishot/recording_process.py
//!
//! An encoder is bound to the app's lifetime with [`RecordGuard`]: a pidfd for
//! the app, the `recording-guard.so` loader, and the take's journal lease are
//! all inherited by the child. If the app dies while paused or crashes, the
//! guard sees the pidfd close and stops the encoder; while the encoder lives it
//! keeps the journal flock, so recovery never mistakes a live take for an
//! abandoned one.
//!
//! Cancelling a take must stop its encoder without hanging on a process that
//! ignores signals. The shutdown escalates: SIGINT, then SIGTERM, then
//! SIGKILL, each with a bounded wait. A discarded take never leaves a live
//! encoder behind, and the last-resort kill is not reported as a failure in
//! its own right.

use std::collections::BTreeMap;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The default grace the encoder gets after SIGINT, in seconds.
pub const DEFAULT_GRACE: Duration = Duration::from_secs(3);
/// The default grace after SIGTERM, in seconds.
pub const DEFAULT_TERMINATE: Duration = Duration::from_secs(2);
/// The final grace after SIGKILL, in seconds.
pub const DEFAULT_KILL: Duration = Duration::from_secs(2);

/// The preload library that watches the app and stops the encoder with it.
pub const GUARD_LIBRARY: &str = "recording-guard.so";

/// The descriptors and environment that bind an encoder to the app.
///
/// Hold one alongside a running [`Encoder`]: the descriptors must stay open in
/// the parent so the child can inherit them, and dropping the guard closes the
/// pidfd, which is exactly the signal `recording-guard.so` waits on. The lease
/// is borrowed from [`super::journal::Journal`]; it stays open as long as the
/// journal does.
#[derive(Debug)]
pub struct RecordGuard<'a> {
    owner: OwnedFd,
    library: OwnedFd,
    lease: Option<BorrowedFd<'a>>,
    library_path: PathBuf,
}

impl<'a> RecordGuard<'a> {
    /// Open the guard against the installed native directory.
    ///
    /// `lease` is the journal lock the encoder must inherit. Fails with
    /// `NotFound` when `recording-guard.so` is missing, so a take never starts
    /// without its lifetime guard.
    pub fn open(lease: Option<BorrowedFd<'a>>) -> io::Result<Self> {
        Self::open_in(&super::native_dir(), lease)
    }

    /// Like [`RecordGuard::open`], against an explicit directory.
    pub fn open_in(native_dir: &Path, lease: Option<BorrowedFd<'a>>) -> io::Result<Self> {
        let library_path = native_dir.join(GUARD_LIBRARY);
        let library = std::fs::OpenOptions::new()
            .read(true)
            .open(&library_path)
            .map_err(|err| {
                if err.kind() == io::ErrorKind::NotFound {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        "OmniShot's recording components need rebuilding. Run install.sh from the OmniShot folder.",
                    )
                } else {
                    err
                }
            })?;
        let owner = pidfd_open_self()?;
        Ok(Self {
            owner,
            library: OwnedFd::from(library),
            lease,
            library_path,
        })
    }

    /// The pidfd for this process, passed as `OMNISHOT_RECORD_OWNER_FD`.
    #[must_use]
    pub fn owner_fd(&self) -> i32 {
        self.owner.as_raw_fd()
    }

    /// The opened guard library, passed as `OMNISHOT_RECORD_LIBRARY_FD`.
    #[must_use]
    pub fn library_fd(&self) -> i32 {
        self.library.as_raw_fd()
    }

    /// The journal lease the child holds, passed as `OMNISHOT_RECORD_LEASE_FD`.
    #[must_use]
    pub fn lease_fd(&self) -> Option<i32> {
        self.lease.map(|fd| fd.as_raw_fd())
    }

    /// The path the guard library was opened from.
    #[must_use]
    pub fn library_path(&self) -> &Path {
        &self.library_path
    }
}

/// `pidfd_open(getpid(), 0)`; the kernel gives back a pollable handle that
/// closes when this process exits, even while the encoder is paused.
fn pidfd_open_self() -> io::Result<OwnedFd> {
    // SAFETY: `pidfd_open` takes the calling pid and no flags here.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, std::process::id() as i32, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a non-negative result is a fresh descriptor owned by this call.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

/// Read a descriptor's `FD_*` flags.
fn fd_flags(fd: RawFd) -> io::Result<i32> {
    // SAFETY: `fcntl` with `F_GETFD` only reads this descriptor's flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(flags)
}

/// Set a descriptor's `FD_*` flags.
fn set_fd_flags(fd: RawFd, flags: i32) -> io::Result<()> {
    // SAFETY: `fcntl` with `F_SETFD` only changes this descriptor's flags.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Restores `FD_CLOEXEC` on borrowed descriptors once the child is spawned.
///
/// The descriptors must be inheritable across `execve`, but only for that one
/// spawn; leaving them open in later children would leak the lease and pidfd.
struct RestoreCloexec(Vec<(RawFd, i32)>);

impl Drop for RestoreCloexec {
    fn drop(&mut self) {
        for (fd, flags) in &self.0 {
            let _ = set_fd_flags(*fd, *flags);
        }
    }
}

/// Clear `FD_CLOEXEC` so the descriptors survive the child's `execve`.
fn make_inheritable(fds: &[RawFd]) -> io::Result<RestoreCloexec> {
    let mut saved = Vec::with_capacity(fds.len());
    for fd in fds {
        saved.push((*fd, fd_flags(*fd)?));
    }
    // Take the restore guard before clearing, so a partial failure still puts
    // back whatever was already made inheritable.
    let restore = RestoreCloexec(saved);
    for fd in fds {
        set_fd_flags(*fd, fd_flags(*fd)? & !libc::FD_CLOEXEC)?;
    }
    Ok(restore)
}

/// A launched encoder that can be stopped without a hang.
#[derive(Debug)]
pub struct Encoder {
    child: Child,
    stdin: Option<std::process::ChildStdin>,
}

impl Encoder {
    /// Launch `command` in its own process group, bound to `guard`.
    ///
    /// The guard's pidfd, library, and journal lease are made inheritable and
    /// their numbers and the library preload are written into the child's
    /// environment, exactly as the source does before `Popen(..., pass_fds=)`.
    /// A dedicated process group lets a stopped encoder be signalled as a
    /// whole, so any child encoder dies with it.
    pub fn launch(
        command: &[String],
        env: &BTreeMap<String, String>,
        guard: &RecordGuard<'_>,
    ) -> io::Result<Self> {
        let Some((program, args)) = command.split_first() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the encoder command is empty",
            ));
        };
        let mut descriptors = vec![guard.owner_fd(), guard.library_fd()];
        if let Some(lease) = guard.lease_fd() {
            descriptors.push(lease);
        }
        let _restore = make_inheritable(&descriptors)?;
        let mut child = Command::new(program);
        child.args(args);
        child.stdin(Stdio::piped());
        child.stdout(Stdio::piped());
        child.stderr(Stdio::piped());
        for (key, value) in env {
            child.env(key, value);
        }
        let preload = format!("/proc/self/fd/{}", guard.library_fd());
        match env.get("LD_PRELOAD") {
            Some(existing) if !existing.is_empty() => {
                child.env("LD_PRELOAD", format!("{preload}:{existing}"));
            }
            _ => {
                child.env("LD_PRELOAD", &preload);
            }
        }
        child.env("OMNISHOT_RECORD_OWNER_FD", guard.owner_fd().to_string());
        child.env("OMNISHOT_RECORD_LIBRARY_FD", guard.library_fd().to_string());
        // Never leak a parent's lease number into a take that has none.
        child.env_remove("OMNISHOT_RECORD_LEASE_FD");
        if let Some(lease) = guard.lease_fd() {
            child.env("OMNISHOT_RECORD_LEASE_FD", lease.to_string());
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            child.process_group(0);
        }
        let mut child = child.spawn()?;
        let stdin = child.stdin.take();
        Ok(Self { child, stdin })
    }

    /// The encoder's process id, for diagnostics.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The child's stdin, when the caller wants to write to the encoder.
    #[must_use]
    pub fn stdin(&self) -> Option<&std::process::ChildStdin> {
        self.stdin.as_ref()
    }

    /// True while the encoder is still running.
    pub fn running(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }

    /// The encoder's exit code once it has exited.
    pub fn exit_code(&mut self) -> Option<i32> {
        self.child
            .try_wait()
            .ok()
            .flatten()
            .map(|status| status.code().unwrap_or(-1))
    }

    /// Stop a discarded encoder without leaving it running.
    ///
    /// The escalation matches the source: SIGINT (a chance to finalize the
    /// file), SIGTERM, then SIGKILL. A process already gone is a no-op; a
    /// process that has not exited after the final kill is reported as still
    /// running, never silently dropped.
    pub fn stop_discarded(&mut self) -> io::Result<()> {
        self.stop_with(DEFAULT_GRACE, DEFAULT_TERMINATE, DEFAULT_KILL)
    }

    /// Like [`Encoder::stop_discarded`], with explicit per-stage timeouts.
    pub fn stop_with(
        &mut self,
        grace: Duration,
        terminate_timeout: Duration,
        kill_timeout: Duration,
    ) -> io::Result<()> {
        for (signal, wait) in [
            (libc::SIGINT, grace),
            (libc::SIGTERM, terminate_timeout),
            (libc::SIGKILL, kill_timeout),
        ] {
            if self.has_exited()? {
                return Ok(());
            }
            self.signal(signal)?;
            if self.wait_until_exit(wait)? {
                return Ok(());
            }
        }
        if self.has_exited()? {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "The cancelled recorder did not exit. Try cancelling again.",
            ))
        }
    }

    fn has_exited(&mut self) -> io::Result<bool> {
        Ok(self.child.try_wait()?.is_some())
    }

    /// Wait up to `limit` for the child to exit, polling to avoid a hang.
    ///
    /// A wait error is propagated rather than reported as an exit: the caller
    /// must not return success while the encoder may still be running.
    fn wait_until_exit(&mut self, limit: Duration) -> io::Result<bool> {
        if limit.is_zero() {
            return self.has_exited();
        }
        let deadline = Instant::now() + limit;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return Ok(true),
                Ok(None) => {}
                Err(err) => return Err(err),
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Signal the child's process group. ESRCH (already gone) is success.
    fn signal(&mut self, signal: i32) -> io::Result<()> {
        let group = -(self.child.id() as i32);
        // SAFETY: `kill` only sends a signal to the child's process group.
        if unsafe { libc::kill(group, signal) } < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(err);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::fd::AsFd;

    fn native_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tidyshot-encoder-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(GUARD_LIBRARY),
            b"unit-test placeholder; never loaded",
        )
        .unwrap();
        dir
    }

    fn guard<'a>(name: &str, lease: &'a std::fs::File) -> (RecordGuard<'a>, PathBuf) {
        let dir = native_dir(name);
        let guard = RecordGuard::open_in(&dir, Some(lease.as_fd())).unwrap();
        (guard, dir)
    }

    fn encoder(body: &str, guard: &RecordGuard<'_>) -> Encoder {
        let command = vec!["sh".to_owned(), "-c".to_owned(), body.to_owned()];
        Encoder::launch(&command, &BTreeMap::new(), guard).unwrap()
    }

    fn lease_file(name: &str) -> std::fs::File {
        let path = std::env::temp_dir().join(format!(
            "tidyshot-encoder-lease-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .unwrap()
    }

    #[test]
    fn missing_guard_library_refuses_to_launch() {
        let dir =
            std::env::temp_dir().join(format!("tidyshot-encoder-missing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let lease = lease_file("missing");
        let err = RecordGuard::open_in(&dir, Some(lease.as_fd())).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(
            err.to_string()
                .contains("recording components need rebuilding")
        );
    }

    #[test]
    fn encoder_inherits_the_guard_descriptors_and_lease() {
        let lease = lease_file("inherit");
        let (guard, _dir) = guard("inherit", &lease);
        let out = std::env::temp_dir().join(format!("tidyshot-encoder-fds-{}", std::process::id()));
        let _ = fs::remove_file(&out);
        let body = format!(
            "echo owner=$(readlink /proc/self/fd/$OMNISHOT_RECORD_OWNER_FD)> {out}; \
             echo lib=$(readlink /proc/self/fd/$OMNISHOT_RECORD_LIBRARY_FD)>> {out}; \
             echo lease=$(readlink /proc/self/fd/$OMNISHOT_RECORD_LEASE_FD)>> {out}; \
             echo preload=$LD_PRELOAD>> {out}",
            out = out.display()
        );
        let mut child = encoder(&body, &guard);
        assert!(child.wait_until_exit(Duration::from_secs(5)).unwrap());
        let text = fs::read_to_string(&out).unwrap();
        assert!(text.contains("owner=anon_inode:[pidfd]"), "{text}");
        assert!(
            text.contains(&format!("lib={}", guard.library_path().display())),
            "{text}"
        );
        assert!(text.contains("lease=/"), "{text}");
        assert!(
            text.contains(&format!("preload=/proc/self/fd/{}", guard.library_fd())),
            "{text}"
        );
    }

    #[test]
    fn preload_appends_to_an_existing_ld_preload() {
        let lease = lease_file("preload");
        let (guard, _dir) = guard("preload", &lease);
        let out =
            std::env::temp_dir().join(format!("tidyshot-encoder-preload-{}", std::process::id()));
        let _ = fs::remove_file(&out);
        let body = format!("echo $LD_PRELOAD > {}", out.display());
        let mut env = BTreeMap::new();
        env.insert("LD_PRELOAD".to_owned(), "/opt/existing.so".to_owned());
        let command = vec!["sh".to_owned(), "-c".to_owned(), body];
        let mut child = Encoder::launch(&command, &env, &guard).unwrap();
        assert!(child.wait_until_exit(Duration::from_secs(5)).unwrap());
        let text = fs::read_to_string(&out).unwrap();
        assert_eq!(
            text.trim(),
            format!("/proc/self/fd/{}:/opt/existing.so", guard.library_fd())
        );
    }

    #[test]
    fn a_take_without_a_lease_clears_a_stale_lease_variable() {
        let dir = native_dir("no-lease");
        let guard = RecordGuard::open_in(&dir, None).unwrap();
        assert_eq!(guard.lease_fd(), None);
        let out =
            std::env::temp_dir().join(format!("tidyshot-encoder-no-lease-{}", std::process::id()));
        let _ = fs::remove_file(&out);
        let body = format!(
            "echo lease=${{OMNISHOT_RECORD_LEASE_FD-unset}} > {}",
            out.display()
        );
        let mut env = BTreeMap::new();
        env.insert("OMNISHOT_RECORD_LEASE_FD".to_owned(), "7".to_owned());
        let command = vec!["sh".to_owned(), "-c".to_owned(), body];
        let mut child = Encoder::launch(&command, &env, &guard).unwrap();
        assert!(child.wait_until_exit(Duration::from_secs(5)).unwrap());
        assert_eq!(fs::read_to_string(&out).unwrap().trim(), "lease=unset");
    }

    #[test]
    fn running_encoder_keeps_the_inherited_lease_held() {
        use crate::json::Value;
        use crate::recording::Workspace;
        use crate::recording::journal::Journal;
        let workspace = Workspace::new(
            std::env::temp_dir().join(format!("tidyshot-encoder-journal-{}", std::process::id())),
        );
        workspace.ensure().unwrap();
        let path = workspace.captures().join("take.mp4");
        let journal = Journal::begin(workspace, &path, Value::Object(BTreeMap::new())).unwrap();
        let lock_path = path.with_extension("recording.lock");

        let dir = native_dir("journal-lease");
        // The guard borrows the journal's own lease, exactly as the recorder
        // would hand it to the encoder.
        let guard = RecordGuard::open_in(&dir, journal.lease_fd()).unwrap();
        let mut child = encoder("while :; do sleep 0.05; done", &guard);

        // The app dies: its pidfd closes and the journal drops its descriptor,
        // but the encoder still holds the inherited lease, so recovery must
        // see the take as live rather than discard a still-writing recording.
        drop(guard);
        drop(journal);
        let probe = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        assert!(probe.try_lock().is_err());

        child
            .stop_with(
                Duration::from_millis(50),
                Duration::from_millis(50),
                Duration::from_millis(50),
            )
            .unwrap();
        // With the encoder gone the lease is free again.
        probe.try_lock().unwrap();
    }

    #[test]
    fn empty_command_is_rejected() {
        let _lease = lease_file("empty");
        let (guard, _dir) = guard("empty", &_lease);
        let err = Encoder::launch(&[], &BTreeMap::new(), &guard).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn graceful_encoder_exits_on_sigint() {
        let _lease = lease_file("graceful");
        let (guard, _dir) = guard("graceful", &_lease);
        let mut child = encoder("trap 'exit 0' INT; while :; do sleep 0.05; done", &guard);
        std::thread::sleep(Duration::from_millis(100));
        child
            .stop_with(
                Duration::from_secs(2),
                Duration::from_millis(50),
                Duration::from_millis(50),
            )
            .unwrap();
        assert_eq!(child.exit_code(), Some(0));
    }

    #[test]
    fn stubborn_encoder_is_killed_after_the_grace() {
        let _lease = lease_file("stubborn");
        let (guard, _dir) = guard("stubborn", &_lease);
        let mut child = encoder("trap '' INT TERM; while :; do sleep 0.05; done", &guard);
        std::thread::sleep(Duration::from_millis(100));
        child
            .stop_with(
                Duration::from_millis(50),
                Duration::from_millis(50),
                Duration::from_millis(50),
            )
            .unwrap();
        assert!(child.exit_code().is_some());
    }

    #[test]
    fn already_exited_encoder_is_not_signalled() {
        let _lease = lease_file("exited");
        let (guard, _dir) = guard("exited", &_lease);
        let mut child = encoder("exit 7", &guard);
        assert!(child.wait_until_exit(Duration::from_secs(2)).unwrap());
        child.stop_discarded().unwrap();
        assert_eq!(child.exit_code(), Some(7));
    }

    #[test]
    fn a_still_running_encoder_reports_its_pid() {
        let _lease = lease_file("running");
        let (guard, _dir) = guard("running", &_lease);
        let mut child = encoder("while :; do sleep 0.05; done", &guard);
        assert!(child.pid() > 0);
        assert!(child.running());
        child
            .stop_with(
                Duration::from_millis(50),
                Duration::from_millis(50),
                Duration::from_millis(50),
            )
            .unwrap();
    }
}
