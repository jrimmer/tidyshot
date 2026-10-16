// One menu row, following the Button conventions in plugin/BarWidget.qml on
// the OmniShot mirror branch pr/16: `text`, `leftAlign`, and a `hasCursor`
// highlight. The label and shortcut hint come from the Rust action table.
import QtQuick

Rectangle {
    id: button

    property string text
    property string shortcut
    property bool leftAlign: true
    property bool hasCursor: false

    signal clicked()

    implicitHeight: 30
    radius: 4
    color: button.hasCursor ? "#333333" : "transparent"

    Text {
        id: caption
        anchors.verticalCenter: parent.verticalCenter
        anchors.left: parent.left
        anchors.leftMargin: button.leftAlign ? 8 : 0
        text: button.text
        color: "#f5f5f5"
        font.pixelSize: 14
        elide: Text.ElideRight
    }

    Text {
        anchors.verticalCenter: parent.verticalCenter
        anchors.right: parent.right
        anchors.rightMargin: 8
        text: button.shortcut
        color: "#9a9a9a"
        font.pixelSize: 12
    }

    MouseArea {
        anchors.fill: parent
        onClicked: button.clicked()
    }
}
