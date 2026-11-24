// Keeping what's drawn (docs/ipad-phase3-contract.md §3–§4): a page's
// drawing and its ink-only preview go to the backend as ink blobs, named by
// their SHA-256 and kept where no agent's tools reach, then saved as the
// next revision of the page's ink layer. Nothing is kept on the device:
// the backend is the truth, the surface only shows it.
//
// What's here:
// - InkSaveState: what the document's status line says, one value for the
//   whole surface (Sorbet's host shows it).
// - InkBlobs: uploading a blob (UPLOAD_PURPOSE_INK, 1 MiB at a time,
//   picking up where an upload stopped) and reading one back (ReadInkBlob).
// - InkLayerUploader.prepare: a page's drawing exported by native
//   (exportLayer), both blobs uploaded and the temporary files gone, and
//   what SaveInkLayer needs.
// - InkLayerSync: one surface's drawings kept as ink layers: saved on every
//   change (compare-and-set, a conflict never overwritten), restored when
//   the surface opens, following other devices' saves, carried onto a new
//   version of the file where its pages didn't change.
import 'dart:async';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:crypto/crypto.dart' as crypto;
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart' show PlatformException;

import '../backend.dart';
import '../protocol/factory/v1/factory.pb.dart' hide InkComposite;
import 'ink_bridge.dart';

// MARK: What the status line says

/// Whether what's drawn on a surface is kept: one value for the whole
/// surface, the most urgent first (conflict, unsaved, outdated, saving,
/// saved, nothing), with the pages it concerns (null in [pages] for a
/// surface with one drawing).
sealed class InkSaveState {
  const InkSaveState();
}

/// Nothing drawn, nothing to say.
class InkNothingToSave extends InkSaveState {
  const InkNothingToSave();
}

class InkSaveInProgress extends InkSaveState {
  const InkSaveInProgress(this.pages);
  final List<int?> pages;
}

/// Everything drawn is kept on [machine] (the connection's name).
class InkSaved extends InkSaveState {
  const InkSaved(this.machine);
  final String machine;
}

/// Why what's drawn isn't kept.
sealed class InkUnsavedReason {
  const InkUnsavedReason();
}

/// Not connected: saved once connected again, while the pane is open.
class InkOffline extends InkUnsavedReason {
  const InkOffline();
}

/// This Claktory can't keep drawings yet (a backend before ink layers).
class InkUnsupported extends InkUnsavedReason {
  const InkUnsupported();
}

/// The backend refused it, in its own words ("This file changed since you
/// opened it.").
class InkRefused extends InkUnsavedReason {
  const InkRefused(this.message);
  final String message;
}

/// Anything else: tried again on the next change.
class InkFailed extends InkUnsavedReason {
  const InkFailed();
}

class InkUnsaved extends InkSaveState {
  const InkUnsaved(this.pages, this.reason);
  final List<int?> pages;
  final InkUnsavedReason reason;
}

/// Another device saved these pages first: the person's ink stays on
/// screen, unsaved, until they keep both.
class InkConflict extends InkSaveState {
  const InkConflict(this.pages);
  final List<int?> pages;
}

/// The PDF changed: the ink on these pages belongs to the earlier version,
/// shown, never moved, and saved onto its own version.
class InkOutdated extends InkSaveState {
  const InkOutdated(this.pages);
  final List<int?> pages;
}

// MARK: Blobs

/// The backend's limits for ink blobs (contract §3).
abstract final class InkBlobLimits {
  static const drawing = 8 * 1024 * 1024;
  static const preview = 4 * 1024 * 1024;

  /// One UploadChunk, one InkBlobData.
  static const chunk = 1024 * 1024;
}

/// A blob nothing refers to yet (ReadInkBlob's "unknown"), or one the
/// backend no longer has.
class InkBlobUnknown implements Exception {
  const InkBlobUnknown(this.sha256);
  final String sha256;

  @override
  String toString() => 'InkBlobUnknown($sha256)';
}

/// A blob came or went wrong: the backend's answers don't add up (never
/// shown to the person; the save says it failed, and is tried again).
class InkTransferFailed implements Exception {
  const InkTransferFailed(this.detail);
  final String detail;

  @override
  String toString() => 'InkTransferFailed($detail)';
}

/// A blob larger than the backend takes for its kind.
class InkBlobTooLarge implements Exception {
  const InkBlobTooLarge(this.bytes, this.limit);
  final int bytes;
  final int limit;

  @override
  String toString() => 'InkBlobTooLarge($bytes > $limit)';
}

/// Ink blobs on the backend: uploaded privately, named by their SHA-256,
/// read back by it.
class InkBlobs {
  InkBlobs(this.backend);

  final Backend backend;
  static final _random = Random.secure();

  /// Uploads [file] as an ink blob and answers its SHA-256, once the
  /// backend has stored it. A blob it already has answers at once.
  Future<String> upload(
    File file, {
    required String mime,
    required String name,
  }) async {
    final bytes = await file.readAsBytes();
    return uploadBytes(bytes, mime: mime, name: name);
  }

