// A PDF in an iPad's document pane, for widget tests: the backend has the
// file (and a question about it, when asked), and DocumentNative stands in
// for the render cache and the native ink views, answering their commands
// and sending their events.
import 'dart:async';
import 'dart:convert';
import 'dart:io' show File;

import 'package:crypto/crypto.dart';
import 'package:factory_app/backend.dart';
import 'package:factory_app/ipad/document_host.dart' show newInkSync;
import 'package:factory_app/ipad/document_review.dart';
import 'package:factory_app/ipad/ink_bridge.dart';
import 'package:factory_app/ipad/ink_sync.dart';
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/mobile/navigation.dart' show PhoneRoutesScope;
import 'package:factory_app/panes.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart'
    hide InkComposite;
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../panes_test.dart' show leaf, row, snapshot, split;

const documentPath = '/w/paper.pdf';
const _codec = StandardMethodCodec();

FileContent documentPdf([String text = 'a paper on curlews']) {
  final bytes = utf8.encode('%PDF-1.7\n$text');
  return FileContent(
    path: documentPath,
    mime: 'application/pdf',
    size: Int64(bytes.length),
    data: bytes,
    sha256: sha256.convert(bytes).toString(),
  );
}

ViewNode _layout() => split('s1', row, [
  leaf('chat', 'chat'),
  leaf('paper', 'pdf', documentPath),
]);

Entry _ask({String path = documentPath}) => Entry(
  seq: Int64(7),
  askPerson: AskPerson(
    id: 'q1',
    title: 'Check the figures on page 3',
    body: 'The totals look off.',
    state: AskPersonState.ASK_PERSON_STATE_PENDING,
    reviewPath: path,
  ),
);

/// What native names every page's exported drawing and preview: the
/// test's blobs take them as they are ([_Blobs]).
const drawingSha = 'drawing-sha';
const previewSha = 'preview-sha';

/// The backend's ink layers, in memory, as contract §4 keeps them:
/// saved with compare-and-set, listed by the file's version.
class InkStore extends RecordingBackend {
  /// Every layer at its latest revision, by id.
  final layers = <String, InkLayer>{};
  final saves = <SaveInkLayer>[];

  /// The drawing each layer's blob holds, by its SHA-256.
  final drawings = <String, Uint8List>{};

  /// A backend from before ink layers: every ink request is unknown.
  bool old = false;

  /// The next save fails so.
  RequestFailure? failNextSave;

  /// Holds every save until [release]: a save under way.
  Completer<void>? hold;
  void release() {
    hold?.complete();
    hold = null;
  }

  var _next = 1;

  void _ink() {
    if (old) {
      throw const RequestFailure(
        'unknown message',
        code: ErrorCode.ERROR_CODE_UNKNOWN_MESSAGE,
      );
    }
  }

  /// Not connected: nothing reaches the Claktory.
  void _online() {
    if (!connected.value) {
      throw const RequestFailure('Not connected to the backend.');
    }
  }

  @override
  Future<InkLayerList> listInkLayers(String path) async {
    _ink();
    _online();
    return InkLayerList(path: path, layers: layers.values);
  }

  /// Another device saved [page] (of the version [source]) first.
  InkLayer otherDevice(int page, {required String source}) {
    final bytes = Uint8List.fromList([9, page]);
    final sha = sha256.convert(bytes).toString();
    drawings[sha] = bytes;
    final layer = InkLayer(
      id: 'theirs-$page',
      path: documentPath,
      sourceSha256: source,
      space: InkSpace(kind: 'pdf_page', page1based: page),
      revision: 1,
      drawing: InkBlobRef(format: 'pencilkit', contentVersion: 1, sha256: sha),
    );
    layers[layer.id] = layer;
    return layer;
  }

  @override
  Future<InkLayerSaved> saveInkLayer(SaveInkLayer save) async {
    _ink();
    if (hold case final waiting?) await waiting.future;
    if (failNextSave case final failure?) {
      failNextSave = null;
      throw failure;
    }
    _online();
    saves.add(save);
    final page = save.space.page1based;
    final had = save.inkLayerId.isEmpty
        ? layers.values
              .where(
                (l) =>
                    l.space.page1based == page &&
                    l.sourceSha256 == save.sourceSha256 &&
                    l.conflictOf.isEmpty,
              )
              .firstOrNull
        : layers[save.inkLayerId];
    if (save.conflictOf.isEmpty &&
        had != null &&
        (save.inkLayerId.isEmpty || had.revision != save.baseRevision)) {
      return InkLayerSaved(clientSaveId: save.clientSaveId, conflict: had);
    }
    final id = save.inkLayerId.isEmpty ? 'layer-${_next++}' : save.inkLayerId;
    final revision = save.inkLayerId.isEmpty ? 1 : save.baseRevision + 1;
    layers[id] = InkLayer(
      id: id,
      path: documentPath,
      sourceSha256: save.sourceSha256,
      space: save.space,
      revision: revision,
      drawing: InkBlobRef(contentVersion: 1, sha256: save.drawingSha256),
      conflictOf: save.conflictOf,
    );
    return InkLayerSaved(
      clientSaveId: save.clientSaveId,
      inkLayerId: id,
      revision: revision,
    );
  }
}

