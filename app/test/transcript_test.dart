import 'dart:async';

import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/outbox.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/transcript.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'outbox_fake.dart';

const windowHeight = 600.0;

Entry user(int seq) => Entry(
  seq: Int64(seq),
  user: UserText(text: 'message $seq'),
);

TranscriptStore transcriptOf(int count) => TranscriptStore()
  ..load(
    Transcript(
      agentId: 'a',
      entries: [for (var i = 0; i < count; i++) user(i)],
    ),
  );

Future<void> show(
  WidgetTester tester,
  TranscriptStore store,
  Listenable follow, {
  QueuedStore? queued,
  Outbox? outbox,
  Future<void> Function()? loadEarlier,
}) async {
  tester.view.physicalSize = const Size(900, windowHeight);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(
    WidgetsApp(
      color: const Color(0xFF000000),
      builder: (context, _) => Overlay(
        initialEntries: [
          OverlayEntry(
            builder: (context) => TranscriptView(
              transcript: store,
              working: false,
              follow: follow,
              queued: queued,
              outbox: outbox,
              loadEarlier: loadEarlier,
              agentId: 'a',
            ),
          ),
        ],
      ),
    ),
  );
  await tester.pumpAndSettle();
}

bool onScreen(WidgetTester tester, String text) {
  final found = find.text(text);
  if (found.evaluate().isEmpty) return false;
  final rect = tester.getRect(found);
  return rect.top >= 0 && rect.bottom <= windowHeight;
}

