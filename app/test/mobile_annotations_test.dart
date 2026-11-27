// Viewers and comments on an iPhone (research mobile.md §4, "Comments and
// annotations"): a viewer's header of Back, its name, the comments' count
// and More, with Close everywhere there rather than a × beside Back;
// Comment and Suggest in the text's own menu; comments written in a sheet
// above the keyboard, Return sending them; a marker a finger's size,
// opening its thread in a sheet; the list of them with Send.
import 'dart:convert';

import 'package:cupertino_ui/cupertino_ui.dart'
    show CupertinoAdaptiveTextSelectionToolbar;
import 'package:factory_app/mobile/comment_editor.dart';
import 'package:factory_app/mobile/presentation.dart' show PhoneSheet;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart' show Sidebar;
import 'package:factory_app/viewers/text.dart' show TextViewer;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, snapshot;

const _essay = '''# On rivers

The first paragraph sets the scene.

The second paragraph makes the claim, and it is far too long.

The third one ends it.
''';

FileContent _file(String path, String body) => FileContent(
  path: path,
  mime: 'text/markdown',
  size: Int64(utf8.encode(body).length),
  data: utf8.encode(body),
  sha256: 'v1',
);

Thread _thread({bool draft = false}) {
  const exact = 'makes the claim';
  const rendered =
      'On rivers\nThe first paragraph sets the scene.\n'
      'The second paragraph ';
  final source = _essay.indexOf(exact);
  return Thread(
    id: 't1',
    agentId: 'a1',
    path: '/w/essay.md',
    kind: 'markdown',
    versionSha256: 'v1',
    status: ThreadStatus.THREAD_STATUS_OPEN,
    anchor: AnchorState.ANCHOR_STATE_ANCHORED,
    rendered: TextAnchor(
      start: rendered.length,
      end: rendered.length + exact.length,
      exact: exact,
    ),
    source: TextAnchor(start: source, end: source + exact.length, exact: exact),
    originalExact: exact,
    comments: [ThreadComment(id: 'c1', body: 'Lead with this.', draft: draft)],
  );
}

