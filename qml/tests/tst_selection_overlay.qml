// Offscreen behavior tests for the area selection overlay.
//
// The component receives its frozen frame and window rows as plain data,
// exactly as Main.qml binds them from the Rust Capture singleton. Synthetic
// pointer and key events assert the observable rectangle, fields and signals.
import QtQuick
import QtTest
import ".."

TestCase {
    id: testcase
    name: "SelectionOverlay"
    when: windowShown

    Window {
        id: host
        width: 800
        height: 600
        visible: true
        SelectionOverlay {
            id: overlay
            frame: ({ "index": 0, "name": "DP-1", "x": 0, "y": 0, "w": 800, "h": 600,
                      "iw": 1600, "ih": 1200, "url": "" })
            accent: "#ff0000"
        }
    }

    SignalSpy { id: acceptedSpy; target: overlay; signalName: "accepted" }
    SignalSpy { id: cancelledSpy; target: overlay; signalName: "cancelled" }
    SignalSpy { id: windowSpy; target: overlay; signalName: "windowChosen" }

    function init() {
        acceptedSpy.clear()
        cancelledSpy.clear()
        windowSpy.clear()
        overlay.mode = "area"
        overlay.rows = []
        overlay.rectX = 0
        overlay.rectY = 0
        overlay.rectW = 0
        overlay.rectH = 0
        overlay.ratio = 0
        overlay.current = -1
        overlay.gesture = ""
        overlay.zoom = 6
    }

    function drag(fromX, fromY, toX, toY, shift) {
        overlay.beginGesture(fromX, fromY, shift === true)
        overlay.updateGesture(toX, toY, shift === true)
        overlay.endGesture(shift === true)
    }

    function test_the_overlay_draws_its_frame_and_magnifier() {
        compare(overlay.width, 800)
        compare(overlay.height, 600)
        verify(findChild(overlay, "selectionMagnifier") !== null)
        verify(findChild(overlay, "selectionMagnifierCanvas") !== null)
        // Painting the magnifier canvas over an empty frame must not fail.
        overlay.pointerX = 400
        overlay.pointerY = 300
        var magnifier = findChild(overlay, "selectionMagnifierCanvas")
        magnifier.requestPaint()
        waitForRendering(magnifier)
    }

    function test_a_drag_sets_the_rectangle() {
        drag(100, 120, 300, 420)
        compare(overlay.rectX, 100)
        compare(overlay.rectY, 120)
        compare(overlay.rectW, 200)
        compare(overlay.rectH, 300)
        verify(overlay.hasRect)
    }

    function test_a_backwards_drag_still_yields_a_positive_rectangle() {
        drag(500, 500, 400, 350)
        compare(overlay.rectX, 400)
        compare(overlay.rectY, 350)
        compare(overlay.rectW, 100)
        compare(overlay.rectH, 150)
    }

    function test_shift_squares_the_drag_and_the_lock_keeps_the_ratio() {
        drag(100, 100, 400, 200, true)
        compare(overlay.rectW, overlay.rectH)
        overlay.rectX = 100
        overlay.rectY = 100
        overlay.rectW = 400
        overlay.rectH = 200
        overlay.setAspect(true)
        compare(overlay.ratio, 2)
        overlay.setAspect(false)
        compare(overlay.ratio, 0)
    }

    function test_the_size_fields_track_the_rectangle() {
        drag(100, 120, 300, 420)
        var widthField = findChild(overlay, "selectionWidthField")
        var heightField = findChild(overlay, "selectionHeightField")
        verify(widthField !== null)
        verify(heightField !== null)
        compare(widthField.text, "200")
        compare(heightField.text, "300")
        // A typed size drives the rectangle.
        widthField.text = "500"
        heightField.text = "250"
        overlay.applySize(500, 250, "width")
        compare(overlay.rectW, 500)
        compare(overlay.rectH, 250)
    }

    function test_the_aspect_lock_is_a_field() {
        drag(0, 0, 400, 200)
        var lock = findChild(overlay, "selectionAspectLock")
        verify(lock !== null)
        mouseClick(lock, 5, 5)
        compare(overlay.ratio, 2)
        mouseClick(lock, 5, 5)
        compare(overlay.ratio, 0)
    }

    function test_enter_accepts_and_escape_cancels() {
        drag(100, 120, 300, 420)
        keyClick(Qt.Key_Return)
        compare(acceptedSpy.count, 1)
        compare(acceptedSpy.signalArguments[0][0], 100)
        compare(acceptedSpy.signalArguments[0][2], 200)
        keyClick(Qt.Key_Escape)
        compare(cancelledSpy.count, 1)
    }

    function test_arrows_nudge_and_shift_arrows_resize() {
        overlay.rectX = 100
        overlay.rectY = 100
        overlay.rectW = 200
        overlay.rectH = 100
        overlay.nudge(10, 5, false)
        compare(overlay.rectX, 110)
        compare(overlay.rectY, 105)
        compare(overlay.rectW, 200)
        overlay.nudge(10, 5, true)
        compare(overlay.rectW, 210)
        compare(overlay.rectH, 105)
    }

    function test_the_magnifier_samples_the_frozen_frame() {
        overlay.pointerX = 400
        overlay.pointerY = 300
        var source = overlay.magnifierRect()
        // The frame is 2x the logical size, so the pointer maps to 800 x 600.
        var point = overlay.imagePoint(400, 300)
        compare(point.x, 800)
        compare(point.y, 600)
        // The source samples a 24 px square at the pointer.
        var span = 24
        verify(Math.abs(source.x - (800 - span / 2)) < 0.001, "source x " + source.x)
        verify(Math.abs(source.w - span) < 0.001)
        // A pointer at the edge clamps instead of wrapping.
        var edge = overlay.magnifierRect(0, 0)
        compare(edge.x, 0)
        compare(edge.y, 0)
    }

    function test_window_mode_hit_tests_rows_and_shift_keeps_transparency() {
        overlay.mode = "window"
        overlay.rows = [
            { "title": "Editor", "app": "code", "x": 10, "y": 20, "w": 200, "h": 150 },
            { "title": "Browser", "app": "firefox", "x": 300, "y": 200, "w": 300, "h": 200 }
        ]
        compare(overlay.rowAt(320, 220), 1)
        compare(overlay.rowAt(700, 500), -1)
        overlay.beginGesture(320, 220, false)
        overlay.updateGesture(320, 220, false)
        compare(overlay.current, 1)
        overlay.endGesture(true)
        compare(windowSpy.count, 1)
        compare(windowSpy.signalArguments[0][0], 1)
        compare(windowSpy.signalArguments[0][1], true)
    }
    // The source geometry the port promises: eight handles, a pixel size in
    // the chip, a 100px magnifier over a 24px sample, and the source hint.
    function test_the_source_geometry_is_preserved() {
        overlay.rectX = 100
        overlay.rectY = 120
        overlay.rectW = 200
        overlay.rectH = 100
        compare(overlay.magnifierSize, 100)
        compare(overlay.magnifierSample, 24)
        compare(overlay.handlePoints().length, 8)
        var points = overlay.handlePoints()
        compare(points[0].x, 100)
        compare(points[0].y, 120)
        compare(points[4].x, 200)   // top midpoint
        compare(points[6].y, 170)   // left midpoint
        var label = overlay.sizeLabel()
        verify(label.indexOf("200 × 100") === 0)
        verify(label.indexOf(" px)") > 0)
        verify(overlay.hint.indexOf("Shift locks proportions") > 0)
    }

    function test_the_magnifier_follows_the_pointer() {
        overlay.pointerX = 400
        overlay.pointerY = 300
        var mag = findChild(overlay, "selectionMagnifier")
        // +22 from the pointer, clamped to the source's bounds.
        compare(mag.x, Math.min(overlay.width - mag.width - 12, 422))
        compare(mag.y, Math.max(10, Math.min(overlay.height - 170, 322)))
    }

    function test_the_magnifier_samples_a_24_pixel_square() {
        overlay.pointerX = 400
        overlay.pointerY = 300
        var source = overlay.magnifierRect()
        compare(source.w, 24)
        compare(source.h, 24)
    }

}
