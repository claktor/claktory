import 'package:cupertino_ui/cupertino_ui.dart'
    show DefaultCupertinoLocalizations;
import 'package:factory_app/markdown.dart' show markdownToPlain;
import 'package:factory_app/mobile/message_actions.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';
import 'selection_test.dart' show copyItem, rightClick, watchClipboard;

/// Copying out of a conversation keeps the text's shape: blank
/// lines between paragraphs, a line per list item with its marker, nested
/// items indented under theirs, a table's cells tab-separated.
void main() {
  final mac = TargetPlatformVariant.only(TargetPlatform.macOS);

  Future<List<String>> showReply(WidgetTester tester, String text) async {
    final copied = watchClipboard(tester);
    final harness = Harness(workspace(agents: [builder(working: false)]));
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(1),
            assistant: AssistantText(text: text),
          ),
        ],
      ),
    );
    await harness.show(tester);
    // Nested selection containers take in their text a frame per level.
    await pumpFrames(tester, const Duration(milliseconds: 100));
    return copied;
  }

  /// Everything selected (⌘A) and copied (⌘C), after a right click on
  /// [word] puts the keyboard in the conversation.
  Future<void> copyAll(WidgetTester tester, String word) async {
    await rightClick(tester, find.textContaining(word, findRichText: true));
    await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyA);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyC);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
    await tester.pump();
  }

  testWidgets('a paragraph pair keeps the blank line between them', (
    tester,
  ) async {
    final copied = await showReply(
      tester,
      'Hi Ana,\n\nThe **launch** is today.\n\nBest,\nMatt',
    );
    await copyAll(tester, 'launch');
    expect(copied.last, 'Hi Ana,\n\nThe launch is today.\n\nBest,\nMatt');
  }, variant: mac);

  testWidgets('a list copies a line per item, with its marker', (tester) async {
    final copied = await showReply(
      tester,
      'Today:\n\n- one\n- two\n\n1. first\n2. second',
    );
    await copyAll(tester, 'Today');
    expect(copied.last, 'Today:\n\n• one\n• two\n\n1. first\n2. second');
  }, variant: mac);

  testWidgets('nested items are indented under theirs', (tester) async {
    final copied = await showReply(
      tester,
      '- one\n- two\n  - nested\n  - again\n- three',
    );
    await copyAll(tester, 'three');
    expect(copied.last, '• one\n• two\n  • nested\n  • again\n• three');
  }, variant: mac);

  testWidgets("a word copied out of a list item doesn't get its marker", (
    tester,
  ) async {
    final copied = await showReply(tester, '- one\n- two');
    await rightClick(tester, find.textContaining('two', findRichText: true));
    await tester.tap(copyItem);
    await tester.pump();
    expect(copied.last, 'two');
  }, variant: mac);

  testWidgets("a table's cells are tab-separated, a row a line", (
    tester,
  ) async {
    final copied = await showReply(
      tester,
      '| Name | Role |\n| --- | --- |\n| Ana | Writer |\n| Kenji | Frontend |',
    );
    await copyAll(tester, 'Kenji');
    expect(copied.last, 'Name\tRole\nAna\tWriter\nKenji\tFrontend');
  }, variant: mac);

  testWidgets("on iPhone, Select text's Copy keeps the shape too", (
    tester,
  ) async {
    final copied = watchClipboard(tester);
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.view.physicalSize = const Size(393, 852);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    const text = 'Hi Ana,\n\n- one\n- two\n  - nested';
    await tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xFF000000),
        localizationsDelegates: const [
          DefaultCupertinoLocalizations.delegate,
          DefaultWidgetsLocalizations.delegate,
        ],
        builder: (context, _) => Overlay(
          initialEntries: [
            OverlayEntry(
              builder: (context) => const Center(
                child: MessageActions(text: text, child: Text('Hi Ana,')),
              ),
            ),
          ],
        ),
      ),
    );
    await tester.longPress(find.text('Hi Ana,'));
    await pumpFrames(tester);
    await tester.tap(find.text('Select text'));
    await pumpFrames(tester);
    final region = tester.state<SelectableRegionState>(
      find.byType(SelectableRegion).last,
    );
    region.selectAll(SelectionChangedCause.toolbar);
    await tester.pump();
    region.contextMenuButtonItems
        .firstWhere((item) => item.type == ContextMenuButtonType.copy)
        .onPressed!();
    await tester.pump();
    expect(copied.last, 'Hi Ana,\n\n• one\n• two\n  • nested');
  });

  test(
    'markdownToPlain gives what a selection of the rendered text copies',
    () {
      expect(
        markdownToPlain('Hi Ana,\n\nThe **launch** is today.\n\nBest,\nMatt'),
        'Hi Ana,\n\nThe launch is today.\n\nBest,\nMatt',
      );
      expect(
        markdownToPlain('Today:\n\n- one\n- two\n\n1. first\n2. second'),
        'Today:\n\n• one\n• two\n\n1. first\n2. second',
      );
      expect(
        markdownToPlain('- one\n- two\n  - nested\n  - again\n- three'),
        '• one\n• two\n  • nested\n  • again\n• three',
      );
      expect(
        markdownToPlain(
          '| Name | Role |\n| --- | --- |\n| Ana | Writer |\n| Kenji | Frontend |',
        ),
        'Name\tRole\nAna\tWriter\nKenji\tFrontend',
      );
      expect(
        markdownToPlain(
          'See [the docs](https://x.dev) and `cargo test`.\n\n'
          '```\nfn main() {}\n```\n\n> quoted\n\n---\n\n3. third\n4. fourth',
        ),
        'See the docs and cargo test.\n\nfn main() {}\n\nquoted\n\n'
        '3. third\n4. fourth',
      );
    },
  );

  const sample =
      '# Plan\n\nHi **Ana**, see [this](https://x.dev):\n\n'
      '1. first\n2. second\n   - nested `code`\n\n'
      '> a quote\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\nBest,\nMatt';

  testWidgets('markdownToPlain and a whole selection agree', (tester) async {
    final copied = await showReply(tester, sample);
    await copyAll(tester, 'Plan');
    expect(copied.last, markdownToPlain(sample));
  }, variant: mac);
}