void main() {
  /// An iPhone 16 with the essay pushed over Builder's conversation.
  Future<Harness> essay(
    WidgetTester tester, {
    List<Thread> threads = const [],
  }) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(snapshot(layout: beside('text', '/w/essay.md')));
    harness.backend.files['/w/essay.md'] = _file('/w/essay.md', _essay);
    tester.view.padding = const FakeViewPadding(top: 59, bottom: 34);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    await tester.tap(find.bySemanticsLabel(RegExp('^More options for ')));
    await pumpFrames(tester);
    await tester.tap(find.text('essay.md').last);
    await pumpFrames(tester);
    harness.backend.threads.apply(
      ThreadList(path: '/w/essay.md', versionSha256: 'v1', threads: threads),
    );
    await pumpFrames(tester);
    await tester.pump();
    await tester.pump();
    expect(
      find.textContaining('The second paragraph', findRichText: true),
      findsWidgets,
    );
    return harness;
  }

  Iterable<ClientMessage> sent(
    Harness harness,
    bool Function(ClientMessage) of,
  ) => harness.backend.sent.where(of);

  testWidgets('the header: Back, the name, the comments and More; '
      'Close everywhere in More, asked first', (tester) async {
    final semantics = tester.ensureSemantics();
    final harness = await essay(tester, threads: [_thread()]);
    expect(find.text('essay.md'), findsWidgets);
    expect(find.bySemanticsLabel('Close'), findsNothing, reason: 'no ×');
    for (final label in ['Back', '1 comment', 'More']) {
      final size = tester.getSize(find.bySemanticsLabel(label).last);
      expect(size.width, greaterThanOrEqualTo(44), reason: label);
      expect(size.height, greaterThanOrEqualTo(44), reason: label);
    }
    await tester.tap(find.bySemanticsLabel('More').last);
    await pumpFrames(tester);
    expect(find.text('Plain text'), findsOneWidget);
    expect(find.text('Reload'), findsOneWidget);
    await tester.tap(find.text('Close everywhere'));
    await pumpFrames(tester);
    // Asked: it goes from every device.
    expect(find.textContaining('on every device'), findsOneWidget);
    expect(sent(harness, (m) => m.hasSetLayout()), isEmpty);
    await tester.tap(find.text('Close everywhere').last);
    await pumpFrames(tester);
    expect(sent(harness, (m) => m.hasSetLayout()), hasLength(1));
    semantics.dispose();
  });

  testWidgets('Back leaves the layout as it is', (tester) async {
    final harness = await essay(tester);
    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester);
    expect(sent(harness, (m) => m.hasSetLayout()), isEmpty);
    expect(
      find.textContaining('The second paragraph', findRichText: true),
      findsNothing,
    );
  });

  /// Selects a word of the second paragraph as a finger does, and the
  /// iPhone's menu over it.
  Future<CupertinoAdaptiveTextSelectionToolbar> select(
    WidgetTester tester,
  ) async {
    final paragraph = find
        .textContaining('The second paragraph', findRichText: true)
        .first;
    await tester.longPressAt(
      tester.getRect(paragraph).topLeft + const Offset(140, 10),
    );
    await pumpFrames(tester);
    await tester.pump();
    return tester.widget<CupertinoAdaptiveTextSelectionToolbar>(
      find.byType(CupertinoAdaptiveTextSelectionToolbar),
    );
  }

  testWidgets("Comment from the text's own menu writes in a sheet above the "
      'keyboard; Return sends it', (tester) async {
    final harness = await essay(tester);
    final menu = await select(tester);
    final labels = [for (final item in menu.buttonItems!) item.label];
    expect(labels, containsAll(['Comment', 'Suggest']));
    expect(
      menu.buttonItems!.map((item) => item.type),
      contains(ContextMenuButtonType.copy),
      reason: "the system's own first",
    );
    menu.buttonItems!
        .firstWhere((item) => item.label == 'Comment')
        .onPressed!();
    await pumpFrames(tester);

    final sheet = find.byType(PhoneSheet);
    expect(sheet, findsOneWidget);
    expect(find.byType(QuotedPassage), findsOneWidget);
    expect(find.text('To Builder'), findsOneWidget);
    final field = find.descendant(
      of: sheet,
      matching: find.byType(EditableText),
    );
    final editable = tester.widget<EditableText>(field);
    expect(editable.focusNode.hasFocus, isTrue);
    expect(editable.textInputAction, TextInputAction.send);
    // The keyboard up: the field stays over it.
    tester.view.viewInsets = const FakeViewPadding(bottom: 336);
    addTearDown(tester.view.resetViewInsets);
    await pumpFrames(tester);
    expect(tester.getRect(field).bottom, lessThan(852 - 336));

    await tester.enterText(field, 'Too long.');
    await tester.testTextInput.receiveAction(TextInputAction.send);
    await pumpFrames(tester);
    final create = sent(
      harness,
      (m) => m.hasCreateThread(),
    ).single.createThread;
    expect(create.body, 'Too long.');
    expect(create.sendNow, isTrue);
    expect(create.path, '/w/essay.md');
    expect(find.byType(PhoneSheet), findsNothing, reason: 'it closes');
  });

  testWidgets('Suggest starts from the text, and a sheet with words asks '
      'before it goes', (tester) async {
    await essay(tester);
    final menu = await select(tester);
    menu.buttonItems!
        .firstWhere((item) => item.label == 'Suggest')
        .onPressed!();
    await pumpFrames(tester);
    expect(find.text('Suggest a change'), findsOneWidget);
    final fields = find.descendant(
      of: find.byType(PhoneSheet),
      matching: find.byType(EditableText),
    );
    expect(fields, findsNWidgets(2));
    await tester.enterText(fields.first, 'states the point');
    await pumpFrames(tester);
    // Closed with a change written: asked.
    await tester.tap(find.bySemanticsLabel('Close').last);
    await pumpFrames(tester);
    expect(find.text('Discard this comment?'), findsOneWidget);
    await tester.tap(find.text('Keep editing'));
    await pumpFrames(tester);
    expect(find.text('Suggest a change'), findsOneWidget);
    await tester.tap(find.bySemanticsLabel('Close').last);
    await pumpFrames(tester);
    await tester.tap(find.text('Discard'));
    await pumpFrames(tester);
    expect(find.text('Suggest a change'), findsNothing);
  });

  testWidgets("a marker is a finger's size and opens its thread in a sheet; "
      'Reply writes in one', (tester) async {
    final semantics = tester.ensureSemantics();
    final harness = await essay(tester, threads: [_thread()]);
    final marker = find.bySemanticsLabel('Comment for Builder');
    expect(tester.getSize(marker), const Size(44, 44));
    await tester.tap(marker);
    await pumpFrames(tester);
    expect(find.byType(PhoneSheet), findsOneWidget);
    expect(find.text('Lead with this.'), findsOneWidget);
    await tester.tap(find.text('Reply'));
    await pumpFrames(tester);
    expect(find.text('Reply'), findsWidgets);
    final field = find.descendant(
      of: find.byType(PhoneSheet).last,
      matching: find.byType(EditableText),
    );
    await tester.enterText(field, 'Agreed.');
    await tester.testTextInput.receiveAction(TextInputAction.send);
    await pumpFrames(tester);
    final reply = sent(harness, (m) => m.hasReplyThread()).single.replyThread;
    expect(reply.threadId, 't1');
    expect(reply.body, 'Agreed.');
    semantics.dispose();
  });

  testWidgets("at the largest text, a comment's Edit and Delete go under who "
      'wrote it', (tester) async {
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final thread = _thread();
    thread.comments.first.createdMs = Int64(
      DateTime.now().millisecondsSinceEpoch,
    );
    await essay(tester, threads: [thread]);
    final marker = find.bySemanticsLabel('Comment for Builder');
    for (var n = 0; n < 12 && tester.getCenter(marker).dy > 700; n++) {
      await tester.drag(find.byType(TextViewer), const Offset(0, -200));
      await pumpFrames(tester);
    }
    await tester.tap(marker);
    await pumpFrames(tester);
    final sheet = find.byType(PhoneSheet);
    Rect inSheet(String text) => tester.getRect(
      find.descendant(of: sheet, matching: find.text(text)).first,
    );
    final you = inSheet('You');
    expect(inSheet('Edit').top, greaterThanOrEqualTo(you.bottom));
    expect(inSheet('Delete').top, greaterThanOrEqualTo(you.bottom));
    // "just now" under it on one line, not a letter a line.
    expect(inSheet('just now').height, lessThan(you.height * 1.5));
    expect(tester.takeException(), isNull);
  });

  testWidgets('the count lists them, with Send for those kept to send', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    final harness = await essay(tester, threads: [_thread(draft: true)]);
    await tester.tap(find.bySemanticsLabel('1 comment'));
    await pumpFrames(tester);
    expect(find.text('Comments'), findsWidgets);
    expect(find.text('Lead with this.'), findsOneWidget);
    await tester.tap(find.text('Send 1 comment to Builder'));
    await pumpFrames(tester);
    expect(sent(harness, (m) => m.hasSendComments()), hasLength(1));
    semantics.dispose();
  });

  testWidgets('the Mac keeps its popover and its header', (tester) async {
    final harness = Harness(snapshot(layout: beside('text', '/w/essay.md')));
    harness.backend.files['/w/essay.md'] = _file('/w/essay.md', _essay);
    await harness.show(tester);
    await pumpFrames(tester);
    final more = find.bySemanticsLabel('More');
    expect(
      more.evaluate().length,
      find
          .descendant(of: find.byType(Sidebar), matching: more)
          .evaluate()
          .length,
      reason: "only the sidebar's More, no header's",
    );
    expect(find.text('Formatted'), findsOneWidget);
  });
}
