// Omarchy-style bar panel for the capture menu, following the Panel,
// KeyboardPanel, and Button conventions in plugin/BarWidget.qml on the
// OmniShot mirror branch pr/16.
//
// The rows arrive as parallel `ids`, `labels`, and `shortcuts` lists and the
// keyboard cursor as `selectedIndex`. Main.qml binds those from the Rust
// `Menu` singleton and forwards navigation back to it, so this file renders
// the single action table without keeping an action list of its own.
pragma ComponentBehavior: Bound
import QtQuick

KeyboardPanel {
    id: menuPanel

    /// Stable action ids, parallel to `labels`.
    property var ids: []
    /// Human-readable labels, parallel to `ids`.
    property var labels: []
    /// Shortcut hints, parallel to `ids`.
    property var shortcuts: []
    /// The highlighted row index, owned by the Rust action table.
    property int selectedIndex: 0

    /// A keyboard move, in rows and columns.
    signal moved(int dx, int dy)
    /// A row was activated by index.
    signal activated(int index)
    /// The menu should close.
    signal dismissed()
    /// The user tabbed to another panel (+1 forward, -1 back).
    signal tabbed(int direction)

    title: "tidyshot"

    focusTarget: menuPanel

    Keys.onPressed: function (event) {
        if (event.key === Qt.Key_J) {
            menuPanel.moved(0, 1)
            event.accepted = true
        } else if (event.key === Qt.Key_K) {
            menuPanel.moved(0, -1)
            event.accepted = true
        } else if (event.key === Qt.Key_H) {
            menuPanel.moved(-1, 0)
            event.accepted = true
        } else if (event.key === Qt.Key_L) {
            menuPanel.moved(1, 0)
            event.accepted = true
        } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
            menuPanel.activated(menuPanel.selectedIndex)
            event.accepted = true
        } else if (event.key === Qt.Key_Escape) {
            menuPanel.dismissed()
            event.accepted = true
        } else if (event.key === Qt.Key_Tab) {
            menuPanel.tabbed(1)
            event.accepted = true
        } else if (event.key === Qt.Key_Backtab) {
            menuPanel.tabbed(-1)
            event.accepted = true
        }
    }

    Panel {
        id: panel
        anchorItem: menuPanel
        open: true
        contentWidth: 440
        contentHeight: column.implicitHeight + 24

        Column {
            id: column
            width: parent.width
            spacing: 2

            Repeater {
                id: actionButtons
                model: menuPanel.labels
                MenuButton {
                    required property var modelData
                    required property int index
                    width: column.width
                    label: modelData
                    shortcut: index < menuPanel.shortcuts.length ? menuPanel.shortcuts[index] : ""
                    hasCursor: menuPanel.selectedIndex === index
                    onActivated: menuPanel.activated(index)
                }
            }
        }
    }
}
