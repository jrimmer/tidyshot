//! Isolated window capture, the window picker model and Hyprland styling.
// Ported from omnishot/backend.py (grab_window, select_window,
// window_background), omnishot/window_selection.py and
// omnishot/pin_shadow.py.
//!
//! A window capture is not a rectangle grab: the Hyprland toplevel-export
//! protocol copies the window's own surface, so overlap, shadows and the
//! wallpaper never leak in, and the alpha channel survives. The helper that
//! speaks the protocol is a standalone Wayland program; this module resolves
//! the window identity, runs the helper, and parses the PAM stream it writes.
//!
//! The picker model keeps the fields the source selector decides on: the
//! focus order it sorts by, the client rectangle it hit-tests, and the
//! identity it captures.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::commands::{CommandError, CommandRunner, CommandSpec, hypr};
use crate::geometry::Rect;
use crate::json::Value;
use crate::pngzip::Image;

/// File name of the window export helper.
pub const HELPER: &str = "window-helper";

/// The largest side, and the largest buffer, a window frame may use.
pub const MAX_SIDE: u32 = 30_000;
pub const MAX_BUFFER: u64 = 512_000_000;

/// The largest padding the window background accepts.
pub const MAX_PADDING: i32 = 200;

/// The default padding around an isolated window.
pub const DEFAULT_PADDING: i32 = 24;

/// The path of the window export helper inside a native directory.
#[must_use]
pub fn helper_path(native: &Path) -> PathBuf {
    native.join(HELPER)
}

/// True when the window helper was built beside the other native binaries.
#[must_use]
pub fn helper_available(native: &Path) -> bool {
    helper_path(native).is_file()
}

/// The error a window capture reports, with the reason the UI shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowError {
    message: String,
}

impl WindowError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for WindowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for WindowError {}

impl From<CommandError> for WindowError {
    fn from(error: CommandError) -> Self {
        Self::new(error.to_string())
    }
}

/// A Hyprland window's capture identity.
///
/// The export protocol identifies a window by the low 32 bits of its address,
/// while the address itself and the stable id are what prove the same window
/// is still there before and after the copy. Titles are mutable and shared, so
/// they are never used to identify a capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowIdentity {
    pub address: String,
    pub stable_id: Option<String>,
}

impl WindowIdentity {
    /// Parse and validate a `0x…` window address.
    #[must_use]
    pub fn from_address(address: &str) -> Option<Self> {
        parse_address(address)?;
        Some(Self {
            address: address.to_owned(),
            stable_id: None,
        })
    }

    /// Build an identity from a `hyprctl clients` row.
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let address = value.get("address").and_then(Value::as_str)?;
        parse_address(address)?;
        Some(Self {
            address: address.to_owned(),
            stable_id: value
                .get("stableId")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }

    /// The handle the export protocol expects.
    #[must_use]
    pub fn handle(&self) -> Option<u32> {
        parse_address(&self.address).map(|address| address as u32)
    }

    /// Whether `other` describes the same window, not merely the same handle.
    #[must_use]
    pub fn same_window(&self, other: &Self) -> bool {
        self.address == other.address
            && match (&self.stable_id, &other.stable_id) {
                (Some(expected), Some(seen)) => expected == seen,
                _ => true,
            }
    }
}

/// Parse a Hyprland `0x` address into its numeric value.
#[must_use]
pub fn parse_address(text: &str) -> Option<u64> {
    let hex = text.strip_prefix("0x")?;
    if hex.is_empty() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(hex, 16).ok()
}

/// One selectable window from `hyprctl -j clients`.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    pub title: String,
    pub class: String,
    pub rect: Rect,
    pub monitor: String,
    pub workspace: String,
    pub address: String,
    pub stable_id: Option<String>,
    pub focus_history: i32,
    pub pinned: bool,
    pub fullscreen: bool,
}

impl WindowInfo {
    /// The line the picker shows for this window.
    #[must_use]
    pub fn label(&self) -> String {
        if self.title.is_empty() {
            self.class.clone()
        } else {
            format!("{} — {}", self.class, self.title)
        }
    }

