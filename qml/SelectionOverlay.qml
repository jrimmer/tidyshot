// Fullscreen area selector for one output.
// Ported from omnishot/selection.py (SelectionOverlay)
//
// The component draws one frozen frame, or the live desktop a refresh timer
// feeds it, and owns the pointer: a drag starts a rectangle, its handles resize
// it afterwards, the magnifier samples the frame pixels around the pointer and
// the W/H fields type an exact size. Rust receives the finished rectangle and
// converts it to absolute desktop coordinates.
import QtQuick
import QtQuick.Controls

Window {
    id: overlay

    /// The frame: x, y, w, h in logical pixels, iw, ih in image pixels, a URL.
    property var frame: null
    /// The client rectangles for window mode: title, app, x, y, w, h, address.
    property var rows: []
    /// area, timer, select or window.
    property string mode: "area"
    /// The live Omarchy accent.
    property string accent: "#FFD60A"
    /// The instruction line, worded as the source words it.
    property string hint: "Drag to select · Shift locks proportions · Enter captures · Esc cancels"
    /// Whether the frame refreshes instead of staying frozen.
    property bool live: false
    /// The magnifier's magnification, 4 to 8.
    property int zoom: 6
    /// A window capture keeps the window's alpha when Shift is held.
    property bool takeFocus: true

    /// The rectangle in local output pixels: -1 width means none.
    property int rectX: 0
    property int rectY: 0
    property int rectW: 0
    property int rectH: 0
    /// A locked width/height ratio, 0 when unlocked.
    property double ratio: 0
    /// The highlighted window row in window mode, -1 when none.
    property int current: -1

    /// A rectangle was accepted; coordinates are local to this output.
    signal accepted(int x, int y, int width, int height)
    /// A window row was chosen.
    signal windowChosen(int index, bool transparent, bool forceCopy)
    /// The selector was dismissed.
    signal cancelled()

    readonly property bool hasRect: rectW >= 2 && rectH >= 2
    // Source geometry: a 100x100 inset holding a 24px sample.
    readonly property int magnifierSize: 100
    readonly property int magnifierSample: 24
    readonly property int handleSlack: 8
    // The capture scale, from the frame's image size and logical size.
    readonly property double captureScale: frame && frame.w > 0 ? frame.iw / frame.w : 1

    width: frame ? frame.w : 1280
    height: frame ? frame.h : 720
    x: frame ? frame.x : 0
    y: frame ? frame.y : 0
    visible: frame !== null
    color: "black"
    title: "TidyShot Selection"
    flags: Qt.FramelessWindowHint | Qt.WindowStaysOnTopHint | Qt.BypassWindowManagerHint

    // The drag in progress: "new", "move", a handle token, or "".
    property string gesture: ""
    property int pressX: 0
    property int pressY: 0
    property int startX: 0
    property int startY: 0
    property int startW: 0
    property int startH: 0
    property int pointerX: 0
    property int pointerY: 0

    /// The pointer in image pixels, for the magnifier.
    function imagePoint(px, py) {
        var sx = frame && frame.w > 0 ? frame.iw / frame.w : 1
        var sy = frame && frame.h > 0 ? frame.ih / frame.h : 1
        return Qt.point(px * sx, py * sy)
    }

    /// The source rectangle the magnifier shows, in image pixels.
    function magnifierRect(px, py) {
        var point = imagePoint(px !== undefined ? px : pointerX, py !== undefined ? py : pointerY)
        var span = magnifierSample
        var x = Math.max(0, Math.min(Math.max(0, (frame ? frame.iw : 0) - span), point.x - span / 2))
        var y = Math.max(0, Math.min(Math.max(0, (frame ? frame.ih : 0) - span), point.y - span / 2))
        return { "x": x, "y": y, "w": span, "h": span }
    }

    /// The eight handles: corners first, then the edge midpoints.
    function handlePoints() {
        var midX = rectX + rectW / 2
        var midY = rectY + rectH / 2
        return [
            Qt.point(rectX, rectY),
            Qt.point(rectX + rectW, rectY),
            Qt.point(rectX, rectY + rectH),
            Qt.point(rectX + rectW, rectY + rectH),
            Qt.point(midX, rectY),
            Qt.point(midX, rectY + rectH),
            Qt.point(rectX, midY),
            Qt.point(rectX + rectW, midY)
        ]
    }

    /// The size chip text: logical size and captured pixels.
    function sizeLabel() {
        var scale = captureScale
        return rectW + " × " + rectH
            + "   (" + Math.round(rectW * scale) + " × "
            + Math.round(rectH * scale) + " px)"
    }

    function clampX(value) {
        return Math.max(0, Math.min(frame ? frame.w : width, value))
    }

    function clampY(value) {
        return Math.max(0, Math.min(frame ? frame.h : height, value))
    }

    /// Fit a size to the output, keeping the ratio when one applies.
    function fitSize(w, h, driver, ratioValue, originX, originY) {
        var originLeft = originX !== undefined ? originX : rectX
        var originTop = originY !== undefined ? originY : rectY
        var maxWidth = (frame ? frame.w : width) - originLeft
        var maxHeight = (frame ? frame.h : height) - originTop
        if (ratioValue > 0) {
            var fitted = driver === "height" ? h * ratioValue : w
            var low = Math.max(2, 2 * ratioValue)
            var high = Math.min(maxWidth, maxHeight * ratioValue)
            fitted = Math.max(low, Math.min(high, fitted))
            return { "w": Math.round(fitted), "h": Math.max(1, Math.round(fitted / ratioValue)) }
        }
        return {
            "w": Math.max(2, Math.min(Math.max(2, maxWidth), Math.round(w))),
            "h": Math.max(2, Math.min(Math.max(2, maxHeight), Math.round(h)))
        }
    }

    /// Apply an exact size typed into the fields.
    function applySize(w, h, driver) {
        if (!frame)
            return
        var size = fitSize(w, h, driver, ratio)
        rectW = Math.max(2, Math.min(size.w, frame.w - rectX))
        rectH = Math.max(2, Math.min(size.h, frame.h - rectY))
    }

    function handleAt(px, py) {
        if (!hasRect)
            return ""
        var left = Math.abs(px - rectX) <= handleSlack
        var right = Math.abs(px - (rectX + rectW)) <= handleSlack
        var top = Math.abs(py - rectY) <= handleSlack
        var bottom = Math.abs(py - (rectY + rectH)) <= handleSlack
        var insideX = px >= rectX - handleSlack && px <= rectX + rectW + handleSlack
        var insideY = py >= rectY - handleSlack && py <= rectY + rectH + handleSlack
        if (!insideX || !insideY)
            return ""
        if (left && top) return "lt"
        if (right && top) return "rt"
        if (left && bottom) return "lb"
        if (right && bottom) return "rb"
        if (top) return "t"
        if (bottom) return "b"
        if (left) return "l"
        if (right) return "r"
        if (px >= rectX && px < rectX + rectW && py >= rectY && py < rectY + rectH)
            return "move"
        return ""
    }

    function beginGesture(px, py, shift) {
        pressX = clampX(px)
        pressY = clampY(py)
        pointerX = pressX
        pointerY = pressY
        startX = rectX
        startY = rectY
        startW = rectW
        startH = rectH
        if (mode === "window") {
            current = rowAt(pressX, pressY)
            canvas.requestPaint()
            return
        }
        var handle = handleAt(pressX, pressY)
        if (handle !== "") {
            gesture = handle
            if (shift && ratio <= 0)
                ratio = rectH > 0 ? rectW / rectH : 1
            return
        }
        gesture = "new"
        rectW = 0
        rectH = 0
    }

    function updateGesture(px, py, shift) {
        pointerX = clampX(px)
        pointerY = clampY(py)
        if (mode === "window") {
            var index = rowAt(pointerX, pointerY)
            if (index !== current) {
                current = index
                canvas.requestPaint()
            }
            return
        }
        var lock = ratio > 0 ? ratio : (shift ? 1 : 0)
        var dx = pointerX - pressX
        var dy = pointerY - pressY
        if (gesture === "new") {
            var rawW = Math.max(1, Math.abs(dx))
            var rawH = Math.max(1, Math.abs(dy))
            var maxW = dx >= 0 ? (frame ? frame.w : width) - pressX : pressX
            var maxH = dy >= 0 ? (frame ? frame.h : height) - pressY : pressY
            var originX = dx >= 0 ? pressX : pressX - maxW
            var originY = dy >= 0 ? pressY : pressY - maxH
            var size = fitSize(rawW, rawH, Math.abs(dx) >= Math.abs(dy) ? "width" : "height",
                                lock, originX, originY)
            var w = Math.max(2, Math.min(maxW, size.w))
            var h = Math.max(2, Math.min(maxH, size.h))
            rectX = dx >= 0 ? pressX : pressX - w
            rectY = dy >= 0 ? pressY : pressY - h
            rectW = w
            rectH = h
        } else if (gesture === "move") {
            rectX = Math.max(0, Math.min((frame ? frame.w : width) - startW, startX + dx))
            rectY = Math.max(0, Math.min((frame ? frame.h : height) - startH, startY + dy))
        } else if (gesture !== "") {
            var left = gesture.indexOf("l") >= 0
            var right = gesture.indexOf("r") >= 0
            var top = gesture.indexOf("t") >= 0
            var bottom = gesture.indexOf("b") >= 0
            var draggedWidth = startW + (left ? -dx : (right ? dx : 0))
            var draggedHeight = startH + (top ? -dy : (bottom ? dy : 0))
            var anchored = fitSize(Math.max(2, draggedWidth), Math.max(2, draggedHeight),
                                   Math.abs(dx) >= Math.abs(dy) ? "width" : "height", lock)
            if (left)
                rectX = startX + startW - anchored.w
            if (top)
                rectY = startY + startH - anchored.h
            rectW = anchored.w
            rectH = anchored.h
        }
        canvas.requestPaint()
    }

    function endGesture(shift) {
        var finished = gesture
        gesture = ""
        if (mode === "window") {
            if (current >= 0)
                windowChosen(current, shift, false)
            return
        }
        // A click outside an existing rectangle starts a new, empty one.
        if (finished === "new" && hasRect) {
            rectW = Math.max(2, rectW)
            rectH = Math.max(2, rectH)
        }
        canvas.requestPaint()
    }

    function accept() {
        if (!hasRect)
            return
        accepted(rectX, rectY, rectW, rectH)
    }

    function nudge(dx, dy, shift) {
        if (!hasRect)
            return
        if (shift) {
            var lock = ratio > 0 ? ratio : 0
            var size = fitSize(rectW + dx, rectH + dy,
                               Math.abs(dx) >= Math.abs(dy) ? "width" : "height", lock)
            rectW = Math.max(2, Math.min(size.w, (frame ? frame.w : width) - rectX))
            rectH = Math.max(2, Math.min(size.h, (frame ? frame.h : height) - rectY))
        } else {
            rectX = Math.max(0, Math.min((frame ? frame.w : width) - rectW, rectX + dx))
            rectY = Math.max(0, Math.min((frame ? frame.h : height) - rectH, rectY + dy))
        }
        canvas.requestPaint()
    }

    function setAspect(locked) {
        ratio = locked && hasRect ? rectW / rectH : 0
    }

    function toggleAspect() {
        setAspect(ratio <= 0)
    }

    /// The window row at a local point, honouring the focus-sorted order.
    function rowAt(px, py) {
        for (var index = 0; index < rows.length; ++index) {
            var row = rows[index]
            if (px >= row.x && px <= row.x + row.w && py >= row.y && py <= row.y + row.h)
                return index
        }
        return -1
    }

    onVisibleChanged: {
        if (visible && takeFocus) {
            overlay.requestActivate()
            keys.forceActiveFocus()
        }
    }

    Shortcut {
        sequences: ["Escape"]
        enabled: overlay.visible
        onActivated: overlay.cancelled()
    }

    Canvas {
        id: canvas
        anchors.fill: parent
        onPaint: {
            var context = getContext("2d")
            context.reset()
            if (overlay.frame && overlay.frame.url !== "")
                context.drawImage(overlay.frame.url, 0, 0, canvas.width, canvas.height)
            else {
                context.fillStyle = "rgba(20, 20, 20, 0.35)"
                context.fillRect(0, 0, canvas.width, canvas.height)
            }
            // The source dims everything by 75/255.
            context.fillStyle = "rgba(0, 0, 0, 0.294117647)"
            context.fillRect(0, 0, canvas.width, canvas.height)
            if (overlay.hasRect) {
                if (overlay.frame && overlay.frame.url !== "") {
                    context.save()
                    context.beginPath()
                    context.rect(overlay.rectX, overlay.rectY, overlay.rectW, overlay.rectH)
                    context.clip()
                    context.drawImage(overlay.frame.url, 0, 0, canvas.width, canvas.height)
                    context.restore()
                }
                // A one-pixel accent outline, drawn on the rectangle itself.
                context.strokeStyle = overlay.accent
                context.lineWidth = 1
                context.strokeRect(overlay.rectX + 0.5, overlay.rectY + 0.5,
                                   overlay.rectW - 1, overlay.rectH - 1)
                // Eight 6x6 handles: white with an accent border, at the
                // corners and the edge midpoints, as the source paints them.
                var handles = overlay.handlePoints()
                for (var h = 0; h < handles.length; ++h) {
                    context.fillStyle = "white"
                    context.fillRect(handles[h].x - 3, handles[h].y - 3, 6, 6)
                    context.strokeStyle = overlay.accent
                    context.lineWidth = 1
                    context.strokeRect(handles[h].x - 3.5, handles[h].y - 3.5, 6, 6)
                }
                // The size chip: logical size and the captured pixel size,
                // 28 px above the rectangle and clamped into the screen.
                var label = overlay.sizeLabel()
                context.font = "14px sans-serif"
                var labelWidth = context.measureText(label).width + 16
                var labelX = Math.max(8, Math.min(overlay.rectX,
                                                canvas.width - labelWidth - 8))
                var labelY = Math.max(5, Math.min(overlay.rectY - 28,
                                                  canvas.height - 29))
                context.fillStyle = "#1e1e1e"
                context.fillRect(labelX, labelY, labelWidth, 24)
                context.fillStyle = "white"
                context.fillText(label, labelX + 8, labelY + 16)
            }
            // Window mode highlights the client rectangle under the pointer.
            if (overlay.mode === "window" && overlay.current >= 0
                    && overlay.current < overlay.rows.length) {
                var row = overlay.rows[overlay.current]
                context.strokeStyle = overlay.accent
                context.lineWidth = 2
                context.strokeRect(row.x + 1, row.y + 1, row.w - 2, row.h - 2)
            }
            // The crosshair follows the pointer until a rectangle exists.
            if (!overlay.hasRect && overlay.mode !== "window") {
                context.strokeStyle = "rgba(255, 255, 255, 0.568627451)"
                context.lineWidth = 1
                context.beginPath()
                context.moveTo(overlay.pointerX, 0)
                context.lineTo(overlay.pointerX, canvas.height)
                context.moveTo(0, overlay.pointerY)
                context.lineTo(canvas.width, overlay.pointerY)
                context.stroke()
            }
            // The instruction line sits at 24,36 with no band behind it.
            context.fillStyle = "white"
            context.font = "14px sans-serif"
            context.fillText(overlay.hint, 24, 36, Math.min(780, canvas.width - 48))
        }
    }

    // The magnifier inset: the frozen frame's own pixels, nearest-neighbour.
    Rectangle {
        id: magnifier
        objectName: "selectionMagnifier"
        visible: overlay.mode !== "window"
        width: overlay.magnifierSize
        height: overlay.magnifierSize
        border.color: overlay.accent
        border.width: 2
        color: "black"
        x: Math.min(canvas.width - width - 12, overlay.pointerX + 22)
        y: Math.max(10, Math.min(canvas.height - 170, overlay.pointerY + 22))

        Canvas {
            id: magnifierCanvas
            objectName: "selectionMagnifierCanvas"
            anchors.fill: parent
            onPaint: {
                var context = getContext("2d")
                context.reset()
                if (!overlay.frame || overlay.frame.url === "")
                    return
                var source = overlay.magnifierRect()
                context.drawImage(overlay.frame.url, source.x, source.y, source.w, source.h,
                                  0, 0, width, height)
                context.strokeStyle = overlay.accent
                context.lineWidth = 1
                context.beginPath()
                context.moveTo(width / 2, 0)
                context.lineTo(width / 2, height)
                context.moveTo(0, height / 2)
                context.lineTo(width, height / 2)
                context.stroke()
            }
        }
    }

    // The exact-size fields and the aspect lock.
    Rectangle {
        objectName: "selectionFields"
        visible: overlay.mode !== "window"
        z: 2
        x: 12
        y: canvas.height - height - 12
        width: fieldRow.width + 20
        height: fieldRow.height + 16
        color: Qt.rgba(0.12, 0.12, 0.12, 0.92)
        radius: 6
        border.color: "#3a3a3a"

        Row {
            id: fieldRow
            anchors.centerIn: parent
            spacing: 8

            TextInput {
                id: widthField
                objectName: "selectionWidthField"
                width: 58
                text: overlay.hasRect ? String(overlay.rectW) : ""
                color: "white"
                font.pixelSize: 13
                selectByMouse: true
                inputMethodHints: Qt.ImhDigitsOnly
                KeyNavigation.tab: heightField
                KeyNavigation.backtab: ratioLock
                onEditingFinished: overlay.applySize(parseInt(text) || 0,
                                                     parseInt(heightField.text) || 0, "width")
            }
            Text {
                text: "×"
                color: "#9a9a9a"
                anchors.verticalCenter: parent.verticalCenter
            }
            TextInput {
                id: heightField
                objectName: "selectionHeightField"
                width: 58
                text: overlay.hasRect ? String(overlay.rectH) : ""
                color: "white"
                font.pixelSize: 13
                selectByMouse: true
                inputMethodHints: Qt.ImhDigitsOnly
                KeyNavigation.tab: ratioLock
                KeyNavigation.backtab: widthField
                onEditingFinished: overlay.applySize(parseInt(widthField.text) || 0,
                                                     parseInt(text) || 0, "height")
            }
            CheckBox {
                id: ratioLock
                objectName: "selectionAspectLock"
                text: "Lock ratio"
                checked: overlay.ratio > 0
                onClicked: overlay.setAspect(checked)
            }
        }
    }

    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        onPressed: function (mouse) {
            if (mouse.button === Qt.RightButton) {
                overlay.cancelled()
                return
            }
            overlay.beginGesture(mouse.x, mouse.y,
                                 (mouse.modifiers & Qt.ShiftModifier) !== 0)
        }
        onPositionChanged: function (mouse) {
            overlay.updateGesture(mouse.x, mouse.y,
                                  (mouse.modifiers & Qt.ShiftModifier) !== 0)
        }
        onReleased: function (mouse) {
            overlay.endGesture((mouse.modifiers & Qt.ShiftModifier) !== 0)
        }
        onDoubleClicked: overlay.accept()
    }

    FocusScope {
        id: keys
        anchors.fill: parent
        focus: overlay.visible
        Keys.onPressed: function (event) {
            if (event.key === Qt.Key_Escape) {
                overlay.cancelled()
                event.accepted = true
            } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                overlay.accept()
                event.accepted = true
            } else if (event.key === Qt.Key_Left) {
                overlay.nudge(-1, 0, (event.modifiers & Qt.ShiftModifier) !== 0)
                event.accepted = true
            } else if (event.key === Qt.Key_Right) {
                overlay.nudge(1, 0, (event.modifiers & Qt.ShiftModifier) !== 0)
                event.accepted = true
            } else if (event.key === Qt.Key_Up) {
                overlay.nudge(0, -1, (event.modifiers & Qt.ShiftModifier) !== 0)
                event.accepted = true
            } else if (event.key === Qt.Key_Down) {
                overlay.nudge(0, 1, (event.modifiers & Qt.ShiftModifier) !== 0)
                event.accepted = true
            } else if (event.key === Qt.Key_A) {
                overlay.toggleAspect()
                event.accepted = true
            }
        }
    }
}
