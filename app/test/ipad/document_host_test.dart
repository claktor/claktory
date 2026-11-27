// The iPad's document host (ipad/document_host.dart): a "pdf" pane on an
// iPad is the host, read whole into the render cache from one version of
// the file, and anywhere else the viewers' read-only pane.
import 'dart:convert';

import 'package:crypto/crypto.dart';
import 'package:factory_app/ipad/document_host.dart';
import 'package:factory_app/ipad/ink_bridge.dart';
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/mobile/device.dart';
import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/panes.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../panes_test.dart' show leaf, row, snapshot, split;
import 'as_ipad.dart';

const _path = '/w/paper.pdf';

FileContent _pdf(String body) {
  final bytes = utf8.encode('%PDF-1.7\n$body');
  return FileContent(
    path: _path,
    mime: 'application/pdf',
    size: Int64(bytes.length),
    data: bytes,
    sha256: sha256.convert(bytes).toString(),
  );
}

ViewNode _layout() =>
    split('s1', row, [leaf('chat', 'chat'), leaf('paper', 'pdf', _path)]);

/// The render cache's side of the channel: what it was asked, and the
/// copies it made.
class _Cache {
  _Cache() {
    InkRenderCache.channel = const MethodChannel('test/ink-cache');
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(InkRenderCache.channel, (call) async {
          final args = (call.arguments as Map?)?.cast<String, Object?>() ?? {};
          calls.add(call.method);
          switch (call.method) {
            case 'cacheLookup':
              return <String, Object?>{'file': null};
            case 'cacheBegin':
              _writing = [];
              _sha = args['sha256'] as String;
              return {'token': 't${calls.length}'};
            case 'cacheAppend':
              _writing.addAll(args['bytes'] as Uint8List);
              return null;
            case 'cacheFinish':
              if (sha256.convert(_writing).toString() != _sha) {
                throw PlatformException(code: 'hash_mismatch');
              }
              kept = utf8.decode(_writing);
              return {'file': '/cache/$_sha/paper.pdf'};
            case 'cacheCancel':
              return null;
          }
          return null;
        });
    addTearDown(() {
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(InkRenderCache.channel, null);
      InkRenderCache.channel = const MethodChannel('claktory/ink');
    });
  }

  final calls = <String>[];
  var _writing = <int>[];
  var _sha = '';

  /// The copy kept, as text.
  String? kept;
}

/// The native views a test makes: none, but asked for.
List<Map<Object?, Object?>> _platformViews() {
  final made = <Map<Object?, Object?>>[];
  TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
      .setMockMethodCallHandler(SystemChannels.platform_views, (call) async {
        if (call.method == 'create') {
          made.add(call.arguments as Map<Object?, Object?>);
        }
        return null;
      });
  addTearDown(
    () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform_views, null),
  );
  return made;
}

Future<Harness> _show(WidgetTester tester, FileContent file) async {
  final harness = Harness(snapshot(layout: _layout()));
  harness.backend.files[_path] = file;
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  await harness.show(
    tester,
    size: const Size(1366, 1024),
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
  for (var i = 0; i < 20; i++) {
    await tester.pump(const Duration(milliseconds: 20));
  }
  return harness;
}

List<ReadFile> _reads(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasReadFile()) message.readFile,
];