    /// True when this client is one of TidyShot's own windows.
    ///
    /// A preview or pin is always on top, so leaving it in the window picker
    /// would offer the user a capture of TidyShot itself. The desktop id and
    /// the binary name are both matched, because the compositor's class comes
    /// from whichever of them Qt used when the window was mapped.
    #[must_use]
    pub fn is_own_preview(&self) -> bool {
        self.class.starts_with("tidyshot")
            || self.class.starts_with("org.omarchy.TidyShot")
            || self.class.starts_with("com.jasonrimmer.tidyshot")
            // A pin is always on top and can carry a different compositor
            // class, so its title is the identity that excludes it.
            || crate::pin_window::is_pin_title(&self.title)
    }

    /// The detail line: monitor, workspace and size.
    #[must_use]
    pub fn detail(&self) -> String {
        format!(
            "{} · {} · {} × {}",
            self.monitor, self.workspace, self.rect.width, self.rect.height
        )
    }

    /// The identity the isolated capture uses.
    #[must_use]
    pub fn identity(&self) -> Option<WindowIdentity> {
        let mut identity = WindowIdentity::from_address(&self.address)?;
        identity.stable_id.clone_from(&self.stable_id);
        Some(identity)
    }

    /// The focus-order key the picker sorts by.
    ///
    /// The source treats a missing or negative `focusHistoryID` as last, so a
    /// window the compositor did not rank sorts after every ranked one.
    // Ported from omnishot/window_selection.py (WindowSelector.__init__)
    #[must_use]
    pub fn focus_key(&self) -> i32 {
        if self.focus_history >= 0 {
            self.focus_history
        } else {
            9999
        }
    }

    /// Whether a point in absolute logical pixels lies inside the window.
    #[must_use]
    pub fn contains(&self, x: f64, y: f64) -> bool {
        let rect = self.rect;
        x >= f64::from(rect.x)
            && x <= f64::from(rect.right())
            && y >= f64::from(rect.y)
            && y <= f64::from(rect.bottom())
    }
}

/// Parse every usable window in a `hyprctl -j clients` array.
#[must_use]
pub fn windows_from(value: &Value) -> Vec<WindowInfo> {
    value
        .as_array()
        .unwrap_or(&[])
        .iter()
        .filter_map(window_from)
        .collect()
}

