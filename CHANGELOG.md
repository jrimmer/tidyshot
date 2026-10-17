# Changelog

All notable changes to TidyShot are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-10-16

The first release: a Rust and Qt Quick rebuild of the OmniShot workflow for
Omarchy.

### Added

- Area, window, display, full-desktop, timed, and previous-area screenshots.
- Vertical and horizontal scrolling capture with manual and Auto-Scroll modes.
  Alignment rejects a jump that repeats rows in the wrong place, and captures
  are capped at 120 megapixels.
- Corner previews with copy, save, annotate, drag, pin, and history actions,
  including auto-close and save-on-timeout, restore, and the close-after-drag
  and Alt-to-keep rules.
- Annotation editor with arrows, text, shapes, counters, pencil, smart
  highlighter, blur, pixelation, crop, resize, rotate, backgrounds, and
  spotlight. Undo and redo span every edit.
- Editable `.omnishot` image projects (ZIP with `image.png`, `project.json` and
  the optional `original.png`) and `.omnishot-video` video projects.
- Local OCR and QR recognition with installable tessdata language models, and a
  recognized-text dialog that can copy the result.
- PNG, JPEG, and WebP image export, and H.264 and animated GIF recording.
- Recording with pause/resume, audio tracks, camera overlays, and an editable
  video timeline with split boundaries, cuts, zooms, and motion blur. Exports
  default to software x264, with VAAPI hardware encoding opt-in.
- Recording journal and checkpoint recovery: an unfinished take is leased with a
  lock file, and a crashed take is recovered or discarded without racing a live
  encoder.
- An Omarchy bar widget and plugin manifest, with first-time setup, update, and
  uninstall flows driven from the bar.
- User-level install (`install.sh`) and a plugin setup path (`scripts/setup.sh`)
  that mirror the checkout into `$XDG_DATA_HOME/tidyshot-app` and back up
  configuration before changing it.
- Hyprland bindings and window rules for capture, recording, and preview
  windows.
- A CLI and local `omnishot://` / `cleanshot://` URL API, plus a single-instance
  socket so a running app receives commands.
- Headless `doctor`, `compat`, and `menu` commands that work when the display or
  the running app is broken.
- Offscreen QML tests and a fixed-workload timing harness for the scroll
  stitcher and annotation effects.
- A native Arch package (`pkgbuild/PKGBUILD`, `bin/install`) that installs the
  release binary, desktop entry, MIME definition, auto-scroll helper, and a
  theme-reflective icon generated from the live Omarchy accent.

### Security

- Captures and settings are written atomically, and the user's originals are
  never modified.
- The single-instance socket is user-only.

[0.1.0]: https://github.com/jrimmer/tidyshot/releases/tag/v0.1.0