  Future<String> uploadBytes(
    Uint8List bytes, {
    required String mime,
    required String name,
  }) async {
    final limit = mime == 'image/png'
        ? InkBlobLimits.preview
        : InkBlobLimits.drawing;
    if (bytes.length > limit) throw InkBlobTooLarge(bytes.length, limit);
    final sha = crypto.sha256.convert(bytes).toString();
    final id = 'ink-${sha.substring(0, 16)}-${_suffix()}';
    var offset = await backend.startInkUpload(
      id,
      name: name,
      size: bytes.length,
      mime: mime,
    );
    try {
      while (offset < bytes.length) {
        final end = min(offset + InkBlobLimits.chunk, bytes.length);
        final received = await backend.uploadChunk(
          id,
          offset,
          Uint8List.sublistView(bytes, offset, end),
        );
        // The backend says where it is: never backwards, never past.
        if (received <= offset || received > bytes.length) {
          throw InkTransferFailed('took $received of ${end - offset} bytes');
        }
        offset = received;
      }
      final stored = await backend.finishInkUpload(id, sha256: sha);
      if (stored != sha) {
        throw const InkTransferFailed('kept another blob than sent');
      }
      return sha;
    } catch (_) {
      unawaited(backend.cancelUpload(id).catchError((Object _) {}));
      rethrow;
    }
  }

  /// The blob named [sha256], read a part at a time and checked against
  /// its name. Throws [InkBlobUnknown] when nothing refers to it.
  Future<Uint8List> read(String sha256) async {
    final out = BytesBuilder(copy: false);
    var size = -1;
    while (size < 0 || out.length < size) {
      final part = await backend.readInkBlob(sha256, offset: out.length);
      if (part.error == 'unknown') throw InkBlobUnknown(sha256);
      if (part.error.isNotEmpty) throw InkTransferFailed(part.error);
      if (part.offset.toInt() != out.length) {
        throw const InkTransferFailed('a part out of order');
      }
      size = part.size.toInt();
      if (part.data.isEmpty && out.length < size) {
        throw const InkTransferFailed('an empty part before the end');
      }
      out.add(part.data);
      if (out.length > size) {
        throw const InkTransferFailed('more than the blob');
      }
    }
    final bytes = out.takeBytes();
    if (crypto.sha256.convert(bytes).toString() != sha256) {
      throw const InkTransferFailed('bytes that are not the name\'s');
    }
    return bytes;
  }

  static String _suffix() => List.generate(
    8,
    (_) => _random.nextInt(256).toRadixString(16).padLeft(2, '0'),
  ).join();
}

// MARK: A layer's next revision

/// What SaveInkLayer needs for one surface's drawing, its blobs uploaded
/// (contract §4).
@immutable
class InkLayerUpload {
  const InkLayerUpload({
    required this.page1based,
    required this.inkLayerId,
    required this.baseRevision,
    required this.localSeq,
    required this.sourceSha256,
    required this.coordinateSpace,
    required this.drawingSpace,
    required this.drawingToDocument,
    required this.contentVersion,
    required this.drawingSha256,
    required this.previewSha256,
    required this.strokeCount,
  });

  /// Null for a surface with one drawing.
  final int? page1based;

  /// Null to create the layer.
  final String? inkLayerId;

  /// The revision this follows; null to create.
  final int? baseRevision;

  /// The drawingChanged this is.
  final int? localSeq;
  final String sourceSha256;

  /// As the evidence fixtures' `coordinate_space` (InkSpace's fields).
  final Map<String, Object?> coordinateSpace;
  final String drawingSpace;
  final List<double> drawingToDocument;
  final int contentVersion;
  final String drawingSha256;
  final String previewSha256;
  final int strokeCount;
}

/// A page's export didn't match what the surface shows: its blobs aren't
/// what native said, or it's of another version of the file than the
/// layer's.
class InkExportMismatch implements Exception {
  const InkExportMismatch(this.what);
  final String what;

  @override
  String toString() => 'InkExportMismatch($what)';
}

/// A page's drawing exported and its blobs uploaded, ready to save.
class InkLayerUploader {
  InkLayerUploader(this.blobs);

  final InkBlobs blobs;

  /// [page1based]'s drawing (null: the surface's one drawing) exported by
  /// native, both blobs uploaded and checked against what native named,
  /// and the temporary files deleted whatever happens. [sourceSha256]: the
  /// version the layer belongs to; an export of another is refused.
  Future<InkLayerUpload> prepare(
    InkSurfaceController controller, {
    int? page1based,
    String? sourceSha256,
  }) async {
    final export = await controller.exportLayer(page1based: page1based);
    try {
      final source = export.sourceSha256 ?? '';
      if (sourceSha256 != null && source != sourceSha256) {
        throw const InkExportMismatch('source_sha256');
      }
      final name = page1based == null ? 'drawing' : 'page-$page1based';
      final drawing = await blobs.upload(
        File(export.drawingFile),
        mime: 'application/x-pencilkit',
        name: '$name.pkdrawing',
      );
      if (drawing != export.drawingSha256) {
        throw const InkExportMismatch('drawing');
      }
      final preview = await blobs.upload(
        File(export.previewFile),
        mime: 'image/png',
        name: '$name-preview.png',
      );
      if (preview != export.previewSha256) {
        throw const InkExportMismatch('ink_preview');
      }
      return InkLayerUpload(
        page1based: export.page1based,
        inkLayerId: export.inkLayerId,
        baseRevision: export.baseRevision,
        localSeq: export.localSeq,
        sourceSha256: source,
        coordinateSpace: export.coordinateSpace,
        drawingSpace: export.drawingSpace,
        drawingToDocument: export.drawingToDocument,
        contentVersion: export.contentVersion,
        drawingSha256: drawing,
        previewSha256: preview,
        strokeCount: export.strokeCount,
      );
    } finally {
      for (final path in [export.drawingFile, export.previewFile]) {
        if (path.isEmpty) continue;
        await File(path).delete().then((_) {}, onError: (Object _) {});
      }
    }
  }
}

