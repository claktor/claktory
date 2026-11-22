# iPad document and ink fixtures

Frozen contracts for the iPad's document surfaces (Phase 0 of the iPad
plan, ipad.md in the project's Research folder). Pinned by
`test/ipad/fixtures_test.dart`; the Swift side can read the same files via
`#filePath` in its own tests, so the Xcode project needs no resources.

**Frozen** (Petra and Yuki, 2026-11-21): a change needs Petra (workbench) and Yuki (ink/document) to
agree, plus Omar for anything the backend will store (`evidence/`). Add new
files and optional fields; don't change the meaning of a field.

## bridge/: Dart ⇄ Swift

- `capabilities_*.json`: `factory/device` → `capabilities`, called once at
  start (Tomás's Capabilities.swift). Nothing in it changes with the
  window. `pencil.*` says what this OS on this kind of device can deliver,
  not whether a Pencil is paired. `keyboard_changed.json`: the
  `factory/device/keyboard` event when a hardware keyboard comes or goes.
- `create_*.json`: a surface's creation params (UiKitView
  `claktory/ink`), one per surface kind: `pdf`, `image`, `excerpt`,
  `blank`. `source` is the file on the backend; `document` is what native
  opens: a non-authoritative cached copy whose hash native checks against
  `source.sha256` (else `error` `document_hash_mismatch`), the frozen
  excerpt, or a blank canvas's size. `leaf_id` is the pane leaf the
  surface belongs to; `active: false` means the leaf is hidden by the
  column projection (native may drop live canvases, never committed ink).
