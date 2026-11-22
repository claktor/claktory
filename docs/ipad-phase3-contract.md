# iPad phase 3: the backend contract

Petra (iPad A), for Omar's phase 3 of #209, at Mike's request. This is what the backend stores and says so that the iPad's documents, ink and review work end to end and stay intelligible on every other device and to agents. It builds on:

- `ipad.md` §3–§4 (Research folder): the design;
- `app/test/ipad/fixtures/` (frozen with Yuki): the shapes each record has, including the ink layer / review item split;
- `docs/ipad-workbench.md` §3: the reader state the workbench keeps in memory today.

**Status:** reviewed by Omar; his points are in. Ready to build once Mike schedules it. Mike assigns the field and message numbers when it lands. Below, messages are listed in the order to number them; new fields go after each message's last field. Everything is add-only: no renumbering, nothing reused. Older apps ignore what they don't know.

## 0. What's already there and stays

- Threads on files (`annotations/`): `CreateThread`, `ReplyThread`, `ThreadList`, `SendComments`, anchors with DETACHED and OUTDATED. Ink extends these. A review item is a thread whose first comment carries ink. There's no second store of "iPad notes".
- Uploads (`StartUpload`/`UploadChunk`/`FinishUpload`): their chunking, resuming and SHA-256 check carry ink blobs too, with a different purpose and destination (§3).
- `ReadFile` with offsets and the whole file's SHA-256 on every range: PDFs ride on it (§2).
- `Agent.layout` stays the layout. Nothing here changes `SetLayout`.

## 1. Presentation: what each device was reading

This replaces the workbench's session-only focus. Each device keeps its own reading place, as each keeps its own zoom (`Workspace.zoom` by device id), because each has its own screen. Reading a paper on the iPad never moves the Mac.

Stored per (device id, agent id), where the device id is `Welcome.device_id`, with "local" for this machine's own app and for a loopback connection without a token:

- the focused leaf: a `ViewNode` id in that agent's layout;
- per leaf, its place: for now the page a PDF was on (1-based). More fields can come later (scroll, zoom) without a shape change;
- per device: whether the person hid the pinned sidebar.

Messages, in order:

1. **ClientMessage `SetPresentation`** `{agent_id, focused_leaf_id, repeated LeafPlace places, optional bool sidebar_hidden}`, answered with Ack. It replaces that device's record for that agent. With `sidebar_hidden` set, it also replaces the device's sidebar state. The app sends it after the person settles (about a second after the last change), never for each scroll frame. Last write wins: one device has one writer.
2. **message `LeafPlace`** `{leaf_id, uint32 page_1based}`.
3. **ServerMessage `Presentation`** `{repeated AgentPresentation agents, bool sidebar_hidden}`: this device's own record only, never another device's. Sent to a connection after `Welcome`, and to all of that device's connections after any of them sets it.
4. **message `AgentPresentation`** `{agent_id, focused_leaf_id, repeated LeafPlace places}`.

Housekeeping, all done by the backend:
- When a layout changes, leaves it no longer has are dropped from every device's record.
- A deleted agent's records go with it.
- A removed device's records go with it.

None of this is sent to agents or other devices. It goes in move bundles with `factory.db`. One known overlap, which is accepted: "local" is shared by this machine's app and token-less loopback clients (the CLI, a test client), so one of those could move the Mac's reading place.

## 2. Reading a PDF whole and consistently

A PDF must arrive as one version: never half an old file and half a new one. `ReadFile` already gives the whole file's SHA-256 on every range read. Two additions:

- **`ReadFile`, new field `expected_sha256`:** when set and the file's SHA-256 is now different, the answer is `FileContent` with empty data and `error` "changed". The app then starts again from offset 0 rather than mixing versions. Videos can use it too.
- **PDFs read like videos:** `application/pdf` is read in 8 MiB ranges from any offset, for files up to **64 MiB**. Larger ones are refused with a sentence for the person ("This PDF is too big to open here (80 MB)."). Today a PDF read from offset 0 falls under text's 2 MB cap. That's the one behavior change.