// MARK: One surface's drawings

/// What the sync knows of one page's ink (or the surface's one drawing):
/// the layer it saves to, the drawing on screen, and where saving is.
class _Page {
  /// The layer this page saves to; null until its first save, and again
  /// when its ink is carried onto a new version of the file.
  String? id;

  /// The layer's latest revision this device knows; 0 before the first.
  int revision = 0;

  /// The version of the file the layer belongs to.
  String? source;

  /// What's on screen, for a view made again.
  InkDrawingData? drawing;

  /// The newest change not saved yet (its local_seq).
  int? pending;

  /// The drawing last saved from here, or loaded: a broadcast of it is no
  /// news.
  String? savedSha;
  bool saving = false;
  bool again = false;

  /// The save under way, from the moment it's asked for.
  Future<void>? running;
  InkUnsavedReason? reason;

  /// Another device saved this page first: its layer as it is now.
  InkLayer? conflict;

  /// The PDF changed: this ink is of the earlier version, not shown.
  bool outdated = false;

  /// Strokes on screen: drawingChanged's count, or loadDrawing's for a
  /// drawing restored or followed.
  int strokes = 0;
}

/// One surface's drawings kept as ink layers on the backend (contract §4),
/// for Sorbet's document host: it calls [changed] for every drawingChanged,
/// [ready] for every ready, [reloaded] after a new version shows, [flush]
/// before the view goes and [dispose] when the surface does, and shows
/// [state]. Nothing is kept on the device: what's held here is what's on
/// screen and what's being saved, for the surface's life.
class InkLayerSync {
  InkLayerSync({
    required this.backend,
    required this.path,
    required String machine,
    InkBlobs? blobs,
    this.journal = const InkJournal(),
  }) : _name = machine,
       blobs = blobs ?? InkBlobs(backend) {
    _uploader = InkLayerUploader(this.blobs);
    backend.connected.addListener(_connectedChanged);
    backend.inkLayerLists.addListener(_listed);
  }

  final Backend backend;
  final InkBlobs blobs;

  /// Drawings not yet saved, kept on the iPad until the backend has them;
  /// null keeps nothing.
  final InkJournal? journal;

  /// This Claktory and this file in the journal, by hash.
  late final _claktoryKey = InkJournal.claktoryKey(backend.target?.endpointId);
  late final _documentKey = InkJournal.documentKey(path);
  late final InkLayerUploader _uploader;

  /// The file, as the pane names it.
  final String path;

  /// The Claktory's name, for "Saved to `machine`"; a new one shows at
  /// once.
  String get machine => _name;
  set machine(String name) {
    if (name == _name) return;
    _name = name;
    _publish();
  }

  String _name;

  /// What the status line says.
  final state = ValueNotifier<InkSaveState>(const InkNothingToSave());

  /// Fires whenever what the sync knows of the pages may have changed
  /// ([inkedPages], [layerOf]), even when [state] stays the same.
  Listenable get changes => _changes;
  final _changes = _Changes();

  InkSurfaceController? _controller;

  /// The version shown.
  String? _source;

  /// [path] as the backend names it (its lists and broadcasts).
  String? _canonical;
  Future<void>? _restoring;
  var _restored = false;

  /// This Claktory can't keep drawings (a backend before ink layers).
  var _unsupported = false;
  var _disposed = false;
  final _pages = <int?, _Page>{};
  final _saves = <Future<void>>{};
  static final _random = Random.secure();

  /// The pages with ink drawn here that isn't saved, or whose save failed.
  bool get hasUnsaved => _pages.values.any(
    (p) => p.pending != null || p.reason != null || p.conflict != null,
  );

  /// The PDF pages with ink on screen, in order: drawn here, restored or
  /// followed from another device; never an earlier version's.
  List<int> get inkedPages => [
    for (final MapEntry(:key, :value) in _pages.entries)
      if (key != null && value.strokes > 0 && !value.outdated) key,
  ]..sort();

  /// Whether a page ([page1based]; null for an image or a sketch) has ink
  /// on screen.
  bool inked(int? page1based) {
    final page = _pages[page1based];
    return page != null && page.strokes > 0 && !page.outdated;
  }

  /// The layer a page's ink is saved to and its revision, as a composite
  /// names them; both null before its first save (or after its ink was
  /// carried onto a new version and not saved yet).
  ({String? inkLayerId, int? revision}) layerOf(int? page1based) {
    final page = _pages[page1based];
    final id = page?.outdated == false ? page?.id : null;
    return (inkLayerId: id, revision: id == null ? null : page!.revision);
  }

  // MARK: The host's calls

