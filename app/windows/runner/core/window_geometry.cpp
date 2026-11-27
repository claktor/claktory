#include "core/window_geometry.h"

#include <algorithm>
#include <cmath>

namespace claktory {

namespace {

int Scaled(int logical, double scale) {
  return static_cast<int>(std::lround(logical * scale));
}

// How much of the window's top band, in logical pixels, must be on a screen
// for the person to grab it: as much as the band is tall, and wide enough to
// find.
constexpr int kGrabWidth = 120;
constexpr int kGrabHeight = 44;

}  // namespace

Rect DefaultFrame(const Rect& work, double scale) {
  if (scale <= 0) scale = 1;
  int width = static_cast<int>(std::lround(work.width() * 0.78));
  int height = static_cast<int>(std::lround(work.height() * 0.85));
  width = std::min(width, Scaled(1440, scale));
  height = std::min(height, Scaled(940, scale));
  // At least the smallest size, even on a screen smaller than it (the person
  // can still move it).
  width = std::max(width, Scaled(kMinWidth, scale));
  height = std::max(height, Scaled(kMinHeight, scale));
  Rect frame;
  frame.left = work.left + (work.width() - width) / 2;
  frame.top = work.top + std::max(0, (work.height() - height) / 2);
  frame.right = frame.left + width;
  frame.bottom = frame.top + height;
  return frame;
}

Json SavedFrameToJson(const SavedFrame& saved) {
  Json json = Json::MakeMap();
  json.Set("left", Json::Int(saved.frame.left));
  json.Set("top", Json::Int(saved.frame.top));
  json.Set("right", Json::Int(saved.frame.right));
  json.Set("bottom", Json::Int(saved.frame.bottom));
  json.Set("maximized", Json::Bool(saved.maximized));
  return json;
}

std::optional<SavedFrame> SavedFrameFromJson(const Json& json) {
  if (!json.is_map()) return std::nullopt;
  int sides[4] = {0, 0, 0, 0};
  const char* names[4] = {"left", "top", "right", "bottom"};
  for (int i = 0; i < 4; ++i) {
    const Json* side = json.Find(names[i]);
    if (side == nullptr || side->kind() != Json::Kind::kInt) return std::nullopt;
    int64_t value = side->int_value();
    if (value < -1000000 || value > 1000000) return std::nullopt;
    sides[i] = static_cast<int>(value);
  }
  SavedFrame saved;
  saved.frame = Rect{sides[0], sides[1], sides[2], sides[3]};
  if (saved.frame.width() <= 0 || saved.frame.height() <= 0) {
    return std::nullopt;
  }
  const Json* maximized = json.Find("maximized");
  saved.maximized = maximized != nullptr &&
                    maximized->kind() == Json::Kind::kBool &&
                    maximized->bool_value();
  return saved;
}

bool FrameStillFits(const Rect& frame, const std::vector<Rect>& work_areas,
                    double scale) {
  if (scale <= 0) scale = 1;
  // A pixel short of the smallest size is rounding from another scale.
  if (frame.width() < Scaled(kMinWidth, scale) - 2 ||
      frame.height() < Scaled(kMinHeight, scale) - 2) {
    return false;
  }
  Rect band{frame.left, frame.top, frame.right,
            frame.top + Scaled(kGrabHeight, scale)};
  for (const Rect& work : work_areas) {
    int left = std::max(band.left, work.left);
    int right = std::min(band.right, work.right);
    int top = std::max(band.top, work.top);
    int bottom = std::min(band.bottom, work.bottom);
    if (right - left >= Scaled(kGrabWidth, scale) &&
        bottom - top >= Scaled(kGrabHeight, scale) / 2) {
      return true;
    }
  }
  return false;
}

}  // namespace claktory
