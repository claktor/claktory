import 'package:factory_app/controls.dart' show GlyphButton;
import 'package:factory_app/forms.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/transcript.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

Agent writer() => Agent(
  id: 'a2',
  projectId: 'p1',
  name: 'Writer',
  model: 'anthropic/claude-opus-5-5',
  effort: 'high',
  status: AgentStatus.AGENT_STATUS_IDLE,
  modelAvailable: true,
  color: 'green',
);

void main() {
  testWidgets(
    "messages that aren't the person's show as events, not as theirs",
    (tester) async {
      final agent = builder(working: false)..color = 'violet';
      final harness = Harness(workspace(agents: [agent, writer()]));
      harness.backend.transcript.load(
        Transcript(
          agentId: 'a1',
          entries: [
            Entry(
              seq: Int64(1),
              user: UserText(text: 'Ship it'),
            ),
            Entry(
              seq: Int64(2),
              event: Event(
                source: EventSource.EVENT_SOURCE_FACTORY,
                from: 'Factory',
                summary: 'Factory restarted: updated from A to B',
                text:
                    'Factory restarted: updated from A to B while you were '
                    'working. Carry on with your task.',
              ),
            ),
            Entry(
              seq: Int64(3),
              event: Event(
                source: EventSource.EVENT_SOURCE_AGENT,
                from: 'Writer',
                fromId: 'a2',
                text: 'The notes are ready.',
              ),
            ),
            Entry(
              seq: Int64(4),
              event: Event(
                source: EventSource.EVENT_SOURCE_TASK,
                from: 'task-1',
                fromId: 'task-1',
                summary: 'task-1 finished (exit 0): cargo test',
                text: 'Background task-1 finished (exit 0).\n\ntest result: ok',
              ),
            ),
          ],
        ),
      );
      await harness.show(tester);

      // The person's message is a bubble in the agent's color.
      final mine = tester.widget<Bubble>(
        find.ancestor(of: find.text('Ship it'), matching: find.byType(Bubble)),
      );
      expect(mine.outgoing, isTrue);
      expect(mine.color, const Color(0xFF7B52E0));

      // Factory's note is one line until opened.
      expect(
        find.text('Factory restarted: updated from A to B'),
        findsOneWidget,
      );
      expect(find.textContaining('Carry on with your task.'), findsNothing);
      await tester.tap(find.text('Factory restarted: updated from A to B'));
      await settle(tester);
      expect(find.textContaining('Carry on with your task.'), findsOneWidget);

      // Another agent's message comes in on the left, under its name in its color.
      final theirs = tester.widget<Bubble>(
        find.ancestor(
          of: find.textContaining('The notes are ready.'),
          matching: find.byType(Bubble),
        ),
      );
      expect(theirs.outgoing, isFalse);
      final name = tester.widget<Text>(find.text('Writer').last);
      expect(name.style?.color, const Color(0xFF1F8549));

      // A finished task says so in a line, with its output inside.
      expect(find.text('task-1 finished (exit 0): cargo test'), findsOneWidget);
      expect(find.textContaining('test result: ok'), findsNothing);
      await tester.tap(find.text('task-1 finished (exit 0): cargo test'));
      await settle(tester);
      expect(find.textContaining('test result: ok'), findsOneWidget);
    },
  );

  group("another agent's message opens its conversation", () {
    Future<Harness> showMessage(
      WidgetTester tester, {
      String fromId = 'a2',
      bool queued = false,
    }) async {
      final harness = Harness(
        workspace(agents: [builder(working: false), writer()]),
      );
      final event = Event(
        source: EventSource.EVENT_SOURCE_AGENT,
        from: 'Writer',
        fromId: fromId,
        text: 'The notes are ready.',
      );
      harness.backend.transcript.load(
        Transcript(
          agentId: 'a1',
          entries: [if (!queued) Entry(seq: Int64(1), event: event)],
        ),
      );
      if (queued) {
        harness.backend.queued.apply(
          QueuedMessages(
            agentId: 'a1',
            messages: [
              QueuedMessage(
                id: Int64(1),
                source: 'agent',
                sender: 'Writer',
                senderId: fromId,
                text: 'The notes are ready.',
              ),
            ],
          ),
        );
      }
      await harness.show(tester);
      harness.backend.sent.clear();
      return harness;
    }

    Finder name() => find.descendant(
      of: find.byType(TranscriptView),
      matching: find.text('Writer'),
    );
    Finder bubble() => find.ancestor(
      of: find.textContaining('The notes are ready.'),
      matching: find.byType(Bubble),
    );
    List<String> opened(Harness harness) => [
      for (final message in harness.backend.sent)
        if (message.hasSelectAgent()) message.selectAgent.agentId,
    ];

    testWidgets('clicking the name opens the sender', (tester) async {
      final harness = await showMessage(tester);
      await tester.tap(name());
      await tester.pump();
      expect(opened(harness), ['a2']);
    });

    testWidgets('hovering underlines the name, with a pointer', (tester) async {
      await showMessage(tester);
      TextDecoration? decoration() =>
          tester.widget<Text>(name()).style?.decoration;
      expect(decoration(), isNot(TextDecoration.underline));
      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: tester.getCenter(bubble()));
      await tester.pump();
      expect(decoration(), TextDecoration.underline);
      expect(
        RendererBinding.instance.mouseTracker.debugDeviceActiveCursor(1),
        SystemMouseCursors.click,
      );
      await mouse.moveTo(Offset.zero);
      await tester.pump();
      expect(decoration(), isNot(TextDecoration.underline));
      await mouse.removePointer();
    });

    testWidgets('a click on the bubble opens it; a double click selects', (
      tester,
    ) async {
      final harness = await showMessage(tester);
      await tester.tap(bubble());
      await tester.pump();
      expect(opened(harness), isEmpty, reason: 'it may be a double click');
      await tester.pump(kDoubleTapTimeout);
      expect(opened(harness), ['a2']);

      harness.backend.sent.clear();
      await tester.tap(bubble());
      await tester.pump(const Duration(milliseconds: 80));
      await tester.tap(bubble());
      await tester.pump(kDoubleTapTimeout * 2);
      expect(opened(harness), isEmpty);
    });

    testWidgets('dragging across the bubble selects instead', (tester) async {
      final harness = await showMessage(tester);
      final box = tester.getRect(bubble());
      final mouse = await tester.startGesture(
        box.centerLeft + const Offset(14, 0),
        kind: PointerDeviceKind.mouse,
      );
      await mouse.moveTo(box.centerRight - const Offset(14, 0));
      await mouse.up();
      await tester.pump(kDoubleTapTimeout * 2);
      expect(opened(harness), isEmpty);

      // The text is selected, so the next click only clears it.
      await tester.tap(bubble(), kind: PointerDeviceKind.mouse);
      await tester.pump(kDoubleTapTimeout * 2);
      expect(opened(harness), isEmpty);
      // And the one after that opens the sender.
      await tester.tap(bubble(), kind: PointerDeviceKind.mouse);
      await tester.pump(kDoubleTapTimeout * 2);
      expect(opened(harness), ['a2']);
    });

    testWidgets('a sender the app doesn\'t know isn\'t clickable', (
      tester,
    ) async {
      final harness = await showMessage(tester, fromId: 'gone');
      await tester.tap(name());
      await tester.tap(bubble());
      await tester.pump(kDoubleTapTimeout * 2);
      expect(opened(harness), isEmpty);
    });

    testWidgets('a queued message from an agent opens it too', (tester) async {
      final harness = await showMessage(tester, queued: true);
      expect(find.text('Queued'), findsOneWidget);
      await tester.tap(name());
      await tester.pump();
      expect(opened(harness), ['a2']);
    });
  });

  testWidgets('the marketplace shows each kind and closes', (tester) async {
    final harness = Harness(workspace());
    harness.backend.appliesLayouts = true;
    await harness.show(tester);
    await tester.tap(find.bySemanticsLabel('Marketplace'));
    await settle(tester);
    expect(find.text('No agent templates yet.'), findsOneWidget);
    await tester.tap(find.text('Skills'));
    await settle(tester);
    expect(find.text('No skills yet.'), findsOneWidget);
    await tester.tap(find.text('Extensions'));
    await settle(tester);
    expect(find.text('No extensions yet.'), findsOneWidget);
    // Beside the conversation, its pane's × closes it.
    expect(find.text('Conversation'), findsOneWidget);
    await tester.tap(
      find.byWidgetPredicate((w) => w is GlyphButton && w.label == 'Close'),
    );
    await settle(tester);
    expect(find.text('No extensions yet.'), findsNothing);

    // The cog opens Settings, and closes it again.
    await tester.tap(find.bySemanticsLabel('Settings'));
    await settle(tester);
    expect(find.text('Providers'), findsOneWidget);
    await tester.tap(find.bySemanticsLabel('Settings').first);
    await settle(tester);
    expect(find.text('Providers'), findsNothing);
  });

  testWidgets("an agent's color is picked in its settings", (tester) async {
    final agent = builder(working: false)..color = 'violet';
    final harness = Harness(workspace(agents: [agent]));
    await harness.show(tester);
    await tester.tap(find.text('Opus 5.5 · High'));
    await settle(tester);
    await tester.tap(find.byKey(const ValueKey('color-teal')));
    await settle(tester);
    await tester.tap(find.text('Save'));
    await settle(tester);
    final update = harness.last.updateAgent;
    expect(update.color, 'teal');
    expect(update.hasName(), isFalse, reason: 'only what changed is sent');
  });

  testWidgets(
    'a new agent gets the color Factory offers unless one is picked',
    (tester) async {
      final harness = Harness(workspace()..nextAgentColor = 'red');
      await harness.show(tester);
      await tapProjectPlus(tester, 'Factory');
      await settle(tester);
      final form = find.byType(CreateAgentForm);
      await tester.enterText(
        find.descendant(of: form, matching: find.byType(EditableText)).first,
        'Tester',
      );
      await tester.pump();
      await tester.tap(find.text('Create'));
      await settle(tester);
      expect(harness.last.createAgent.color, 'red');
    },
  );
}
