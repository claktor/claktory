// A sealed file in a viewer (the Seal extension): "Sealed" in the header
// while it matches its latest seal, a line saying who sealed it and when
// once it changed, and Open sealed version, which reads the version the
// seal kept: in the pane's place on a computer, as a page on a phone.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/seal.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart' show PointerDeviceKind;
import 'package:flutter/widgets.dart'
    show LayoutBuilder, SingleChildScrollView, Size;
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'mobile_viewer_shots_test.dart'
    show ViewerKind, openOnPhone, phonePortrait;
import 'panes_test.dart' show beside, column, leaf, row, showPanes, split;

const _path = '/w/main.rs';

/// Today, a minute past midnight: "today" whenever the test runs.
int get _todayMs {
  final now = DateTime.now();
  return DateTime(now.year, now.month, now.day, 0, 1).millisecondsSinceEpoch;
}

FileSeal _seal({bool kept = true, String sha = 'sealed-sha'}) => FileSeal(
  sha256: sha,
  agentId: 'a1',
  agentName: 'Tomás',
  sealedMs: Int64(_todayMs),
  note: 'Pass line for run 3',
  kept: kept,
);

FileContent _text(String text, {FileSeals? seals, String path = _path}) {
  final file = FileContent(
    path: path,
    mime: 'text/plain',
    size: Int64(text.length),
    data: text.codeUnits,
    sha256: 'v-${text.hashCode}',
  );
  if (seals != null) file.seals = seals;
  return file;
}

FileSeals _seals(FileSealState state, {bool kept = true}) => FileSeals(
  path: _path,
  seals: [_seal(kept: kept)],
  state: state,
);

const _matches = FileSealState.FILE_SEAL_STATE_MATCHES;
const _changed = FileSealState.FILE_SEAL_STATE_CHANGED;

Future<Harness> _show(WidgetTester tester, FileContent file) async {
  final harness = await showPanes(
    tester,
    beside('text', _path),
    files: {_path: file},
  );
  harness.backend.sealedVersions['$_path@sealed-sha'] = _text('pass >= 0.8\n');
  await settle(tester);
  return harness;
}

/// Lets a read come and its text be decoded.
Future<void> _read(WidgetTester tester) async {
  for (var i = 0; i < 3; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 60)),
    );
    await settle(tester);
  }
}

List<ReadFile> _reads(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasReadFile()) message.readFile,
];

