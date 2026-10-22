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

Auto-Scroll needs the standalone `native/scroll-helper`, and isolated window
capture needs `native/window-helper`; both are built against the running
compositor's Wayland protocols. GPU recording requires a working
`gpu-screen-recorder` setup.

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
  checkout into `$XDG_CONFIG_HOME/omarchy/plugins/com.jasonrimmer.tidyshot`;
  the directory name is the manifest's plugin id.
- Installs from before the id rename still work: uninstall removes either
  plugin id's checkout and bar widget, and doctor accepts a bar widget that
  answers under the previous id until the plugin is updated.
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

## Settings

The Settings row in the menu, `tidyshot settings` and an
`omnishot://open-settings?tab=…` link all open `qml/SettingsWindow.qml`, a
top-level window over the ten pages the settings schema declares in
`src/settings_page.rs`. Every change is merged into the existing JSON and
committed through the atomic store, and the next capture, preview, recording
or editor reads it back, so it takes effect without a restart (the hardware
video decoding choice is the one restart-only setting, with its tooltip). The
Annotate page's color, stroke, text size, arrow style and highlighter opacity
seed a fresh editor document; the Quick Access page drives the preview's
corner, width, margin, auto-close delay/action and close-after-drag; and the
Recording, Text and Advanced pages feed the recording defaults, OCR and the
video decoder policy.

The Shortcuts page edits the managed `bindings.lua` block. A rewrite copies the
file beside itself first, changes only the managed block and keeps every
unrelated binding and line after it; a user binding that also launches
`tidyshot` is never read as the managed key or captured as the launcher.

## Capture and annotate

`src/capture_runtime.rs` is the one capture path: it reads the live monitors,
runs `capture_session::run_capture`, answers a frozen selection with slurp,
writes the PNG through the atomic store and prepends its history row. The QML
`Capture` singleton calls it from a worker thread, so the menu, the tray menu,
a forwarded socket command and a direct `tidyshot area` all reach the same
grabs, save and history. Saved captures live under `captures/` beside
`history.json` in the data directory, and the corner preview opens the saved
file.

## Window capture

A window capture is not a rectangle grab. `native/window-helper` speaks
Hyprland's `hyprland_toplevel_export_v1` protocol and copies the window's own
surface, so overlap, the wallpaper and other windows never leak in and the
alpha channel survives. `src/window.rs` resolves the window identity
(address plus stable id), runs the helper and parses the PAM RGBA stream;
`src/window_picker.rs` freezes every output, sorts the visible windows by
`focusHistoryID`, hit-tests the pointer and cycles with Tab. One fullscreen
`qml/WindowHover.qml` per output shows the frozen frame, dims it, redraws the
hovered window undimmed and outlines it in the Omarchy accent; Shift keeps the
window's alpha and Escape cancels. When the frames cannot be grabbed, the
keyboard fallback lists the same windows in `qml/WindowPicker.qml`.

The isolated window is saved as the history capture. Unless the capture was
with Shift, `src/window_edit.rs` also composes the editable frame: Hyprland's
`decoration:rounding` and `rounding_power` (superellipse) scaled by the capture
ratio, the `window_padding` setting, the drop shadow, and the desktop
wallpaper from `~/.local/state/omarchy/current/background` (or
`~/.config/omarchy/current/background`) when `window_wallpaper` is on.
`src/wallpaper.rs` reads and decodes that file through ffmpeg without touching
it. The project is written beside the capture under `image-edits/`, so the
preview shows the composed picture while Open Annotate reopens the editable
frame instead of a flattened PNG.

`window-helper` is built by `build.rs` and `bin/native` when `wayland-scanner`
and the `wayland-client` headers are present. Without them the build only
warns, and a window capture reports that the helper must be built instead of
silently falling back to a desktop crop.

A scrolling capture runs `capture_session::run_capture`'s `Scroll` outcome
through `scroll_runtime::drive`: slurp selects the region, `qml/ScrollControls.qml`
drives Start, Auto-Scroll, Done and Cancel over the frame count from the
`Capture` singleton, grim PPM frames feed `stitch::ScrollStitcher`, and Done
saves the composite as an ordinary PNG with a `scroll` history row. The same
session runs headlessly for `tidyshot scroll`, stopping at the end of the
content, at a repeated row, at the 120 megapixel ceiling or on cancel. Without
the auto-scroll helper the headless run still saves its first viewport and says
on stderr that auto-scroll is off; a frame error that stops a capture early is
reported alongside the saved image instead of being dropped.

