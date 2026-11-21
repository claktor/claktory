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
- `ev_*.json`: native → Dart on the same channel. Pen motion never crosses
  the bridge: native sends `drawingChanged` after a finished stroke (or an
  erase, undo, redo, lasso move), coalesced; Dart saves it as a new
  revision of its ink layer (`ink_layer_id`, null on a page with no ink
  yet: Dart creates the layer). `bounds` covers the whole layer,
  `new_bounds` the strokes added since the previous event, the default
  region of a new review item. `pageChanged` and `selectionChanged` (PDF
  text selection, for an item's extracted quote) keep the pane's header
  and review column current.
- `create_*.json` carry `layers`: the ink layer and revision of each page
  (or the one of an image) to load; `loadDrawing` brings their bytes.

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
  CropBox's top left, y down (`[1, 0, 0, -1, cx, cy + ch]`): **pending
  verification in the native spike**; correcting it once PDFKit says
  otherwise is a correction, not a schema change.
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