void main() {
  testWidgets('a file still as it was sealed says "Sealed" quietly', (
    tester,
  ) async {
    await _show(tester, _text('pass >= 0.8\n', seals: _seals(_matches)));
    expect(find.textContaining('Sealed ·'), findsOneWidget);
    expect(find.byType(SealNotice), findsNothing);
  });

  testWidgets('a file changed since its seal says who sealed it and when, '
      'and opens the sealed version in its place', (tester) async {
    final harness = await _show(
      tester,
      _text('pass >= 0.5\n', seals: _seals(_changed)),
    );
    expect(
      find.textContaining('Changed since Tomás sealed it today at '),
      findsOneWidget,
    );
    expect(find.text('Pass line for run 3'), findsOneWidget);
    expect(find.textContaining('pass >= 0.5'), findsOneWidget);
    expect(find.textContaining('Sealed ·'), findsNothing);

    await tester.tap(find.text('Open sealed version'));
    await _read(tester);
    expect(_reads(harness).last.sealedSha256, 'sealed-sha');
    expect(find.textContaining('pass >= 0.8'), findsOneWidget);
    expect(find.textContaining('pass >= 0.5'), findsNothing);
    expect(find.textContaining('Sealed version'), findsOneWidget);
    expect(
      find.textContaining('The version Tomás sealed today at '),
      findsOneWidget,
    );

    await tester.tap(find.text('Show current version'));
    await _read(tester);
    expect(find.textContaining('pass >= 0.5'), findsOneWidget);
    expect(_reads(harness).last.sealedSha256, isEmpty);
  });

  testWidgets('Delete kept copy asks first, then the version no longer '
      'opens', (tester) async {
    final harness = await _show(
      tester,
      _text('pass >= 0.5\n', seals: _seals(_changed)),
    );
    List<DeleteSealedCopy> deletes() => [
      for (final message in harness.backend.sent)
        if (message.hasDeleteSealedCopy()) message.deleteSealedCopy,
    ];

    await tester.tap(find.text('Delete kept copy…'));
    await settle(tester);
    expect(find.text('Delete the kept copy of this version?'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await settle(tester);
    expect(deletes(), isEmpty);

    // Refused: why, in the line.
    harness.backend.refuse = "This sealed version wasn't kept.";
    await tester.tap(find.text('Delete kept copy…'));
    await settle(tester);
    await tester.tap(find.text('Delete kept copy').last);
    await settle(tester);
    expect(find.text("This sealed version wasn't kept."), findsOneWidget);

    // Done: the backend says every seal of it keeps no copy now.
    harness.backend.refuse = null;
    harness.backend.onSend = (message) {
      if (!message.hasDeleteSealedCopy()) return;
      harness.backend.fileSeals.value = _seals(_changed, kept: false);
    };
    await tester.tap(find.text('Delete kept copy…'));
    await settle(tester);
    await tester.tap(find.text('Delete kept copy').last);
    await settle(tester);
    expect(deletes().last.path, _path);
    expect(deletes().last.sha256, 'sealed-sha');
    expect(find.text("This sealed version wasn't kept."), findsNothing);
    expect(find.text('Open sealed version'), findsNothing);
    expect(find.text('Delete kept copy…'), findsNothing);
    expect(
      find.textContaining('Changed since Tomás sealed it'),
      findsOneWidget,
    );
  });

  testWidgets('a version too big to keep has nothing to open', (tester) async {
    await _show(
      tester,
      _text('pass >= 0.5\n', seals: _seals(_changed, kept: false)),
    );
    expect(
      find.textContaining('Changed since Tomás sealed it'),
      findsOneWidget,
    );
    expect(find.text('Open sealed version'), findsNothing);
  });

  testWidgets('a seal made while the file shows is followed by its '
      'canonical path, not the path the viewer asked for', (tester) async {
    // Read by a path with a link in it: the backend names the file by
    // where it really is.
    final shown = _text('pass >= 0.8\n')
      ..seals = FileSeals(path: '/real/main.rs');
    final harness = await _show(tester, shown);
    expect(find.textContaining('Sealed ·'), findsNothing);

    harness.backend.fileSeals.value = FileSeals(
      path: '/w/other.rs',
      seals: [_seal()],
      state: _matches,
    );
    await settle(tester);
    expect(find.textContaining('Sealed ·'), findsNothing);

    harness.backend.fileSeals.value = FileSeals(
      path: '/real/main.rs',
      seals: [_seal()],
      state: _matches,
    );
    await settle(tester);
    expect(find.textContaining('Sealed ·'), findsOneWidget);
  });

  testWidgets('a long line in a short pane takes part of it and scrolls, '
      'and the wheel scrolls it', (tester) async {
    final long = _seals(_changed);
    long.seals.single.note = List.filled(60, 'Pass line for run 3.').join(' ');
    await showPanes(
      tester,
      split('s1', row, [
        leaf('chat', 'chat'),
        split(
          's2',
          column,
          [leaf('v1', 'text', _path), leaf('v2', 'text', '/w/other.rs')],
          [1, 4],
        ),
      ]),
      files: {
        _path: _text('pass >= 0.5\n', seals: long),
        '/w/other.rs': _text('fn main() {}\n', path: '/w/other.rs'),
      },
    );
    await settle(tester);
    expect(tester.takeException(), isNull);

    final notice = find.byType(SealNotice);
    final scroll = find.ancestor(
      of: notice,
      matching: find.byType(SingleChildScrollView),
    );
    final pane = find.ancestor(
      of: scroll,
      matching: find.byType(LayoutBuilder),
    );
    expect(
      tester.getSize(scroll).height,
      lessThanOrEqualTo(tester.getSize(pane.first).height * 0.4 + 0.5),
    );
    expect(find.textContaining('pass >= 0.5'), findsOneWidget);

    final top = tester.getTopLeft(notice).dy;
    final wheel = TestPointer(1, PointerDeviceKind.mouse);
    await tester.sendEventToBinding(wheel.hover(tester.getCenter(scroll)));
    await tester.sendEventToBinding(wheel.scroll(const Offset(0, 60)));
    await settle(tester);
    expect(tester.getTopLeft(notice).dy, lessThan(top));
  });

  testWidgets('a short line shows whole, with no room left under it', (
    tester,
  ) async {
    await _show(tester, _text('pass >= 0.5\n', seals: _seals(_changed)));
    final notice = find.byType(SealNotice);
    final scroll = find.ancestor(
      of: notice,
      matching: find.byType(SingleChildScrollView),
    );
    expect(tester.getSize(scroll).height, tester.getSize(notice).height);
  });

  testWidgets('on a phone the sealed version opens as a page, and Back '
      'comes back to the file', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    const path = '/w/notes.md';
    final harness = await openOnPhone(
      tester,
      ViewerKind.text,
      phonePortrait,
      text: 'pass >= 0.5\n',
    );
    // The file as it is now, changed since its seal.
    harness.backend.files[path] = _text(
      'pass >= 0.5\n',
      path: path,
      seals: FileSeals(path: path, seals: [_seal()], state: _changed),
    );
    harness.backend.sealedVersions['$path@sealed-sha'] = _text(
      'pass >= 0.8\n',
      path: path,
    );
    harness.backend.fileChanged.value = FileChanged(path: path);
    await pumpFrames(tester);
    expect(
      find.textContaining('Changed since Tomás sealed it'),
      findsOneWidget,
    );

    await tester.tap(find.text('Open sealed version'));
    await pumpFrames(tester);
    expect(find.textContaining('pass >= 0.8'), findsOneWidget);
    expect(
      find.textContaining('The version Tomás sealed today at '),
      findsOneWidget,
    );
    // Back does what Show current version does in a pane.
    expect(find.text('Show current version'), findsNothing);

    await tester.tap(find.bySemanticsLabel(RegExp('^Back')).last);
    await pumpFrames(tester);
    expect(find.textContaining('pass >= 0.8'), findsNothing);
    expect(find.textContaining('pass >= 0.5'), findsOneWidget);

    // Delete kept copy is in More, apart from the line's one action.
    expect(find.text('Delete kept copy…'), findsNothing);
    await tester.tap(find.bySemanticsLabel(RegExp('^More')).last);
    await pumpFrames(tester);
    await tester.tap(find.text('Delete kept copy…'));
    await pumpFrames(tester);
    expect(find.text('Delete the kept copy of this version?'), findsOneWidget);
    await tester.tap(find.text('Delete kept copy'));
    await pumpFrames(tester);
    final deletes = [
      for (final message in harness.backend.sent)
        if (message.hasDeleteSealedCopy()) message.deleteSealedCopy,
    ];
    expect(deletes.single.sha256, 'sealed-sha');
  });

  testWidgets('on a small iPhone at the largest text the line fits, and '
      'Open sealed version can be tapped', (tester) async {
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    const path = '/w/notes.md';
    final harness = await openOnPhone(tester, ViewerKind.text, (
      size: const Size(375, 667),
      padding: const FakeViewPadding(top: 20),
    ), text: 'pass >= 0.5\n');
    harness.backend.files[path] = _text(
      'pass >= 0.5\n',
      path: path,
      seals: FileSeals(path: path, seals: [_seal()], state: _changed),
    );
    harness.backend.fileChanged.value = FileChanged(path: path);
    await pumpFrames(tester);
    final open = find.text('Open sealed version');
    expect(open, findsOneWidget);
    expect(tester.getRect(open).bottom, lessThan(667));
    expect(tester.takeException(), isNull);
  });

  test('when a seal was made reads as part of a sentence', () {
    final now = DateTime(2026, 11, 27, 18);
    FileSeal at(DateTime time) =>
        FileSeal(sealedMs: Int64(time.millisecondsSinceEpoch));
    expect(
      sealedWhen(at(DateTime(2026, 11, 27, 14, 2)), now),
      'today at 14:02',
    );
    expect(
      sealedWhen(at(DateTime(2026, 11, 26, 9, 15)), now, h24: false),
      'yesterday at 9:15 AM',
    );
    expect(
      sealedWhen(at(DateTime(2026, 11, 23, 14, 2)), now),
      'on Monday at 14:02',
    );
    expect(
      sealedWhen(at(DateTime(2026, 11, 17, 14, 2)), now),
      'on Tue 17 Nov at 14:02',
    );
  });
}
