//! Behavior of the pin registry and the windows it coordinates.
//!
//! The pin window's drawing and gestures are covered by
//! `qml/tests/tst_pin_window.qml`; these tests drive the registry and the
//! live coordinator the app and the menu commands reach. Every pin is written
//! under a throwaway data root, never the user's store.

use std::fs;
use std::path::{Path, PathBuf};

use tidyshot::geometry::Rect;
use tidyshot::json::Value;
use tidyshot::pin_window::{self, PinRegistry, PinnedWindow, is_pin_title, pin_title, pins_path};
use tidyshot::window::WindowInfo;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tidyshot-pins-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn png(width: usize, height: usize) -> Vec<u8> {
    tidyshot::pngzip::Image::new(width, height, vec![7u8; width * height * 4]).to_png()
}

fn window(title: &str, class: &str) -> WindowInfo {
    WindowInfo {
        title: title.to_owned(),
        class: class.to_owned(),
        rect: Rect::new(0, 0, 200, 120),
        monitor: "DP-1".to_owned(),
        workspace: "1".to_owned(),
        address: "0x1".to_owned(),
        stable_id: None,
        focus_history: 0,
        pinned: true,
        fullscreen: false,
    }
}

#[test]
fn a_pin_can_be_added_listed_and_removed() {
    let dir = scratch("registry");
    let first = dir.join("first.png");
    let second = dir.join("second.png");
    fs::write(&first, png(400, 200)).unwrap();
    fs::write(&second, png(160, 90)).unwrap();

    let mut pins = PinRegistry::new();
    pins.add(&first, 400, 200, 10.0, 20.0);
    pins.add(&second, 160, 90, 30.0, 40.0);
    let listed: Vec<PathBuf> = pins.entries().iter().map(|pin| pin.path.clone()).collect();
    assert_eq!(listed, vec![first.clone(), second.clone()]);
    assert_eq!(pins.len(), 2);

    assert!(pins.remove(&first));
    assert_eq!(pins.len(), 1);
    assert_eq!(pins.entries()[0].path, second);
    assert!(!pins.remove(&first));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_persistence_round_trip_keeps_position_size_opacity_and_click_through() {
    let dir = scratch("round-trip");
    let path = dir.join("capture.png");
    fs::write(&path, png(400, 200)).unwrap();

    let mut pins = PinRegistry::new();
    pins.add(&path, 400, 200, 120.0, 240.0);
    pins.set_geometry(&path, 130.0, 250.0, 360.0, 180.0);
    pins.set_opacity(&path, 0.6);
    pins.set_click_through(&path, true);
    pins.save_at(&dir).unwrap();

    let text = fs::read_to_string(pins_path(&dir)).unwrap();
    let parsed = tidyshot::json::parse(&text).unwrap();
    let row = parsed.as_array().unwrap()[0].as_object().unwrap();
    assert_eq!(
        row.get("clickThrough"),
        Some(&Value::Bool(true)),
        "the persisted row carries the click-through state"
    );

    let restored = PinRegistry::restore_at(&dir).unwrap();
    let pin = &restored.entries()[0];
    assert_eq!(pin.path, path);
    assert_eq!((pin.x, pin.y), (130.0, 250.0));
    assert_eq!((pin.state.width, pin.state.height), (360.0, 180.0));
    assert!((pin.state.opacity - 0.6).abs() < 1e-9);
    assert!(pin.click_through);
    // A pin restored from disk maps visible, even after a hidden session.
    assert!(pin.visible);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn hiding_showing_and_unlocking_cover_every_pin() {
    let dir = scratch("set");
    let first = dir.join("a.png");
    let second = dir.join("b.png");
    fs::write(&first, png(100, 80)).unwrap();
    fs::write(&second, png(100, 80)).unwrap();
    let mut pins = PinRegistry::new();
    pins.add(&first, 100, 80, 0.0, 0.0);
    pins.add(&second, 100, 80, 0.0, 0.0);
    pins.set_locked(&second, true);

    assert!(pins.toggle());
    assert!(pins.hidden());
    assert!(pins.entries().iter().all(|pin| !pin.visible));
    assert!(!pins.toggle());
    assert!(pins.entries().iter().all(|pin| pin.visible));

    pins.unlock_all();
    assert!(pins.entries().iter().all(|pin| !pin.state.locked));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_click_through_pin_is_not_offered_by_the_window_picker() {
    // A pinned window can carry a foreign compositor class, but its title is
    // what the pickers and the capture paths use to exclude it.
    let own = window(&pin_title("shot.png", false), "com.example.Other");
    let locked = window(&pin_title("shot.png", true), "com.example.Other");
    let foreign = window("shot.png", "com.example.Other");
    assert!(is_pin_title(&own.title));
    assert!(is_pin_title(&locked.title));
    assert!(own.is_own_preview());
    assert!(locked.is_own_preview());
    assert!(!foreign.is_own_preview());

    let clients = Value::Array(vec![
        serde_value(&own),
        serde_value(&locked),
        serde_value(&foreign),
    ]);
    let monitors = tidyshot::json::parse(
        r#"[{"name":"DP-1","x":0,"y":0,"width":1920,"height":1080,"focused":true,"activeWorkspace":{"name":"1"}}]"#,
    )
    .unwrap();
    let frames = vec![tidyshot::window_picker::FrozenFrame {
        name: "DP-1".to_owned(),
        bounds: Rect::new(0, 0, 1920, 1080),
        focused: true,
        url: "file:///tmp/frozen.png".to_owned(),
    }];
    let model = tidyshot::window_picker::build(&clients, &monitors, frames, "#fff".to_owned());
    let offered: Vec<&str> = model
        .rows
        .iter()
        .map(|row| row.window.title.as_str())
        .collect();
    assert_eq!(offered, ["shot.png"]);
}

/// The picker reads `hyprctl -j clients` JSON, so build the row it would see.
fn serde_value(window: &WindowInfo) -> Value {
    let text = format!(
        r#"{{"title":"{}","class":"{}","at":[{},{}],"size":[{},{}],"monitor":0,"workspace":{{"name":"1"}},"address":"{}","pinned":true,"mapped":true,"focusHistoryID":0}}"#,
        window.title,
        window.class,
        window.rect.x,
        window.rect.y,
        window.rect.width,
        window.rect.height,
        window.address,
    );
    tidyshot::json::parse(&text).unwrap()
}

#[test]
fn the_live_coordinator_pins_persists_and_restores() {
    // The live registry is process-wide, so one test drives it end to end to
    // keep the assertions independent of test scheduling.
    let dir = scratch("live");
    let path = dir.join("shot.png");
    fs::write(&path, png(640, 320)).unwrap();
    assert_eq!(pin_window::restore_on_start(&dir).unwrap(), 0);

    let message = pin_window::pin_file(&path).unwrap();
    assert!(message.contains("shot.png"), "{message}");
    assert_eq!(pin_window::rows_json().len(), 1);
    let saved = PinRegistry::restore_at(&dir).unwrap();
    assert_eq!(saved.len(), 1, "pinning writes the listing");
    assert_eq!((saved.entries()[0].state.width), 520.0);

    assert_eq!(pin_window::set_opacity(&path, 0.5), "");
    assert!(pin_window::set_click_through(&path, true).contains("click-through"));
    let saved = PinRegistry::restore_at(&dir).unwrap();
    assert!((saved.entries()[0].state.opacity - 0.5).abs() < 1e-9);
    assert!(saved.entries()[0].click_through);
    assert!(saved.entries()[0].state.locked);

    pin_window::set_geometry(&path, 40.0, 50.0, 300.0, 150.0);
    let saved = PinRegistry::restore_at(&dir).unwrap();
    assert_eq!((saved.entries()[0].x, saved.entries()[0].y), (40.0, 50.0));
    assert_eq!(
        (
            saved.entries()[0].state.width,
            saved.entries()[0].state.height
        ),
        (300.0, 150.0)
    );

    assert_eq!(pin_window::toggle(), "Pins hidden");
    assert!(PinRegistry::restore_at(&dir).unwrap().hidden() == false);
    // A hidden set stays in the listing but does not restore hidden.
    assert_eq!(pin_window::toggle(), "Pins shown");

    assert_eq!(pin_window::unlock_all(), "Pins unlocked");
    assert!(pin_window::set_locked(&path, true).contains("locked"));
    assert_eq!(pin_window::unlock_all(), "Pins unlocked");
    let saved = PinRegistry::restore_at(&dir).unwrap();
    assert!(!saved.entries()[0].state.locked);
    assert!(!saved.entries()[0].click_through);

    assert_eq!(pin_window::close_all(), "Pins closed");
    assert!(pin_window::rows_json().is_empty());
    assert!(PinRegistry::restore_at(&dir).unwrap().is_empty());

    // A pin that is gone from disk is reported instead of being stored.
    assert!(pin_window::pin_file(Path::new("/tmp/definitely-missing-pin.png")).is_err());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_restored_pin_keeps_its_saved_geometry_and_title() {
    let dir = scratch("restore");
    let path = dir.join("shot.png");
    fs::write(&path, png(400, 200)).unwrap();
    let listing = format!(
        r#"[{{"path":"{}","name":"shot.png","x":12,"y":34,"width":260,"height":130,"imageWidth":400,"imageHeight":200,"opacity":0.8,"locked":true,"clickThrough":false,"visible":true}}]"#,
        path.display()
    );
    fs::write(pins_path(&dir), listing).unwrap();
    let pins = PinRegistry::restore_at(&dir).unwrap();
    let pin: &PinnedWindow = &pins.entries()[0];
    assert_eq!((pin.x, pin.y), (12.0, 34.0));
    assert_eq!((pin.state.width, pin.state.height), (260.0, 130.0));
    assert_eq!(pin.state.image_width, 400);
    assert_eq!(pin.title(), "TidyShot Locked Pin — shot.png");
    let _ = fs::remove_dir_all(&dir);
}
