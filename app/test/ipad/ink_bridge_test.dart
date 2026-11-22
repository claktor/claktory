// The ink bridge's Dart side (lib/ipad/ink_bridge.dart) against the frozen
// fixtures in test/ipad/fixtures/bridge: what it sends is the fixtures'
// arguments, and every event the fixtures hold reads as its own kind. The
// native side reads the same files in app/ios/RunnerTests/InkTests.swift.

import 'dart:convert';
import 'dart:io';

import 'package:factory_app/ipad/ink_bridge.dart';
import 'package:factory_app/ipad/ink_surface.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

Map<String, Object?> _read(String path) =>
    jsonDecode(File('test/ipad/fixtures/$path').readAsStringSync())
        as Map<String, Object?>;

Map<String, Object?> _args(String path) =>
    _read(path)['arguments'] as Map<String, Object?>;

const _codec = StandardMethodCodec();

/// The native view's channel: answers each command with its fixture's
/// result and keeps what it was sent.
class _Native {
  _Native(this.name) {
    messenger.setMockMethodCallHandler(MethodChannel(name), (call) async {
      calls.add(call);
      return results[call.method];
    });
  }

  final String name;
  final calls = <MethodCall>[];
  final results = <String, Object?>{};

  TestDefaultBinaryMessenger get messenger =>
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

  MethodChannel get channel => MethodChannel(name);

  /// Native sends an event to Dart.
  Future<void> send(String method, Object? arguments) =>
      messenger.handlePlatformMessage(
        name,
        _codec.encodeMethodCall(MethodCall(method, arguments)),
        (_) {},
      );

  Future<void> sendFixture(String path) {
    final f = _read(path);
    return send(f['method'] as String, f['arguments']);
  }

  void answerAsFixture(String path) {
    final f = _read(path);
    results[f['method'] as String] = f['result'];
  }

  void dispose() => messenger.setMockMethodCallHandler(channel, null);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('creation params', () {
    for (final kind in ['pdf', 'image', 'excerpt', 'blank']) {
      test('create_$kind reads and writes as the fixture has it', () {
        final args = _args('bridge/create_$kind.json');
        final params = InkParams.fromJson(args);
        expect(params.surface.name, kind);
        expect(params.toJson(), equals(args));
      });
    }

    test('typed as the workbench uses them', () {
      final pdf = InkParams.fromJson(_args('bridge/create_pdf.json'));
      expect(pdf.leafId, 'leaf-7f3a');
      expect(pdf.document.file, isNotNull);
      expect(pdf.layers.map((l) => l.page1based), [3, 5, 7]);
      expect(pdf.initialPage, 3);
      expect(pdf.paper, 0xFFFFFFFF);
      final image = InkParams.fromJson(_args('bridge/create_image.json'));
      expect(image.drawingPolicy, InkDrawingPolicy.anyInput);
      expect(image.layers.single.page1based, isNull);
      final blank = InkParams.fromJson(_args('bridge/create_blank.json'));
      expect(blank.document.blank, const Size(1024, 768));
      expect(blank.source.sha256, isNull);
    });
  });

  test('colours are ARGB here and #RRGGBBAA on the wire', () {
    expect(inkHex(0xFF1A1A1A), '#1A1A1AFF');
    expect(inkHex(0x80FF0000), '#FF000080');
    expect(inkArgb('#1A1A1AFF'), 0xFF1A1A1A);
    expect(inkArgb('#FF000080'), 0x80FF0000);
    expect(inkArgb('#336699'), 0xFF336699);
    expect(inkArgb(null), isNull);
    expect(inkArgb('red'), isNull);
  });

