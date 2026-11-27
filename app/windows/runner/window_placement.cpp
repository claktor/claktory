#include "window_placement.h"

#include <flutter_windows.h>

#include <cmath>
#include <optional>
#include <vector>

#include "core/window_geometry.h"
#include "file_store.h"

namespace {

constexpr wchar_t kPlacementFile[] = L"window.json";

claktory::Rect FromRect(const RECT& rect) {
  return claktory::Rect{rect.left, rect.top, rect.right, rect.bottom};
}

RECT ToRect(const claktory::Rect& rect) {
  return RECT{rect.left, rect.top, rect.right, rect.bottom};
}

BOOL CALLBACK AddWorkArea(HMONITOR monitor, HDC, LPRECT, LPARAM data) {
  MONITORINFO info{};
  info.cbSize = sizeof(info);
  if (::GetMonitorInfoW(monitor, &info)) {
    reinterpret_cast<std::vector<claktory::Rect>*>(data)->push_back(
        FromRect(info.rcWork));
  }
  return TRUE;
}

double ScaleOf(HMONITOR monitor) {
  return FlutterDesktopGetDpiForMonitor(monitor) / 96.0;
}

}  // namespace

RECT InitialWindowFrame(bool* maximized) {
  *maximized = false;
  std::vector<claktory::Rect> work_areas;
  ::EnumDisplayMonitors(nullptr, nullptr, AddWorkArea,
                        reinterpret_cast<LPARAM>(&work_areas));

  if (std::optional<claktory::Json> kept = JsonFile(kPlacementFile).Read()) {
    if (std::optional<claktory::SavedFrame> saved =
            claktory::SavedFrameFromJson(*kept)) {
      RECT frame = ToRect(saved->frame);
      HMONITOR monitor = ::MonitorFromRect(&frame, MONITOR_DEFAULTTONEAREST);
      if (claktory::FrameStillFits(saved->frame, work_areas, ScaleOf(monitor))) {
        *maximized = saved->maximized;
        return frame;
      }
    }
  }

  POINT pointer{};
  ::GetCursorPos(&pointer);
  HMONITOR monitor = ::MonitorFromPoint(pointer, MONITOR_DEFAULTTOPRIMARY);
  MONITORINFO info{};
  info.cbSize = sizeof(info);
  if (!::GetMonitorInfoW(monitor, &info)) {
    return RECT{0, 0, 1280, 820};
  }
  return ToRect(claktory::DefaultFrame(FromRect(info.rcWork), ScaleOf(monitor)));
}

void SaveWindowPlacement(HWND window) {
  WINDOWPLACEMENT placement{};
  placement.length = sizeof(placement);
  if (!::GetWindowPlacement(window, &placement)) return;
  // rcNormalPosition is in workspace coordinates (the work area's, without
  // the taskbar), while CreateWindow takes screen coordinates: they differ
  // only with the taskbar at the top or left, so it's turned into screen
  // coordinates here.
  RECT frame = placement.rcNormalPosition;
  HMONITOR monitor = ::MonitorFromRect(&frame, MONITOR_DEFAULTTONEAREST);
  MONITORINFO info{};
  info.cbSize = sizeof(info);
  if (::GetMonitorInfoW(monitor, &info)) {
    LONG dx = info.rcWork.left - info.rcMonitor.left;
    LONG dy = info.rcWork.top - info.rcMonitor.top;
    ::OffsetRect(&frame, dx, dy);
  }
  claktory::SavedFrame saved;
  saved.frame = FromRect(frame);
  saved.maximized = placement.showCmd == SW_SHOWMAXIMIZED ||
                    (placement.showCmd == SW_SHOWMINIMIZED &&
                     (placement.flags & WPF_RESTORETOMAXIMIZED) != 0);
  JsonFile(kPlacementFile).Write(claktory::SavedFrameToJson(saved));
}

POINT MinimumWindowSize(UINT dpi) {
  double scale = dpi / 96.0;
  return POINT{static_cast<LONG>(std::lround(claktory::kMinWidth * scale)),
               static_cast<LONG>(std::lround(claktory::kMinHeight * scale))};
}
