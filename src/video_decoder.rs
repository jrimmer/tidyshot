//! Choose between hardware and software decoding for Qt video previews.
// Ported from omnishot/video_decoder.py
//!
//! Qt copies decoded surfaces into a QImage for the annotation compositor, and
//! hardware decoders ask for large contiguous buffers that fail repeatedly
//! under memory pressure. Previews therefore decode in software unless the
//! user opts in, and an explicit value set before launch stays authoritative.
//! Intel graphics always stays software because iHD can crash in
//! `vaSyncSurface` during rapid seeks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::json::Value;

/// The environment variable Qt's media backend reads once at startup.
pub const VARIABLE: &str = "QT_FFMPEG_DECODING_HW_DEVICE_TYPES";

/// The stored setting's default: previews decode in software.
pub const DEFAULT_HARDWARE_VIDEO_DECODING: bool = false;

/// The interaction between the setting, the GPU and an explicit override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderPolicy {
    /// A value was set before launch; leave it untouched.
    Explicit,
    /// Force software decoding by writing the empty device list.
    Software,
    /// Remove the variable so Qt picks its hardware decoders.
    Hardware,
}

/// Whether a render node under `root` reports an Intel vendor id.
///
/// Asahi's platform GPU exposes no PCI vendor file, so an absent file is not
/// an error: only an actual `0x8086` match marks Intel.
#[must_use]
pub fn intel_gpu_present_in(root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("renderD") {
            continue;
        }
        let vendor = entry.path().join("device/vendor");
        let Ok(text) = std::fs::read_to_string(vendor) else {
            continue;
        };
        if text.trim() == "0x8086" {
            return true;
        }
    }
    false
}

/// The system render-node root.
#[must_use]
pub fn drm_root() -> PathBuf {
    PathBuf::from("/sys/class/drm")
}

/// Whether hardware decoding is safe on this machine.
///
/// The setting is still required before hardware is actually used; this only
/// reports that the GPU has no known decoder hazard.
#[must_use]
pub fn hardware_decoding_supported() -> bool {
    hardware_decoding_supported_in(&drm_root())
}

/// Like [`hardware_decoding_supported`] against an explicit `root`.
#[must_use]
pub fn hardware_decoding_supported_in(root: &Path) -> bool {
    !intel_gpu_present_in(root)
}

/// The explicit value of [`VARIABLE`] observed before this module wrote it.
///
/// A value set before TidyShot started (for example while debugging) stays
/// authoritative; only the value this module writes follows the setting. The
/// capture happens once, on first use, so a later configure call cannot mistake
/// this module's own write for a user choice.
static EXPLICIT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// The pre-launch value of [`VARIABLE`], captured on first use.
#[must_use]
pub fn explicit_override() -> Option<&'static str> {
    EXPLICIT
        .get_or_init(|| std::env::var(VARIABLE).ok())
        .as_deref()
}

/// Resolve the preview decoding policy.
///
/// `explicit` is the value of [`VARIABLE`] observed before this process wrote
/// it; a `None` is an unset variable. `intel_present` comes from the render
/// node scan so callers can test the policy without a GPU.
#[must_use]
pub fn decoder_policy(
    settings: &BTreeMap<String, Value>,
    explicit: Option<&str>,
    intel_present: bool,
) -> DecoderPolicy {
    if explicit.is_some() {
        return DecoderPolicy::Explicit;
    }
    let enabled = settings
        .get("hardware_video_decoding")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if enabled && !intel_present {
        DecoderPolicy::Hardware
    } else {
        DecoderPolicy::Software
    }
}

/// Apply a policy, reporting the value Qt should see next launch.
///
/// `apply` receives the variable name and the value the setting dictates;
/// `Explicit` yields `None`, meaning the caller leaves the variable alone.
#[must_use]
pub fn policy_environment(policy: DecoderPolicy) -> Option<(&'static str, Option<&'static str>)> {
    match policy {
        DecoderPolicy::Explicit => None,
        DecoderPolicy::Software => Some((VARIABLE, Some(","))),
        DecoderPolicy::Hardware => Some((VARIABLE, None)),
    }
}

/// Configure the preview decoder for the stored settings.
///
/// Qt reads the variable when its media backend starts, so a change applies
/// after the app restarts.
pub fn configure_preview_decoder(settings: &BTreeMap<String, Value>) {
    let explicit = explicit_override();
    let policy = decoder_policy(settings, explicit, intel_gpu_present_in(&drm_root()));
    // SAFETY: this runs on the main thread before Qt initialises its media
    // backend, and no other thread reads the environment during this call.
    unsafe {
        if let Some((name, value)) = policy_environment(policy) {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

/// Whether the Advanced settings checkbox is usable.
#[must_use]
pub fn setting_enabled(intel_present: bool) -> bool {
    !intel_present
}

/// The checkbox tooltip, including the restart reminder.
#[must_use]
pub fn setting_tooltip(intel_present: bool) -> &'static str {
    if intel_present {
        "Unavailable on Intel graphics, which can crash while seeking."
    } else {
        "Can play large videos more smoothly, but may stall the desktop when memory is low. Applies after TidyShot restarts."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(enabled: bool) -> BTreeMap<String, Value> {
        let mut map = BTreeMap::new();
        map.insert("hardware_video_decoding".to_owned(), Value::Bool(enabled));
        map
    }

    #[test]
    fn software_decoding_is_the_default_on_every_gpu() {
        for intel in [true, false] {
            assert_eq!(
                decoder_policy(&BTreeMap::new(), None, intel),
                DecoderPolicy::Software
            );
            assert_eq!(
                decoder_policy(&settings(false), None, intel),
                DecoderPolicy::Software
            );
        }
        assert_eq!(
            policy_environment(DecoderPolicy::Software),
            Some((VARIABLE, Some(",")))
        );
    }

    #[test]
    fn setting_enables_hardware_decoding_except_on_intel() {
        assert_eq!(
            decoder_policy(&settings(true), None, false),
            DecoderPolicy::Hardware
        );
        assert_eq!(
            decoder_policy(&settings(true), None, true),
            DecoderPolicy::Software
        );
        assert_eq!(
            policy_environment(DecoderPolicy::Hardware),
            Some((VARIABLE, None))
        );
    }

    #[test]
    fn value_set_before_startup_stays_authoritative() {
        assert_eq!(
            decoder_policy(&settings(true), Some("vaapi"), false),
            DecoderPolicy::Explicit
        );
        assert_eq!(
            decoder_policy(&settings(false), Some(","), true),
            DecoderPolicy::Explicit
        );
        assert_eq!(policy_environment(DecoderPolicy::Explicit), None);
    }

    #[test]
    fn intel_is_detected_from_the_render_node_vendor_file() {
        let root = std::env::temp_dir().join(format!("tidyshot-drm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("card0")).unwrap();
        std::fs::create_dir_all(root.join("renderD128/device")).unwrap();
        std::fs::write(root.join("renderD128/device/vendor"), "0x1002\n").unwrap();
        assert!(!intel_gpu_present_in(&root));
        std::fs::write(root.join("renderD128/device/vendor"), "0x8086\n").unwrap();
        assert!(intel_gpu_present_in(&root));
        // A render node without a vendor file (Asahi) is not Intel.
        std::fs::remove_file(root.join("renderD128/device/vendor")).unwrap();
        assert!(!intel_gpu_present_in(&root));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn setting_is_disabled_only_on_intel() {
        assert!(setting_enabled(false));
        assert!(!setting_enabled(true));
        assert!(setting_tooltip(false).contains("restarts"));
        assert!(setting_tooltip(true).contains("Intel"));
    }
}
