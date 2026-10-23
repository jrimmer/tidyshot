// One pinned capture: an always-on-top frameless window at its pin size.
//
// The pin geometry, opacity and controls live in src/pin.rs; this component
// only draws one row from the PinApi singleton. It stays free of the
// `tidyshot` module so qmltestrunner can drive it offscreen.
import QtQuick
import QtQuick.Controls

Window {
    id: pinWindow

    /// The pinned file.
    property string path: ""
    /// The pin's display name.
    property string name: ""
    /// Pin size and opacity from the Rust pin state.
    property real pinWidth: 200
    property real pinHeight: 150
    property real opacityValue: 1.0
    property bool locked: false
    /// The picture to draw.
    property url source: ""

    signal closeRequested()
    signal unlockRequested()

    width: pinWindow.pinWidth
    height: pinWindow.pinHeight
    visible: pinWindow.path !== ""
    title: qsTr("TidyShot Pin — ") + pinWindow.name
    color: "transparent"
    opacity: pinWindow.opacityValue
    flags: Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint | Qt.Tool

    Rectangle {
        anchors.fill: parent
        radius: 8
        color: "#242424"

        Image {
            objectName: "pinImage"
            anchors.fill: parent
            anchors.margins: 2
            fillMode: Image.PreserveAspectFit
            source: pinWindow.source
            asynchronous: true
        }

        Button {
            objectName: "pinClose"
            visible: !pinWindow.locked
            anchors.top: parent.top
            anchors.right: parent.right
            text: "×"
            onClicked: pinWindow.closeRequested()
        }

        Button {
            objectName: "pinUnlock"
            visible: pinWindow.locked
            anchors.bottom: parent.bottom
            anchors.horizontalCenter: parent.horizontalCenter
            text: "Unlock"
            onClicked: pinWindow.unlockRequested()
        }
    }
}
