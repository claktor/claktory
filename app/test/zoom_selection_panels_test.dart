// Selecting under the app's zoom (#326) in Needs you and the Checklist
// panel: scrolled part way down, a drag selects what's under the pointer,
// not lines away from it (ScrolledSelection).
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'extensions_test.dart' show at, now, workspace;
import 'harness.dart';
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

/// A mouse drag from [from]'s start to [to]'s end, then ⌘C: what's copied.
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

void _zoom(Harness harness, double zoom) {
  final next = harness.backend.workspace.workspace.deepCopy();
  next.settings.zoom[harness.backend.zoomKey] = zoom;
  harness.backend.workspace.apply(next);
}

/// The vertical scroll view that scrolls, the last such: the page's.
ScrollableState _scrolling(WidgetTester tester) => tester
    .stateList<ScrollableState>(find.byType(Scrollable))
    .lastWhere(
      (state) =>
          state.axisDirection == AxisDirection.down &&
          state.position.maxScrollExtent > 0,
    );

/// The numbers [word] is shown with in [list]'s sight, in order.
List<int> _shown(
  WidgetTester tester,
  ScrollableState list,
  String word,
  int count,
) {
  final box = list.context.findRenderObject()! as RenderBox;
  final top = box.localToGlobal(Offset.zero).dy;
  final bottom = box.localToGlobal(box.size.bottomLeft(Offset.zero)).dy;
  return [
    for (var i = 1; i <= count; i++)
      if (tester.allRenderObjects.whereType<RenderParagraph>().any((p) {
        if (!p.text.toPlainText().contains('$word$i ')) return false;
        final y = p.localToGlobal(Offset.zero).dy;
        return y > top + 10 && y < bottom - 60;
      }))
        i,
  ];
}

void main() {
  for (final zoom in [1.0, 1.25, 0.8]) {
    testWidgets('Needs you scrolled down: a drag selects what is under the '
        'pointer (zoom $zoom)', (tester) async {
      final copied = watchClipboard(tester);
      final snapshot = workspace();
      final tester2 = snapshot.agents.firstWhere((agent) => agent.id == 'a2');
      for (var i = 1; i <= 30; i++) {
        tester2.waitingSeqs.add(Int64(i));
      }
      final harness = Harness(snapshot);
      for (var i = 1; i <= 30; i++) {
        harness.backend.entriesById['a2/$i'] = Entry(
          seq: Int64(i),
          timestampMs: at(now.subtract(Duration(minutes: 40 - i))),
          askPerson: AskPerson(
            id: 'ask$i',
            title: 'Question start$i asks until finish$i ends',
            wantsText: true,
          ),
        );
      }
      _zoom(harness, zoom);
      await harness.show(tester, size: const Size(1400, 1000));
      await settle(tester);
      harness.page.value = AppPage.inbox;
      await settle(tester);
      await settle(tester);
      final page = _scrolling(tester);
      page.position.jumpTo(page.position.maxScrollExtent / 2);
      await settle(tester);
      final shown = _shown(tester, page, 'start', 30);
      expect(shown.length, greaterThan(3));
      // Newest first: the one below has the lower number.
      final mid = shown[shown.length ~/ 2];
      final got = await _drag(tester, copied, 'start$mid', 'finish${mid - 1}');
      expect(got, startsWith('start$mid'));
      expect(got.trimRight(), endsWith('finish${mid - 1}'));
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

    testWidgets('the Checklist panel scrolled up: a drag selects what is '
        'under the pointer (zoom $zoom)', (tester) async {
      final copied = watchClipboard(tester);
      final harness = Harness(workspace(openPanel: 'checklist'));
      _zoom(harness, zoom);
      await harness.show(tester, size: const Size(1400, 1000));
      await settle(tester);
      harness.backend.extensionView.apply(
        ExtensionView(
          extensionId: 'checklist',
          agentId: 'a1',
          title: 'Checklist',
          checklist: Checklist(
            items: [
              for (var i = 1; i <= 60; i++)
                ChecklistItem(
                  id: i,
                  text: 'Step start$i runs to finish$i ',
                  status: ChecklistStatus.CHECKLIST_STATUS_OPEN,
                  created: at(DateTime(now.year, now.month, now.day, 1, i)),
                ),
            ],
          ),
        ),
      );
      await pumpFrames(tester);
      await settle(tester);
      // It follows its end: back up through it.
      final list = _scrolling(tester);
      list.position.jumpTo(list.position.pixels - 900);
      await settle(tester);
      final shown = _shown(tester, list, 'start', 60);
      expect(shown.length, greaterThan(3));
      final mid = shown[shown.length ~/ 2];
      final got = await _drag(tester, copied, 'start$mid', 'finish${mid + 1}');
      expect(got, startsWith('start$mid'));
      expect(got.trimRight(), endsWith('finish${mid + 1}'));
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  }
}
