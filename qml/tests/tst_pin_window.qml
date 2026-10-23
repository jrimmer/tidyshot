// Offscreen behavior tests for one pinned capture window.
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
    SignalSpy { id: unlockSpy; target: pin; signalName: "unlockRequested" }

    function init() {
        closeSpy.clear()
        unlockSpy.clear()
        pin.locked = false
    }

    function test_the_pin_uses_its_rust_size() {
        compare(pin.width, 320)
        compare(pin.height, 200)
        compare(pin.visible, true)
    }

    function test_close_shows_when_unlocked_and_emits() {
        waitForRendering(pin.contentItem)
        compare(findChild(pin.contentItem, "pinClose").visible, true)
        mouseClick(findChild(pin.contentItem, "pinClose"), 5, 5)
        compare(closeSpy.count, 1)
    }

    function test_a_locked_pin_offers_unlock_instead() {
        pin.locked = true
        waitForRendering(pin.contentItem)
        compare(findChild(pin.contentItem, "pinClose").visible, false)
        compare(findChild(pin.contentItem, "pinUnlock").visible, true)
        mouseClick(findChild(pin.contentItem, "pinUnlock"), 5, 5)
        compare(unlockSpy.count, 1)
    }
}