  /// A view is ready, its version [sourceSha256]: the first time, what the
  /// backend keeps of this version is loaded onto it; after that (a view
  /// made again), what's held here, saved or not.
  Future<void> ready(
    InkSurfaceController controller, {
    required String sourceSha256,
  }) async {
    if (_disposed) return;
    _controller = controller;
    _source ??= sourceSha256;
    if (!_restored) {
      _restored = true;
      await (_restoring = _restore());
    } else {
      await _restoring;
      await _loadHeld();
    }
  }

  /// A drawingChanged: saved as its layer's next revision, once any save
  /// of the page under way is done.
  Future<void> changed(
    InkSurfaceController controller,
    InkDrawingChanged event,
  ) async {
    if (_disposed) return;
    _controller = controller;
    final page = _pages.putIfAbsent(event.page1based, _Page.new);
    // Drawn on again after its ink was set aside as an earlier version's:
    // a new layer of this version, never that layer's next revision (it
    // stays on the backend with the earlier version).
    if (page.outdated || (page.source != null && page.source != _source)) {
      page
        ..id = null
        ..revision = 0
        ..source = null
        ..savedSha = null
        ..conflict = null
        ..reason = null;
    }
    page.drawing = event.drawing;
    page.strokes = event.strokeCount;
    page.pending = event.localSeq;
    page.outdated = false;
    // Kept on the iPad until it's saved, before it's sent.
    _journalPut(event.page1based, page, event.localSeq);
    await _save(event.page1based);
  }

  /// A new version shows (reloadDocument's answer): ink on pages that
  /// changed or went belongs to the earlier version and stays there
  /// (never moved, never saved onto the new one); ink still shown is
  /// carried onto the new version as layers of its own.
  Future<void> reloaded(InkReload reload) async {
    if (_disposed || !reload.reloaded) return;
    _source = reload.sourceSha256;
    for (final ref in reload.outdatedLayers) {
      // The earlier version's: it can't be saved onto this one.
      _journalRemove(ref.page1based);
      final page = _pages.putIfAbsent(ref.page1based, _Page.new);
      // Erased there (its layer kept, empty): no ink of that version to
      // name, and nothing of it to build on.
      final erased = page.strokes == 0;
      page
        ..outdated = !erased
        ..drawing = null
        ..pending = null
        ..reason = null
        ..conflict = null;
      if (erased) {
        page
          ..id = null
          ..revision = 0
          ..source = null
          ..savedSha = null;
      }
    }
    _publish();
    // What the version now shown already has (a file changed back, say):
    // shown as on opening it, never carried over nor saved beside.
    InkLayerList? list;
    try {
      list = await backend.listInkLayers(path);
    } on Object {
      list = null;
    }
    if (_disposed) return;
    final has = <int?, InkLayer>{
      if (list != null)
        for (final layer in _current(list)) _pageOf(layer): layer,
    };
    final carried = <int?>[];
    for (final ref in reload.keptLayers) {
      final page = _pages.putIfAbsent(ref.page1based, _Page.new);
      if (page.drawing == null) continue;
      // Already drawn on in this version, and nothing new here: its own.
      if (has.containsKey(ref.page1based) &&
          page.pending == null &&
          page.reason == null) {
        continue;
      }
      page
        ..id = null
        ..revision = 0
        ..source = reload.sourceSha256
        ..conflict = null
        ..pending = page.pending ?? 0;
      carried.add(ref.page1based);
    }
    final loads = <InkLayerDrawing>[];
    for (final MapEntry(key: at, value: layer) in has.entries) {
      if (carried.contains(at)) continue;
      final page = _pages.putIfAbsent(at, _Page.new);
      if (page.pending != null || page.saving) continue;
      final drawing = await _drawingOf(layer);
      if (_disposed) return;
      if (drawing == null || page.pending != null || page.saving) continue;
      page
        ..id = layer.id
        ..revision = layer.revision
        ..source = layer.sourceSha256
        ..drawing = drawing
        ..savedSha = drawing.sha256
        ..outdated = false
        ..reason = null
        ..conflict = null;
      loads.add(
        InkLayerDrawing(
          page1based: at,
          inkLayerId: layer.id,
          revision: layer.revision,
          drawing: drawing,
        ),
      );
    }
    await _load(loads);
    _publish();
    await Future.wait([for (final p in carried) _save(p)]);
  }

  /// Before the view goes: every change is saved now, or says why not.
  Future<void> flush() async {
    if (_disposed) return;
    for (final MapEntry(:key, :value) in _pages.entries.toList()) {
      if (value.pending != null && !value.saving && value.conflict == null) {
        unawaited(_save(key));
      }
    }
    await Future.wait(_saves.toList());
  }

