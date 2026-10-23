// Offscreen behavior tests for one pinned capture window.
//
// The pin receives its state as plain properties, exactly as Main.qml binds
// them from the Rust PinApi singleton. Synthetic events assert the controls
// and the signals the window exposes to the registry.
import QtQuick
import QtTest
import ".."

TestCase {
    id: testcase
    name: "PinWindow"
    when: windowShown

    Component {
        id: pinComponent
        PinWindow {
            visible: true
            path: "/tmp/shot.png"
            name: "shot.png"
            pinWidth: 320
            pinHeight: 200
            opacityValue: 1.0
            locked: false
            clickThrough: false
            source: ""
        }
    }

    property var pin: null

    function initTestCase() {
        pin = pinComponent.createObject(null)
        verify(pin !== null)
    }

    function cleanupTestCase() {
        pin.destroy()
        pin = null
    }

    SignalSpy { id: closeSpy; target: pin; signalName: "closeRequested" }
    SignalSpy { id: lockSpy; target: pin; signalName: "lockRequested" }
    SignalSpy { id: unlockSpy; target: pin; signalName: "unlockRequested" }
    SignalSpy { id: opacitySpy; target: pin; signalName: "opacityChosen" }
    SignalSpy { id: clickSpy; target: pin; signalName: "clickThroughChosen" }
    SignalSpy { id: geometrySpy; target: pin; signalName: "geometryChanged" }
    SignalSpy { id: dragSpy; target: pin; signalName: "dragStarted" }
    SignalSpy { id: resizeSpy; target: pin; signalName: "resizeStarted" }

    function init() {
        closeSpy.clear()
        lockSpy.clear()
        unlockSpy.clear()
        opacitySpy.clear()
        clickSpy.clear()
        geometrySpy.clear()
        dragSpy.clear()
        resizeSpy.clear()
        pin.locked = false
        pin.clickThrough = false
        pin.showShadow = true
        pin.rounded = true
        pin.controlsShown = false
        pin.pinWidth = 320
        pin.pinHeight = 200
        pin.opacityValue = 1.0
        waitForRendering(pin.contentItem)
    }

    function child(name) {
        return findChild(pin.contentItem, name)
    }

    function test_the_window_is_frameless_always_on_top_and_uses_the_pin_size() {
        compare(pin.flags & Qt.FramelessWindowHint, Qt.FramelessWindowHint)
        compare(pin.flags & Qt.WindowStaysOnTopHint, Qt.WindowStaysOnTopHint)
        compare(pin.width, 320)
        compare(pin.height, 200)
        compare(pin.visible, true)
    }

    function test_the_image_shadow_and_rounding_are_drawn() {
        verify(child("pinImage") !== null)
        verify(child("pinFrame") !== null)
        compare(child("pinShadow").visible, true)
        compare(child("pinFrame").radius, 8)
        pin.showShadow = false
        compare(child("pinShadow").visible, false)
        pin.rounded = false
        compare(child("pinFrame").radius, 0)
    }

    function test_a_click_through_pin_ignores_pointer_input() {
        compare(pin.flags & Qt.WindowTransparentForInput, 0)
        pin.clickThrough = true
        compare(pin.flags & Qt.WindowTransparentForInput, Qt.WindowTransparentForInput)
    }

    function test_hover_shows_the_controls_and_escape_hides_them() {
        mouseMove(pin.contentItem, 160, 100)
        tryCompare(pin, "controlsShown", true)
        compare(child("pinControls").visible, true)
        pin.requestActivate()
        tryCompare(pin, "active", true)
        keyClick(Qt.Key_Escape)
        tryCompare(pin, "controlsShown", false)
        compare(child("pinControls").visible, false)
        // Moving over the pin brings the controls back.
        mouseMove(pin.contentItem, 150, 90)
        compare(pin.controlsShown, true)
    }

    function test_close_and_middle_click_dismiss_the_pin() {
        pin.controlsShown = true
        mouseClick(child("pinClose"), 5, 5)
        compare(closeSpy.count, 1)
        mouseClick(pin.contentItem, 160, 100, Qt.MiddleButton)
        compare(closeSpy.count, 2)
    }

    function test_lock_switches_to_unlock_and_hides_close() {
        pin.controlsShown = true
        compare(child("pinLock").visible, true)
        compare(child("pinClose").visible, true)
        mouseClick(child("pinLock"), 5, 5)
        compare(lockSpy.count, 1)
        pin.locked = true
        waitForRendering(pin.contentItem)
        compare(child("pinLock").visible, false)
        compare(child("pinClose").visible, false)
        compare(child("pinUnlock").visible, true)
        mouseClick(child("pinUnlock"), 5, 5)
        compare(unlockSpy.count, 1)
        compare(pin.title.indexOf("Locked") >= 0, true)
    }

    function test_the_opacity_and_click_through_controls_report_their_choice() {
        pin.controlsShown = true
        compare(child("pinOpacityLabel").text, "100%")
        mouseClick(child("pinOpacityDown"), 5, 5)
        compare(opacitySpy.count, 1)
        compare(opacitySpy.signalArguments[0][0], 0.9)
        pin.opacityValue = 0.45
        mouseClick(child("pinOpacityUp"), 5, 5)
        compare(opacitySpy.count, 2)
        compare(opacitySpy.signalArguments[1][0], 0.55)
        mouseClick(child("pinClickThrough"), 5, 5)
        compare(clickSpy.count, 1)
        compare(clickSpy.signalArguments[0][0], true)
    }

    function test_dragging_anywhere_starts_a_move() {
        mousePress(pin.contentItem, 160, 100, Qt.LeftButton)
        compare(dragSpy.count, 1)
        mouseRelease(pin.contentItem, 160, 100, Qt.LeftButton)
    }

    function test_each_edge_starts_a_resize() {
        var expected = [
            ["pinResizeLeft", Qt.LeftEdge], ["pinResizeRight", Qt.RightEdge],
            ["pinResizeTop", Qt.TopEdge], ["pinResizeBottom", Qt.BottomEdge]
        ]
        for (var index = 0; index < expected.length; ++index) {
            var handle = child(expected[index][0])
            verify(handle !== null, expected[index][0])
            var pressX = handle.x + handle.width / 2
            var pressY = handle.y + handle.height / 2
            mousePress(pin.contentItem, pressX, pressY, Qt.LeftButton)
            compare(resizeSpy.count, index + 1)
            compare(resizeSpy.signalArguments[index][0], expected[index][1])
            mouseRelease(pin.contentItem, pressX, pressY, Qt.LeftButton)
        }
    }

    function test_a_resize_reports_the_new_geometry() {
        pin.pinWidth = 280
        pin.pinHeight = 160
        tryCompare(pin, "width", 280)
        tryCompare(geometrySpy, "count", 1, 2000)
        compare(geometrySpy.signalArguments[0][2], 280)
        compare(geometrySpy.signalArguments[0][3], 160)
    }
}
