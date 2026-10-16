// Focus and key handling wrapper, following plugin/BarWidget.qml conventions
// on the OmniShot mirror branch pr/16. The panel owns no action list of its
// own; it forwards keyboard navigation to the Rust Menu singleton.
import QtQuick

Item {
    id: keyboardPanel

    property Item focusTarget
    property string title: ""

    default property alias content: contentItem.data

    focus: true

    Item {
        id: contentItem
        anchors.fill: parent
    }
}
