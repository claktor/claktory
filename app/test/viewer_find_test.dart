// Find in a file's viewer (⌘F): the text shown (formatted or plain) is
// searched here, each occurrence in turn, brought into sight and marked.
import 'dart:convert';

import 'package:factory_app/find.dart';
import 'package:factory_app/loopback_forward.dart' show linkLauncher;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, showPanes;
import 'phone_sizes_test.dart' show loadGeist;

FileContent _file(String path, String body, {String sha = 'v1'}) => FileContent(
  path: path,
  mime: path.endsWith('.md') ? 'text/markdown' : 'text/plain',
  size: Int64(utf8.encode(body).length),
  data: utf8.encode(body),
  sha256: sha,
);

/// The washed words in [text]'s rich texts: (the words, whether it's the
/// match gone to).
List<(String, bool)> _washes(WidgetTester tester, Finder text) {
  final found = <(String, bool)>[];
  for (final element in text.evaluate()) {
    final widget = element.widget;
    if (widget is! RichText) continue;
    widget.text.visitChildren((span) {
      final background = span.style?.backgroundColor;
      if (span is TextSpan && background != null && span.text != null) {
        found.add((span.text!, background.a > 0.9));
      }
      return true;
    });
  }
  return found;
}

Finder _rich(String words) => find.byWidgetPredicate(
  (widget) => widget is RichText && widget.text.toPlainText().contains(words),
);

String _count(WidgetTester tester) =>
    tester.widget<FindBar>(find.byType(FindBar)).session.target.countLabel;

Future<void> _openFind(WidgetTester tester, Finder inside) async {
  // A click in the viewer makes it the pane ⌘F finds in.
  await tester.tapAt(tester.getCenter(inside));
  await tester.pump();
  await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
  await tester.sendKeyEvent(LogicalKeyboardKey.keyF);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
  await pumpFrames(tester);
}

Future<void> _type(WidgetTester tester, String words) async {
  final field = tester.widget<FindBar>(find.byType(FindBar)).session.field;
  final editable = find.byWidgetPredicate(
    (widget) => widget is EditableText && widget.focusNode == field,
  );
  await tester.enterText(editable, words);
  await tester.pump(const Duration(milliseconds: 200));
  await pumpFrames(tester, const Duration(milliseconds: 300));
}

