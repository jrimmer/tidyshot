# TidyShot

Screen capture, scrolling screenshots, annotation, and screen recording for
Omarchy, rebuilt in Rust with Qt Quick via cxx-qt. TidyShot is a native
reimplementation of OmniShot; it is an independent project with no affiliation
with CleanShot X.

Developed and tested on **Omarchy 4.0.2 with Hyprland 0.56.2**.

TidyShot follows the active Omarchy theme automatically and keeps captures on
your machine. Crates are vendored under `vendor/`; builds run offline.

## What it does

- Area, window, display, full-desktop, timed, and previous-area screenshots.
- Vertical and horizontal scrolling capture, manually or with Auto-Scroll.
- Corner previews with copy, save, annotate, drag, pin, and history actions.
- Arrows, text, shapes, counters, pencil, smart highlighter, blur,
  pixelation, and spotlight.
- Crop, resize, rotate, combine images, and add backgrounds. Save editable
  `.omnishot` projects.
- Local OCR and QR recognition; PNG, JPEG, and WebP image exports.
- H.264 and animated GIF recording, pause/resume, audio, camera overlays, and
  an editable video timeline with cuts and zooms.
- An Omarchy bar widget, configurable shortcuts, and a local CLI/URL API.

No accounts, telemetry, uploads, or cloud syncing. The interface follows
Omarchy's palette; there is no separate light/dark switch.

## Compatibility

Developed and tested on **Omarchy 4.0.2 with Hyprland 0.56.2**, including
fractional scale 1.6 and mixed-scale displays. This release requires Omarchy's
Lua-based Hyprland configuration and shell plugin API. Other Hyprland versions
and distributions are not currently supported.

Auto-Scroll needs the standalone `native/scroll-helper`, which is built
against the running compositor's Wayland virtual-pointer protocol. GPU
recording requires a working `gpu-screen-recorder` setup.

## Install

Run these commands as your normal desktop user in a terminal inside an
unlocked, supported Omarchy session. `omarchy pkg add` handles elevation for
system packages; run `bash install.sh` **without sudo**.

The supported install is the Omarchy plugin:

```sh
omarchy plugin add https://github.com/jrimmer/tidyshot --enable
```

The bar widget then offers **Finish setup**, which installs the build and
runtime dependencies and builds TidyShot from the plugin checkout.

A development checkout installs directly:

```sh
# Install missing build and runtime dependencies.
omarchy pkg add git rust qt6-base qt6-declarative python gcc pkgconf \
  wayland wayland-protocols cairo libxkbcommon libglvnd lua54 grim slurp \
  wl-clipboard ffmpeg tesseract tesseract-data-eng tesseract-data-osd zbar \
  gpu-screen-recorder libpulse desktop-file-utils shared-mime-info xdg-utils \
  libnotify

git clone https://github.com/jrimmer/tidyshot "${XDG_DATA_HOME:-$HOME/.local/share}/tidyshot-app"
cd "${XDG_DATA_HOME:-$HOME/.local/share}/tidyshot-app"
bash install.sh
tidyshot menu
```

`install.sh` refuses to run as root or outside a Hyprland session, validates the
toolchain, builds the app with `bin/build`, and installs its launcher, desktop
and MIME entries, Hyprland bindings and window rules, and bar widget. It backs
up existing files under `backups/<timestamp>/` and does not edit
`/usr/share/omarchy/`.

