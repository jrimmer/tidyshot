#include "qml_load.h"

#include <QtCore/QUrl>
#include <QtQml/QQmlApplicationEngine>

std::int32_t
tidyshot_qml_root_count(const QString &url)
{
  QQmlApplicationEngine engine;
  engine.load(QUrl(url));
  return static_cast<std::int32_t>(engine.rootObjects().size());
}
