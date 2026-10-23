// One pinned capture: an always-on-top frameless window at its pin size.
//
// The geometry, opacity and control rules live in src/pin.rs; this component
// only draws one row from the PinApi singleton. It stays free of the
// `tidyshot` module so qmltestrunner can drive it offscreen.
import QtQuick
import QtQuick.Controls

Window {
    id: pinWindow

    /// The pinned file.
    property string path: ""
    /// The pin's display name.
    property string name: ""
    /// Pin size and opacity from the Rust pin state.
    property real pinWidth: 200
    property real pinHeight: 150
    property real opacityValue: 1.0
    property bool locked: false
    /// Swallow pointer input so the pin never takes a click from the app
    /// underneath it.
    property bool clickThrough: false
    /// Draw the drop shadow the source pin uses.
    property bool showShadow: true
    /// Round the pin's corners.
    property bool rounded: true
    /// The picture to draw.
    property url source: ""
    /// True while the hover controls are shown.
    property bool controlsShown: false
    /// Report a user-driven move or resize back to the registry.
    property bool reportGeometry: true

    /// The pixel radius the source applies to a rounded pin.
    readonly property real cornerRadius: rounded ? 8 : 0

    signal closeRequested()
    signal lockRequested()
    signal unlockRequested()
    signal opacityChosen(real value)
    signal clickThroughChosen(bool value)
    signal geometryChanged(real x, real y, real width, real height)
    signal dragStarted()
    signal resizeStarted(int edges)

    width: pinWidth
    height: pinHeight
    visible: true
    title: locked ? "TidyShot Locked Pin — " + name : "TidyShot Pin — " + name
    color: "transparent"
    opacity: opacityValue
    flags: Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint | Qt.Tool
           | (clickThrough ? Qt.WindowTransparentForInput : 0)

    function showControls() {
        controlsShown = true
    }

    function hideControls() {
        controlsShown = false
    }

    function report() {
        if (reportGeometry)
            geometryChanged(x, y, width, height)
    }

    // Escape hides the controls without closing the pin.
    Shortcut {
        objectName: "pinEscape"
        sequences: ["Escape"]
        context: Qt.WindowShortcut
        enabled: pinWindow.controlsShown && pinWindow.active
        onActivated: pinWindow.hideControls()
    }

    // The shadow sits behind the visible frame; the source grows the pin by
    // the same margin on every side.
    Rectangle {
        objectName: "pinShadow"
        anchors.fill: pinFrame
        anchors.margins: -4
        z: -1
        radius: pinWindow.cornerRadius + 3
        color: "#55000000"
        visible: pinWindow.showShadow
    }

    Rectangle {
        id: pinFrame
        objectName: "pinFrame"
        anchors.fill: parent
        anchors.margins: pinWindow.showShadow ? 4 : 0
        radius: pinWindow.cornerRadius
        color: "#242424"
        border.color: pinWindow.rounded ? "#3a3a3a" : "transparent"
        clip: true

        Image {
            objectName: "pinImage"
            anchors.fill: parent
            fillMode: Image.PreserveAspectFit
            source: pinWindow.source
            asynchronous: true
        }
    }

    // Drag anywhere on the pin; a middle click closes it. The hover controls
    // are drawn on top of this area, so a button press never starts a move.
    MouseArea {
        objectName: "pinBody"
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.MiddleButton
        onEntered: pinWindow.showControls()
        onPositionChanged: pinWindow.showControls()
        onExited: if (!pinWindow.locked)
                      pinWindow.hideControls()
        onPressed: function (mouse) {
            pinWindow.showControls()
            if (mouse.button === Qt.MiddleButton) {
                pinWindow.closeRequested()
                return
            }
            if (!pinWindow.locked) {
                pinWindow.dragStarted()
                pinWindow.startSystemMove()
            }
        }
    }

    Row {
        objectName: "pinControls"
        anchors.top: parent.top
        anchors.right: parent.right
        anchors.margins: 6
        spacing: 4
        visible: pinWindow.controlsShown

        Button {
            objectName: "pinOpacityDown"
            visible: !pinWindow.locked
            width: 24
            height: 24
            padding: 0
            text: "−"
            onClicked: pinWindow.opacityChosen(
                           Math.max(0.1, pinWindow.opacityValue - 0.1))
        }
        Text {
            objectName: "pinOpacityLabel"
            visible: !pinWindow.locked
            anchors.verticalCenter: parent.verticalCenter
            text: Math.round(pinWindow.opacityValue * 100) + "%"
            color: "white"
        }
        Button {
            objectName: "pinOpacityUp"
            visible: !pinWindow.locked
            width: 24
            height: 24
            padding: 0
            text: "+"
            onClicked: pinWindow.opacityChosen(
                           Math.min(1.0, pinWindow.opacityValue + 0.1))
        }
        Button {
            objectName: "pinLock"
            visible: !pinWindow.locked
            width: 24
            height: 24
            padding: 0
            text: "Lock"
            onClicked: pinWindow.lockRequested()
        }
        Button {
            objectName: "pinUnlock"
            visible: pinWindow.locked
            width: 24
            height: 24
            padding: 0
            text: "Unlock"
            onClicked: pinWindow.unlockRequested()
        }
        Button {
            objectName: "pinClose"
            visible: !pinWindow.locked
            width: 24
            height: 24
            padding: 0
            text: "×"
            onClicked: pinWindow.closeRequested()
        }
    }

    // The click-through toggle keeps the pin visible while every click lands
    // on the window below it.
    Button {
        objectName: "pinClickThrough"
        visible: pinWindow.controlsShown && !pinWindow.locked
        anchors.bottom: parent.bottom
        anchors.right: parent.right
        anchors.margins: 6
        width: 24
        height: 24
        padding: 0
        text: pinWindow.clickThrough ? "◎" : "○"
        onClicked: pinWindow.clickThroughChosen(!pinWindow.clickThrough)
    }

    // A compositor move changes x/y without a press here, and a resize that
    // starts a system resize changes width/height the same way, so the
    // registry is told once the user pauses rather than on every frame.
    Timer {
        id: reportTimer
        interval: 250
        repeat: false
        onTriggered: pinWindow.report()
    }
    onXChanged: reportTimer.restart()
    onYChanged: reportTimer.restart()
    onWidthChanged: {
        // Keep the Rust-side size in step when the registry drives the
        // window, without reporting the model load as a user resize.
        if (Math.abs(width - pinWidth) > 0.5)
            pinWidth = width
        reportTimer.restart()
    }
    onHeightChanged: {
        if (Math.abs(height - pinHeight) > 0.5)
            pinHeight = height
        reportTimer.restart()
    }

    // Each edge and corner starts a compositor resize; the strips are on top
    // of the body so a resize press never turns into a move.
    component ResizeEdge: MouseArea {
        property int edges: Qt.LeftEdge
        hoverEnabled: true
        cursorShape: Qt.SizeHorCursor
        onPressed: {
            pinWindow.resizeStarted(edges)
            pinWindow.startSystemResize(edges)
        }
    }

    ResizeEdge {
        objectName: "pinResizeLeft"
        edges: Qt.LeftEdge
        anchors.left: parent.left
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        width: 6
    }
    ResizeEdge {
        objectName: "pinResizeRight"
        edges: Qt.RightEdge
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        width: 6
    }
    ResizeEdge {
        objectName: "pinResizeTop"
        edges: Qt.TopEdge
        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        height: 6
    }
    ResizeEdge {
        objectName: "pinResizeBottom"
        edges: Qt.BottomEdge
        anchors.bottom: parent.bottom
        anchors.left: parent.left
        anchors.right: parent.right
        height: 6
    }
}
