//! Versions TidyShot is tested with, and the Hyprland plugin ABI hash.
// Ported from omnishot/compat.py
//!
//! This is the single source for the supported versions. It uses only the
//! standard library so `tidyshot compat` can run before the application, and
//! before Qt, like the Python module install.sh calls.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Omarchy releases the app is tested against.
pub const TESTED_OMARCHY: &[&str] = &["4.0.2"];
/// Hyprland releases the app is tested against.
pub const TESTED_HYPRLAND: &[&str] = &["0.56.2"];

/// A one-line, human-readable summary of the supported versions.
#[must_use]
pub fn tested_summary() -> String {
    format!(
        "Omarchy {} and Hyprland {}",
        TESTED_OMARCHY.join(" / "),
        TESTED_HYPRLAND.join(" / ")
    )
}

/// Names of the given versions that are not in the tested lists.
///
/// `omarchy version` reports the package version, e.g. `4.0.3rc4-1`, so the
/// release suffix is stripped before comparing.
// Ported from omnishot/compat.py (untested)
#[must_use]
pub fn untested(omarchy: &str, hyprland: &str) -> Vec<String> {
    let mut found = Vec::new();
    if !omarchy.is_empty() {
        let base = omarchy.rsplit_once('-').map_or(omarchy, |(base, _)| base);
        if !TESTED_OMARCHY.contains(&base) {
            found.push(format!("Omarchy {omarchy}"));
        }
    }
    if !hyprland.is_empty() && !TESTED_HYPRLAND.contains(&hyprland) {
        found.push(format!("Hyprland {hyprland}"));
    }
    found
}

/// The ABI hash a plugin built against these headers expects.
///
/// Mirrors `__hyprland_api_get_client_hash()` in Hyprland's `PluginAPI.hpp`,
/// so it can be compared with the running compositor's `hyprctl -j version`
/// `abiHash`.
// Ported from omnishot/compat.py (header_abi_hash)
#[must_use]
pub fn header_abi_hash(version_h: &Path) -> Option<String> {
    let text = std::fs::read_to_string(version_h).ok()?;
    let defines = parse_defines(&text);
    let commit = defines.get("GIT_COMMIT_HASH")?;
    let parts = [
        ("aq", "AQUAMARINE_VERSION"),
        ("hu", "HYPRUTILS_VERSION"),
        ("hg", "HYPRGRAPHICS_VERSION"),
        ("hc", "HYPRCURSOR_VERSION"),
        ("hlg", "HYPRLANG_VERSION"),
    ];
    let mut hash = commit.clone();
    for (tag, name) in parts {
        let value = defines.get(name)?;
        hash.push_str(&format!("_{tag}_{}", strip_patch(value)));
    }
    Some(hash)
}

