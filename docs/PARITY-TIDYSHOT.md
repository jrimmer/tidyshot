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
production caller. `src/menu.rs` marks 14 of 24 action rows
`Invocation::NotImplemented`, and `capture_runtime::dispatch`/
`run_app_command` perform only `Command::Capture`, so every routed command
outside capture (history, settings, pin, open, save-all, record, stop, ...)
is dropped after routing.

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
| Annotation | Arrow (4 styles), line, ellipse, rectangle, filled rectangle | **Test only** | `annotation.rs`, `render.rs` and `raster.rs` implement the shapes, handles and arrow geometry with tests. Only `annotation::png_dimensions` is reached at runtime; no editor window or QML toolbar exists. |
| Annotation | Text (7 styles), counters, smooth pencil, smart highlighter | **Test only** | `text_styles.rs` and the text/render paths in `annotation.rs`/`render.rs` are test only; `editor::smart_highlight` has no caller outside tests. No bundled Nunito fonts (OmniShot ships `data/fonts`; TidyShot does not). |
| Annotation | Blur, pixelate, secure concealment, spotlight | **Test only** | `effects.rs` (blur/pixelate) and `spotlight.rs` are exercised by unit tests and `bin/bench`; `render.rs` applies them only in the unreachable editor pipeline. |
| Editing | Select/move/resize/delete, palette/picker, undo/redo | **Test only** | `editor::Document` (undo/redo, crop, resize, rotate, flip, combine, nudge, duplicate, snap) has callers only in `tests/editor_port.rs`, `overlay.rs` and `crop.rs`, none reachable. `color.rs` is reachable only for theme/icon parsing. No color picker UI. |
| Image operations | Crop/aspect/snap, resize, rotate/flip, combine/drop | **Test only** | `crop.rs`, `editor.rs` transforms and `backgrounds.rs` combine are test only; no drop handling or file-open editor in production. |
| Editable files | Reopen editable projects; original plus movable annotations | **Test only** | `project.rs` ZIP (`image.png`, `project.json`, `original.png`) is covered by `tests/editor_port.rs` and `tests/e2e_binary.rs`; `Command::Edit` is routed but `dispatch` drops it, so no project ever opens. |
| Background | Presets, gradients/images, padding, alignment, auto balance, aspect/shadow/rounding | **Test only** | `backgrounds.rs` renders presets/padding/radius/shadow; `tests/editor_port.rs` asserts them. Reachable only from the unreachable editor. |
| Window capture | Editable background, transparency, shadows; isolated RGBA capture | **Partial** | `window` works as a rectangle grab: `openWindowPicker` lists `hyprctl -j clients`, `captureWindow` passes the client rect to slurp/grim. It is not an isolated RGBA window image: no transparency, wallpaper, shadow, rounding, Tab cycling, or covered-window handling. `WindowInfo` ignores title identity beyond display. |
| Pin | Above windows, resize/opacity, arrows, click-through, hide | **Test only** | `pin.rs` (wheel resize/opacity, controls, shadow, clip radius) has tests only. `qml/Overlay.qml` has a Pin button whose signal has no handler; `pin`, `toggle-pins`, `close-pins`, `unlock-pins` are routed and then dropped. Hyprland pin rules are installed by `scripts/install_user.py` for windows the app never opens. |
| OCR | Local text extraction, languages, line breaks, QR | **Partial** | `TextOutput` drives `tidyshot ocr <file>` and startup `capture-text`/`ocr-lines`/`ocr-single-line` over `tesseract` + `zbarimg`, with the recognized-text dialog and explicit Open Link. `tidyshot ocr` (no file) selects with slurp, grabs, then recognizes (`capture_then_recognize`). Gaps: the menu OCR row does nothing (`capture_row("ocr") → false`), a forwarded `Command::Ocr` is dropped (only the first process notes startup OCR), and there is no language-selection or preference UI. |
| History | Restore, filters, search, delete, retention up to a month | **Partial** | Captures are appended to `history.json` and `captures/` by `capture_runtime`; `last_area` feeds `previous`. No history window, filmstrip, restore, search, filter, delete, trash, retention or import. `history`, `restore`, `last`, `last-screenshot` route then drop. |
| Files | PNG/JPEG/WebP/HEIC, clipboard, drag/drop, external app, multipage print | **Absent** | Only PNG is written (`pngzip`). No JPEG/WebP/HEIC encoder, no print, no image clipboard or drag-out (`clipboard.rs` and `drag.rs` are test only; `wl-copy` is used for recognized text only), no external-app launch path. |
| Recording | Area/window/display, H.264 MP4/GIF, FPS/quality/resolution | **Test only** | The full recorder lives in `recording::recorder` (`recorder_command`, `gpu-screen-recorder`, clean capture), `recording::gif`, `recording::options` and `recording::display`; none has a production caller. `record`, `stop`, `pause`, `restart` are routed and dropped. No recording setup UI. |
| Audio/camera | Mic/system audio, meter, camera shape/size/position/fullscreen | **Test only** | `recording::audio`, `video_audio_tracks`, `video_camera`, `video_position`, `camera` logic exists with tests; no capture path and no meter/camera preview window. |
| Video audio edits | Independent source inclusion/gain, mono, playback/export | **Test only** | `video_audio_tracks` and `studio` audio filter planning are tested in `tests/video_studio.rs`; nothing calls them in the app. |
| Recorder controls | Countdown, timer, pause/resume/restart/cancel, hidden controls | **Absent** | No recording window or controls. The bar widget expects `$XDG_RUNTIME_DIR/tidyshot-status.json` for its recording timer, Stop and Pause/Resume rows, and the app never writes that file (grep finds only the widget reader), so the widget always looks idle. |
| Presentation | Cursor/click effects, keystrokes, DND, clean desktop | **Test only** | `studio` paints cursor/clicks/keys, `video_clicks`/`video_keys`/`video_cursor` model events, `motion_blur` blurs; no recording runtime consumes them. |
| Video preview | Full-screen composition, transport, seek, keyboard return | **Test only** | `video_preview` and `video_ui` build transport/placement models with tests; no video window is opened. |
| Video timeline | Source thumbnails, zoom, scroll, edit gestures | **Test only** | `timeline`, `timeline_thumbnails`, `video_splits`, `zoom_focus` are covered by tests and `tests/video_studio.rs`; no timeline UI. |
| Video editor (5.0) | Trim/cut, smart zoom, cursor smoothing/style/size, camera/key editing | **Test only** | `studio::smart_zooms`, `video_motion`, `video_styles`, `video_camera` and `video_splits` have tests only. `video_motion` is the only cross-module caller, itself unreachable. |
| Video editor (5.0) | Background, aspect, motion blur, hardware export | **Test only** | `studio::export_studio` is called only by `tests/video_studio.rs` (real ffmpeg export test); software x264 default and VAAPI opt-in are implemented there. No editor invokes it. |
| Integration | Omarchy shell plugin, launcher, configurable shortcuts, local command API | **Partial** | The plugin (`manifest.json`, `plugin/BarWidget.qml`), setup/update/uninstall scripts, desktop entry, Hyprland bindings/window rules and the single-instance CLI/URL API all exist and are tested. But the widget's state channel depends on the missing `tidyshot-status.json`, most menu rows are inert, and only capture commands do anything after routing. `doctor`/`compat` work headless. |
| Preferences | Persistent settings/shortcuts, active Omarchy theme | **Absent** | `settings.rs` only reads `settings.json` (OCR keys, `hardware_video_decoding`); nothing writes it, no settings window, no shortcut editor. `settings` is routed and dropped. Theme accent is read only for the app icon (`ui_scale::accent_from` → `icon::refresh_installed`); `ui_scale` reflow is test only. |

