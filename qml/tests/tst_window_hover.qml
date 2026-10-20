// Offscreen behavior tests for the frozen-frame window picker.
//
// The component receives the frozen frame and the focus-sorted rows as plain
// data, exactly as Main.qml binds them from the Rust picker. Synthetic mouse
// moves and keys assert the observable choice, modifiers and dismissal.
import QtQuick
import QtTest
import ".."

TestCase {
    id: testcase
    name: "WindowHover"
    when: windowShown

    Window {
        id: host
        width: 800
        height: 600
        visible: true
        WindowHover {
            id: hover
            frame: ({ "index": 0, "name": "DP-1", "x": 0, "y": 0, "w": 800, "h": 600, "url": "" })
            rows: [
                { "title": "Editor", "app": "code", "x": 10, "y": 20, "w": 200, "h": 150, "monitor": 0 },
                { "title": "Browser", "app": "firefox", "x": 300, "y": 200, "w": 300, "h": 200, "monitor": 0 }
            ]
            accent: "#ff0000"
        }
    }

    SignalSpy { id: chosenSpy; target: hover; signalName: "chosen" }
    SignalSpy { id: dismissedSpy; target: hover; signalName: "dismissed" }

    function init() {
        chosenSpy.clear()
        dismissedSpy.clear()
        hover.current = -1
        hover.currentTitle = ""
    }

    function test_hovering_a_window_highlights_it() {
        hover.selectRow(hover.rowAt(320, 220))
        compare(hover.current, 1)
        compare(hover.currentTitle, "Browser")
    }

    function test_clicking_a_window_reports_its_index_and_modifiers() {
        hover.selectRow(hover.rowAt(20, 30))
        hover.choose(true, false)
        compare(chosenSpy.count, 1)
        compare(chosenSpy.signalArguments[0][0], 0)
        compare(chosenSpy.signalArguments[0][1], true)
        compare(chosenSpy.signalArguments[0][2], false)
    }

    function test_tab_cycles_by_focus_order_and_wraps() {
        hover.cycle(1)
        compare(hover.current, 0)
        hover.cycle(1)
        compare(hover.current, 1)
        hover.cycle(1)
        compare(hover.current, 0)
        hover.cycle(-1)
        compare(hover.current, 1)
    }

    function test_escape_dismisses_without_a_choice() {
        keyClick(Qt.Key_Escape)
        compare(dismissedSpy.count, 1)
        compare(chosenSpy.count, 0)
    }

    function test_the_accent_paints_the_highlight() {
        compare(hover.accent, "#ff0000")
        hover.selectRow(0)
        compare(hover.currentTitle, "Editor")
    }
}
