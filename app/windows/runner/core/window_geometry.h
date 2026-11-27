#ifndef RUNNER_CORE_WINDOW_GEOMETRY_H_
#define RUNNER_CORE_WINDOW_GEOMETRY_H_

// The window's size and place (docs/IDENTITY.md, Window): at least 800 x 500;
// the first launch centered at about 78% x 85% of the screen's work area, at
// most 1440 x 940; after that where the person left it, kept on this device
// (tenet 7's exception), unless that's no longer on any screen.
// In physical pixels, with [scale] the screen's DPI / 96. Standard C++17
// only, so the tests build it anywhere.

#include <optional>
#include <vector>

#include "core/json.h"

namespace claktory {

struct Rect {
  int left = 0;
  int top = 0;
  int right = 0;
  int bottom = 0;

  int width() const { return right - left; }
  int height() const { return bottom - top; }
  bool operator==(const Rect& other) const {
    return left == other.left && top == other.top && right == other.right &&
           bottom == other.bottom;
  }
};

// The smallest the window goes, in logical pixels.
constexpr int kMinWidth = 800;
constexpr int kMinHeight = 500;

// The first launch's frame on a screen whose work area (the screen less the
// taskbar) is [work], at [scale].
Rect DefaultFrame(const Rect& work, double scale);

// The window's saved place, as the runner keeps it in window.json.
struct SavedFrame {
  Rect frame;  // Its restored (not maximized) frame, in workspace pixels.
  bool maximized = false;
};

Json SavedFrameToJson(const SavedFrame& saved);
std::optional<SavedFrame> SavedFrameFromJson(const Json& json);

// Whether a saved [frame] can be shown again on these screens' work areas:
// enough of its top (where it's dragged by) is on one of them, and it's at
// least the smallest size at [scale]. Otherwise the window opens at its
// default frame (a screen unplugged since, say).
bool FrameStillFits(const Rect& frame, const std::vector<Rect>& work_areas,
                    double scale);

}  // namespace claktory

#endif  // RUNNER_CORE_WINDOW_GEOMETRY_H_
