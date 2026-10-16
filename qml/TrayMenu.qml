// Tray fallback menu, rendered from the same Rust action table as the bar
// panel. `trayLabels` is one label per row with an empty string marking a
// separator between groups, so the tray never keeps its own list.
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls

Menu {
    id: trayMenu

    /// Labels from Menu.trayLabels; empty strings render as separators.
    property var trayLabels: []
    /// Stable action ids from Menu.trayIds, parallel to trayLabels.
    property var trayIds: []

    /// Emitted with the row id when a tray entry is chosen.
    signal actionTriggered(string id)

    Repeater {
        model: trayMenu.trayLabels
        delegate: MenuItem {
            required property var modelData
            required property int index
            visible: modelData.length > 0
            enabled: modelData.length > 0
            text: modelData
            onTriggered: trayMenu.actionTriggered(trayMenu.trayIds[index])
        }
    }
}
