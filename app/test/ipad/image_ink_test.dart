// Drawing on an image on an iPad (#362 I2): the image's pane offers Draw,
// which turns it into the document host on an image surface (one drawing,
// no pages), saved as the image's ink layer; Done turns it back into the
// image's viewer. Wherever the image shows, on any device, the ink saved
// on its version lies over it, read-only, following saves.
import 'dart:convert';
import 'dart:io';

import 'package:crypto/crypto.dart' show sha256;

import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/claktory_identity.dart';
import 'package:factory_app/ink_preview.dart';
import 'package:factory_app/ipad/document_host.dart' show documentChunk;
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/mobile/navigation.dart' show PhoneRoutesScope;
import 'package:factory_app/panes.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart'
    hide InkComposite;
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:factory_app/viewers/file_cache.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart' hide settle;
import '../panes_test.dart' show beside, leaf, row, snapshot, split;
import '../viewers_test.dart' show png;
import 'as_ipad.dart';
import 'document_native.dart';

const _path = '/w/shot.png';

ViewNode _layout() =>
    split('s1', row, [leaf('chat', 'chat'), leaf('shot', 'image', _path)]);

FileContent _file(List<int> data) => FileContent(
  path: _path,
  mime: 'image/png',
  size: Int64(data.length),
  data: data,
  sha256: 'v1',
);

InkLayer _layer({
  String id = 'layer-1',
  String source = 'v1',
  int revision = 1,
  String preview = 'preview-1',
  String kind = 'image',
  String conflictOf = '',
}) => InkLayer(
  id: id,
  path: _path,
  sourceSha256: source,
  space: InkSpace(kind: kind),
  revision: revision,
  preview: InkBlobRef(sha256: preview),
  conflictOf: conflictOf,
);

/// Lets the image and the ink's preview decode.
Future<void> _decode(WidgetTester tester) async {
  // The image first, then the ink's preview over it.
  for (var i = 0; i < 2; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 300)),
    );
    await settle(tester);
  }
}

/// The image in an agent's pane on an iPad, beside the conversation, or
/// on its own page in a window narrowed to the phone's routes.
Future<(DocumentNative, Harness, WorkbenchFocus)> _open(
  WidgetTester tester, {
  Size size = const Size(1366, 1024),
  bool phoneRoutes = false,
  bool asking = false,
  void Function(Harness harness)? before,
}) async {
  asIPad();
  final native = DocumentNative();
  final data = await png(tester, 400, 200);
  final workspace = snapshot(layout: _layout());
  final harness = Harness(workspace);
  harness.backend.files[_path] = _file(data);
  if (asking) {
    workspace.agents
        .firstWhere((agent) => agent.id == 'a1')
        .waitingSeqs
        .add(Int64(7));
    harness.backend.entriesById['a1/7'] = Entry(
      seq: Int64(7),
      askPerson: AskPerson(
        id: 'q1',
        title: 'Circle what looks wrong',
        state: AskPersonState.ASK_PERSON_STATE_PENDING,
        reviewPath: _path,
      ),
    );
  }
  before?.call(harness);
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  if (phoneRoutes) {
    await harness.show(
      tester,
      size: size,
      home: WorkbenchScope(focus: focus, child: const Shell()),
    );
    await settle(tester);
    tester
        .widget<PhoneRoutesScope>(find.byType(PhoneRoutesScope))
        .routes
        .showFile('a1', _path);
  } else {
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
                  chat: const SizedBox.expand(key: ValueKey('conversation')),
                ),
              ),
            );
          },
        ),
      ),
    );
    await tester.pump();
  }
  await _decode(tester);
  return (native, harness, focus);
}

final _draw = find.byKey(const ValueKey('image-draw'));
final _done = find.byKey(const ValueKey('document-done'));
final _preview = find.byKey(const ValueKey('ink-preview'));

/// Native says something, as the newest view.
Future<void> _native(
  DocumentNative native,
  String method,
  Map<String, Object?> arguments,
) => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
    .handlePlatformMessage(
      'claktory/ink/${native.views.last}',
      const StandardMethodCodec().encodeMethodCall(
        MethodCall(method, arguments),
      ),
      (_) {},
    );