- `cmd_*.json`: Dart → native on `claktory/ink/<view_id>`, with the result.
  Native holds its events until Dart sends `attach` (`cmd_attach.json`),
  so none is lost before Dart listens. `layerSaved` tells native the
  layer and revision a `drawingChanged` was saved as, the next change's
  base; `setDrawingPolicy` switches Pencil-only and any-input live, ink
  untouched. The workbench draws its own tool controls, so
  `showToolPicker` answers `visible: false` (PKToolPicker takes focus from
  Flutter's text fields).
- `ev_*.json`: native → Dart on the same channel. Pen motion never crosses
  the bridge: native sends `drawingChanged` after a finished stroke (or an
  erase, undo, redo, lasso move), coalesced; Dart saves it as a new
  revision of its ink layer (`ink_layer_id`, null on a page with no ink
  yet: Dart creates the layer). `bounds` covers the whole layer,
  `new_bounds` the strokes added since the previous event, the default
  region of a new review item. `pageChanged` and `selectionChanged` (PDF
  text selection, for an item's extracted quote) keep the pane's header
  and review column current. `strokeActive` says a tool went down on a
  page or lifted; when a resize, rotation, undo or hiding cuts a stroke
  short, `strokeActive` false comes first, then `strokeInterrupted`
  (PencilKit drops such a stroke, so native says `kept: false`;
  `ev_stroke_interrupted.json` shows the field's other value).
- `create_*.json` carry `layers`: the ink layer and revision of each page
  (or the one of an image) to load; `loadDrawing` brings their bytes.
- PDFs (phase 3, all optional): `ready` may carry the PDF's `outline`
  (`ev_ready_outline.json`, at most 500 entries, depth first). `search`
  answers its own `query` with matches (page, region, the matched line),
  highlighted natively; an empty query clears them, and an answer
  overtaken by a newer search says `superseded: true`. `goToPage` takes an
  optional `region` to scroll into view (`cmd_go_to_page_region.json`).
  `linkTapped` reports a tap on a link out of the PDF, which never opens by
  itself. `document_too_large` refuses a PDF of more than 2000 pages.
  `exportComposite` with no `region` exports the whole page.
  `drawing_policy` `none` (`cmd_set_drawing_policy_none.json`, also in
  creation params) turns drawing off: neither Pencil nor finger draws, and
  the document scrolls, zooms, selects text and follows links as when only
  read; a stroke in progress ends as `strokeInterrupted` `drawing_off`.
- Handwriting and interruptions (phase 4/6, all optional): `ready` may
  carry `handwriting` (`ev_ready_handwriting*.json`): whether this iPad
  can read handwriting (iPadOS 27+, on the device; no cloud fallback).
  `recognizeText` reads the strokes of a lasso `selection`, a `region` or
  `all` of a page and answers a candidate text, never applied or sent by
  native: the host offers it, editable. The answer names the drawing it
  read (`base_revision`, `local_seq` of the drawingChanged that carried
  it, null if unchanged since loaded, and `drawing_sha256`); a
  drawing that changed meanwhile answers `stale: true` with no text; below
  27 it answers `available: false`, `os_too_old`. `strokeSelectionChanged`
  reports the lasso selection (27+). `strokeInterrupted` `background`:
  the app left the screen (or it locked) mid-stroke; native cancels the
  stroke and flushes the last change.
- A PDF changing while open (all optional): `reloadDocument` hands native
  a new version (`source` and `document`, as in creation params). Native
  checks its hash (else PlatformException `document_hash_mismatch` or
  `document_too_large`, and the version shown stays), ends a stroke in
  progress (`strokeInterrupted` `reload`), sends any pending change for
  the old version, then shows the new one in place, at the same page and
  zoom where that page still exists. The answer lists the pages
  `changed` (same number, different text or rendering), `added` and
  `removed`, compared page by page, never remapped. Ink on unchanged
  pages stays (`kept_layers`); ink on changed or removed pages is no
  longer shown and is listed in `outdated_layers`, for the host to show
  as OUTDATED, never moved onto the new page.
- Pencil gestures (all optional): double-tap and squeeze do what the
  person chose in Settings, and only that: switching to the eraser or the
  previous tool changes the tool natively and says so (`toolChanged`,
  `source` `pencil_double_tap` or `pencil_squeeze`); showing the colour
  palette, ink attributes or the contextual palette is Dart's to do
  (`pencilAction`, with `at`, where the Pencil hovered, 0..1 of the
  surface, or null). Never send, accept or delete. Hover previews the
  tool's mark natively; nothing crosses the bridge.
- Ink keeps its exact colours whatever the appearance: surfaces are always
  drawn light, so PencilKit never turns black ink white; `ink.appearance`
  is read but never makes ink adapt.

Drawings travel as `PKDrawing.dataRepresentation()` in base64
(`bytes_base64`) with their SHA-256 and PencilKit content version. Large
images (composites) travel as temporary files, never through the channel.
`<…>` values are placeholders; hashes are made up but well formed.

## evidence/: what an annotation keeps

ipad.md §3's `InkEvidence`, split in two, because one PencilKit canvas per
page holds one drawing:

- **Ink layers** (`layer_*.json`): the durable drawing. One per PDF page,
  per image, per excerpt, per sketch, with its own `ink_layer_id` and
  revisions (`revision`, `parent_revision`; saving is compare-and-set on
  the layer). It carries the source, the coordinate space, the PencilKit
  drawing and a PNG preview.
- **Review items** (`item_*.json`): what a thread points at. A region (or
  quote) on a layer, at a frozen `layer_revision`, with its context
  composite, question and optional recognized text. Several items can share
  one layer revision (`item_pdf_region` and `item_pdf_second`, page 3).

Cases: a plain page (3), a page with /Rotate 90 (5), a page whose CropBox
sits inside its MediaBox (7), an image with EXIF orientation 6, a Markdown
excerpt, a blank sketch, an OUTDATED thread (`outdated`), a DETACHED one
(`detached`) and a review batch of three items with a `client_batch_id`, so
a retried Send can't deliver twice. Blob ids are the backend's to assign;
every distinct blob has its own hash.

- `region` is `{x, y, w, h}` in 0..1 from the top left, as
  `annotations.dart`'s `Region`. For a PDF it is relative to the page's
  **unrotated CropBox** (`coordinate_space.box: "crop"`); `rotation` is the
  page's /Rotate. The anchor's selector is an RFC 8118 `page=N` fragment
  refined by a `ClaktoryPageRegion` (version 1) carrying the same numbers;
  an image's is the existing `xywh=percent:` fragment.
- Transforms are affine `[a, b, c, d, tx, ty]`: x' = a·x + c·y + tx,
  y' = b·x + d·y + ty. `context_composite.crop_to_document_transform`
  maps composite pixels (y down) to PDF user space (unrotated, y up) or to
  oriented image pixels; `drawing.drawing_to_document` maps the drawing's
  own space to the same. A page overlay's drawing is in points from the
  CropBox's top left, y down (`[1, 0, 0, -1, cx, cy + ch]`), whatever the
  page's /Rotate: verified through PDFKit's own conversions in the native
  spike (docs/ipad-ink-spike.md).
- Composites stay within 2048 px on the long edge and 3 MiB, so
  `read_image` passes them through unchanged.

## Parameters, not assumptions

Matt's pending decisions (ipad.md §8) are values here, never shapes:

- **Ink while offline:** `persistence.mode` is `online` (the
  recommendation: durable only once the backend acknowledges) or `journal`
  (an encrypted on-device journal, a new local-state exception only Matt can
  approve). The messages are the same either way.
- **Minimum iOS:** each gate in `capabilities` carries `available` and its
  `min_os` (`pdf_page_overlay` 16.0, `stroke_recognizer` 27.0);
  `capabilities_pad_ios15.json` is an iPad below both. Raising the app's
  minimum changes values, not shapes. Below the gate native answers
  `error` `os_too_old`.
- **Ink over reflowing text:** `excerpt` surfaces draw on a frozen
  rendition of a quoted passage; nothing here assumes ink over live
  reflowing text.
