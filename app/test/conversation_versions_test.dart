// Going back in an agent's conversation (friend bug #9): "Go back to
// here…" in a message's menu (the right click on a Mac and an iPad, the
// touch and hold on a phone) asks first, then asks Claktory to take the
// conversation back to that message; the person's own message goes back
// into the message box. The header lists the other versions, to switch
// to one or delete it.
import 'package:factory_app/controls.dart' show GlyphButton;
import 'package:factory_app/conversation_versions.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/transcript.dart' show TranscriptView;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart' show LogicalKeyboardKey;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'events_test.dart' show writer;
import 'harness.dart';
import 'selection_test.dart' show rightClick;

final _at = DateTime(2026, 11, 23, 14, 2).millisecondsSinceEpoch;

Entry _entry(int seq) =>
    Entry(seq: Int64(seq), timestampMs: Int64(_at + seq * 60000));

Harness _harness({bool working = false, int branches = 0}) {
  final harness = Harness(
    workspace(
      agents: [
        builder(working: working)..branches = branches,
        writer(),
      ],
    )..selectedAgentId = 'a1',
  );
  harness.backend.transcript.load(
    Transcript(
      agentId: 'a1',
      entries: [
        _entry(1)..user = UserText(text: 'Hello there'),
        _entry(2)..assistant = AssistantText(text: 'Hi, what next?'),
        _entry(3)..user = UserText(text: 'Look at the scheduler again'),
        _entry(4)..assistant = AssistantText(text: 'Looking at it now.'),
        _entry(5)
          ..event = Event(
            source: EventSource.EVENT_SOURCE_AGENT,
            from: 'Writer (Docs)',
            fromId: 'a2',
            text: 'Docs are done',
          ),
        _entry(6)..assistant = AssistantText(text: 'All shipped.'),
      ],
    ),
  );
  return harness;
}

Finder _message(String words) => find
    .descendant(
      of: find.byType(TranscriptView),
      matching: find.textContaining(words, findRichText: true),
    )
    .first;

Future<Harness> _show(
  WidgetTester tester, {
  Size size = const Size(1200, 900),
  bool working = false,
  int branches = 0,
}) async {
  final harness = _harness(working: working, branches: branches);
  await harness.show(tester, size: size);
  await settle(tester);
  await pumpFrames(tester);
  return harness;
}

Future<TestGesture> _mouse(WidgetTester tester) async {
  final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
  await mouse.addPointer(location: const Offset(1, 1));
  addTearDown(mouse.removePointer);
  return mouse;
}

/// Right-clicks [words]' message with the pointer over it, as a Mac does.
Future<void> _menuOn(
  WidgetTester tester,
  TestGesture mouse,
  String words,
) async {
  final at = tester.getCenter(_message(words));
  await mouse.moveTo(at);
  await tester.pump();
  await rightClick(tester, _message(words), at: at);
  await pumpFrames(tester);
}

const _goBack = 'Go back to here…';

