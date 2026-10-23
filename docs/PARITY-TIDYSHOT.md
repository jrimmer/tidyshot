# TidyShot parity audit

Compare: OmniShot `docs/PARITY.md` and `docs/USAGE.md` on `main` and
`pr/14`.. `pr/17`, against TidyShot `src/`, `qml/`, `plugin/`, `scripts/` and
`tests/` at `work/parity-audit`. This is a read-only audit; no source changed.

## How a row is judged

Runtime reachability decides the status, not the presence of a module or a
unit test. A path counts as **working** only when a production entry point
reaches it and a user action can start it. A module that only tests, benches
or `lib.rs` touch is **test only**. A component that exists but is never
connected is **unwired**.

Production roots used for the call-graph check:

- `src/main.rs` → `tidyshot::run` → `src/lib.rs` (`run_with`, socket server)
- `src/lib.rs` → `app::route_and_resolve`, `capture_runtime::dispatch`
- QML singletons compiled into the module: `Menu` (`src/menu.rs`),
  `Capture` (`src/capture_runtime.rs`), `TextOutput` (`src/text_result.rs`),
  `Args` (`src/bridge.rs`), loaded from `qml/Main.qml` by
  `src/lib.rs::run_with`.

Everything else has to be reached from those. A function with no caller in
that subgraph is recorded as test only, however complete it looks. The audit
did not run the GUI; QML facts are read from `qml/*.qml` and
`plugin/BarWidget.qml`.

## Reachability ledger

`src/lib.rs` declares 68 modules. Only these are on a production path:

| Reached from production | How |
|---|---|
| `main`, `lib` | process entry, Qt startup |
| `cli`, `api`, `app`, `instance` | argument/URL parse, route, single-instance socket |
| `capture`, `capture_session`, `capture_runtime`, `mirror` | capture request, slurp/grim, save, history append |
| `commands` | runner trait and every external program |
| `history`, `settings`, `filenames`, `storage` | capture store, settings read, atomic writes |
| `annotation` (only `png_dimensions`) | PNG size for a captured frame |
| `pngzip`, `compress`, `raster`, `geometry`, `json`, `color` | image decode/encode, canvas compose, parsing |
| `text_result`, `ocr` | `tidyshot ocr <file>`, `capture-text` startup path |
| `menu`, `bridge` | QML action table, startup args |
| `doctor`, `compat` | headless subcommands |
| `icon`, `ui_scale`, `video_decoder` | startup icon, accent parser, preview decoding policy |
| `doctor::*` | `tidyshot doctor` |

Everything below is compiled and tested but **not reachable at runtime**:
`backgrounds`, `clean_capture`, `clipboard`, `crop`, `drag`, `editor`, `effects`, `gif`,
`hover_keys`, `media_import`, `motion_blur`, `overlay`, `pin`, `project`,
`qml_load` (tests only), the whole `recording::*` tree (`audio`,
`checkpoint`, `display`, `encoder`, `gif`, `journal`, `options`, `recorder`,
`recovery`), `render`, `scroll`, `spotlight`, `stitch`, `studio`,
`text_styles`, `timeline`, `timeline_thumbnails`, `toolbar`,
`video_audio_tracks`, `video_camera`, `video_clicks`, `video_cursor`,
`video_history`, `video_keys`, `video_motion`, `video_position`,
`video_preview`, `video_project`, `video_splits`, `video_styles`, `video_ui`,
`zoom_focus`, and `bin/bench` / `bin/generate-icon` (separate binaries).

The recording tree is reached only through `recording::native_dir()`; none of
its journal, checkpoint, recovery, recorder, encoder or export code has a
production caller. Every row in the action table now routes: captures and
recording commands start their runtime, and the remaining rows become command
words `capture_runtime::perform_route` performs (open, clipboard, history,
restore, settings, pin, toggle/close/unlock pins, save-all, close-all, quit).
The menu's pin row has no argument and pins the newest capture; `pin <path>`
carries the chosen file. `clipboard` imports the clipboard file list instead of
opening an empty chooser.
`capture_runtime::dispatch`/`run_app_command` hand every routed `Command` to
`perform_command`, so a command with nothing to act on reports a visible
message instead of being dropped. The reachability list below is otherwise
unchanged; the history window (`history_window`), preferences
(`settings_window`) and pins (`pin_window`) are the new production roots.

