// A file an agent named with characters that reorder text or show nothing:
// `invoice\u202Efdp.exe` read as
// "invoiceexe.pdf" in its pane's header. Every name the app draws shows
// them as their codes, as the conversation's tool lines do; and what's done
// with the file (opened, shared) is the file itself, not the name as shown
// ("No such file …\u{202E}…").
import 'dart:async';
import 'dart:io';

import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/pane_chrome.dart' show PhonePaneHeader;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/share_files.dart' show debugSharesFolder;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'files_test.dart' show inFiles, showFiles, tree, withFilesOf;
import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'panes_test.dart' show beside, snapshot;
import 'tool_views_test.dart' show call;

/// The file as it's named, and as the app shows its name.
const _name = 'invoice\u202Efdp.mp4';
const _shown = r'invoice\u{202E}fdp.mp4';
const _path = '/w/media/$_name';

FileContent _file() => FileContent(
  path: _path,
  mime: 'text/plain',
  size: Int64(9),
  data: 'Ship it.\n'.codeUnits,
  sha256: 'v1',
);

void main() {
  for (final pad in [false, true]) {
    testWidgets("a pane's header shows the name's hidden characters as codes "
        '(${pad ? 'iPad' : 'Mac'})', (tester) async {
      if (pad) asIPad();
      final harness = Harness(
        snapshot(layout: beside('text', '/w/media/invoice\u202Efdp.exe')),
      );
      await harness.show(
        tester,
        size: pad ? const Size(1024, 1366) : const Size(1200, 900),
      );
      await settle(tester);
      expect(find.text(r'invoice\u{202E}fdp.exe'), findsWidgets);
      expect(find.text('invoice\u202Efdp.exe'), findsNothing);
    });
  }

  testWidgets("a message's file and a row in Files show them too", (
    tester,
  ) async {
    final harness = await showFiles(
      tester,
      layout: withFilesOf('p1'),
      focused: 'p1',
      explorers: [
        tree(
          entries: [FileEntry(path: _name, name: _name)],
        ),
      ],
    );
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(1),
            user: UserText(
              text: 'The take',
              attachments: [
                Attachment(path: _path, name: _name, size: Int64(9)),
              ],
            ),
          ),
        ],
      ),
    );
    await settle(tester);
    expect(inFiles(_shown), findsOneWidget);
    expect(find.text(_shown), findsNWidgets(2));
    expect(find.text(_name), findsNothing);
  });

  testWidgets("on a phone, a show line's file opens and shares as itself", (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    debugSharesFolder = Directory.systemTemp.createTempSync('shares-test-');
    addTearDown(() {
      debugSharesFolder?.deleteSync(recursive: true);
      debugSharesFolder = null;
    });
    final shared = <List<String>>[];
    final names = <String>[];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(const MethodChannel('factory/device'), (
          message,
        ) async {
          if (message.method != 'shareFiles') return null;
          final paths = [
            for (final p in (message.arguments as Map)['paths'] as List)
              p as String,
          ];
          shared.add([for (final p in paths) File(p).readAsStringSync()]);
          names.addAll(paths.map((p) => p.split('/').last));
          return false;
        });
    addTearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(
            const MethodChannel('factory/device'),
            null,
          ),
    );
    final harness = Harness(snapshot(layout: beside('text', _path)));
    harness.backend
      ..appliesLayouts = true
      ..files[_path] = _file();
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          call(
            'show',
            {'path': _path},
            seq: 3,
            output: 'Showing it to Matt.',
            details: {
              'path': _path,
              'kind': 'text',
              'pane': 'v1',
              'agent': 'a1',
            },
          ),
        ],
      ),
    );
    tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    expect(find.text(_shown), findsOneWidget);

    // Opened: the file itself, read and shown.
    await tester.tap(find.text(_shown));
    await pumpFrames(tester);
    expect(find.textContaining('Ship it.', findRichText: true), findsWidgets);
    expect(find.textContaining('No such file'), findsNothing);
    // Its page's header names it with the codes.
    expect(
      find.descendant(
        of: find.byType(PhonePaneHeader),
        matching: find.text(_shown),
      ),
      findsOneWidget,
    );
    expect(
      harness.backend.sent
          .where((m) => m.hasReadFile())
          .map((m) => m.readFile.path)
          .toSet(),
      {_path},
    );
    tester.widget<PhoneRoutesScope>(find.byType(PhoneRoutesScope)).routes.pop();
    await pumpFrames(tester);

    // Shared: the file itself, named without what hides in its name, the
    // sheet's title too.
    final read = Completer<void>();
    harness.backend.holdRead = (_) => read.future;
    await tester.longPress(find.text(_shown));
    await pumpFrames(tester);
    await tester.tap(find.text('Share…'));
    Future<void> work() async {
      for (var i = 0; i < 30; i++) {
        await tester.runAsync(
          () => Future<void>.delayed(const Duration(milliseconds: 30)),
        );
        await tester.pump(const Duration(milliseconds: 30));
      }
    }

    await work();
    expect(find.text('Share invoicefdp.mp4'), findsOneWidget);
    read.complete();
    await work();
    expect(shared.single, ['Ship it.\n']);
    expect(names, ['invoicefdp.mp4']);
  });
}