  /// "Keep both" on a conflict: the drawing another device saved first is
  /// put under the person's on the page, and the two are saved together as
  /// that layer's next revision, so every device shows both. Native sends
  /// the change ([changed]); nothing of theirs is lost, nothing of the
  /// person's either.
  Future<void> keepBoth(int? page1based) async {
    final page = _pages[page1based];
    final theirs = page?.conflict;
    final controller = _controller;
    if (page == null || theirs == null || controller == null) return;
    if (theirs.id.isEmpty) {
      // The backend didn't say whose: the next change tries again.
      page.conflict = null;
      return _save(page1based);
    }
    final drawing = await _drawingOf(theirs);
    if (_disposed) return;
    if (drawing == null) {
      page.reason = const InkFailed();
      return _publish();
    }
    page
      ..id = theirs.id
      ..revision = theirs.revision
      ..source = theirs.sourceSha256
      ..savedSha = drawing.sha256
      ..conflict = null
      ..reason = null;
    _publish();
    try {
      await controller.loadDrawing([
        InkLayerDrawing(
          page1based: page1based,
          inkLayerId: theirs.id,
          revision: theirs.revision,
          drawing: drawing,
          keepMine: true,
        ),
      ]);
    } on Object {
      // The view went: a view made again is loaded with what's held, and
      // the person's ink saves on their next change.
    }
  }

  void dispose() {
    if (_disposed) return;
    _disposed = true;
    backend.connected.removeListener(_connectedChanged);
    backend.inkLayerLists.removeListener(_listed);
    state.dispose();
    _changes.dispose();
  }

  // MARK: Restoring

  Future<void> _restore() async {
    final InkLayerList list;
    try {
      list = await backend.listInkLayers(path);
    } on RequestFailure catch (failure) {
      if (failure.code == ErrorCode.ERROR_CODE_UNKNOWN_MESSAGE) {
        _unsupported = true;
      } else {
        // Not connected, say: asked again when it connects.
        _restored = false;
      }
      _publish();
      return;
    }
    if (_disposed) return;
    _canonical = list.path;
    final loads = <InkLayerDrawing>[];
    // What was drawn here and never saved (the app quit, offline): shown
    // in place of the backend's, and saved now on the layer it was drawn
    // on, so another device's save since is a conflict, never overwritten.
    for (final entry in await _journalList()) {
      final at = entry.page1based;
      final page = _pages.putIfAbsent(at, _Page.new);
      if (page.pending != null) continue;
      // Saved after all (the app went between the save and forgetting it):
      // nothing to send again.
      final saved = list.layers.any(
        (l) =>
            l.drawing.sha256 == entry.drawing.sha256 &&
            l.sourceSha256 == entry.sourceSha256 &&
            _pageOf(l) == at,
      );
      if (saved) {
        _journalRemove(at);
        continue;
      }
      if (entry.sourceSha256 != _source) {
        // Drawn on an earlier version: it can't be saved onto this one.
        _journalRemove(at);
        if (page.id == null && page.drawing == null) page.outdated = true;
        continue;
      }
      page
        ..id = entry.inkLayerId
        ..revision = entry.inkLayerId == null ? 0 : entry.baseRevision ?? 0
        ..source = entry.sourceSha256
        ..drawing = entry.drawing
        ..pending = 0;
      loads.add(
        InkLayerDrawing(
          page1based: at,
          inkLayerId: entry.inkLayerId,
          revision: entry.baseRevision,
          drawing: entry.drawing,
        ),
      );
    }
    if (_disposed) return;
    for (final layer in _current(list)) {
      final at = _pageOf(layer);
      final page = _pages.putIfAbsent(at, _Page.new);
      // Drawn on here before the backend's came: not loaded over. Saving
      // it creates the page's layer, which says another device has one.
      if (page.pending != null) continue;
      final drawing = await _drawingOf(layer);
      if (_disposed) return;
      if (drawing == null || page.pending != null) continue;
      page
        ..id = layer.id
        ..revision = layer.revision
        ..source = layer.sourceSha256
        ..drawing = drawing
        ..savedSha = drawing.sha256;
      loads.add(
        InkLayerDrawing(
          page1based: at,
          inkLayerId: layer.id,
          revision: layer.revision,
          drawing: drawing,
        ),
      );
    }
    await _markOutdated(list);
    if (_disposed) return;
    if (loads.isNotEmpty) await _load(loads);
    _publish();
    // Changes made meanwhile go now.
    for (final MapEntry(:key, :value) in _pages.entries.toList()) {
      if (value.pending != null && !value.saving) unawaited(_save(key));
    }
  }

  /// The layers of the version shown, one per page: never another
  /// version's (they're of the earlier file), never a kept-beside copy.
  Iterable<InkLayer> _current(InkLayerList list) {
    final best = <int?, InkLayer>{};
    for (final layer in list.layers) {
      if (layer.sourceSha256 != _source || layer.conflictOf.isNotEmpty) {
        continue;
      }
      final at = _pageOf(layer);
      final had = best[at];
      if (had == null || layer.revision > had.revision) best[at] = layer;
    }
    return best.values;
  }

  /// Pages whose only ink is of an earlier version of the file: each
  /// version's latest drawing of the page, when it has strokes (a page
  /// erased there keeps its layer, empty, and isn't named).
  Future<void> _markOutdated(InkLayerList list) async {
    final latest = <(int?, String), InkLayer>{};
    for (final layer in list.layers) {
      if (layer.sourceSha256 == _source || layer.conflictOf.isNotEmpty) {
        continue;
      }
      final key = (_pageOf(layer), layer.sourceSha256);
      final had = latest[key];
      if (had == null || layer.revision > had.revision) latest[key] = layer;
    }
    bool open(int? at) {
      final page = _pages[at];
      return page == null ||
          (page.id == null && page.drawing == null && page.pending == null);
    }

    for (final layer in latest.values) {
      final at = _pageOf(layer);
      if (!open(at) || (_pages[at]?.outdated ?? false)) continue;
      final inked = await _hasStrokes(layer);
      if (_disposed) return;
      if (inked && open(at)) _pages.putIfAbsent(at, _Page.new).outdated = true;
    }
  }