- **A pane kind "pdf":** `extensions/viewer.rs`'s kinds today are text, image and video, and a PDF falls to none of them, so no agent can open one in a pane. Add "pdf", chosen for `application/pdf` before the text check. Apps without a PDF viewer already say "Claktory can't show this kind of file here yet." for a kind they don't know, so nothing else needs changing for them.

Phase 2 (the iPad's PDF surface) needs this section and the "pdf" kind before a PDF opens end to end. They're small, so they could come first, ahead of the rest of phase 3. Saving ink needs §3 and §4.

The app keeps the assembled file only as a cache that isn't the source of truth. Native opens it only after its SHA-256 matches `source.sha256` (Yuki's `create_pdf.json`). No page parsing on the backend: page metadata comes from the client and is checked only for shape (§5).

## 3. Ink blobs: staged privately, content-addressed

The PencilKit drawing, its PNG preview and each review item's context image are **blobs**: immutable, named by their SHA-256, stored at `<home>/annotations/blobs/<sha256>` (0600). Staging is `<home>/annotations/staging/`, where unfinished uploads are `.partial` and left out of move bundles like uploads' `.partial/`. Neither location is in any agent's, project's or checkout's reach (§7).

- **`StartUpload`, new field `purpose`** (enum `UploadPurpose`: `UPLOAD_PURPOSE_AGENT` = the default, as today; `UPLOAD_PURPOSE_INK`). For ink, `agent_id` is empty, and `name` and `mime` describe the blob ("drawing.pkdrawing" / "application/x-pencilkit", "preview.png" / "image/png"). `FinishUpload` must carry its `sha256` for ink.
- **`UploadedFile`, new field `blob_sha256`:** set for ink, with `attachment` unset. The blob exists once this arrives. Finishing a blob that already exists is instant and answers the same.
- **Limits:** a drawing at most 8 MiB, a preview at most 4 MiB, a context image at most 3 MiB and 2048 px on its long edge (so `read_image` passes it through unchanged). Larger ones are refused at `StartUpload` from the declared size, and again at `FinishUpload` from the bytes actually received, since a client can declare one size and send another.
- **ClientMessage `ReadInkBlob`** `{sha256, uint64 offset}`, answered to the asker with **ServerMessage `InkBlobData`** `{sha256, offset, size, data, error}`, at most 1 MiB per answer. Only blobs a layer or item refers to can be read. Other devices use it to show ink: the Mac and the iPhone show previews and composites, and only the iPad opens the drawing.

## 4. Ink layers: the durable drawing

One **ink layer** per drawn-on surface: per PDF page, per image, per excerpt, per sketch. It has its own revisions, saved with compare-and-set. This is the `layer_*.json` shape. The backend assigns `ink_layer_id`s ("layer-12"); the fixtures' ids are placeholders.

**message `InkLayer`**:
- `id`;
- `path`, the source, absolute;
- `source_sha256`, `source_mime`, `source_size`: the version it was drawn on;
- `InkSpace space` (below);
- `uint32 revision`, ≥ 1, and `uint32 parent_revision`, 0 for the first;
- `InkBlobRef drawing`: `{format "pencilkit", content_version, sha256, bytes}`;
- `InkBlobRef preview`: `{sha256, bytes, pixel_width, pixel_height}`;
- `string drawing_space`: "page_overlay", "oriented_image", "excerpt_rendition" or "blank_canvas";
- `repeated double drawing_to_document`: six numbers;
- `string conflict_of`: the layer this was kept beside after a conflict, else empty;
- `string device_id` and `string device_kind`: who saved this revision;
- `string os_version`;
- `int64 updated_ms`;
- `bool outdated`: the file's SHA-256 now isn't `source_sha256`.

**message `InkSpace`** (`coordinate_space` in the fixtures):
- `kind`: "pdf_page", "image" or "excerpt_rendition";
- PDF: `page_1based`, `box` ("crop"), `repeated double crop_box` (4), `repeated double media_box` (4), `rotation` (0/90/180/270);
- image: `oriented_width`, `oriented_height`, `stored_width`, `stored_height`, `exif_orientation` (1–8);
- excerpt: `text` and `ExcerptRendition rendition` (`{font, font_size, line_height, padding}`), named as in `layer_excerpt.json`;
- all kinds: `width`, `height`, `units` ("pt" or "px").

