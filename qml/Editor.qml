// The annotate editor window: canvas, toolbar and text entry.
// Ported from omnishot/editor.py (EditorWindow) and omnishot/editor_toolbar.py
//
// Rust owns the document, the renderer and undo; this window only maps pointer
// and key events onto the `Editor` singleton and draws its overlays. The
// canvas pixels come from the `image://tidyshot/canvas` provider, so a render
// happens once per document revision instead of once per frame.
import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import tidyshot

Window {
    id: editorWindow

    /// Mirrors the singleton so bindings survive a property notification the
    /// cxx-qt bridge does not deliver to QML.
    property bool open: false
    property bool standalone: false
    property bool chooserOpen: false
    property string documentTitle: "TidyShot Editor"
    property string path: ""
    property string message: ""
    property bool dirty: false
    property int revision: 0
    property int canvasWidth: 0
    property int canvasHeight: 0
    property var toolIds: []
    property var toolLabels: []
    property var toolShortcuts: []
    property string tool: "select"
    property string annotationColor: "#ff5b61"
    property double stroke: 4
    property bool canUndo: false
    property bool canRedo: false
    property bool cropActive: false
    property double cropX: 0
    property double cropY: 0
    property double cropW: 0
    property double cropH: 0
    property bool textActive: false
    property bool hasSelection: false
    property double selectionX: 0
    property double selectionY: 0
    property double selectionW: 0
    property double selectionH: 0
    property bool provisional: false
    property string provisionalKind: ""
    property double provisionalX: 0
    property double provisionalY: 0
    property double provisionalW: 0
    property double provisionalH: 0
    property double textX: 0
    property double textY: 0
    property double textW: 0
    property double textH: 0
    property string saveSuggestion: ""
    property double uiScale: 1

    /// Ask before a close would discard unsaved annotations.
    property bool discardPrompt: false
    property bool saveAsPrompt: false

    width: Math.max(640, 720 * uiScale)
    height: Math.max(480, 560 * uiScale)
    visible: open
    title: editorWindow.documentTitle
    color: "#1e1e1e"
    flags: Qt.Window

    function px(value) {
        return Math.max(1, Math.round(value * editorWindow.uiScale))
    }

    /// Map a pointer position inside the canvas item to image coordinates.
    function canvasPoint(item, x, y) {
        var w = item.paintedWidth
        var h = item.paintedHeight
        if (w <= 0 || h <= 0 || editorWindow.canvasWidth <= 0)
            return Qt.point(0, 0)
        var left = (item.width - w) / 2
        var top = (item.height - h) / 2
        return Qt.point(
            Math.max(0, Math.min(editorWindow.canvasWidth, (x - left) * editorWindow.canvasWidth / w)),
            Math.max(0, Math.min(editorWindow.canvasHeight, (y - top) * editorWindow.canvasHeight / h)))
    }

    /// Save over the opened file, or ask for a path when there is none.
    function saveOrAsk() {
        if (editorWindow.path === "")
            editorWindow.askSaveAs()
        else
            EditorApi.save()
    }

    /// Open the Save As dialog at the current file or a suggested name.
    function askSaveAs() {
        saveDialog.selectedFile = editorWindow.saveSuggestion !== ""
            ? editorWindow.saveSuggestion
            : editorWindow.path
        editorWindow.saveAsPrompt = true
        saveDialog.open()
    }

    Component.onCompleted: {
        EditorApi.bind()
        canvasFrame.forceActiveFocus()
    }

    // cxx-qt singleton notifications do not reach QML bindings here, so the
    // window is fed from the singleton on a short timer.
    Timer {
        id: editorSync
        interval: 60
        repeat: true
        running: true
        onTriggered: {
            editorWindow.open = EditorApi.open
            editorWindow.standalone = EditorApi.standalone
            editorWindow.chooserOpen = EditorApi.chooserOpen
            editorWindow.documentTitle = EditorApi.title
            editorWindow.path = EditorApi.path
            editorWindow.message = EditorApi.message
            editorWindow.dirty = EditorApi.dirty
            editorWindow.revision = EditorApi.revision
            editorWindow.canvasWidth = EditorApi.canvasWidth
            editorWindow.canvasHeight = EditorApi.canvasHeight
            editorWindow.toolIds = EditorApi.toolIds
            editorWindow.toolLabels = EditorApi.toolLabels
            editorWindow.toolShortcuts = EditorApi.toolShortcuts
            editorWindow.tool = EditorApi.tool
            editorWindow.annotationColor = EditorApi.color
            editorWindow.stroke = EditorApi.stroke
            editorWindow.canUndo = EditorApi.canUndo
            editorWindow.canRedo = EditorApi.canRedo
            editorWindow.cropActive = EditorApi.cropActive
            editorWindow.cropX = EditorApi.cropX
            editorWindow.cropY = EditorApi.cropY
            editorWindow.cropW = EditorApi.cropW
            editorWindow.cropH = EditorApi.cropH
            editorWindow.textActive = EditorApi.textActive
            editorWindow.hasSelection = EditorApi.hasSelection
            editorWindow.selectionX = EditorApi.selectionX
            editorWindow.selectionY = EditorApi.selectionY
            editorWindow.selectionW = EditorApi.selectionW
            editorWindow.selectionH = EditorApi.selectionH
            editorWindow.provisional = EditorApi.provisional
            editorWindow.provisionalKind = EditorApi.provisionalKind
            editorWindow.provisionalX = EditorApi.provisionalX
            editorWindow.provisionalY = EditorApi.provisionalY
            editorWindow.provisionalW = EditorApi.provisionalW
            editorWindow.provisionalH = EditorApi.provisionalH
            editorWindow.textX = EditorApi.textX
            editorWindow.textY = EditorApi.textY
            editorWindow.textW = EditorApi.textW
            editorWindow.textH = EditorApi.textH
            editorWindow.saveSuggestion = EditorApi.saveSuggestion
            editorWindow.uiScale = EditorApi.uiScale
        }
    }

    // A close with unsaved work asks first; the discard prompt owns the
    // decision so an accidental Alt+F4 cannot lose annotations.
    onClosing: function (close) {
        if (editorWindow.dirty) {
            close.accepted = false
            editorWindow.discardPrompt = true
        } else {
            EditorApi.discard()
        }
    }

    // A command-line `open` starts only the editor; once it closes there is
    // nothing left to show, so the process exits instead of idling invisible.
    onOpenChanged: {
        if (!editorWindow.open && editorWindow.standalone)
            Qt.quit()
    }

    Shortcut {
        objectName: "undoShortcut"
        sequences: ["Ctrl+Z"]
        context: Qt.WindowShortcut
        enabled: editorWindow.open
        onActivated: EditorApi.undo()
    }
    Shortcut {
        objectName: "redoShortcut"
        sequences: ["Ctrl+Shift+Z"]
        context: Qt.WindowShortcut
        enabled: editorWindow.open
        onActivated: EditorApi.redo()
    }
    Shortcut {
        objectName: "saveShortcut"
        sequences: ["Ctrl+S"]
        context: Qt.WindowShortcut
        enabled: editorWindow.open
        onActivated: editorWindow.saveOrAsk()
    }
    Shortcut {
        objectName: "copyShortcut"
        sequences: ["Ctrl+C"]
        context: Qt.WindowShortcut
        enabled: editorWindow.open && !textInput.activeFocus
        onActivated: EditorApi.copy()
    }
    Shortcut {
        objectName: "cancelShortcut"
        sequences: ["Escape"]
        context: Qt.WindowShortcut
        enabled: editorWindow.open
        onActivated: {
            if (editorWindow.textActive) {
                EditorApi.cancelText()
                textInput.text = ""
            } else if (editorWindow.cropActive) {
                EditorApi.cancelCrop()
            } else {
                EditorApi.key("Escape", false, false)
            }
        }
    }

    Column {
        anchors.fill: parent
        anchors.margins: editorWindow.px(8)
        spacing: editorWindow.px(8)

        EditorToolbar {
            id: toolbar
            width: parent.width
            toolIds: editorWindow.toolIds
            toolLabels: editorWindow.toolLabels
            toolShortcuts: editorWindow.toolShortcuts
            currentTool: editorWindow.tool
            annotationColor: editorWindow.annotationColor
            stroke: editorWindow.stroke
            canUndo: editorWindow.canUndo
            canRedo: editorWindow.canRedo
            cropActive: editorWindow.cropActive
            uiScale: editorWindow.uiScale
            onToolChosen: function (id) { EditorApi.chooseTool(id) }
            onColorChosen: function (value) { EditorApi.chooseColor(value) }
            onStrokeChosen: function (value) { EditorApi.chooseStroke(value) }
            onUndoRequested: EditorApi.undo()
            onRedoRequested: EditorApi.redo()
            onSaveRequested: editorWindow.saveOrAsk()
            onSaveAsRequested: editorWindow.askSaveAs()
            onCopyRequested: EditorApi.copy()
            onDoneRequested: editorWindow.close()
            onApplyCropRequested: EditorApi.applyCrop()
            onCancelCropRequested: EditorApi.cancelCrop()
        }

        Item {
            id: canvasFrame
            // Plain letters arm their tool through the same key map the Rust
            // side tests; the text box keeps its own typing because it has
            // focus while it is open.
            Keys.onPressed: function (event) {
                if (event.text === "")
                    return
                var handled = EditorApi.key(event.text,
                                            (event.modifiers & Qt.ControlModifier) !== 0,
                                            (event.modifiers & Qt.ShiftModifier) !== 0)
                if (handled)
                    event.accepted = true
            }
            width: parent.width
            height: parent.height - toolbar.height - statusLabel.height - editorWindow.px(24)
            clip: true

            Image {
                id: canvas
                objectName: "canvasImage"
                anchors.fill: parent
                fillMode: Image.PreserveAspectFit
                cache: false
                asynchronous: false
                source: editorWindow.canvasWidth > 0
                        ? "image://tidyshot/canvas?" + editorWindow.revision
                        : ""
            }

            // A released crop stays on screen until Apply or Escape, so the
            // user can see the region before it changes the canvas.
            Rectangle {
                objectName: "cropOverlay"
                visible: editorWindow.cropActive
                color: "#33ffd60a"
                border.color: "#ffd60a"
                border.width: editorWindow.px(2)
                x: canvas.x + (canvas.width - canvas.paintedWidth) / 2
                   + editorWindow.cropX * canvas.paintedWidth / Math.max(1, editorWindow.canvasWidth)
                y: canvas.y + (canvas.height - canvas.paintedHeight) / 2
                   + editorWindow.cropY * canvas.paintedHeight / Math.max(1, editorWindow.canvasHeight)
                width: editorWindow.cropW * canvas.paintedWidth / Math.max(1, editorWindow.canvasWidth)
                height: editorWindow.cropH * canvas.paintedHeight / Math.max(1, editorWindow.canvasHeight)
            }

            // The provisional shape follows the pointer; the document render
            // only changes when the drag finishes.
            Rectangle {
                objectName: "provisionalShape"
                visible: editorWindow.provisional && editorWindow.provisionalKind !== "crop"
                color: editorWindow.provisionalKind === "crop" ? "#33ffd60a" : "#22000000"
                border.color: editorWindow.provisionalKind === "crop" ? "#ffd60a" : editorWindow.annotationColor
                border.width: editorWindow.px(2)
                x: canvas.x + (canvas.width - canvas.paintedWidth) / 2
                   + editorWindow.provisionalX * canvas.paintedWidth / Math.max(1, editorWindow.canvasWidth)
                y: canvas.y + (canvas.height - canvas.paintedHeight) / 2
                   + editorWindow.provisionalY * canvas.paintedHeight / Math.max(1, editorWindow.canvasHeight)
                width: editorWindow.provisionalW * canvas.paintedWidth / Math.max(1, editorWindow.canvasWidth)
                height: editorWindow.provisionalH * canvas.paintedHeight / Math.max(1, editorWindow.canvasHeight)
            }

            Rectangle {
                objectName: "selectionOutline"
                visible: editorWindow.hasSelection && editorWindow.tool === "select"
                color: "transparent"
                border.color: "#60a5fa"
                border.width: editorWindow.px(1)
                x: canvas.x + (canvas.width - canvas.paintedWidth) / 2
                   + editorWindow.selectionX * canvas.paintedWidth / Math.max(1, editorWindow.canvasWidth)
                y: canvas.y + (canvas.height - canvas.paintedHeight) / 2
                   + editorWindow.selectionY * canvas.paintedHeight / Math.max(1, editorWindow.canvasHeight)
                width: editorWindow.selectionW * canvas.paintedWidth / Math.max(1, editorWindow.canvasWidth)
                height: editorWindow.selectionH * canvas.paintedHeight / Math.max(1, editorWindow.canvasHeight)
            }

            MouseArea {
                id: canvasMouse
                objectName: "canvasMouseArea"
                anchors.fill: parent
                cursorShape: editorWindow.tool === "select" ? Qt.ArrowCursor : Qt.CrossCursor
                enabled: !editorWindow.textActive
                onPressed: function (mouse) {
                    var point = editorWindow.canvasPoint(canvas, mouse.x, mouse.y)
                    EditorApi.press(point.x, point.y)
                }
                onPositionChanged: function (mouse) {
                    if (!pressed)
                        return
                    var point = editorWindow.canvasPoint(canvas, mouse.x, mouse.y)
                    EditorApi.moveTo(point.x, point.y)
                }
                onReleased: function (mouse) {
                    var point = editorWindow.canvasPoint(canvas, mouse.x, mouse.y)
                    EditorApi.release(point.x, point.y)
                }
            }

            // Text entry sits over the annotation rectangle in canvas space.
            Rectangle {
                objectName: "textEntry"
                visible: editorWindow.textActive
                color: "#242424"
                border.color: "#ffd60a"
                border.width: editorWindow.px(1)
                x: canvas.x + (canvas.width - canvas.paintedWidth) / 2
                   + editorWindow.textX * canvas.paintedWidth / Math.max(1, editorWindow.canvasWidth)
                y: canvas.y + (canvas.height - canvas.paintedHeight) / 2
                   + editorWindow.textY * canvas.paintedHeight / Math.max(1, editorWindow.canvasHeight)
                width: Math.max(editorWindow.px(160),
                                editorWindow.textW * canvas.paintedWidth / Math.max(1, editorWindow.canvasWidth))
                height: Math.max(editorWindow.px(48),
                                 editorWindow.textH * canvas.paintedHeight / Math.max(1, editorWindow.canvasHeight))

                TextArea {
                    id: textInput
                    objectName: "textInput"
                    anchors.fill: parent
                    anchors.margins: editorWindow.px(4)
                    wrapMode: TextArea.Wrap
                    placeholderText: "Type, then Ctrl+Enter"
                    font.pixelSize: editorWindow.px(14)
                    onVisibleChanged: {
                        if (visible) {
                            text = ""
                            forceActiveFocus()
                        }
                    }
                    Keys.onPressed: function (event) {
                        if (event.key === Qt.Key_Return && (event.modifiers & Qt.ControlModifier)) {
                            EditorApi.commitText(text)
                            text = ""
                            event.accepted = true
                        } else if (event.key === Qt.Key_Escape) {
                            EditorApi.cancelText()
                            text = ""
                            event.accepted = true
                        }
                    }
                }
            }
        }

        Text {
            id: statusLabel
            objectName: "editorStatus"
            width: parent.width
            text: editorWindow.message
            color: "#FFD60A"
            wrapMode: Text.WordWrap
            font.pixelSize: editorWindow.px(12)
            visible: text !== ""
        }
    }

    FileDialog {
        id: openDialog
        objectName: "openDialog"
        title: "Open an image or project"
        nameFilters: ["Images and projects (*.png *.jpg *.jpeg *.webp *.omnishot)", "All files (*)"]
        onAccepted: {
            EditorApi.openUrl(selectedFile)
            EditorApi.closeChooser()
        }
        onRejected: EditorApi.closeChooser()
    }

    FileDialog {
        id: saveDialog
        objectName: "saveDialog"
        title: "Save the annotated image"
        fileMode: FileDialog.SaveFile
        defaultSuffix: "png"
        nameFilters: ["PNG image (*.png)", "Editable project (*.omnishot)"]
        onAccepted: {
            EditorApi.saveUrl(selectedFile)
            editorWindow.saveAsPrompt = false
        }
        onRejected: editorWindow.saveAsPrompt = false
    }

    // Opening from the preview, a command line or after a capture lands here.
    onChooserOpenChanged: {
        if (editorWindow.chooserOpen) {
            openDialog.selectedFile = editorWindow.path !== ""
                ? editorWindow.path
                : editorWindow.saveSuggestion
            openDialog.open()
        }
    }

    Dialog {
        id: discardDialog
        objectName: "discardDialog"
        title: "Discard unsaved annotations?"
        modal: true
        standardButtons: Dialog.Discard | Dialog.Cancel
        anchors.centerIn: parent
        onDiscarded: {
            EditorApi.discard()
            editorWindow.discardPrompt = false
            if (editorWindow.standalone)
                Qt.quit()
            else
                editorWindow.close()
        }
        onRejected: editorWindow.discardPrompt = false
        onAccepted: editorWindow.discardPrompt = false
    }

    onDiscardPromptChanged: {
        if (editorWindow.discardPrompt)
            discardDialog.open()
        else
            discardDialog.close()
    }
}
