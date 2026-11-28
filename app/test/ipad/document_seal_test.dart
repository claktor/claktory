// A sealed PDF in the iPad's document host (ipad/document_host.dart): the
// same line under the header as the other viewers once it changed since
// its seal, with Open sealed version in the pane's place, the host kept
// hidden behind it; "Sealed" in the details while it still matches; on a
// window at the phone's routes, a page of its own and Delete in More.
import 'package:factory_app/ipad/ink_tools.dart' show InkTools;
import 'package:factory_app/pane_chrome.dart' show PaneScaffold;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/seal.dart' show SealNotice;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart' show Harness;
import '../panes_test.dart' show column, leaf, row, split;
import 'as_ipad.dart';
import 'document_native.dart';

const _changed = FileSealState.FILE_SEAL_STATE_CHANGED;
const _matches = FileSealState.FILE_SEAL_STATE_MATCHES;

/// Today, a minute past midnight: "today" whenever the test runs.
int get _todayMs {
  final now = DateTime.now();
  return DateTime(now.year, now.month, now.day, 0, 1).millisecondsSinceEpoch;
}

FileSeal _seal({bool kept = true}) => FileSeal(
  sha256: 'sealed-sha',
  agentId: 'a1',
  agentName: 'Tomás',
  sealedMs: Int64(_todayMs),
  note: 'The figures Matt signed off',
  kept: kept,
);

FileContent _sealed(FileSealState? state, {bool kept = true}) {
  final file = documentPdf('the figures, revised');
  file.seals = FileSeals(
    path: documentPath,
    seals: [if (state != null) _seal(kept: kept)],
    state: state ?? FileSealState.FILE_SEAL_STATE_UNSPECIFIED,
  );
  return file;
}

/// The native side of the document [_open] showed last.
late DocumentNative _native;

Future<Harness> _open(
  WidgetTester tester,
  FileContent file, {
  Size size = const Size(1366, 1024),
  bool phoneRoutes = false,
  ViewNode? layout,
}) async {
  asIPad();
  final native = _native = DocumentNative();
  final harness = await showDocument(
    tester,
    asking: false,
    file: file,
    size: size,
    phoneRoutes: phoneRoutes,
    layout: layout,
  );
  harness.backend.sealedVersions['$documentPath@sealed-sha'] = documentPdf(
    'the figures, as signed off',
  );
  await native.ready();
  await settle(tester);
  return harness;
}

List<ReadFile> _reads(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasReadFile() && message.readFile.path == documentPath)
      message.readFile,
];

void main() {
  testWidgets('changed since its seal: who sealed it and when, and the '
      'sealed version in the pane\'s place, the PDF as it was behind it', (
    tester,
  ) async {
    final harness = await _open(tester, _sealed(_changed));
    expect(
      find.textContaining('Changed since Tomás sealed it today at '),
      findsOneWidget,
    );
    expect(find.text('The figures Matt signed off'), findsOneWidget);
    expect(find.text('Delete kept copy…'), findsOneWidget);
    final fileReads = _reads(harness).length;

    await tester.tap(find.text('Open sealed version'));
    await settle(tester);
    expect(_reads(harness).last.sealedSha256, 'sealed-sha');
    expect(
      find.textContaining('The version Tomás sealed today at '),
      findsOneWidget,
    );
    expect(find.byType(InkTools), findsNothing);

    await tester.tap(find.text('Show current version'));
    await settle(tester);
    expect(find.byType(InkTools), findsOneWidget);
    expect(
      find.textContaining('Changed since Tomás sealed it'),
      findsOneWidget,
    );
    // The host was kept, not made again: the file isn't read again.
    expect(
      _reads(harness).skip(fileReads).where((r) => r.sealedSha256.isEmpty),
      isEmpty,
    );
  });

  testWidgets('still as sealed, it says "Sealed" quietly', (tester) async {
    await _open(tester, _sealed(_matches));
    expect(find.textContaining('Sealed'), findsOneWidget);
    expect(find.byType(SealNotice), findsNothing);
  });

  for (final scale in [1.0, 2.0]) {
    testWidgets('in a pane too short for the line, the tools and the '
        'status, the pane scrolls rather than overflowing, text at $scale', (
      tester,
    ) async {
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await _open(
        tester,
        _sealed(_changed),
        layout: split('s1', row, [
          leaf('chat', 'chat'),
          split(
            's2',
            column,
            [leaf('paper', 'pdf', documentPath), leaf('t', 'text', '/w/t.md')],
            [1, 6],
          ),
        ]),
      );
      await _native.drew(1, 1);
      await settle(tester);
      expect(tester.takeException(), isNull);
      // The weights left the pane short.
      expect(
        tester
            .getSize(
              find.ancestor(
                of: find.byType(SealNotice),
                matching: find.byType(PaneScaffold),
              ),
            )
            .height,
        lessThanOrEqualTo(210),
      );
      expect(find.text('Open sealed version'), findsOneWidget);
      expect(find.byType(InkTools), findsOneWidget);
    });
  }

  testWidgets('a version too big to keep has nothing to open', (tester) async {
    await _open(tester, _sealed(_changed, kept: false));
    expect(
      find.textContaining('Changed since Tomás sealed it'),
      findsOneWidget,
    );
    expect(find.text('Open sealed version'), findsNothing);
    expect(find.text('Delete kept copy…'), findsNothing);
  });

  testWidgets('a seal made while it shows is followed', (tester) async {
    final harness = await _open(tester, _sealed(null));
    expect(find.byType(SealNotice), findsNothing);

    harness.backend.fileSeals.value = FileSeals(
      path: '/w/other.pdf',
      seals: [_seal()],
      state: _changed,
    );
    await settle(tester);
    expect(find.byType(SealNotice), findsNothing);

    harness.backend.fileSeals.value = FileSeals(
      path: documentPath,
      seals: [_seal()],
      state: _changed,
    );
    await settle(tester);
    expect(find.byType(SealNotice), findsOneWidget);
  });

  testWidgets('at the phone\'s routes the sealed version is a page, and '
      'Delete kept copy is in More', (tester) async {
    final harness = await _open(
      tester,
      _sealed(_changed),
      size: const Size(390, 844),
      phoneRoutes: true,
    );
    expect(
      find.textContaining('Changed since Tomás sealed it'),
      findsOneWidget,
    );
    expect(find.text('Delete kept copy…'), findsNothing);

    await tester.tap(find.text('Open sealed version'));
    await settle(tester);
    expect(_reads(harness).last.sealedSha256, 'sealed-sha');
    expect(
      find.textContaining('The version Tomás sealed today at '),
      findsOneWidget,
    );
    // Back does what Show current version does in a pane.
    expect(find.text('Show current version'), findsNothing);

    await tester.tap(find.bySemanticsLabel(RegExp('^Back')).last);
    await settle(tester);
    expect(
      find.textContaining('Changed since Tomás sealed it'),
      findsOneWidget,
    );
    await tester.tap(find.bySemanticsLabel(RegExp('^More')).hitTestable().last);
    await settle(tester);
    await tester.tap(find.text('Delete kept copy…'));
    await settle(tester);
    await tester.tap(find.text('Delete kept copy'));
    await settle(tester);
    final deletes = [
      for (final message in harness.backend.sent)
        if (message.hasDeleteSealedCopy()) message.deleteSealedCopy,
    ];
    expect(deletes.single.sha256, 'sealed-sha');
  });
}
