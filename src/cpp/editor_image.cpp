#include "editor_image.h"

#include <QtQuick/QQuickImageProvider>

namespace {

class EditorImageProvider : public QQuickImageProvider
{
public:
  EditorImageProvider()
    : QQuickImageProvider(QQuickImageProvider::Image)
  {
  }

  QImage requestImage(const QString& id, QSize* size, const QSize&) override
  {
    if (id != QStringLiteral("canvas")) {
      return QImage();
    }
    if (size != nullptr) {
      *size = image.size();
    }
    return image;
  }

  QImage image;
};

EditorImageProvider* provider = nullptr;

} // namespace

void
tidyshot_editor_image_install(QQmlEngine& engine)
{
  if (provider == nullptr) {
    provider = new EditorImageProvider();
    engine.addImageProvider(QStringLiteral("tidyshot"), provider);
  }
}

void
tidyshot_editor_image_set(rust::Slice<const std::uint8_t> data,
                          std::int32_t width,
                          std::int32_t height)
{
  if (provider == nullptr || width <= 0 || height <= 0) {
    return;
  }
  const QImage view(data.data(),
                    width,
                    height,
                    width * 4,
                    QImage::Format_RGBA8888);
  // The provider outlives the slice, so the pixels are copied once here.
  provider->image = view.copy();
}
