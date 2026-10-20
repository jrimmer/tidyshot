// Fullscreen frozen-frame window picker for one output.
// Ported from omnishot/window_selection.py (WindowSelector)
//
// The desktop was captured once before this window opened, so the picture is
// frozen while the pointer moves: the hovered window's own region shows
// through the dimmed frame in full colour, outlined in the Omarchy accent.
// The component takes flat row lists exactly as the Rust picker projects
// them and forwards the choice; it never resolves a window itself.
import QtQuick

Window {
    id: hover

    /// The frozen frame: name, x, y, w, h and its file URL.
    property var frame: null
    /// Every picker row: title, app, x, y, w, h and monitor index.
    property var rows: []
    /// The live Omarchy accent used for the highlight.
    property string accent: "#FFD60A"
    /// The instruction band text.
    property string hint: "Click a window · Shift for transparency · Tab switches windows · Esc cancels"
    /// The index of the highlighted row, or -1.
    property int current: -1
    /// The highlighted row's short title.
    property string currentTitle: ""
    /// Keep focus after opening; a picker with no menu behind it needs it.
    property bool takeFocus: true

    /// A row was chosen; transparent keeps the window's alpha.
    signal chosen(int index, bool transparent, bool forceCopy)
    /// The picker was dismissed without a choice.
    signal dismissed()

    width: frame ? frame.w : 1280
    height: frame ? frame.h : 720
    x: frame ? frame.x : 0
    y: frame ? frame.y : 0
    visible: frame !== null
    color: "black"
    title: "TidyShot Window Selection"
    flags: Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint | Qt.BypassWindowManagerHint

    function rowsOnThisScreen() {
        var result = []
        for (var index = 0; index < rows.length; ++index) {
            if (rows[index].monitor === (frame ? frame.index : 0))
                result.push(index)
        }
        return result
    }

    function localX(row) { return row.x - (frame ? frame.x : 0) }
    function localY(row) { return row.y - (frame ? frame.y : 0) }

    /// The row at a point in this window, honouring the focused-first order.
    function rowAt(px, py) {
        var visible = rowsOnThisScreen()
        for (var slot = 0; slot < visible.length; ++slot) {
            var index = visible[slot]
            var row = rows[index]
            var x = localX(row)
            var y = localY(row)
            if (px >= x && px <= x + row.w && py >= y && py <= y + row.h)
                return index
        }
        return -1
    }

    function selectRow(index) {
        current = index
        currentTitle = index >= 0 ? rows[index].title : ""
        canvas.requestPaint()
    }

    /// Cycle by focus order, wrapping at both ends.
    function cycle(step) {
        var visible = rowsOnThisScreen()
        if (visible.length === 0) {
            selectRow(-1)
            return
        }
        var position = visible.indexOf(current)
        var next
        if (position < 0)
            next = step < 0 ? visible[visible.length - 1] : visible[0]
        else
            next = visible[(position + step + visible.length) % visible.length]
        selectRow(next)
    }

    function choose(shift, control) {
        if (current < 0)
            return
        chosen(current, shift, control)
    }

    onVisibleChanged: {
        if (visible && takeFocus) {
            hover.requestActivate()
            keys.forceActiveFocus()
        }
    }

    Shortcut {
        sequences: ["Escape"]
        enabled: hover.visible
        onActivated: hover.dismissed()
    }

    Canvas {
        id: canvas
        anchors.fill: parent
        onPaint: {
            var context = getContext("2d")
            context.reset()
            // The frozen frame, then the dimming layer every picker uses.
            if (hover.frame && hover.frame.url !== "")
                context.drawImage(hover.frame.url, 0, 0, canvas.width, canvas.height)
            context.fillStyle = "rgba(0, 0, 0, 0.39)"
            context.fillRect(0, 0, canvas.width, canvas.height)
            if (hover.current >= 0 && hover.current < hover.rows.length) {
                var row = hover.rows[hover.current]
                var x = hover.localX(row)
                var y = hover.localY(row)
                // Redraw the highlighted window undimmed, clipped to the frame.
                if (hover.frame && hover.frame.url !== "") {
                    context.save()
                    context.beginPath()
                    context.rect(x, y, row.w, row.h)
                    context.clip()
                    context.drawImage(hover.frame.url, 0, 0, canvas.width, canvas.height)
                    context.restore()
                }
                context.strokeStyle = hover.accent
                context.lineWidth = 2
                context.strokeRect(x + 1, y + 1, row.w - 2, row.h - 2)
            }
            // The instruction band sits at the top-left of every output.
            var bandWidth = Math.min(760, canvas.width - 24)
            context.fillStyle = "rgba(30, 30, 30, 0.9)"
            context.fillRect(12, 12, bandWidth, 58)
            context.fillStyle = "white"
            context.font = "13px sans-serif"
            context.fillText(hover.hint, 24, 36, bandWidth - 24)
            if (hover.currentTitle !== "")
                context.fillText(hover.currentTitle, 24, 57, bandWidth - 24)
        }
    }

    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        onPositionChanged: function (mouse) {
            var index = hover.rowAt(mouse.x, mouse.y)
            if (index !== hover.current)
                hover.selectRow(index)
        }
        onPressed: function (mouse) {
            if (mouse.button === Qt.RightButton) {
                hover.dismissed()
                return
            }
            var index = hover.rowAt(mouse.x, mouse.y)
            hover.selectRow(index)
            hover.choose((mouse.modifiers & Qt.ShiftModifier) !== 0,
                         (mouse.modifiers & Qt.ControlModifier) !== 0)
        }
    }

    FocusScope {
        id: keys
        anchors.fill: parent
        focus: hover.visible
        Keys.onPressed: function (event) {
            if (event.key === Qt.Key_Escape) {
                hover.dismissed()
                event.accepted = true
            } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                hover.choose((event.modifiers & Qt.ShiftModifier) !== 0,
                             (event.modifiers & Qt.ControlModifier) !== 0)
                event.accepted = true
            } else if (event.key === Qt.Key_Tab || event.key === Qt.Key_Backtab) {
                var step = (event.modifiers & Qt.ShiftModifier) !== 0 ? -1 : 1
                hover.cycle(step)
                event.accepted = true
            }
        }
    }
}
