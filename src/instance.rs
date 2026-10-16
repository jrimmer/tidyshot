//! One running instance and the local command socket.
// Ported from omnishot/app.py (main, QLocalServer/QLocalSocket)
//!
//! Commands are delivered to the running instance over a user-only local
//! socket. The payload is the raw argument vector as JSON, so the receiver
//! re-parses it through the same [`crate::cli`] path the sender used and the
//! two can never disagree about routing. Headless commands that must work
//! when the app is broken, such as `doctor`, never open the socket.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::json::Value;

/// The maximum forwarded command size, matching the clipboard file-list bound
/// rather than trusting a peer to send an unbounded line.
pub const MAX_COMMAND_BYTES: usize = 1024 * 1024;

/// The timeout for connecting to an existing instance.
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);

/// The socket path for one user, isolated by data directory so a test or a
/// second install never talks to the primary app.
#[must_use]
pub fn socket_path(state_dir: &Path) -> PathBuf {
    state_dir.join(format!("tidyshot-{}.sock", user_id()))
}

/// The per-user runtime directory the socket lives in.
///
/// `XDG_RUNTIME_DIR` is the right home for a socket; when it is unset the data
/// directory is a working fallback so the protocol still functions.
#[must_use]
pub fn state_dir() -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("XDG_DATA_HOME").map(PathBuf::from))
        .unwrap_or_else(|_| {
            std::env::var("HOME").map_or_else(|_| PathBuf::from("."), PathBuf::from).join(".local/share")
        })
        .join("tidyshot")
}

fn user_id() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// Serialize an argument vector into the one-line wire form.
#[must_use]
pub fn encode(argv: &[String]) -> Vec<u8> {
    let value = Value::Array(argv.iter().map(|s| Value::String(s.clone())).collect());
    let mut bytes = value.to_string_compact().into_bytes();
    bytes.push(b'\n');
    bytes
}

/// Parse one wire line back into an argument vector.
///
/// A malformed payload is a protocol error, never a panic.
pub fn decode(line: &str) -> Result<Vec<String>, String> {
    let value = crate::json::parse(line.trim()).map_err(|error| error.to_string())?;
    let array = value
        .as_array()
        .ok_or_else(|| "command payload must be an array".to_owned())?;
    let mut argv = Vec::with_capacity(array.len());
    for item in array {
        let text = item
            .as_str()
            .ok_or_else(|| "command arguments must be strings".to_owned())?;
        argv.push(text.to_owned());
    }
    Ok(argv)
}

/// The result of trying to hand a command to a running instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Forwarded {
    /// A running instance accepted the command.
    Delivered,
    /// No instance is listening; the caller should become the server.
    NoInstance,
}

/// Forward `argv` to a running instance, if one is listening.
///
/// A refused, missing or broken socket means there is no instance; the caller
/// goes on to host one. A write that fails after connecting is an error so the
/// user is told the command was not delivered.
pub fn forward(argv: &[String], socket: &Path) -> io::Result<Forwarded> {
    let mut stream = match connect(socket) {
        Some(stream) => stream,
        None => return Ok(Forwarded::NoInstance),
    };
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    stream.write_all(&encode(argv))?;
    stream.flush()?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    Ok(Forwarded::Delivered)
}

/// Connect to the socket, mapping the "not listening" errors to `None`.
fn connect(socket: &Path) -> Option<UnixStream> {
    match UnixStream::connect(socket) {
        Ok(stream) => Some(stream),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::AddrNotAvailable
            ) =>
        {
            None
        }
        Err(_) => None,
    }
}

/// Errors binding or serving the instance socket.
#[derive(Debug)]
pub enum InstanceError {
    /// The socket could not be created.
    Bind(io::Error),
    /// A connection could not be read.
    Read(io::Error),
}

impl std::fmt::Display for InstanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstanceError::Bind(error) => write!(f, "could not listen for commands: {error}"),
            InstanceError::Read(error) => write!(f, "could not read a command: {error}"),
        }
    }
}

impl std::error::Error for InstanceError {}

/// The listening half of the single-instance protocol.
pub struct Instance {
    listener: UnixListener,
    socket: PathBuf,
}

