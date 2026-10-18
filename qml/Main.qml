import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import tidyshot

ApplicationWindow {
    id: window
    width: 480
    height: 620
    visible: true
    title: qsTr("tidyshot")
    color: "#1e1e1e"

    Material.theme: Material.Dark
    Material.accent: "#FFD60A"

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
            // The Rust runtime performs the capture for a capture row; other
            // rows still move the shared cursor and report their own state.
            Menu.activateIndex(index)
            var id = index >= 0 && index < Menu.ids.length ? Menu.ids[index] : ""
            if (id !== "")
                Capture.captureRow(id)
        }
        onDismissed: Qt.quit()
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

    // Corner preview for a completed capture. The overlay owns its layout;
    // the captured path and dimensions come from the Rust runtime.
    Overlay {
        id: preview
        anchors.left: parent.left
        anchors.bottom: parent.bottom
        anchors.margins: 12
        visible: Capture.path !== ""
        title: "Capture ready"
        info: Capture.width > 0 ? Capture.width + " × " + Capture.height : ""
        contentUrl: Capture.path !== "" ? "file://" + Capture.path : ""
        onDismissed: Capture.clear()
    }

    // Window picker for a window capture; the client rectangles stay in Rust.
    WindowPicker {
        id: windowPicker
        visible: Capture.windowPickerOpen
        onClosed: Capture.closeWindowPicker()
    }

    // The tray fallback menu reads the same table through Menu.trayLabels.
    TrayMenu {
        id: trayMenu
        trayLabels: Menu.trayLabels
        trayIds: Menu.trayIds
        onActionTriggered: function (id) {
            Menu.activateId(id)
            Capture.captureRow(id)
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
