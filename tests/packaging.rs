//! The native package and the user installer must agree on the desktop
//! integration: the MimeType list, the Desktop Actions and the icon name are
//! copied verbatim from `scripts/install_user.py`.

use std::fs;
use std::path::PathBuf;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

const MIME: &str = "MimeType=image/png;image/jpeg;image/webp;image/heic;image/heif;image/gif;video/mp4;video/webm;video/quicktime;video/x-matroska;application/x-tidyshot;application/x-tidyshot-video;x-scheme-handler/tidyshot;";
const ACTIONS: &str = "Actions=Capture;Scroll;History;";

#[test]
fn the_packaged_desktop_entry_matches_the_user_installer() {
    let root = repo();
    let packaged = fs::read_to_string(root.join("packaging/tidyshot.desktop")).unwrap();
    let installer = fs::read_to_string(root.join("scripts/install_user.py")).unwrap();
    for line in [MIME, ACTIONS] {
        assert!(packaged.contains(line), "packaged entry is missing {line}");
        assert!(
            installer.contains(line),
            "the installer no longer carries {line}"
        );
    }
    assert!(packaged.contains("Icon=tidyshot"));
    for action in [
        "[Desktop Action Capture]",
        "[Desktop Action Scroll]",
        "[Desktop Action History]",
    ] {
        assert!(packaged.contains(action), "missing {action}");
    }
}

#[test]
fn the_pkgbuild_installs_the_binary_desktop_entry_and_icon() {
    let root = repo();
    let pkgbuild = fs::read_to_string(root.join("pkgbuild/PKGBUILD")).unwrap();
    for needle in [
        "usr/bin/tidyshot",
        "usr/share/applications/org.omarchy.TidyShot.desktop",
        "usr/share/icons/hicolor/scalable/apps/tidyshot.svg",
        "usr/share/mime/packages/tidyshot.xml",
        "usr/lib/tidyshot/native/scroll-helper",
    ] {
        assert!(
            pkgbuild.contains(needle),
            "PKGBUILD does not install {needle}"
        );
    }
    assert!(pkgbuild.contains("cargo build --release --offline"));
    assert!(pkgbuild.contains("bin/native"));
}

#[test]
fn the_install_script_runs_makepkg_from_the_pkgbuild_directory() {
    let root = repo();
    let text = fs::read_to_string(root.join("bin/install")).unwrap();
    assert!(text.contains("makepkg -fsi"));
    assert!(text.contains("cd pkgbuild"));
    // Running makepkg as root is forbidden, and the script checks first.
    assert!(text.contains("EUID == 0"));
}

/// `bin/make-release-artifacts` must produce the layout `scripts/setup.sh`
/// consumes: a tarball named from the manifest version whose members are the
/// executable, the source commit stamp, and the icon helper.
#[test]
fn the_release_artifact_script_packages_the_layout_setup_expects() {
    use std::process::Command;

    let base = std::env::temp_dir().join(format!(
        "tidyshot-release-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(base.join("bin")).unwrap();
    fs::copy(
        repo().join("bin/make-release-artifacts"),
        base.join("bin/make-release-artifacts"),
    )
    .unwrap();
    fs::write(
        base.join("manifest.json"),
        "{\n  \"version\": \"9.8.7\",\n  \"name\": \"TidyShot\"\n}\n",
    )
    .unwrap();
    // A build stub stands in for the cargo and Wayland toolchain.
    fs::write(
        base.join("bin/build"),
        r#"#!/usr/bin/env bash
set -eu
mkdir -p build/native
printf '#!/bin/sh\n' >build/tidyshot
printf icon >build/generate-icon
printf svg >build/tidyshot.svg
chmod +x build/tidyshot build/generate-icon
printf helper >build/native/scroll-helper
"#,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt as _;
    for name in ["build", "make-release-artifacts"] {
        let path = base.join("bin").join(name);
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
    }
    let git = |args: &[&str]| {
        let status = Command::new("git")
            .arg("-C")
            .arg(&base)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&["commit", "-qm", "one"]);
    let head = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(&base)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    let result = Command::new("bash")
        .arg(base.join("bin/make-release-artifacts"))
        .env("HOME", "")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    let archive = base.join("dist/tidyshot-9.8.7-x86_64.tar.gz");
    assert!(archive.exists(), "the archive takes the manifest version");
    let checksums = fs::read_to_string(base.join("dist/checksums.txt")).unwrap();
    let mut fields = checksums.split_whitespace();
    let digest = fields.next().unwrap();
    assert_eq!(digest.len(), 64);
    assert_eq!(fields.next().unwrap(), "tidyshot-9.8.7-x86_64.tar.gz");
    let actual = Command::new("sha256sum").arg(&archive).output().unwrap();
    assert_eq!(
        String::from_utf8(actual.stdout)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap(),
        digest
    );

    let listing = Command::new("tar")
        .args(["-tzf"])
        .arg(&archive)
        .output()
        .unwrap();
    let listing = String::from_utf8(listing.stdout).unwrap();
    for member in [
        ".tidyshot-source",
        "./build/tidyshot",
        "./build/generate-icon",
    ] {
        assert!(
            listing.contains(member),
            "archive lacks {member}: {listing}"
        );
    }
    let extracted = base.join("extracted");
    fs::create_dir_all(&extracted).unwrap();
    assert!(
        Command::new("tar")
            .args(["-xzf"])
            .arg(&archive)
            .arg("-C")
            .arg(&extracted)
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        fs::read_to_string(extracted.join(".tidyshot-source")).unwrap(),
        head
    );
    assert!(extracted.join("build/tidyshot").exists());
    let _ = fs::remove_dir_all(&base);
}
