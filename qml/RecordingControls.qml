// Always-on-top controls for one running recording.
//
// Layout only: the session state and every action live in
// src/recording_runtime.rs, bound here through the Recording singleton. The
// component is free of the `tidyshot` module so qmltestrunner can drive it
// offscreen with synthetic clicks.
import QtQuick
import QtQuick.Controls

Rectangle {
    id: controls

    /// Recorded time as `MM:SS`, excluding pauses.
    property string elapsed: "00:00"
    /// Whether the take is suspended.
    property bool paused: false
    /// The session's status or failure line.
    property string status: ""

    /// Suspend or resume the take.
    signal pauseRequested()
    /// Finish the take and keep it.
    signal stopRequested()
    /// Discard the take and start another.
    signal restartRequested()
    /// Abandon the take without saving.
    signal cancelRequested()

    width: 380
    implicitHeight: row.implicitHeight + row.anchors.margins * 2
    color: "#242424"
    radius: 8
    border.color: "#3a3a3a"

    Row {
        id: row
        anchors.fill: parent
        anchors.margins: 10
        spacing: 8

        Rectangle {
            width: 10
            height: 10
            radius: 5
            anchors.verticalCenter: parent.verticalCenter
            color: controls.paused ? "#FFD60A" : "#ff4d4d"
        }

        Text {
            objectName: "recordingElapsed"
            text: controls.elapsed
            color: "white"
            font.bold: true
            font.pixelSize: 18
            anchors.verticalCenter: parent.verticalCenter
        }

        Button {
            objectName: "recordingPause"
            text: controls.paused ? "Resume" : "Pause"
            onClicked: controls.pauseRequested()
        }
        Button {
            objectName: "recordingStop"
            text: "Stop"
            onClicked: controls.stopRequested()
        }
        Button {
            objectName: "recordingRestart"
            text: "Restart"
            onClicked: controls.restartRequested()
        }
        Button {
            objectName: "recordingCancel"
            text: "Cancel"
            onClicked: controls.cancelRequested()
        }
    }

    Text {
        objectName: "recordingStatus"
        anchors.top: row.bottom
        anchors.left: parent.left
        anchors.margins: 10
        text: controls.status
        color: "#9a9a9a"
        visible: text !== ""
    }
}
