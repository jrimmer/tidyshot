import QtQuick
import QtQuick.Controls
import QtQuick.Controls.Material
import tidyshot

ApplicationWindow {
    id: window
    width: 480
    height: 620
    visible: true
    title: qsTr("tidyshot")
    color: "#1e1e1e"

    Material.theme: Material.Dark
    Material.accent: "#FFD60A"

    // The bar panel renders the same action table the tray menu and the
    // `tidyshot menu` listing render. Menu is the Rust side of that table;
    // MenuPanel stays free of it so the component lints with QtQuick alone.
    MenuPanel {
        id: menuPanel
        anchors.centerIn: parent
        visible: true
        ids: Menu.ids
        labels: Menu.labels
        shortcuts: Menu.shortcuts
        selectedIndex: Menu.selectedIndex
        onMoved: function (dx, dy) {
            Menu.moveSelection(dx, dy)
        }
        onActivated: function (index) {
            Menu.activateIndex(index)
        }
        onDismissed: Qt.quit()
    }

    Label {
        anchors.bottom: parent.bottom
        anchors.horizontalCenter: parent.horizontalCenter
        text: Args.label
        color: "#9a9a9a"
    }
}
