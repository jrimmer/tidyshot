// Offscreen behavior tests for the capture history window.
//
// The window receives its state as plain properties, exactly as Main.qml binds
// them from the Rust HistoryApi singleton. Synthetic clicks and key presses
// assert the emitted signals and the rows the filmstrip draws.
import QtQuick
import QtTest
import ".."

TestCase {
    id: testcase
    name: "HistoryWindow"
    when: windowShown

    Component {
        id: historyComponent
        HistoryWindow {
            visible: true
            rows: [
                { name: "shot.png", kind: "image", width: 640, height: 480, selected: false, source: "" },
                { name: "page.png", kind: "scroll", width: 800, height: 1600, selected: true, source: "" },
                { name: "take.mp4", kind: "video", width: 1920, height: 1080, selected: false, source: "" }
            ]
        }
    }

    property var history: null

    function initTestCase() {
        history = historyComponent.createObject(null)
        verify(history !== null)
    }

    function cleanupTestCase() {
        history.destroy()
        history = null
    }

    SignalSpy { id: filterSpy; target: history; signalName: "filterChosen" }
    SignalSpy { id: searchSpy; target: history; signalName: "searchChanged" }
    SignalSpy { id: movedSpy; target: history; signalName: "moved" }
    SignalSpy { id: selectedSpy; target: history; signalName: "rowSelected" }
    SignalSpy { id: activatedSpy; target: history; signalName: "rowActivated" }
    SignalSpy { id: deleteSpy; target: history; signalName: "deleteRequested" }
    SignalSpy { id: clearSpy; target: history; signalName: "clearRequested" }
    SignalSpy { id: restoreSpy; target: history; signalName: "restoreRequested" }
    SignalSpy { id: annotateSpy; target: history; signalName: "annotateRequested" }
    SignalSpy { id: pinSpy; target: history; signalName: "pinRequested" }
    SignalSpy { id: dismissedSpy; target: history; signalName: "dismissed" }

    function init() {
        filterSpy.clear()
        searchSpy.clear()
        movedSpy.clear()
        selectedSpy.clear()
        activatedSpy.clear()
        deleteSpy.clear()
        clearSpy.clear()
        restoreSpy.clear()
        annotateSpy.clear()
        pinSpy.clear()
        dismissedSpy.clear()
        history.cursor = 0
        history.message = ""
    }

    function button(name) {
        return findChild(history.contentItem, name)
    }

    function test_the_filmstrip_renders_every_row() {
        waitForRendering(history.contentItem)
        for (var i = 0; i < history.rows.length; ++i) {
            var row = findChild(history.contentItem, "historyRow_" + i)
            verify(row !== null, "row " + i + " is missing")
            compare(findChild(history.contentItem, "historyName_" + i).text, history.rows[i].name)
        }
        compare(findChild(history.contentItem, "historyRow_3"), null)
    }

    function test_a_row_click_selects_it_and_double_click_opens_it() {
        mouseClick(findChild(history.contentItem, "historyRow_0"), 10, 10)
        compare(selectedSpy.count, 1)
        compare(selectedSpy.signalArguments[0][0], 0)
        compare(selectedSpy.signalArguments[0][1], false)
        var row = findChild(history.contentItem, "historyRow_1")
        mouseClick(row, 10, 10)
        row.childAt(10, 10).doubleClicked(Qt.LeftButton)
        compare(activatedSpy.count, 1)
        compare(activatedSpy.signalArguments[0][0], 1)
    }

    function test_enter_opens_the_cursor_row_and_delete_emits() {
        history.cursor = 2
        verify(history.handleKey(Qt.Key_Return))
        compare(activatedSpy.count, 1)
        compare(activatedSpy.signalArguments[0][0], 2)
        verify(history.handleKey(Qt.Key_Delete))
        compare(deleteSpy.count, 1)
    }

    function test_arrow_and_hjkl_keys_move_the_cursor() {
        verify(history.handleKey(Qt.Key_Right))
        compare(movedSpy.count, 1)
        compare(movedSpy.signalArguments[0][0], 1)
        verify(history.handleKey(Qt.Key_H))
        compare(movedSpy.count, 2)
        compare(movedSpy.signalArguments[1][0], -1)
        verify(history.handleKey(Qt.Key_Escape))
        compare(dismissedSpy.count, 1)
        verify(!history.handleKey(Qt.Key_A))
    }

    function test_tabs_filter_and_the_search_box_reports_its_text() {
        mouseClick(findChild(history.contentItem, "historyTab_image"), 5, 5)
        compare(filterSpy.count, 1)
        compare(filterSpy.signalArguments[0][0], "image")
        var search = findChild(history.contentItem, "historySearch")
        search.text = "page"
        search.textEdited("page")
        compare(searchSpy.count, 1)
        compare(searchSpy.signalArguments[0][0], "page")
    }

    function test_the_action_buttons_emit_their_signals() {
        mouseClick(findChild(history.contentItem, "historyRestore"), 5, 5)
        compare(restoreSpy.count, 1)
        mouseClick(findChild(history.contentItem, "historyAnnotate"), 5, 5)
        compare(annotateSpy.count, 1)
        mouseClick(findChild(history.contentItem, "historyPin"), 5, 5)
        compare(pinSpy.count, 1)
        mouseClick(findChild(history.contentItem, "historyDelete"), 5, 5)
        compare(deleteSpy.count, 1)
        mouseClick(findChild(history.contentItem, "historyClear"), 5, 5)
        compare(clearSpy.count, 1)
        mouseClick(findChild(history.contentItem, "historyClose"), 5, 5)
        compare(dismissedSpy.count, 1)
    }

    function test_a_selected_row_is_marked_visibly() {
        waitForRendering(history.contentItem)
        compare(findChild(history.contentItem, "historyRow_1").border.color.toString(), "#ffd60a")
        compare(findChild(history.contentItem, "historyRow_0").border.color.toString(), "#3a3a3a")
    }

    function test_a_message_is_only_shown_when_present() {
        compare(button("historyMessage").visible, false)
        history.message = "Moved 1 capture to the trash"
        compare(button("historyMessage").visible, true)
        compare(button("historyMessage").text, "Moved 1 capture to the trash")
    }
}
