#!/usr/bin/python
# -*- coding: utf-8 -*-
# launcher.py -- qmlviewer-Ersatz fuer Harmattan (Python 2.7 + PySide)
#   python launcher.py [pfad/zu/main.qml]
import os
import sys

from PySide.QtCore import QUrl
from PySide.QtGui import QApplication
from PySide.QtDeclarative import QDeclarativeView

try:                       # GL-Viewport, falls PySide.QtOpenGL da ist
    from PySide.QtOpenGL import QGLWidget, QGLFormat
except ImportError:
    QGLWidget = None


def main():
    qml = sys.argv[1] if len(sys.argv) > 1 else "main.qml"
    qml = os.path.abspath(qml)

    app = QApplication(sys.argv)
    view = QDeclarativeView()

    if QGLWidget is not None:
        fmt = QGLFormat.defaultFormat()
        fmt.setSampleBuffers(False)
        view.setViewport(QGLWidget(fmt))
    else:
        print "kein QtOpenGL -- Software-Rendering"

    view.setResizeMode(QDeclarativeView.SizeRootObjectToView)
    view.setSource(QUrl.fromLocalFile(qml))
    if view.status() == QDeclarativeView.Error:
        for e in view.errors():
            print e
        sys.exit(1)
    view.showFullScreen()
    sys.exit(app.exec_())


if __name__ == "__main__":
    main()