fn window_from(value: &Value) -> Option<WindowInfo> {
    if value
        .get("hidden")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || value
            .get("mapped")
            .and_then(Value::as_bool)
            .is_some_and(|mapped| !mapped)
    {
        return None;
    }
    let at = value.get("at").and_then(Value::as_array)?;
    let size = value.get("size").and_then(Value::as_array)?;
    if at.len() < 2 || size.len() < 2 {
        return None;
    }
    let x = at[0].as_f64()? as i32;
    let y = at[1].as_f64()? as i32;
    let width = size[0].as_f64()? as i32;
    let height = size[1].as_f64()? as i32;
    if width < 2 || height < 2 {
        return None;
    }
    let workspace = value
        .get("workspace")
        .and_then(|workspace| workspace.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    Some(WindowInfo {
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        class: value
            .get("class")
            .and_then(Value::as_str)
            .unwrap_or("Window")
            .to_owned(),
        rect: Rect::new(x, y, width, height),
        monitor: value
            .get("monitor")
            .and_then(Value::as_f64)
            .map_or_else(String::new, |monitor| monitor.to_string()),
        workspace: workspace.to_owned(),
        address: value
            .get("address")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        stable_id: value
            .get("stableId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        focus_history: value
            .get("focusHistoryID")
            .and_then(Value::as_f64)
            .map_or(-1, |id| id as i32),
        pinned: value
            .get("pinned")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        // Hyprland reports 2 for a real fullscreen surface; 1 and 3 are the
        // maximized states, which keep their rounding, as the source does.
        fullscreen: value.get("fullscreen").and_then(Value::as_f64) == Some(2.0),
    })
}

/// The windows on the visible workspaces, plus pinned ones.
///
/// A window on another workspace cannot be captured or hovered, so the picker
/// and the selectors both use this narrowed list, exactly as the source does.
// Ported from omnishot/backend.py (select_window) and
// omnishot/app.py (window_selection)
#[must_use]
pub fn visible_windows(clients: &Value, monitors: &Value) -> Vec<WindowInfo> {
    let visible: Vec<&str> = monitors
        .as_array()
        .unwrap_or(&[])
        .iter()
        .filter_map(|monitor| {
            monitor
                .get("activeWorkspace")
                .and_then(|workspace| workspace.get("name"))
                .and_then(Value::as_str)
        })
        .collect();
    windows_from(clients)
        .into_iter()
        .filter(|window| window.pinned || visible.contains(&window.workspace.as_str()))
        .collect()
}

/// Sort windows by focus order, keeping compositor order for equal keys.
#[must_use]
pub fn focus_order(windows: &[WindowInfo]) -> Vec<WindowInfo> {
    let mut sorted = windows.to_vec();
    sorted.sort_by_key(WindowInfo::focus_key);
    sorted
}

/// Parse the PAM RGBA stream the window helper writes.
///
/// The header names its width and height, so a truncated or oversized stream
/// is rejected before any pixels are copied. The helper already un-premultiplies
/// partial alpha, leaving straight RGBA8 like the rest of the pipeline.
// Ported from omnishot/backend.py (grab_window header parsing)
pub fn parse_pam(data: &[u8]) -> Result<Image, WindowError> {
    if !data.starts_with(b"P7\n") {
        return Err(WindowError::new("Invalid window capture"));
    }
    let boundary = data
        .windows(b"ENDHDR\n".len())
        .position(|window| window == b"ENDHDR\n")
        .ok_or_else(|| WindowError::new("Invalid window capture"))?;
    let header = &data[..boundary];
    let pixels = &data[boundary + b"ENDHDR\n".len()..];
    let text =
        std::str::from_utf8(header).map_err(|_| WindowError::new("Invalid window capture"))?;
    let mut width = None;
    let mut height = None;
    let mut depth = None;
    let mut maxval = None;
    let mut tuple = None;
    for line in text.lines().skip(1) {
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        match key {
            "WIDTH" => width = value.parse::<u32>().ok(),
            "HEIGHT" => height = value.parse::<u32>().ok(),
            "DEPTH" => depth = value.parse::<u32>().ok(),
            "MAXVAL" => maxval = value.parse::<u32>().ok(),
            "TUPLTYPE" => tuple = Some(value),
            _ => {}
        }
    }
    let (width, height) = match (width, height) {
        (Some(width), Some(height)) => (width, height),
        _ => return Err(WindowError::new("Invalid window capture")),
    };
    if depth != Some(4) || maxval != Some(255) || tuple != Some("RGB_ALPHA") {
        return Err(WindowError::new("Invalid window capture"));
    }
    if width == 0
        || height == 0
        || width > MAX_SIDE
        || height > MAX_SIDE
        || u64::from(width) * u64::from(height) * 4 > MAX_BUFFER
    {
        return Err(WindowError::new("Invalid window capture"));
    }
    let expected = width as usize * height as usize * 4;
    if pixels.len() != expected {
        return Err(WindowError::new("Incomplete window capture"));
    }
    Ok(Image::new(width as usize, height as usize, pixels.to_vec()))
}

/// The helper's unavailability message, naming the build step.
#[must_use]
pub fn helper_missing_message() -> &'static str {
    "Isolated window capture is unavailable: native/window-helper was not built. \
     Install wayland-scanner and the wayland-client headers, then run bin/native."
}

/// Capture one window through the toplevel-export helper.
///
/// The window is resolved before the copy, and again afterwards: a closed or
/// replaced window must fail instead of returning pixels of a different
/// window that inherited the same address handle. Every `hyprctl` query and
/// the helper go through the shared runner.
// Ported from omnishot/backend.py (grab_window)
pub fn capture<R: CommandRunner>(
    runner: &R,
    native: &Path,
    identity: &WindowIdentity,
    cursor: bool,
) -> Result<Image, WindowError> {
    let handle = identity
        .handle()
        .ok_or_else(|| WindowError::new("Invalid selected window"))?;
    let before = resolve(runner, identity)?;
    let helper = helper_path(native);
    if !helper.is_file() {
        return Err(WindowError::new(helper_missing_message()));
    }
    let spec = CommandSpec::new(helper.to_string_lossy().into_owned())
        .arg(handle.to_string())
        .arg(if cursor { "1" } else { "0" })
        .timeout(Duration::from_secs(12));
    let output = runner.run(&spec)?;
    let after = resolve(runner, identity)?;
    if !before.same_window(&after) {
        return Err(WindowError::new(
            "The selected window closed during capture. Select a window again.",
        ));
    }
    parse_pam(&output.stdout)
}

/// Re-resolve the selected identity against the live client list.
fn resolve<R: CommandRunner>(
    runner: &R,
    identity: &WindowIdentity,
) -> Result<WindowIdentity, WindowError> {
    let clients = hypr(runner, "clients")?;
    let handle = identity
        .handle()
        .ok_or_else(|| WindowError::new("Invalid selected window"))?;
    let mut matches = clients
        .as_array()
        .unwrap_or(&[])
        .iter()
        .filter_map(WindowIdentity::from_value)
        .filter(|found| found.handle() == Some(handle));
    let Some(current) = matches.next() else {
        return Err(WindowError::new(
            "Window closed or is unavailable for capture",
        ));
    };
    if matches.next().is_some() {
        return Err(WindowError::new(
            "Window closed or has an ambiguous capture handle. Select it again.",
        ));
    }
    if !identity.same_window(&current) {
        return Err(WindowError::new(
            "The selected window closed. Select a window again.",
        ));
    }
    Ok(current)
}

/// Capture a window chosen from the `slurp` list fallback.
///
/// The list carries the address as its slurp format label, so a headless
/// invocation resolves the same identity the picker would.
// Ported from omnishot/backend.py (select_window)
pub fn select<R: CommandRunner>(
    runner: &R,
    native: &Path,
    clients: &Value,
    cursor: bool,
) -> Result<Image, WindowError> {
    let windows = windows_from(clients);
    if windows.is_empty() {
        return Err(WindowError::new("There are no windows to capture"));
    }
    let rows: String = windows
        .iter()
        .map(|window| {
            format!(
                "{} {}\n",
                crate::geometry::geometry_text(window.rect),
                window.address
            )
        })
        .collect();
    let spec = CommandSpec::new("slurp")
        .args([
            "-r",
            "-f",
            "%l",
            "-b",
            "#00000000",
            "-c",
            "#ffffffff",
            "-w",
            "2",
        ])
        .stdin(rows.into_bytes())
        .timeout(Duration::from_secs(180));
    let output = runner.run(&spec).map_err(|error| match error {
        CommandError::Failure { .. } => WindowError::new("selection cancelled"),
        other => WindowError::new(other.to_string()),
    })?;
    let address = output.stdout_string().trim().to_owned();
    let window = windows
        .iter()
        .find(|window| window.address == address)
        .ok_or_else(|| WindowError::new("That window is no longer available"))?;
    let identity = window
        .identity()
        .ok_or_else(|| WindowError::new("Invalid selected window"))?;
    capture(runner, native, &identity, cursor)
}

/// The settings a window background is composed from.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowStyle {
    /// Hyprland's `decoration:rounding`, in logical pixels.
    pub rounding: i32,
    /// Hyprland's `decoration:rounding_power`, its superellipse exponent.
    pub rounding_power: f64,
    /// The padding around the window, clamped like the source.
    pub padding: i32,
    /// Whether the frame sits on a drop shadow.
    pub shadow: bool,
    /// The logical-to-physical scale the window frame was captured at.
    pub scale: f64,
    /// Whether the window is fullscreen, which removes rounding.
    pub fullscreen: bool,
}

