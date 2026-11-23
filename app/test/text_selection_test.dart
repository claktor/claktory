// Selecting in the file viewer (Matt): a drag selects what's under the
// pointer, from top to bottom of a long Markdown file with tables,
// formatted and plain, commenting or not.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'panes_test.dart' show beside, showPanes;
import 'selection_test.dart' show watchClipboard;

const _path = '/w/report.md';

String _report() {
  final out = StringBuffer('# Report\n\n');
  for (var section = 1; section <= 6; section++) {
    out.write('## Section $section\n\n');
    for (var line = 1; line <= 4; line++) {
      out.write(
        'Paragraph s${section}p$line says something long enough to wrap '
        'onto the next line of the pane, with words such as alpha$section$line '
        'and omega$section$line in it.\n\n',
      );
    }
    out.write('| What exists | What the row says |\n|---|---|\n');
    for (var row = 1; row <= 4; row++) {
      out.write(
        '| cell$section$row left | **cell$section$row right**, '
        'a longer cell that may wrap |\n',
      );
    }
    out.write('\n- point$section one\n- point$section two\n\n');
  }
  return out.toString();
}

/// Where [word] is drawn, global: its first letter's middle, or its last
/// letter's ([end]).
Offset _at(WidgetTester tester, String word, {bool end = false}) {
  for (final paragraph
      in tester.allRenderObjects.whereType<RenderParagraph>()) {
    final text = paragraph.text.toPlainText();
    final i = text.indexOf(word);
    if (i < 0) continue;
    final at = end ? i + word.length - 1 : i;
    final boxes = paragraph.getBoxesForSelection(
      TextSelection(baseOffset: at, extentOffset: at + 1),
    );
    final box = boxes.first.toRect();
    return paragraph.localToGlobal(
      end
          ? box.centerRight - const Offset(1, 0)
          : box.centerLeft + const Offset(1, 0),
    );
  }
  throw StateError('no $word');
}

/// Where [state]'s view is in the window.
Rect _rectOf(ScrollableState state) {
  final box = state.context.findRenderObject()! as RenderBox;
  return Rect.fromPoints(
    box.localToGlobal(Offset.zero),
    box.localToGlobal(box.size.bottomRight(Offset.zero)),
  );
}

Future<String> _drag(
  WidgetTester tester,
  List<String> copied,
  String from,
  String to, {
  bool hold = false,
}) async {
  // Where it starts, near the viewer's top, the file scrolled to it.
  for (final paragraph
      in tester.allRenderObjects.whereType<RenderParagraph>()) {
    if (!paragraph.text.toPlainText().contains(from)) continue;
    // With room below it for where the drag ends.
    paragraph.showOnScreen(
      rect: Rect.fromLTWH(0, -60, paragraph.size.width, 600),
      duration: Duration.zero,
    );
    break;
  }
  await tester.pump();
  final start = _at(tester, from);
  final end = _at(tester, to, end: true);
  // The scroll views the drag stays inside, where nothing should scroll.
  final scrolled = [
    for (final state in tester.stateList<ScrollableState>(
      find.byType(Scrollable),
    ))
      if (_rectOf(state).contains(start) && _rectOf(state).contains(end))
        (state, state.position.pixels),
  ];

  final gesture = await tester.startGesture(
    start,
    kind: PointerDeviceKind.mouse,
  );
  await tester.pump();
  for (var i = 1; i <= 10; i++) {
    await gesture.moveTo(Offset.lerp(start, end, i / 10)!);
    await tester.pump();
  }
  // Held still a moment, as a hand does before letting go: inside the
  // view, nothing scrolls (Abbacchio: at 125% the conversation ran off).
  if (hold && scrolled.isNotEmpty) {
    for (var i = 0; i < 30; i++) {
      await tester.pump(const Duration(milliseconds: 16));
    }
    for (final (state, pixels) in scrolled) {
      expect(state.position.pixels, pixels, reason: '$from → $to');
    }
  }
  await gesture.up();
  await tester.pump();
  await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
  await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
  await tester.pump();
  return copied.isEmpty ? '' : copied.last;
}

