// Keeping what's drawn (lib/ipad/ink_sync.dart) against a backend that
// answers as docs/ipad-phase3-contract.md §3–§4 says: blobs uploaded
// privately and read back by their SHA-256, layers saved with
// compare-and-set (a conflict never overwrites), the same save id answered
// once, and every save broadcast to every client.
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:crypto/crypto.dart' as crypto;
import 'package:factory_app/backend.dart';
import 'package:factory_app/ipad/ink_bridge.dart';
import 'package:factory_app/ipad/ink_sync.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart'
    hide InkComposite;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';

const _path = '/w/paper.pdf';
const _v1 = '2633caf4ddce6226590f39adee47b6ba86ac582d4794051adaf77dd25041f3fb';
const _v2 = '9c1e7f0a52b3d4e6f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f';

String _sha(List<int> bytes) => crypto.sha256.convert(bytes).toString();

Map<String, Object?> _read(String path) =>
    jsonDecode(File('test/ipad/fixtures/$path').readAsStringSync())
        as Map<String, Object?>;

/// A backend keeping ink as the contract says, in memory.
class _InkBackend extends RecordingBackend {
  /// The file's SHA-256 now.
  String current = _v1;

  /// A backend from before ink layers: every ink request is unknown.
  bool old = false;

  /// The next save fails as a request (not connected, say).
  RequestFailure? failNextSave;

  /// Answers that it took more than it was sent.
  bool lieAboutChunks = false;

  /// Parts of a blob read back at most this long.
  int part = InkBlobLimits.chunk;

  final blobs = <String, Uint8List>{};
  final _uploads = <String, (String mime, int size, BytesBuilder bytes)>{};
  final starts = <(String id, String mime, int size)>[];
  final chunks = <int>[];
  final saves = <SaveInkLayer>[];
  final _answered = <String, InkLayerSaved>{};

  /// Every layer, at its latest revision, by id.
  final layers = <String, InkLayer>{};
  var _next = 1;

  void _ink() {
    if (old) {
      throw const RequestFailure(
        'unknown message',
        code: ErrorCode.ERROR_CODE_UNKNOWN_MESSAGE,
      );
    }
  }

  @override
  Future<int> startInkUpload(
    String uploadId, {
    required String name,
    required int size,
    required String mime,
  }) async {
    _ink();
    final limit = mime == 'image/png' ? 4 << 20 : 8 << 20;
    if (size > limit) throw const RequestFailure('Too large.');
    starts.add((uploadId, mime, size));
    final had = _uploads[uploadId];
    if (had != null) return had.$3.length;
    _uploads[uploadId] = (mime, size, BytesBuilder());
    return 0;
  }

  @override
  Future<int> uploadChunk(String uploadId, int offset, List<int> data) async {
    final upload = _uploads[uploadId];
    if (upload == null) return super.uploadChunk(uploadId, offset, data);
    if (offset != upload.$3.length) throw const RequestFailure('offset');
    if (data.length > InkBlobLimits.chunk) throw const RequestFailure('chunk');
    chunks.add(data.length);
    upload.$3.add(data);
    return lieAboutChunks ? upload.$3.length + 7 : upload.$3.length;
  }

  @override
  Future<String> finishInkUpload(
    String uploadId, {
    required String sha256,
  }) async {
    final upload = _uploads.remove(uploadId)!;
    final bytes = upload.$3.takeBytes();
    if (bytes.length != upload.$2 || _sha(bytes) != sha256) {
      throw const RequestFailure('The file differs.');
    }
    blobs[sha256] = bytes;
    return sha256;
  }

  @override
  Future<void> cancelUpload(String uploadId) async {
    _uploads.remove(uploadId);
  }

  bool _referenced(String sha) => layers.values.any(
    (l) => l.drawing.sha256 == sha || l.preview.sha256 == sha,
  );

  @override
  Future<InkBlobData> readInkBlob(String sha256, {int offset = 0}) async {
    _ink();
    final bytes = blobs[sha256];
    if (bytes == null || !_referenced(sha256)) {
      return InkBlobData(sha256: sha256, error: 'unknown');
    }
    final end = (offset + part).clamp(0, bytes.length);
    return InkBlobData(
      sha256: sha256,
      offset: Int64(offset),
      size: Int64(bytes.length),
      data: bytes.sublist(offset, end),
    );
  }

  InkLayerList _list() => InkLayerList(
    path: _path,
    currentSha256: current,
    layers: [
      for (final l in layers.values)
        l.deepCopy()..outdated = l.sourceSha256 != current,
    ],
  );

  @override
  Future<InkLayerList> listInkLayers(String path) async {
    _ink();
    final list = _list();
    inkLayerLists.value = list;
    return list;
  }

  static String _surface(InkSpace s) =>
      s.kind == 'pdf_page' ? 'page ${s.page1based}' : s.kind;

