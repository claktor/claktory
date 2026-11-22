# iPad workbench: phase 0 contracts

Petra (iPad A), for #209. The spec is the Research folder's `ipad.md`. This file covers my half of phase 0: how layout and capabilities split, how an agent's layout is projected into columns or routes, and the window-size fixtures. Yuki (iPad B) writes the document and ink fixtures (bridge and evidence). They're described in §6 and live in `app/test/ipad/fixtures/`.

**Status:** these are contracts and pure code only. Nothing here is wired into the app yet, the project still builds for iPhone only (`TARGETED_DEVICE_FAMILY = 1`), and nothing here may be claimed as an iPad feature. Changing the device family is phase 1's gate. Nothing here depends on Matt's open §8 decisions: ink while offline, minimum OS, and ink on reflowable text.

Pinned by code:

- `app/lib/ipad/columns.dart`: the tiers, the projection of a layout into the room a tier leaves, mapping a divider drag back to the layout, and the compact routes.
- `app/test/ipad/window_fixtures.dart`: the window matrix from §7.
- `app/test/ipad/columns_test.dart`: every fixture's tier, plus the projection's rules across the matrix.

## 1. Two axes, not one predicate

Today `isPhoneLayout(context)` (identity.dart) decides two separate things at once:

1. **Composition** (window): phone routes and drawer, or the rail, sidebar and panes.
2. **Input and density** (platform): finger-sized targets, 17 pt reading type, the phone palette, Return adds a line, no autofocus that raises the keyboard, a hold for message actions.

On an iPhone the two always agree. On a wide iPad window they must not: it needs the panes and the finger's sizes together.

### Proposed API (Mei's to land: identity.dart, mobile/metrics.dart)

```dart
/// Unchanged meaning: the window takes the phone's routes.
/// iPhone/Android: shortestSide < 600, as today.
/// iPad idiom: width < 600 or height < 440 (tierFor(...) == compact).
/// Mac/desktop: width < 600, as today.
bool isPhoneLayout(BuildContext context);

/// New: sized and behaving for a finger: iOS or Android at any window
/// size, or any compact window (a Mac window under 600 keeps the phone's
/// sizes, as today).
bool touchFirst(BuildContext context) =>
    isPhoneLayout(context) || defaultTargetPlatform is iOS or Android;

/// New, from the native side (`factory/device` → `capabilities`, once at
/// start): pad, phone, other. Never from the window's size.
Idiom deviceIdiom(BuildContext context);
```

`Metrics.of`, `TypeScale.of` and `Palette.of` key on `touchFirst`. Composition calls keep `isPhoneLayout`.

Mei reviewed this section and agreed it, with these refinements:

