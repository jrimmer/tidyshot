// Capture history: a newest-first filmstrip with type tabs, search, selection
// and the delete/restore/annotate/pin actions.
//
// Layout only: the listing, filters, search and trash move live in
// src/history_window.rs, bound here through the HistoryApi singleton. The
// component is free of the `tidyshot` module so qmltestrunner can drive it
// offscreen with synthetic events.
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls

Window {
    id: historyWindow

    /// One JSON row per capture, newest first.
    property var rows: []
    /// The current type tab id.
    property string filter: "all"
    /// The current search text.
    property string query: ""
    /// The last action's line, including an I/O failure.
    property string message: ""
    /// The keyboard cursor row.
    property int cursor: 0
    /// How many rows are selected.
    property int selected: 0
    /// Parallel type tab ids and labels.
    property var tabIds: ["all", "image", "scroll", "video"]
    property var tabLabels: ["All", "Images", "Scroll", "Video"]

    signal filterChosen(string id)
    signal searchChanged(string text)
    signal moved(int delta)
    signal rowSelected(int index, bool extend)
    signal rowActivated(int index)
    signal deleteRequested()
    signal clearRequested()
    signal restoreRequested()
    signal annotateRequested()
    signal pinRequested()
    signal dismissed()

    width: 900
    height: 460
    visible: false
    title: qsTr("TidyShot History")
    color: "#1e1e1e"
    flags: Qt.Window

    /// The thumbnail a row shows; a missing file falls back to the icon.
    function rowSource(row) {
        return row && row.source ? row.source : ""
    }

    /// Apply one Qt key code, so a key event and a test drive one map.
    function handleKey(key) {
        if (key === Qt.Key_H || key === Qt.Key_Left || key === Qt.Key_K || key === Qt.Key_Up) {
            historyWindow.moved(-1)
            return true
        }
        if (key === Qt.Key_L || key === Qt.Key_Right || key === Qt.Key_J || key === Qt.Key_Down) {
            historyWindow.moved(1)
            return true
        }
        if (key === Qt.Key_Return || key === Qt.Key_Enter) {
            historyWindow.rowActivated(historyWindow.cursor)
            return true
        }
        if (key === Qt.Key_Delete) {
            historyWindow.deleteRequested()
            return true
        }
        if (key === Qt.Key_Escape) {
            historyWindow.dismissed()
            return true
        }
        return false
    }

    /// Give the key map the window's keyboard focus.
    function takeKeyFocus() {
        historyWindow.requestActivate()
        keys.forceActiveFocus()
    }

    onVisibleChanged: {
        if (visible)
            historyWindow.takeKeyFocus()
    }

    Component.onCompleted: {
        if (visible)
            historyWindow.takeKeyFocus()
    }

    // Qt refuses a Keys handler attached directly to a Window, so the map
    // lives in a focus scope filling the content and claims activation when
    // the window shows; h/j/k/l, arrows, Enter, Delete and Escape work without
    // a first click.
    FocusScope {
        id: keys
        anchors.fill: parent
        focus: historyWindow.visible
        Keys.onPressed: function (event) {
            // The search box keeps its own keys, including h/j/k/l, so typing
            // a name never moves the filmstrip cursor.
            if (searchField.activeFocus)
                return
            if (historyWindow.handleKey(event.key))
                event.accepted = true
        }

        Column {
            anchors.fill: parent
            anchors.margins: 12
            spacing: 8

            Row {
                spacing: 8

                Repeater {
                    model: historyWindow.tabIds.length

                    Button {
                        required property int index
                        objectName: "historyTab_" + historyWindow.tabIds[index]
                        text: historyWindow.tabLabels[index]
                        checkable: true
                        checked: historyWindow.filter === historyWindow.tabIds[index]
                        onClicked: historyWindow.filterChosen(historyWindow.tabIds[index])
                    }
                }

                TextField {
                    id: searchField
                    objectName: "historySearch"
                    width: 240
                    placeholderText: "Search captures…"
                    text: historyWindow.query
                    onTextEdited: historyWindow.searchChanged(text)
                }

                Item { width: 1; height: 1 }

                Button {
                    objectName: "historyClear"
                    text: "Clear History"
                    onClicked: historyWindow.clearRequested()
                }
                Button {
                    objectName: "historyClose"
                    text: "×"
                    onClicked: historyWindow.dismissed()
                }
            }

            Text {
                objectName: "historyMessage"
                width: parent.width
                text: historyWindow.message
                color: "#FFD60A"
                wrapMode: Text.WordWrap
                visible: text !== ""
            }

            ListView {
                id: filmstrip
                objectName: "filmstrip"
                width: parent.width
                height: parent.height - 130
                orientation: ListView.Horizontal
                spacing: 8
                clip: true
                model: historyWindow.rows

                delegate: Rectangle {
                    required property var modelData
                    required property int index
                    objectName: "historyRow_" + index
                    width: 190
                    height: ListView.view.height - 8
                    radius: 6
                    color: historyWindow.cursor === index ? "#333333" : "#242424"
                    border.width: modelData.selected ? 2 : 1
                    border.color: modelData.selected ? "#FFD60A" : "#3a3a3a"

                    Column {
                        anchors.fill: parent
                        anchors.margins: 6
                        spacing: 4

                        Rectangle {
                            width: parent.width
                            height: parent.height - 48
                            color: "#303030"
                            Image {
                                objectName: "historyThumb_" + index
                                anchors.fill: parent
                                fillMode: Image.PreserveAspectFit
                                asynchronous: true
                                source: historyWindow.rowSource(modelData)
                            }
                        }

                        Text {
                            objectName: "historyName_" + index
                            width: parent.width
                            text: modelData.name
                            color: "white"
                            elide: Text.ElideMiddle
                        }

                        Text {
                            objectName: "historyKind_" + index
                            text: modelData.kind + " · " + modelData.width + " × " + modelData.height
                            color: "#9a9a9a"
                            font.pixelSize: 11
                        }
                    }

                    MouseArea {
                        anchors.fill: parent
                        acceptedButtons: Qt.LeftButton
                        onClicked: function (mouse) {
                            historyWindow.rowSelected(index, (mouse.modifiers & Qt.ControlModifier) !== 0)
                        }
                        onDoubleClicked: historyWindow.rowActivated(index)
                    }
                }
            }

            Row {
                spacing: 8

                Button {
                    objectName: "historyRestore"
                    text: "Restore preview"
                    onClicked: historyWindow.restoreRequested()
                }
                Button {
                    objectName: "historyAnnotate"
                    text: "Annotate"
                    onClicked: historyWindow.annotateRequested()
                }
                Button {
                    objectName: "historyPin"
                    text: "Pin"
                    onClicked: historyWindow.pinRequested()
                }
                Button {
                    objectName: "historyDelete"
                    text: "Delete"
                    onClicked: historyWindow.deleteRequested()
                }
            }
        }
    }
}