  /// Another device saves [page]'s next revision (or its first).
  InkLayer otherDevice(
    int page,
    List<int> drawing, {
    String? source,
    List<int>? preview,
  }) {
    final sha = _sha(drawing);
    blobs[sha] = Uint8List.fromList(drawing);
    if (preview != null) blobs[_sha(preview)] = Uint8List.fromList(preview);
    final had = layers.values
        .where(
          (l) =>
              l.space.page1based == page &&
              l.conflictOf.isEmpty &&
              (source == null || l.sourceSha256 == source),
        )
        .firstOrNull;
    final layer = InkLayer(
      id: had?.id ?? 'layer-${_next++}',
      path: _path,
      sourceSha256: source ?? had?.sourceSha256 ?? current,
      space: InkSpace(kind: 'pdf_page', page1based: page),
      revision: (had?.revision ?? 0) + 1,
      parentRevision: had?.revision ?? 0,
      drawing: InkBlobRef(
        format: 'pencilkit',
        contentVersion: 1,
        sha256: sha,
        bytes: Int64(drawing.length),
      ),
      preview: preview == null
          ? null
          : InkBlobRef(sha256: _sha(preview), bytes: Int64(preview.length)),
      deviceKind: 'tablet',
    );
    layers[layer.id] = layer;
    inkLayerLists.value = _list();
    return layer;
  }

  /// Another device deleted a layer: the list goes to everyone.
  void deletedElsewhere(String id) {
    layers.remove(id);
    inkLayerLists.value = _list();
  }

  /// Saves wait for it, while set.
  Completer<void>? hold;

  /// Saves waiting for [hold] now.
  var holding = 0;

  Future<void> untilHolding() async {
    while (holding == 0) {
      await Future<void>.delayed(const Duration(milliseconds: 1));
    }
  }

  @override
  Future<InkLayerSaved> saveInkLayer(SaveInkLayer save) async {
    _ink();
    if (hold case final held?) {
      holding++;
      await held.future;
      holding--;
    }
    if (failNextSave case final failure?) {
      failNextSave = null;
      throw failure;
    }
    saves.add(save);
    if (_answered[save.clientSaveId] case final again?) return again;
    for (final sha in [save.drawingSha256, save.previewSha256]) {
      if (!blobs.containsKey(sha)) {
        throw const RequestFailure(
          'That drawing isn\'t here; upload it first.',
        );
      }
    }
    final surface = _surface(save.space);
    InkLayerSaved answer;
    if (save.inkLayerId.isEmpty) {
      if (save.sourceSha256 != current) {
        throw const RequestFailure(
          'This file changed since you opened it.',
          code: ErrorCode.ERROR_CODE_INVALID_REQUEST,
        );
      }
      final taken = layers.values.where(
        (l) =>
            l.sourceSha256 == save.sourceSha256 &&
            _surface(l.space) == surface &&
            l.conflictOf.isEmpty,
      );
      if (taken.isNotEmpty && save.conflictOf.isEmpty) {
        answer = InkLayerSaved(
          clientSaveId: save.clientSaveId,
          conflict: taken.first,
        );
      } else {
        answer = _write(save, 'layer-${_next++}', 0);
      }
    } else {
      final layer = layers[save.inkLayerId];
      if (layer == null) {
        throw RequestFailure(
          'There\'s no ink layer "${save.inkLayerId}"; it may have been '
          'deleted.',
          code: ErrorCode.ERROR_CODE_NOT_FOUND,
        );
      }
      answer = save.baseRevision == layer.revision
          ? _write(save, layer.id, layer.revision)
          : InkLayerSaved(clientSaveId: save.clientSaveId, conflict: layer);
    }
    _answered[save.clientSaveId] = answer;
    if (answer.revision > 0) inkLayerLists.value = _list();
    return answer;
  }

  InkLayerSaved _write(SaveInkLayer save, String id, int base) {
    final layer = InkLayer(
      id: id,
      path: _path,
      sourceSha256: save.sourceSha256,
      space: save.space,
      revision: base + 1,
      parentRevision: base,
      drawing: InkBlobRef(
        format: 'pencilkit',
        contentVersion: save.contentVersion,
        sha256: save.drawingSha256,
        bytes: Int64(blobs[save.drawingSha256]!.length),
      ),
      preview: InkBlobRef(
        sha256: save.previewSha256,
        bytes: Int64(blobs[save.previewSha256]!.length),
      ),
      drawingSpace: save.drawingSpace,
      drawingToDocument: save.drawingToDocument,
      conflictOf: save.conflictOf,
      osVersion: save.osVersion,
    );
    layers[id] = layer;
    return InkLayerSaved(
      clientSaveId: save.clientSaveId,
      inkLayerId: id,
      revision: base + 1,
    );
  }
}

/// The surface's native side: what's on each page, exported to temporary
/// files as exportLayer does, and what it was told.
class _Native {
  _Native() {
    _messenger.setMockMethodCallHandler(channel, (call) async {
      calls.add(call);
      final a = (call.arguments as Map?)?.cast<String, Object?>() ?? {};
      switch (call.method) {
        case 'exportLayer':
          return _export(a['page_1based'] as int?);
        case 'loadDrawing':
          loaded.add([
            for (final l in (a['layers'] as List).cast<Map>())
              l.cast<String, Object?>(),
          ]);
          final layers = (a['layers'] as List).cast<Map>();
          bool bad(Map l) => unreadable.contains(l['page_1based']);
          Map<String, Object?> entry(Map l) => {
            'page_1based': l['page_1based'],
            'ink_layer_id': l['ink_layer_id'],
            'revision': l['revision'],
          };
          return {
            'loaded': [
              for (final l in layers)
                if (!bad(l))
                  {...entry(l), 'strokes': l['clear'] == true ? 0 : 2},
            ],
            'unreadable': [
              for (final l in layers)
                if (bad(l)) {...entry(l), 'content_version': 9},
            ],
          };
        case 'layerSaved':
          told.add(a);
          return <String, Object?>{};
      }
      return null;
    });
  }

  static TestDefaultBinaryMessenger get _messenger =>
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

