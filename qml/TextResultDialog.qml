import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import tidyshot

// Recognized-text dialog: review or edit OCR/QR output, copy it, and open a
// detected link explicitly. The TextResultDialog singleton owns the state and
// all link decisions; this file only lays it out.
Dialog {
    id: root

    property var links: TextResultDialog

    title: "tidyshot \u2014 Recognized Text"
    modal: false
    width: 600
    height: 400
    standardButtons: Dialog.Close
    visible: false

    ColumnLayout {
        anchors.fill: parent
        spacing: 12

        Label {
            Layout.fillWidth: true
            text: TextResultDialog.status
            wrapMode: Text.WordWrap
        }

        ScrollView {
            Layout.fillWidth: true
            Layout.fillHeight: true

            TextArea {
                id: editor
                objectName: "recognizedText"
                text: TextResultDialog.text
                wrapMode: TextArea.Wrap
                selectByMouse: true
                onTextChanged: TextResultDialog.updateText(text)
            }
        }

        Label {
            Layout.fillWidth: true
            visible: TextResultDialog.linkLabels.length > 0
            text: "Detected links"
        }

        ListView {
            id: linkList
            Layout.fillWidth: true
            Layout.preferredHeight: 120
            visible: TextResultDialog.linkLabels.length > 0
            model: TextResultDialog.linkLabels
            clip: true

            delegate: ItemDelegate {
                width: ListView.view.width
                text: modelData
                onClicked: linkList.currentIndex = index
            }
        }

        RowLayout {
            Layout.fillWidth: true

            Button {
                text: "Copy Text"
                enabled: TextResultDialog.text.length > 0
                onClicked: TextResultDialog.copy()
            }

            Button {
                objectName: "openLink"
                text: "Open Link"
                visible: TextResultDialog.canOpen
                enabled: TextResultDialog.canOpen
                onClicked: {
                    var target = TextResultDialog.linkUrl(linkList.currentIndex)
                    if (target.length > 0)
                        Qt.openUrlExternally(target)
                }
            }
        }
    }
}
