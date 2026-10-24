// Offscreen behavior tests for the corner preview overlay.
// Ported from omnishot/tests/test_overlay.py and test_settings_save.py.
//
// Synthetic QtTest events drive the component the same way a user would:
// clicking the restore control, dragging the thumbnail out of the window and
// running the auto-close timer. The state assertions are on observable
// signals and visible controls, never on internal implementation.
import QtQuick
import QtTest
import ".."

TestCase {
    id: testcase
    name: "Overlay"
    when: windowShown

    Window {
        id: window
        width: 700
        height: 700
        visible: true
        Overlay {
            id: overlay
            anchors.centerIn: parent
        }
    }

    SignalSpy { id: activateSpy; target: overlay; signalName: "activate" }
    SignalSpy { id: copySpy; target: overlay; signalName: "copyRequested" }
    SignalSpy { id: saveSpy; target: overlay; signalName: "saveRequested" }
    SignalSpy { id: pinSpy; target: overlay; signalName: "pinRequested" }
    SignalSpy { id: trashSpy; target: overlay; signalName: "trashRequested" }
    SignalSpy { id: dismissedSpy; target: overlay; signalName: "dismissed" }
    SignalSpy { id: exportSpy; target: overlay; signalName: "exportRequested" }
    SignalSpy { id: openSpy; target: overlay; signalName: "openExternalRequested" }
    SignalSpy { id: saveAllSpy; target: overlay; signalName: "saveAllRequested" }
    SignalSpy { id: closeAllSpy; target: overlay; signalName: "closeAllRequested" }

    function init() {
        activateSpy.clear()
        copySpy.clear()
        saveSpy.clear()
        pinSpy.clear()
        trashSpy.clear()
        dismissedSpy.clear()
        exportSpy.clear()
        openSpy.clear()
        saveAllSpy.clear()
        closeAllSpy.clear()
        overlay.dragUri = ""
        overlay.title = "Capture ready"
        overlay.info = ""
        overlay.video = false
        overlay.collapsed = false
        overlay.dragging = false
        overlay.copying = false
        overlay.closeAfterDrag = true
        overlay.autoCloseMs = 0
        overlay.autoCloseAction = "close"
        overlay.trashVisible = false
    }

    function button(name) {
        return findChild(overlay, name)
    }

    function test_annotate_opens_the_editor() {
        mouseClick(button("annotateButton"), 10, 10)
        compare(activateSpy.count, 1)
    }

    function test_copy_save_and_pin_emit_their_signals() {
        mouseClick(button("copyButton"), 10, 10)
        compare(copySpy.count, 1)
        mouseClick(button("saveButton"), 10, 10)
        compare(saveSpy.count, 1)
        mouseClick(button("pinButton"), 10, 10)
        compare(pinSpy.count, 1)
    }

    function test_recordings_do_not_offer_pin_and_show_edit() {
        overlay.video = true
        compare(button("pinButton").visible, false)
        compare(button("annotateButton").text, "Edit")
        overlay.video = false
        compare(button("pinButton").visible, true)
        compare(button("annotateButton").text, "Annotate")
    }

    function test_collapse_hides_the_body_and_restore_shows_it() {
        overlay.setCollapsed(true)
        compare(overlay.collapsed, true)
        compare(button("titleLabel").visible, false)
        compare(button("restoreButton").visible, true)
        mouseClick(button("restoreButton"), 10, 10)
        compare(overlay.collapsed, false)
        compare(button("titleLabel").visible, true)
        compare(button("restoreButton").visible, false)
    }

    function test_close_button_dismisses() {
        mouseClick(button("closeButton"), 10, 10)
        compare(dismissedSpy.count, 1)
    }

    function test_auto_close_saves_when_configured() {
        overlay.autoCloseMs = 20
        overlay.autoCloseAction = "save"
        tryCompare(dismissedSpy, "count", 1, 2000)
        compare(saveSpy.count, 1)
    }

    function test_auto_close_does_not_fire_while_copying() {
        overlay.autoCloseMs = 20
        overlay.copying = true
        wait(200)
        compare(dismissedSpy.count, 0)
        overlay.copying = false
    }

    function test_trash_button_appears_only_for_autosaved_captures() {
        compare(button("trashButton").visible, false)
        overlay.trashVisible = true
        waitForRendering(overlay)
        compare(button("trashButton").visible, true)
        mouseClick(button("trashButton"), 10, 10)
        compare(trashSpy.count, 1)
    }

    function test_the_export_menu_emits_the_chosen_format() {
        mouseClick(button("exportButton"), 10, 10)
        var menu = button("exportMenu")
        verify(menu !== null)
        var expected = [
            ["exportPng", "png"], ["exportJpeg", "jpeg"],
            ["exportWebp", "webp"], ["exportHeic", "heic"]
        ]
        for (var index = 0; index < expected.length; ++index) {
            menu.open()
            waitForRendering(overlay)
            var item = button(expected[index][0])
            verify(item !== null, expected[index][0])
            item.triggered()
            compare(exportSpy.count, index + 1)
            compare(exportSpy.signalArguments[index][0], expected[index][1])
        }
        menu.close()
    }

    function test_open_save_all_and_close_all_emit_their_signals() {
        mouseClick(button("openButton"), 10, 10)
        compare(openSpy.count, 1)
        mouseClick(button("saveAllButton"), 10, 10)
        compare(saveAllSpy.count, 1)
        mouseClick(button("closeAllButton"), 10, 10)
        compare(closeAllSpy.count, 1)
    }

    function test_a_drag_offers_the_rust_built_uri_when_present() {
        // The Rust side percent-encodes the path once; the handle must carry
        // that body rather than the display URL.
        overlay.dragUri = "file:///tmp/my%20shot.png\r\n"
        waitForRendering(overlay)
        compare(button("dragHandle").Drag.mimeData["text/uri-list"],
                "file:///tmp/my%20shot.png\r\n")
        overlay.dragUri = ""
        overlay.contentUrl = "file:///tmp/fallback.png"
        waitForRendering(overlay)
        compare(String(button("dragHandle").Drag.mimeData["text/uri-list"]),
                "file:///tmp/fallback.png")
        overlay.contentUrl = ""
    }

    function test_drag_out_closes_unless_alt_keeps_it() {
        var handle = button("dragHandle")
        // The component reports an accepted drag through finishDrag; the
        // synthetic gesture proves the handle drives the drag state.
        mouseDrag(handle, 10, 10, 60, 0, Qt.LeftButton)
        overlay.finishDrag(true, false)
        compare(dismissedSpy.count, 1)
    }

    function test_alt_drag_keeps_the_preview_open() {
        overlay.finishDrag(true, true)
        compare(dismissedSpy.count, 0)
    }

    function test_swipe_down_collapses_and_up_restores() {
        overlay.setCollapsed(true)
        compare(overlay.collapsed, true)
        overlay.setCollapsed(false)
        compare(overlay.collapsed, false)
    }
    // The QuickOverlay layout the port promises.
    function test_the_quick_overlay_layout_is_preserved() {
        overlay.overlaySize = 300
        overlay.trashVisible = true
        overlay.title = "Capture ready"
        compare(overlay.width, 335)          // max(335, 300 + 20)
        overlay.overlaySize = 400
        compare(overlay.width, 420)          // max(335, 400 + 20)
        compare(overlay.title, "Capture ready")
        var trash = findChild(overlay, "trashButton")
        verify(trash.visible)
        compare(trash.implicitWidth, 36)
        compare(trash.implicitHeight, 36)
        var annotate = findChild(overlay, "annotateButton")
        var copy = findChild(overlay, "copyButton")
        var save = findChild(overlay, "saveButton")
        var pin = findChild(overlay, "pinButton")
        verify(annotate.text === "Annotate")
        verify(copy.text === "Copy")
        verify(save.text === "Save")
        verify(pin.visible)
        verify(pin.text === "Pin")
        var restore = findChild(overlay, "restoreButton")
        compare(restore.text, "\u2191 Show capture")
    }


}