1. Press **Super+Shift+Print** and select the scrolling content.
2. Click **Start Capture**. Scroll manually or choose **Auto-Scroll**.
3. Click **Done**, then click the corner preview to annotate.
4. Draw, crop, or conceal information, then copy/save/drag the result.

If alignment fails, scroll back slightly or slow down. Captures are limited to
120 megapixels. Save a `.omnishot` project to keep editing later. Projects
contain the source image; share a flattened PNG/JPEG/WebP when concealing
information.

## Recording

`src/recording_runtime.rs` is the one recording path. It plans the take from
`hyprctl -j monitors` (an explicit `--geometry` region or the display under the
pointer), leases it with `recording::journal::Journal`, spawns
`gpu-screen-recorder` through the command runner, and commits the MP4 or GIF
with its sidecar, history row and corner preview. `RecordOptions` honours the
saved FPS, quality, size, audio, cursor, Studio and GIF preferences.

While a take runs the runtime writes `$XDG_RUNTIME_DIR/tidyshot-status.json`
with `recording`, `paused`, `seconds`, `show_time` and `updated`, so the bar
widget shows its timer and its Stop and Pause/Resume rows. The
`qml/RecordingControls.qml` strip is its own always-on-top window with elapsed
time, Pause/Resume, Stop, Restart and Cancel; a Hyprland rule keeps it out of
the recording on compositors that can exclude a window from a screencast.
`tidyshot record`, `stop`, `pause` and `restart` drive the same runtime, from
the command line, a URL or the bar panel.

Pause suspends the encoder's process group and Resume continues it, so no
frames are committed while paused. A take whose encoder dies keeps its journal
marker and Studio checkpoint; the next launch recovers the playable file,
restores the checkpoint, appends its history row and shows it in the preview,
while a cancelled take is discarded.

## Capture history

`qml/HistoryWindow.qml` shows the saved captures as a newest-first horizontal
filmstrip with type tabs (all, images, scroll, video), a name search and single
or multi selection. `src/history_window.rs` owns the listing, the filter and
search, the cursor and every action: Delete and Clear History move captures
into `trash/` beside the store (a rename on one filesystem, a temp file and
rename across filesystems) and drop their rows, never unlinking a capture;
Restore preview, Annotate and Pin act on the selected capture. The window
activates itself on open and its key map lives in a focus scope over the
content, so Enter opens the selection, arrows or h/j/k/l move the cursor,
Delete trashes it and Escape closes the window without a first click. A row
click selects the row it hit rather than moving the cursor by a delta. The
History menu row and `tidyshot history` open it.

`history_retention_days` in `settings.json` sets how long captures are kept;
the default is 30 days, zero keeps everything. A cleanup pass runs on startup
and moves older captures to the trash. The preferences window
(`qml/SettingsWindow.qml`) edits the retention and the OCR and decoder keys the
rest of the app reads, writing them atomically through `src/settings.rs`.

## Command surface

`src/cli.rs` parses the same argument vector the OmniShot launcher uses;
`src/api.rs` parses `omnishot://` and `cleanshot://` URLs; `src/app.rs` routes
both into one command and decides the after-capture action set;
`src/instance.rs` delivers commands to a running instance over a user-only
socket. Headless commands (`doctor`, `compat`, `menu`) run before Qt and before
the socket, so they work when the display or the running app is broken.

## Quick Access preview

After every capture a frameless, always-on-top preview window
(`qml/PreviewWindow.qml`) opens in the bottom-right corner of the focused
monitor. It reuses `qml/Overlay.qml` for its body, so the preview shows the
saved image or the recording's first frame with Copy, Save, Annotate/Edit,
Pin, drag-out and close. Enter copies and Escape closes while the preview is
the active window, so the capture menu keeps its own keyboard handling; the
action results and any I/O failure appear on the window. A window capture
reopens its editable frame when annotating; the pin window is not ported yet,
so that action says so instead of claiming a window opened.

