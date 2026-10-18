// Window picker for a window capture.
//
// The visible windows arrive from the Rust `Capture` singleton as parallel
// `windowLabels` and `windowDetails` lists; this file only lays them out and
// forwards the chosen row back, so the client geometry never lives in QML.
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import tidyshot

Popup {
    id: picker
    objectName: "windowPicker"
    modal: true
    focus: true
    width: 460
    height: 420
    anchors.centerIn: Overlay.Window.contentItem
    closePolicy: Popup.CloseOnEscape
    padding: 0

    background: Rectangle {
        color: "#242424"
        radius: 8
        border.color: "#3a3a3a"
    }

    contentItem: Column {
        spacing: 8
        anchors.fill: parent
        anchors.margins: 12

        Label {
            text: "Choose a window"
            color: "white"
            font.bold: true
        }

        Label {
            objectName: "pickerMessage"
            width: parent.width
            text: Capture.windowLabels.length === 0 ? "No window to capture" : ""
            visible: text !== ""
            color: "#9a9a9a"
            wrapMode: Text.WordWrap
        }

        ListView {
            id: windowList
            objectName: "windowList"
            width: parent.width
            height: parent.height - 70
            clip: true
            model: Capture.windowLabels

            delegate: ItemDelegate {
                required property var modelData
                required property int index
                width: ListView.view.width
                text: modelData
                onClicked: Capture.captureWindow(index)
            }
        }

        Row {
            spacing: 8
            Button {
                objectName: "cancelPicker"
                text: "Cancel"
                onClicked: Capture.closeWindowPicker()
            }
        }
    }

    onClosed: Capture.closeWindowPicker()
}