## Parity table

Status legend: **Working** = reachable and drivable; **Partial** = a real
sub-path works and the rest is missing; **Test only** = logic exists with no
runtime caller; **Absent** = no implementation.

| Area (OmniShot `PARITY.md`) | Required local behavior | TidyShot status | Runtime evidence |
|---|---|---|---|
| Capture | Area, selected window, display/full desktop, previous area, delay | **Partial** | `capture_runtime::CaptureRuntime` runs `area`, `window`, `fullscreen`, `desktop`, `previous`, `timer`; `desktop` composites per-output frames. Delay runs, but `run_with_cancel` passes `on_tick = |_| {}` and no QML countdown exists, so the timer is silent and only polled for cancel between steps. Cursor mirror is best-effort; freeze is hardcoded `true` in the runtime options. |
| Selection | Crosshair, magnifier, freeze, exact size, aspect ratio, cancel | **Absent** | Selection is slurp (`capture_session::slurp_region`) or nothing. No QML selection surface, magnifier, size/aspect fields, live-vs-frozen switching, or keyboard adjust. Escape in `qml/Main.qml` only cancels a busy capture flag; it does not close a selector. |
| Scrolling | Select region → Start → manual/auto scroll → Done → preview | **Working** | `capture_runtime` runs `CaptureOutcome::Scroll` through `scroll_runtime::drive`: slurp selects the region, `qml/ScrollControls.qml` binds Start, Auto-Scroll, Done, Cancel and the frame count from the `Capture` singleton properties, `SystemScrollCapture` feeds grim PPM frames to the stitcher, and Done saves the composite PNG, its history row (`kind: scroll`) and the preview. `tidyshot scroll` runs the same session headlessly. Stop conditions: four unchanged frames, repeated rows (`Could not align`), the 120 MP cap and cancel. `scroll-done` finishes the open session; before any frame has landed it cancels instead of ending the session on a failed finish. A headless capture without the scroll helper still saves its first viewport and reports that auto-scroll is off, and a frame error that stops a capture early is reported with the saved image. |
| Scrolling directions | Vertical and horizontal, reverse frames, alignment failure | **Working** | `scroll-horizontal` and the controls' horizontal label drive the same session with transposed frames; `stitch` and `scroll` tests still cover reverse capture, repeated-row rejection and the 120 MP cap. |
| Preview | Corner thumbnail, annotate, copy, save, drag, pin, dismiss/restore | **Partial** | `qml/Overlay.qml` is instantiated in `qml/Main.qml` and shows the saved file, dimensions, collapse/restore and the auto-close timer; only `onDismissed: Capture.clear()` is connected. `copyRequested`, `saveRequested`, `pinRequested`, `activate`, `trashRequested`, `saveAllRequested`, `closeAllRequested` have no handler, so annotate/copy/save/pin/drag/trash do nothing. `src/overlay.rs` (gestures, auto-close, corners, drag rules) is test only. |
| Overlay preferences | Position, size, autoclose, monitors, gestures, hide | **Test only** | `overlay.rs` implements corner placement, swipe thresholds, auto-close and close-after-drag with 11 tests; no settings are read and no window is positioned by it. `Main.qml` hardcodes a bottom-left overlay. |
| Annotation | Arrow (4 styles), line, ellipse, rectangle, filled rectangle | **Partial** | `qml/Editor.qml` and `qml/EditorToolbar.qml` drive arrow, line, rectangle and ellipse drags through `editor_ui::EditorInteraction` into `render.rs`; the toolbar exposes those plus text, counter, pencil, highlighter, blur, pixelate and crop. Arrow styles, filled rectangle and spotlight remain reachable only from the model and tests, and there is no bundled font or resize handles yet. |
| Annotation | Text (7 styles), counters, smooth pencil, smart highlighter | **Partial** | The editor enters text through an inline box and commits one annotation; counters number in drag order, pencil stores smoothed normalized points and highlighter draws a translucent band. Only the plain text style is offered, `editor::smart_highlight` still has no caller, and no font is bundled. |
| Annotation | Blur, pixelate, secure concealment, spotlight | **Partial** | Blur and pixelate are reachable from the editor toolbar and render through `effects.rs` over the base image. Secure blur and spotlight remain model/test only. |
| Editing | Select/move/resize/delete, palette/picker, undo/redo | **Partial** | The editor selects the topmost annotation, moves it as one undo step, deletes it, arms a swatch palette and undo/redoes with Ctrl+Z / Ctrl+Shift+Z. Resize handles, free rotation and the full color picker are still missing. |
| Image operations | Crop/aspect/snap, resize, rotate/flip, combine/drop | **Partial** | The editor draws a provisional crop and applies or cancels it; a cancel leaves the document untouched. `editor.rs` resize/rotate/flip/combine and `crop.rs` aspect/snap still have no UI, and there is no drop handling. |
| Editable files | Reopen editable projects; original plus movable annotations | **Working** | `tidyshot open <file>` (and a path-less `open`, which shows a file chooser), the preview's Annotate button, annotate links and `--action annotate` all open `qml/Editor.qml`; a `.omnishot` path saves the project, any other path flattens to PNG, and Ctrl+S writes atomically. Covered by `tests/editor_window.rs` and `tests/link.rs`. |
| Background | Presets, gradients/images, padding, alignment, auto balance, aspect/shadow/rounding | **Test only** | `backgrounds.rs` renders presets/padding/radius/shadow and the editor renders them when a document carries one, but no UI adds or edits a background and no preset is read from settings. |
| Window capture | Editable background, transparency, shadows; isolated RGBA capture | **Partial** | `window` works as a rectangle grab: `openWindowPicker` lists `hyprctl -j clients`, `captureWindow` passes the client rect to slurp/grim. It is not an isolated RGBA window image: no transparency, wallpaper, shadow, rounding, Tab cycling, or covered-window handling. `WindowInfo` ignores title identity beyond display. |
| Pin | Above windows, resize/opacity, arrows, click-through, hide | **Working** | `qml/PinWindow.qml` is an always-on-top frameless window per registry row with the shadow and rounded frame from `pin.rs`, hover controls for opacity, lock/unlock, close and click-through, drag anywhere, edge resize, middle-click close and Escape to hide the controls. `src/pin_window.rs` owns add/remove/list, the `pins.json` listing (position, size, opacity, click-through, path), restore on startup, hide/show, close all and unlock all; the preview's Pin action and the `pin`, `toggle-pins`, `close-pins` and `unlock-pins` commands reach it. Pin titles are excluded from the picker and isolated window capture. Covered by `tests/pin_window.rs` and `qml/tests/tst_pin_window.qml`. |
| OCR | Local text extraction, languages, line breaks, QR | **Working** | `TextOutput` drives `tidyshot ocr <file>` and `capture-text`/`ocr-lines`/`ocr-single-line` over `tesseract` + `zbarimg`, with the recognized-text dialog and explicit Open Link. The menu OCR row, a forwarded `Command::Ocr` and a forwarded `capture-text` capture all reach `text_result::request_recognition`, which queues until the dialog binds, so every invocation runs the same completion; `tidyshot ocr` (no file) selects with slurp, grabs, then recognizes. The OCR languages and line-break defaults are editable in the preferences window. |
| History | Restore, filters, search, delete, retention up to a month | **Working** | `qml/HistoryWindow.qml` renders the newest-first filmstrip from `history_window::HistoryView`, with all/image/scroll/video tabs, name search and single or multi selection. Delete and Clear History move captures into `trash/` (rename, or temp file plus rename across filesystems) and drop their rows; Restore preview, Annotate and Pin act on the selection; the window activates on open and a focus scope over the content carries the keys, so Enter opens, arrows or h/j/k/l move, Delete trashes and Escape closes. `history`, `restore`, `last`, `last-screenshot` are routed to their handlers, and `history_retention_days` (default a month) drives a startup cleanup that trashes older captures. |
| Files | PNG/JPEG/WebP/HEIC, clipboard, drag/drop, external app, multipage print | **Absent** | Only PNG is written (`pngzip`). No JPEG/WebP/HEIC encoder, no print, no image clipboard or drag-out (`clipboard.rs` and `drag.rs` are test only; `wl-copy` is used for recognized text only), no external-app launch path. |
| Recording | Area/window/display, H.264 MP4/GIF, FPS/quality/resolution | **Partial** | `recording_runtime::RecordingRuntime` plans a take from `hyprctl -j monitors` (an explicit `--geometry` region or the display under the pointer), leases it with `recording::journal::Journal`, launches `recording::recorder::recorder_command` through the command runner, and commits the MP4 or GIF with its sidecar, history row and preview. `record`, `stop`, `pause` and `restart` reach it from the menu, CLI and URL API. FPS/quality/size/audio/cursor/Studio/GIF choices come from settings; there is still no recording setup dialog. |
| Audio/camera | Mic/system audio, meter, camera shape/size/position/fullscreen | **Test only** | `recording::audio`, `video_audio_tracks`, `video_camera`, `video_position`, `camera` logic exists with tests; no capture path and no meter/camera preview window. |
| Video audio edits | Independent source inclusion/gain, mono, playback/export | **Test only** | `video_audio_tracks` and `studio` audio filter planning are tested in `tests/video_studio.rs`; nothing calls them in the app. |
| Recorder controls | Countdown, timer, pause/resume/restart/cancel, hidden controls | **Partial** | `qml/RecordingControls.qml` is an always-on-top strip with elapsed time, Pause/Resume, Stop, Restart and Cancel, fed by the `Recording` singleton; a Hyprland rule marks the strip `no_screen_share` for compositors that exclude it. The runtime writes `$XDG_RUNTIME_DIR/tidyshot-status.json` (`recording`, `paused`, `seconds`, `show_time`, `updated`) while a take runs, so the bar widget activates its timer and Stop/Pause rows. A recording setup dialog and countdown are still missing. |
| Presentation | Cursor/click effects, keystrokes, DND, clean desktop | **Test only** | `studio` paints cursor/clicks/keys, `video_clicks`/`video_keys`/`video_cursor` model events, `motion_blur` blurs; no recording runtime consumes them. |
| Video preview | Full-screen composition, transport, seek, keyboard return | **Test only** | `video_preview` and `video_ui` build transport/placement models with tests; no video window is opened. |
| Video timeline | Source thumbnails, zoom, scroll, edit gestures | **Test only** | `timeline`, `timeline_thumbnails`, `video_splits`, `zoom_focus` are covered by tests and `tests/video_studio.rs`; no timeline UI. |
| Video editor (5.0) | Trim/cut, smart zoom, cursor smoothing/style/size, camera/key editing | **Test only** | `studio::smart_zooms`, `video_motion`, `video_styles`, `video_camera` and `video_splits` have tests only. `video_motion` is the only cross-module caller, itself unreachable. |
| Video editor (5.0) | Background, aspect, motion blur, hardware export | **Test only** | `studio::export_studio` is called only by `tests/video_studio.rs` (real ffmpeg export test); software x264 default and VAAPI opt-in are implemented there. No editor invokes it. |
| Integration | Omarchy shell plugin, launcher, configurable shortcuts, local command API | **Partial** | The plugin (`manifest.json`, `plugin/BarWidget.qml`), setup/update/uninstall scripts, desktop entry, Hyprland bindings/window rules and the single-instance CLI/URL API all exist and are tested. The recording runtime writes `$XDG_RUNTIME_DIR/tidyshot-status.json` while a take runs, so the widget's recording timer and Stop/Pause rows are live during a take; most other menu rows are still inert, and only capture and recording commands do anything after routing. `doctor`/`compat` work headless. |
| Preferences | Persistent settings/shortcuts, active Omarchy theme | **Partial** | `qml/SettingsWindow.qml` edits the retention, OCR and decoder keys through `settings_window::SettingsView`, which validates each value and writes it atomically with `settings::write_setting` while keeping every other key. The `settings` command, URL tabs and menu row open it. There is still no shortcut editor and no theme picker; theme accent is read only for the app icon, and `ui_scale` reflow is test only. |