  /// Whether a layer's drawing has strokes, told by its preview (the ink
  /// alone, transparent elsewhere): one with nothing opaque is empty. A
  /// preview that can't be read or decoded counts as ink.
  Future<bool> _hasStrokes(InkLayer layer) async {
    final sha = layer.preview.sha256;
    if (sha.isEmpty) return true;
    try {
      final bytes = await blobs.read(sha);
      final codec = await ui.instantiateImageCodec(Uint8List.fromList(bytes));
      final frame = await codec.getNextFrame();
      codec.dispose();
      final image = frame.image;
      final data = await image.toByteData();
      image.dispose();
      if (data == null) return true;
      // RGBA, a byte each: alpha is every fourth.
      final pixels = data.buffer.asUint8List(
        data.offsetInBytes,
        data.lengthInBytes,
      );
      for (var i = 3; i < pixels.length; i += 4) {
        if (pixels[i] != 0) return true;
      }
      return false;
    } on Object {
      return true;
    }
  }

  static int? _pageOf(InkLayer layer) =>
      layer.space.kind == 'pdf_page' ? layer.space.page1based : null;

  Future<InkDrawingData?> _drawingOf(InkLayer layer) async {
    try {
      final bytes = await blobs.read(layer.drawing.sha256);
      return InkDrawingData.bytes(
        bytes,
        sha256: layer.drawing.sha256,
        contentVersion: layer.drawing.contentVersion,
      );
    } on Object {
      // Unreadable now: the page shows nothing of it, and isn't saved
      // over (its layer stays as it is until it's read).
      return null;
    }
  }

  Future<void> _load(List<InkLayerDrawing> layers) async {
    final controller = _controller;
    if (controller == null || layers.isEmpty) return;
    final InkLoadResult result;
    try {
      result = await controller.loadDrawing(layers);
    } on Object {
      // The view went; the next ready loads what's held.
      return;
    }
    if (_disposed) return;
    int? at(Map<String, Object?> entry) =>
        (entry['page_1based'] as num?)?.toInt();
    for (final entry in result.loaded) {
      _pages[at(entry)]?.strokes = (entry['strokes'] as num?)?.toInt() ?? 0;
    }
    // A newer PencilKit's, or damaged: nothing of it shows.
    for (final entry in result.unreadable) {
      _pages[at(entry)]?.strokes = 0;
    }
    _changes.notify();
  }

  /// A view made again: everything held, saved or not.
  Future<void> _loadHeld() => _load([
    for (final MapEntry(:key, :value) in _pages.entries)
      if (value.drawing case final drawing? when !value.outdated)
        InkLayerDrawing(
          page1based: key,
          inkLayerId: value.id,
          revision: value.id == null ? null : value.revision,
          drawing: drawing,
        ),
  ]);

  // MARK: Another device saved

  /// The backend's list of this file's layers, sent whenever one is saved
  /// or deleted, from any device: a page this device isn't changing takes
  /// the newer drawing; one it is changing finds out when it saves.
  void _listed() {
    final list = backend.inkLayerLists.value;
    if (_disposed || list == null || !_restored || _restoring == null) return;
    if (list.path != (_canonical ?? path)) return;
    unawaited(_follow(list));
  }

  Future<void> _follow(InkLayerList list) async {
    await _restoring;
    final loads = <InkLayerDrawing>[];
    // A layer deleted elsewhere: its ink goes from the page, unless the
    // person is drawing there (then it saves as a new layer).
    final ids = {for (final layer in list.layers) layer.id};
    for (final MapEntry(:key, :value) in _pages.entries) {
      if (value.id == null || ids.contains(value.id) || value.outdated) {
        continue;
      }
      if (value.saving || value.pending != null || value.conflict != null) {
        continue;
      }
      value
        ..id = null
        ..revision = 0
        ..source = null
        ..drawing = null
        ..savedSha = null
        ..strokes = 0;
      loads.add(InkLayerDrawing.cleared(page1based: key));
    }
    for (final layer in _current(list)) {
      final at = _pageOf(layer);
      final page = _pages.putIfAbsent(at, _Page.new);
      if (page.saving || page.pending != null || page.conflict != null) {
        continue;
      }
      if (layer.revision <= page.revision && layer.id == page.id) continue;
      if (layer.drawing.sha256 == page.savedSha) {
        page
          ..id = layer.id
          ..revision = layer.revision;
        continue;
      }
      final drawing = await _drawingOf(layer);
      if (_disposed) return;
      if (drawing == null || page.saving || page.pending != null) continue;
      page
        ..id = layer.id
        ..revision = layer.revision
        ..source = layer.sourceSha256
        ..drawing = drawing
        ..savedSha = drawing.sha256
        ..outdated = false;
      loads.add(
        InkLayerDrawing(
          page1based: at,
          inkLayerId: layer.id,
          revision: layer.revision,
          drawing: drawing,
        ),
      );
    }
    await _load(loads);
    _publish();
  }

