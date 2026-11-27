#ifndef RUNNER_WINDOW_PLACEMENT_H_
#define RUNNER_WINDOW_PLACEMENT_H_

#include <windows.h>

// The window's size and place across launches (docs/IDENTITY.md, Window;
// core/window_geometry.h): kept on this device in window.json, as the Mac
// keeps its frame in user defaults (tenet 7's exception).

// Where the window opens: its last place if that's still on a screen, else
// its default frame on the screen the pointer is on. [maximized] says whether
// it was left maximized.
RECT InitialWindowFrame(bool* maximized);

// Keeps [window]'s place for next time: its restored frame, and whether it's
// maximized (a minimized window keeps the place it'll restore to).
void SaveWindowPlacement(HWND window);

// The smallest the window goes at [dpi], in physical pixels, for
// WM_GETMINMAXINFO.
POINT MinimumWindowSize(UINT dpi);

#endif  // RUNNER_WINDOW_PLACEMENT_H_
