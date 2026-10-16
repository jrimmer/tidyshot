//! Bounded shutdown for encoders the user chose to discard.
// Ported from omnishot/recording_process.py
//!
//! Cancelling a take must stop its encoder without hanging on a process that
//! ignores signals. The shutdown escalates: SIGINT, then SIGTERM, then
//! SIGKILL, each with a bounded wait. A discarded take never leaves a live
//! encoder behind, and the last-resort kill is not reported as a failure in
//! its own right.

use std::io;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The default grace the encoder gets after SIGINT, in seconds.
pub const DEFAULT_GRACE: Duration = Duration::from_secs(3);
/// The default grace after SIGTERM, in seconds.
pub const DEFAULT_TERMINATE: Duration = Duration::from_secs(2);
/// The final grace after SIGKILL, in seconds.
pub const DEFAULT_KILL: Duration = Duration::from_secs(2);

/// A launched encoder that can be stopped without a hang.
#[derive(Debug)]
pub struct Encoder {
    child: Child,
    stdin: Option<std::process::ChildStdin>,
}

impl Encoder {
    /// Launch `command` in its own process group.
    ///
    /// The caller supplies the journal lease through `env`; a dedicated
    /// process group lets a stopped encoder be signalled as a whole, so any
    /// child encoder dies with it.
    pub fn launch(
        command: &[String],
        env: &std::collections::BTreeMap<String, String>,
    ) -> io::Result<Self> {
        let Some((program, args)) = command.split_first() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the encoder command is empty",
            ));
        };
        let mut child = Command::new(program);
        child.args(args);
        child.stdin(Stdio::piped());
        child.stdout(Stdio::piped());
        child.stderr(Stdio::piped());
        for (key, value) in env {
            child.env(key, value);
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
            ("SIGINT", grace),
            ("SIGTERM", terminate_timeout),
            ("SIGKILL", kill_timeout),
        ] {
            if self.has_exited()? {
                return Ok(());
            }
            self.signal(signal)?;
            if self.wait_until_exit(wait) {
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
    fn wait_until_exit(&mut self, limit: Duration) -> bool {
        if limit.is_zero() {
            return self.has_exited().unwrap_or(true);
        }
        let deadline = Instant::now() + limit;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) => {}
                Err(_) => return true,
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    fn signal(&mut self, name: &str) -> io::Result<()> {
        let pid = self.child.id();
        let signal = match name {
            "SIGINT" => "-INT",
            "SIGTERM" => "-TERM",
            _ => "-KILL",
        };
        // The child leads its own process group, so a negative pid signals
        // the whole group and takes any encoder child with it. ESRCH (the
        // child is already gone) is success.
        let _ = Command::new("kill")
            .args([signal, &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        Ok(())
    }

    #[cfg(not(unix))]
    fn signal(&mut self, _name: &str) -> io::Result<()> {
        self.child.kill()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoder(body: &str) -> Encoder {
        let command = vec!["sh".to_owned(), "-c".to_owned(), body.to_owned()];
        Encoder::launch(&command, &std::collections::BTreeMap::new()).unwrap()
    }

    #[test]
    fn graceful_encoder_exits_on_sigint() {
        let mut child = encoder("trap 'exit 0' INT; while :; do sleep 0.05; done");
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
        let mut child = encoder("trap '' INT TERM; while :; do sleep 0.05; done");
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
        let mut child = encoder("exit 7");
        assert!(child.wait_until_exit(Duration::from_secs(2)));
        child.stop_discarded().unwrap();
        assert_eq!(child.exit_code(), Some(7));
    }

    #[test]
    fn a_still_running_encoder_reports_its_pid() {
        let mut child = encoder("while :; do sleep 0.05; done");
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