Future<void> _ready(DocumentNative native) => _native(native, 'ready', {
  'surface': 'image',
  'source_sha256': 'v1',
  'sha256_verified': true,
  'page_count': 1,
  'pages': [
    {'page_1based': null, 'width': 400.0, 'height': 200.0},
  ],
  'drawing_space': 'oriented_image',
});

/// The person drew [strokes] strokes on the image.
Future<void> _drew(DocumentNative native, int strokes) {
  native.drawnSeq[null] = strokes;
  return _native(native, 'drawingChanged', {
    'page_1based': null,
    'ink_layer_id': null,
    'base_revision': null,
    'local_seq': strokes,
    'cause': 'stroke',
    'stroke_count': strokes,
    'drawing': {
      'format': 'pencilkit',
      'content_version': 1,
      'bytes_base64': base64Encode([1, 2, 3]),
      'sha256': 'd',
    },
  });
}

String _status(WidgetTester tester) => tester
    .widget<Text>(
      find
          .descendant(
            of: find.byKey(const ValueKey('document-status')),
            matching: find.byType(Text),
          )
          .first,
    )
    .data!;

void main() {
  test('the layer shown is the version\'s own, at its latest revision', () {
    expect(shownInkLayer([], 'v1'), isNull);
    final shown = shownInkLayer([
      _layer(id: 'old', source: 'v0', revision: 9),
      _layer(id: 'a', revision: 2, preview: 'p2'),
      _layer(id: 'a', revision: 3, preview: 'p3'),
      _layer(id: 'beside', revision: 7, conflictOf: 'a'),
      _layer(id: 'page', revision: 8, kind: 'pdf_page'),
      _layer(id: 'none', revision: 9, preview: ''),
    ], 'v1');
    expect(shown?.preview.sha256, 'p3');
  });

  testWidgets('the ink saved on an image lies over it, at its size, and '
      'follows saves from any device', (tester) async {
    // Blobs are named by their SHA-256, and checked against it.
    final first = await png(tester, 400, 200);
    final next = await png(tester, 200, 100);
    final p1 = sha256.convert(first).toString();
    final p2 = sha256.convert(next).toString();
    final harness = Harness(snapshot(layout: beside('image', _path)));
    final backend = harness.backend
      ..files[_path] = _file(await png(tester, 400, 200))
      ..inkLayers[_path] = [_layer(preview: p1)]
      ..inkBlobs[p1] = first
      ..inkBlobs[p2] = next;
    await harness.show(tester);
    await _decode(tester);

    expect(_preview, findsOneWidget);
    final image = find.byWidgetPredicate(
      (w) => w is RawImage && w.key != const ValueKey('ink-preview'),
    );
    expect(tester.getRect(_preview), tester.getRect(image));
    // It takes no touches: the image's own gestures are as they were.
    expect(
      find.ancestor(of: _preview, matching: find.byType(IgnorePointer)),
      findsWidgets,
    );

    // Saved again on another device.
    backend.inkLayerLists.value = InkLayerList(
      path: _path,
      layers: [_layer(revision: 2, preview: p2)],
    );
    await _decode(tester);
    expect(tester.widget<RawImage>(_preview).image!.width, 200);

    // Only an earlier version's: nothing over this one.
    backend.inkLayerLists.value = InkLayerList(
      path: _path,
      layers: [_layer(source: 'v0', revision: 3, preview: p1)],
    );
    await _decode(tester);
    expect(_preview, findsNothing);
  });

  testWidgets('a preview that can\'t be read shows nothing', (tester) async {
    final harness = Harness(snapshot(layout: beside('image', _path)));
    harness.backend
      ..files[_path] = _file(await png(tester, 400, 200))
      ..inkLayers[_path] = [_layer(preview: 'gone')];
    await harness.show(tester);
    await _decode(tester);
    expect(_preview, findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('not on a Mac or a phone: no Draw', (tester) async {
    final harness = Harness(snapshot(layout: beside('image', _path)));
    harness.backend.files[_path] = _file(await png(tester, 400, 200));
    await harness.show(tester);
    await _decode(tester);
    expect(_draw, findsNothing);
  });

  testWidgets('Draw turns the pane into a drawing surface on the image, '
      'saved as its ink layer; Done turns it back', (tester) async {
    final (native, harness, _) = await _open(tester);
    expect(find.text('Draw'), findsOneWidget);
    await tester.tap(_draw);
    await settle(tester);

    final params = native.created.last;
    expect(params['surface'], 'image');
    expect((params['source'] as Map)['path'], _path);
    expect((params['source'] as Map)['sha256'], 'v1');
    await _ready(native);
    await settle(tester);
    // No pages: no search, no contents, no page count.
    expect(find.byKey(const ValueKey('document-search')), findsNothing);
    expect(find.textContaining('Page '), findsNothing);
    expect(_done, findsOneWidget);

    await _drew(native, 2);
    await settle(tester);
    expect(_status(tester), 'Saved to ${claktoryName(harness.backend)}');
    final save = native.ink.saves.single;
    expect((save.path, save.sourceSha256), (_path, 'v1'));

    await tester.tap(_done);
    await settle(tester);
    await _decode(tester);
    expect(_done, findsNothing);
    expect(_draw, findsOneWidget);
    // The surface's last save was asked for as its view went.
    expect(
      native.calls.where((c) => c.method == 'setActive').length,
      greaterThanOrEqualTo(1),
    );
  });

  testWidgets('from a Claktory on another machine, Draw opens: the image '
      "kept with the device's files, its version from the next byte's read, "
      'since a part of an image read from its start has no hash (row 466)', (
    tester,
  ) async {
    final folder = Directory.systemTemp.createTempSync('kept-ipad-image');
    keepsFilesOf = (_) => true;
    fileCacheFolder = () async => folder.path;
    addTearDown(() {
      keepsFilesOf = (backend) => backend.target != null;
      FileCache.forgetAllInMemory();
      folder.deleteSync(recursive: true);
    });
    Future<void> working() async {
      for (var i = 0; i < 30; i++) {
        await tester.runAsync(
          () => Future<void>.delayed(const Duration(milliseconds: 20)),
        );
        await tester.pump();
      }
    }

    final (native, harness, _) = await _open(
      tester,
      before: (harness) => harness.backend.partsUnhashed = true,
    );
    // The image itself is kept too (the viewer's), real work on the disk;
    // cleared, as when the viewer kept only a preview (Zucchero's row 468),
    // so Draw reads and keeps the original.
    await working();
    await _decode(tester);
    await tester.runAsync(() => FileCache.of(harness.backend)!.clear());
    final before = harness.backend.sent.length;
    await tester.tap(_draw);
    await working();
    expect(find.textContaining('to draw on images'), findsNothing);
    final params = native.created.last;
    expect(params['surface'], 'image');
    expect((params['source'] as Map)['sha256'], 'v1');
    final file = (params['document'] as Map)['file'] as String;
    expect(file, startsWith(folder.path));
    expect(File(file).readAsBytesSync(), harness.backend.files[_path]!.data);
    // Kept, under its version: Settings → Storage counts it.
    final kept = await tester.runAsync(
      () => FileCache.of(harness.backend)!.find(_path, originalVariant),
    );
    expect((kept?.sha256, kept?.file.path), ('v1', file));
    final reads = [
      for (final message in harness.backend.sent.skip(before))
        if (message.hasReadFile() && message.readFile.path == _path)
          message.readFile,
    ];
    expect(
      [
        for (final read in reads.take(2))
          (read.offset.toInt(), read.maxBytes.toInt()),
      ],
      [(0, 1), (1, 1)],
    );
  });

  testWidgets('a backend from before ranged reads still says to update it', (
    tester,
  ) async {
    final (_, harness, _) = await _open(
      tester,
      before: (harness) => harness.backend
        ..partsUnhashed = true
        ..rangedReads = false,
    );
    documentChunk = 8;
    addTearDown(() => documentChunk = 8 * 1024 * 1024);
    await tester.tap(_draw);
    await settle(tester);
    expect(
      find.text(
        'Update Claktory on ${claktoryName(harness.backend)} to draw on '
        'images.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('drawn on an image offline, it says it saves once the '
      'Claktory is back', (tester) async {
    final (native, harness, _) = await _open(tester);
    final name = claktoryName(harness.backend);
    await tester.tap(_draw);
    await settle(tester);
    await _ready(native);
    native.ink
      ..failNextSave = const RequestFailure('Not connected to the backend.')
      ..connected.value = false;
    await _drew(native, 1);
    await settle(tester);
    expect(
      _status(tester),
      'Not connected to $name. Your drawing is kept on this iPad, and saves '
      'once $name is back and you open this image again.',
    );
    native.ink.connected.value = true;
    await settle(tester);
    expect(_status(tester), 'Saved to $name');
  });

  testWidgets('a drawing on an image goes with a review\'s answer, whole', (
    tester,
  ) async {
    final (native, harness, _) = await _open(tester, asking: true);
    await tester.tap(_draw);
    await settle(tester);
    await _ready(native);
    await _drew(native, 1);
    await settle(tester);
    expect(find.text('Your drawing goes with it.'), findsOneWidget);

    await tester.enterText(
      find.descendant(
        of: find.byKey(const ValueKey('review-answer')),
        matching: find.byType(EditableText),
      ),
      'The sky',
    );
    await tester.pump();
    await tester.tap(find.text('Submit'));
    await settle(tester);
    final export = native.calls.lastWhere((c) => c.method == 'exportComposite');
    expect((export.arguments as Map)['page_1based'], isNull);
    expect((export.arguments as Map)['region'], {
      'x': 0,
      'y': 0,
      'w': 1,
      'h': 1,
    });
    final answer = sentOf(harness, (m) => m.hasAnswerAsk()).single.answerAsk;
    expect([for (final a in answer.attachments) a.name], ['shot-drawing.png']);
    // Sent with Submit: a reply, the drawing with it (#370).
    expect(answer.reply, isTrue);
  });

  testWidgets('the image changing while drawn on keeps the version drawn on, '
      'and says so', (tester) async {
    final (native, harness, _) = await _open(tester);
    await tester.tap(_draw);
    await settle(tester);
    await _ready(native);
    await settle(tester);
    final changed = await png(tester, 300, 300);
    harness.backend
      ..files[_path] = FileContent(
        path: _path,
        mime: 'image/png',
        size: Int64(changed.length),
        data: changed,
        sha256: 'v2',
      )
      ..fileChanged.value = FileChanged(path: _path, sha256: 'v2');
    await settle(tester);
    expect(
      _status(tester),
      "This image changed. You're drawing on the earlier version. Tap Done "
      'to see the new one.',
    );
    expect(native.calls.where((c) => c.method == 'reloadDocument'), isEmpty);
  });

  testWidgets('a narrow window\'s image page offers Draw too, and Done '
      'brings the image back', (tester) async {
    final (native, _, _) = await _open(
      tester,
      size: const Size(390, 844),
      phoneRoutes: true,
    );
    expect(tester.takeException(), isNull);
    await tester.tap(find.text('Draw'));
    await settle(tester);
    expect(native.created.last['surface'], 'image');
    await tester.tap(find.text('Done'));
    await settle(tester);
    await _decode(tester);
    expect(find.text('Draw'), findsOneWidget);
  });

  testWidgets('drawing on an image in a focused window, the switch still '
      'goes to the conversation and back to the drawing', (tester) async {
    final (native, _, focus) = await _open(tester, size: const Size(744, 1133));
    await tester.tap(find.byKey(const ValueKey('workbench-switch-document')));
    await settle(tester);
    await _decode(tester);
    await tester.tap(_draw);
    await settle(tester);
    expect(_done, findsOneWidget);
    final views = native.views.length;

    await tester.tap(
      find.byKey(const ValueKey('workbench-switch-conversation')),
    );
    await settle(tester);
    expect(find.byKey(const ValueKey('conversation')), findsOneWidget);
    expect(_done, findsNothing);

    await tester.tap(find.byKey(const ValueKey('workbench-switch-document')));
    await settle(tester);
    // Still drawing, on a view made again.
    expect(_done, findsOneWidget);
    expect(native.views.length, views + 1);
    expect(focus.projectionOf('a1')!.hidden, ['chat']);
  });
}