/// Blobs as the store has them: an upload is taken as native named it
/// (no temporary files in a widget test), a read gives the drawing kept.
class _Blobs extends InkBlobs {
  _Blobs(InkStore super.backend);

  InkStore get store => backend as InkStore;

  @override
  Future<String> upload(
    File file, {
    required String mime,
    required String name,
  }) async => mime == 'image/png' ? previewSha : drawingSha;

  @override
  Future<Uint8List> read(String sha256) async =>
      store.drawings[sha256] ?? Uint8List.fromList(const [1, 2, 3]);
}

TestDefaultBinaryMessenger get _messenger =>
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

/// The render cache and the native views: the file is kept as it comes,
/// and each view answers its commands, exporting a page as a PNG named
/// after it.
class DocumentNative {
  DocumentNative() {
    InkRenderCache.channel = const MethodChannel('test/review-cache');
    _messenger.setMockMethodCallHandler(InkRenderCache.channel, (call) async {
      return switch (call.method) {
        'cacheLookup' => <String, Object?>{'file': null},
        'cacheBegin' => {'token': 't'},
        'cacheFinish' => {'file': '/cache/paper.pdf'},
        _ => null,
      };
    });
    _messenger.setMockMethodCallHandler(SystemChannels.platform_views, (
      call,
    ) async {
      if (call.method == 'create') {
        final id = (call.arguments as Map)['id'] as int;
        views.add(id);
        if ((call.arguments as Map)['params'] case final Uint8List bytes) {
          created.add(
            (const StandardMessageCodec().decodeMessage(
              ByteData.sublistView(bytes),
            ) as Map).cast<String, Object?>(),
          );
        }
        _messenger.setMockMethodCallHandler(MethodChannel('claktory/ink/$id'), (
          call,
        ) async {
          calls.add(call);
          if (call.method == 'reloadDocument') {
            shownSha =
                ((call.arguments as Map)['source'] as Map)['sha256'] as String;
            return reloadWith((call.arguments as Map).cast<String, Object?>());
          }
          if (call.method == 'search') {
            return searchFor((call.arguments as Map).cast<String, Object?>());
          }
          if (call.method == 'goToPage') {
            return {'page_1based': (call.arguments as Map)['page_1based']};
          }
          if (call.method == 'recognizeText') {
            return recognize((call.arguments as Map).cast<String, Object?>());
          }
          if (call.method == 'exportLayer') {
            final page = (call.arguments as Map)['page_1based'] as int?;
            return {
              'page_1based': page,
              'ink_layer_id': null,
              'base_revision': null,
              'local_seq': drawnSeq[page],
              'source_sha256': shownSha,
              'stroke_count': drawnSeq[page] ?? 0,
              'coordinate_space': {
                'kind': 'pdf_page',
                'page_1based': page,
                'box': 'crop',
                'crop_box': [0, 0, 612, 792],
                'media_box': [0, 0, 612, 792],
                'rotation': 0,
                'width': 612,
                'height': 792,
                'units': 'pt',
              },
              'drawing': {
                'format': 'pencilkit',
                'content_version': 1,
                'space': 'page_overlay',
                'drawing_to_document': [1, 0, 0, -1, 0, 792],
                'file': '',
                'sha256': drawingSha,
              },
              'ink_preview': {'file': '', 'sha256': previewSha},
            };
          }
          if (call.method == 'loadDrawing') {
            final layers = ((call.arguments as Map)['layers'] as List)
                .cast<Map>();
            // Keep both: native puts theirs under the person's and sends
            // the two as the page's change, after answering.
            for (final l in layers) {
              if (l['keep_mine'] != true) continue;
              final page = l['page_1based'] as int;
              drawnSeq[page] = (drawnSeq[page] ?? 0) + 1;
              Future(
                () => _drew(
                  page,
                  drawnSeq[page]!,
                  layerId: l['ink_layer_id'] as String?,
                  revision: l['revision'] as int?,
                  cause: 'keep_both',
                ),
              );
            }
            return {
              'loaded': [
                for (final l in layers)
                  {
                    'page_1based': l['page_1based'],
                    'ink_layer_id': l['ink_layer_id'],
                    'revision': l['revision'],
                    'strokes': 2,
                  },
              ],
              'unreadable': <Object?>[],
            };
          }
          if (call.method == 'layerSaved') return <String, Object?>{};
          if (call.method == 'exportComposite') {
            final page = (call.arguments as Map)['page_1based'];
            return {
              'file': '/tmp/page-$page.png',
              'mime': 'image/png',
              'pixel_width': 1536,
              'pixel_height': 2048,
            };
          }
          return null;
        });
      }
      return null;
    });
    readComposite = (file) async =>
        Uint8List.fromList(utf8.encode('png of $file'));
    newInkSync = ({required backend, required path, required machine}) =>
        InkLayerSync(
          backend: ink,
          path: path,
          machine: machine,
          blobs: _Blobs(ink),
          // Nothing kept on this device unless a test gives it a journal
          // (ink_sync_test covers the journal itself).
          journal: journal,
        );
    addTearDown(() {
      newInkSync = InkLayerSync.new;
      _messenger
        ..setMockMethodCallHandler(InkRenderCache.channel, null)
        ..setMockMethodCallHandler(SystemChannels.platform_views, null);
      InkRenderCache.channel = const MethodChannel('claktory/ink');
      readComposite = (file) => throw UnimplementedError();
    });
  }

