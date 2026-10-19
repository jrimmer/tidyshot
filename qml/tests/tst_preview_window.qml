// Offscreen behavior tests for the frameless Quick Access preview window.
//
// The component receives its state from the Rust Preview singleton in the
// app; here synthetic clicks and key events assert the signals it forwards.
import QtQuick
import QtTest
import ".."

TestCase {
    id: testcase
    name: "PreviewWindow"
    when: windowShown

    // A top-level Window cannot be a child of the TestCase, so create it
    // without a parent and tear it down after the suite.
    Component {
        id: previewComponent
        PreviewWindow {
            open: true
            title: "TidyShot Preview — shot.png"
            info: "640 × 480"
            contentUrl: ""
        }
    }
    property var preview: null

    function initTestCase() {
        preview = previewComponent.createObject(null)
        verify(preview !== null)
    }

    function cleanupTestCase() {
        preview.destroy()
        preview = null
    }

    SignalSpy { id: annotateSpy; target: preview; signalName: "annotateRequested" }
    SignalSpy { id: copySpy; target: preview; signalName: "copyRequested" }
    SignalSpy { id: saveSpy; target: preview; signalName: "saveRequested" }
    SignalSpy { id: pinSpy; target: preview; signalName: "pinRequested" }
    SignalSpy { id: dismissedSpy; target: preview; signalName: "dismissed" }

    function init() {
        annotateSpy.clear()
        copySpy.clear()
        saveSpy.clear()
        pinSpy.clear()
        dismissedSpy.clear()
        preview.open = true
        preview.autoCloseMs = 0
        preview.autoCloseAction = "close"
    }

    function button(name) {
        return findChild(preview.contentItem, name)
    }

    function test_the_window_is_frameless_and_always_on_top() {
        compare(preview.flags & Qt.FramelessWindowHint, Qt.FramelessWindowHint)
        compare(preview.flags & Qt.WindowStaysOnTopHint, Qt.WindowStaysOnTopHint)
        compare(preview.visible, true)
    }

    function test_the_overlay_body_is_reused_inside_the_window() {
        compare(button("titleLabel").text, "TidyShot Preview — shot.png")
        compare(button("infoLabel").text, "640 × 480")
    }

    function test_actions_forward_from_the_overlay() {
        mouseClick(button("copyButton"), 10, 10)
        compare(copySpy.count, 1)
        mouseClick(button("saveButton"), 10, 10)
        compare(saveSpy.count, 1)
        mouseClick(button("pinButton"), 10, 10)
        compare(pinSpy.count, 1)
        mouseClick(button("annotateButton"), 10, 10)
        compare(annotateSpy.count, 1)
    }

    function test_escape_and_close_forward_the_dismissal() {
        preview.dismissed()
        compare(dismissedSpy.count, 1)
        mouseClick(button("closeButton"), 10, 10)
        compare(dismissedSpy.count, 2)
    }

    function test_enter_copies() {
        keyClick(Qt.Key_Return)
        compare(copySpy.count, 1)
    }

    function test_auto_close_saves_then_dismisses() {
        preview.autoCloseMs = 20
        preview.autoCloseAction = "save"
        tryCompare(dismissedSpy, "count", 1, 2000)
        compare(saveSpy.count, 1)
    }

    function test_a_message_is_shown_including_a_failure() {
        preview.message = "Copied to the clipboard"
        waitForRendering(preview.contentItem)
        var message = findChild(preview, "previewMessage")
        verify(message !== null)
        compare(message.visible, true)
        compare(message.text.toString(), "Copied to the clipboard")
        preview.message = ""
    }

    function test_a_recording_offers_edit_instead_of_pin() {
        preview.video = true
        compare(button("pinButton").visible, false)
        compare(button("annotateButton").text, "Edit")
        preview.video = false
    }
}
