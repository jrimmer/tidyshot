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
        type Args = super::ArgsRust;
    }
}

use core::pin::Pin;
use cxx_qt_lib::QString;

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

pub struct ArgsRust {
    label: QString,
}

impl Default for ArgsRust {
    fn default() -> Self {
        Self {
            label: QString::from("tidyshot"),
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