`src/preview.rs` owns the window: it resolves the focused monitor with
`hyprctl -j monitors`, computes the anchor through `src/overlay.rs`, reads the
Quick Access settings (`preview_timeout` or the grouped `quick_access` object,
`preview_action`, `preview_corner`, `preview_margin`, `close_after_drag`) and
performs the
actions through the existing clipboard, storage, annotator and pin modules. A
`tidyshot area` invocation saves, prints the path and then starts the preview
instead of exiting, so the capture can still be acted on; closing that preview
ends the process. `qml/Main.qml` keeps the menu window hidden for a
command-line capture and shows it only for an interactive menu session.

Wayland ignores an application-set toplevel position, so `src/preview_placement.rs`
pins the computed anchor through a Hyprland rule named after the preview title:
`hyprctl keyword windowrule "move X Y,title:^(TidyShot Preview)"`, falling back
to `windowrulev2` on a compositor that still uses that keyword. Each open removes
the previous rule first (`unset,title:^(TidyShot Preview)`), so a changed corner,
margin or focused monitor moves the next preview instead of stacking rules.
`scripts/uninstall_user.py` removes the same rule with the same matcher.

`src/overlay.rs` owns the preview behavior: auto-close and the
save-on-timeout action, dismiss and restore, pin, and drag-out with the
close-after-drag and Alt-to-keep rules. `src/drag.rs` tracks definitive Wayland
drag completion through a `drag-status` preload when one is present, and falls
back to Qt's own action otherwise. `src/ui_scale.rs` reflows UI measurements to
the Omarchy theme without touching captured pixels.

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

`qml/Editor.qml` is the window: the Rust `Editor` singleton owns the document
and every edit, and the canvas is the rendered document served through the
`image://tidyshot/canvas` provider, so there is one renderer. The toolbar
(`qml/EditorToolbar.qml`) arms select, arrow, line, rectangle, ellipse, text,
counter, pencil, highlighter, blur, pixelate and crop; each drag becomes one
document change and one undo step. Ctrl+Z and Ctrl+Shift+Z undo and redo,
Escape drops a provisional crop or a drag without changing the image, Ctrl+S
saves atomically and Ctrl+C copies the rendered result. Saving a path ending
in `.omnishot` keeps the project editable; any other path flattens to PNG and
the copy/export path never modifies the opened file until Save is pressed. A
close with unsaved annotations asks before discarding.

The window opens from the preview's Annotate button, `tidyshot open <file>`
(a path-less `tidyshot open` shows a file chooser), `cleanshot://open-annotate`
links, and after a capture with `--action annotate`. A command-line open starts
only the editor and closing it ends the process. Layout measurements follow the
Omarchy text scale (`Editor.uiScale` comes from `src/ui_scale.rs`), so a theme
change reflows the chrome without resampling the captured pixels.

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
clipboard and renderer; `tests/editor_window.rs` drives the editor window's
open, undo, save, copy, lossy-format export and key paths.
`tests/settings_page.rs` drives the settings writer, the live-effect readers
and the managed shortcut block against throwaway files. `tests/video_studio.rs` drives the video decoder
policy, timeline persistence, motion blur and export planning.
`tests/app_surface.rs` asserts every documented CLI subcommand and URL
parameter still routes, and covers the single-instance socket, UI scale,
preview and clipboard rules. `qml/tests/tst_overlay.qml` drives the corner
preview overlay with synthetic events offscreen, and
`qml/tests/tst_preview_window.qml` drives the frameless preview window, its
key bindings and its action forwarding, and
`qml/tests/tst_editor_toolbar.qml` drives the editor toolbar,
`qml/tests/tst_settings_window.qml` drives the settings pages,
`qml/tests/tst_history_window.qml` drives the history filmstrip, selection,
search and delete signals, `qml/tests/tst_pin_window.qml` drives a pinned
capture, and the overlay and preview suites cover the export menu, Open, Save
All and Close All. `tests/history_commands.rs` asserts every routed command and
menu row reaches a handler, that the clipboard file list opens the file it
names, and that history delete moves a capture to the trash.

## License

MIT.
