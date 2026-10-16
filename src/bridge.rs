// The Rust side of tidyshot: a QML-visible counter and an args singleton,
// the pilot proof of the cxx-qt bridge.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(i32, count)]
        type Counter = super::CounterRust;

        #[qinvokable]
        fn increment(self: Pin<&mut Self>);

        #[qinvokable]
        fn reset(self: Pin<&mut Self>);
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, label)]
        #[qproperty(QString, ocr_path, cxx_name = "ocrPath")]
        #[qproperty(i32, linebreaks)]
        type Args = super::ArgsRust;
    }
}

use core::pin::Pin;
use cxx_qt_lib::QString;
use std::sync::OnceLock;

#[derive(Default)]
pub struct CounterRust {
    count: i32,
}

impl qobject::Counter {
    pub fn increment(self: Pin<&mut Self>) {
        let next = *self.count() + 1;
        self.set_count(next);
    }

    pub fn reset(self: Pin<&mut Self>) {
        self.set_count(0);
    }
}

/// What the command line asked the GUI to do on startup.
#[derive(Default)]
struct Startup {
    /// A file for the recognized-text dialog to run OCR over, when any.
    ocr_path: Option<String>,
    /// The `ocr-lines` / `ocr-single-line` override: -1 unset, 0 false, 1 true.
    linebreaks: i32,
}

static STARTUP: OnceLock<Startup> = OnceLock::new();

/// Record the command-line OCR request before QML loads.
///
/// The `Args` singleton reads this when it is first touched, so QML can start
/// the recognized-text completion for a file or `capture-text` URL.
pub fn set_startup_ocr(path: Option<String>, linebreaks: Option<bool>) {
    let _ = STARTUP.set(Startup {
        ocr_path: path,
        linebreaks: match linebreaks {
            None => -1,
            Some(false) => 0,
            Some(true) => 1,
        },
    });
}

fn startup() -> &'static Startup {
    STARTUP.get_or_init(Startup::default)
}

pub struct ArgsRust {
    label: QString,
    ocr_path: QString,
    linebreaks: i32,
}

impl Default for ArgsRust {
    fn default() -> Self {
        let startup = startup();
        Self {
            label: QString::from("tidyshot"),
            ocr_path: QString::from(startup.ocr_path.clone().unwrap_or_default()),
            linebreaks: startup.linebreaks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_counter_starts_at_zero() {
        assert_eq!(CounterRust::default().count, 0);
    }

    #[test]
    fn default_args_label_is_tidyshot() {
        assert_eq!(ArgsRust::default().label.to_string(), "tidyshot");
    }
}
