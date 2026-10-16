# tidyshot

Screen capture, annotation and recording for Omarchy, rebuilt in Rust with
Qt Quick via cxx-qt. A reimplementation of OmniShot; no affiliation with
CleanShot X.

Developed and tested on **Omarchy 4.0.2 with Hyprland 0.56.2**.

Status: pilot scaffold. `bin/build` builds, `bin/test` runs the suite
offscreen. Crates are vendored under `vendor/`; builds run offline.

## Omarchy plugin

The repository root is a complete Omarchy plugin: `manifest.json` declares it
as `io.github.joshdaws.tidyshot`, with `plugin/BarWidget.qml` as the bar widget
entry point. Installation mirrors OmniShot's plugin flow:

- `omarchy plugin add <repo> --enable` clones the checkout into
  `$XDG_CONFIG_HOME/omarchy/plugins/io.github.joshdaws.tidyshot`.
- The widget offers **Finish setup**, which runs `scripts/setup.sh`: it
  installs packages through `omarchy pkg add`, mirrors the checkout into
  `$XDG_DATA_HOME/tidyshot-app`, builds the Rust binary there, and stamps
  `.tidyshot-source` with the plugin commit.
- A later `omarchy plugin update` moves HEAD; the widget offers **Apply update**
  when the checkout's commit no longer matches the stamp. Setup mirrors the
  checkout again, keeping build outputs (`target/`, `build/`, `native/`,
  `.venv/`, `backups/`) that the plugin validator must not see inside the
  plugin folder.
- **Uninstall TidyShot** runs `scripts/uninstall.sh`, which removes the
  launcher, Hyprland bindings and window rules, desktop integration, the built
  app files and the bar widget. Captures and settings are kept.

A development checkout uses `bash install.sh` instead: it builds with
`bin/build`, installs the launcher and desktop files with
`scripts/install_user.py`, and registers the bar widget as a plain copy.

The installer writes configuration backups under `backups/<timestamp>/` and
refuses to touch an app folder it did not create. The repository tracks no
symlinks, as `omarchy plugin validate` requires.

## Tests

`bin/test` runs everything offscreen. `tests/plugin_manifest.rs` checks the
plugin contract, `tests/setup_scripts.rs` drives `setup.sh`/`uninstall.sh`
against a throwaway HOME with stubbed desktop commands, and
`tests/install_user.rs` covers the user-level installer in an isolated home.
`tests/editor_port.rs` drives the annotate editor document, project file,
clipboard and renderer.

## Annotate editor

The editor is ported from OmniShot's `editor.py`, `annotation_*.py`, `crop.py`,
`backgrounds.py`, `spotlight.py` and `text_styles.py`. The model lives in
`src/annotation.rs`; raster drawing in `src/raster.rs`; rendering in
`src/render.rs`; document, undo and canvas edits in `src/editor.rs`; the
`.omnishot` archive in `src/project.rs`. Projects are ZIP archives holding
`image.png`, `project.json` and the optional `original.png`, so version-1
projects without an original still open. PNG and ZIP are written and read by
`src/pngzip.rs` on top of the vendored-free codec in `src/compress.rs`.

## License

MIT.