  final channel = const MethodChannel('claktory/ink/test');
  final calls = <MethodCall>[];
  final loaded = <List<Map<String, Object?>>>[];
  final told = <Map<String, Object?>>[];
  final files = <String>[];
  final dir = Directory.systemTemp.createTempSync('ink-sync');

  /// What's on screen: each page's drawing and the change it was.
  final drawn = <int?, (List<int> bytes, int seq)>{};
  String source = _v1;

  /// Pages whose drawing loadDrawing can't read (a newer PencilKit's).
  final unreadable = <int?>{};

  /// A blob larger than the backend takes.
  var hugeDrawing = false;

  Map<String, Object?> _export(int? page) {
    final now = drawn[page];
    if (now == null) throw PlatformException(code: 'nothing_drawn');
    final bytes = hugeDrawing ? Uint8List(9 << 20) : Uint8List.fromList(now.$1);
    final png = Uint8List.fromList([
      137,
      80,
      78,
      71,
      ...utf8.encode('$page ${now.$2}'),
    ]);
    final drawing = File('${dir.path}/${files.length}.pkdrawing')
      ..writeAsBytesSync(bytes);
    final preview = File('${dir.path}/${files.length}.png')
      ..writeAsBytesSync(png);
    files.addAll([drawing.path, preview.path]);
    final fixture = _read('bridge/cmd_export_layer.json')['result'] as Map;
    return {
      ...fixture.cast<String, Object?>(),
      'page_1based': page,
      'local_seq': now.$2,
      'source_sha256': source,
      'coordinate_space': {
        ...(fixture['coordinate_space'] as Map).cast<String, Object?>(),
        'page_1based': page,
      },
      'drawing': {
        ...(fixture['drawing'] as Map).cast<String, Object?>(),
        'file': drawing.path,
        'sha256': _sha(bytes),
        'bytes': bytes.length,
      },
      'ink_preview': {
        ...(fixture['ink_preview'] as Map).cast<String, Object?>(),
        'file': preview.path,
        'sha256': _sha(png),
        'bytes': png.length,
      },
    };
  }

  void dispose() {
    _messenger.setMockMethodCallHandler(channel, null);
    dir.deleteSync(recursive: true);
  }
}

/// The iPad's journal, in memory: what a relaunch finds.
class _Journal extends InkJournal {
  final kept = <String, InkJournalEntry>{};
  String? claktory;

  String _key(String document, int? page) => '$document/$page';

  @override
  Future<void> put({
    required String claktory,
    required String document,
    required InkJournalEntry entry,
  }) async {
    this.claktory = claktory;
    kept[_key(document, entry.page1based)] = entry;
  }

  @override
  Future<void> remove({
    required String claktory,
    required String document,
    int? page1based,
    int? uptoSeq,
  }) async => kept.remove(_key(document, page1based));

  @override
  Future<List<InkJournalEntry>> list({
    required String claktory,
    required String document,
  }) async => [
    for (final MapEntry(:key, :value) in kept.entries)
      if (key.startsWith('$document/')) value,
  ];
}

/// A preview as the iPad makes one: the ink alone, transparent elsewhere.
Future<List<int>> _png({required bool ink}) async {
  final recorder = ui.PictureRecorder();
  final canvas = ui.Canvas(recorder);
  if (ink) {
    canvas.drawLine(
      const ui.Offset(2, 2),
      const ui.Offset(14, 10),
      ui.Paint()
        ..strokeWidth = 2
        ..color = const ui.Color(0xFF1A1A1A),
    );
  }
  final image = await recorder.endRecording().toImage(16, 12);
  final data = await image.toByteData(format: ui.ImageByteFormat.png);
  image.dispose();
  return data!.buffer.asUint8List();
}