  group('events', () {
    final kinds = <String, Type>{
      'ev_ready.json': InkReady,
      'ev_page_changed.json': InkPageChanged,
      'ev_drawing_changed.json': InkDrawingChanged,
      'ev_drawing_changed_new_layer.json': InkDrawingChanged,
      'ev_selection_changed.json': InkSelectionChanged,
      'ev_undo_state.json': InkUndoState,
      'ev_tool_changed.json': InkToolChanged,
      'ev_stroke_active.json': InkStrokeActive,
      'ev_stroke_interrupted.json': InkStrokeInterrupted,
      'ev_error_document_changed.json': InkError,
      'ev_error_drawing_unreadable.json': InkError,
      'ev_error_os_too_old.json': InkError,
    };

    test('every event fixture has its kind', () {
      final names = Directory('test/ipad/fixtures/bridge')
          .listSync()
          .map((f) => f.uri.pathSegments.last)
          .where((n) => n.startsWith('ev_'))
          .toSet();
      expect(names, kinds.keys.toSet());
      for (final entry in kinds.entries) {
        final f = _read('bridge/${entry.key}');
        final event = InkEvent.parse(f['method'] as String, f['arguments']);
        expect(event.runtimeType, entry.value, reason: entry.key);
      }
    });

    InkEvent parse(String name) {
      final f = _read('bridge/$name');
      return InkEvent.parse(f['method'] as String, f['arguments']);
    }

    test('ready: pages with their boxes and turn', () {
      final ready = parse('ev_ready.json') as InkReady;
      expect(ready.surface, InkSurfaceKind.pdf);
      expect(ready.sha256Verified, isTrue);
      expect(ready.pageCount, 12);
      expect(ready.pages, hasLength(12));
      expect(ready.pages[4].rotation, 90);
      expect(ready.pages[6].cropBox, const Rect.fromLTWH(72, 144, 432, 504));
      expect(ready.drawingSpace, 'page_overlay');
    });

    test('drawingChanged: the layer, its base and the new strokes', () {
      final change = parse('ev_drawing_changed.json') as InkDrawingChanged;
      expect(change.page1based, 3);
      expect(change.inkLayerId, 'layer-paper-p3');
      expect(change.baseRevision, 2);
      expect(change.localSeq, 7);
      expect(change.cause, 'stroke');
      expect(change.strokeCount, 5);
      expect(change.newBounds, const Rect.fromLTWH(0.15, 0.8, 0.3, 0.07));
      expect(change.drawing.format, 'pencilkit');
      expect(change.drawing.sha256, hasLength(64));
      final fresh =
          parse('ev_drawing_changed_new_layer.json') as InkDrawingChanged;
      expect(fresh.inkLayerId, isNull);
      expect(fresh.baseRevision, isNull);
    });

    test('tools, strokes, selection and errors', () {
      final tool = parse('ev_tool_changed.json') as InkToolChanged;
      expect(tool.tool, const InkTool(kind: InkToolKind.eraser, width: 12));
      expect(tool.source, 'pencil_double_tap');
      final active = parse('ev_stroke_active.json') as InkStrokeActive;
      expect((active.page1based, active.active), (3, true));
      final cut = parse('ev_stroke_interrupted.json') as InkStrokeInterrupted;
      expect((cut.page1based, cut.reason), (3, 'resize'));
      final selection =
          parse('ev_selection_changed.json') as InkSelectionChanged;
      expect(selection.text, startsWith('h_t'));
      expect(selection.region, const Rect.fromLTWH(0.14, 0.33, 0.42, 0.03));
      final error = parse('ev_error_drawing_unreadable.json') as InkError;
      expect(error.code, 'drawing_unreadable');
      expect(error.recoverable, isFalse);
      expect(error.details['max_content_version'], 2);
      expect(error.details.containsKey('code'), isFalse);
    });

    test('an event a newer native side sends is kept, not lost', () {
      final event = InkEvent.parse('somethingNew', {'a': 1});
      expect(event, isA<InkUnknownEvent>());
    });
  });