- **The idiom is read before `runApp`.** main awaits the channel, and the idiom is a synchronous value from then on. That way a wide iPad never paints one frame at Mac sizes, and a narrow Split View window never paints one frame of phone routes.
- **`capabilities` also says `ios_app_on_mac`** (`ProcessInfo.isiOSAppOnMac`). "Designed for iPad" on an Apple silicon Mac reports the pad idiom with a pointer and no touch, so it must not count as `touchFirst`. We don't ship that today; the field is cheap.
- **`capabilities` also says `hardware_keyboard`**, with a change event (`GCKeyboard.coalesced`, connect and disconnect notifications), because Flutter can't see it. Autofocus (rail 810, forms 681, 1126) follows it: there's no harm in focusing a field when no on-screen keyboard rises.
- **Return in the message box follows the keyboard (Mike's decision, overriding ipad.md §1).** With a hardware keyboard, Return sends and ⇧Return or ⌥Return adds a line, as in Messages and the Mac app's composer. With the on-screen keyboard, Return adds a line. message_box 525 reads the keyboard signal, not the idiom.
- **Mobile/presentation 54/80/97/114 split:** a bottom sheet versus a centered surface is composition (`isPhoneLayout`), and the sizes inside are `touchFirst`. On a wide iPad, action sheets become popovers anchored to their control and forms become centered form sheets, as the HIG has it. That anchoring is new phase 1 work under Mei.
- **No third type ladder.** The workbench uses the existing `TypeScale`/`DenseList` scheme. The iPad sidebar overlay wraps `DenseList` as the phone drawer does (15 pt labels, 36 pt rows), since Matt found the 17 pt drawer too big.

**No phone regression by construction.** On every iPhone and Android phone, `touchFirst == isPhoneLayout` in every orientation, because their shortest side is under 600. On the Mac, `touchFirst == isPhoneLayout` too: false unless the window is under 600. Moving a density or keyboard call site from `isPhoneLayout` to `touchFirst` therefore changes nothing on those devices. It changes only iPads, and Android tablets with a shortest side of 600 or more. **Raj and Katya need to know about the Android tablets:** they gain finger sizes in their wide layout, which is arguably right, but it's their call.

The iPad's compact rule changes from shortest side to width/height, for the pad idiom only. A 1000 × 560 Stage Manager window is a short Mac-like window, not a phone on its side. Today's rule would give it phone routes; under mine, a window 440 pt or taller keeps the workbench. iPhones keep the shortest-side rule.

`device_kind.dart` should take the idiom too. Today a narrow iPad window could report itself as a phone in Hello (§1's last row).

### Call-site audit (main at `682c7e2e`, 170 uses)

Codes: **C** composition, which stays on `isPhoneLayout`. **T** touch geometry or type, which moves to `touchFirst`. **K** keyboard/input policy, which moves to `touchFirst`. **P** palette/contrast, which moves to `touchFirst`. **W** wrapping for narrow width or large text, better asked of the actual width (LayoutBuilder) than of the device. **M** a `phone` variable that drives several of these at once, to split when that file migrates. Each owner migrates their own files; I edit none of them.

| Code | Sites |
|---|---|
| C | main 172; shell 168, 181, 188, 210, 536, 702; scope 91; settings_view 50, 1027, 1372; extensions 207; map 128; connect_step 18; sidebar 362, 907, 1479; annotations 1091, 1135, 1846, 1866; controls 714, 923; message_box 763; transcript 1249; video 619, 798; your_claktories 1084; name_step 272; folder_browser 215; marketplace 66, 108; changelog 54; inbox 126; compatibility 73; tasks 167; monitors 219 |
| C, iPad-specific | controls 82 `TitleSpace`: the Mac's 44 pt window-drag band. An iPad window has no drag band, and its system window controls sit over the top-left corner instead. The workbench needs the window-controls safe area, not this band |
| T | metrics 158, 233; folder_browser 155, 278, 325–326; tool_views 1119–1120; database 143; memory 186; account 682; extensions 623–624; viewers/image 270; annotations 1468; transcript 2622; usage_view 568–569; marketplace 146, 283–284; inbox 346; settings_view 702; changelog 107, 119; secrets 1788; sidebar 1367, 1405, 1582; controls 1789 |
| K | message_box 297, 302 (refocus after voice), 525 (Return adds a line; ⌘Return sends): these follow `hardware_keyboard`, see above; rail 810, forms 681, 1126 (autofocus): `hardware_keyboard` too; transcript 1036 (scrolling dismisses the keyboard) and mobile/message_actions 37 (hold for Copy/Select All; a right click already works): `touchFirst` |
| P | identity 94 (phone palette); transcript 113 (queued opacity ≥ 3:1) |
| W | devices 174, 579; account 349, 435, 563, 1032; your_claktories 253, 326, 486, 552, 939, 954; annotations 2049; forms 1211, 1233–1234, 1255, 1291; settings_view 1130, 1312, 1360; resources 668; secrets 1809; mobile/lines 24, 54; transcript 2487 |
| M | tasks 183; annotations 977, 1517, 2119; tool_views 1075; map 371; transcript 924, 2772; account 265, 467; extensions 470, 667; connectors 1059; monitors 235; message_box 768; forms 65, 1305; memory 106; inbox 845; your_claktories 617, 1147; mobile/presentation 54, 80, 97, 114; viewers/image 449; viewers/video 910; resources 399, 552, 807; name_step 150, 356, 362; secrets 2225; agent_view 787; messaging 153; controls 174, 373, 782, 1758; usage_view 233, 274; chats 260, 675, 756; sidebar 51, 830, 1049, 1117; attachments 235 |

Already right on an iPad, keep as is: `mobile/input.dart::nativeSelectionHere` keys on iOS, not on layout, so a wide iPad already gets native selection and the edit menu.

## 2. What the wide iPad reuses

| From the Mac's pane world | From the phone's primitives | New for the iPad (mine) |
|---|---|---|
| Shell's row: rail, sidebar, extension bar, content, panel | `Metrics`/`TypeScale` touch sizes, the phone palette, through `touchFirst` | The composition seam: which tier, the sidebar as an overlay in the paired tier, and the toolbar and Chat/Document switch in the focused tier |
| `PaneLayout`, `Pane`, `PaneHeader`, the backend's `ViewNode` tree, `open()`/`close()`/`move()`/`resize()` | The keyboard policies (K) | The projection (`columns.dart`) and the pane picker for hidden leaves |
| Settings, Marketplace and the other pages as pane leaves, not pushed pages | Native selection and edit menu (already) | The command registry with discoverable shortcuts (commands.dart), mirrored into the iPadOS menu through a native `UIMenuBuilder` hook |
| Dialogs as bounded, centered surfaces (`showModal` off the phone path) | The whole compact tier: PhoneShell's routes and drawer, unchanged | The document host for Yuki's ink surface; the review column |
| ⌘[ / ⌘] history, pointer hover through `Pressable` | | |

Things the Mac's pane world needs changed for touch (Frontend's files: panes.dart, controls.dart):

- `paneHeaderHeight` 32 → `Metrics.of(context).row` on touch: 44 or more, growing with text.
- `ResizeHandle.width` 8 → a hit area of at least 44 on touch. The visible hairline stays 1 pt.
- Dragging a pane by its header starts with a hold on touch, so a swipe across the header still scrolls.

## 3. The projection

### Tiers (`tierFor`)

The input is the safe window size: the view less its safe area, **never less the keyboard** (a floating keyboard has no meaningful inset), plus the text factor (body size ÷ 17, at least 1). Larger text pushes each threshold by the reading columns it carries.

| Tier | Width at default text | At text factor *s* | Composition |
|---|---|---|---|
| compact | < 600, or height < 440 | same | PhoneShell's routes and drawer, unchanged |
| focused | 600 to paired | — | One pane (the focused leaf); Chat/Document switch; navigation and rail as an overlay |
| paired | ≥ 840 | 840 + (*s*−1)·360 | Rail 56, then the projected panes; the projects sidebar as an overlay |
| full | ≥ 1180 | 1180 + (*s*−1)·(232+360) | Rail 56, the sidebar pinned at the person's width (`Settings.sidebar_width`), then the projected panes |

A phone idiom is always compact, on its side too.

### Leaves in the room (`project`)

- Least widths: a text column (the conversation, a page, a text file) is 320·*s*. A fixed-layout document (PDF, image, video) is 400, since it zooms instead of reflowing. Files is 240·*s*. Every pane is at least 200 tall (a 44 header plus content). One hairline separates each pair.
- **The focused leaf is always shown.** In the focused tier it's the only one. Wider, leaves are kept in this priority: the focused leaf, the conversation, then the rest in the layout's order. The projection keeps as many as fit at their least sizes in both directions.
- The shown tree is the layout minus its hidden leaves. Splits keep their ids and children keep their order. Shares stay relative to each other, raised for presentation only where a child would get less than its least width. A split left with one child gives way to it. Splits are never joined, so any shown split's id is the layout's own.
- Hidden leaves are listed in the layout's order for the pane picker. They are never closed and never sent anywhere.
- **Window resizes send nothing.** Only the person's own Split, Open, Close and divider drags send `SetLayout`. `resizeBack` maps a drag on a shown split back to the layout: hidden children keep their shares, and the shown ones share the rest in the dragged proportions. Close and Move already work by id on the full tree.

### Focus

`focused` is the leaf the person is reading. A pane they tap, open, or switch to with the Chat/Document switch or a shortcut becomes it. When a leaf is closed elsewhere, focus falls back to the conversation. **An agent's `show` never takes the focus away:** the new pane glows and waits in the picker, and in the wide tiers it appears if it fits.

For phase 1, focus lives in memory for the session: not persisted, and not device-local state. ipad.md wants durable reader state (focused leaf, page, review destination) in backend presentation records. That's a backend request to make through Mike before anything is persisted. Until then, focus survives resizes but not relaunches.

### Compact routes (`compactRoutes`)

In the compact tier the routes are the conversation, with the focused leaf pushed over it when the leaf isn't the conversation. That's what PhoneShell's `_showViewer` already does. Narrowing a window with the paper focused therefore lands on the paper with Back to the conversation. Widening it from a pushed viewer keeps that viewer focused.

### Hooks I need (none of these files are mine)

1. **Mei** (identity.dart, mobile/metrics.dart): `touchFirst`, the iPad's compact rule, and `deviceIdiom`, as in §1. Then she migrates the T/K/P sites in her files, and each other owner theirs, at their pace. Nothing breaks before they do: a wide iPad simply keeps desktop sizes until then.
2. **Mei** (main.dart, shell.dart): one seam. When `deviceIdiom == pad` and the tier isn't compact, Shell builds its non-phone row through `ipad/workbench.dart` (passing the rail, sidebar, extension bar, content and panel it already builds) instead of its own Row. A shared focus notifier above `_buildWindow` survives the switch between PhoneClaktories and Shell. PhoneShell gets a public `showViewer(agentId, paneId)` so the compact tier opens on the focused leaf, and it reports the viewer it pushes.
3. **Frontend** (panes.dart, `ResizeHandle` in controls.dart). Agreed with Priya, who builds it in phase 1:
   - panes.dart defines `PaneView {shown, hidden, resized(full, splitId, shownShares), minExtent(node, row)}` and `PaneProjection(project: PaneView Function(ViewNode full))`. `Projection` implements `PaneView`, so panes.dart never imports ipad/.
   - `PaneLayout` projects its own `full = _sent ?? layoutOf(agent)`, so a Close isn't undone on screen before the backend's echo. Close and Move work by id on `full`. A divider drag works on the shown split, with the iPad's minimum sizes, and sends `resized(full, …)`; a reset sends `resized(full, id, const [])`. Every leaf of `full` keeps its key, so a hidden pane comes back with its state. With no `PaneProjection` (the Mac), nothing changes, and compact never builds a `PaneLayout`.
   - On touch: `paneHeaderHeightOf(context)` (Metrics.row), a `ResizeHandle` hit area of 44 (`ResizeHandle.widthOf(context)` for shell.dart's positioning), and a header drag that starts with a hold (Flutter's default delay until measured on hardware) and a selection haptic.
4. **Tomás** (AppDelegate.swift, the `factory/device` channel): `capabilities`, the shape agreed with Yuki and Mei: `{schema_version, idiom, ios_app_on_mac, os_version, hardware_keyboard, pencil {hover, squeeze, double_tap}, pdf_page_overlay {available, min_os}, stroke_recognizer {available, min_os}}`, plus a change event for `hardware_keyboard`. Its fixtures are in `app/test/ipad/fixtures/bridge/`.
   Tomás is landing it himself (`Runner/Capabilities.swift`). Keyboard changes arrive on the event channel `factory/device/keyboard`, only when the value changes; the starting value comes from `capabilities`. The `pencil.*` fields say whether this OS on this kind of device can deliver the interaction (hover: pad and iOS 16.1 or later; squeeze: pad and 17.5 or later; double tap: pad). They don't say what the model supports or whether a Pencil is paired; iOS has no API for either. The idiom is "phone" until the device family includes iPad, which is phase 1's change. The Mac app doesn't answer `capabilities`: the Dart reader takes MissingPluginException as `other`.
5. **device_kind.dart's owner:** use the idiom for Hello's kind and name.

## 4. Window fixtures

`app/test/ipad/window_fixtures.dart` has 39 windows: 320 and 375 compact; each tier's edges (599/600, 839/840, 1179/1180); the full-screen sizes of the mini, Air 11 and 13, and Pro 11 and 13, upright and on their side; Split View thirds, halves and two-thirds; a short Stage Manager window; a docked keyboard; xxxLarge, AX1, AX3 and AX5 text; and the iPhone upright, on its side, and the Pro Max on its side. Each fixture names a Simulator to run it on.

Device full-screen sizes are Apple's documented point sizes. **Split View, Slide Over and Stage Manager widths are estimates marked "to measure":** Gelato is recording real view sizes and idioms from a local, uncommitted family 1,2 Dev build, and the table gets corrected from those (`measured: true`).

Measured so far (Gelato, iPad mini A17 Pro, iPadOS 27.2, Windowed Apps):
- **Full screen:** the view is 744×1133 with safe insets 32 top and 20 bottom, so the safe size is 744×1081.
- **Narrowed windows:** about 1097 tall, with insets of 32 top and 10 bottom at every width (1055 safe). The widths tracked live were 744, 706, 679, 662, 625, 554, 543, 480, 426 and 375. **375 is the narrowest a window gets.** The idiom stays pad at every width.
- **iPad Pro 13 windows:** about 1340 tall upright (insets 32/10, so 1298 safe) and 1032 on its side (insets 32/20, so 980 safe). No window is narrower than 375 here either.
- **Fixtures added:** `mini-window-679`, `-662`, `-625` (focused) and `-554`, `-375` (compact); `pro13-window-portrait-671` and `pro13-window-landscape-667` (focused) and `pro13-window-375` (compact).
- The full spike is Gelato's `docs/ipad-ink-spike.md`.

**Size classes and the 600 threshold.** UIKit's horizontal size class turns compact between 668 and 662, but the workbench stays focused down to 600. A focused window shows one pane with the toolbar, much as the phone's routes do. Its toolbar keeps the Sidebar, the switch and Panes, where the phone has its drawer and Back. So the 600–679 band gets a one-column layout that follows the HIG's compact width, with the iPad's own controls. I keep 600 because it's ipad.md §1's number, and because the size class isn't sent to the app (the composition comes from the window's size, not from UIKit). **Decided (Prosciutto):** 600 stays even once the flip is measured. It's the same line the wide iPad's popovers use (`popoverHere` is `!isPhoneLayout`), so one cut covers both. The flip's width is recorded here for reference only: on the mini, regular at 668 and wider and compact at 662 and narrower, the same going down and up (the window snaps in 2–6 pt steps, so that's as tight as the simulator gets). On the Pro 13 it's regular at 671 and compact at 666 upright, and regular at 669 and compact at 667 on its side: about 668 everywhere.

