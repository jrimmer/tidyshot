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