## Upstream branches

| Branch | Change | TidyShot state |
|---|---|---|
| `pr/14` | Clean mirror degrades instead of blocking when Hyprland cannot hook | **Working**: `mirror.rs` caches an unavailable plugin, `capture_session` uses `start_best_effort`, and the aarch64 "Could not hook" case degrades in `MirrorRegistry::acquire`. `clean_capture.rs` itself is test only. |
| `pr/15` | Software video preview decoding by default, hardware opt-in off Intel, explicit env wins | **Working**: `video_decoder::configure_preview_decoder` runs from `src/main.rs` before Qt, driven by `settings::stored_settings`. Covered by `tests/video_studio.rs`. |
| `pr/16` | Install as an Omarchy plugin, widget setup/update/uninstall, bar state command | **Partial**: manifest, widget, `scripts/setup.sh`, release verification and tests exist; the widget's `state` channel has no producer because the app writes no runtime status file. |
| `pr/17` | `omnishot doctor` and header discovery through compiler include paths | **Working**: `doctor`/`compat` are headless subcommands with `tests/doctor.rs`, including session skipping and include-path header lookup. |

## Top-10 prioritized gaps

Ordered by how much of the documented workflow each unlocks. "Reachable"
means a production caller and a UI or CLI entry exist.

1. **Recording has no runtime.** No production caller reaches `recorder_command`,
   `Journal`, `CheckpointWriter`, `recover`, the encoder guard or the export
   planner; `record`/`stop`/`pause`/`restart` are routed then dropped, and there
   is no setup/controls window. Recording is the largest absent user workflow
   and the durability core (journal, checkpoint, recovery) is already ported
   and tested, so wiring it is the highest-value next step.
2. **Non-capture commands are dropped after routing.** `capture_runtime::dispatch`
   and `run_app_command` handle `Command::Capture` only; `Quit` is handled only
   by the socket server. `history`, `settings`, `pin`, `open`, `clipboard`,
   `save-all`, `close-all`, `restore`, `last`, `last-screenshot`,
   `toggle-pins`, `close-pins`, `unlock-pins`, `record`, `pause`, `restart`,
   `stop`, `scroll-done`, `Command::Ocr` and `Command::Overlay` all reach a dead
   end. This also leaves 14 of 24 menu rows inert and makes the CLI/API surface
   look much larger than the behavior behind it.
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
5. **Quick Access preview and pins are display-only.** `overlay.rs` is test
   only. `qml/Main.qml` connects only dismissal, so copy, save, annotate, pin,
   drag-out, trash, Save All, Close All, gestures, auto-close action and
   corner/timeout settings never run. `pin.rs` has no window at all.
6. **History has no UI or management.** Captures append rows correctly, but
   restore, filmstrip, search, type filters, multi-select, delete, Trash,
   retention and import are absent; `history.rs` has no presentation layer.
   Video history (`video_history`) is likewise test only.
7. **Settings are read-only and the bar widget has no state feed.** There is no
   settings writer, no settings pages, no shortcut editor, and no producer for
   `$XDG_RUNTIME_DIR/tidyshot-status.json`, so the widget's recording timer,
   Stop and Pause rows cannot ever become active. Preferences currently affect
   only OCR defaults and video decode policy.
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
