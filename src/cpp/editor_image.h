// Image provider that publishes the rendered editor canvas to QML.
#pragma once

#include <rust/cxx.h>

#include <cstdint>

#include <QtGui/QImage>
#include <QtQml/QQmlEngine>

void tidyshot_editor_image_install(QQmlEngine& engine);
void tidyshot_editor_image_set(rust::Slice<const std::uint8_t> data,
                               std::int32_t width,
                               std::int32_t height);
