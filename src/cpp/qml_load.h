// Offscreen QML load probe: loads a document in a fresh engine and reports
// QQmlApplicationEngine::rootObjects(). No event loop runs, so a test can
// assert that Main.qml loaded instead of hanging on a window.
#pragma once

#include <QtCore/QString>
#include <cstdint>

std::int32_t tidyshot_qml_root_count(const QString &url);