void main() {
  setUp(() => documentChunk = 8);
  tearDown(() => documentChunk = 8 * 1024 * 1024);

  testWidgets('on an iPad a PDF is read in ranges from one version into the '
      'render cache, and drawn on', (tester) async {
    asIPad();
    final cache = _Cache();
    final views = _platformViews();
    final file = _pdf('a paper on curlews');
    final harness = await _show(tester, file);

    final reads = _reads(harness);
    expect(reads.first.offset, Int64(0));
    expect(reads.first.expectedSha256, isEmpty);
    // Every range after the first asks for the first's version.
    expect(reads.length, greaterThan(2));
    for (final read in reads.skip(1)) {
      expect(read.expectedSha256, file.sha256);
    }
    expect(cache.kept, utf8.decode(file.data));
    expect(cache.calls, contains('cacheFinish'));
    // The native surface, with the cached copy.
    expect(views, hasLength(1));
    final params = views.single['params'];
    expect(params, isNotNull);
    expect(find.text('Drawing tools'), findsNothing);
  });

  testWidgets('a PDF that changes while it comes is read again from its '
      'start, the new version kept', (tester) async {
    asIPad();
    final cache = _Cache();
    _platformViews();
    final before = _pdf('the first draft of the paper');
    final after = _pdf('the second draft, longer than the first one');
    Harness? shown;
    var changed = false;
    final harness = Harness(snapshot(layout: _layout()));
    harness.backend.onSend = (message) {
      // The file is saved again on the machine after its first range.
      if (!changed && message.hasReadFile() && message.readFile.offset > 0) {
        changed = true;
        harness.backend.files[_path] = after;
      }
    };
    harness.backend.files[_path] = before;
    final focus = WorkbenchFocus();
    addTearDown(focus.dispose);
    await harness.show(
      tester,
      size: const Size(1366, 1024),
      home: WorkbenchScope(
        focus: focus,
        child: Builder(
          builder: (context) {
            final store = AppScope.of(context).workspace;
            return Workbench(
              rail: const SizedBox(width: 40),
              sidebar: (width) => SizedBox(width: width),
              sidebarWidth: 248,
              agent: store.selectedAgent,
              content: PaneLayout(
                agent: store.selectedAgent!,
                chat: const SizedBox.expand(),
              ),
            );
          },
        ),
      ),
    );
    shown = harness;
    for (var i = 0; i < 30; i++) {
      await tester.pump(const Duration(milliseconds: 20));
    }
    expect(changed, isTrue);
    expect(cache.calls, contains('cacheCancel'));
    expect(cache.kept, utf8.decode(after.data));
    final reads = _reads(shown);
    expect(reads.where((read) => read.offset == 0), hasLength(2));
  });

  testWidgets('a PDF opened before the Claktory connects is read once it '
      'connects', (tester) async {
    asIPad();
    final cache = _Cache();
    _platformViews();
    final file = _pdf('a paper on lapwings');
    final harness = Harness(snapshot(layout: _layout()));
    harness.backend
      ..files[_path] = file
      ..connected.value = false
      ..refuse = "Can't reach your Claktory right now.";
    final focus = WorkbenchFocus();
    addTearDown(focus.dispose);
    await harness.show(
      tester,
      size: const Size(1366, 1024),
      home: WorkbenchScope(
        focus: focus,
        child: Builder(
          builder: (context) {
            final store = AppScope.of(context).workspace;
            return Workbench(
              rail: const SizedBox(width: 40),
              sidebar: (width) => SizedBox(width: width),
              sidebarWidth: 248,
              agent: store.selectedAgent,
              content: PaneLayout(
                agent: store.selectedAgent!,
                chat: const SizedBox.expand(),
              ),
            );
          },
        ),
      ),
    );
    for (var i = 0; i < 10; i++) {
      await tester.pump(const Duration(milliseconds: 20));
    }
    expect(find.text("Can't reach your Claktory right now."), findsOneWidget);
    expect(cache.kept, isNull);

    harness.backend
      ..refuse = null
      ..connected.value = true;
    for (var i = 0; i < 20; i++) {
      await tester.pump(const Duration(milliseconds: 20));
    }
    expect(find.text("Can't reach your Claktory right now."), findsNothing);
    expect(cache.kept, utf8.decode(file.data));
  });

  testWidgets('in a window narrowed to the phone\'s routes, an agent\'s PDF '
      'page is the host, with its own review bar only', (tester) async {
    asIPad();
    final cache = _Cache();
    _platformViews();
    final file = _pdf('a paper on lapwings');
    final workspace = snapshot(layout: _layout());
    workspace.agents.firstWhere((a) => a.id == 'a1').waitingSeqs.add(Int64(7));
    final harness = Harness(workspace);
    harness.backend
      ..files[_path] = file
      ..entriesById['a1/7'] = Entry(
        seq: Int64(7),
        askPerson: AskPerson(
          id: 'q1',
          title: 'Look over the paper',
          state: AskPersonState.ASK_PERSON_STATE_PENDING,
          reviewPath: _path,
        ),
      );
    final focus = WorkbenchFocus();
    addTearDown(focus.dispose);
    await harness.show(
      tester,
      size: const Size(375, 1024),
      home: WorkbenchScope(focus: focus, child: const Shell()),
    );
    await pumpFrames(tester);
    final routes = tester
        .widget<PhoneRoutesScope>(find.byType(PhoneRoutesScope))
        .routes;
    expect(routes.showFile('a1', _path), isTrue);
    await pumpFrames(tester);
    expect(
      find.text('To see this PDF, open it on a Mac, iPhone or iPad.'),
      findsNothing,
    );
    expect(find.byKey(const ValueKey('ink-finger')), findsOneWidget);
    expect(cache.kept, utf8.decode(file.data));
    // The host's bar, not the phone's as well.
    expect(find.text('Builder asks you to review this'), findsOneWidget);
    expect(find.byKey(const ValueKey('review-answer')), findsOneWidget);
  });

  test('the document host is an iPad\'s, for "pdf" panes only', () {
    final pdf = leaf('paper', 'pdf', _path);
    final text = leaf('notes', 'text', '/w/notes.md');
    bool hosts(ViewNode node) =>
        defaultTargetPlatform == TargetPlatform.iOS &&
        deviceCapabilities.idiom == Idiom.pad &&
        node.leaf.kind == documentKind;
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() {
      debugDefaultTargetPlatformOverride = null;
      debugDeviceCapabilities = null;
    });
    debugDeviceCapabilities = const DeviceCapabilities(idiom: Idiom.pad);
    expect(hosts(pdf), isTrue);
    expect(hosts(text), isFalse);
    debugDeviceCapabilities = const DeviceCapabilities(idiom: Idiom.phone);
    expect(hosts(pdf), isFalse);
  });

  testWidgets('elsewhere a PDF pane says where PDFs open, reading nothing', (
    tester,
  ) async {
    final harness = await _show(tester, _pdf('a paper'));
    expect(_reads(harness), isEmpty);
    expect(find.text('Drawing tools'), findsNothing);
    expect(
      find.text('To see this PDF, open it on a Mac, iPhone or iPad.'),
      findsOneWidget,
    );
  });
}