  group('controller', () {
    late _Native native;
    late InkSurfaceController controller;

    setUp(() {
      native = _Native('claktory/ink/1');
      controller = InkSurfaceController();
    });

    tearDown(() {
      controller.dispose();
      native.dispose();
    });

    test('attaches, then sends each command as its fixture', () async {
      await controller.attach(native.channel);
      expect(controller.attached, isTrue);
      expect(native.calls.single.method, 'attach');
      expect(native.calls.single.arguments, isNull);

      Future<void> check(
        String fixture,
        Future<Object?> Function() send,
        void Function(Object? result) result,
      ) async {
        native.answerAsFixture('bridge/$fixture');
        result(await send());
        final f = _read('bridge/$fixture');
        expect(native.calls.last.method, f['method'], reason: fixture);
        expect(
          native.calls.last.arguments,
          equals(f['arguments']),
          reason: fixture,
        );
      }

      await check(
        'cmd_set_tool.json',
        () => controller.setTool(
          const InkTool(kind: InkToolKind.pen, color: 0xFF1A1A1A, width: 3),
        ),
        (r) => expect(
          r,
          const InkTool(kind: InkToolKind.pen, color: 0xFF1A1A1A, width: 3),
        ),
      );
      await check(
        'cmd_set_drawing_policy.json',
        () => controller.setDrawingPolicy(InkDrawingPolicy.pencilOnly),
        (r) => expect(r, InkDrawingPolicy.pencilOnly),
      );
      await check(
        'cmd_undo.json',
        controller.undo,
        (r) => expect(((r as InkUndoState).canUndo, r.canRedo), (false, true)),
      );
      await check(
        'cmd_redo.json',
        controller.redo,
        (r) => expect(((r as InkUndoState).canUndo, r.canRedo), (true, false)),
      );
      await check(
        'cmd_load_drawing.json',
        () => controller.loadDrawing([
          const InkLayerDrawing(
            page1based: 3,
            inkLayerId: 'layer-paper-p3',
            revision: 2,
            drawing: InkDrawingData(
              contentVersion: 1,
              bytesBase64: '<PKDrawing.dataRepresentation, base64>',
              sha256: '7d2e5d40b5e85149b38abc651cf980b8e20cc9455c4295682d873926702ee511',
            ),
          ),
        ]),
        (r) {
          final load = r as InkLoadResult;
          expect(load.loaded.single['strokes'], 4);
          expect(load.unreadable, isEmpty);
        },
      );
      await check(
        'cmd_layer_saved.json',
        () => controller.layerSaved(
          page1based: 9,
          inkLayerId: 'layer-paper-p9',
          revision: 1,
        ),
        (_) {},
      );
      await check(
        'cmd_set_active.json',
        () => controller.setActive(false),
        (r) => expect(r, isTrue),
      );
      await check(
        'cmd_go_to_page.json',
        () => controller.goToPage(3),
        (r) => expect(r, 3),
      );
      await check(
        'cmd_export_composite.json',
        () => controller.exportComposite(
          const InkExportRequest(
            page1based: 3,
            inkLayerId: 'layer-paper-p3',
            layerRevision: 2,
            region: Rect.fromLTWH(0.08, 0.27, 0.58, 0.26),
          ),
        ),
        (r) {
          final c = r as InkComposite;
          expect(c.pixelWidth, 2048);
          expect(c.cropToDocumentTransform, hasLength(6));
          expect(c.region, const Rect.fromLTWH(0.08, 0.27, 0.58, 0.26));
        },
      );
    });

    test('a command before the view exists waits for it', () async {
      native.answerAsFixture('bridge/cmd_undo.json');
      final undo = controller.undo();
      await Future<void>.delayed(Duration.zero);
      expect(native.calls, isEmpty);
      await controller.attach(native.channel);
      expect((await undo).canRedo, isTrue);
      expect(native.calls.map((c) => c.method), ['attach', 'undo']);
    });

    test('events before the first listener are kept for it', () async {
      await controller.attach(native.channel);
      await native.sendFixture('bridge/ev_ready.json');
      await native.sendFixture('bridge/ev_page_changed.json');
      final seen = <InkEvent>[];
      final sub = controller.events.listen(seen.add);
      await native.sendFixture('bridge/ev_undo_state.json');
      await Future<void>.delayed(Duration.zero);
      expect(seen.map((e) => e.runtimeType), [
        InkReady,
        InkPageChanged,
        InkUndoState,
      ]);
      await sub.cancel();
    });

    test('a surface made again: the controller follows its new view', () async {
      await controller.attach(native.channel);
      final again = _Native('claktory/ink/2');
      addTearDown(again.dispose);
      final seen = <InkEvent>[];
      final sub = controller.events.listen(seen.add);
      await controller.attach(again.channel);
      expect(again.calls.single.method, 'attach');
      // The old view's events are no longer this surface's.
      await native.sendFixture('bridge/ev_undo_state.json');
      await again.sendFixture('bridge/ev_ready.json');
      await Future<void>.delayed(Duration.zero);
      expect(seen.single, isA<InkReady>());
      again.answerAsFixture('bridge/cmd_undo.json');
      await controller.undo();
      expect(again.calls.last.method, 'undo');
      expect(native.calls.map((c) => c.method), ['attach']);
      await sub.cancel();
    });

    test('after dispose, commands fail and events are dropped', () async {
      await controller.attach(native.channel);
      controller.dispose();
      expect(controller.undo, throwsStateError);
    });

    test('disposed before the view existed, waiting commands fail', () async {
      final undo = controller.undo();
      controller.dispose();
      await expectLater(undo, throwsStateError);
    });
  });

