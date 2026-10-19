// Frameless Quick Access preview window for one completed capture.
// Ported from omnishot/widgets.py (QuickOverlay window placement)
//
// The window owns only its chrome and key bindings; the Rust `Preview`
// singleton supplies the position, settings and actions. Overlay.qml lays out
// the body, so the same preview component serves the tests and the app.
import QtQuick

Window {
    id: previewWindow

    /// Whether the preview is showing.
    property bool open: false
    /// The capture's dimensions.
    property string info: ""
    /// The saved file to show.
    property url contentUrl: ""
    /// True when the file is a recording.
    property bool video: false
    /// Auto-close delay in milliseconds; zero keeps the preview open.
    property int autoCloseMs: 0
    /// "close" or "save" when the auto-close timer elapses.
    property string autoCloseAction: "close"
    /// The outcome of the last action, including an I/O failure.
    property string message: ""
    /// Close after an accepted drag unless Alt was held.
    property bool closeAfterDrag: true
    /// Claim activation when the window opens; a command-line capture has no
    /// menu behind it, so Enter and Escape work without a first click.
    property bool takeFocus: false

    signal annotateRequested()
    signal copyRequested()
    signal saveRequested()
    signal pinRequested()
    signal dismissed()
    /// The body was collapsed or restored; the Rust side adjusts the anchor.
    signal collapseRequested(bool collapsed)

    width: 355
    height: 300
    visible: open
    color: "transparent"
    title: "TidyShot Preview"
    flags: Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint | Qt.Tool

    // The shortcuts are scoped to this window, so a capture with no menu
    // behind it takes activation and its keys work straight away. A preview
    // opened from the menu leaves focus there.
    onOpenChanged: {
        if (open && takeFocus)
            previewWindow.requestActivate()
    }

    // Enter copies and Escape closes while this window is active. The scope
    // is the preview window, not the application: a menu window in front must
    // keep its own Return and Escape handling.
    Shortcut {
        objectName: "copyShortcut"
        sequences: ["Return", "Enter"]
        context: Qt.WindowShortcut
        enabled: previewWindow.open && previewWindow.active
        onActivated: previewWindow.copyRequested()
    }
    Shortcut {
        objectName: "closeShortcut"
        sequences: ["Escape"]
        context: Qt.WindowShortcut
        enabled: previewWindow.open && previewWindow.active
        onActivated: previewWindow.dismissed()
    }

    Overlay {
        id: overlay
        anchors.fill: parent
        title: previewWindow.title
        info: previewWindow.info
        contentUrl: previewWindow.contentUrl
        video: previewWindow.video
        autoCloseMs: previewWindow.autoCloseMs
        autoCloseAction: previewWindow.autoCloseAction
        closeAfterDrag: previewWindow.closeAfterDrag
        onActivate: previewWindow.annotateRequested()
        onCopyRequested: previewWindow.copyRequested()
        onSaveRequested: previewWindow.saveRequested()
        onPinRequested: previewWindow.pinRequested()
        onDismissed: previewWindow.dismissed()
    }

    Connections {
        target: overlay
        function onCollapsedChanged() {
            previewWindow.collapseRequested(overlay.collapsed)
        }
    }

    Text {
        objectName: "previewMessage"
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        anchors.margins: 10
        text: previewWindow.message
        color: "#FFD60A"
        wrapMode: Text.WordWrap
        visible: text !== ""
    }
}