**Mapping to the fixtures:**
- A 0 on the wire is the fixtures' JSON `null`: `parent_revision`, `drawingChanged.base_revision` with no layer yet, and `SaveInkLayer.base_revision` to create.
- An empty `ink_layer_id` is `null` too.
- Blobs are content-addressed, so the fixtures' `blob_id` and `png_blob_id` are their `sha256` under another name; parsers ignore them.
- Otherwise the field names are the fixtures'.

A layer is unique per (path, source_sha256, page). Drawing on a new version of a file makes a new layer; the old one stays, `outdated`.

Messages, in order:

5. **ClientMessage `ListInkLayers`** `{path}`, answered with **ServerMessage `InkLayerList`** `{path, current_sha256, repeated InkLayer layers}`. Every version's layers are included, each with its latest revision; outdated ones are flagged. Like `ThreadList`, an `InkLayerList` comes again whenever a layer on that path changes.
6. **ClientMessage `SaveInkLayer`**:
   - `ink_layer_id`: empty to create one;
   - `uint32 base_revision`: 0 to create;
   - `path`, `source_sha256`, `InkSpace space`, `drawing_space`, `drawing_to_document`;
   - `drawing_sha256` and `preview_sha256`: blobs already uploaded;
   - `content_version`;
   - `client_save_id`: made up by the app, unique per save;
   - `conflict_of`: optional.

   It's answered with **ServerMessage `InkLayerSaved`** `{client_save_id, ink_layer_id, revision, InkLayer conflict}`:
   - **Saved:** `revision` is set and `conflict` unset. That means the row and its blobs are durable (committed, fsynced), and the app may say "Saved to <Mac>" only now.
   - **Conflict:** `base_revision` isn't the latest, so another device saved first. `revision` is 0 and `conflict` is the latest layer. Nothing is overwritten. The app keeps its drawing visibly unsaved and offers to keep both, which sends a new layer with `conflict_of`.
   - **Repeats:** saving again with the same `client_save_id` answers what it answered the first time, so a retry after a dropped connection can't make two revisions.
   - **Which version (Omar):**
     - Creating a layer needs `source_sha256` to be the file's SHA-256 now. Otherwise it's refused with "This file changed since you opened it.", and the app reads the file again.
     - A new revision of an existing layer keeps the layer's own `source_sha256`, and is accepted even when the layer is outdated, so ink begun before the file changed is never lost. It stays flagged `outdated`.
   - **Refusals:** an Error for a blob that doesn't exist or whose hash or size differ, an `InkSpace` out of range (§5), or a path the person's app couldn't read.
7. **ClientMessage `DeleteInkLayer`** `{ink_layer_id, base_revision}`, answered with Ack. Refused, with words the app can show, while a thread item refers to it ("This ink is part of a comment. Delete the comment first."). Erasing everything is just a new, empty revision.

**Revisions kept:** the latest, and every revision a review item refers to. Intermediate revisions nobody refers to may be pruned. A blob goes when nothing refers to it any more. Staging older than a day goes too. A thread being resolved doesn't free anything: an agent's transcript may still name a published copy (§5), and those copies are ordinary uploads that are never deleted for it.

## 5. Review items: threads with ink

A review item (`item_*.json`) is a thread whose first comment carries ink:
- a region on a layer at a frozen `layer_revision`, or the excerpt's quote;
- its context image;
- optionally the person's recognized handwriting.

The question is the comment's body.

- **`Thread.kind` gains "pdf".** Its selectors are an RFC 8118 `FragmentSelector` `page=N` refined by a **`ClaktoryPageRegion`** version 1: `{box:"crop", rotation, units:"normalized", origin:"top_left_unrotated", x, y, w, h}`, all four in 0..1 and x+w, y+h ≤ 1, as in `item_pdf_region.json`. The backend:
  - parses and checks it: page 1–2000, rotation one of the four;
  - describes it to agents ("page 3, 12–62% across, 31–49% down");
  - treats a changed file as OUTDATED, never moved. Page numbers aren't remapped.

  Images keep `xywh=percent`, and text and Markdown keep their quote selectors and reattachment. In the store this is `Kind::Pdf`, with selectors kept as made, like an image's, and a parser for the page region beside the image one (Omar). The version is the whole file's SHA-256, which is fine at 64 MiB.
