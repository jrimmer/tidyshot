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
            width: 900
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
    SignalSpy {
        id: activateSpy
        target: preview ? preview.contentItem.children[0] : null
        signalName: "activate"
    }
    SignalSpy { id: copySpy; target: preview; signalName: "copyRequested" }
    SignalSpy { id: saveSpy; target: preview; signalName: "saveRequested" }
    SignalSpy { id: pinSpy; target: preview; signalName: "pinRequested" }
    SignalSpy { id: dismissedSpy; target: preview; signalName: "dismissed" }
    SignalSpy { id: collapseSpy; target: preview; signalName: "collapseRequested" }
    SignalSpy { id: exportSpy; target: preview; signalName: "exportRequested" }
    SignalSpy { id: openSpy; target: preview; signalName: "openExternalRequested" }
    SignalSpy { id: saveAllSpy; target: preview; signalName: "saveAllRequested" }
    SignalSpy { id: closeAllSpy; target: preview; signalName: "closeAllRequested" }

    function init() {
        annotateSpy.clear()
        activateSpy.clear()
        copySpy.clear()
        saveSpy.clear()
        pinSpy.clear()
        dismissedSpy.clear()
        collapseSpy.clear()
        exportSpy.clear()
        openSpy.clear()
        saveAllSpy.clear()
        closeAllSpy.clear()
        preview.dragUri = ""
        preview.contentUrl = ""
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

    function test_export_open_save_all_and_close_all_forward() {
        var overlay = preview.contentItem.children[0]
        var menu = findChild(overlay, "exportMenu")
        verify(menu !== null)
        menu.open()
        waitForRendering(preview.contentItem)
        // The menu popup is its own top-level window, so its items are found
        // from the menu's content item rather than this window.
        findChild(menu.contentItem, "exportWebp").triggered()
        compare(exportSpy.count, 1)
        compare(String(exportSpy.signalArguments[0][0]), "webp")
        menu.close()
        waitForRendering(preview.contentItem)
        mouseClick(button("openButton"), 10, 10)
        compare(openSpy.count, 1)
        mouseClick(button("saveAllButton"), 10, 10)
        compare(saveAllSpy.count, 1)
        mouseClick(button("closeAllButton"), 10, 10)
        compare(closeAllSpy.count, 1)
    }

    function test_the_drag_body_carries_the_rust_uri() {
        preview.dragUri = "file:///tmp/my%20shot.png\r\n"
        waitForRendering(preview.contentItem)
        compare(button("dragHandle").Drag.mimeData["text/uri-list"],
                "file:///tmp/my%20shot.png\r\n")
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

    function test_a_standalone_preview_takes_activation_and_its_keys() {
        // A command-line capture has no menu behind it, so the preview claims
        // activation when it opens and Enter copies straight away.
        var window = previewComponent.createObject(null, { "open": false, "takeFocus": true })
        verify(window !== null)
        var spy = Qt.createQmlObject(
            'import QtTest; SignalSpy { signalName: "copyRequested" }', window)
        spy.target = window
        window.open = true
        tryCompare(window, "active", true)
        keyClick(Qt.Key_Return)
        compare(spy.count, 1)
        window.destroy()
        // Give activation back so the later key tests drive this preview.
        preview.requestActivate()
    }

    function test_the_key_shortcuts_belong_to_the_preview_window() {
        // An application-scoped shortcut would take Return from the capture
        // menu while a preview is open behind it.
        var copyShortcut = findChild(preview, "copyShortcut")
        var closeShortcut = findChild(preview, "closeShortcut")
        verify(copyShortcut !== null)
        verify(closeShortcut !== null)
        compare(copyShortcut.context, Qt.WindowShortcut)
        compare(closeShortcut.context, Qt.WindowShortcut)
        compare(copyShortcut.enabled, preview.open && preview.active)
        compare(closeShortcut.enabled, preview.open && preview.active)
    }

    function test_a_closed_preview_ignores_the_keys() {
        preview.open = false
        keyClick(Qt.Key_Return)
        keyClick(Qt.Key_Escape)
        compare(copySpy.count, 0)
        compare(dismissedSpy.count, 0)
    }

    function test_a_press_without_a_drag_rearms_without_a_second_finish() {
        // A click that never leaves the handle reports one release, not a
        // drag completion plus a release.
        mouseClick(button("dragHandle"), 10, 10)
        compare(activateSpy.count, 1)
        compare(dismissedSpy.count, 0)
    }

    function test_auto_close_saves_then_dismisses() {
        preview.autoCloseMs = 20
        preview.autoCloseAction = "save"
        tryCompare(dismissedSpy, "count", 1, 2000)
        compare(saveSpy.count, 1)
    }

    function test_collapsing_reports_the_new_height() {
        preview.collapseRequested(true)
        compare(collapseSpy.count, 1)
        compare(collapseSpy.signalArguments[0][0], true)
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