  /// The native views made, the newest last: a PDF closed and opened
  /// again makes another.
  final views = <int>[];

  /// What the device keeps of unsaved drawings: nothing unless a test
  /// gives it [MemoryJournal].
  InkJournal? journal;
  final calls = <MethodCall>[];

  /// The backend's ink layers.
  final ink = InkStore();

  /// The version native shows: as created, then as reloaded.
  String get shownSha =>
      _shownSha ??
      ((created.lastOrNull?['source'] as Map?)?['sha256'] as String? ?? '');
  set shownSha(String sha) => _shownSha = sha;
  String? _shownSha;

  /// Each page's latest drawingChanged (its local_seq).
  final drawnSeq = <int?, int>{};

  /// Each view's creation params, as made.
  final created = <Map<String, Object?>>[];

  /// What native reads of the handwriting [recognizeText] asks for.
  FutureOr<Map<String, Object?>> Function(Map<String, Object?> arguments)
  recognize = (arguments) => {
    'available': true,
    'stale': false,
    'page_1based': arguments['page_1based'],
    'text': 'why page ${arguments['page_1based']}?',
  };

  /// What native answers a new version with: by default shown at page 1
  /// of 5, every page the same.
  FutureOr<Map<String, Object?>> Function(Map<String, Object?> arguments)
  reloadWith = (arguments) => {
    'reloaded': true,
    'source_sha256': (arguments['source'] as Map)['sha256'],
    'previous_sha256': 'before',
    'page_count': 5,
    'pages': <Object?>[],
    'page_1based': 1,
    'pages_changed': <Object?>[],
    'pages_added': <Object?>[],
    'pages_removed': <Object?>[],
    'comparison': 'content_then_render',
    'kept_layers': <Object?>[],
    'outdated_layers': <Object?>[],
  };

  /// What native finds for a search: by default a match on pages 2 and
  /// 4 for anything but "nothing".
  FutureOr<Map<String, Object?>> Function(Map<String, Object?> arguments)
  searchFor = (arguments) {
    final query = arguments['query'] as String;
    return {
      'query': query,
      'truncated': false,
      'matches': [
        if (query.isNotEmpty && query != 'nothing')
          for (final page in [2, 4])
            {
              'page_1based': page,
              'region': {'x': 0.1, 'y': 0.2 * page, 'w': 0.2, 'h': 0.02},
              'text': 'Curlews nest on page $page, near the $query.',
            },
      ],
    };
  };

  Future<void> _send(String method, Map<String, Object?> arguments) =>
      _messenger.handlePlatformMessage(
        'claktory/ink/${views.last}',
        _codec.encodeMethodCall(MethodCall(method, arguments)),
        (_) {},
      );