A development checkout uses `bash install.sh` directly. A plugin checkout
installed with `omarchy plugin add` is built and updated from the bar widget;
see [Omarchy plugin](#omarchy-plugin).

On Arch, `bash bin/install` builds the same release binary and installs it as a
native package through `makepkg -fsi`: the binary lands in `/usr/bin/tidyshot`,
the desktop entry in `/usr/share/applications`, the MIME definition in
`/usr/share/mime/packages`, the icon in
`/usr/share/icons/hicolor/scalable/apps`, and the auto-scroll helper in
`/usr/lib/tidyshot/native`. `bin/install` refuses to run as root, as makepkg
requires.

The app icon follows the active Omarchy accent. `packaging/tidyshot.svg` is
the accent-neutral fallback; `generate-icon` reads the accent through the same
theme parser the UI scale uses and rewrites the SVG, and the user installer
registers that command as an Omarchy `theme-set` hook so the icon follows a
theme change without polling.

The launcher is `~/.local/bin/tidyshot`. Capture history, projects and settings
live under `$XDG_DATA_HOME/tidyshot` (normally `~/.local/share/tidyshot`) and
are separate from the checkout. The binary is built at
`<checkout>/build/tidyshot`.

### Update and remove

To update, finish any capture or recording, then:

```sh
tidyshot quit
cd "${XDG_DATA_HOME:-$HOME/.local/share}/tidyshot-app"
git pull --ff-only
bash install.sh
tidyshot menu
```

Use your actual checkout path if different. Removal is `bash scripts/uninstall.sh`
after confirming; it removes the launcher, desktop integration, Hyprland
bindings and rules, the built app files and the bar widget. Captures, projects
and settings are kept.

## Omarchy plugin

The repository root is a complete Omarchy plugin: `manifest.json` declares it
as `com.jasonrimmer.tidyshot`, with `plugin/BarWidget.qml` as the bar widget
entry point. Install it with:

```sh
omarchy plugin add https://github.com/jrimmer/tidyshot --enable
```

Each install step mirrors OmniShot's plugin flow:

- `omarchy plugin add https://github.com/jrimmer/tidyshot --enable` clones the
  checkout into `$XDG_CONFIG_HOME/omarchy/plugins/com.jasonrimmer.tidyshot`.
- The widget offers **Finish setup**, which runs `scripts/setup.sh`: it installs
  packages through `omarchy pkg add`, mirrors the checkout into
  `$XDG_DATA_HOME/tidyshot-app`, installs the Rust binary there, and stamps
  `.tidyshot-source` with the plugin commit. It first tries the prebuilt
  release for this commit and falls back to building from source when the
  release is missing, its sha256 does not match, the host is not x86_64, or the
  checkout is not the released commit.
- A later `omarchy plugin update` moves HEAD; the widget offers **Apply update**
  when the checkout's commit no longer matches the stamp. Setup mirrors the
  checkout again, keeping build outputs (`target/`, `build/`, `native/`,
  `.venv/`, `backups/`) that the plugin validator must not see inside the
  plugin folder.
- **Uninstall TidyShot** runs `scripts/uninstall.sh`, which removes the
  launcher, Hyprland bindings and window rules, desktop integration, the built
  app files and the bar widget. Captures and settings are kept.

The installer writes configuration backups under `backups/<timestamp>/` and
refuses to touch an app folder it did not create. The repository tracks no
symlinks, as `omarchy plugin validate` requires.

### Releases

A tag publishes a prebuilt binary so setup does not need the Rust and Qt
build toolchain. `bash bin/make-release-artifacts` builds the app, packs
`build/tidyshot`, the icon command, the generated icon and the source commit
stamp into `dist/tidyshot-<version>-x86_64.tar.gz`, and writes
`dist/checksums.txt`. The version comes from `manifest.json`. Upload both files
to the matching `v<version>` GitHub release; publishing is deliberately not part
of the script. The auto-scroll helper is never shipped:
install.sh builds it against the running compositor's Wayland protocol.

Setup only trusts a download whose checksum matches `checksums.txt`, whose
source stamp equals the checkout HEAD, and whose archive holds only plain files
and directories at relative paths: absolute or parent-climbing members are
rejected, as are links, devices, fifos and setuid/setgid members. Anything else
prints one line naming the reason and builds from source as before.

`TIDYSHOT_RELEASE_DIR` points setup at a local directory of release files
instead of downloading them, and `TIDYSHOT_RELEASE_URL` replaces the download
root (`https://github.com/jrimmer/tidyshot/releases/download` by default); the
files are then read from `$TIDYSHOT_RELEASE_URL/v<version>/`. Both leave every
check above in place.

## Bar popup

Click the TidyShot camera icon to open a panel anchored to the bar, following
the same Omarchy UI as Display and Tailscale. Click the icon again, click
outside, or press **Esc** to dismiss it. Use arrows or **h/j/k/l** to select an
action, **Enter** to run it, and **Tab** to switch to a neighboring bar panel.
**Print** and `tidyshot menu` open this same panel on the focused display.

During recording, the icon shows recording status and the panel includes
**Stop** and **Pause/Resume**. The separate system-tray icon is hidden while
the bar panel is available; if the widget is removed or the shell is
unavailable, TidyShot falls back to its tray icon and standalone capture menu.

## Shortcuts

| Shortcut | Action |
|---|---|
| Print | Capture menu |
| Ctrl+Print | Area capture |
| Shift+Print | Focused display |
| Super+Shift+Print | Scrolling capture |
| Alt+Print | Screen recording |
| Super+Ctrl+Print | Capture text |

Super+Print retains Omarchy's color picker. Change shortcuts in Settings.
`tidyshot menu` lists the full CLI surface.

## Capture and annotate

1. Press **Super+Shift+Print** and select the scrolling content.
2. Click **Start Capture**. Scroll manually or choose **Auto-Scroll**.
3. Click **Done**, then click the corner preview to annotate.
4. Draw, crop, or conceal information, then copy/save/drag the result.

If alignment fails, scroll back slightly or slow down. Captures are limited to
120 megapixels. Save a `.omnishot` project to keep editing later. Projects
contain the source image; share a flattened PNG/JPEG/WebP when concealing
information.

## Command surface

`src/cli.rs` parses the same argument vector the OmniShot launcher uses;
`src/api.rs` parses `omnishot://` and `cleanshot://` URLs; `src/app.rs` routes
both into one command and decides the after-capture action set;
`src/instance.rs` delivers commands to a running instance over a user-only
socket. Headless commands (`doctor`, `compat`, `menu`) run before Qt and before
the socket, so they work when the display or the running app is broken.

## Quick Access preview

The corner preview overlay is `qml/Overlay.qml` for layout and `src/overlay.rs`
for behavior: auto-close and the save-on-timeout action, dismiss and restore,
pin, and drag-out with the close-after-drag and Alt-to-keep rules. `src/drag.rs`
tracks definitive Wayland drag completion through a `drag-status` preload when
one is present, and falls back to Qt's own action otherwise. `src/ui_scale.rs`
reflows UI measurements to the Omarchy theme without touching captured pixels.

`cargo run --release --bin bench [reps]` is a fixed-workload timing harness for
the scroll stitcher and the annotation effects. It is not a gate; the unit
tests assert behaviour. The workloads are deterministic synthetic pages and
real decoded fixtures, so the numbers are comparable across revisions. The
OmniShot Python baseline is not measured: `omnishot/stitch.py` needs `cv2` and
`numpy`, which the lease does not have.

## Annotate editor

The editor is ported from OmniShot's `editor.py`, `annotation_*.py`, `crop.py`,
`backgrounds.py`, `spotlight.py` and `text_styles.py`. The model lives in
`src/annotation.rs`; raster drawing in `src/raster.rs`; rendering in
`src/render.rs`; document, undo and canvas edits in `src/editor.rs`; the
`.omnishot` archive in `src/project.rs`. Projects are ZIP archives holding
`image.png`, `project.json` and the optional `original.png`, so version-1
projects without an original still open. PNG and ZIP are written and read by
`src/pngzip.rs` on top of the vendored-free codec in `src/compress.rs`.

## Video studio

The recording editor is ported from OmniShot's `video_*.py`, `studio.py`,
`timeline.py`, `zoom_*.py`, `motion_blur.py` and `camera_framing.py`. The model
lives in `src/timeline.rs`, `src/video_splits.rs` and `src/studio.rs`; the
composition and export pipeline in `src/studio.rs` runs media through the
`ffmpeg` command line only, never libav. Timeline split boundaries, cuts and
zooms persist through the `.omnishot-video` project (`src/video_project.rs`),
and exports default to software x264 with VAAPI hardware encoding opt-in.

Preview decoding follows the PR15 policy in `src/video_decoder.rs`:
software-decode by default on every GPU, Intel always software because of the
VAAPI seek crash, hardware only when the Advanced setting is enabled off Intel,
and a `QT_FFMPEG_DECODING_HW_DEVICE_TYPES` value set before launch wins. The
setting applies after TidyShot restarts.

## Tests

`bin/test` runs everything offscreen. `tests/plugin_manifest.rs` checks the
plugin contract, `tests/setup_scripts.rs` drives `setup.sh`/`uninstall.sh`
against a throwaway HOME with stubbed desktop commands, and
`tests/install_user.rs` covers the user-level installer in an isolated home.
`tests/editor_port.rs` drives the annotate editor document, project file,
clipboard and renderer. `tests/video_studio.rs` drives the video decoder
policy, timeline persistence, motion blur and export planning.
`tests/app_surface.rs` asserts every documented CLI subcommand and URL
parameter still routes, and covers the single-instance socket, UI scale,
preview and clipboard rules. `qml/tests/tst_overlay.qml` drives the corner
preview overlay with synthetic events offscreen.

## License

MIT.
