# iPad ink: Phase 0 native spike

What embedding native PencilKit and PDFKit in Flutter actually does, measured
for #209's Phase 0 (ipad.md §2). Gelato (iPad B), 2026-11-21. The spike code
stays local and unlanded, because it edits the Runner project, AppDelegate and
Info.plist; Phase 2 builds the real surfaces from these findings.

## How it was measured

- **Builds:** Claktory Dev built locally with `TARGETED_DEVICE_FAMILY = 1,2` and
  four iPad orientations, against Flutter 3.50.0 and Xcode 27.2.
- **Simulators:** "Yuki iPad Pro 13" (M5) and "Yuki iPad mini" (A17 Pro), both
  iPadOS 27.2.
- **Surfaces:** a `UiKitView` holding either a PencilKit canvas (`InkView`) or a
  PDFView with a canvas on each page through `PDFPageOverlayViewProvider`
  (`PaperView`). Around them sit the Flutter structures the workbench uses: a
  horizontal-swipe drawer, a vertical list, rounded clips, a card and a popup
  menu over the canvas, a text field, animated column widths, column moves kept
  with a `GlobalKey`, and rotation.
- **Input:** real UIKit touches from an XCUITest driver. Every layer logs to one
  file on the native clock (`UITouch.timestamp`): a passive recognizer on each
  native view, PencilKit's delegate, PDFKit's scroll, zoom and page, and Dart.
- **Configurations:** each scenario ran under Flutter's three blocking policies
  (eager, wait until touches end, don't block), each with and without claiming
  the gesture through an `EagerGestureRecognizer`.
- **Screenshots:** results were read from screenshots by pixel and by OCR, never
  assumed.

**The simulator can't judge Pencil.** Its touches are fingers (`UITouch.type`
direct). Pencil pressure, tilt, hover, palm rejection, double-tap, squeeze and
real input-to-ink latency still need a Pencil-capable iPad (ipad.md §8.6).

## Findings

### Touches and gestures

| Scenario | Claim with EagerGestureRecognizer | No claim |
|---|---|---|
| Stroke on the canvas | Draws, under every policy | Draws |
| Horizontal stroke on a canvas inside a swipe drawer | Draws; the drawer stays shut, under every policy | eager/wait: the drawer opens and **no ink**; don't block: the drawer opens **and** it draws |
| Vertical stroke on a canvas inside a scrolling list | Draws; the list doesn't move | eager/wait: the list scrolls, no ink; don't block: the list scrolls **and** a stroke is drawn |
| Later strokes once the arena has settled | Drawing begins normally | Drawing begins only ~1.1–1.3 s later |
| Finger drag on a PDF (pencil_only) | Scrolls at once (pan begins 35–55 ms after the first move); no ink | Pan delayed ~0.8 s inside a drawer |
| Finger drag on a PDF (any_input) | Draws on the page; doesn't scroll | — |

- **Decision:** register the factory with the **eager** policy and always claim
  the surface's touches with an `EagerGestureRecognizer`. The three policies
  behave the same once the surface claims, so eager, the default, is enough. A
  surface that doesn't claim loses ink to Flutter ancestors; with "don't block"
  both handle the same touch.
- **Cost:** none measurable. PencilKit's tool starts 1–33 ms after touch-down
  inside the platform view, against 1–35 ms for the same canvas placed straight
  in the window with no Flutter in between. PencilKit's drawing recognizer
  reports "began" about 410–440 ms after the first move in both cases, so that
  delay comes from synthetic touches, not from Flutter.
- **Pinch on a PDF:** a two-finger pinch was recognized only ~2.3 s in, and the
  scale didn't change, the same under every policy and claim. The simulator's
  synthetic pinch can't tell whether that's Flutter. **Check on a device.** In
  any_input mode the pinch also left a stroke; PencilKit should reject
  multi-touch, so check that on hardware too.

### Layout

- **Resizing:** an animated column change from 1032 to 672 pt keeps the native
  view's bounds within ~3 ms of Flutter's layout, every frame. PDFView re-fits
  each frame, and every page overlay stays exactly on its page.
- **Rotation:** portrait, landscape and upside-down all work. In landscape the
  Flutter view is 1376×1032 and the native view follows in the same frame. Ink
  survives rotation.
