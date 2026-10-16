// Omarchy-style bar panel for the capture menu, following the Panel,
// KeyboardPanel, and Button conventions in plugin/BarWidget.qml on the
// OmniShot mirror branch pr/16.
//
// The rows arrive as parallel `ids`, `labels`, and `shortcuts` lists and the
// keyboard cursor as `selectedIndex`. Main.qml binds those from the Rust
// `Menu` singleton and routes `keyPressed` back to it, so this file renders
// the single action table without keeping an action list or key map of its
// own.
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

    /// A key was pressed; Main.qml forwards it to Menu.handleKey.
    signal keyPressed(int key)
    /// A row was activated by index.
    signal activated(int index)
    /// The menu should close.
    signal dismissed()
    /// The user tabbed to another panel (+1 forward, -1 back).
    signal tabbed(int direction)

    title: "tidyshot"

    focusTarget: menuPanel

    Keys.onPressed: function (event) {
        if (event.key === Qt.Key_H || event.key === Qt.Key_J || event.key === Qt.Key_K
                || event.key === Qt.Key_L || event.key === Qt.Key_Return || event.key === Qt.Key_Enter
                || event.key === Qt.Key_Escape || event.key === Qt.Key_Tab || event.key === Qt.Key_Backtab) {
            menuPanel.keyPressed(event.key)
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
                PanelButton {
                    required property var modelData
                    required property int index
                    width: column.width
                    text: modelData
                    shortcut: index < menuPanel.shortcuts.length ? menuPanel.shortcuts[index] : ""
                    hasCursor: menuPanel.selectedIndex === index
                    onClicked: menuPanel.activated(index)
                }
            }
        }
    }
}
