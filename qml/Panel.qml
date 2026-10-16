// Omarchy-style anchored menu card, following plugin/BarWidget.qml conventions
// on the OmniShot mirror branch pr/16. It only lays out content; every row
// comes from the Rust action table through the Menu singleton.
import QtQuick

Rectangle {
    id: panel

    property Item anchorItem
    property bool open: false
    property real contentWidth: 440
    property real contentHeight: 320

    default property alias content: contentItem.data

    visible: open
    width: contentWidth
    height: contentHeight
    radius: 8
    color: "#242424"
    border.color: "#3a3a3a"

    Item {
        id: contentItem
        anchors.fill: parent
        anchors.margins: 12
    }
}