impl Default for WindowStyle {
    fn default() -> Self {
        Self {
            rounding: 0,
            rounding_power: 2.0,
            padding: DEFAULT_PADDING,
            shadow: true,
            scale: 1.0,
            fullscreen: false,
        }
    }
}

impl WindowStyle {
    /// The background options for a window frame of `frame_width` pixels.
    ///
    /// Hyprland scales the perceived roundness by `power / 2`, so the radius
    /// follows that rule and the padding follows the capture scale.
    // Ported from omnishot/backend.py (window_background)
    #[must_use]
    pub fn background(
        &self,
        frame_width: usize,
        window_width: i32,
    ) -> crate::backgrounds::Background {
        let scale = if window_width > 0 {
            frame_width as f64 / f64::from(window_width)
        } else {
            self.scale.max(1.0)
        };
        let padding = self.padding.clamp(0, MAX_PADDING);
        let radius = if self.fullscreen {
            0.0
        } else {
            (f64::from(self.rounding) * self.rounding_power / 2.0 * scale).round()
        };
        crate::backgrounds::Background {
            color: crate::color::Rgba::new(0, 0, 0, 0),
            color2: crate::color::Rgba::new(0, 0, 0, 0),
            padding: (f64::from(padding) * scale).round() as i32,
            radius,
            radius_power: self.rounding_power.clamp(1.0, 10.0),
            shadow: self.shadow,
            aspect: "Auto".to_owned(),
            align: "Center".to_owned(),
            image_data: None,
        }
    }

