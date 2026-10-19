import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import tidyshot

ApplicationWindow {
    id: window
    width: 480
    height: 620
    // A command-line capture starts only the corner preview; the menu window
    // stays out of the way and the process ends when that preview closes.
    visible: !Preview.standalone
    title: qsTr("tidyshot")
    color: "#1e1e1e"

    Material.theme: Material.Dark
    Material.accent: "#FFD60A"

    // Escape cancels a capture that is still choosing an area; the runtime
    // reports the cancellation on the same message line as a failure.
    Shortcut {
        sequences: ["Escape"]
        enabled: Capture.busy
        onActivated: Capture.cancel()
    }

    // The bar panel renders the same action table the tray menu and the
    // `tidyshot menu` listing render. Menu is the Rust side of that table;
    // MenuPanel stays free of it so the component lints with QtQuick alone.
    MenuPanel {
        id: menuPanel
        anchors.centerIn: parent
        visible: true
        ids: Menu.ids
        labels: Menu.labels
        shortcuts: Menu.shortcuts
        selectedIndex: Menu.selectedIndex
        onKeyPressed: function (key) {
            var event = Menu.handleKey(key)
            if (event === "dismissed")
                menuPanel.dismissed()
            else if (event === "activated")
                Menu.activate()
            else if (event === "tab-forward")
                menuPanel.tabbed(1)
            else if (event === "tab-back")
                menuPanel.tabbed(-1)
        }
        onActivated: function (index) {
            // Menu emits rowActivated for a capture row; Main forwards that
            // signal to the runtime, so the action table owns the dispatch.
            Menu.activateIndex(index)
        }
        onDismissed: Qt.quit()
    }

    // Hand the runtime these objects so forwarded and startup commands reach
    // them; a capture that completed before QML loaded is replayed here.
    Component.onCompleted: {
        Capture.bind()
        Preview.bind()

    }

    // The Rust action table emits the activated capture row; the runtime then
    // performs it on a worker thread and reports back through its own signals.
    Connections {
        target: Menu
        function onRowActivated(id) {
            Capture.captureRow(id)
        }
    }

    // State line: the current capture message, including an I/O failure.
    Label {
        id: statusLabel
        anchors.bottom: parent.bottom
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottomMargin: 28
        text: Capture.message
        color: Capture.path !== "" ? "#FFD60A" : "#9a9a9a"
        visible: text !== ""
    }

    Label {
        anchors.bottom: parent.bottom
        anchors.horizontalCenter: parent.horizontalCenter
        text: Args.label
        color: "#9a9a9a"
    }

    // cxx-qt singleton property notifications do not reach QML bindings in
    // this build, so the windows are fed from the singleton on a short timer.
    // Sixty milliseconds is below perception and costs one comparison pass.
    Timer {
        id: previewSync
        interval: 60
        repeat: true
        running: true
        onTriggered: {
            window.visible = !Preview.standalone
            previewWindow.open = Preview.open
            previewWindow.visible = Preview.open
            previewWindow.x = Preview.x
            previewWindow.y = Preview.y
            previewWindow.width = Preview.windowWidth
            previewWindow.height = Preview.windowHeight
            previewWindow.title = Preview.title
            previewWindow.info = Preview.info
            previewWindow.contentUrl = Preview.contentUrl
            previewWindow.video = Preview.video
            previewWindow.autoCloseMs = Preview.autoCloseMs
            previewWindow.autoCloseAction = Preview.autoCloseAction
            previewWindow.closeAfterDrag = Preview.closeAfterDrag
            previewWindow.takeFocus = Preview.standalone
            previewWindow.message = Preview.message
        }
    }

    // The frameless corner preview is its own always-on-top window, anchored
    // by the Rust runtime to the focused monitor after every capture. The
    // embedded overlay stays out of this window so a capture cannot cover
    // itself with the menu.
    PreviewWindow {
        id: previewWindow
        transientParent: null
        open: Preview.open
        x: Preview.x
        y: Preview.y
        width: Preview.windowWidth
        height: Preview.windowHeight
        title: Preview.title
        info: Preview.info
        contentUrl: Preview.contentUrl
        video: Preview.video
        autoCloseMs: Preview.autoCloseMs
        autoCloseAction: Preview.autoCloseAction
        closeAfterDrag: Preview.closeAfterDrag
        takeFocus: Preview.standalone
        message: Preview.message
        onAnnotateRequested: Preview.annotate()
        onCopyRequested: Preview.copy()
        onSaveRequested: Preview.save()
        onPinRequested: Preview.pin()
        onDismissed: Preview.dismiss()
        onCollapseRequested: function (collapsed) {
            Preview.collapseTo(collapsed)
        }
    }

    // A command-line capture has no menu window behind it; once its preview
    // is dismissed there is nothing left to show, so the process exits
    // instead of idling invisibly.
    Connections {
        target: Preview
        function onOpenChanged() {

            if (!Preview.open && Preview.standalone)
                Qt.quit()
        }
    }

    // Controls for a scrolling capture. The frame count, status and auto
    // state come from the Rust session; every button sends it a command.
    ScrollControls {
        id: scrollControls
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: 12
        visible: Capture.scrollOpen
        status: Capture.scrollStatus
        startLabel: Capture.scrollStartLabel
        frames: Capture.scrollFrames
        autoScroll: Capture.scrollAuto
        running: Capture.scrollRunning
        horizontal: Capture.scrollHorizontal
        onStartRequested: Capture.scrollStart()
        onAutoToggled: Capture.scrollToggleAuto()
        onDoneRequested: Capture.scrollDone()
        onCancelRequested: Capture.scrollCancel()
    }

    // Window picker for a window capture; the client rectangles stay in Rust.
    WindowPicker {
        id: windowPicker
        anchors.centerIn: parent
        labels: Capture.windowLabels
        details: Capture.windowDetails
        visible: Capture.windowPickerOpen
        onChosen: function (index) {
            Capture.captureWindow(index)
        }
        onDismissed: Capture.closeWindowPicker()
    }

    // The tray fallback menu reads the same table through Menu.trayLabels.
    TrayMenu {
        id: trayMenu
        trayLabels: Menu.trayLabels
        trayIds: Menu.trayIds
        onActionTriggered: function (id) {
            Menu.activateId(id)
        }
    }

    // Recognized-text review. TextOutput is a singleton, so it is referenced
    // by id rather than instantiated. When the command line asked for OCR, run
    // the completion path: a file goes straight to recognition, while a bare
    // `tidyshot ocr` or `capture-text` URL first runs the interactive capture.
    TextResultDialog {
        id: textResultDialog
        Component.onCompleted: {
            if (Args.ocrPath !== "")
                TextOutput.recognize(Args.ocrPath, Args.linebreaks)
            else if (Args.captureOcr)
                TextOutput.captureAndRecognize(Args.linebreaks)
        }
    }
}