void main() {
  testWidgets('plain text: each occurrence in turn, scrolled to and marked, '
      'the one gone to in full', (tester) async {
    final lines = [
      for (var i = 1; i <= 300; i++)
        switch (i) {
          10 => 'line ten has a Needle',
          250 => 'needle and needle again',
          _ => 'line $i',
        },
    ];
    await showPanes(
      tester,
      beside('text', '/w/notes.txt'),
      files: {'/w/notes.txt': _file('/w/notes.txt', lines.join('\n'))},
    );
    await settle(tester);
    await _openFind(tester, _rich('line ten'));
    expect(find.text('Search this file'), findsOneWidget);

    await _type(tester, 'needle');
    expect(_count(tester), '1 of 3');
    expect(_washes(tester, _rich('line ten')), [('Needle', true)]);

    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await pumpFrames(tester);
    expect(_count(tester), '2 of 3');
    // Line 250, far below, brought into sight.
    expect(_rich('needle and needle'), findsOneWidget);
    expect(_washes(tester, _rich('needle and needle')), [
      ('needle', true),
      ('needle', false),
    ]);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await pumpFrames(tester);
    expect(_count(tester), '3 of 3');
    expect(_washes(tester, _rich('needle and needle')), [
      ('needle', false),
      ('needle', true),
    ]);
    // Round to the first.
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await pumpFrames(tester);
    expect(_count(tester), '1 of 3');

    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await pumpFrames(tester);
    expect(find.byType(FindBar), findsNothing);
    expect(_washes(tester, _rich('line ten')), isEmpty);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets("markdown: what's rendered is found, formatted or plain", (
    tester,
  ) async {
    const essay =
        '# A **needle** title\n\nThe first needle.\n\n- a listed needle\n';
    await showPanes(
      tester,
      beside('text', '/w/essay.md'),
      files: {'/w/essay.md': _file('/w/essay.md', essay)},
    );
    await settle(tester);
    await tester.pump();
    await _openFind(tester, _rich('The first needle'));
    await _type(tester, 'NEEDLE');
    expect(_count(tester), '1 of 3');
    expect(_washes(tester, _rich('title')), [('needle', true)]);
    expect(_washes(tester, _rich('The first')), [('needle', false)]);

    // Plain: its source, with the asterisks.
    await tester.tap(find.text('Plain'));
    await pumpFrames(tester);
    expect(_count(tester), '1 of 3');
    expect(_washes(tester, _rich('# A **')), [('needle', true)]);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets('formatted: a match far along a wide table or code block is '
      'scrolled to sideways in its own scroll, and stays in sight shown '
      'Plain (Abbacchio row 652)', (tester) async {
    // Real widths: Plain's lines measure the monospaced font's.
    await tester.runAsync(loadGeist);
    final wide = 'x' * 300;
    final essay = [
      '# Notes',
      for (var i = 0; i < 120; i++) 'Paragraph $i.',
      '| $wide WIDEMARK | y |\n|---|---|\n| a | b |',
      '```\n$wide CODEMARK\n```',
    ].join('\n\n');
    await showPanes(
      tester,
      beside('text', '/w/wide.md'),
      files: {'/w/wide.md': _file('/w/wide.md', essay)},
    );
    await settle(tester);
    await tester.pump();

    Future<void> inSight(String mark) async {
      final paragraph = tester.renderObject<RenderParagraph>(_rich(mark).last);
      final text = paragraph.text.toPlainText();
      final at = text.indexOf(mark);
      final box = paragraph
          .getBoxesForSelection(
            TextSelection(baseOffset: at, extentOffset: at + mark.length),
          )
          .first
          .toRect()
          .shift(paragraph.localToGlobal(Offset.zero));
      Rect scroll(AxisDirection direction) {
        final box = tester.renderObject<RenderBox>(
          find
              .ancestor(
                of: _rich(mark).last,
                matching: find.byWidgetPredicate(
                  (widget) =>
                      widget is Scrollable && widget.axisDirection == direction,
                ),
              )
              .first,
        );
        return box.localToGlobal(Offset.zero) & box.size;
      }

      final across = scroll(AxisDirection.right);
      expect(box.left, greaterThanOrEqualTo(across.left), reason: mark);
      expect(box.right, lessThanOrEqualTo(across.right), reason: mark);
      final down = scroll(AxisDirection.down);
      expect(box.top, greaterThanOrEqualTo(down.top), reason: mark);
      expect(box.bottom, lessThanOrEqualTo(down.bottom), reason: mark);
    }

    await _openFind(tester, _rich('Paragraph 0.'));
    await _type(tester, 'WIDEMARK');
    expect(_count(tester), '1 of 1');
    await inSight('WIDEMARK');
    await _type(tester, 'CODEMARK');
    expect(_count(tester), '1 of 1');
    await inSight('CODEMARK');

    // Shown Plain, the find stays on it, in sight there too.
    await tester.tap(find.text('Plain'));
    await pumpFrames(tester);
    expect(_count(tester), '1 of 1');
    expect(_rich('CODEMARK'), findsOneWidget);
    await inSight('CODEMARK');

    // And back (Abbacchio's row 655).
    await tester.tap(find.text('Formatted'));
    await pumpFrames(tester);
    expect(_count(tester), '1 of 1');
    await inSight('CODEMARK');
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets("a formatted file that can't be commented on still opens its "
      'links, found in or not', (tester) async {
    final opened = <String>[];
    linkLauncher = (url) async {
      opened.add(url.toString());
      return true;
    };
    addTearDown(() => linkLauncher = null);
    await showPanes(
      tester,
      beside('text', '/w/links.md'),
      // No version: nothing to comment on.
      files: {
        '/w/links.md': _file(
          '/w/links.md',
          'See [the site](https://example.com/x) for more.',
          sha: '',
        ),
      },
    );
    await settle(tester);
    await tester.pump();
    final link = find.textContaining('the site', findRichText: true).last;
    // On "site", where the paragraph lays it out.
    final paragraph = tester.renderObject<RenderParagraph>(link);
    final caret = paragraph.getOffsetForCaret(
      const TextPosition(offset: 9),
      Rect.zero,
    );
    final at = paragraph.localToGlobal(
      caret + Offset(2, paragraph.size.height / 2),
    );
    await tester.tapAt(at);
    await pumpFrames(tester);
    expect(opened, ['https://example.com/x']);

    await _openFind(tester, link);
    expect(find.text('Search this file'), findsOneWidget);
    await _type(tester, 'site');
    expect(_count(tester), '1 of 1');
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await pumpFrames(tester);
    await tester.tapAt(at);
    await pumpFrames(tester);
    expect(opened, hasLength(greaterThanOrEqualTo(2)));
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
}
