import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import tidyshot

ApplicationWindow {
    id: window
    width: 480
    height: 320
    visible: true
    title: qsTr("tidyshot")
    color: "#1e1e1e"

    Material.theme: Material.Dark
    Material.accent: "#FFD60A"

    Column {
        anchors.centerIn: parent
        spacing: 16

        Label {
            text: Args.label + " — " + Counter.count
            color: "#f5f5f5"
            font.pixelSize: 18
            anchors.horizontalCenter: parent.horizontalCenter
        }

        Button {
            text: qsTr("Count")
            anchors.horizontalCenter: parent.horizontalCenter
            onClicked: Counter.increment()
        }
    }
}