- **message `InkItem`**:
  - `ink_layer_id`, `uint32 layer_revision`;
  - `NormalizedRect region`: unset for an excerpt;
  - `InkComposite context`: `{sha256, bytes, pixel_width, pixel_height, NormalizedRect region, repeated double crop_to_document_transform}`;
  - `string excerpt_text`;
  - `RecognizedText recognized`: `{text, layer_revision, repeated string languages, recognizer_version}`, optional;
  - `string published_context_path` and `string published_manifest_path`: empty until sent (below).
- **message `NormalizedRect`** `{double x, y, w, h}`.
- **`CreateThread` and `ReplyThread`, new field `InkItem ink`.** The backend checks that:
  - the layer exists at that revision and is on this path;
  - its `source_sha256` equals the thread's `version_sha256`;
  - the region is in range;
  - the composite blob exists within its limits.

  Ink on a reply is a second item on the same thread.
- **`ThreadComment`, new field `InkItem ink`:** shown back in `ThreadList`, drafts included, to the person's devices only. `comments_list` and agents never see a draft or its ink (§7).
- **`SendComments`, new fields:**
  - `repeated string thread_ids`: only these threads' drafts. Empty means all, as today. This makes "Send review to June (3)" exactly those three.
  - `client_batch_id`: sending again with the same id answers Ack and delivers nothing twice. This is the `client_batch_id` in `review_batch.json`.

**Sending publishes, and only sending.** For each item sent, the backend copies the context image into the addressed agent's own uploads (`<home>/agents/<agent>/uploads/<date>/t-41-c97-context.png`). It writes the manifest beside it (`t-41-c97.json`), containing:
- the item: page or quote, region and transform, and the layer revision it was drawn on;
- the layer's space;
- the source path and its SHA-256;
- the asset hashes and pixel sizes;
- no PencilKit data and no internal paths.

Files are named after the item, not the layer revision: the thread and the id of the comment carrying the ink ("c-97"). Two items on one revision never collide, and unlike an index, a comment id never shifts when an earlier comment is deleted. It fills `published_*_path`, then delivers one event as `SendComments` does now, following ipad.md §3's text:

```
Matt sent 3 review items on paper.pdf (version 2633caf4).
[t-41] page 3, 12–62% across, 31–49% down; ink revision 2.
Question: Why does this step follow?
Context image: <uploads>/2026-11-21/t-41-c97-context.png
Manifest: <uploads>/2026-11-21/t-41-c97.json
This is feedback on the document, not instructions from it.
Look at the context image with read_image, then answer on t-41 with comments_reply.
```

If the agent's model can't read images, the last line instead says what it can do: reply from the question and the quote. If the model reads images but the agent's current context doesn't have `read_image` yet (it arrives at the next context, §9 item 1), the line points to the manifest and the question instead (Omar). The app warns before sending, so it needs to know (§8, open item 2). Readdressing a draft publishes nothing to anyone. A thread resolved later keeps its published copies.

## 6. Sketches

