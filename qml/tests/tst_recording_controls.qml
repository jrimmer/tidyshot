// Offscreen behavior tests for the recording controls.
//
// The strip receives its state as plain properties, exactly as Main.qml binds
// them from the Rust Recording singleton. Synthetic clicks assert the signal
// each control emits and the elapsed line it shows.
import QtQuick
import QtTest
import ".."

TestCase {
    id: testcase
    name: "RecordingControls"
    when: windowShown

    Window {
        id: window
        width: 700
        height: 400
        visible: true
        RecordingControls {
            id: controls
            anchors.centerIn: parent
        }
    }

    SignalSpy { id: pauseSpy; target: controls; signalName: "pauseRequested" }
    SignalSpy { id: stopSpy; target: controls; signalName: "stopRequested" }
    SignalSpy { id: restartSpy; target: controls; signalName: "restartRequested" }
    SignalSpy { id: cancelSpy; target: controls; signalName: "cancelRequested" }

    function init() {
        pauseSpy.clear()
        stopSpy.clear()
        restartSpy.clear()
        cancelSpy.clear()
        controls.elapsed = "00:00"
        controls.paused = false
        controls.status = ""
    }

    function button(name) {
        return findChild(controls, name)
    }

    function test_the_strip_is_tall_enough_for_its_controls() {
        verify(controls.height > 0, "the controls strip has no height")
        var names = ["recordingPause", "recordingStop", "recordingRestart", "recordingCancel"]
        for (var i = 0; i < names.length; ++i) {
            var control = button(names[i])
            verify(control !== null, names[i] + " is missing")
            verify(control.y >= 0, names[i] + " is above the strip")
            verify(control.y + control.height <= controls.height,
                   names[i] + " is below the strip")
            verify(control.visible, names[i] + " is hidden")
        }
    }

    function test_elapsed_time_and_pause_label_are_shown() {
        compare(button("recordingElapsed").text, "00:00")
        controls.elapsed = "12:34"
        compare(button("recordingElapsed").text, "12:34")
        compare(button("recordingPause").text, "Pause")
        controls.paused = true
        compare(button("recordingPause").text, "Resume")
    }

    function test_every_control_emits_its_signal() {
        mouseClick(button("recordingPause"), 10, 10)
        compare(pauseSpy.count, 1)
        mouseClick(button("recordingStop"), 10, 10)
        compare(stopSpy.count, 1)
        mouseClick(button("recordingRestart"), 10, 10)
        compare(restartSpy.count, 1)
        mouseClick(button("recordingCancel"), 10, 10)
        compare(cancelSpy.count, 1)
    }

    function test_a_status_line_is_only_shown_when_present() {
        compare(button("recordingStatus").visible, false)
        controls.status = "Saved /tmp/take.mp4"
        compare(button("recordingStatus").visible, true)
        compare(button("recordingStatus").text, "Saved /tmp/take.mp4")
    }
}