InkDrawingChanged _change(
  int page,
  List<int> bytes,
  int seq, {
  int strokes = 1,
  String? cause,
}) => InkDrawingChanged(
  page1based: page,
  localSeq: seq,
  cause: cause ?? (strokes == 0 ? 'erase' : 'stroke'),
  strokeCount: strokes,
  drawing: InkDrawingData.bytes(Uint8List.fromList(bytes), sha256: _sha(bytes)),
  sourceSha256: _v1,
);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('blobs', () {
    late _InkBackend backend;
    setUp(() => backend = _InkBackend());

    test('go up privately, a MiB at a time, and come back checked', () async {
      final blobs = InkBlobs(backend);
      final bytes = Uint8List.fromList(
        List.generate((2 << 20) + 5, (i) => i % 251),
      );
      final sha = await blobs.uploadBytes(
        bytes,
        mime: 'application/x-pencilkit',
        name: 'page-3.pkdrawing',
      );
      expect(sha, _sha(bytes));
      expect(backend.starts.single.$2, 'application/x-pencilkit');
      expect(backend.chunks, [1 << 20, 1 << 20, 5]);
      // Nothing refers to it yet: it can't be read.
      await expectLater(blobs.read(sha), throwsA(isA<InkBlobUnknown>()));
      backend.layers['layer-x'] = InkLayer(
        id: 'layer-x',
        drawing: InkBlobRef(sha256: sha),
      );
      backend.part = 700 * 1024;
      expect(await blobs.read(sha), bytes);
      // Bytes that aren't the name's are refused.
      backend.blobs[sha] = Uint8List.fromList([1, 2, 3]);
      await expectLater(blobs.read(sha), throwsA(isA<InkTransferFailed>()));
    });

    test('too large for its kind is refused before anything goes', () async {
      final blobs = InkBlobs(backend);
      await expectLater(
        blobs.uploadBytes(
          Uint8List((4 << 20) + 1),
          mime: 'image/png',
          name: 'preview.png',
        ),
        throwsA(isA<InkBlobTooLarge>()),
      );
      expect(backend.starts, isEmpty);
    });
  });

  group('a surface', () {
    late _InkBackend backend;
    late _Native native;
    late InkSurfaceController controller;
    late InkLayerSync sync;
    late _Journal journal;

    setUp(() async {
      backend = _InkBackend();
      native = _Native();
      journal = _Journal();
      controller = InkSurfaceController();
      await controller.attach(native.channel);
      sync = InkLayerSync(
        backend: backend,
        path: _path,
        machine: 'Studio',
        journal: journal,
      );
    });

    tearDown(() {
      sync.dispose();
      controller.dispose();
      native.dispose();
    });

    Future<void> draw(int page, String what, int seq) {
      final bytes = utf8.encode(what);
      native.drawn[page] = (bytes, seq);
      return sync.changed(controller, _change(page, bytes, seq));
    }

    test('saves each change as its layer\'s next revision', () async {
      await sync.ready(controller, sourceSha256: _v1);
      expect(sync.state.value, isA<InkNothingToSave>());
      final states = <InkSaveState>[];
      sync.state.addListener(() => states.add(sync.state.value));

      await draw(3, 'a circle', 1);
      final first = backend.saves.single;
      expect((first.inkLayerId, first.baseRevision), ('', 0));
      expect(first.path, _path);
      expect(first.sourceSha256, _v1);
      expect(first.space.kind, 'pdf_page');
      expect(first.space.page1based, 3);
      expect(first.space.cropBox, [0, 0, 612, 792]);
      expect(first.drawingSpace, 'page_overlay');
      expect(first.drawingToDocument, [1, 0, 0, -1, 0, 792]);
      expect(first.drawingSha256, _sha(utf8.encode('a circle')));
      expect(first.contentVersion, 1);
      expect(first.clientSaveId, isNotEmpty);
      expect(first.osVersion.length, lessThanOrEqualTo(32));
      // Uploaded before saving, both as ink.
      expect(backend.starts.map((s) => s.$2), [
        'application/x-pencilkit',
        'image/png',
      ]);
      expect(native.told.single, {
        'page_1based': 3,
        'ink_layer_id': 'layer-1',
        'revision': 1,
      });
      // The temporary files are gone.
      expect(native.files.where((f) => File(f).existsSync()), isEmpty);
      expect(states.first, isA<InkSaveInProgress>());
      expect(sync.state.value, isA<InkSaved>());
      expect((sync.state.value as InkSaved).machine, 'Studio');
      // The Claktory renamed: shown at once.
      sync.machine = 'Workshop';
      expect((sync.state.value as InkSaved).machine, 'Workshop');
      sync.machine = 'Studio';

      await draw(3, 'a circle and a line', 2);
      final second = backend.saves.last;
      expect((second.inkLayerId, second.baseRevision), ('layer-1', 1));
      expect(native.told.last['revision'], 2);
      expect(backend.layers['layer-1']!.revision, 2);
      expect(second.clientSaveId, isNot(first.clientSaveId));
    });

    test('a change made while saving goes next, never lost', () async {
      await sync.ready(controller, sourceSha256: _v1);
      final one = draw(3, 'one', 1);
      final two = draw(3, 'one, two', 2);
      await Future.wait([one, two]);
      await sync.flush();
      expect(
        backend.layers['layer-1']!.drawing.sha256,
        _sha(utf8.encode('one, two')),
      );
      // The second came before the first was exported: one save of both.
      expect(backend.saves.length, 1);
      expect(sync.state.value, isA<InkSaved>());

      // One that comes while a save is under way goes next.
      backend.hold = Completer<void>();
      final three = draw(3, 'one, two, three', 3);
      await backend.untilHolding();
      final four = draw(3, 'one, two, three, four', 4);
      backend.hold!.complete();
      backend.hold = null;
      await Future.wait([three, four]);
      await sync.flush();
      expect(
        backend.layers['layer-1']!.drawing.sha256,
        _sha(utf8.encode('one, two, three, four')),
      );
      expect(backend.saves.length, 3);
    });

    test('another device saving first is a conflict, never overwritten; '
        'keep both puts theirs under the person\'s, saved as one', () async {
      await sync.ready(controller, sourceSha256: _v1);
      await draw(3, 'mine', 1);
      final theirs = backend.otherDevice(3, utf8.encode('theirs'));
      expect(theirs.revision, 2);
      // Their save reached this device while it was drawing again.
      native.drawn[3] = (utf8.encode('mine, more'), 2);
      await sync.changed(controller, _change(3, utf8.encode('mine, more'), 2));
      expect(
        backend.layers['layer-1']!.drawing.sha256,
        _sha(utf8.encode('theirs')),
      );
      expect(sync.state.value, isA<InkConflict>());
      expect((sync.state.value as InkConflict).pages, [3]);
      final told = native.told.length;

      final saves = backend.saves.length;
      await sync.keepBoth(3);
      // Native is given theirs, to put under what's on the page.
      final load = native.loaded.last.single;
      expect(load['keep_mine'], isTrue);
      expect((load['ink_layer_id'], load['revision']), ('layer-1', 2));
      final wire = (load['drawing'] as Map).cast<String, Object?>();
      expect(
        base64Decode(wire['bytes_base64'] as String),
        utf8.encode('theirs'),
      );
      expect(backend.saves.length, saves);
      expect(sync.state.value, isA<InkSaveInProgress>());

      // Native sends both as the page's change: saved on their layer.
      final both = utf8.encode('theirs + mine, more');
      native.drawn[3] = (both, 3);
      await sync.changed(controller, _change(3, both, 3, cause: 'keep_both'));
      final save = backend.saves.last;
      expect((save.inkLayerId, save.baseRevision), ('layer-1', 2));
      expect(save.conflictOf, isEmpty);
      expect(backend.layers['layer-1']!.revision, 3);
      expect(backend.layers['layer-1']!.drawing.sha256, _sha(both));
      expect(
        backend.layers.values.where((l) => l.conflictOf.isNotEmpty),
        isEmpty,
      );
      expect(native.told.length, told + 1);
      expect(sync.layerOf(3), (inkLayerId: 'layer-1', revision: 3));
      expect(sync.state.value, isA<InkSaved>());
    });

    test('opening loads what the backend keeps of this version only', () async {
      final kept = backend.otherDevice(3, utf8.encode('kept'));
      backend.otherDevice(5, utf8.encode('of before'), source: _v2);
      backend.part = 3;
      await sync.ready(controller, sourceSha256: _v1);
      final layers = native.loaded.single;
      expect(layers.map((l) => l['page_1based']), [3]);
      expect(layers.single['ink_layer_id'], kept.id);
      expect(layers.single['revision'], 1);
      final drawing = (layers.single['drawing'] as Map).cast<String, Object?>();
      expect(
        base64Decode(drawing['bytes_base64'] as String),
        utf8.encode('kept'),
      );
      expect(drawing['sha256'], kept.drawing.sha256);
      // Page 5's ink is of an earlier version: not shown, and said.
      expect(sync.state.value, isA<InkOutdated>());
      expect((sync.state.value as InkOutdated).pages, [5]);

      // Drawing on page 3 again saves its next revision.
      await draw(3, 'kept, and more', 1);
      expect(
        (backend.saves.last.inkLayerId, backend.saves.last.baseRevision),
        (kept.id, 1),
      );
    });

    test('which pages have ink and their layers: drawn here, restored and '
        'followed', () async {
      final kept = backend.otherDevice(3, utf8.encode('kept'));
      backend.otherDevice(5, utf8.encode('of before'), source: _v2);
      backend.otherDevice(7, utf8.encode('from a newer iPad'));
      native.unreadable.add(7);
      var fired = 0;
      sync.changes.addListener(() => fired++);
      await sync.ready(controller, sourceSha256: _v1);
      // Restored: page 3. Page 5's is of an earlier version and page 7's
      // can't be shown.
      expect(sync.inkedPages, [3]);
      expect(sync.layerOf(3), (inkLayerId: kept.id, revision: 1));
      expect(sync.layerOf(5), (inkLayerId: null, revision: null));
      expect(sync.layerOf(9), (inkLayerId: null, revision: null));
      expect(sync.inked(3), isTrue);
      expect(sync.inked(null), isFalse);
      expect(fired, greaterThan(0));

      await draw(4, 'drawn here', 1);
      expect(sync.inkedPages, [3, 4]);
      expect(sync.layerOf(4).inkLayerId, isNotNull);
      expect(sync.layerOf(4).revision, 1);

      // Another device draws on page 9.
      fired = 0;
      final theirs = backend.otherDevice(9, utf8.encode('theirs'));
      await Future<void>.delayed(const Duration(milliseconds: 10));
      expect(sync.inkedPages, [3, 4, 9]);
      expect(sync.layerOf(9), (inkLayerId: theirs.id, revision: 1));
      expect(fired, greaterThan(0));

      // Page 4 erased: no ink, though its layer stays.
      native.drawn[4] = (const <int>[], 2);
      await sync.changed(controller, _change(4, const [], 2, strokes: 0));
      expect(sync.inkedPages, [3, 9]);
      expect(sync.layerOf(4).revision, 2);
    });

    test('a layer deleted elsewhere goes from the page; ink drawn there '
        'meanwhile saves as a new layer', () async {
      await sync.ready(controller, sourceSha256: _v1);
      await draw(3, 'on page 3', 1);
      await draw(4, 'on page 4', 2);
      expect(sync.inkedPages, [3, 4]);

      // Page 3's layer deleted on another device: gone from the page.
      backend.deletedElsewhere('layer-1');
      await Future<void>.delayed(const Duration(milliseconds: 10));
      final cleared = native.loaded.last.single;
      expect(cleared['page_1based'], 3);
      expect(cleared['clear'], isTrue);
      expect(cleared['drawing'], isNull);
      expect(sync.inkedPages, [4]);
      expect(sync.layerOf(3), (inkLayerId: null, revision: null));
      expect(sync.state.value, isA<InkSaved>());

      // Drawing there again makes a new layer.
      await draw(3, 'again', 3);
      expect(
        (backend.saves.last.inkLayerId, backend.saves.last.baseRevision),
        ('', 0),
      );
      expect(sync.state.value, isA<InkSaved>());

      // Deleted while the person was drawing on it: their ink is kept, as
      // a new layer, never "no such layer".
      final four = sync.layerOf(4).inkLayerId!;
      backend.layers.remove(four);
      await draw(4, 'on page 4, more', 4);
      await sync.flush();
      final saved = backend.saves.last;
      expect((saved.inkLayerId, saved.baseRevision), ('', 0));
      expect(sync.layerOf(4).inkLayerId, isNot(four));
      expect(sync.state.value, isA<InkSaved>());
    });

    group('drawn offline, the app quit', () {
      /// Offline, page 3 drawn on again after its first save; the app
      /// quits and opens again connected.
      Future<InkLayerSync> quitAndOpen({void Function()? meanwhile}) async {
        await sync.ready(controller, sourceSha256: _v1);
        await draw(3, 'one', 1);
        await sync.journalWritten;
        expect(journal.kept, isEmpty);
        backend.failNextSave = const RequestFailure('Not connected.');
        backend.connected.value = false;
        await draw(3, 'one, two', 2);
        expect((sync.state.value as InkUnsaved).reason, isA<InkOffline>());
        await sync.journalWritten;
        final entry = journal.kept.values.single;
        expect(
          (entry.page1based, entry.inkLayerId, entry.baseRevision),
          (3, 'layer-1', 1),
        );
        expect(entry.sourceSha256, _v1);
        expect(entry.drawing.bytes, utf8.encode('one, two'));
        // Named by hash: no file name is kept.
        expect(journal.kept.keys.single, isNot(contains(_path)));
        sync.dispose();
        meanwhile?.call();
        backend.connected.value = true;
        final again = InkLayerSync(
          backend: backend,
          path: _path,
          machine: 'Studio',
          journal: journal,
        );
        addTearDown(again.dispose);
        await again.ready(controller, sourceSha256: backend.current);
        await again.flush();
        return again;
      }

      test(
        'is shown and saved on its layer, then gone from the iPad',
        () async {
          final again = await quitAndOpen(
            meanwhile: () => native.drawn[3] = (utf8.encode('one, two'), 1),
          );
          final shown = native.loaded.last.single;
          expect((shown['ink_layer_id'], shown['revision']), ('layer-1', 1));
          expect(
            base64Decode((shown['drawing'] as Map)['bytes_base64'] as String),
            utf8.encode('one, two'),
          );
          final save = backend.saves.last;
          expect((save.inkLayerId, save.baseRevision), ('layer-1', 1));
          expect(backend.layers['layer-1']!.revision, 2);
          expect(
            backend.layers['layer-1']!.drawing.sha256,
            _sha(utf8.encode('one, two')),
          );
          await again.journalWritten;
          expect(journal.kept, isEmpty);
          expect(again.state.value, isA<InkSaved>());
        },
      );

      test('another device saved meanwhile: a conflict, kept until it\'s '
          'settled', () async {
        final again = await quitAndOpen(
          meanwhile: () {
            native.drawn[3] = (utf8.encode('one, two'), 1);
            backend.otherDevice(3, utf8.encode('theirs'));
          },
        );
        expect(again.state.value, isA<InkConflict>());
        expect(
          backend.layers['layer-1']!.drawing.sha256,
          _sha(utf8.encode('theirs')),
        );
        await again.journalWritten;
        expect(journal.kept, hasLength(1));
      });

      test('a save confirmed after the view went still leaves the iPad; one '
          'overtaken keeps the newer change on the saved layer', () async {
        await sync.ready(controller, sourceSha256: _v1);
        backend.hold = Completer<void>();
        final first = draw(3, 'first', 1);
        await backend.untilHolding();
        // A newer change while the first is being saved.
        native.drawn[3] = (utf8.encode('first, second'), 2);
        final second = sync.changed(
          controller,
          _change(3, utf8.encode('first, second'), 2),
        );
        await Future<void>.delayed(Duration.zero);
        await sync.journalWritten;
        expect(journal.kept.values.single.seq, 2);
        // The first saves; the second waits.
        await backend.untilHolding();
        final firstHold = backend.hold!;
        backend.hold = Completer<void>();
        firstHold.complete();
        await Future.wait([first, second]);
        await backend.untilHolding();
        // The first was saved as layer-1; the second, not yet, is kept as
        // that layer's next revision, never as a page's first drawing.
        expect(backend.layers['layer-1']!.revision, 1);
        await sync.journalWritten;
        final kept = journal.kept.values.single;
        expect(
          (kept.seq, kept.inkLayerId, kept.baseRevision),
          (2, 'layer-1', 1),
        );
        backend.hold!.complete();
        backend.hold = null;
        await sync.flush();
        expect(backend.layers['layer-1']!.revision, 2);
        await sync.journalWritten;
        expect(journal.kept, isEmpty);

        // A save that lands after the view went still forgets it.
        backend.hold = Completer<void>();
        final third = draw(3, 'first, second, third', 3);
        await backend.untilHolding();
        sync.dispose();
        backend.hold!.complete();
        backend.hold = null;
        await third;
        expect(backend.layers['layer-1']!.revision, 3);
        await sync.journalWritten;
        expect(journal.kept, isEmpty);
      });

      test('the file changed meanwhile: not saved onto the new version, and '
          'said', () async {
        final again = await quitAndOpen(
          meanwhile: () {
            backend.current = _v2;
            native.source = _v2;
          },
        );
        expect(backend.saves.where((s) => s.sourceSha256 == _v2), isEmpty);
        await again.journalWritten;
        expect(journal.kept, isEmpty);
        expect(again.state.value, isA<InkOutdated>());
        expect((again.state.value as InkOutdated).pages, [3]);
      });
    });

    test('a view made again gets everything held, saved or not', () async {
      await sync.ready(controller, sourceSha256: _v1);
      await draw(3, 'saved', 1);
      backend.failNextSave = const RequestFailure('Not connected.');
      backend.connected.value = false;
      await draw(4, 'not saved', 1);
      expect(sync.state.value, isA<InkUnsaved>());
      expect((sync.state.value as InkUnsaved).reason, isA<InkOffline>());

      await sync.ready(controller, sourceSha256: _v1);
      final held = native.loaded.last;
      expect(
        {for (final l in held) l['page_1based']: l['ink_layer_id']},
        {3: 'layer-1', 4: null},
      );

      // Connected again: what wasn't saved is.
      backend.connected.value = true;
      await Future<void>.delayed(Duration.zero);
      await sync.flush();
      expect(backend.layers.values.map((l) => l.space.page1based), [3, 4]);
      expect(sync.state.value, isA<InkSaved>());
    });

    test('another device\'s save shows on a page not being drawn on', () async {
      await sync.ready(controller, sourceSha256: _v1);
      await draw(3, 'mine', 1);
      final loads = native.loaded.length;
      // Our own save's broadcast is no news.
      expect(native.loaded.length, loads);
      backend.otherDevice(3, utf8.encode('theirs'));
      await Future<void>.delayed(const Duration(milliseconds: 10));
      final layers = native.loaded.last;
      expect(native.loaded.length, loads + 1);
      expect(layers.single['revision'], 2);
      // The next change builds on theirs.
      await draw(3, 'theirs, and mine', 2);
      expect(backend.saves.last.baseRevision, 2);
    });

    test('a new version: unchanged pages\' ink is carried onto it, changed '
        'pages\' stays with the earlier one', () async {
      await sync.ready(controller, sourceSha256: _v1);
      await draw(3, 'on page 3', 1);
      await draw(5, 'on page 5', 2);
      backend.current = _v2;
      native.source = _v2;
      final reload = Map<String, Object?>.from(
        _read('bridge/cmd_reload_document.json')['result'] as Map,
      );
      reload['kept_layers'] = [
        {'page_1based': 3, 'ink_layer_id': 'layer-1', 'revision': 1},
      ];
      reload['outdated_layers'] = [
        {'page_1based': 5, 'ink_layer_id': 'layer-2', 'revision': 1},
      ];
      final saves = backend.saves.length;
      await sync.reloaded(InkReload(reload));
      final carried = backend.saves.sublist(saves);
      expect(carried.single.space.page1based, 3);
      expect(
        (carried.single.inkLayerId, carried.single.sourceSha256),
        ('', _v2),
      );
      expect(native.told.last['ink_layer_id'], 'layer-3');
      expect(sync.state.value, isA<InkOutdated>());
      expect((sync.state.value as InkOutdated).pages, [5]);
    });

    test('a new version names only changed pages with strokes, never one '
        'erased (#368 QA)', () async {
      await sync.ready(controller, sourceSha256: _v1);
      await draw(4, 'on page 4', 1);
      await draw(5, 'on page 5', 2);
      // Page 4 erased: its layer stays, empty.
      native.drawn[4] = (const <int>[], 3);
      await sync.changed(controller, _change(4, const [], 3, strokes: 0));
      expect(sync.inkedPages, [5]);
      backend.current = _v2;
      native.source = _v2;
      final reload = Map<String, Object?>.from(
        _read('bridge/cmd_reload_document.json')['result'] as Map,
      );
      reload['kept_layers'] = [];
      reload['outdated_layers'] = [
        {'page_1based': 4, 'ink_layer_id': 'layer-1', 'revision': 2},
        {'page_1based': 5, 'ink_layer_id': 'layer-2', 'revision': 1},
      ];
      await sync.reloaded(InkReload(reload));
      expect(sync.state.value, isA<InkOutdated>());
      expect((sync.state.value as InkOutdated).pages, [5]);
      expect(sync.layerOf(4), (inkLayerId: null, revision: null));

      // Drawing on page 4 again starts a layer of the new version.
      await draw(4, 'of v2', 4);
      expect(backend.saves.last.inkLayerId, isEmpty);
      expect(backend.saves.last.sourceSha256, _v2);
      expect((sync.state.value as InkOutdated).pages, [5]);
    });

    test('opening names only the earlier version\'s pages with strokes, '
        'never one erased there (#368 QA)', () async {
      final blank = await _png(ink: false);
      final inked = await _png(ink: true);
      // Page 3 drawn, then erased, on the earlier version.
      backend.otherDevice(
        3,
        utf8.encode('a line'),
        source: _v2,
        preview: inked,
      );
      backend.otherDevice(
        3,
        utf8.encode('erased'),
        source: _v2,
        preview: blank,
      );
      backend.otherDevice(
        4,
        utf8.encode('circle'),
        source: _v2,
        preview: inked,
      );
      await sync.ready(controller, sourceSha256: _v1);
      expect(sync.state.value, isA<InkOutdated>());
      expect((sync.state.value as InkOutdated).pages, [4]);
    });

    test('drawing again on a page whose ink was set aside starts a layer of '
        'the new version', () async {
      final before = backend.otherDevice(1, utf8.encode('of v1'));
      await sync.ready(controller, sourceSha256: _v1);
      expect(sync.layerOf(1), (inkLayerId: before.id, revision: 1));
      backend.current = _v2;
      native.source = _v2;
      final reload = Map<String, Object?>.from(
        _read('bridge/cmd_reload_document.json')['result'] as Map,
      );
      reload['kept_layers'] = [];
      reload['outdated_layers'] = [
        {'page_1based': 1, 'ink_layer_id': before.id, 'revision': 1},
      ];
      await sync.reloaded(InkReload(reload));
      expect(sync.state.value, isA<InkOutdated>());
      expect(sync.layerOf(1), (inkLayerId: null, revision: null));

      // The first stroke with the backend away says so.
      backend.failNextSave = const RequestFailure('Not connected.');
      backend.connected.value = false;
      await draw(1, 'of v2', 1);
      expect(sync.state.value, isA<InkUnsaved>());
      expect((sync.state.value as InkUnsaved).reason, isA<InkOffline>());

      backend.connected.value = true;
      await Future<void>.delayed(Duration.zero);
      await sync.flush();
      final save = backend.saves.last;
      expect(
        (save.inkLayerId, save.baseRevision, save.sourceSha256),
        ('', 0, _v2),
      );
      expect(sync.state.value, isA<InkSaved>());
      // The earlier version's layer is as it was.
      expect(backend.layers[before.id]!.revision, 1);
      expect(backend.layers[before.id]!.sourceSha256, _v1);
      expect(sync.layerOf(1).inkLayerId, isNot(before.id));
    });

    test('a file changed back to a version with ink of its own shows that '
        'ink, never a false conflict', () async {
      Map<String, Object?> reload(
        String sha, {
        List<int> kept = const [],
        List<int> outdated = const [],
      }) {
        final r = Map<String, Object?>.from(
          _read('bridge/cmd_reload_document.json')['result'] as Map,
        );
        r['source_sha256'] = sha;
        r['kept_layers'] = [
          for (final p in kept) {'page_1based': p, 'ink_layer_id': null},
        ];
        r['outdated_layers'] = [
          for (final p in outdated) {'page_1based': p, 'ink_layer_id': null},
        ];
        return r;
      }

      // v1 has ink on pages 1 and 2; v2 (page 1's text changed) on page 2.
      final a = backend.otherDevice(1, utf8.encode('v1 p1'), source: _v1);
      final b = backend.otherDevice(2, utf8.encode('v1 p2'), source: _v1);
      final c = backend.otherDevice(2, utf8.encode('v2 p2'), source: _v2);
      expect({a.id, b.id, c.id}, hasLength(3));
      await sync.ready(controller, sourceSha256: _v1);
      expect(sync.inkedPages, [1, 2]);

      // To v2: page 1's ink is the earlier version's, page 2 shows v2's.
      backend.current = _v2;
      native.source = _v2;
      await sync.reloaded(InkReload(reload(_v2, kept: [2], outdated: [1])));
      expect(backend.saves, isEmpty);
      final two = native.loaded.last.single;
      expect((two['page_1based'], two['ink_layer_id']), (2, c.id));
      expect(sync.layerOf(2), (inkLayerId: c.id, revision: 1));
      expect(sync.state.value, isA<InkOutdated>());

      // Back to v1: its own ink on both pages, as on opening it.
      backend.current = _v1;
      native.source = _v1;
      await sync.reloaded(InkReload(reload(_v1, kept: [2], outdated: [])));
      expect(backend.saves, isEmpty);
      expect(
        {
          for (final l in native.loaded.last)
            l['page_1based']: l['ink_layer_id'],
        },
        {1: a.id, 2: b.id},
      );
      expect(sync.inkedPages, [1, 2]);
      expect(sync.layerOf(1), (inkLayerId: a.id, revision: 1));
      expect(sync.state.value, isA<InkSaved>());

      // Drawing there now is that layer's next revision.
      await draw(1, 'v1 p1, more', 9);
      expect(
        (backend.saves.last.inkLayerId, backend.saves.last.baseRevision),
        (a.id, 1),
      );
    });

    test(
      'a Claktory that can\'t keep drawings says so and sends nothing',
      () async {
        backend.old = true;
        await sync.ready(controller, sourceSha256: _v1);
        await draw(3, 'a circle', 1);
        expect(backend.starts, isEmpty);
        expect(sync.state.value, isA<InkUnsaved>());
        expect((sync.state.value as InkUnsaved).reason, isA<InkUnsupported>());
      },
    );

    test('answers that don\'t add up are a failure, never words', () async {
      await sync.ready(controller, sourceSha256: _v1);
      backend.lieAboutChunks = true;
      await draw(5, 'a line', 1);
      final state = sync.state.value as InkUnsaved;
      expect(state.reason, isA<InkFailed>());
      // Tried again on the next change.
      backend.lieAboutChunks = false;
      await draw(5, 'a line, and more', 2);
      expect(sync.state.value, isA<InkSaved>());
    });

    test('refusals say why in the backend\'s words', () async {
      await sync.ready(controller, sourceSha256: _v1);
      backend.current = _v2;
      await draw(3, 'a circle', 1);
      final state = sync.state.value as InkUnsaved;
      expect(state.pages, [3]);
      expect(
        (state.reason as InkRefused).message,
        'This file changed since you opened it.',
      );
      native.hugeDrawing = true;
      backend.current = _v1;
      await draw(2, 'huge', 2);
      final huge = sync.state.value as InkUnsaved;
      expect(
        (huge.reason as InkRefused).message,
        "This page's drawing is too large to keep. Erase some of it to save it.",
      );
      expect(native.files.where((f) => File(f).existsSync()), isEmpty);
    });
  });
}
