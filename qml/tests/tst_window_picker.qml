// Offscreen behavior tests for the window picker.
//
// The picker receives its rows as plain lists, exactly as Main.qml binds them
// from the Rust Capture singleton. Synthetic clicks assert the observable
// signals a choice, a dismissal and an empty list produce.
import QtQuick
import QtTest
import ".."

TestCase {
    id: testcase
    name: "WindowPicker"
    when: windowShown

    Window {
        id: window
        width: 700
        height: 700
        visible: true
        WindowPicker {
            id: picker
            anchors.centerIn: parent
        }
    }

    SignalSpy { id: chosenSpy; target: picker; signalName: "chosen" }
    SignalSpy { id: dismissedSpy; target: picker; signalName: "dismissed" }

    function init() {
        chosenSpy.clear()
        dismissedSpy.clear()
        picker.labels = ["code — Editor", "firefox — Browser"]
        picker.details = ["DP-1 · 1 · 600 × 400", "DP-1 · 1 · 800 × 600"]
        picker.open()
    }

    function cleanup() {
        picker.close()
    }

    function test_rows_are_listed_from_the_supplied_labels() {
        var list = findChild(picker, "windowList")
        compare(list.count, 2)
        compare(list.itemAtIndex(0).text, "code — Editor")
        compare(list.itemAtIndex(1).text, "firefox — Browser")
    }

    function test_clicking_a_row_reports_its_index() {
        var list = findChild(picker, "windowList")
        mouseClick(list.itemAtIndex(1), 10, 10)
        compare(chosenSpy.count, 1)
        compare(chosenSpy.signalArguments[0][0], 1)
    }

    function test_cancel_dismisses_without_a_choice() {
        mouseClick(findChild(picker, "cancelPicker"), 10, 10)
        compare(dismissedSpy.count, 1)
        compare(chosenSpy.count, 0)
    }

    function test_an_empty_list_explains_that_there_is_no_window() {
        picker.labels = []
        picker.details = []
        compare(findChild(picker, "pickerMessage").visible, true)
        compare(findChild(picker, "pickerMessage").text, "No window to capture")
        compare(findChild(picker, "windowList").count, 0)
    }
}