## Upstream branches

| Branch | Change | TidyShot state |
|---|---|---|
| `pr/14` | Clean mirror degrades instead of blocking when Hyprland cannot hook | **Working**: `mirror.rs` caches an unavailable plugin, `capture_session` uses `start_best_effort`, and the aarch64 "Could not hook" case degrades in `MirrorRegistry::acquire`. `clean_capture.rs` itself is test only. |
| `pr/15` | Software video preview decoding by default, hardware opt-in off Intel, explicit env wins | **Working**: `video_decoder::configure_preview_decoder` runs from `src/main.rs` before Qt, driven by `settings::stored_settings`. Covered by `tests/video_studio.rs`. |
| `pr/16` | Install as an Omarchy plugin, widget setup/update/uninstall, bar state command | **Partial**: manifest, widget, `scripts/setup.sh`, release verification and tests exist; the widget's `state` channel reads `$XDG_RUNTIME_DIR/tidyshot-status.json`, which the recording runtime writes for the duration of a take and removes when it ends. |
| `pr/17` | `omnishot doctor` and header discovery through compiler include paths | **Working**: `doctor`/`compat` are headless subcommands with `tests/doctor.rs`, including session skipping and include-path header lookup. |

## Top-10 prioritized gaps

Ordered by how much of the documented workflow each unlocks. "Reachable"
means a production caller and a UI or CLI entry exist.

