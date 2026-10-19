// Floating controls for a scrolling capture.
//
// The next capture waits for Start, then grows a preview while the Rust
// session stitches frames. Layout only: the session state and every action
// live in src/scroll.rs and src/scroll_runtime.rs, bound here through the
// Capture singleton. The component is free of the `tidyshot` module so it can
// be driven by qmltestrunner offscreen.
import QtQuick
import QtQuick.Controls

Rectangle {
    id: controls

    /// The session's status line.
    property string status: ""
    /// The start button's label, so a failed start offers its retry.
    property string startLabel: "Start Capture"
    /// Frames folded into the stitch so far.
    property int frames: 0
    /// Whether auto-scroll is on.
    property bool autoScroll: false
    /// Whether frames are currently being accepted.
    property bool running: false
    /// Whether this is a horizontal capture.
    property bool horizontal: false

    /// Start grabbing frames.
    signal startRequested()
    /// Flip auto-scroll.
    signal autoToggled()
    /// Finish and save the stitched image.
    signal doneRequested()
    /// Abandon the capture without saving.
    signal cancelRequested()

    width: 420
    // The panel is anchored by one corner only, so it has to size itself from
    // its column; without this the buttons would be laid out in zero height.
    implicitHeight: column.implicitHeight + column.anchors.margins * 2
    color: "#242424"
    radius: 8
    border.color: "#3a3a3a"

    Column {
        id: column
        anchors.fill: parent
        anchors.margins: 10
        spacing: 6

        Text {
            objectName: "scrollTitle"
            text: controls.horizontal ? "Scrolling capture · horizontal"
                                      : "Scrolling capture"
            color: "white"
            font.bold: true
        }

        Text {
            objectName: "scrollStatus"
            width: parent.width
            text: controls.status
            color: "#9a9a9a"
            wrapMode: Text.WordWrap
        }

        Text {
            objectName: "scrollFrames"
            text: controls.frames + (controls.frames === 1 ? " frame" : " frames")
            color: "#FFD60A"
        }

        Row {
            spacing: 6
            Button {
                objectName: "scrollStart"
                text: controls.running ? "Capturing…" : controls.startLabel
                enabled: !controls.running
                onClicked: controls.startRequested()
            }
            Button {
                objectName: "scrollAuto"
                text: controls.autoScroll ? "Auto-Scroll: On" : "Auto-Scroll: Off"
                onClicked: controls.autoToggled()
            }
            Button {
                objectName: "scrollDone"
                text: "Done"
                enabled: controls.frames > 0
                onClicked: controls.doneRequested()
            }
            Button {
                objectName: "scrollCancel"
                text: "Cancel"
                onClicked: controls.cancelRequested()
            }
        }
    }
}
