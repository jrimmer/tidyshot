// Window picker for a window capture.
//
// The visible windows arrive as parallel `labels` and `details` lists from the
// Rust `Capture` singleton; this file only lays them out and forwards the
// chosen row back, so the client geometry never lives in QML.
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls

Popup {
    id: picker
    objectName: "windowPicker"
    modal: true
    focus: true
    width: 460
    height: 420
    closePolicy: Popup.CloseOnEscape
    padding: 0

    /// One line per visible window.
    property var labels: []
    /// Monitor, workspace and size for each row.
    property var details: []
    /// Shown when no window can be captured.
    property string emptyText: "No window to capture"

    /// A row was chosen.
    signal chosen(int index)
    /// The picker should close without a choice.
    signal dismissed()

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
            text: picker.labels.length === 0 ? picker.emptyText : ""
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
            model: picker.labels

            delegate: ItemDelegate {
                required property var modelData
                required property int index
                width: ListView.view.width
                text: modelData
                onClicked: picker.chosen(index)
            }
        }

        Row {
            spacing: 8
            Button {
                objectName: "cancelPicker"
                text: "Cancel"
                onClicked: picker.dismissed()
            }
        }
    }

    onClosed: picker.dismissed()
}