    /// Apply a wallpaper image to the background when it should show through.
    pub fn with_wallpaper(
        &self,
        mut background: crate::backgrounds::Background,
        wallpaper: Option<Image>,
    ) -> crate::backgrounds::Background {
        if let Some(image) = wallpaper {
            background.image_data = Some(image);
        }
        background
    }
}

/// Read Hyprland's window rounding and power for the selected address.
///
/// `hyprctl getprop` reports a per-window override first; a window without one
/// falls back to the global decoration option. A failed query keeps the
/// defaults, so a capture never fails because the theme could not be read.
// Ported from omnishot/backend.py (window_background property_value)
pub fn hyprland_style<R: CommandRunner>(
    runner: &R,
    address: &str,
    scale: f64,
    fullscreen: bool,
) -> WindowStyle {
    let property = |name: &str| -> Option<f64> {
        let spec = CommandSpec::new("hyprctl").args([
            "-j",
            "getprop",
            &format!("address:{address}"),
            name,
        ]);
        let value = runner.run(&spec).ok()?;
        let text = value.stdout_string();
        let parsed = crate::json::parse(&text).ok()?;
        parsed.get(name).and_then(Value::as_f64)
    };
    let option = |name: &str, kind: &str| -> Option<f64> {
        let spec =
            CommandSpec::new("hyprctl").args(["-j", "getoption", &format!("decoration:{name}")]);
        let value = runner.run(&spec).ok()?;
        let text = value.stdout_string();
        let parsed = crate::json::parse(&text).ok()?;
        parsed.get(kind).and_then(Value::as_f64)
    };
    let rounding = property("rounding")
        .or_else(|| option("rounding", "int"))
        .unwrap_or(0.0);
    let power = property("rounding_power")
        .or_else(|| option("rounding_power", "float"))
        .unwrap_or(2.0);
    WindowStyle {
        rounding: rounding.round() as i32,
        rounding_power: power,
        padding: DEFAULT_PADDING,
        shadow: true,
        scale,
        fullscreen,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandOutput, CommandSpec};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fake {
        clients: Mutex<String>,
        results: Mutex<VecDeque<(String, CommandOutput)>>,
        seen: Mutex<Vec<CommandSpec>>,
    }

    impl Fake {
        fn new(clients: &str) -> Self {
            Self {
                clients: Mutex::new(clients.to_owned()),
                ..Self::default()
            }
        }

        fn push(&self, program: &str, stdout: Vec<u8>) {
            self.results.lock().unwrap().push_back((
                program.to_owned(),
                CommandOutput {
                    status: 0,
                    stdout,
                    stderr: Vec::new(),
                },
            ));
        }

        fn seen(&self) -> Vec<CommandSpec> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl CommandRunner for Fake {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            self.seen.lock().unwrap().push(spec.clone());
            if spec.program == "hyprctl" && spec.args == ["-j", "clients"] {
                return Ok(CommandOutput {
                    status: 0,
                    stdout: self.clients.lock().unwrap().clone().into_bytes(),
                    stderr: Vec::new(),
                });
            }
            let mut results = self.results.lock().unwrap();
            let position = results
                .iter()
                .position(|(program, _)| *program == spec.program)
                .unwrap_or_else(|| panic!("no queued {} call", spec.program));
            Ok(results.remove(position).unwrap().1)
        }
    }

    fn helpers(dir: &Path) {
        std::fs::write(dir.join(HELPER), b"#!/bin/sh\n").unwrap();
    }

    fn pam(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
        let mut data = format!(
            "P7\nWIDTH {width}\nHEIGHT {height}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n"
        )
        .into_bytes();
        for _ in 0..width * height {
            data.extend_from_slice(&pixel);
        }
        data
    }

    fn client(address: &str, title: &str, focus: i32) -> String {
        format!(
            r#"{{"address":"{address}","stableId":"s-{title}","title":"{title}","class":"code","at":[10,20],"size":[600,400],"monitor":0,"workspace":{{"name":"1"}},"mapped":true,"focusHistoryID":{focus}}}"#
        )
    }

    #[test]
    fn a_pam_stream_keeps_the_alpha_channel() {
        let image = parse_pam(&pam(2, 2, [10, 20, 30, 0])).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.pixel(0, 0), [10, 20, 30, 0]);
        assert_eq!(image.pixel(1, 0), [10, 20, 30, 0]);
    }

    #[test]
    fn a_malformed_or_truncated_pam_stream_is_rejected() {
        let mut data = pam(4, 4, [1, 2, 3, 255]);
        data.truncate(data.len() - 4);
        assert!(
            parse_pam(&data)
                .unwrap_err()
                .to_string()
                .contains("Incomplete")
        );
        assert!(parse_pam(b"P6\n2 2\n255\n").is_err());
        let mut bad_depth = pam(2, 2, [0, 0, 0, 255]);
        let marker = b"DEPTH 4";
        let offset = bad_depth
            .windows(marker.len())
            .position(|window| window == marker)
            .unwrap();
        bad_depth[offset + marker.len() - 1] = b'3';
        assert!(parse_pam(&bad_depth).is_err());
    }

    #[test]
    fn the_address_handle_is_the_low_thirty_two_bits() {
        let identity = WindowIdentity::from_address("0x112345678").unwrap();
        assert_eq!(identity.handle(), Some(0x1234_5678));
        assert!(WindowIdentity::from_address("0xZZ").is_none());
        assert!(WindowIdentity::from_address("1234").is_none());
    }

    #[test]
    fn an_isolated_window_is_read_through_the_helper() {
        let dir = std::env::temp_dir().join(format!("tidyshot-window-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        helpers(&dir);
        let clients = format!("[{}]", client("0x112345678", "Editor", 0));
        let runner = Fake::new(&clients);
        let image = pam(3, 2, [4, 5, 6, 255]);
        runner.push(&helper_path(&dir).to_string_lossy(), image.clone());
        let identity = WindowIdentity::from_address("0x112345678").unwrap();
        let captured = capture(&runner, &dir, &identity, false).unwrap();
        assert_eq!((captured.width, captured.height), (3, 2));
        assert_eq!(captured.pixel(1, 1), [4, 5, 6, 255]);
        let helper = runner
            .seen()
            .into_iter()
            .find(|spec| spec.program.ends_with(HELPER))
            .unwrap();
        assert_eq!(helper.args, ["305419896", "0"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_vanished_window_is_reported_instead_of_grabbed() {
        let dir = std::env::temp_dir().join(format!("tidyshot-window-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        helpers(&dir);
        let runner = Fake::new("[]");
        let identity = WindowIdentity::from_address("0x112345678").unwrap();
        let error = capture(&runner, &dir, &identity, false).unwrap_err();
        assert!(error.to_string().contains("closed"), "{error}");
        assert!(
            runner
                .seen()
                .iter()
                .all(|spec| !spec.program.ends_with(HELPER)),
            "a vanished window must not run the helper"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_windows_sharing_a_handle_are_ambiguous() {
        let dir = std::env::temp_dir().join(format!("tidyshot-window-dup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        helpers(&dir);
        let clients = format!(
            "[{},{}]",
            client("0x112345678", "One", 0),
            client("0x212345678", "Two", 1)
        );
        let runner = Fake::new(&clients);
        let identity = WindowIdentity::from_address("0x112345678").unwrap();
        let error = capture(&runner, &dir, &identity, false).unwrap_err();
        assert!(error.to_string().contains("ambiguous"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_helper_is_built_and_reported_when_missing() {
        let dir =
            std::env::temp_dir().join(format!("tidyshot-window-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let runner = Fake::new(&format!("[{}]", client("0x112345678", "Editor", 0)));
        let identity = WindowIdentity::from_address("0x112345678").unwrap();
        let error = capture(&runner, &dir, &identity, false).unwrap_err();
        assert_eq!(error.to_string(), helper_missing_message());
        assert!(error.to_string().contains("wayland-scanner"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn picker_rows_sort_by_focus_and_missing_ranks_last() {
        let value = crate::json::parse(&format!(
            "[{},{},{}]",
            client("0x1", "Editor", 2),
            r#"{"address":"0x2","title":"Browser","class":"firefox","at":[0,0],"size":[10,10],"workspace":{"name":"1"},"mapped":true}"#,
            client("0x3", "Terminal", 0)
        ))
        .unwrap();
        let windows = windows_from(&value);
        let ordered = focus_order(&windows);
        let ordered: Vec<&str> = ordered.iter().map(|window| window.title.as_str()).collect();
        assert_eq!(ordered, ["Terminal", "Editor", "Browser"]);
        assert_eq!(windows[1].focus_key(), 9999);
    }

    #[test]
    fn the_picker_hit_tests_absolute_client_rectangles() {
        let value =
            crate::json::parse(&format!("[{}]", client("0x112345678", "Editor", 0))).unwrap();
        let window = windows_from(&value).remove(0);
        assert!(window.contains(10.0, 20.0));
        assert!(window.contains(609.5, 419.5));
        assert!(!window.contains(5.0, 20.0));
    }

    #[test]
    fn windows_on_other_workspaces_are_not_offered() {
        let clients = crate::json::parse(&format!(
            r#"[{},{{"address":"0x4","title":"Hidden","class":"x","at":[0,0],"size":[10,10],"workspace":{{"name":"9"}},"mapped":true}},{{"address":"0x5","title":"Pinned","class":"x","at":[0,0],"size":[10,10],"workspace":{{"name":"9"}},"pinned":true,"mapped":true}}]"#,
            client("0x112345678", "Editor", 0)
        ))
        .unwrap();
        let monitors =
            crate::json::parse(r#"[{"name":"DP-1","activeWorkspace":{"name":"1"}}]"#).unwrap();
        let offered = visible_windows(&clients, &monitors);
        let offered: Vec<&str> = offered.iter().map(|window| window.title.as_str()).collect();
        assert_eq!(offered, ["Editor", "Pinned"]);
    }

    #[test]
    fn a_window_background_scales_padding_and_rounding() {
        let style = WindowStyle {
            rounding: 10,
            rounding_power: 2.0,
            padding: 24,
            shadow: true,
            scale: 2.0,
            fullscreen: false,
        };
        let background = style.background(1200, 600);
        assert_eq!(background.padding, 48);
        assert_eq!(background.radius, 20.0);
        assert_eq!(background.radius_power, 2.0);
        assert!(background.shadow);
        assert_eq!(background.color.alpha, 0);
    }

    #[test]
    fn a_fullscreen_window_has_no_rounded_corners() {
        let style = WindowStyle {
            rounding: 12,
            fullscreen: true,
            ..WindowStyle::default()
        };
        assert_eq!(style.background(600, 600).radius, 0.0);
    }

    #[test]
    fn only_a_real_fullscreen_client_loses_its_rounding() {
        let clients = r#"[
            {"address":"0x1","title":"Full","class":"code","at":[0,0],"size":[10,10],"workspace":{"name":"1"},"mapped":true,"fullscreen":2},
            {"address":"0x2","title":"Maximized","class":"code","at":[0,0],"size":[10,10],"workspace":{"name":"1"},"mapped":true,"fullscreen":1},
            {"address":"0x3","title":"Windowed","class":"code","at":[0,0],"size":[10,10],"workspace":{"name":"1"},"mapped":true}
        ]"#;
        let windows = windows_from(&crate::json::parse(clients).unwrap());
        assert!(windows[0].fullscreen);
        assert!(!windows[1].fullscreen);
        assert!(!windows[2].fullscreen);
    }
}