/// The version.h a plugin build would compile against, or None.
///
/// Found through the same `-I` paths a plugin build uses. `hyprland.pc` sets
/// only a prefix, not includedir, so asking pkg-config for includedir is not
/// enough.
// Ported from omnishot/compat.py (header_version_file)
#[must_use]
pub fn header_version_file() -> Option<PathBuf> {
    header_version_file_with(|| {
        let output = Command::new("pkg-config")
            .args(["--cflags-only-I", "hyprland"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8(output.stdout).ok()
    })
}

/// Resolve the version header from pkg-config output, injectable for tests.
pub fn header_version_file_with<F>(run: F) -> Option<PathBuf>
where
    F: FnOnce() -> Option<String>,
{
    version_file_from_flags(&run()?)
}

/// Find `version.h` among whitespace-separated compiler include flags.
#[must_use]
pub fn version_file_from_flags(flags: &str) -> Option<PathBuf> {
    for flag in flags.split_whitespace() {
        let Some(include) = flag.strip_prefix("-I") else {
            continue;
        };
        if include.is_empty() {
            continue;
        }
        let include = Path::new(include);
        let nested = include.join("hyprland/src/version.h");
        if nested.is_file() {
            return Some(nested);
        }
        if include.ends_with("hyprland/src") {
            let sibling = include.join("version.h");
            if sibling.is_file() {
                return Some(sibling);
            }
        }
    }
    None
}

/// Handle `tidyshot compat header-abi|warn`, mirroring the Python module.
pub fn cli(argv: &[String]) -> i32 {
    match argv {
        [command] if command == "header-abi" => {
            let Some(path) = header_version_file() else {
                eprintln!("Hyprland headers not found");
                return 1;
            };
            match header_abi_hash(&path) {
                Some(hash) => {
                    println!("{hash}");
                    0
                }
                None => {
                    eprintln!("Hyprland headers not found");
                    1
                }
            }
        }
        [command] if command == "warn" => {
            // Untested versions may work: say so, but never block the caller.
            let hyprland = command_stdout(&["hyprctl", "-j", "version"])
                .and_then(|text| crate::json::parse(&text).ok())
                .and_then(|value| {
                    value
                        .get("version")
                        .and_then(|v| v.as_str().map(str::to_owned))
                })
                .unwrap_or_default();
            let found = untested(
                &command_stdout(&["omarchy", "version"]).unwrap_or_default(),
                &hyprland,
            );
            if !found.is_empty() {
                eprintln!(
                    "Warning: TidyShot is tested with {}; this is {}. It may still work; report problems with the output of: tidyshot doctor --json",
                    tested_summary(),
                    found.join(" and ")
                );
            }
            0
        }
        _ => {
            eprintln!("usage: tidyshot compat header-abi|warn");
            2
        }
    }
}

fn command_stdout(args: &[&str]) -> Option<String> {
    let (program, rest) = args.split_first()?;
    let output = Command::new(program).args(rest).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Parse `#define NAME "value"` lines into a map.
fn parse_defines(text: &str) -> BTreeMap<String, String> {
    let mut defines = BTreeMap::new();
    for line in text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("#define") else {
            continue;
        };
        let mut parts = rest.trim_start().splitn(2, char::is_whitespace);
        let (Some(name), Some(value)) = (parts.next(), parts.next()) else {
            continue;
        };
        let value = value.trim();
        let Some(start) = value.find('"') else {
            continue;
        };
        let Some(end) = value[start + 1..].find('"') else {
            continue;
        };
        defines.insert(
            name.to_owned(),
            value[start + 1..start + 1 + end].to_owned(),
        );
    }
    defines
}

/// Drop the patch component of a version, as the ABI hash does.
fn strip_patch(version: &str) -> String {
    version
        .rsplit_once('.')
        .map_or_else(|| version.to_owned(), |(base, _)| base.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untested_versions_are_named() {
        assert!(untested("4.0.2-1", "0.56.2").is_empty());
        assert_eq!(untested("4.0.3rc4-1", ""), ["Omarchy 4.0.3rc4-1"]);
        assert_eq!(untested("", "0.57.0"), ["Hyprland 0.57.0"]);
    }

    #[test]
    fn header_hash_uses_the_plugin_api_scheme() {
        let dir = std::env::temp_dir().join(format!("tidyshot-compat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("version.h");
        std::fs::write(
            &path,
            "#define GIT_COMMIT_HASH \"efb50993780079460b0cbed1363e2166a2de1d9f\"\n\
             #define AQUAMARINE_VERSION \"0.15.0\"\n\
             #define HYPRUTILS_VERSION \"0.14.1\"\n\
             #define HYPRGRAPHICS_VERSION \"0.5.1\"\n\
             #define HYPRCURSOR_VERSION \"0.1.13\"\n\
             #define HYPRLANG_VERSION \"0.6.8\"\n",
        )
        .unwrap();
        assert_eq!(
            header_abi_hash(&path).as_deref(),
            Some("efb50993780079460b0cbed1363e2166a2de1d9f_aq_0.15_hu_0.14_hg_0.5_hc_0.1_hlg_0.6")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