  // MARK: Saving

  Future<void> _save(int? at) {
    final page = _pages[at];
    if (page == null || _disposed) return Future.value();
    // One save of a page at a time, from its start (before it waits for
    // the restore): a second exports what's on the page once it's done.
    if (page.saving || page.running != null) {
      page.again = true;
      return Future.value();
    }
    final save = _saveOnce(at, page);
    page.running = save;
    _saves.add(save);
    return save.whenComplete(() {
      _saves.remove(save);
      if (identical(page.running, save)) page.running = null;
    });
  }

  Future<void> _saveOnce(int? at, _Page page) async {
    await _restoring;
    if (_disposed || page.pending == null || page.outdated) return;
    if (page.conflict != null) return _publish();
    if (_unsupported) {
      page.reason = const InkUnsupported();
      return _publish();
    }
    final controller = _controller;
    if (controller == null) return;
    page
      ..saving = true
      ..again = false
      ..reason = null;
    _publish();
    final pending = page.pending;
    try {
      final source = page.source ?? _source;
      final upload = await _uploader.prepare(
        controller,
        page1based: at,
        sourceSha256: source,
      );
      if (_disposed) return;
      final saved = await backend.saveInkLayer(
        SaveInkLayer(
          inkLayerId: page.id ?? '',
          baseRevision: page.id == null ? 0 : page.revision,
          path: path,
          sourceSha256: upload.sourceSha256,
          space: inkSpace(upload.coordinateSpace),
          drawingSpace: upload.drawingSpace,
          drawingToDocument: upload.drawingToDocument,
          drawingSha256: upload.drawingSha256,
          previewSha256: upload.previewSha256,
          contentVersion: upload.contentVersion,
          clientSaveId: _saveId(),
          osVersion: _osVersion(),
        ),
      );
      if (saved.revision > 0) {
        page
          ..id = saved.inkLayerId
          ..revision = saved.revision
          ..source = upload.sourceSha256
          ..savedSha = upload.drawingSha256;
        // Saved what was on screen when exported: newer changes go next.
        // The iPad keeps only what isn't saved, even if the view went
        // meanwhile: nothing saved is sent again after a relaunch.
        final seq = upload.localSeq;
        if (page.pending == pending &&
            (seq == null || pending == null || seq >= pending)) {
          page.pending = null;
          _journalRemove(at);
        } else {
          page.again = true;
          // The newer change is now this layer's next revision.
          _journalPut(at, page, page.pending ?? 0);
        }
        if (_disposed) return;
        try {
          await controller.layerSaved(
            page1based: at,
            inkLayerId: saved.inkLayerId,
            revision: saved.revision,
          );
        } on Object {
          // The view went; a view made again is loaded with the ids.
        }
      } else {
        if (_disposed) return;
        page.conflict = saved.hasConflict() ? saved.conflict : InkLayer();
        page.again = false;
      }
    } on PlatformException catch (error) {
      if (error.code == 'nothing_drawn') {
        page.pending = null;
        _journalRemove(at);
      } else {
        page.reason = const InkFailed();
      }
    } on InkBlobTooLarge {
      page.reason = InkRefused(at == null ? _tooLarge : _pageTooLarge);
    } on InkExportMismatch {
      page.reason = const InkFailed();
    } on RequestFailure catch (failure) {
      if (failure.code == ErrorCode.ERROR_CODE_UNKNOWN_MESSAGE) {
        _unsupported = true;
        page.reason = const InkUnsupported();
      } else if (!backend.connected.value) {
        page.reason = const InkOffline();
      } else if (failure.code == ErrorCode.ERROR_CODE_NOT_FOUND &&
          page.id != null) {
        // Its layer was deleted elsewhere: the ink on the page now saves
        // as a new layer, at once.
        page
          ..id = null
          ..revision = 0
          ..source = null
          ..savedSha = null
          ..again = true;
      } else if (failure.code == ErrorCode.ERROR_CODE_INVALID_REQUEST ||
          failure.code == ErrorCode.ERROR_CODE_NOT_FOUND) {
        // The backend's own refusal, in words for the person ("This file
        // changed since you opened it.").
        page.reason = InkRefused(failure.message);
      } else {
        // The backend failing, or this app's own (an answer missing): a
        // failure, tried again.
        page.reason = const InkFailed();
      }
    } on Object {
      page.reason = const InkFailed();
    } finally {
      page.saving = false;
      page.running = null;
      if (!_disposed) {
        _publish();
        if (page.again && page.reason == null && page.conflict == null) {
          unawaited(_save(at));
        }
      }
    }
  }

  /// Connected again: what couldn't be saved or read is tried again.
  void _connectedChanged() {
    if (_disposed || !backend.connected.value) return;
    if (!_restored && _controller != null && _source != null) {
      unawaited(ready(_controller!, sourceSha256: _source!));
    }
    for (final MapEntry(:key, :value) in _pages.entries.toList()) {
      if (value.pending != null &&
          (value.reason is InkOffline || value.reason is InkFailed)) {
        unawaited(_save(key));
      }
    }
  }

  // MARK: What the status line says

