// Selecting in a scrolled panel under the app's zoom (#326, Matt): a drag
// selects what's under the pointer, not (zoom − 1) × the scroll away.
import 'package:factory_app/database.dart' show DatabaseView;
import 'package:factory_app/memory.dart' show MemoryView;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'database_test.dart' as database;
import 'harness.dart';
import 'memory_test.dart' as memory;
import 'selection_test.dart' show watchClipboard;

/// Where [word] is drawn, global: its first letter's middle, or its last
/// letter's ([end]).
Offset _at(WidgetTester tester, String word, {bool end = false}) {
  for (final paragraph
      in tester.allRenderObjects.whereType<RenderParagraph>()) {
    final text = paragraph.text.toPlainText();
    final i = text.indexOf(word);
    if (i < 0) continue;
    final at = end ? i + word.length - 1 : i;
    final box = paragraph
        .getBoxesForSelection(
          TextSelection(baseOffset: at, extentOffset: at + 1),
        )
        .first
        .toRect();
    return paragraph.localToGlobal(
      end
          ? box.centerRight - const Offset(1, 0)
          : box.centerLeft + const Offset(1, 0),
    );
  }
  throw StateError('no $word');
}

/// Drags with the mouse from [from]'s start to [to]'s end and copies.
Future<String> _drag(
  WidgetTester tester,
  List<String> copied,
  String from,
  String to,
) async {
  final start = _at(tester, from);
  final end = _at(tester, to, end: true);
  final gesture = await tester.startGesture(
    start,
    kind: PointerDeviceKind.mouse,
  );
  await tester.pump();
  for (var i = 1; i <= 10; i++) {
    await gesture.moveTo(Offset.lerp(start, end, i / 10)!);
    await tester.pump();
  }
  await gesture.up();
  await tester.pump();
  await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
  await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
  await tester.pump();
  return copied.isEmpty ? '' : copied.last;
}

/// [harness]'s app at [zoom], its panel [view] scrolled [by] down; the
/// numbers of the lines ([line] finds each) in sight then.
Future<List<int>> _zoomedAndScrolled(
  WidgetTester tester,
  Harness harness,
  double zoom,
  Type view,
  double by,
  Finder Function(int) line,
) async {
  final now = harness.backend.workspace.workspace.deepCopy();
  now.settings.zoom[harness.backend.zoomKey] = zoom;
  harness.backend.workspace.apply(now);
  await settle(tester);
  final list = tester.state<ScrollableState>(
    find
        .descendant(of: find.byType(view), matching: find.byType(Scrollable))
        .first,
  );
  list.position.jumpTo(by);
  await pumpFrames(tester);
  final panel = tester.getRect(find.byType(view));
  return [
    for (var i = 0; i < 60; i++)
      if (line(i).evaluate().isNotEmpty &&
          tester.getCenter(line(i).first).dy > panel.top + 40 &&
          tester.getCenter(line(i).first).dy < panel.bottom - 40)
        i,
  ];
}

void main() {
  for (final zoom in [1.25, 0.8]) {
    testWidgets('memory scrolled down: a drag selects what is under the '
        'pointer (zoom $zoom)', (tester) async {
      final copied = watchClipboard(tester);
      final harness = Harness(memory.workspace());
      harness.backend.extensionView.apply(
        memory.memoryView(
          Memory(
            preferences: [
              for (var i = 0; i < 60; i++)
                MemoryPreference(
                  id: i + 1,
                  rule: 'Rule$i keeps alpha$i apart from omega$i',
                  learned: memory.at(memory.now),
                ),
            ],
          ),
        ),
      );
      await harness.show(tester, size: const Size(1400, 900));
      await settle(tester);
      final shown = await _zoomedAndScrolled(
        tester,
        harness,
        zoom,
        MemoryView,
        900,
        (i) => find.textContaining('Rule$i keeps', findRichText: true),
      );
      expect(shown.length, greaterThan(3));
      final mid = shown[shown.length ~/ 2];
      final got = await _drag(tester, copied, 'alpha$mid', 'omega${mid + 1}');
      expect(got, startsWith('alpha$mid '));
      expect(got.trimRight(), endsWith('omega${mid + 1}'));
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('the database scrolled down: a drag selects what is under '
        'the pointer (zoom $zoom)', (tester) async {
      final copied = watchClipboard(tester);
      final harness = await database.show(
        tester,
        Database(
          tables: [
            for (var i = 0; i < 60; i++)
              DatabaseTable(name: 'table$i', rows: Int64(i)),
          ],
        ),
      );
      tester.view.physicalSize = const Size(1400, 900);
      final shown = await _zoomedAndScrolled(
        tester,
        harness,
        zoom,
        DatabaseView,
        700,
        (i) => find.text('table$i'),
      );
      expect(shown.length, greaterThan(3));
      final mid = shown[shown.length ~/ 2];
      final got = await _drag(tester, copied, 'table$mid', 'table${mid + 2}');
      expect(got, startsWith('table$mid'));
      expect(got.trimRight(), endsWith('table${mid + 2}'));
      expect(got, contains('table${mid + 1}'));
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  }
}
