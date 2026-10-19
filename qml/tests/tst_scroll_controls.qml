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

    // Main.qml anchors the panel by one corner inside the window, so the panel
    // must size itself. A zero height would lay out the buttons outside the
    // visible area even though every property binding is correct.
    function test_the_panel_is_tall_enough_for_its_controls() {
        verify(controls.height > 0, "the controls panel has no height")
        verify(controls.width > 0)
        var buttons = ["scrollStart", "scrollAuto", "scrollDone", "scrollCancel"]
        for (var i = 0; i < buttons.length; ++i) {
            var control = button(buttons[i])
            verify(control.y >= 0, buttons[i] + " is above the panel")
            verify(control.y + control.height <= controls.height,
                   buttons[i] + " is below the panel")
            verify(control.visible, buttons[i] + " is hidden")
        }
    }

    // The app instantiates the panel the same way Main.qml does: created on
    // the window's content item with only right and top anchors set.
    function test_a_corner_anchored_panel_sizes_itself() {
        var component = Qt.createComponent("../ScrollControls.qml")
        compare(component.status, Component.Ready)
        var panel = component.createObject(window.contentItem, {
            "anchors.right": window.contentItem.right,
            "anchors.top": window.contentItem.top
        })
        verify(panel !== null)
        compare(panel.width, 420)
        verify(panel.height > 40, "a corner-anchored panel must not collapse to height zero")
        var done = findChild(panel, "scrollDone")
        verify(done !== null)
        verify(done.y + done.height <= panel.height, "the Done button must sit inside the panel")
        panel.destroy()
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