  /// The view is ready, on an iPad that reads handwriting or doesn't
  /// ([handwriting] null: a native too old to say).
  Future<void> ready({
    bool? handwriting,
    List<Map<String, Object?>> outline = const [],
  }) => _send('ready', {
    'outline': outline,
    'surface': 'pdf',
    'page_count': 5,
    'pages': <Object?>[],
    'drawing_space': 'page_overlay',
    if (handwriting != null)
      'handwriting': {
        'available': handwriting,
        'min_os': '27.0',
        if (handwriting) 'supported_languages': ['en'],
      },
  });

  /// The person turned to [page] (of 5).
  Future<void> turned(int page) =>
      _send('pageChanged', {'page_1based': page, 'page_count': 5});

  /// The person chose [strokes] strokes on [page] with the lasso.
  Future<void> lasso(int page, int strokes) => _send('strokeSelectionChanged', {
    'page_1based': page,
    'stroke_count': strokes,
    if (strokes > 0) 'bounds': {'x': 0.1, 'y': 0.1, 'w': 0.2, 'h': 0.1},
  });

  /// Native says a stroke was cut short for [reason].
  Future<void> cut(String reason) => _messenger.handlePlatformMessage(
    'claktory/ink/${views.last}',
    _codec.encodeMethodCall(
      MethodCall('strokeInterrupted', {
        'page_1based': 1,
        'reason': reason,
        'kept': false,
      }),
    ),
    (_) {},
  );

  /// Native says what's drawn on [page].
  Future<void> drew(int page, int strokes) {
    drawnSeq[page] = strokes;
    return _drew(page, strokes);
  }

  Future<void> _drew(
    int page,
    int strokes, {
    String? layerId,
    int? revision,
    String cause = 'stroke',
  }) => _messenger.handlePlatformMessage(
    'claktory/ink/${views.last}',
    _codec.encodeMethodCall(
      MethodCall('drawingChanged', {
        'page_1based': page,
        'ink_layer_id': layerId ?? 'layer-p$page',
        'base_revision': revision ?? 1,
        'local_seq': strokes,
        'cause': cause,
        'stroke_count': strokes,
        'drawing': {
          'format': 'pencilkit',
          'content_version': 1,
          'bytes_base64': base64Encode([1, 2, 3]),
          'sha256': 'd',
        },
      }),
    ),
    (_) {},
  );
}

Future<Harness> showDocument(
  WidgetTester tester, {
  bool asking = true,
  Size size = const Size(1366, 1024),
  bool phoneRoutes = false,
  void Function(AskPerson ask)? ask,
}) async {
  final workspace = snapshot(layout: _layout());
  if (asking) {
    workspace.agents
        .firstWhere((agent) => agent.id == 'a1')
        .waitingSeqs
        .add(Int64(7));
  }
  final entry = _ask();
  ask?.call(entry.askPerson);
  final harness = Harness(workspace);
  harness.backend
    ..files[documentPath] = documentPdf()
    ..entriesById['a1/7'] = entry;
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  if (phoneRoutes) {
    // A window narrowed to the phone's routes: the whole app, the file
    // opened on its own page.
    await harness.show(
      tester,
      size: size,
      home: WorkbenchScope(focus: focus, child: const Shell()),
    );
    await settle(tester);
    tester
        .widget<PhoneRoutesScope>(find.byType(PhoneRoutesScope))
        .routes
        .showFile('a1', documentPath);
    await settle(tester);
    return harness;
  }
  await harness.show(
    tester,
    size: size,
    home: WorkbenchScope(
      focus: focus,
      child: Builder(
        builder: (context) {
          final store = AppScope.of(context).workspace;
          return ListenableBuilder(
            listenable: store,
            builder: (context, _) => Workbench(
              rail: const SizedBox(width: 40),
              sidebar: (width) => SizedBox(width: width),
              sidebarWidth: 248,
              agent: store.selectedAgent,
              content: PaneLayout(
                agent: store.selectedAgent!,
                chat: const SizedBox.expand(),
              ),
            ),
          );
        },
      ),
    ),
  );
  await settle(tester);
  return harness;
}

Future<void> settle(WidgetTester tester) async {
  for (var i = 0; i < 20; i++) {
    await tester.pump(const Duration(milliseconds: 20));
  }
}

List<ClientMessage> sentOf(Harness harness, bool Function(ClientMessage) of) =>
    harness.backend.sent.where(of).toList();

/// The device's journal of unsaved drawings, in memory: by document and
/// page, for this Claktory.
class MemoryJournal extends InkJournal {
  final kept = <String, InkJournalEntry>{};

  String _key(String document, int? page) => '$document/$page';

  @override
  Future<void> put({
    required String claktory,
    required String document,
    required InkJournalEntry entry,
  }) async => kept[_key(document, entry.page1based)] = entry;

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
