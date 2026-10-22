// Offscreen behavior tests for the editor toolbar.
//
// The component is driven with synthetic clicks, and every assertion is on an
// emitted signal or a visible control, never on internal state.
import QtQuick
import QtTest
import ".."

TestCase {
    id: testcase
    name: "EditorToolbar"
    when: windowShown

    Window {
        width: 900
        height: 300
        visible: true
        EditorToolbar {
            id: toolbar
            anchors.centerIn: parent
            width: 880
            toolIds: ["select", "arrow", "rect"]
            toolLabels: ["Select", "Arrow", "Rectangle"]
            toolShortcuts: ["V", "A", "R"]
        }
    }

    SignalSpy { id: toolSpy; target: toolbar; signalName: "toolChosen" }
    SignalSpy { id: colorSpy; target: toolbar; signalName: "colorChosen" }
    SignalSpy { id: undoSpy; target: toolbar; signalName: "undoRequested" }
    SignalSpy { id: saveSpy; target: toolbar; signalName: "saveRequested" }
    SignalSpy { id: cropSpy; target: toolbar; signalName: "applyCropRequested" }
    SignalSpy { id: saveAsSpy; target: toolbar; signalName: "saveAsRequested" }
    SignalSpy { id: exportSpy; target: toolbar; signalName: "exportRequested" }

    function init() {
        toolSpy.clear()
        colorSpy.clear()
        undoSpy.clear()
        saveSpy.clear()
        cropSpy.clear()
        saveAsSpy.clear()
        exportSpy.clear()
        toolbar.currentTool = "select"
        toolbar.canUndo = false
        toolbar.cropActive = false
    }

    function test_a_tool_click_reports_its_id() {
        mouseClick(findChild(toolbar, "tool_arrow"), 5, 5)
        compare(toolSpy.count, 1)
        compare(toolSpy.signalArguments[0][0], "arrow")
    }

    function test_the_current_tool_is_checked() {
        toolbar.currentTool = "rect"
        waitForRendering(toolbar)
        compare(findChild(toolbar, "tool_rect").checked, true)
        compare(findChild(toolbar, "tool_arrow").checked, false)
    }

    function test_a_swatch_reports_its_color() {
        mouseClick(findChild(toolbar, "swatch_53d49b"), 2, 2)
        compare(colorSpy.count, 1)
        compare(colorSpy.signalArguments[0][0], "#53d49b")
    }

    function test_undo_is_disabled_until_the_document_can_undo() {
        compare(findChild(toolbar, "undoButton").enabled, false)
        toolbar.canUndo = true
        waitForRendering(toolbar)
        compare(findChild(toolbar, "undoButton").enabled, true)
        mouseClick(findChild(toolbar, "undoButton"), 5, 5)
        compare(undoSpy.count, 1)
    }

    function test_save_is_always_reachable() {
        mouseClick(findChild(toolbar, "saveButton"), 5, 5)
        compare(saveSpy.count, 1)
    }

    function test_save_as_is_reachable() {
        mouseClick(findChild(toolbar, "saveAsButton"), 5, 5)
        compare(saveAsSpy.count, 1)
    }

    function test_the_export_menu_emits_the_chosen_format() {
        mouseClick(findChild(toolbar, "exportButton"), 5, 5)
        var menu = findChild(toolbar, "exportMenu")
        verify(menu !== null)
        var expected = [
            ["exportPng", "png"], ["exportJpeg", "jpeg"],
            ["exportWebp", "webp"], ["exportHeic", "heic"]
        ]
        for (var index = 0; index < expected.length; ++index) {
            menu.open()
            waitForRendering(toolbar)
            findChild(menu.contentItem, expected[index][0]).triggered()
            compare(exportSpy.count, index + 1)
            compare(exportSpy.signalArguments[index][0], expected[index][1])
        }
        menu.close()
    }

    function test_crop_actions_only_show_while_a_crop_is_provisional() {
        compare(findChild(toolbar, "applyCropButton").visible, false)
        toolbar.cropActive = true
        waitForRendering(toolbar)
        compare(findChild(toolbar, "applyCropButton").visible, true)
        mouseClick(findChild(toolbar, "applyCropButton"), 5, 5)
        compare(cropSpy.count, 1)
    }

    function test_the_toolbar_scales_with_the_omarchy_text_scale() {
        var base = findChild(toolbar, "tool_select").height
        toolbar.uiScale = 2
        waitForRendering(toolbar)
        verify(findChild(toolbar, "tool_select").height > base)
    }
}
