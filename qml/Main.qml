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
    visible: !Preview.standalone && !EditorApi.standalone
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
        Recording.bind()
        HistoryApi.bind()
        SettingsApi.bind()
        PinApi.bind()
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
            window.visible = !Preview.standalone && !EditorApi.standalone
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
            var dragUri = Preview.dragUri()
            if (previewWindow.dragUri !== dragUri)
                previewWindow.dragUri = dragUri
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
        onSaveAllRequested: Preview.saveAll()
        onCloseAllRequested: Preview.closeAll()
        onExportRequested: function (format) { Preview.exportAs(format) }
        onOpenExternalRequested: Preview.openExternal()
        onPinRequested: Preview.pin()
        onDismissed: Preview.dismiss()
        onCollapseRequested: function (collapsed) {
            Preview.collapseTo(collapsed)
        }
    }

    // Pinned captures: one top-level window per registry row. The rows are
    // parsed from the PinApi singleton on a short timer because cxx-qt
    // singleton property notifications do not reach QML bindings here.
    property var pinRows: []
    property string pinRowsJson: ""
    Timer {
        id: pinSync
        interval: 60
        repeat: true
        running: true
        onTriggered: {
            // Only rebuild the windows when the registry actually changed;
            // recreating them on every tick would fight a user move.
            var text = PinApi.rows.join("\n")
            if (text === window.pinRowsJson)
                return
            window.pinRowsJson = text
            var rows = []
            for (var r = 0; r < PinApi.rows.length; ++r)
                rows.push(JSON.parse(PinApi.rows[r]))
            window.pinRows = rows
        }
    }

    Instantiator {
        id: pinWindows
        model: window.pinRows
        delegate: PinWindow {
            required property var modelData
            path: modelData.path
            name: modelData.name
            pinWidth: modelData.width
            pinHeight: modelData.height
            opacityValue: modelData.opacity
            locked: modelData.locked
            clickThrough: modelData.clickThrough
            source: modelData.url !== "" ? modelData.url
                                         : "file://" + modelData.path
            x: modelData.x
            y: modelData.y
            visible: modelData.visible
            // The registry is the source of truth once a row exists; the
            // compositor's own move/resize is reported back to it.
            reportGeometry: true
            onCloseRequested: PinApi.close(modelData.path)
            onLockRequested: PinApi.lock(modelData.path)
            onUnlockRequested: PinApi.unlock(modelData.path)
            onOpacityChosen: function (value) {
                PinApi.setOpacity(modelData.path, value)
            }
            onClickThroughChosen: function (value) {
                PinApi.setClickThrough(modelData.path, value)
            }
            onGeometryChanged: function (x, y, width, height) {
                PinApi.saveGeometry(modelData.path, x, y, width, height)
            }
        }
    }

    Text {
        objectName: "pinMessage"
        anchors.top: parent.top
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.topMargin: 12
        text: PinApi.message
        color: "#FFD60A"
        visible: text !== ""
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

    // The recording controls are their own always-on-top window so they never
    // take focus from the app being recorded. A Hyprland rule keeps them out
    // of the take, and the status feed carries their display position.
    Timer {
        id: recordingSync
        interval: 100
        repeat: true
        running: true
        onTriggered: {
            recordingWindow.visible = Recording.open && Recording.showControls
            recordingWindow.x = Recording.x
            recordingWindow.y = Recording.y
            recordingControls.elapsed = Recording.elapsed
            recordingControls.paused = Recording.paused
            recordingControls.status = Recording.message
        }
    }

    Window {
        id: recordingWindow
        transientParent: null
        visible: Recording.open
        title: "TidyShot Recording Controls"
        flags: Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint | Qt.Tool
        color: "transparent"
        width: Recording.windowWidth
        height: Recording.windowHeight

        RecordingControls {
            id: recordingControls
            anchors.fill: parent
            elapsed: Recording.elapsed
            paused: Recording.paused
            status: Recording.message
            onPauseRequested: Recording.pause()
            onStopRequested: Recording.stop()
            onRestartRequested: Recording.restart()
            onCancelRequested: Recording.cancel()
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

    // cxx-qt singleton property notifications do not reach QML bindings in
    // this build, so the scroll panel is fed from the singleton. The worker
    // publishes a status for every frame, so this keeps the frame count and
    // the Start/Retry label moving without touching the session.
    Timer {
        id: scrollSync
        interval: 60
        repeat: true
        running: true
        onTriggered: {
            scrollControls.visible = Capture.scrollOpen
            scrollControls.status = Capture.scrollStatus
            scrollControls.startLabel = Capture.scrollStartLabel
            scrollControls.frames = Capture.scrollFrames
            scrollControls.autoScroll = Capture.scrollAuto
            scrollControls.running = Capture.scrollRunning
            scrollControls.horizontal = Capture.scrollHorizontal
        }
    }

    // The frozen-frame picker takes JSON rows from the capture singleton,
    // for the same reason: property notifications do not reach QML here.
    Timer {
        id: windowSync
        interval: 60
        repeat: true
        running: true
        onTriggered: {
            var frames = []
            for (var f = 0; f < Capture.windowFrames.length; ++f)
                frames.push(JSON.parse(Capture.windowFrames[f]))
            var rows = []
            for (var r = 0; r < Capture.windowRows.length; ++r)
                rows.push(JSON.parse(Capture.windowRows[r]))
            window.hoverFrames = frames
            window.hoverRows = rows
            window.hoverOpen = Capture.windowHoverOpen
            window.hoverAccent = Capture.windowAccent
            window.hoverHint = Capture.windowHint
        }
    }

    // Fullscreen frozen-frame pickers, one per output. Each takes the frozen
    // frame it draws and the focus-sorted rows, and reports the chosen row.
    // A window capture that cannot freeze the desktop falls back to the list
    // picker below.
    property var hoverFrames: []
    property var hoverRows: []
    property bool hoverOpen: false
    property string hoverAccent: "#FFD60A"
    property string hoverHint: "Click a window · Shift for transparency · Tab switches windows · Esc cancels"
    // A Repeater only accepts Item delegates, so an Instantiator owns the
    // per-output windows instead; it keeps each one for the life of its row.
    Instantiator {
        id: hoverPickers
        model: window.hoverFrames
        delegate: WindowHover {
            required property var modelData
            frame: modelData
            rows: window.hoverRows
            accent: window.hoverAccent
            hint: window.hoverHint
            visible: window.hoverOpen
            onChosen: function (index, transparent, forceCopy) {
                Capture.captureWindowHover(index, transparent, forceCopy)
            }
            onDismissed: Capture.closeWindowPicker()
        }
    }

    // The area selector: one fullscreen overlay on the focused output. Rust
    // prepares the frozen frame and accepts the rectangle it returns, so the
    // absolute desktop coordinates never live in QML.
    property var selectionFrame: null
    property bool selectionOpen: false
    property bool selectionLive: false

    Timer {
        id: selectionSync
        interval: 60
        repeat: true
        running: true
        onTriggered: {
            try {
                window.selectionFrame = Capture.selectionFrame !== ""
                    ? JSON.parse(Capture.selectionFrame) : null
            } catch (error) {
                window.selectionFrame = null
            }
            window.selectionOpen = Capture.selectionOpen
            window.selectionLive = Capture.selectionLive
            selectionOverlay.visible = Capture.selectionOpen
            selectionOverlay.mode = Capture.selectionMode
            selectionOverlay.accent = Capture.selectionAccent
            selectionOverlay.hint = Capture.selectionHint
            selectionOverlay.live = Capture.selectionLive
            selectionOverlay.zoom = Capture.magnifierZoom
            selectionOverlay.frame = window.selectionFrame
            var rows = []
            for (var r = 0; r < Capture.selectionRows.length; ++r)
                rows.push(JSON.parse(Capture.selectionRows[r]))
            selectionOverlay.rows = rows
        }
    }

    // A live selector refreshes its frame through grim at about 10 Hz, so the
    // desktop is current while the pointer moves.
    Timer {
        id: selectionRefresh
        interval: 100
        repeat: true
        running: window.selectionOpen && window.selectionLive
        onTriggered: Capture.refreshSelectionFrame()
    }

    SelectionOverlay {
        id: selectionOverlay
        transientParent: null
        takeFocus: true
        onAccepted: function (x, y, width, height) {
            Capture.selectionAccept(x, y, width, height)
        }
        onWindowChosen: function (index, transparent, forceCopy) {
            Capture.captureWindowHover(index, transparent, forceCopy)
        }
        onCancelled: Capture.selectionCancel()
    }

    // The countdown label: always on top, small and never stealing the frame.
    Window {
        id: countdownWindow
        transientParent: null
        visible: Capture.countdownOpen
        title: "TidyShot Countdown"
        flags: Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint | Qt.Tool
        color: "transparent"
        width: countdownControls.width
        height: countdownControls.height
        x: Math.round((Screen.width - width) / 2)
        y: 48

        CountdownOverlay {
            id: countdownControls
            label: Capture.countdownLabel
            seconds: Capture.countdownSeconds
            onCancelRequested: Capture.countdownCancel()
        }
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
    // The annotate editor: its own top-level window, opened by the preview's
    // Annotate button, `tidyshot open`, or an after-capture annotate action.
    Editor {
        id: editorWindow
        transientParent: null
    }

    // The capture history window. It is its own top-level window so the
    // filmstrip can be wider than the menu, and its state comes from the
    // Rust history view through the HistoryApi singleton.
    // `visible` has one writer, the sync timer below, so the window cannot be
    // shown by one path and hidden by the other.
    HistoryWindow {
        id: historyContent
        transientParent: null
        filter: HistoryApi.filter
        query: HistoryApi.query
        message: HistoryApi.message
        cursor: HistoryApi.cursor
        selected: HistoryApi.selected
        tabIds: HistoryApi.tabIds
        tabLabels: HistoryApi.tabLabels
        onFilterChosen: function (id) { HistoryApi.chooseFilter(id) }
        onSearchChanged: function (text) { HistoryApi.search(text) }
        onMoved: function (delta) { HistoryApi.moveCursor(delta) }
        onRowSelected: function (index, extend) {
            HistoryApi.selectRow(index, extend)
        }
        onRowActivated: function (index) {
            HistoryApi.selectRow(index, false)
            HistoryApi.annotate()
        }
        onDeleteRequested: HistoryApi.deleteSelected()
        onClearRequested: HistoryApi.clearHistory()
        onRestoreRequested: HistoryApi.restore()
        onAnnotateRequested: HistoryApi.annotate()
        onPinRequested: HistoryApi.pin()
        onDismissed: HistoryApi.close()
    }

    // cxx-qt singleton property notifications do not reach QML bindings in
    // this build, so the history window is fed from the singleton on a timer.
    Timer {
        id: historySync
        interval: 60
        repeat: true
        running: true
        onTriggered: {
            var rows = []
            for (var r = 0; r < HistoryApi.rows.length; ++r)
                rows.push(JSON.parse(HistoryApi.rows[r]))
            historyContent.visible = HistoryApi.open
            historyContent.rows = rows
            historyContent.filter = HistoryApi.filter
            historyContent.query = HistoryApi.query
            historyContent.message = HistoryApi.message
            historyContent.cursor = HistoryApi.cursor
            historyContent.selected = HistoryApi.selected
            historyContent.tabIds = HistoryApi.tabIds
            historyContent.tabLabels = HistoryApi.tabLabels
        }
    }

    // Settings: its own top-level window, opened by the menu row,
    // `tidyshot settings` or a settings URL. Every change writes the file
    // atomically and the next capture, preview or recording reads it back, so
    // it applies live. It is not a child of this window: an Item cannot be a
    // standalone dialog, and the menu window is only 480x620.
    SettingsWindow {
        id: settingsWindow
        transientParent: null
        // The model is a property binding, so QML only re-parses the document
        // when the singleton's notification fires, not on a polling tick.
        modelJson: SettingsApi.modelJson
        onValueChosen: function (key, jsonValue) {
            SettingsApi.setValue(key, jsonValue)
        }
        onShortcutChosen: function (id, key) {
            SettingsApi.setShortcut(id, key)
        }
        onResetShortcutsRequested: SettingsApi.resetShortcuts()
        onReloadRequested: SettingsApi.reload()
        onClosed: SettingsApi.close()
    }

    // cxx-qt property notifications do not reach every QML binding in this
    // build, so the window's visibility is still polled; the model itself is
    // bound above rather than copied on each tick. `visible` has one writer,
    // this timer, so the window cannot be shown by one path and hidden by the
    // other.
    Timer {
        id: settingsSync
        interval: 60
        repeat: true
        running: true
        onTriggered: settingsWindow.visible = SettingsApi.open
    }

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
