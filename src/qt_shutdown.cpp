// Ported from omnishot/app.py (Controller.request_quit)
#include "tidyshot/src/qt_shutdown.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QMetaObject>
#include <QtCore/Qt>

void
tidyshot_request_quit()
{
  // A no-op before Qt exists; the socket only runs after QGuiApplication is
  // constructed, but the guard keeps the helper safe if it is called early.
  if (!QCoreApplication::instance()) {
    return;
  }
  // Qt::QueuedConnection delivers quit() on the event-loop thread, so the
  // QGuiApplication shuts down normally and the socket is removed by drop.
  QMetaObject::invokeMethod(
    QCoreApplication::instance(), "quit", Qt::QueuedConnection);
}