Phase 1's screenshot plan comes from the same table. Each screenshot is named after its fixture and taken only after asserting the tier and the shown leaves (§7). Window sizes are set by rotating the Simulator or by a real Stage Manager resize, never by forcing app orientation. Each one gets visual inspection before any claim that nothing overflows.

## 5. Where I depart from ipad.md

1. **The paired tier's minimums.** §1 asks for document ≥ 480 + collaboration ≥ 360 and puts the paired tier at 840. With the rail, those minimums need 897 pt, which an 840 window doesn't have (840 − rail 56 leaves 784 for both). I kept the 840 threshold and set the least widths to 400 and 320 instead, so the paired tier really holds two columns from its first width. If Mike or Iris prefer 480/360, the paired tier starts at 897 and the 840–896 windows become focused; it's one constant.
2. **Short windows.** §1 says "unusably short content" means compact but gives no number. I use 440 pt, which is about the height of an iPhone on its side, the layout the phone routes are built for.
3. **Durable focus.** Session-only until the backend has presentation records (§3, Focus).
4. **IDENTITY.md** needs an iPad section, as ipad.md says: touch pane headers, divider hit areas and 17 pt reading type in panes. The Mac's 44 pt title band doesn't apply. I'll draft it with the phase 1 work, for Mike and Hana to review, rather than change IDENTITY.md in phase 0.

