// Offscreen behavior tests for the scrolling capture controls.
//
// The controls receive their state as plain properties, exactly as Main.qml
// binds them from the Rust Capture singleton. Synthetic clicks assert the
// observable signals each control produces and the frame count it shows.
import QtQuick
import QtTest
import ".."

TestCase {
    id: testcase
    name: "ScrollControls"
    when: windowShown

    Window {
        id: window
        width: 700
        height: 700
        visible: true
        ScrollControls {
            id: controls
            anchors.centerIn: parent
        }
    }

    SignalSpy { id: startSpy; target: controls; signalName: "startRequested" }
    SignalSpy { id: autoSpy; target: controls; signalName: "autoToggled" }
    SignalSpy { id: doneSpy; target: controls; signalName: "doneRequested" }
    SignalSpy { id: cancelSpy; target: controls; signalName: "cancelRequested" }

    function init() {
        startSpy.clear()
        autoSpy.clear()
        doneSpy.clear()
        cancelSpy.clear()
        controls.status = "Scroll slowly inside the selection."
        controls.frames = 0
        controls.autoScroll = false
        controls.running = false
        controls.horizontal = false
    }

    function button(name) {
        return findChild(controls, name)
    }

    function test_status_and_frame_count_are_shown() {
        compare(button("scrollStatus").text, "Scroll slowly inside the selection.")
        compare(button("scrollFrames").text, "0 frames")
        controls.frames = 3
        compare(button("scrollFrames").text, "3 frames")
        controls.frames = 1
        compare(button("scrollFrames").text, "1 frame")
    }

    function test_start_auto_and_cancel_emit_their_signals() {
        mouseClick(button("scrollStart"), 10, 10)
        compare(startSpy.count, 1)
        mouseClick(button("scrollAuto"), 10, 10)
        compare(autoSpy.count, 1)
        mouseClick(button("scrollCancel"), 10, 10)
        compare(cancelSpy.count, 1)
    }

    function test_done_waits_for_the_first_frame() {
        compare(button("scrollDone").enabled, false)
        controls.frames = 2
        compare(button("scrollDone").enabled, true)
        mouseClick(button("scrollDone"), 10, 10)
        compare(doneSpy.count, 1)
    }

    function test_a_running_capture_refuses_a_second_start() {
        controls.running = true
        compare(button("scrollStart").enabled, false)
        compare(button("scrollStart").text, "Capturing…")
    }

    function test_auto_state_and_axis_are_labelled() {
        compare(button("scrollAuto").text, "Auto-Scroll: Off")
        controls.autoScroll = true
        compare(button("scrollAuto").text, "Auto-Scroll: On")
        compare(button("scrollTitle").text, "Scrolling capture")
        controls.horizontal = true
        compare(button("scrollTitle").text, "Scrolling capture · horizontal")
    }
}
