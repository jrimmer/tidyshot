// Corner preview overlay for one capture or recording.
// Ported from omnishot/widgets.py (QuickOverlay, DragPreview)
//
// Layout only: the lifecycle, gesture thresholds and action set live in
// src/overlay.rs and are applied to this component through its properties.
// The component is deliberately free of the `tidyshot` module so it can be
// driven by qmltestrunner with synthetic events offscreen.
import QtQuick
import QtQuick.Controls

Rectangle {
    id: preview

    /// Title line, already elided for a renamed capture.
    property string title: "Capture ready"
    /// Dimensions and size text under the preview.
    property string info: ""
    /// The image to show, or the recording's first frame.
    property url contentUrl: ""
    /// True when the content is a recording.
    property bool video: false
    /// True while the body is hidden and only the restore control shows.
    property bool collapsed: false
    /// True while a drag is in progress.
    property bool dragging: false
    /// True while a copy is in flight.
    property bool copying: false
    /// Close after an accepted drag unless Alt was held.
    property bool closeAfterDrag: true
    /// Auto-close delay in milliseconds; zero keeps the preview open.
    property int autoCloseMs: 0
    /// "close" or "save" when the auto-close timer elapses.
    property string autoCloseAction: "close"
    /// Thumbnail width in pixels.
    property int overlaySize: 300
    /// Show the Move to Trash control for an automatically saved capture.
    property bool trashVisible: false
    /// The `text/uri-list` body a drag-out carries, from the Rust side.
    property string dragUri: ""

    /// Open the annotate editor (or the video editor).
    signal activate()
    signal copyRequested()
    signal saveRequested()
    signal pinRequested()
    signal saveAllRequested()
    signal closeAllRequested()
    /// A format id from the export menu.
    signal exportRequested(string format)
    /// Open the file in the desktop's default application.
    signal openExternalRequested()
    signal trashRequested()
    /// The preview closed by any path; carries the file path.
    signal dismissed()

    width: Math.max(335, overlaySize + 20)
    color: "#242424"
    radius: 8
    border.color: "#3a3a3a"

    function requestDismiss() {
        autoTimer.stop()
        dismissed()
    }

    /// Apply an elapsed auto-close: save first when configured, then dismiss.
    function autoClose() {
        if (copying || dragging)
            return
        if (autoCloseAction === "save")
            saveRequested()
        requestDismiss()
    }

    function setCollapsed(value) {
        collapsed = value
        if (!value && autoCloseMs > 0)
            autoTimer.restart()
    }

    function beginDrag() {
        dragging = true
        autoTimer.stop()
    }

    function finishDrag(accepted, keep) {
        dragging = false
        if (accepted && closeAfterDrag && !keep)
            requestDismiss()
        else if (autoCloseMs > 0)
            autoTimer.restart()
    }

    Column {
        anchors.fill: parent
        anchors.margins: 10
        spacing: 6
        visible: !preview.collapsed

        Row {
            width: parent.width
            spacing: 4
            Text {
                objectName: "titleLabel"
                text: preview.title
                color: "white"
                font.bold: true
            }
            Item { width: 1; height: 1 }
            // QuickOverlay's header chip: a 36x36 trash control.
            Button {
                objectName: "trashButton"
                visible: preview.trashVisible
                text: "🗑"
                font.pixelSize: 16
                implicitWidth: 36
                implicitHeight: 36
                padding: 0
                Accessible.name: qsTr("Move to Trash")
                ToolTip.visible: hovered
                ToolTip.text: qsTr("Move capture and automatically saved file to Trash")
                onClicked: preview.trashRequested()
            }
            Button {
                objectName: "closeButton"
                text: "×"
                onClicked: preview.requestDismiss()
            }
        }

        Rectangle {
            id: dragHandle
            objectName: "dragHandle"
            width: preview.overlaySize
            height: 150
            color: "#303030"
            Image {
                anchors.fill: parent
                fillMode: Image.PreserveAspectFit
                source: preview.contentUrl
                asynchronous: true
            }
            Drag.active: dragArea.drag.active
            Drag.hotSpot.x: 10
            Drag.hotSpot.y: 10
            // A drag offers the same file list a paste does; the Rust side
            // builds the URL so an unusual filename is percent-encoded once.
            Drag.mimeData: { "text/uri-list": preview.dragUri !== ""
                ? preview.dragUri : preview.contentUrl }
            // A Wayland receiver can accept a drop while Qt reports a
            // cancelled drag, so the drop action alone is not definitive.
            // Every action other than Ignore counts as accepted here.
            Drag.onDragStarted: dragArea.dragStarted = true
            Drag.onDragFinished: function (dropAction) {
                preview.finishDrag(dropAction !== Qt.IgnoreAction, dragArea.altHeld)
                dragArea.altHeld = false
                dragArea.dragStarted = false
            }
            MouseArea {
                id: dragArea
                property bool altHeld: false
                // A started drag always ends through Drag.onDragFinished, so
                // the release that follows must not report a second finish.
                property bool dragStarted: false
                anchors.fill: parent
                drag.target: parent
                onPressed: function (mouse) {
                    altHeld = (mouse.modifiers & Qt.AltModifier) !== 0
                    dragStarted = false
                    preview.beginDrag()
                }
                // A press without a drag never finishes through Drag; release
                // it here so the preview is not left mid-drag.
                onReleased: {
                    if (!dragStarted) {
                        altHeld = false
                        preview.finishDrag(false, false)
                    }
                }
                onClicked: preview.activate()
            }
        }

        Text {
            objectName: "infoLabel"
            text: preview.info
            color: "#9a9a9a"
        }

        // A Flow keeps every action reachable when the preview is narrow:
        // Export, Open, Save All and Close All wrap onto a second line.
        Flow {
            width: parent.width
            spacing: 6
            Button {
                objectName: "annotateButton"
                text: preview.video ? "Edit" : "Annotate"
                onClicked: preview.activate()
            }
            Button {
                objectName: "copyButton"
                text: "Copy"
                onClicked: preview.copyRequested()
            }
            Button {
                objectName: "saveButton"
                text: "Save"
                onClicked: preview.saveRequested()
            }
            Button {
                objectName: "pinButton"
                visible: !preview.video
                text: "Pin"
                onClicked: preview.pinRequested()
            }
            Button {
                objectName: "exportButton"
                text: "Export"
                onClicked: exportMenu.open()
            }
            Button {
                objectName: "openButton"
                text: "Open"
                onClicked: preview.openExternalRequested()
            }
            Button {
                objectName: "saveAllButton"
                text: "Save All"
                onClicked: preview.saveAllRequested()
            }
            Button {
                objectName: "closeAllButton"
                text: "Close All"
                onClicked: preview.closeAllRequested()
            }
        }

        Menu {
            id: exportMenu
            objectName: "exportMenu"
            MenuItem {
                objectName: "exportPng"
                text: "PNG"
                onTriggered: preview.exportRequested("png")
            }
            MenuItem {
                objectName: "exportJpeg"
                text: "JPEG"
                onTriggered: preview.exportRequested("jpeg")
            }
            MenuItem {
                objectName: "exportWebp"
                text: "WebP"
                onTriggered: preview.exportRequested("webp")
            }
            MenuItem {
                objectName: "exportHeic"
                text: "HEIC"
                onTriggered: preview.exportRequested("heic")
            }
            MenuItem {
                objectName: "exportDefault"
                text: "Default format"
                onTriggered: preview.exportRequested("")
            }
        }
    }

    Button {
        objectName: "restoreButton"
        visible: preview.collapsed
        anchors.left: parent.left
        anchors.bottom: parent.bottom
        anchors.margins: 10
        text: "↑ Show capture"
        onClicked: preview.setCollapsed(false)
    }

    Timer {
        id: autoTimer
        interval: preview.autoCloseMs
        running: preview.autoCloseMs > 0
        repeat: false
        onTriggered: preview.autoClose()
    }
}