## 6. Document and ink fixtures (with Yuki)

These live in `app/test/ipad/fixtures/`. Every file is JSON with `schema_version: 1`, and Swift tests read them through `#filePath`.

- `bridge/`: Yuki's. The ink platform view's creation parameters, the commands Dart sends (tool, undo/redo, load revision, export a composite), the events native sends (stroke committed with its revision and page, undo/redo availability, save needed, errors) and the capability report. `capabilities_pad.json` and `capabilities_phone.json` fake both idioms for my tests.
- `evidence/`: §3's InkEvidence and anchors. The cases: a PDF page region (unrotated CropBox, normalized top-left `{x, y, w, h}` in 0..1, the same as annotations.dart's Region), a rotated page, an image with EXIF orientation, a text/Markdown excerpt (TextQuote, position and frozen rendition), a blank sketch, an OUTDATED source hash and a three-item review batch. Blob ids and hashes are placeholders; the backend assigns the real ones.
- Matt's open decisions are parameters, not assumptions: `persistence: "online" | "journal"`, and each capability gate carries its `min_os` (PDF page overlays 16.0, stroke recognition 27.0).
- **Frozen** means both of us agree, plus Omar for anything the backend will store. After that, changes need the same three. The document host (mine) passes the ink surface: the leaf id (stable across resizes), source `{path, sha256, mime}`, the drawing revision, `drawing_policy`, `persistence` and `active` (false while the projection hides the leaf, so the canvas can go while the committed revision stays).