- **Moving between columns:** moving a leaf between two columns three times,
  kept with a `GlobalKey`, causes no re-create and no dispose: the same
  PKCanvasView keeps its ink.
- **Clipping:** a `ClipRRect` (radius 48) clips the native view visually, but
  **not for touches**. A stroke that starts in the clipped-off corner still
  draws. Don't rely on a Flutter clip to stop input to the surface; size the
  view itself.
- **Flutter over the canvas:** a card over the canvas takes taps, and a stroke
  starting on the card doesn't reach the canvas. A popup menu opened over the
  canvas draws in full, the same as over plain Flutter.
- **Keyboard and tool picker:** a stroke while a text field has focus draws and
  leaves the focus alone. Showing `PKToolPicker` makes the canvas first
  responder, which **takes focus from the Flutter text field**. Phase 2 needs
  its own tool controls in Flutter, or must hand focus back deliberately;
  ipad.md also asks for Undo/Redo outside the compact picker.

### A stroke in flight during a resize

A stroke that began before a column-width change ended with PencilKit's tool
stopping 50 ms after the change, and **no stroke was committed**. The finger
kept moving for 6 more seconds, and nothing told Dart. This is the drop ipad.md
§2 warns of. Phase 2 must notice a layout change while a tool is active and
report it (`strokeInterrupted`, in the frozen bridge fixtures). `kept: false`
is the honest value until something can keep the partial stroke. Confirm
on a device with Pencil.

### PDF page overlays

- **iOS version:** `pageOverlayViewProvider` and `isInMarkupMode` are iOS 16+.
  With the app's iOS 15 target they compile only behind `@available`, which
  confirms the gate in ipad.md §4 and the fixtures' `pdf_page_overlay` value.
- **Overlays are made lazily:** PDFKit made them for 2–4 nearby pages, not for
  all of them. Drawings must live outside the views (the spike keeps a
  page → PKDrawing map), as §3 says.
- **Spurious callbacks:** assigning a drawing to a fresh overlay fires
  `canvasViewDrawingDidChange` with 0 strokes. Native must ignore its own
  loads, or Dart would save empty revisions.
- **`drawing_to_document` confirmed.** Measured through PDFKit's own
  conversions, overlay to page is `[1, 0, 0, -1, cx, cy + ch]`: points from
  the CropBox's top left, y down, unrotated. It's `[1,0,0,-1,0,792]` on a
  Letter page and the same on a /Rotate 90 page, where the overlay stays in
  unrotated space and PDFKit turns it for display. It's `[1,0,0,-1,72,648]` on a
  page cropped to `[72,144,432,504]`. Circles placed from those numbers land on
  crosses printed at the same page points within ~1 px at @2x (0.5 pt) on all
  three pages.

### Device and window sizes

| Device / mode | Window (pt) | Flutter view | Safe insets t/b | Size class h/v | Idiom |
|---|---|---|---|---|---|
| iPad Pro 13 (M5), full screen portrait | 1032×1376 | same, @2x | 32 / 20 | regular / regular | pad |
| iPad Pro 13, landscape | 1376×1032 | same | 32 / 20 | regular / regular | pad |
| iPad mini (A17 Pro), full screen | 744×1133 | same, @2x | 32 / 20 | regular / regular | pad |
| iPad mini, Windowed Apps | 744 down to **375** × ~1097 | follows live | 32 / 10 | regular ≥ 668, **compact ≤ 662** | pad |
| iPad Pro 13, Windowed Apps, portrait | 1032 down to **375** × ~1340 | follows live | 32 / 10 | regular ≥ 671, **compact ≤ 666** | pad |
| iPad Pro 13, Windowed Apps, landscape | 1376 down to **375** × 1032 | follows live | 32 / 20 | regular ≥ 669, **compact ≤ 667** | pad |

Windows were resized by dragging their corner in Settings → Multitasking &
Gestures → Windowed Apps; they snap in 2–6 pt steps, so a flip is only known
to within that. **The horizontal size class flips at about 668 pt on both
devices and in both orientations**, and no window gets narrower than 375 pt.
The idiom is pad at every size. With today's `isPhoneLayout` (shortest side < 600), a window
narrower than 600 pt would take phone routes. The workbench's split between
idiom and window size avoids that (docs/ipad-workbench.md).
