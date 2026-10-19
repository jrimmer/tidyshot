//! The QML image provider that serves the rendered editor canvas.
//!
//! `cxx-qt-lib` exposes provider registration but not a writable image, so a
//! small C++ provider owns one `QImage`; this module is its Rust bridge.

use core::pin::Pin;

#[cxx::bridge]
mod image_bridge {
    unsafe extern "C++" {
        include!("editor_image.h");
        type QImage = cxx_qt_lib::QImage;
        include!("cxx-qt-lib/qml/qqmlengine.h");
        type QQmlEngine = cxx_qt_lib::QQmlEngine;

        /// Register the `tidyshot` image provider on `engine`.
        fn tidyshot_editor_image_install(engine: Pin<&mut QQmlEngine>);

        /// Replace the image the provider serves.
        #[rust_name = "set_image_raw"]
        fn tidyshot_editor_image_set(data: &[u8], width: i32, height: i32);
    }
}

/// Register the editor image provider with the application's engine.
pub fn install(engine: Pin<&mut cxx_qt_lib::QQmlEngine>) {
    image_bridge::tidyshot_editor_image_install(engine);
}

/// Publish RGBA8 pixels as the provider's canvas image.
pub fn set_image(data: &[u8], width: i32, height: i32) {
    image_bridge::set_image_raw(data, width, height);
}