### What the document host takes from Gelato's spike (phase 2)

These come from `docs/ipad-ink-spike.md` and constrain `document_host.dart` and the workbench around it:

- **Moving a document between columns keeps its native view.** Keep each document leaf under a GlobalKey keyed by its leaf id, so a projection change (a tier change, Panes, the switch) moves the view rather than re-creating it, and the ink stays.
- **A width change drops a stroke in progress silently.** Rotating, resizing a window or dragging a divider mid-stroke loses the stroke. Native reports strokeInterrupted with kept: false. The host says so in words rather than pretending, and the workbench doesn't start a divider drag while a stroke is down on that document. Gelato's bridge tells it with `strokeActive {page_1based, active}`: true when PencilKit's tool starts, false when it ends, always before the matching drawingChanged or strokeInterrupted (fixture `bridge/ev_stroke_active.json`, with the phase 2 bridge). It arrives a few ms after the touch, but a touch that starts on the canvas is the canvas's, so no divider drag can begin on it.
- **Clipping doesn't clip touches.** A pane's rounded clip hides the native view's edge, but the view still takes touches there. Pane headers, dividers and popovers must sit outside the native view's bounds, or over it as Flutter widgets that take their own taps. The spike shows those do.
- **PKToolPicker takes focus from text fields,** so the tools are Claktory's own controls, and the composer keeps the keyboard.
- **The bar in a narrow pane (Prosciutto).** Every control is a whole 44pt target, 4pt apart, never cut. Below the full bar's width (about 433pt: five tools, the color, Undo, Redo, Draw with finger), pen, pencil and highlighter share one button showing the last in hand with a dot of its color (VoiceOver "Pen, Black"). A tap takes it; a tap while it's in hand opens a menu with the three tools, then that tool's colors. Eraser, lasso, Undo, Redo and Draw with finger stay as they are.
- **Words (Trish's OK).** Tools: "Pen", "Pencil", "Highlighter", "Eraser", "Lasso", "Undo", "Redo", "More", "Draw with finger"; VoiceOver "Drawing tools" and "Color, Black". The status line's words are under "Keeping drawings" below; a stroke cut short says "Your last stroke was lost when the pane changed size. Draw it again."

### The PDF pane and its review (phase 3)

A "pdf" pane on an iPad is the document host. It reads the file in ranges from one version (contract §2) into the device's render cache, then hands it to Gelato's surface. On a Mac the viewers' PDF pane shows it without ink (viewers/pdf.dart); on an iPhone it's a page of its own, only to read (the same viewer, nothing drawing; Search in More opens a find bar at its foot); elsewhere the pane says "To see this PDF, open it on a Mac, iPhone or iPad." (Trish), and the review is answered from its card (and the iPhone's review bar). When an agent asks for a review of the PDF (#275), a bar at the pane's foot shows who asks and what, then Dismiss and Approve, with the shared comment control under them as on every card (decision.dart's DecisionComment). Either answer uploads each page with ink to the agent, whole with its ink, as `<stem>-p<page>.png` (long edge ≤ 2048, ≤ 3 MiB), and names them in AnswerAsk.attachments. They reach the agent only then. A backend that takes a PDF for review also takes the attachments, so no older backend ever sees them. What's under the page (the link offer, the status, the review) takes at most 60% of the pane and scrolls past it, so the page stays at the largest text. In a window narrowed to the phone's routes, an agent's PDF page is the host too, with its own review bar instead of the phone's.

Words (Trish's OK):
- Opening: "Opening…" and "Opening… 42%".
- Problems:
  - "This file changed while it was opening. Close it and open it again."
  - "This file keeps changing while it opens. Try again once it stops changing."
  - "To draw on PDFs here, update Claktory on the computer it runs on."
  - "Drawing on PDFs needs iPadOS 16 or later."
  - "Claktory can't open this file here."
  - "Some drawings on this file can't be read."
  - "This PDF has 2,400 pages, more than the 2,000 Claktory can open here." When the count is unknown: "This PDF is too long to open here."
- A link in the PDF: "Open example.com?" or "Email june@example.com?", with "Not now" and "Open". Nothing opens by itself. While a finger draws (Draw with finger on) or with a Pencil, a tap draws, as in Apple's Markup (Prosciutto's call): Draw with finger is in the tool bar itself, lit while it's on, in place of More, so the person sees why a link doesn't open and turns it off.
- The review, answered in the pane (Matt's request; the same model and words on the Mac and iPhone, Trish's words):
  - "Theo asks you to review this", then the ask's title, the question.
  - The answer field, always open: placeholder "Your answer to Theo", VoiceOver hint "Return sends it to Theo.", the note "Theo gets it with your answer." Return (a hardware keyboard's, or the on-screen keyboard's Send) answers with what's typed; on an empty field it does nothing, so a stray Return never approves. Shift+Return starts a line.
  - Dismiss (quiet) and the main button: "Approve" while the field is empty, "Send" once something is typed, and always "Send", faint until there are words, when the agent wants a written answer. Whatever is typed goes with either, and so do the drawings.
  - Answered, the pane showing the file closes (SetLayout): an expanded pane gives the window back and the conversation has the focus again, so the person is back where they were.
  - "Your drawing on 1 page goes with your answer." / "Your drawings on 3 pages go with your answer."
  - "Sending your drawing…" / "Sending your drawings… 2 of 3"
  - "Couldn't send your answer. Try again." (the words and the pane stay); "Couldn't send your drawings, so your answer wasn't sent. Try again."

### Handwriting into the answer (phase 4)

On an iPad that reads handwriting (iPadOS 27+, as `ready` says), the review bar offers to turn what's drawn into the answer, under its field: native reads the lasso's strokes, or every drawn page, and the text goes after what the comment has, on a new line, opened to change, sent only with an answer. An answer the ink changed under (native's `stale`, or a stroke or lasso change while it read) is dropped without a word. Below 27 nothing shows. Only where there's a comment to fill: with no review waiting, a lasso selection offers nothing. Words (Trish): "Turn handwriting into your answer", "Turn selected handwriting into your answer", "Reading your handwriting…", "No handwriting to read.", "Couldn't read your handwriting. Try again."

### A PDF that changes while open

When the backend says the file changed on the machine (FileChanged, for a file in an agent's pane), the host fetches the new version into the render cache and hands it to native (reloadDocument), which shows it in place at the same page. Before that it draws out each page with ink as it is; afterwards, ink on pages native lists as outdated (changed or gone, saved or not) is set aside: no longer shown, never moved onto the new page, and its picture from the earlier version still goes with a review's answer (a page with new ink goes as it is now). Ink on pages that stayed the same stays. A version native can't show leaves the one shown, without a word, and the next change tries again; a search finds again in the new version. Words (Trish): "This PDF changed. Showing the new version." (until the person draws) and "This PDF changed. Your drawing on page 5 is from the earlier version." ("on pages 5 and 7"), which stays.

### Search and contents

The pane's header has Search and, when the PDF has its own outline, Contents (on a phone-width window, rows in its More menu); ⌘F ("Find…", in the system's menu and the ⌘-hold list) opens Search in the PDF being read. Either opens a panel at the page's start: in a pane at least 560 pt wide it's 320 pt and the page narrows beside it; narrower, it covers the page and going somewhere closes it. Search asks native once typing pauses (250 ms), drops an answer to an older query, and lists each match with its page and its line, the match in bold (a long one, such as a page PDFKit reads as one block, cut to the words around that match); native highlights them on the pages. Tapping a match or a contents entry goes there. Close, Esc, or switching to Contents ends the search and its highlights. Words (Trish): "Search", "Contents", "Search this PDF", "Searching…", "1 match" / "12 matches" / "No matches", "Over 200 matches · showing the first 200", "Page 3", "Close", and "Find…" in the menu.

### A project's PDF

A PDF opened from a project's Files on an iPad (no agent, so no review) opens in the same host, read only: drawing is off (`InkDrawingPolicy.none`), so the tool bar, Draw with finger and the save line don't show, while Search, Contents, reloading when the file changes and the header's page count all work. Its surface is keyed by path and released when its viewer closes. An iPhone keeps the phone's PDF viewer.

### Keeping drawings

What's drawn is kept on the backend as ink layers (contract §3–§4) by Gelato's `InkLayerSync` (ink_sync.dart), one per surface, made once the file is here. The host tells it what native says: `ready` on every ready (it loads what the backend keeps of the version shown, or, for a view made again, what it holds), `changed` on every drawingChanged (saved at once as the page's next revision), `reloaded` once a new version shows (ink on pages that changed is the earlier version's; the rest is carried onto the new one), `flush` when the pane is hidden, and on closing, the last save finishes before the sync and the controller go. Which pages have ink, and each page's layer for a composite, come from the sync, restored and followed ink included; the earlier version's pictures stay in the host for a review's answer. A view made again opens the version shown now, with the drawing policy as the tools have it. A project's PDF, only to read, shows what's kept and says nothing about it. A pane closed for good (even the last, leaving only the conversation) drops its surface once its host goes, so its sync stops; what wasn't saved stays in the journal.

The line under the page, the most urgent first (Trish's words, the Claktory's name as `claktoryName` gives it):
- "Page 3 was saved on another device first. Your drawing on it isn't saved yet." / "Pages 3 and 5 were saved on another device first. Your drawings on them aren't saved yet.", with **Keep both**, which puts the other device's drawing under the person's and saves both as the page's next revision, so every device shows both.
- Not saved: "Not connected to Studio. Your drawing is kept on this iPad and saves the next time this PDF is open and Studio is back." (Gelato's journal keeps a page's unsaved drawing in the app, closed or quit, until the save is confirmed; the PDF opened again while connected saves it); "Not saved. To keep drawings, update Claktory on the computer it runs on." (a backend from before ink layers); "Not saved. <the backend's words>", such as "This page's drawing is too large to keep. Erase some of it to save it."; "Couldn't save your drawing. Claktory tries again the next time you draw."
- "This PDF changed. Your drawing on page 5 is from the earlier version." ("on pages 5 and 7"), then "This PDF changed. Showing the new version." until the person draws.
- "Saving your drawing…" / "Saving your drawings…", then "Saved to Studio".

Its changelog line waits for the change that ships the iPad app (Bruno's call; Trish's words): "- Added: On iPad, draw on PDFs with Apple Pencil or your finger. Your drawings save to your Claktory as you draw, and are there on any of your iPads. Drawn while you're not connected, they're kept on the iPad until they can save. When an agent asks you to review a PDF, it gets a picture of each page you drew on."

## 7. IDENTITY.md's iPad section (draft, for Mike and Hana)

This goes into `docs/IDENTITY.md`, after Phone, once the workbench is wired in. IDENTITY describes what is, so it lands with that change, not before. Where Claktory's look and Apple's iPad guidelines pull apart, it says which way it goes and why.

> ## iPad
>
> An iPad window 600pt wide or more (narrower, or under 440pt tall, it takes the Phone's layout) is the Mac's layout made for a finger, as Apple's iPad apps are: the rail, the projects sidebar and the agent's panes, at the phone's type and touch sizes and with the phone's contrast. Pencil, keyboard, trackpad and touch all work at once. A mouse moving never shrinks anything to a Mac's sizes.
>
> - **Window:** three compositions by the window's width, without its safe area and never without the keyboard (a docked or floating keyboard moves only the field being typed in into view). From 1180pt: the rail, the sidebar pinned and the panes. From 840pt: the rail and the panes, with the sidebar opening over them. From 600pt: one pane at a time under a toolbar. Larger text moves each threshold up: by 360pt for each step past the default size for the paired layout, and by 592pt (sidebar and conversation) for the pinned sidebar. Resizing a window, rotating it or putting it in Split View or Stage Manager never changes the agent's layout. It changes only how much of it shows. In a window of its own, the system's window controls keep the top left; Claktory draws no traffic lights.
> - **Rail column:** 56pt wide, the rail under two 44pt buttons with 16px glyphs. Sidebar (Lucide panel-left) has a `selected` fill while the sidebar shows. Panes (Lucide columns-3) appears only while some panes don't fit, with their count on a pill at its top right: `text` fill, the digits in the canvas color at 10pt.
> - **Sidebar:** pinned at the person's width, at most 40% of the window, behind a hairline. Over the window it slides from the leading edge in the sidebar's color with the floating shadow, over a 20% black dim. A tap on the dim, Esc, Sidebar again or choosing an agent closes it. In the one-pane layout the rail comes with it. Its rows are the phone drawer's: 15pt names, 44pt rows with a line under the name, 36pt with one line.
> - **Toolbar (one pane at a time):** 52pt high, behind a bottom hairline. Sidebar is at its start and Panes at its end. Between them, centered, is a switch between "Conversation" and the document (its name, cut short): a track in `selected`, 8px radius, 2pt inside it; the chosen side has a canvas fill and its words in label `text`, the other `textMuted`. It's at most 420pt wide and at least 88pt a side. It shows only when the agent has a pane besides its conversation.
> - **Panes:** every pane of the agent, in the layout's order. Each is a 44pt row (growing with the text) with its glyph and title; the one being read is `selected`, and one the window doesn't show says "Not shown" in caption `textMuted`. Choosing one shows it and makes it the one being read. It's a popover, the same one the wide iPad's action sheets use (Mei's, 63ab32d9): 320pt wide, pointing at the Panes button, with nothing dimmed behind it. ⌥⌘P with every pane shown has no button to point at, so it opens in the middle of the window with no arrow. Esc or a tap outside closes it. It takes the keyboard focus as it opens, even from the composer, so Esc reaches it.
> - **Panes in a layout:** as on a Mac, with a finger's sizes. A pane's header is at least 44pt and grows with the text. The one being read has a `selected` header; a tap anywhere in a pane makes it so. The 1px hairline between panes has a 44pt edge to drag. A pane is dragged by holding its header, which lifts with a light tap; a swipe across a header still scrolls. A viewer of a fixed-layout file (a PDF, an image, a video) is at least 400pt wide; one of words (the conversation, a page, a text file) at least 320pt, growing with the text. Panes that don't fit wait in Panes.
> - **Panel:** beside the panes while they keep at least 440pt; narrower, it opens over them from the trailing edge with the floating shadow, at most 85% of the window, rather than squeezing the document.
> - **Keyboard:** the same shortcuts as a Mac where they exist (⌘[ and ⌘] back and forward). Added: ⌃⌘S shows or hides the sidebar, ⌃Tab and ⌃⇧Tab go to the next and previous pane, and ⌥⌘P opens Panes. All of them are listed in the iPad's menu and when ⌘ is held. With a hardware keyboard, Return sends a message and ⇧Return or ⌥Return starts a new line; with the on-screen keyboard, Return starts a new line.
> - **Pointer:** hover tints as on a Mac, and the pointer takes the system's own shapes. A secondary click opens what a hold opens. Nothing needs hover.

Notes for review:

- **Contrast and type (Mike's decision):** the phone's, not the Mac's: 17pt reading text and the phone's `textFaint` in panes too, because an iPad is read at arm's length. If Matt's taste differs once he holds one, it's one switch in `TypeScale`.
- **Claktory's look over Apple's (Mike's decision):** graphite surfaces and hairlines, no Liquid Glass sidebars or toolbars (principle 1); ipad.md agrees. PencilKit's native tool palette is the one place the system look shows.
- **Words (Hana's OK):** VoiceOver's "Show sidebar"/"Hide sidebar"; "Panes", "Panes, 2 not shown" and "Not shown"; "Conversation" on the switch with the file's name. The system's menus (the iPadOS menu bar, the ⌘-hold list) take title case, as the Mac's menu bar does: "Show Sidebar", "Hide Sidebar", "Next Pane", "Previous Pane", "Panes…". Everything Claktory draws itself stays in sentence case.
