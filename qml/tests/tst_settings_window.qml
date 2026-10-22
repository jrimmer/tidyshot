// Offscreen behavior tests for the settings window.
//
// The window is driven with a real model document, then synthetic edits are
// asserted through the signals it emits. Nothing here reads internal state:
// every check is on an emitted save or a visible control.
import QtQuick
import QtTest
import ".."

TestCase {
    id: testcase
    name: "SettingsWindow"
    when: windowShown

    // The settings window is its own top-level Window, not an Item, so it is
    // created without a parent and torn down after the suite.
    Component {
        id: settingsComponent
        SettingsWindow {
            width: 900
            height: 700
            visible: true
        }
    }
    property var settings: null

    function initTestCase() {
        settings = settingsComponent.createObject(null)
        verify(settings !== null)
    }

    function cleanupTestCase() {
        settings.destroy()
        settings = null
    }

    SignalSpy { id: valueSpy; target: settings; signalName: "valueChosen" }
    SignalSpy { id: shortcutSpy; target: settings; signalName: "shortcutChosen" }
    SignalSpy { id: resetSpy; target: settings; signalName: "resetShortcutsRequested" }
    SignalSpy { id: closedSpy; target: settings; signalName: "closed" }

    readonly property string model: JSON.stringify({
        "pages": [
            {
                "id": "general",
                "title": "General",
                "fields": [
                    {"key": "capture_delay", "label": "Self-timer delay", "kind": "number",
                     "value": 3, "min": 0, "max": 60, "step": 1, "choices": [],
                     "tooltip": "", "restart": false},
                    {"key": "filename_format", "label": "Capture filename", "kind": "text",
                     "value": "shot", "min": 0, "max": 0, "step": 0, "choices": [],
                     "tooltip": "", "restart": false}
                ]
            },
            {
                "id": "quickaccess",
                "title": "Quick Access",
                "fields": [
                    {"key": "preview_corner", "label": "Preview corner", "kind": "choice",
                     "value": "bottom-right", "min": 0, "max": 0, "step": 0,
                     "choices": ["bottom-left", "bottom-right", "top-left", "top-right"],
                     "tooltip": "", "restart": false},
                    {"key": "close_after_drag", "label": "Close after drag", "kind": "bool",
                     "value": true, "min": 0, "max": 0, "step": 0, "choices": [],
                     "tooltip": "", "restart": false}
                ]
            },
            {
                "id": "advanced",
                "title": "Advanced",
                "fields": [
                    {"key": "hardware_video_decoding", "label": "Hardware decoding", "kind": "bool",
                     "value": false, "min": 0, "max": 0, "step": 0, "choices": [],
                     "tooltip": "Applies after TidyShot restarts.", "restart": true}
                ]
            },
            {"id": "shortcuts", "title": "Shortcuts", "fields": []},
            {"id": "about", "title": "About", "fields": []}
        ],
        "shortcuts": [
            {"id": "print", "label": "Capture menu", "key": "PRINT", "defaultKey": "PRINT"},
            {"id": "area", "label": "Capture area", "key": "CTRL + PRINT", "defaultKey": "CTRL + PRINT"}
        ],
        "tab": "general",
        "message": "",
        "uiScale": 1
    })

    function init() {
        valueSpy.clear()
        shortcutSpy.clear()
        resetSpy.clear()
        closedSpy.clear()
        settings.visible = true
        settings.modelJson = ""
        settings.modelJson = testcase.model
        settings.selectTab("general")
    }

    function child(name) {
        return findChild(settings.contentItem, name)
    }

    function test_the_window_is_a_top_level_window() {
        verify(settings instanceof Window)
        compare(settings.flags & Qt.Window, Qt.Window)
        compare(settings.title, "TidyShot Settings")
    }

    function test_closing_reports_to_the_singleton() {
        mouseClick(child("closeSettingsButton"), 5, 5)
        compare(closedSpy.count, 1)
        compare(settings.visible, false)
    }

    function test_every_page_renders_its_button() {
        for (var index = 0; index < settings.pages.length; ++index) {
            var id = settings.pages[index].id
            verify(child("page_" + id) !== null, "page button for " + id)
        }
    }

    function test_a_number_field_emits_a_number_save() {
        var slider = child("field_capture_delay")
        verify(slider !== null)
        slider.value = 12
        slider.moved()
        compare(valueSpy.count, 1)
        compare(valueSpy.signalArguments[0][0], "capture_delay")
        compare(valueSpy.signalArguments[0][1], "12")
    }

    function test_a_text_field_emits_a_json_string_save() {
        var input = child("field_filename_format")
        verify(input !== null)
        input.text = "TidyShot %S"
        input.editingFinished()
        compare(valueSpy.count, 1)
        compare(valueSpy.signalArguments[0][0], "filename_format")
        compare(JSON.parse(valueSpy.signalArguments[0][1]), "TidyShot %S")
    }

    function test_a_bool_field_emits_a_bool_save() {
        settings.selectTab("quickaccess")
        waitForRendering(settings.contentItem)
        var check = child("field_close_after_drag")
        verify(check !== null)
        compare(check.checked, true)
        mouseClick(check, 5, 5)
        compare(check.checked, false)
        compare(valueSpy.count, 1)
        compare(valueSpy.signalArguments[0][0], "close_after_drag")
        compare(valueSpy.signalArguments[0][1], "false")
    }

    function test_a_choice_field_emits_the_selected_value() {
        settings.selectTab("quickaccess")
        waitForRendering(settings.contentItem)
        var combo = child("field_preview_corner")
        verify(combo !== null)
        combo.currentIndex = 2
        combo.activated(2)
        compare(valueSpy.count, 1)
        compare(valueSpy.signalArguments[0][0], "preview_corner")
        compare(JSON.parse(valueSpy.signalArguments[0][1]), "top-left")
    }

    function test_the_shortcut_page_lists_the_managed_block() {
        settings.selectTab("shortcuts")
        waitForRendering(settings.contentItem)
        var field = child("shortcut_area")
        verify(field !== null)
        compare(field.text, "CTRL + PRINT")
        field.text = "SUPER + A"
        field.editingFinished()
        compare(shortcutSpy.count, 1)
        compare(shortcutSpy.signalArguments[0][0], "area")
        compare(shortcutSpy.signalArguments[0][1], "SUPER + A")
    }

    function test_restore_defaults_is_reachable() {
        settings.selectTab("shortcuts")
        waitForRendering(settings.contentItem)
        mouseClick(child("resetShortcutsButton"), 5, 5)
        compare(resetSpy.count, 1)
    }

    function test_the_advanced_page_carries_the_restart_tooltip() {
        settings.selectTab("advanced")
        waitForRendering(settings.contentItem)
        var field = settings.fieldFor("hardware_video_decoding")
        verify(field !== null)
        compare(field.restart, true)
        verify(field.tooltip.indexOf("restarts") >= 0)
    }

    function test_the_about_page_renders() {
        settings.selectTab("about")
        waitForRendering(settings.contentItem)
        verify(child("aboutVersion") !== null)
        compare(child("pageTitle").text, "About")
    }
}