void main() {
  // Zucchero's row 105: a selection survived the Formatted/Plain toggle at
  // the same character offset, landing on other words.
  testWidgets('switching to Plain and back drops what was selected', (
    tester,
  ) async {
    asIPad();
    final copied = watchClipboard(tester);
    final body = _report();
    await showPanes(
      tester,
      beside('text', _path),
      files: {
        _path: FileContent(
          path: _path,
          mime: 'text/markdown',
          data: body.codeUnits,
          size: Int64(body.length),
          sha256: 'v1',
        ),
      },
    );
    tester.view.physicalSize = const Size(1400, 1100);
    await settle(tester);
    List<TextSelection> lit() => [
      for (final paragraph
          in tester.allRenderObjects.whereType<RenderParagraph>())
        for (final selection in paragraph.selections)
          if (!selection.isCollapsed) selection,
    ];
    expect(await _drag(tester, copied, 'alpha23', 'omega24'), isNotEmpty);
    expect(lit(), isNotEmpty);
    for (final view in ['Plain', 'Formatted']) {
      copied.clear();
      await tester.tap(find.text(view));
      await settle(tester);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
      await tester.pump();
      expect(copied.where((text) => text.isNotEmpty), isEmpty, reason: view);
      // Nothing lit in the text either.
      expect(lit(), isEmpty, reason: view);
      // Nothing to comment on either.
      expect(find.text('Comment'), findsNothing, reason: view);
    }
  });

  for (final (sha, zoom, plain) in [
    ('v1', 1.0, false),
    ('', 1.0, false),
    ('v1', 1.25, false),
    ('v1', 0.8, false),
    ('v1', 1.25, true),
    ('', 0.8, true),
  ]) {
    testWidgets('a drag selects what is under the pointer '
        '(${plain ? 'plain' : 'formatted'}, '
        '${sha.isEmpty ? 'not commenting' : 'commenting'}, zoom $zoom)', (
      tester,
    ) async {
      final copied = watchClipboard(tester);
      final body = _report();
      final harness = await showPanes(
        tester,
        beside('text', _path),
        files: {
          _path: FileContent(
            path: _path,
            mime: 'text/markdown',
            data: body.codeUnits,
            size: Int64(body.length),
            sha256: sha,
          ),
        },
      );
      tester.view.physicalSize = const Size(1400, 1100);
      final now = harness.backend.workspace.workspace.deepCopy();
      now.settings.zoom[harness.backend.zoomKey] = zoom;
      harness.backend.workspace.apply(now);
      await settle(tester);
      if (plain) {
        await tester.tap(find.text('Plain'));
        await settle(tester);
      }
      // Scrolled part way down, as reading a long file is.
      final scrollable = find.byWidgetPredicate(
        (w) => w is Scrollable && w.axisDirection == AxisDirection.down,
      );
      for (final state in tester.stateList<ScrollableState>(scrollable)) {
        debugPrint('scrollable max ${state.position.maxScrollExtent}');
      }
      final viewer = tester.stateList<ScrollableState>(scrollable).last;
      viewer.position.jumpTo(600);
      await settle(tester);
      // Plain lines don't wrap: words near their starts, in sight.
      final pairs = plain
          ? [
              ('s2p3', 's2p4'),
              ('s2p4', 'cell23'),
              ('cell22', 's3p1'),
              ('point2', 's3p2'),
            ]
          : [
              ('alpha23', 'omega24'),
              ('alpha24', 'cell23'),
              ('cell22', 'alpha31'),
              ('omega24', 'point3'),
            ];
      for (final (from, to) in pairs) {
        final got = await _drag(tester, copied, from, to, hold: true);
        debugPrint(
          '$from → $to: ${got.length} chars: '
          '${got.substring(0, got.length.clamp(0, 30))} … '
          '${got.substring((got.length - 30).clamp(0, got.length))}',
        );
        expect(got, startsWith(from), reason: '$from → $to');
        expect(got.trimRight(), endsWith(to), reason: '$from → $to');
        // Plain copies the file's own lines: line breaks and blank lines
        // kept.
        if (plain) {
          final start = body.indexOf(from);
          expect(
            got,
            body.substring(start, body.indexOf(to, start) + to.length),
            reason: '$from → $to',
          );
        }
      }
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  }

  for (final zoom in [1.25, 0.8]) {
    testWidgets('in the conversation scrolled up, a drag selects what is '
        'under the pointer (zoom $zoom)', (tester) async {
      final copied = watchClipboard(tester);
      final harness = Harness(workspace(agents: [builder(working: false)]));
      final now = harness.backend.workspace.workspace.deepCopy();
      now.settings.zoom[harness.backend.zoomKey] = zoom;
      harness.backend.workspace.apply(now);
      harness.backend.transcript.load(
        Transcript(
          agentId: 'a1',
          entries: [
            for (var i = 1; i <= 80; i++)
              Entry(
                seq: Int64(i),
                assistant: AssistantText(
                  text:
                      'Reply r$i says enough to fill a line or two of '
                      'the conversation, from start$i to finish$i.',
                ),
              ),
          ],
        ),
      );
      await harness.show(tester, size: const Size(1400, 1100));
      await pumpFrames(tester);
      // Back up through it, far from where it ends.
      final list = tester
          .stateList<ScrollableState>(find.byType(Scrollable))
          .firstWhere(
            (state) =>
                state.position.maxScrollExtent > 0 ||
                state.position.minScrollExtent < 0,
          );
      list.position.jumpTo(list.position.pixels - 1500);
      await pumpFrames(tester);
      final shown = [
        for (var i = 1; i <= 80; i++)
          if (find
              .textContaining('start$i ', findRichText: true)
              .evaluate()
              .isNotEmpty)
            i,
      ];
      final mid = shown[shown.length ~/ 2];
      final got = await _drag(
        tester,
        copied,
        'start$mid',
        'finish${mid + 1}',
        hold: true,
      );
      expect(got, startsWith('start$mid'));
      expect(got.trimRight(), endsWith('finish${mid + 1}'));
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  }
}
