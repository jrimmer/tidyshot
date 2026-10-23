// Offscreen behavior tests for the timer countdown label.
//
// The label receives the seconds from the Rust countdown; Cancel and Escape
// emit the one signal the runtime listens for.
import QtQuick
import QtTest
import ".."

TestCase {
    id: testcase
    name: "CountdownOverlay"
    when: windowShown

    Window {
        id: host
        width: 500
        height: 200
        visible: true
        CountdownOverlay {
            id: countdown
            anchors.centerIn: parent
        }
    }

    SignalSpy { id: cancelSpy; target: countdown; signalName: "cancelRequested" }

    function init() {
        cancelSpy.clear()
        countdown.label = ""
        countdown.seconds = 0
    }

    function test_the_seconds_are_shown() {
        var label = findChild(countdown, "countdownLabel")
        verify(label !== null)
        countdown.seconds = 3
        compare(label.text, "Capturing in 3…")
        countdown.label = "Capturing in 2…"
        compare(label.text, "Capturing in 2…")
    }

    function test_the_cancel_button_emits_the_signal() {
        mouseClick(findChild(countdown, "countdownCancel"), 5, 5)
        compare(cancelSpy.count, 1)
    }

    function test_escape_cancels() {
        keyClick(Qt.Key_Escape)
        compare(cancelSpy.count, 1)
    }

    function test_the_label_is_wide_enough_for_its_text() {
        countdown.seconds = 5
        verify(countdown.width >= 220)
        verify(countdown.height > 0)
    }
}