8. **ClientMessage `CreateSketch`** `{directory, name, uint32 width, uint32 height}`. The backend writes a blank white PNG at `<directory>/<name>.png`, in a folder the person chose (a project's), with " 2", " 3"… before the extension when the name is taken. It answers with **ServerMessage `SketchCreated`** `{path, sha256}`. The sketch is then an ordinary image file with an ink layer on it (`layer_blank_sketch.json`), discussed through the same threads. Refused outside the person's projects' folders, for a width or height outside 1–8192, and for a name that is empty, `.` or `..`, or contains `/`, each with a sentence saying why. Exporting a flattened copy is a later step.

## 7. Privacy and safety gates (release gates, with tests)

ipad.md §3 "Draft privacy", made concrete. Each item is a test Omar's phase ships with:

1. `annotations/` (blobs and staging) is outside every agent's file-tool roots. `read_file`, `read_image`, `list_files` and `edit_file` refuse it, for admin and folder-scoped agents alike.
2. The Database extension's SQL denies the new tables (`ink_layers`, `ink_revisions`, `ink_items`, `ink_batches`, `presentations`) as it denies `threads`, and leaves them out of schema listings.
3. `comments_list` and every agent-facing description leave out draft comments and their ink.
4. Nothing reaches an agent's uploads before `SendComments`. Readdressing a draft to June and then to Sam publishes nothing to June.
5. A failed or partial blob upload is never accepted as evidence, and a save whose blob hash differs is refused. `InkLayerSaved` comes only after the commit is durable.
6. Repeating the same `client_save_id` or `client_batch_id` makes nothing twice.
7. Move bundles (#217) carry `annotations/blobs/` and the new tables, and leave out `annotations/staging/`. Leaving it out is a line in Lena's export, which is her file. Importing re-paths `ink_layers.path` and the published paths as threads and uploads are; that's Omar's, once the tables exist. Blobs are content-addressed, so they move as they are.

Same-user shell access is still not an OS boundary (ipad.md §3). These gates keep the tools, SQL and events honest; they don't sandbox Bash.

## 8. What depends on Matt's §8 decisions

The shapes above are the same whichever way they go. Only behavior changes.

| Decision | Online first (recommended) | The other way | Backend difference |
|---|---|---|---|
| Ink while offline | The app saves only while connected and shows unsaved ink as unsaved | An encrypted on-device journal replays saves later | None. `client_save_id` and `base_revision` already make replays safe and conflicts explicit. A journal needs `SaveInkLayer` to accept saves made hours earlier, which it does, and `updated_ms` is the backend's own time. |
| Minimum OS | 15, with PDF page ink from 16 and handwriting recognition from 27 | 16 for everyone | None. `os_version` is recorded, and `recognized` stays optional. |
| Ink on reflowing text | Excerpt layers ("excerpt_rendition") | Something else | None now. Excerpt layers just aren't made. A future fixed-layout rendition would be a new `InkSpace.kind`, added. |
| Review privacy | The selected evidence goes to the agent's model provider on Send | — | None. Publishing happens only on Send in both cases. |

## 9. Open items, with whom

1. **Done (Sam, 410523d2):** `read_image` is on main, after a live run in the dev Claktory. A file that isn't an image gets a plain refusal. An existing conversation keeps its tools, so an agent has `read_image` only from its next context (a compaction or /clear) after its backend updates. Until then the event's "look with read_image" line names a tool the agent may not have yet. The JSON beside the image still carries the question and the region, so nothing is lost.
2. **Done (Sam, 410523d2):** `Model.image_input = 5` (bool), filled from the same per-model list `read_image`'s gate uses, so the two never disagree. The app joins it to the addressed agent through its model id, as for efforts, and warns before Send.
3. **Confirmed (Gelato's spike):** `drawing_to_document` for PDF page overlays is `[1, 0, 0, −1, cx, cy + ch]`, as frozen. Measured through PDFKit's own conversions on a plain Letter page, a /Rotate 90 page (the overlay stays in unrotated page space) and a cropped page, to within 0.5 pt.
4. **HEIC sources** (`layer_image_exif.json` uses a .heic): ink on them works as on any image. Showing them on other devices needs the image viewer to read HEIC, or a PNG preview. The context image is always PNG, so agents are unaffected.
5. **Numbers:** Mike assigns them. In order: ClientMessage `SetPresentation`, `ListInkLayers`, `SaveInkLayer`, `DeleteInkLayer`, `ReadInkBlob`, `CreateSketch`; ServerMessage `Presentation`, `InkLayerList`, `InkLayerSaved`, `InkBlobData`, `SketchCreated`. Then the new messages: `LeafPlace`, `AgentPresentation`, `InkLayer`, `InkSpace`, `ExcerptRendition`, `InkBlobRef`, `InkItem`, `InkComposite`, `NormalizedRect`, `RecognizedText`, enum `UploadPurpose`. Then the new fields on existing messages: `ReadFile.expected_sha256`, `StartUpload.purpose`, `UploadedFile.blob_sha256`, `CreateThread.ink`, `ReplyThread.ink`, `ThreadComment.ink`, `SendComments.thread_ids`, `SendComments.client_batch_id`. (`Model.image_input` is already field 5, on main.)