  void _publish() {
    if (_disposed) return;
    List<int?> where(bool Function(_Page) test) => [
      for (final MapEntry(:key, :value) in _pages.entries)
        if (test(value)) key,
    ]..sort((a, b) => (a ?? 0).compareTo(b ?? 0));
    final conflicts = where((p) => p.conflict != null);
    final unsaved = where((p) => p.reason != null);
    final outdated = where((p) => p.outdated);
    final saving = where((p) => p.saving || p.pending != null);
    final saved = where((p) => p.id != null);
    state.value = conflicts.isNotEmpty
        ? InkConflict(conflicts)
        : unsaved.isNotEmpty
        ? InkUnsaved(unsaved, _pages[unsaved.first]!.reason!)
        : outdated.isNotEmpty
        ? InkOutdated(outdated)
        : saving.isNotEmpty
        ? (_unsupported
              ? InkUnsaved(saving, const InkUnsupported())
              : InkSaveInProgress(saving))
        : saved.isNotEmpty
        ? InkSaved(machine)
        : const InkNothingToSave();
    _changes.notify();
  }

  // MARK: The journal

  /// The journal's writes, one after another in the order asked for, so
  /// a page's entry is never removed before it was written; saving never
  /// waits for them.
  Future<void> _journalTail = Future.value();

  /// Done once every write asked for so far is.
  @visibleForTesting
  Future<void> get journalWritten => _journalTail;

  void _journalQueue(Future<void> Function(InkJournal journal) write) {
    final journal = this.journal;
    if (journal == null) return;
    _journalTail = _journalTail.then((_) async {
      try {
        await write(journal);
      } on Object {
        // Not kept on the iPad (no room, or locked): still on screen, and
        // saved when it can be; a removal goes after the page's next save.
      }
    });
  }

  /// What's on [page] now, as its change [seq], kept until it's saved.
  void _journalPut(int? at, _Page page, int seq) {
    final drawing = page.drawing;
    final source = page.source ?? _source;
    if (drawing == null || source == null) return;
    final entry = InkJournalEntry(
      page1based: at,
      seq: seq,
      sourceSha256: source,
      inkLayerId: page.id,
      baseRevision: page.id == null ? null : page.revision,
      drawing: drawing,
    );
    _journalQueue(
      (journal) => journal.put(
        claktory: _claktoryKey,
        document: _documentKey,
        entry: entry,
      ),
    );
  }

  void _journalRemove(int? at) => _journalQueue(
    (journal) => journal.remove(
      claktory: _claktoryKey,
      document: _documentKey,
      page1based: at,
    ),
  );

  Future<List<InkJournalEntry>> _journalList() async {
    try {
      return await journal?.list(
            claktory: _claktoryKey,
            document: _documentKey,
          ) ??
          const [];
    } on Object {
      return const [];
    }
  }

  static String _saveId() =>
      'save-${List.generate(12, (_) => _random.nextInt(256).toRadixString(16).padLeft(2, '0')).join()}';

  /// "18.3.1", from the system's "Version 18.3.1 (Build 22D8075)".
  static String _osVersion() {
    final found = RegExp(r'\d+(\.\d+)+')
        .firstMatch(Platform.operatingSystemVersion)
        ?.group(0);
    final version = found ?? Platform.operatingSystemVersion;
    return version.length <= 32 ? version : version.substring(0, 32);
  }
}

class _Changes extends ChangeNotifier {
  void notify() => notifyListeners();
}

/// Trish's: the limit is per page (8 MiB of drawing).
const _pageTooLarge =
    "This page's drawing is too large to keep. Erase some of it to save it.";
const _tooLarge =
    'This drawing is too large to keep. Erase some of it to save it.';

/// A layer's `coordinate_space` (as the evidence fixtures and exportLayer
/// have it) as the protocol's InkSpace.
InkSpace inkSpace(Map<String, Object?> space) {
  double number(String key) => (space[key] as num?)?.toDouble() ?? 0;
  int whole(String key) => (space[key] as num?)?.toInt() ?? 0;
  List<double> box(String key) => [
    for (final v in space[key] as List? ?? const []) (v as num).toDouble(),
  ];
  final out = InkSpace(
    kind: space['kind'] as String? ?? '',
    units: space['units'] as String? ?? '',
  );
  switch (out.kind) {
    case 'pdf_page':
      out
        ..page1based = whole('page_1based')
        ..box = space['box'] as String? ?? 'crop'
        ..rotation = whole('rotation')
        ..width = number('width')
        ..height = number('height');
      out.cropBox.addAll(box('crop_box'));
      out.mediaBox.addAll(box('media_box'));
    case 'image':
      out
        ..orientedWidth = whole('oriented_width')
        ..orientedHeight = whole('oriented_height')
        ..storedWidth = whole('stored_width')
        ..storedHeight = whole('stored_height')
        ..exifOrientation = whole('exif_orientation');
    case 'excerpt_rendition':
      final r = space['rendition'] as Map? ?? const {};
      double of(String key) => (r[key] as num?)?.toDouble() ?? 0;
      out
        ..text = space['text'] as String? ?? ''
        ..width = number('width')
        ..height = number('height')
        ..rendition = ExcerptRendition(
          font: r['font'] as String? ?? '',
          fontSize: of('font_size'),
          lineHeight: of('line_height'),
          padding: of('padding'),
        );
  }
  return out;
}