void main() {
  final mac = TargetPlatformVariant.only(TargetPlatform.macOS);

  testWidgets('your message: asks first, goes back to it, and puts it in '
      'the box to change and send again', (tester) async {
    final harness = await _show(tester);
    final mouse = await _mouse(tester);
    await _menuOn(tester, mouse, 'Look at the scheduler');
    // After Reply and Copy message.
    expect(
      tester.getRect(find.text(_goBack)).top,
      greaterThan(tester.getRect(find.text('Copy message')).top),
    );
    await tester.tap(find.text(_goBack));
    await pumpFrames(tester);
    expect(
      find.text("Take Builder's conversation back to here?"),
      findsOneWidget,
    );
    expect(
      find.textContaining('Your message goes back into the message box'),
      findsOneWidget,
    );
    expect(find.textContaining('stops first'), findsNothing);
    await tester.tap(find.text('Go back'));
    await pumpFrames(tester);
    final fork = harness.backend.sent.last.forkConversation;
    expect(fork.agentId, 'a1');
    expect(fork.seq, Int64(3));
    expect(fork.timestampMs, Int64(_at + 3 * 60000));
    expect(harness.drafts.of('a1'), 'Look at the scheduler again');
    expect(
      find.text("Take Builder's conversation back to here?"),
      findsNothing,
    );
  }, variant: mac);

  testWidgets("an agent's reply or event: no message back in the box; a "
      'working agent stops first', (tester) async {
    final harness = await _show(tester, working: true);
    final mouse = await _mouse(tester);
    for (final words in ['Looking at it', 'Docs are done']) {
      await _menuOn(tester, mouse, words);
      await tester.tap(find.text(_goBack));
      await pumpFrames(tester);
      expect(find.textContaining('Builder stops first.'), findsOneWidget);
      expect(find.textContaining('Your message goes back'), findsNothing);
      await tester.tap(find.text('Go back'));
      await pumpFrames(tester);
    }
    expect([
      for (final sent in harness.backend.sent) sent.forkConversation.seq,
    ], containsAll([Int64(4), Int64(5)]));
    expect(harness.drafts.of('a1'), isEmpty);
  }, variant: mac);

  testWidgets('not offered where it would set nothing aside or keep '
      'nothing', (tester) async {
    await _show(tester);
    final mouse = await _mouse(tester);
    // The last reply, with nothing after it yet; the very first message.
    for (final words in ['All shipped', 'Hello there']) {
      await _menuOn(tester, mouse, words);
      expect(find.text('Copy message'), findsOneWidget, reason: words);
      expect(find.text(_goBack), findsNothing, reason: words);
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await pumpFrames(tester);
    }
  }, variant: mac);

  testWidgets("Claktory's refusal shows in the dialog, which stays", (
    tester,
  ) async {
    final harness = await _show(tester);
    final mouse = await _mouse(tester);
    harness.backend.refuse = 'The conversation changed; try again.';
    await _menuOn(tester, mouse, 'Look at the scheduler');
    await tester.tap(find.text(_goBack));
    await pumpFrames(tester);
    await tester.tap(find.text('Go back'));
    await pumpFrames(tester);
    expect(find.text('The conversation changed; try again.'), findsOneWidget);
    expect(
      find.text("Take Builder's conversation back to here?"),
      findsOneWidget,
    );
    expect(harness.drafts.of('a1'), isEmpty);
  }, variant: mac);

  testWidgets("on a phone, in the message's touch and hold", (tester) async {
    final harness = await _show(tester, size: const Size(390, 844));
    await tester.longPress(_message('Looking at it'));
    await pumpFrames(tester);
    expect(find.text('Select text'), findsOneWidget);
    await tester.tap(find.text(_goBack));
    await pumpFrames(tester);
    await tester.tap(find.text('Go back'));
    await pumpFrames(tester);
    expect(harness.backend.sent.last.forkConversation.seq, Int64(4));
    // Not on the last reply.
    await tester.longPress(_message('All shipped'));
    await pumpFrames(tester);
    expect(find.text('Select text'), findsOneWidget);
    expect(find.text(_goBack), findsNothing);
  }, variant: TargetPlatformVariant.only(TargetPlatform.iOS));

  testWidgets('the header shows other versions while there are any; switch '
      'and delete each ask first', (tester) async {
    final harness = await _show(tester, branches: 2);
    harness.backend.branches['a1'] = [
      Branch(
        id: 'b1',
        label: 'Look at the scheduler again',
        setAsideMs: Int64(DateTime.now().millisecondsSinceEpoch),
        forkedAtMs: Int64(_at),
        messages: 46,
      ),
      Branch(id: 'b2', forkedAtMs: Int64(_at), messages: 1),
    ];
    // In the header's ⋯ (agent-view.md §4).
    await _menu(tester);
    final row = find.text('Other versions (2)');
    expect(row, findsOneWidget);
    await tester.tap(row);
    await pumpFrames(tester);
    expect(find.text("Builder's other versions"), findsOneWidget);
    expect(find.text('Look at the scheduler again'), findsWidgets);
    expect(find.text('From 14:02'), findsOneWidget);
    expect(find.textContaining('· 46 messages'), findsOneWidget);
    expect(find.textContaining('· 1 message'), findsOneWidget);

    await tester.tap(find.text('Delete…').last);
    await pumpFrames(tester);
    expect(find.text('Delete this version?'), findsOneWidget);
    await tester.tap(find.text('Delete'));
    await pumpFrames(tester);
    expect(harness.backend.sent.any((m) => m.hasDeleteBranch()), isTrue);
    expect(
      harness.backend.sent.lastWhere((m) => m.hasDeleteBranch()).deleteBranch,
      DeleteBranch(agentId: 'a1', branchId: 'b2'),
    );

    await tester.tap(find.text('Switch to this version').first);
    await pumpFrames(tester);
    expect(find.text('Switch Builder to this version?'), findsOneWidget);
    await tester.tap(find.text('Switch'));
    await pumpFrames(tester);
    expect(
      harness.backend.sent.lastWhere((m) => m.hasSwitchBranch()).switchBranch,
      SwitchBranch(agentId: 'a1', branchId: 'b1'),
    );
    expect(find.text("Builder's other versions"), findsNothing);
  }, variant: mac);

  testWidgets('the list follows a change made elsewhere', (tester) async {
    final harness = await _show(tester, branches: 1);
    harness.backend.branches['a1'] = [
      Branch(id: 'b1', forkedAtMs: Int64(_at), messages: 3),
    ];
    await _menu(tester);
    await tester.tap(find.text('Other versions (1)'));
    await pumpFrames(tester);
    expect(find.text('Switch to this version'), findsOneWidget);
    int asked() =>
        harness.backend.sent.where((m) => m.hasListBranches()).length;
    final before = asked();
    // Another device went back again: two now.
    harness.backend.branches['a1']!.add(
      Branch(id: 'b2', forkedAtMs: Int64(_at), messages: 5),
    );
    final now = harness.backend.workspace.workspace.deepCopy();
    now.agents.firstWhere((a) => a.id == 'a1').branches = 2;
    await harness.apply(tester, now);
    await tester.pump(const Duration(milliseconds: 400));
    await pumpFrames(tester);
    expect(asked(), before + 1);
    expect(find.text('Switch to this version'), findsNWidgets(2));
  }, variant: mac);

  for (final scale in [1.0, 3.1]) {
    testWidgets("on a phone, Versions in the agent's ⋯ after Search, with "
        'its count (Versions (2)…), at 320 points and text ×$scale', (
      tester,
    ) async {
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final harness = await _show(
        tester,
        size: const Size(320, 568),
        branches: 2,
      );
      harness.backend.branches['a1'] = [
        Branch(id: 'b1', forkedAtMs: Int64(_at), messages: 3),
      ];
      await tester.tap(
        find.bySemanticsLabel(RegExp('^More options for ')).first,
      );
      await pumpFrames(tester);
      final row = find.text('Versions (2)…');
      expect(row, findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.tap(row);
      await pumpFrames(tester);
      expect(find.text("Builder's other versions"), findsOneWidget);
      expect(find.text('Switch to this version'), findsOneWidget);
      expect(tester.takeException(), isNull);
    }, variant: TargetPlatformVariant.only(TargetPlatform.iOS));
  }

  testWidgets('no versions, no row', (tester) async {
    await _show(tester);
    await _menu(tester);
    expect(find.textContaining('Other versions'), findsNothing);
  }, variant: mac);

  test("your message's files come back into the box; a voice message's "
      'words without its recording', () async {
    final harness = _harness();
    final backend = harness.backend;
    backend.files['/u/plan.txt'] = FileContent(
      path: '/u/plan.txt',
      size: Int64(4),
      data: 'plan'.codeUnits,
    );
    putBack(
      backend,
      harness.drafts,
      'a1',
      UserText(
        text: 'See the plan',
        attachments: [Attachment(path: '/u/plan.txt', name: 'plan.txt')],
      ),
    );
    expect(harness.drafts.of('a1'), 'See the plan');
    for (var i = 0; i < 100 && backend.attachments.of('a1').isEmpty; i++) {
      await Future<void>.delayed(const Duration(milliseconds: 50));
    }
    expect(
      [for (final f in backend.attachments.of('a1')) f.name],
      ['plan.txt'],
    );

    putBack(
      backend,
      harness.drafts,
      'a1',
      UserText(
        text: 'Said aloud',
        voice: Transcription(),
        attachments: [Attachment(path: '/u/voice.m4a', name: 'voice.m4a')],
      ),
    );
    expect(harness.drafts.of('a1'), 'See the plan\n\nSaid aloud');
    expect(
      backend.sent.where((m) => m.readFile.path == '/u/voice.m4a'),
      isEmpty,
    );
  });

  test('when a version was set aside', () {
    final now = DateTime(2026, 11, 23, 16);
    expect(
      setAsideAt(DateTime(2026, 11, 23, 15, 31).millisecondsSinceEpoch, now),
      'at 15:31',
    );
    expect(
      setAsideAt(DateTime(2026, 11, 17, 9, 5).millisecondsSinceEpoch, now),
      'on Tue 17 Nov at 09:05',
    );
  });
}

/// Opens the agent's ⋯.
Future<void> _menu(WidgetTester tester) async {
  await tester.tap(
    find.byWidgetPredicate(
      (widget) =>
          widget is GlyphButton && widget.label.startsWith('More options for '),
    ),
  );
  await pumpFrames(tester);
}
