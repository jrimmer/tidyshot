pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import tidyshot

// Recognized-text dialog: review or edit OCR/QR output, copy it, and open a
// detected link explicitly. The TextOutput singleton owns the state and
// all link decisions; this file only lays it out and opens when asked.
Dialog {
    id: root

    title: "tidyshot \u2014 Recognized Text"
    modal: false
    width: 600
    height: 400
    standardButtons: Dialog.Close
    // The completion path sets TextOutput.shown once a result is ready.
    visible: TextOutput.shown
    onClosed: TextOutput.dismiss()

    ColumnLayout {
        anchors.fill: parent
        spacing: 12

        Label {
            Layout.fillWidth: true
            text: TextOutput.status
            wrapMode: Text.WordWrap
        }

        ScrollView {
            Layout.fillWidth: true
            Layout.fillHeight: true

            TextArea {
                id: editor
                objectName: "recognizedText"
                text: TextOutput.text
                wrapMode: TextArea.Wrap
                selectByMouse: true
                // Only a user edit transitions to the edited state; a
                // programmatic result must keep its own status and copy flag.
                onTextEdited: TextOutput.updateText(text)
            }
        }

        Label {
            Layout.fillWidth: true
            visible: TextOutput.linkCount > 0
            text: "Detected links"
        }

        ListView {
            id: linkList
            Layout.fillWidth: true
            Layout.preferredHeight: 120
            visible: TextOutput.linkCount > 0
            model: TextOutput.linkLabels
            clip: true

            onCountChanged: currentIndex = count > 0 ? 0 : -1

            delegate: ItemDelegate {
                required property var modelData
                required property int index
                width: ListView.view.width
                text: modelData
                onClicked: linkList.currentIndex = index
            }
        }

        RowLayout {
            Layout.fillWidth: true

            Button {
                text: "Copy Text"
                enabled: TextOutput.text !== ""
                onClicked: TextOutput.copy()
            }

            Button {
                objectName: "openLink"
                text: "Open Link"
                visible: TextOutput.canOpen
                enabled: TextOutput.canOpen
                onClicked: {
                    var target = TextOutput.linkUrl(linkList.currentIndex)
                    if (target !== "")
                        Qt.openUrlExternally(target)
                }
            }
        }
    }
}
