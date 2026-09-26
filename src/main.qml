// main.qml -- Standalone-Start: qmlviewer -fullscreen main.qml
import QtQuick 1.1
import com.nokia.meego 1.1

PageStackWindow {
    id: appWindow
    showStatusBar: false
    showToolBar: false
    initialPage: RadarMap {}
}
