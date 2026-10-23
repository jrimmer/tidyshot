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
    onClosed: TextOutput.dismiss()

    // The completion path sets TextOutput.shown once a result is ready. Open
    // and close through methods rather than a binding on `visible`, because
    // Dialog.close() writes `visible` and would remove a declarative binding.
    Connections {
        target: TextOutput
        function onShownChanged() {
            if (TextOutput.shown)
                root.open()
            else
                root.close()
        }
    }
    Component.onCompleted: {
        TextOutput.bind()
        if (TextOutput.shown)
            root.open()
    }

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
                wrapMode: TextArea.Wrap
                selectByMouse: true
                // Only a user edit transitions to the edited state; a
                // programmatic result must keep its own status and copy flag.
                onTextEdited: TextOutput.updateText(text)
            }

            // The editor's text binding is replaced on the first user edit, so
            // reload the recognized text whenever the singleton supplies a new
            // result without disturbing an edit already in the box.
            Binding {
                target: editor
                property: "text"
                value: TextOutput.text
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
