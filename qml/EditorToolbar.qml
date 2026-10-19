// Editor toolbar: tools, color, stroke and the document actions.
// Ported from omnishot/editor_toolbar.py (TOOLS)
//
// Layout only. The tool table and every action live in src/editor_ui.rs and
// src/editor_window.rs; this component is free of the `tidyshot` module so it
// can be driven by qmltestrunner offscreen.
import QtQuick
import QtQuick.Controls

Rectangle {
    id: toolbar

    /// Tool ids, labels and single-key shortcuts, in toolbar order.
    property var toolIds: []
    property var toolLabels: []
    property var toolShortcuts: []
    /// The armed tool id.
    property string currentTool: "select"
    /// The annotation color and stroke width.
    property string annotationColor: "#ff5b61"
    property double stroke: 4
    /// Whether undo and redo can run.
    property bool canUndo: false
    property bool canRedo: false
    /// Whether a provisional crop is waiting for Apply.
    property bool cropActive: false
    /// The Omarchy text scale applied to every explicit measurement.
    property double uiScale: 1

    /// A tool was chosen.
    signal toolChosen(string id)
    signal colorChosen(string color)
    signal strokeChosen(double stroke)
    signal undoRequested()
    signal redoRequested()
    signal saveRequested()
    signal saveAsRequested()
    signal copyRequested()
    signal doneRequested()
    signal applyCropRequested()
    signal cancelCropRequested()

    /// The annotation swatches, in the order color.rs lists them.
    readonly property var swatches: [
        "#15171d", "#ff5b61", "#ff8a34", "#ffbc42", "#53d49b",
        "#26c6be", "#60a5fa", "#b48bfa", "#ef4b9b", "#ffffff"
    ]

    implicitHeight: column.implicitHeight + 12 * uiScale
    color: "#242424"
    border.color: "#3a3a3a"
    radius: 6 * uiScale

    function px(value) {
        return Math.max(1, Math.round(value * toolbar.uiScale))
    }

    Column {
        id: column
        anchors.fill: parent
        anchors.margins: toolbar.px(6)
        spacing: toolbar.px(6)

        Row {
            spacing: toolbar.px(4)

            Repeater {
                model: toolbar.toolIds
                delegate: Button {
                    required property string modelData
                    required property int index
                    objectName: "tool_" + modelData
                    text: toolbar.toolLabels[index]
                    checkable: true
                    checked: toolbar.currentTool === modelData
                    font.pixelSize: toolbar.px(12)
                    // The shortcut stays visible so the keyboard path is
                    // discoverable without opening a menu.
                    ToolTip.visible: hovered
                    ToolTip.text: toolbar.toolLabels[index] + " (" + toolbar.toolShortcuts[index] + ")"
                    onClicked: toolbar.toolChosen(modelData)
                }
            }
        }

        Row {
            spacing: toolbar.px(4)

            Repeater {
                model: toolbar.swatches
                delegate: Rectangle {
                    required property string modelData
                    objectName: "swatch_" + modelData.substring(1)
                    width: toolbar.px(18)
                    height: toolbar.px(18)
                    radius: toolbar.px(3)
                    color: modelData
                    border.width: toolbar.annotationColor.toLowerCase() === modelData ? toolbar.px(2) : 0
                    border.color: "white"
                    MouseArea {
                        anchors.fill: parent
                        onClicked: toolbar.colorChosen(modelData)
                    }
                }
            }

            Slider {
                objectName: "strokeSlider"
                width: toolbar.px(110)
                from: 1
                to: 24
                stepSize: 1
                value: toolbar.stroke
                onMoved: toolbar.strokeChosen(value)
            }
        }

        Row {
            spacing: toolbar.px(4)

            Button {
                objectName: "undoButton"
                text: "Undo"
                font.pixelSize: toolbar.px(12)
                enabled: toolbar.canUndo
                onClicked: toolbar.undoRequested()
            }
            Button {
                objectName: "redoButton"
                text: "Redo"
                font.pixelSize: toolbar.px(12)
                enabled: toolbar.canRedo
                onClicked: toolbar.redoRequested()
            }
            Button {
                objectName: "copyButton"
                text: "Copy"
                font.pixelSize: toolbar.px(12)
                onClicked: toolbar.copyRequested()
            }
            Button {
                objectName: "saveButton"
                text: "Save"
                font.pixelSize: toolbar.px(12)
                onClicked: toolbar.saveRequested()
            }
            Button {
                objectName: "saveAsButton"
                text: "Save As…"
                font.pixelSize: toolbar.px(12)
                onClicked: toolbar.saveAsRequested()
            }
            Button {
                objectName: "doneButton"
                text: "Done"
                font.pixelSize: toolbar.px(12)
                onClicked: toolbar.doneRequested()
            }

            Button {
                objectName: "applyCropButton"
                visible: toolbar.cropActive
                text: "Apply Crop"
                font.pixelSize: toolbar.px(12)
                onClicked: toolbar.applyCropRequested()
            }
            Button {
                objectName: "cancelCropButton"
                visible: toolbar.cropActive
                text: "Cancel Crop"
                font.pixelSize: toolbar.px(12)
                onClicked: toolbar.cancelCropRequested()
            }
        }
    }
}