impl Instance {
    /// Bind the user-only socket, replacing a stale one from a crashed run.
    pub fn bind(socket: impl Into<PathBuf>) -> Result<Self, InstanceError> {
        let socket = socket.into();
        if let Some(parent) = socket.parent() {
            let _ = fs::create_dir_all(parent);
        }
        // A socket file left by a crashed instance would block `bind`, so
        // remove it first; a live instance is detected before this point.
        let _ = fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).map_err(InstanceError::Bind)?;
        // Keep the command channel to the owner. The umask can make a new file
        // group- or world-readable, so set the mode explicitly.
        if let Err(error) = fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)) {
            let _ = fs::remove_file(&socket);
            return Err(InstanceError::Bind(error));
        }
        Ok(Self { listener, socket })
    }

    /// The socket path this instance owns.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.socket
    }

    /// Accept one connection and parse its command.
    ///
    /// Returns `Ok(None)` when the peer sent nothing usable, so one bad client
    /// never brings the server down.
    pub fn accept(&self) -> Result<Option<Vec<String>>, InstanceError> {
        let (stream, _) = self.listener.accept().map_err(InstanceError::Read)?;
        match read_command(stream) {
            Ok(Some(argv)) => Ok(Some(argv)),
            Ok(None) => Ok(None),
            Err(InstanceError::Read(error)) if error.kind() == io::ErrorKind::Interrupted => {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
    }
}

/// Read one newline-terminated command from a connection.
fn read_command(mut stream: UnixStream) -> Result<Option<Vec<String>>, InstanceError> {
    stream
        .set_read_timeout(Some(Duration::from_millis(1000)))
        .map_err(InstanceError::Read)?;
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => {
                buffer.extend_from_slice(&chunk[..count]);
                if buffer.len() > MAX_COMMAND_BYTES {
                    return Ok(None);
                }
                if buffer.contains(&b'\n') {
                    break;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(InstanceError::Read(error)),
        }
    }
    let line = match buffer.iter().position(|byte| *byte == b'\n') {
        Some(index) => &buffer[..index],
        None => &buffer[..],
    };
    let text = match std::str::from_utf8(line) {
        Ok(text) => text,
        Err(_) => return Ok(None),
    };
    match decode(text) {
        Ok(argv) => Ok(Some(argv)),
        Err(_) => Ok(None),
    }
}

/// Run the accept loop, calling `on_command` for every delivered command.
///
/// The loop blocks; production runs it on its own thread so the GUI stays
/// responsive. It returns when `on_command` asks to stop.
pub fn serve<F>(instance: &Instance, mut on_command: F) -> Result<(), InstanceError>
where
    F: FnMut(Vec<String>) -> bool,
{
    loop {
        match instance.accept()? {
            Some(argv) => {
                if !on_command(argv) {
                    return Ok(());
                }
            }
            None => continue,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tidyshot-instance-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn arguments_round_trip_through_the_wire_format() {
        let argv = vec![
            "area".to_owned(),
            "--geometry".to_owned(),
            "10,20 300x200".to_owned(),
        ];
        let encoded = encode(&argv);
        assert!(encoded.ends_with(b"\n"));
        assert_eq!(decode(std::str::from_utf8(&encoded).unwrap()).unwrap(), argv);
    }

    #[test]
    fn a_malformed_payload_is_a_protocol_error_not_a_panic() {
        assert!(decode("not json").is_err());
        assert!(decode("{}").is_err());
        assert!(decode(r#"[1,2]"#).is_err());
    }

    #[test]
    fn a_command_is_delivered_to_the_instance() {
        let socket = scratch("deliver").join("tidyshot.sock");
        let instance = Instance::bind(&socket).unwrap();
        assert_eq!(instance.path(), socket);
        assert_eq!(
            forward(&["scroll".to_owned(), "--action".to_owned(), "copy".to_owned()], &socket)
                .unwrap(),
            Forwarded::Delivered
        );
        let received = instance.accept().unwrap().unwrap();
        assert_eq!(received, vec!["scroll", "--action", "copy"]);
    }

    #[test]
    fn with_no_instance_the_caller_becomes_the_server() {
        let socket = scratch("absent").join("tidyshot.sock");
        assert_eq!(
            forward(&["menu".to_owned()], &socket).unwrap(),
            Forwarded::NoInstance
        );
        // Binding then removing the socket is what a shutdown does; a later
        // command must not be delivered to a dead instance.
        {
            let instance = Instance::bind(&socket).unwrap();
            drop(instance);
        }
        assert!(!socket.exists());
        assert_eq!(
            forward(&["menu".to_owned()], &socket).unwrap(),
            Forwarded::NoInstance
        );
    }

    #[test]
    fn a_stale_socket_file_is_replaced_on_bind() {
        let socket = scratch("stale").join("tidyshot.sock");
        fs::write(&socket, b"stale").unwrap();
        let instance = Instance::bind(&socket).unwrap();
        assert_eq!(instance.path(), socket);
    }

    #[test]
    fn the_socket_is_owner_only() {
        let socket = scratch("mode").join("tidyshot.sock");
        let _instance = Instance::bind(&socket).unwrap();
        let mode = fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn the_serve_loop_hands_each_command_to_the_handler() {
        let socket = scratch("serve").join("tidyshot.sock");
        let instance = Instance::bind(&socket).unwrap();
        let path = instance.path().to_path_buf();
        let worker = thread::spawn(move || {
            let mut seen = Vec::new();
            serve(&instance, |argv| {
                seen.push(argv);
                seen.len() < 2
            })
            .unwrap();
            seen
        });
        // Wait for the listener to accept, then deliver two commands.
        for command in ["area", "scroll-done"] {
            for _ in 0..50 {
                if forward(&[command.to_owned()], &path).unwrap() == Forwarded::Delivered {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
        let seen = worker.join().unwrap();
        assert_eq!(seen, vec![vec!["area"], vec!["scroll-done"]]);
    }
}
