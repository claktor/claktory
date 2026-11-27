// A Markdown file shown Formatted beside the conversation, rewritten by
// an agent: the viewer reads it again and its blocks
// land in other places (a paragraph now in a quote, a list, a code
// block). Each block's selection notifier went to the new place's
// listener while the old one still held it, and the pane turned into the
// debug red box ("This SelectionListenerNotifier is already registered
// to another SelectionListener").
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, showPanes;
import 'selection_test.dart' show watchClipboard;

const _path = '/w/mac-backup-plan.md';

FileContent _file(String body, String sha) => FileContent(
  path: _path,
  mime: 'text/markdown',
  data: body.codeUnits,
  size: Int64(body.length),
  sha256: sha,
);

/// The plan as first written, and as the agent rewrote it: the same
/// paragraphs, moved into a quote, a list and a code block, and some new.
const _versions = [
  '# Mac backup plan\n\n'
      'Back up the vault every night.\n\n'
      'Keep three copies.\n\n'
      '## Steps\n\n'
      'Copy it to the disk.\n\n'
      'Check the copy.\n',
  '> Back up the vault every night.\n\n'
      '- Keep three copies.\n'
      '- Copy it to the disk.\n\n'
      '```\nCheck the copy.\n```\n\n'
      '# Mac backup plan\n\n'
      'Restore it once a month to be sure.\n',
  'Mac backup plan, short:\n\n'
      '- Back up the vault every night.\n',
];

/// The words of [version]'s last line, without what marks it up.
String _plainLast(String version) =>
    version.trim().split('\n').last.replaceAll(RegExp(r'^[-#>` ]+'), '');

/// Drags across the paragraph holding [words], from its first letter to
/// its last, and copies: what was copied.
Future<String> _copyDrag(
  WidgetTester tester,
  List<String> copied,
  String words,
) async {
  final paragraph = tester.allRenderObjects
      .whereType<RenderParagraph>()
      .firstWhere((each) => each.text.toPlainText().contains(words));
  final text = paragraph.text.toPlainText();
  final at = text.indexOf(words);
  Offset point(int offset, {bool end = false}) {
    final box = paragraph
        .getBoxesForSelection(
          TextSelection(baseOffset: offset, extentOffset: offset + 1),
        )
        .first
        .toRect();
    return paragraph.localToGlobal(
      end
          ? box.centerRight - const Offset(1, 0)
          : box.centerLeft + const Offset(1, 0),
    );
  }

  final gesture = await tester.startGesture(
    point(at),
    kind: PointerDeviceKind.mouse,
  );
  await tester.pump();
  final end = point(at + words.length - 1, end: true);
  for (var i = 1; i <= 5; i++) {
    await gesture.moveTo(Offset.lerp(point(at), end, i / 5)!);
    await tester.pump();
  }
  await gesture.up();
  await tester.pump();
  await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
  await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
  await tester.pump();
  return copied.lastOrNull ?? '';
}

void main() {
  for (final (from, to) in [(0, 1), (1, 0), (1, 2), (2, 1)]) {
    testWidgets('rewritten while shown (version ${from + 1} to ${to + 1}), '
        'it shows the new text, and a selection in it still copies', (
      tester,
    ) async {
      final copied = watchClipboard(tester);
      final harness = await showPanes(
        tester,
        beside('text', _path),
        files: {_path: _file(_versions[from], 'v$from')},
      );
      await settle(tester);
      expect(tester.takeException(), isNull);
      final first = _plainLast(_versions[from]);
      expect(await _copyDrag(tester, copied, first), contains(first));

      harness.backend.files[_path] = _file(_versions[to], 'v$to');
      harness.backend.fileChanged.value = FileChanged(
        path: _path,
        sha256: 'v$to',
        byAgentId: 'a1',
      );
      await pumpFrames(tester);
      final last = _versions[to]
          .trim()
          .split('\n')
          .last
          .replaceAll(RegExp(r'^[-#>` ]+'), '');
      expect(find.textContaining(last, findRichText: true), findsWidgets);

      copied.clear();
      expect(await _copyDrag(tester, copied, last), contains(last));
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  }

  testWidgets("a selection made before a rewrite goes with it: it never "
      "stays on other words or skips some (Abbacchio's row 1012)", (
    tester,
  ) async {
    const plain =
        'Alpha paragraph one.\n\n'
        'Beta paragraph two with more words.\n\n'
        'Gamma paragraph three.\n\n'
        'Variant PLAIN\n';
    const mixed =
        '# Title\n\n'
        'Alpha paragraph one.\n\n'
        '> Beta paragraph two with more words.\n\n'
        '- Gamma paragraph three.\n\n'
        '```\ncode line\n```\n\n'
        'Variant MIXED\n';
    final copied = watchClipboard(tester);
    final harness = await showPanes(
      tester,
      beside('text', _path),
      files: {_path: _file(plain, 'p')},
    );
    await settle(tester);
    RenderParagraph paragraph(String words) => tester.allRenderObjects
        .whereType<RenderParagraph>()
        .firstWhere((each) => each.text.toPlainText().contains(words));
    // From Alpha's first letter to Variant's last.
    final from = paragraph('Alpha').localToGlobal(const Offset(1, 8));
    final variant = paragraph('Variant');
    final to = variant.localToGlobal(
      Offset(variant.getMaxIntrinsicWidth(double.infinity) - 1, 8),
    );
    final gesture = await tester.startGesture(
      from,
      kind: PointerDeviceKind.mouse,
    );
    await tester.pump();
    for (var i = 1; i <= 8; i++) {
      await gesture.moveTo(Offset.lerp(from, to, i / 8)!);
      await tester.pump();
    }
    await gesture.up();
    await tester.pump();

    Future<String> copy() async {
      copied.clear();
      await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
      await tester.pump();
      return copied.lastOrNull ?? '';
    }

    expect(await copy(), allOf(contains('Alpha'), contains('Variant')));
    for (final (version, sha) in [(mixed, 'm'), (plain, 'p2')]) {
      harness.backend.files[_path] = _file(version, sha);
      harness.backend.fileChanged.value = FileChanged(
        path: _path,
        sha256: sha,
        byAgentId: 'a1',
      );
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      expect(await copy(), isEmpty, reason: sha);
    }
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
}