void main() {
  testWidgets('a long transcript opens at its end without building the rest', (
    tester,
  ) async {
    final store = transcriptOf(500);
    await show(tester, store, ValueNotifier(0));

    expect(onScreen(tester, 'message 499'), isTrue);
    expect(find.text('message 0'), findsNothing);
    expect(find.text('message 400'), findsNothing);
  });

  testWidgets('new and growing entries are followed at the bottom', (
    tester,
  ) async {
    final store = transcriptOf(100);
    await show(tester, store, ValueNotifier(0));

    store.update(EntryUpdate(agentId: 'a', entry: user(100)));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    expect(onScreen(tester, 'message 100'), isTrue);

    store.update(
      EntryUpdate(
        agentId: 'a',
        entry: Entry(seq: Int64(101), assistant: AssistantText()),
      ),
    );
    for (var i = 0; i < 40; i++) {
      store.delta(
        EntryDelta(agentId: 'a', seq: Int64(101), text: 'line $i\n\n'),
      );
    }
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    expect(onScreen(tester, 'line 39'), isTrue);
  });

  testWidgets('scrolled up, growth below leaves the view where it is', (
    tester,
  ) async {
    final store = transcriptOf(100);
    final follow = ValueNotifier(0);
    await show(tester, store, follow);

    await tester.drag(find.byType(CustomScrollView), const Offset(0, 400));
    await tester.pumpAndSettle();
    final anchor = find.text('message 90');
    final before = tester.getTopLeft(anchor);

    store.update(
      EntryUpdate(
        agentId: 'a',
        entry: Entry(
          seq: Int64(100),
          assistant: AssistantText(text: 'start'),
        ),
      ),
    );
    for (var i = 0; i < 40; i++) {
      store.delta(EntryDelta(agentId: 'a', seq: Int64(100), text: '\n\nmore'));
    }
    await tester.pump();
    expect(tester.getTopLeft(anchor), before);

    // Asking to follow again (as sending a message does) returns to the end.
    follow.value++;
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    expect(onScreen(tester, 'message 90'), isFalse);
    expect(find.textContaining('start'), findsOneWidget);
    expect(
      tester.getRect(find.textContaining('start')).bottom,
      lessThanOrEqualTo(windowHeight),
    );
  });

  testWidgets('a short transcript sits at the top and does not scroll', (
    tester,
  ) async {
    final store = transcriptOf(3);
    await show(tester, store, ValueNotifier(0));

    final top = tester.getTopLeft(find.text('message 0'));
    expect(top.dy, lessThan(100));

    await tester.drag(find.byType(CustomScrollView), const Offset(0, -300));
    await tester.pumpAndSettle();
    await tester.drag(find.byType(CustomScrollView), const Offset(0, 300));
    await tester.pumpAndSettle();
    expect(tester.getTopLeft(find.text('message 0')), top);
  });

  group('tool call rows say what the call does', () {
    ToolSummary summary(String name, String input) => toolSummary(name, input);

    test('bash: the command, and how many lines more', () {
      expect(
        summary(
          'bash',
          '{"timeout": 30, "command": "cargo test\\ncargo clippy"}',
        ),
        const ToolSummary('cargo test', more: '+1 line'),
      );
      expect(
        summary('bash', '{\n  "command": "ls -la"\n}'),
        const ToolSummary('ls -la'),
      );
    });

    test('while it is being written, what there is so far', () {
      expect(
        summary(
          'bash',
          '{"command": "cat > notes.md <<\'EOF\'\\nline one\\nli',
        ),
        const ToolSummary("cat > notes.md <<'EOF'", more: '+2 lines'),
      );
      expect(summary('bash', '{"comm'), ToolSummary.none);
      expect(
        summary('read_file', '{"path": "app/lib/ma'),
        const ToolSummary('app/lib/ma'),
      );
      expect(
        summary('checklist_update', '{"updates": [{"id": 3, "status": "do'),
        const ToolSummary('#3', code: false),
      );
      expect(
        summary('mystery', '{"x": "say \\"hi\\"\\u00e9'),
        const ToolSummary('say "hi"é'),
      );
    });

    test('files: the path', () {
      for (final tool in ['read_file', 'write_file', 'edit_file']) {
        expect(
          summary(tool, '{"path": "src/lib.rs", "content": "a\\nb"}').text,
          'src/lib.rs',
        );
      }
    });

    test('agents_send: to whom, and the start of the message', () {
      expect(
        summary(
          'agents_send',
          '{"to": "designer", "text": "The notes are ready.\\nMore below."}',
        ),
        const ToolSummary(
          '→ designer: The notes are ready.',
          more: '+1 line',
          code: false,
        ),
      );
    });

    test('checklist and memory: which items and what happens to them', () {
      expect(
        summary(
          'checklist_update',
          '{"updates": [{"id": 1, "status": "done"}, {"id": 4, "note": "x"}]}',
        ),
        const ToolSummary('#1 done, #4', code: false),
      );
      expect(
        summary(
          'checklist_add',
          '{"items": ["Fix the header", "Ship it", "Test"]}',
        ),
        const ToolSummary('Fix the header', more: '+2 more', code: false),
      );
      expect(
        summary('checklist_item', '{"id": 7}'),
        const ToolSummary('#7', code: false),
      );
      expect(
        summary(
          'memory_update',
          '{"ops": [{"op": "note_write", "slug": "release"}, {"op": "pref_remove", "id": 2}]}',
        ),
        const ToolSummary('note write release, pref remove #2', code: false),
      );
    });

    test('anything else: the first text argument', () {
      expect(
        summary('web_search', '{"limit": 3, "query": "flutter"}').text,
        'flutter',
      );
      expect(summary('mystery', '{"edits": [], "reason": "why"}').text, 'why');
      expect(summary('bash', ''), ToolSummary.none);
    });

    testWidgets('the row shows it, beside the tool name', (tester) async {
      final store = TranscriptStore()
        ..load(
          Transcript(
            agentId: 'a',
            entries: [
              Entry(
                seq: Int64(1),
                toolCall: ToolCall(
                  callId: 'c1',
                  name: 'bash',
                  input: '{"command": "cargo test --workspace\\necho done"}',
                  state: ToolCallState.TOOL_CALL_STATE_DONE,
                ),
              ),
            ],
          ),
        );
      await show(tester, store, ValueNotifier(0));
      expect(find.text('cargo test --workspace'), findsOneWidget);
      expect(find.text('+1 line'), findsOneWidget);
      double top(String text) => tester.getTopLeft(find.text(text)).dy;
      expect((top('bash') - top('cargo test --workspace')).abs(), lessThan(4));
    });
  });

  group('arrivals', () {
    double offset(WidgetTester tester) =>
        tester.state<ScrollableState>(find.byType(Scrollable)).position.pixels;
    double bottom(WidgetTester tester) => tester
        .state<ScrollableState>(find.byType(Scrollable))
        .position
        .maxScrollExtent;
    double opacityOf(WidgetTester tester, String text) => tester
        .widget<Opacity>(
          find
              .ancestor(of: find.text(text), matching: find.byType(Opacity))
              .first,
        )
        .opacity;

    testWidgets('a new entry glides in rather than jumping', (tester) async {
      final store = transcriptOf(100);
      await show(tester, store, ValueNotifier(0));
      final start = offset(tester);

      store.update(EntryUpdate(agentId: 'a', entry: user(100)));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 100));
      // Partway there, and the entry is still easing in.
      expect(offset(tester), greaterThan(start));
      expect(offset(tester), lessThan(bottom(tester)));
      expect(opacityOf(tester, 'message 100'), lessThan(1));

      await tester.pump(const Duration(milliseconds: 300));
      expect(offset(tester), bottom(tester));
      expect(opacityOf(tester, 'message 100'), 1);
    });

    testWidgets('a load lands at the bottom at once, nothing easing in', (
      tester,
    ) async {
      final store = transcriptOf(100);
      await show(tester, store, ValueNotifier(0));
      expect(offset(tester), bottom(tester));
      expect(opacityOf(tester, 'message 99'), 1);
    });

    testWidgets('streaming text is followed without a glide', (tester) async {
      final store = transcriptOf(100);
      store.update(
        EntryUpdate(
          agentId: 'a',
          entry: Entry(seq: Int64(100), assistant: AssistantText()),
        ),
      );
      await show(tester, store, ValueNotifier(0));
      store.delta(
        EntryDelta(agentId: 'a', seq: Int64(100), text: 'more\n\n' * 20),
      );
      await tester.pump();
      expect(offset(tester), bottom(tester), reason: 'same frame');
    });

    testWidgets('scrolling up during a glide stops it', (tester) async {
      final store = transcriptOf(100);
      await show(tester, store, ValueNotifier(0));
      store.update(EntryUpdate(agentId: 'a', entry: user(100)));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 50));
      await tester.drag(find.byType(CustomScrollView), const Offset(0, 300));
      await tester.pump();
      final held = offset(tester);
      await tester.pump(const Duration(milliseconds: 400));
      expect(offset(tester), held);
      expect(offset(tester), lessThan(bottom(tester)));
    });

    testWidgets('with reduced motion, new entries are just there', (
      tester,
    ) async {
      tester.platformDispatcher.accessibilityFeaturesTestValue =
          const FakeAccessibilityFeatures(disableAnimations: true);
      addTearDown(
        tester.platformDispatcher.clearAccessibilityFeaturesTestValue,
      );
      final store = transcriptOf(100);
      await show(tester, store, ValueNotifier(0));
      store.update(EntryUpdate(agentId: 'a', entry: user(100)));
      await tester.pump();
      expect(offset(tester), bottom(tester));
      expect(opacityOf(tester, 'message 100'), 1);
    });
  });

  group('queued', () {
    QueuedMessage person(int id, String text) =>
        QueuedMessage(id: Int64(id), source: 'person', text: text);
    QueuedMessages queue(List<QueuedMessage> messages) =>
        QueuedMessages(agentId: 'a', messages: messages);
    double opacityOf(WidgetTester tester, String text) {
      var opacity = 1.0;
      for (final element
          in find
              .ancestor(of: find.text(text), matching: find.byType(Opacity))
              .evaluate()) {
        opacity *= (element.widget as Opacity).opacity;
      }
      for (final element
          in find
              .ancestor(
                of: find.text(text),
                matching: find.byType(FadeTransition),
              )
              .evaluate()) {
        opacity *= (element.widget as FadeTransition).opacity.value;
      }
      return opacity;
    }

    testWidgets('waiting messages show at the end, faded and labeled', (
      tester,
    ) async {
      final store = transcriptOf(3);
      final queued = QueuedStore();
      await show(tester, store, ValueNotifier(0), queued: queued);
      queued.apply(
        queue([
          person(1, 'and then this'),
          QueuedMessage(
            id: Int64(2),
            source: 'agent',
            sender: 'Lead (App)',
            senderId: 'lead',
            text: 'a word from Lead',
          ),
          QueuedMessage(
            id: Int64(3),
            source: 'task',
            summary: 'Task build finished',
            text: 'long output',
          ),
        ]),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 400));
      expect(find.text('Queued'), findsNWidgets(3));
      expect(find.text('Lead (App)'), findsOneWidget);
      expect(find.text('Task build finished'), findsOneWidget);
      expect(opacityOf(tester, 'and then this'), 0.5);
      // After the last entry, the person's on the right.
      expect(
        tester.getTopLeft(find.text('and then this')).dy,
        greaterThan(tester.getTopLeft(find.text('message 2')).dy),
      );
      expect(
        tester.getCenter(find.text('and then this')).dx,
        greaterThan(tester.getCenter(find.text('a word from Lead')).dx),
      );
    });

    for (final entryFirst in [false, true]) {
      testWidgets('a delivered message brightens in place '
          '(${entryFirst ? 'entry' : 'queue'} first)', (tester) async {
        final store = transcriptOf(3);
        final queued = QueuedStore()..apply(queue([person(1, 'and more')]));
        await show(tester, store, ValueNotifier(0), queued: queued);
        expect(find.text('Queued'), findsOneWidget);
        final place = tester.getTopLeft(find.text('and more'));

        void deliver() => store.update(
          EntryUpdate(
            agentId: 'a',
            entry: Entry(
              seq: Int64(3),
              user: UserText(text: 'and more'),
            ),
          ),
        );
        if (entryFirst) {
          deliver();
          await tester.pump();
          queued.apply(queue([]));
        } else {
          queued.apply(queue([]));
          await tester.pump(const Duration(milliseconds: 100));
          expect(find.text('and more'), findsOneWidget, reason: 'waits');
          deliver();
        }
        await tester.pump();
        for (var i = 0; i < 6; i++) {
          expect(find.text('and more'), findsOneWidget);
          expect(opacityOf(tester, 'and more'), greaterThanOrEqualTo(0.5));
          await tester.pump(const Duration(milliseconds: 40));
        }
        await tester.pump(const Duration(seconds: 2));
        expect(find.text('Queued'), findsNothing);
        expect(opacityOf(tester, 'and more'), 1);
        expect(tester.getTopLeft(find.text('and more')).dy, place.dy);
      });
    }

    testWidgets('Stop clears the queue, and its bubbles fade away', (
      tester,
    ) async {
      final store = transcriptOf(3);
      final queued = QueuedStore()..apply(queue([person(1, 'never mind')]));
      await show(tester, store, ValueNotifier(0), queued: queued);
      queued.apply(queue([]));
      await tester.pump(const Duration(milliseconds: 500));
      expect(find.text('never mind'), findsOneWidget, reason: 'a moment');
      await tester.pump(const Duration(seconds: 2));
      await tester.pump(const Duration(milliseconds: 300));
      await tester.pump(const Duration(milliseconds: 300));
      expect(find.text('never mind'), findsNothing);
    });
  });

  group('earlier entries', () {
    Entry numbered(int seq) => Entry(
      seq: Int64(seq),
      user: UserText(text: 'message $seq'),
    );

    testWidgets('load as you scroll up, and what you see stays put', (
      tester,
    ) async {
      final store = TranscriptStore()
        ..load(
          Transcript(
            agentId: 'a',
            entries: [for (var i = 100; i < 160; i++) numbered(i)],
            hasEarlier: true,
          ),
        );
      final asked = <Completer<void>>[];
      await show(
        tester,
        store,
        ValueNotifier(0),
        loadEarlier: () {
          final done = Completer<void>();
          asked.add(done);
          return done.future;
        },
      );
      // At the bottom, far from the top: nothing asked yet.
      expect(asked, isEmpty);
      expect(onScreen(tester, 'message 159'), isTrue);

      // Up near the top: the earlier ones are asked for, once.
      await tester.drag(find.byType(CustomScrollView), const Offset(0, 1500));
      await tester.pump();
      await tester.pump();
      expect(asked, hasLength(1));
      await tester.drag(find.byType(CustomScrollView), const Offset(0, 50));
      await tester.pump();
      expect(asked, hasLength(1), reason: 'one at a time');
      final shown = [
        for (var i = 100; i < 160; i++)
          if (onScreen(tester, 'message $i')) i,
      ].first;
      final place = tester.getTopLeft(find.text('message $shown'));

      store.prepend('a', [
        for (var i = 70; i < 100; i++) numbered(i),
      ], hasEarlier: false);
      asked.single.complete();
      await tester.pump();
      await tester.pump();
      expect(tester.getTopLeft(find.text('message $shown')), place);
      expect(find.text('Loading earlier messages…'), findsNothing);

      // The earlier ones are there above.
      for (var i = 0; i < 4; i++) {
        await tester.drag(find.byType(CustomScrollView), const Offset(0, 3000));
        await tester.pump();
      }
      expect(find.text('message 70'), findsOneWidget);
      expect(asked, hasLength(1), reason: 'none left');
    });

    testWidgets("why they couldn't load, with Retry", (tester) async {
      final store = TranscriptStore()
        ..load(
          Transcript(agentId: 'a', entries: [numbered(100)], hasEarlier: true),
        );
      var asks = 0;
      // Too short to scroll: it loads at once.
      await show(
        tester,
        store,
        ValueNotifier(0),
        loadEarlier: () async {
          asks++;
          if (asks == 1) throw const RequestFailure('Not connected.');
          store.prepend('a', [numbered(99)], hasEarlier: false);
        },
      );
      expect(asks, 1);
      expect(
        find.text("Couldn't load earlier messages. Not connected."),
        findsOneWidget,
      );
      await tester.tap(find.text('Retry'));
      await tester.pump();
      await tester.pump();
      expect(asks, 2);
      expect(find.text('message 99'), findsOneWidget);
      expect(find.textContaining('earlier messages'), findsNothing);
    });

    test('an update older than what is held waits to be paged in', () {
      final store = TranscriptStore()
        ..load(
          Transcript(agentId: 'a', entries: [numbered(100)], hasEarlier: true),
        );
      store.update(EntryUpdate(agentId: 'a', entry: numbered(50)));
      expect(store.length, 1);
      store.update(EntryUpdate(agentId: 'a', entry: numbered(101)));
      expect(store.length, 2);
      store.prepend('a', [numbered(50), numbered(99)], hasEarlier: true);
      expect(
        [for (var i = 0; i < store.length; i++) store[i].seq],
        [50, 99, 100, 101],
      );
      expect(store.earlier, 2);
    });
  });

  group('sending', () {
    QueuedMessages queue(List<QueuedMessage> messages) =>
        QueuedMessages(agentId: 'a', messages: messages);
    QueuedMessage person(int id, String text) =>
        QueuedMessage(id: Int64(id), source: 'person', text: text);
    double opacityOf(WidgetTester tester, String text) {
      var opacity = 1.0;
      for (final element
          in find
              .ancestor(of: find.text(text), matching: find.byType(Opacity))
              .evaluate()) {
        opacity *= (element.widget as Opacity).opacity;
      }
      for (final element
          in find
              .ancestor(
                of: find.text(text),
                matching: find.byType(FadeTransition),
              )
              .evaluate()) {
        opacity *= (element.widget as FadeTransition).opacity.value;
      }
      return opacity;
    }

    /// An outbox whose sends wait for [answers], in order.
    (Outbox, List<Completer<void>>) outbox() {
      final backend = FakeOutboxBackend();
      return (Outbox(backend), backend.answers);
    }

    testWidgets('a message sent shows at once, and its queued one takes its '
        'place where it is', (tester) async {
      final store = transcriptOf(3);
      final queued = QueuedStore();
      final (box, answers) = outbox();
      await show(tester, store, ValueNotifier(0), queued: queued, outbox: box);

      box.send('a', 'ship it');
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 400));
      expect(find.text('ship it'), findsOneWidget);
      expect(find.text('Sending…'), findsOneWidget);
      expect(opacityOf(tester, 'ship it'), 0.5);
      final place = tester.getTopLeft(find.text('ship it'));

      // The backend queued it (the agent is working), then answered.
      queued.apply(queue([person(7, 'ship it')]));
      answers.single.complete();
      await tester.pump();
      expect(find.text('ship it'), findsOneWidget);
      expect(tester.getTopLeft(find.text('ship it')), place);
      expect(opacityOf(tester, 'ship it'), 0.5);
      expect(box.of('a'), isEmpty);
      // An idle agent takes it at once, so it doesn't flash "Queued".
      expect(find.text('Sending…'), findsOneWidget);
      await tester.pump(const Duration(seconds: 1));
      expect(find.text('Queued'), findsOneWidget);

      // Then delivered: it brightens in place.
      store.update(
        EntryUpdate(
          agentId: 'a',
          entry: Entry(
            seq: Int64(3),
            user: UserText(text: 'ship it'),
          ),
        ),
      );
      queued.apply(queue([]));
      await tester.pump();
      await tester.pump(const Duration(seconds: 2));
      expect(find.text('ship it'), findsOneWidget);
      expect(opacityOf(tester, 'ship it'), 1);
      expect(tester.getTopLeft(find.text('ship it')).dy, place.dy);
    });

    testWidgets('its entry can take its place straight away', (tester) async {
      final store = transcriptOf(3);
      final (box, answers) = outbox();
      await show(
        tester,
        store,
        ValueNotifier(0),
        queued: QueuedStore(),
        outbox: box,
      );
      box.send('a', 'ship it');
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 400));
      final place = tester.getTopLeft(find.text('ship it'));

      store.update(
        EntryUpdate(
          agentId: 'a',
          entry: Entry(
            seq: Int64(3),
            user: UserText(text: 'ship it'),
          ),
        ),
      );
      await tester.pump();
      expect(find.text('ship it'), findsOneWidget);
      expect(find.text('Sending…'), findsNothing);
      expect(box.of('a'), isEmpty);
      for (var i = 0; i < 6; i++) {
        expect(opacityOf(tester, 'ship it'), greaterThanOrEqualTo(0.5));
        await tester.pump(const Duration(milliseconds: 40));
      }
      await tester.pump(const Duration(seconds: 1));
      expect(opacityOf(tester, 'ship it'), 1);
      expect(tester.getTopLeft(find.text('ship it')).dy, place.dy);
      answers.single.complete();
      await tester.pump(const Duration(seconds: 2));
      expect(find.text('ship it'), findsOneWidget);
    });

    testWidgets('one the backend has, but whose own never shows here, goes', (
      tester,
    ) async {
      final (box, answers) = outbox();
      await show(
        tester,
        transcriptOf(3),
        ValueNotifier(0),
        queued: QueuedStore(),
        outbox: box,
      );
      box.send('a', 'ship it');
      await tester.pump();
      expect(find.text('ship it'), findsOneWidget);
      // The backend sends what it became before it answers: had it shown
      // here, it would have taken its place already.
      answers.single.complete();
      await tester.pump();
      expect(find.text('ship it'), findsNothing);
      expect(box.of('a'), isEmpty);
    });

    testWidgets("one that isn't sent stays, with Retry", (tester) async {
      final (box, answers) = outbox();
      await show(
        tester,
        transcriptOf(3),
        ValueNotifier(0),
        queued: QueuedStore(),
        outbox: box,
      );
      box.send('a', 'ship it');
      await tester.pump();
      answers.single.completeError(
        const RequestFailure('Not connected to the backend.'),
      );
      await tester.pump();
      await tester.pump(const Duration(seconds: 3));
      expect(find.text('ship it'), findsOneWidget);
      expect(find.text('Not sent'), findsOneWidget);
      expect(find.text('Sending…'), findsNothing);
      // "Not sent" isn't faded with the bubble.
      expect(opacityOf(tester, 'Not sent'), 1);

      await tester.tap(find.text('Retry'));
      await tester.pump();
      expect(answers, hasLength(2));
      expect(find.text('Not sent'), findsNothing);
      expect(find.text('Sending…'), findsOneWidget);
      answers.last.complete();
      await tester.pump(const Duration(seconds: 2));
      expect(box.of('a'), isEmpty);
    });

    testWidgets('matched by the id it was sent with, not its text', (
      tester,
    ) async {
      final store = transcriptOf(3);
      final queued = QueuedStore();
      final fake = FakeOutboxBackend();
      final ids = fake.ids;
      final answers = fake.answers;
      final box = Outbox(fake);
      await show(tester, store, ValueNotifier(0), queued: queued, outbox: box);
      box
        ..send('a', 'ok')
        ..send('a', 'my key is hunter2');
      await tester.pump();
      expect(ids, hasLength(2));
      expect(ids.toSet(), hasLength(2), reason: 'one id per send');

      // The second comes back first, its secret hidden; the first after.
      queued.apply(
        QueuedMessages(
          agentId: 'a',
          messages: [
            QueuedMessage(
              id: Int64(8),
              source: 'person',
              text: 'my key is [secret KEY]',
              clientId: ids[1],
            ),
          ],
        ),
      );
      await tester.pump();
      expect(find.text('my key is hunter2'), findsNothing);
      expect(find.text('my key is [secret KEY]'), findsOneWidget);
      expect(box.of('a').single.text, 'ok');

      store.update(
        EntryUpdate(
          agentId: 'a',
          entry: Entry(
            seq: Int64(3),
            user: UserText(text: 'ok', clientId: ids[0]),
          ),
        ),
      );
      await tester.pump();
      expect(box.of('a'), isEmpty);
      expect(find.text('ok'), findsOneWidget);
      for (final answer in answers) {
        answer.complete();
      }
      await tester.pump(const Duration(seconds: 2));
    });

    testWidgets('a message queued before it stays before it', (tester) async {
      final queued = QueuedStore();
      final (box, answers) = outbox();
      await show(
        tester,
        transcriptOf(3),
        ValueNotifier(0),
        queued: queued,
        outbox: box,
      );
      box.send('a', 'mine');
      await tester.pump();
      queued.apply(
        queue([
          QueuedMessage(
            id: Int64(1),
            source: 'agent',
            sender: 'Lead',
            senderId: 'lead',
            text: 'from Lead',
          ),
        ]),
      );
      await tester.pump(const Duration(milliseconds: 400));
      expect(
        tester.getTopLeft(find.text('from Lead')).dy,
        lessThan(tester.getTopLeft(find.text('mine')).dy),
      );
      answers.single.complete();
      await tester.pump(const Duration(seconds: 2));
    });
  });

  test('a message to several agents names the others apart', () {
    expect(
      splitAlsoSentTo('Hi.\n\nAlso sent to: Lena (Backend), Omar (Backend).'),
      ('Hi.', 'Lena (Backend), Omar (Backend)'),
    );
    expect(splitAlsoSentTo('Hi.'), ('Hi.', null));
  });

  testWidgets("an agent's message to several says who else got it", (
    tester,
  ) async {
    final store = TranscriptStore()
      ..load(
        Transcript(
          agentId: 'a',
          entries: [
            Entry(
              seq: Int64(1),
              event: Event(
                source: EventSource.EVENT_SOURCE_AGENT,
                from: 'Mike',
                fromId: 'coordinator',
                text: 'Release at six.\n\nAlso sent to: Lena (Backend).',
              ),
            ),
          ],
        ),
      );
    await show(tester, store, ValueNotifier(0));
    expect(find.text('Release at six.'), findsOneWidget);
    expect(find.textContaining('Also sent to:'), findsNothing);
    expect(find.text('Also sent to Lena (Backend)'), findsOneWidget);
  });

  test('the device mark names the device, or its kind without a name', () {
    expect(
      sentFromText(
        SentFrom(
          deviceName: "Matt's iPhone",
          kind: DeviceKind.DEVICE_KIND_PHONE,
        ),
      ),
      "Sent from Matt's iPhone",
    );
    expect(
      sentFromText(SentFrom(kind: DeviceKind.DEVICE_KIND_LAPTOP)),
      'Sent from a laptop',
    );
    expect(sentFromText(SentFrom()), 'Sent from another device');
    expect(deviceGlyph(DeviceKind.DEVICE_KIND_PHONE), 'smartphone');
    expect(deviceGlyph(DeviceKind.DEVICE_KIND_DESKTOP), 'monitor');
    expect(deviceGlyph(DeviceKind.DEVICE_KIND_LAPTOP), 'laptop-minimal');
    expect(deviceGlyph(DeviceKind.DEVICE_KIND_TABLET), 'tablet');
    expect(
      deviceGlyph(DeviceKind.DEVICE_KIND_UNSPECIFIED),
      'monitor-smartphone',
    );
  });

  testWidgets('your message shows its device, named on hover or when held', (
    tester,
  ) async {
    final phone = SentFrom(
      deviceName: "Matt's iPhone",
      kind: DeviceKind.DEVICE_KIND_PHONE,
    );
    final store = TranscriptStore()
      ..load(
        Transcript(
          agentId: 'a',
          entries: [
            Entry(
              seq: Int64(1),
              user: UserText(text: 'from before'),
            ),
            Entry(
              seq: Int64(2),
              user: UserText(text: 'on the go', sentFrom: phone),
            ),
          ],
        ),
      );
    final queued = QueuedStore()
      ..apply(
        QueuedMessages(
          agentId: 'a',
          messages: [
            QueuedMessage(
              id: Int64(1),
              source: 'person',
              text: 'and this',
              sentFrom: SentFrom(
                deviceName: 'Studio',
                kind: DeviceKind.DEVICE_KIND_DESKTOP,
              ),
            ),
          ],
        ),
      );
    await show(tester, store, ValueNotifier(0), queued: queued);
    // Nothing for a message from before the backend kept where it came
    // from; one for each that says.
    expect(find.byType(DeviceMark), findsNWidgets(2));
    final mark = find.byType(DeviceMark).first;
    // Under its bubble, at its right.
    expect(
      tester.getTopLeft(mark).dy,
      greaterThan(tester.getBottomLeft(find.text('on the go')).dy),
    );
    expect(
      tester.getCenter(mark).dx,
      greaterThan(tester.getCenter(find.text('on the go')).dx),
    );
    expect(find.text("Sent from Matt's iPhone"), findsNothing);

    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: Offset.zero);
    addTearDown(mouse.removePointer);
    await mouse.moveTo(tester.getCenter(mark));
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text("Sent from Matt's iPhone"), findsOneWidget);
    await mouse.moveTo(Offset.zero);
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.text("Sent from Matt's iPhone"), findsNothing);

    final finger = await tester.startGesture(tester.getCenter(mark));
    await tester.pump(const Duration(milliseconds: 600));
    expect(find.text("Sent from Matt's iPhone"), findsOneWidget);
    await finger.up();
    await tester.pump(const Duration(milliseconds: 200));
    expect(find.text("Sent from Matt's iPhone"), findsNothing);

    // The queued one's mark sits beside "Queued".
    final queuedMark = find.byType(DeviceMark).last;
    expect(
      tester.getCenter(queuedMark).dy,
      moreOrLessEquals(tester.getCenter(find.text('Queued')).dy, epsilon: 2),
    );
  });

  testWidgets('a compaction says whether it happened while idle', (
    tester,
  ) async {
    final store = TranscriptStore()
      ..load(
        Transcript(
          agentId: 'a',
          entries: [
            Entry(
              seq: Int64(1),
              compaction: Compaction(
                summary: 'Built the app.',
                tokensBefore: Int64(412000),
              ),
            ),
            Entry(
              seq: Int64(2),
              compaction: Compaction(
                summary: 'Fixed the tests.',
                tokensBefore: Int64(180000),
                idle: true,
              ),
            ),
          ],
        ),
      );
    await show(tester, store, ValueNotifier(0));
    expect(find.text('Earlier messages summarized'), findsOneWidget);
    expect(find.text('Earlier messages summarized while idle'), findsOneWidget);
    await tester.tap(find.text('Earlier messages summarized while idle'));
    await tester.pumpAndSettle();
    expect(find.text('Summarized at 180k tokens.'), findsOneWidget);
    expect(find.text('Fixed the tests.'), findsOneWidget);
  });

  group('tool calls', () {
    Entry call(int seq, int reply, String name) => Entry(
      seq: Int64(seq),
      reply: Int64(reply),
      toolCall: ToolCall(
        callId: 'c$seq',
        name: name,
        state: ToolCallState.TOOL_CALL_STATE_DONE,
        output: 'out $seq',
      ),
    );

    testWidgets('one reply\'s calls form one block; the next reply its own', (
      tester,
    ) async {
      final store = TranscriptStore()
        ..load(
          Transcript(
            agentId: 'a',
            entries: [
              user(0),
              call(1, 1, 'read_file'),
              call(2, 1, 'bash'),
              call(3, 1, 'grep'),
              call(4, 4, 'write_file'),
            ],
          ),
        );
      await show(tester, store, ValueNotifier(0));
      expect(tester.takeException(), isNull);
      ToolBlock blockOf(String name) => tester
          .widget<EntryView>(
            find.ancestor(
              of: find.text(name),
              matching: find.byType(EntryView),
            ),
          )
          .block;
      expect(blockOf('read_file'), const ToolBlock(first: true, last: false));
      expect(blockOf('bash'), const ToolBlock(first: false, last: false));
      expect(blockOf('grep'), const ToolBlock(first: false, last: true));
      expect(blockOf('write_file'), const ToolBlock(first: true, last: true));
      // One block's calls touch; the next reply's starts apart.
      double top(String name) => tester.getTopLeft(find.text(name)).dy;
      final step = top('grep') - top('bash');
      expect(top('bash') - top('read_file'), step);
      expect(top('write_file') - top('grep'), greaterThan(step));

      // Each call still opens on its own.
      await tester.tap(find.text('bash'));
      await tester.pumpAndSettle();
      expect(find.text('out 2'), findsOneWidget);
      expect(find.text('out 1'), findsNothing);

      // A call arriving in the same reply joins its block.
      store.update(EntryUpdate(agentId: 'a', entry: call(5, 4, 'edit_file')));
      await tester.pumpAndSettle();
      expect(blockOf('write_file'), const ToolBlock(first: true, last: false));
      expect(blockOf('edit_file'), const ToolBlock(first: false, last: true));
    });

    test('calls with no reply id stand alone', () {
      final store = TranscriptStore()
        ..load(
          Transcript(agentId: 'a', entries: [call(1, 0, 'a'), call(2, 0, 'b')]),
        );
      expect(toolBlockAt(store, 0), const ToolBlock(first: true, last: true));
      expect(toolBlockAt(store, 1), const ToolBlock(first: true, last: true));
    });
  });
}