1. **Recording has a runtime but no setup dialog.** `recording_runtime` reaches
   `recorder_command`, `Journal`, `CheckpointWriter`, the encoder guard, the
   controls strip and the launch recovery; the missing pieces are a recording
   setup dialog, the countdown, and a Studio/camera capture path. The video
   editor remains test only.
2. **Command routing is complete and history, settings and pins have windows.**
   Every action-table row and every routed `Command` reaches a handler that
   reports its result; the remaining work in those areas is import into
   history, a shortcut editor and the pin gestures listed below.
3. **Scrolling capture is reachable but the UI is minimal.** The controls
   window has Start/Auto-Scroll/Done/Cancel and a frame count, and the CLI runs
   the same session headlessly; there is no in-place selection overlay, no
   live preview image in the controls, and the auto-scroll pointer guard only
   reports when the pointer leaves the area.
4. **Annotation editor has no window.** `Document`, `render`, `project`,
   `crop`, `backgrounds`, `effects`, `spotlight` and `text_styles` are test
   only; no QML editor/toolbar exists, the preview's `activate` signal is
   unconnected, and after-capture `annotate` is ignored. The editing model is
   substantial but unreachable from any user action.
5. **Quick Access preview gestures and trash are display-only.** `overlay.rs`
   is test only. `qml/Main.qml` connects copy, save, annotate, pin, dismiss,
   Save All and Close All, but drag-out, trash and the swipe gestures still do
   nothing, and only the timeout/corner settings are read.
