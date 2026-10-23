// Always-on-top countdown label for a timer capture.
// Ported from omnishot/capture_countdown.py (CaptureCountdown)
//
// The seconds come from the Rust countdown on every tick; Cancel and Escape
// stop the wait before the pixels are taken.
import QtQuick
import QtQuick.Controls

Rectangle {
    id: countdown

    /// The label the runtime publishes, e.g. "Capturing in 3…".
    property string label: ""
    /// The whole seconds still remaining.
    property int seconds: 0

    /// The countdown should stop.
    signal cancelRequested()

    width: Math.max(220, row.implicitWidth + 20)
    implicitHeight: row.implicitHeight + 16
    color: "#242424"
    radius: 8
    border.color: "#3a3a3a"

    Row {
        id: row
        anchors.centerIn: parent
        spacing: 10

        Text {
            objectName: "countdownLabel"
            text: countdown.label !== "" ? countdown.label : ("Capturing in " + countdown.seconds + "…")
            color: "white"
            font.bold: true
            anchors.verticalCenter: parent.verticalCenter
        }

        Button {
            objectName: "countdownCancel"
            text: "Cancel"
            onClicked: countdown.cancelRequested()
        }
    }

    Shortcut {
        sequences: ["Escape"]
        onActivated: countdown.cancelRequested()
    }
}