  group('render cache', () {
    late _Native native;
    setUp(() {
      native = _Native('claktory/ink');
      InkRenderCache.channel = native.channel;
    });
    tearDown(() => native.dispose());

    test('writes a copy in pieces and names its file', () async {
      native.results['cacheBegin'] = {'token': 't-1'};
      native.results['cacheFinish'] = {'file': '/cache/demo-mac/aa/paper.pdf'};
      final writer = await InkRenderCache.begin(
        backend: 'demo-mac',
        sha256: 'a' * 64,
        name: 'paper.pdf',
      );
      await writer.append(Uint8List.fromList([1, 2, 3]));
      expect(await writer.finish(), '/cache/demo-mac/aa/paper.pdf');
      expect(native.calls.map((c) => c.method), [
        'cacheBegin',
        'cacheAppend',
        'cacheFinish',
      ]);
      expect(native.calls[0].arguments, {
        'backend': 'demo-mac',
        'sha256': 'a' * 64,
        'name': 'paper.pdf',
      });
      expect((native.calls[1].arguments as Map)['bytes'], [1, 2, 3]);
      expect(
        await InkRenderCache.lookup(
          backend: 'demo-mac',
          sha256: 'b' * 64,
          name: 'x.pdf',
        ),
        isNull,
      );
    });
  });

  testWidgets('the surface claims its touches and attaches', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    final views = <Map<Object?, Object?>>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform_views,
      (call) async {
        if (call.method == 'create') views.add(call.arguments as Map);
        return null;
      },
    );
    final native = _Native('claktory/ink/0');
    final controller = InkSurfaceController();
    final params = InkParams.fromJson(_args('bridge/create_blank.json'));
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: InkSurface(
          key: GlobalKey(),
          params: params,
          controller: controller,
        ),
      ),
    );
    await tester.pump();
    final view = tester.widget<UiKitView>(find.byType(UiKitView));
    expect(view.viewType, inkViewType);
    expect(view.gestureRecognizers!.single.type, EagerGestureRecognizer);
    expect(views.single['viewType'], inkViewType);
    expect(controller.attached, isTrue);
    expect(native.calls.single.method, 'attach');
    controller.dispose();
    native.dispose();
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform_views,
      null,
    );
    debugDefaultTargetPlatformOverride = null;
  });
}