6. **History has a window but no import.** The filmstrip, tabs, search,
   multi-select, trash delete, restore, annotate, pin and retention cleanup
   are reachable; importing an external image or video into history is still
   absent, and `video_history` (the video editor's undo stack) remains test
   only.
7. **Settings have a window but no shortcut editor.** The preferences window
   writes retention, OCR and decoder keys atomically; there is no shortcut
   editor, no theme page, and the bar widget's
   `$XDG_RUNTIME_DIR/tidyshot-status.json` producer still exists only while a
   recording runs, so its recording timer, Stop and Pause rows work during a
   take while the rest of the bar state stays idle.
8. **Window capture is a rectangle, not an isolated window.** The picker uses
   `hyprctl clients` and grim grabs the client rect. Transparency, wallpaper,
   shadow, rounding, Tab cycling, covered/renamed-window identity and editable
   background are missing.
9. **Image export and clipboard/drag are PNG-only and unwired.** JPEG, WebP and
   HEIC export, multipage print, image/file clipboard, drag-out and external
   application launch have no reachable path (`clipboard.rs`, `drag.rs`,
   `gif.rs` are test only; `wl-copy` is only used for OCR text).
10. **Capture selection and countdown UX is missing.** No live/frozen selection
    overlay, magnifier, exact size/aspect fields, keyboard adjustment or
    frozen-frame window hit-testing. `CaptureOptions.freeze` is hardcoded true,
    the countdown produces no UI and no immediate cancel, and after-capture
    action selection (`screenshot_actions`, `capture_actions`) is test only, so
    `--action` and `after_capture` never change what a capture does.

## Verification

- `cargo clippy --offline --all-targets -- -D warnings`: pass.
- `cargo test --offline --lib`: pass, 505 tests.
- `cargo fmt --check`: reports one pre-existing formatting difference in a
  `src/capture_session.rs` test literal; that file is untouched by this audit
  and the difference is present on the branch base.
- No GUI was started; QML behavior was read from the files above.
