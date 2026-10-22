// Settings: the ten grouped pages, each rendered from the Rust schema.
//
// Layout only. The pages, field kinds, bounds, defaults and the atomic writer
// live in src/settings_page.rs; this window renders `SettingsApi.modelJson`
// and calls `setValue`/`setShortcut` with the field's key. The component is
// free of the `tidyshot` module so qmltestrunner can drive it offscreen.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Window {
    id: settings

    /// The settings model: pages, fields, shortcuts, active tab, message.
    property string modelJson: ""

    /// The Rust singleton these actions reach in the app.
    signal valueChosen(string key, string jsonValue)
    signal shortcutChosen(string id, string key)
    signal resetShortcutsRequested()
    signal reloadRequested()
    signal closed()

    property var model: ({})
    property var pages: []
    property var shortcuts: []
    property string activeTab: "general"
    property string message: ""
    property double uiScale: 1
    property bool dirty: false

    onModelJsonChanged: parseModel()
    Component.onCompleted: parseModel()

    function parseModel() {
        if (modelJson === "") {
            model = ({})
            pages = []
            shortcuts = []
            return
        }
        model = JSON.parse(modelJson)
        settings.pages = model.pages || []
        settings.shortcuts = model.shortcuts || []
        settings.activeTab = model.tab || "general"
        settings.message = model.message || ""
        if (model.uiScale)
            settings.uiScale = model.uiScale
    }

    function pageById(id) {
        for (var index = 0; index < settings.pages.length; ++index) {
            if (settings.pages[index].id === id)
                return settings.pages[index]
        }
        return null
    }

    /// The field object for a key, or null when the page does not own it.
    function fieldFor(key) {
        for (var p = 0; p < settings.pages.length; ++p) {
            var fields = settings.pages[p].fields || []
            for (var f = 0; f < fields.length; ++f) {
                if (fields[f].key === key)
                    return fields[f]
            }
        }
        return null
    }

    /// The current value of a key, from the latest model.
    function valueFor(key) {
        var field = fieldFor(key)
        return field === null ? null : field.value
    }

    /// Write a JSON-encoded scalar for a key.
    function send(key, jsonValue) {
        settings.dirty = true
        settings.valueChosen(key, jsonValue)
    }

    function sendBool(key, value) {
        send(key, value ? "true" : "false")
    }

    function sendNumber(key, value) {
        send(key, String(value))
    }

    function sendText(key, value) {
        send(key, JSON.stringify(value))
    }

    /// Switch page without writing anything.
    function selectTab(id) {
        settings.activeTab = id
    }

    width: Math.max(560, 820 * uiScale)
    height: Math.max(420, 620 * uiScale)
    color: "#1e1e1e"
    title: "TidyShot Settings"
    flags: Qt.Window
    // Closing through the window manager still tells the singleton, so the
    // menu row and `tidyshot settings` agree about whether it is open.
    onClosing: settings.closed()

    function px(value) {
        return Math.max(1, Math.round(value * settings.uiScale))
    }

    RowLayout {
        anchors.fill: parent
        anchors.margins: settings.px(10)
        spacing: settings.px(10)

        // The page list. Every page in the schema is a button, so a new page
        // never needs a second list here.
        Rectangle {
            Layout.preferredWidth: settings.px(180)
            Layout.fillHeight: true
            color: "#242424"
            radius: settings.px(6)

            ListView {
                id: pageList
                objectName: "pageList"
                anchors.fill: parent
                anchors.margins: settings.px(6)
                model: settings.pages
                spacing: settings.px(2)
                delegate: Button {
                    required property var modelData
                    objectName: "page_" + modelData.id
                    width: pageList.width
                    text: modelData.title
                    font.pixelSize: settings.px(13)
                    checkable: true
                    checked: settings.activeTab === modelData.id
                    onClicked: settings.selectTab(modelData.id)
                }
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: settings.px(8)

            Text {
                objectName: "pageTitle"
                Layout.fillWidth: true
                text: {
                    var page = settings.pageById(settings.activeTab)
                    return page === null ? "" : page.title
                }
                color: "white"
                font.bold: true
                font.pixelSize: settings.px(16)
            }

            ScrollView {
                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true

                ColumnLayout {
                    width: parent.width
                    spacing: settings.px(8)

                    // Ordinary pages render their fields from the schema.
                    Repeater {
                        model: {
                            var page = settings.pageById(settings.activeTab)
                            return page === null ? [] : (page.fields || [])
                        }

                        delegate: Loader {
                            required property var modelData
                            Layout.fillWidth: true
                            sourceComponent: modelData.kind === "bool"
                                ? boolRow : modelData.kind === "number"
                                    ? numberRow : modelData.kind === "choice"
                                        ? choiceRow : textRow
                            onLoaded: {
                                item.field = modelData
                                item.uiScale = settings.uiScale
                            }
                        }
                    }

                    // The shortcut page lists the managed bindings block.
                    ColumnLayout {
                        visible: settings.activeTab === "shortcuts"
                        Layout.fillWidth: true
                        spacing: settings.px(6)

                        Repeater {
                            model: settings.activeTab === "shortcuts" ? settings.shortcuts : []

                            delegate: RowLayout {
                                required property var modelData
                                Layout.fillWidth: true
                                spacing: settings.px(6)

                                Text {
                                    Layout.preferredWidth: settings.px(180)
                                    text: modelData.label
                                    color: "white"
                                    font.pixelSize: settings.px(12)
                                }
                                TextField {
                                    objectName: "shortcut_" + modelData.id
                                    Layout.fillWidth: true
                                    text: modelData.key
                                    font.pixelSize: settings.px(12)
                                    placeholderText: modelData.defaultKey
                                    onEditingFinished: {
                                        if (text !== modelData.key)
                                            settings.shortcutChosen(modelData.id, text)
                                    }
                                }
                            }
                        }

                        RowLayout {
                            spacing: settings.px(6)
                            Button {
                                objectName: "resetShortcutsButton"
                                text: "Restore defaults"
                                onClicked: settings.resetShortcutsRequested()
                            }
                            Button {
                                objectName: "reloadShortcutsButton"
                                text: "Reload"
                                onClicked: settings.reloadRequested()
                            }
                        }
                    }

                    // About names the running version and where settings live.
                    ColumnLayout {
                        visible: settings.activeTab === "about"
                        Layout.fillWidth: true
                        spacing: settings.px(4)
                        Text {
                            objectName: "aboutVersion"
                            text: "TidyShot " + Qt.application.version
                            color: "white"
                        }
                        Text {
                            text: "Settings are stored beside the capture history and written atomically."
                            color: "#9a9a9a"
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                        }
                    }
                }
            }

            Text {
                objectName: "settingsMessage"
                Layout.fillWidth: true
                text: settings.message
                color: "#FFD60A"
                wrapMode: Text.WordWrap
                font.pixelSize: settings.px(12)
                visible: text !== ""
            }
        }
    }

    Button {
        objectName: "closeSettingsButton"
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: settings.px(8)
        text: "×"
        onClicked: settings.dismiss()
    }

    /// Close the window; the close handler reports it to the singleton.
    function dismiss() {
        settings.close()
    }

    Component {
        id: boolRow
        RowLayout {
            property var field: null
            property double uiScale: 1
            Layout.fillWidth: true
            spacing: 6
            CheckBox {
                objectName: field === null ? "" : "field_" + field.key
                text: field === null ? "" : field.label
                checked: field !== null && field.value === true
                ToolTip.visible: hovered && field !== null && field.tooltip !== ""
                ToolTip.text: field === null ? "" : field.tooltip
                onToggled: settings.sendBool(field.key, checked)
            }
        }
    }

    Component {
        id: numberRow
        ColumnLayout {
            property var field: null
            property double uiScale: 1
            Layout.fillWidth: true
            spacing: 2
            Text {
                text: field === null ? "" : field.label
                color: "white"
                font.pixelSize: Math.max(11, Math.round(12 * uiScale))
            }
            RowLayout {
                spacing: 6
                Slider {
                    objectName: field === null ? "" : "field_" + field.key
                    Layout.fillWidth: true
                    from: field === null ? 0 : field.min
                    to: field === null ? 1 : field.max
                    stepSize: field === null || field.step <= 0 ? 1 : field.step
                    value: field === null ? 0 : field.value
                    onMoved: settings.sendNumber(field.key, value)
                }
                Text {
                    objectName: field === null ? "" : "fieldValue_" + field.key
                    text: field === null ? "" : String(field.value)
                    color: "#9a9a9a"
                    font.pixelSize: Math.max(11, Math.round(12 * uiScale))
                }
            }
        }
    }

    Component {
        id: choiceRow
        RowLayout {
            property var field: null
            property double uiScale: 1
            Layout.fillWidth: true
            spacing: 6
            Text {
                Layout.preferredWidth: Math.round(220 * uiScale)
                text: field === null ? "" : field.label
                color: "white"
                wrapMode: Text.WordWrap
                font.pixelSize: Math.max(11, Math.round(12 * uiScale))
            }
            ComboBox {
                objectName: field === null ? "" : "field_" + field.key
                Layout.fillWidth: true
                model: field === null ? [] : field.choices
                currentIndex: {
                    if (field === null)
                        return -1
                    return field.choices.indexOf(String(field.value))
                }
                onActivated: function (index) {
                    settings.sendText(field.key, String(field.choices[index]))
                }
            }
        }
    }

    Component {
        id: textRow
        RowLayout {
            property var field: null
            property double uiScale: 1
            Layout.fillWidth: true
            spacing: 6
            Text {
                Layout.preferredWidth: Math.round(220 * uiScale)
                text: field === null ? "" : field.label
                color: "white"
                wrapMode: Text.WordWrap
                font.pixelSize: Math.max(11, Math.round(12 * uiScale))
            }
            TextField {
                objectName: field === null ? "" : "field_" + field.key
                Layout.fillWidth: true
                text: field === null ? "" : String(field.value)
                font.pixelSize: Math.max(11, Math.round(12 * uiScale))
                ToolTip.visible: hovered && field !== null && field.tooltip !== ""
                ToolTip.text: field === null ? "" : field.tooltip
                onEditingFinished: settings.sendText(field.key, text)
            }
        }
    }
}
