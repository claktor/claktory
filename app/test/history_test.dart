import 'package:factory_app/agent_view.dart';
import 'package:factory_app/history.dart';
import 'package:factory_app/identity.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/transcript.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'events_test.dart' show writer;
import 'harness.dart';

/// Builder's conversation has a message from Writer; the backend selects
/// what the app asks it to, as the real one does.
Future<Harness> showMessage(WidgetTester tester) async {
  final harness = Harness(
    workspace(agents: [builder(working: false), writer()]),
  );
  harness.backend.onSend = (message) {
    if (message.hasSelectAgent()) {
      harness.backend.workspace.apply(
        harness.backend.workspace.workspace.deepCopy()
          ..selectedAgentId = message.selectAgent.agentId,
      );
    }
  };
  harness.backend.transcript.load(
    Transcript(
      agentId: 'a1',
      entries: [
        Entry(
          seq: Int64(1),
          event: Event(
            source: EventSource.EVENT_SOURCE_AGENT,
            from: 'Writer',
            fromId: 'a2',
            text: 'The notes are ready.',
          ),
        ),
      ],
    ),
  );
  await harness.show(tester);
  return harness;
}

Finder senderName() => find.descendant(
  of: find.byType(TranscriptView),
  matching: find.text('Writer'),
);

Finder backTo(String name) => find.byWidgetPredicate(
  (widget) => widget is Semantics && widget.properties.label == 'Back to $name',
);

String selected(Harness harness) =>
    harness.backend.workspace.workspace.selectedAgentId;

Future<void> followWriter(WidgetTester tester, Harness harness) async {
  await tester.tap(senderName());
  await settle(tester);
  expect(selected(harness), 'a2');
}

void main() {
  test('back and forward skip agents deleted meanwhile', () {
    final store = WorkspaceStore()
      ..apply(
        Workspace(
          agents: [
            for (final id in ['a', 'b', 'c']) Agent(id: id, name: id),
          ],
        ),
      );
    final shown = <String>[];
    final history = AgentHistory(workspace: store, select: shown.add);
    history.follow('a', 'b');
    history.follow('b', 'c');
    expect(history.back, 'b');
    store.apply(
      Workspace(
        agents: [
          for (final id in ['a', 'c']) Agent(id: id, name: id),
        ],
      ),
    );
    expect(history.back, 'a');
    history.goBack('c');
    expect(shown, ['b', 'c', 'a']);
    expect(history.back, isNull);
    expect(history.forward, 'c');
    history.goForward('a');
    expect(shown.last, 'c');
    expect(history.back, 'a');
    history.clear();
    expect(history.back, isNull);
    expect(history.forward, isNull);
  });

  testWidgets("following a message shows a chip back to where it was", (
    tester,
  ) async {
    final harness = await showMessage(tester);
    expect(backTo('Builder'), findsNothing);
    await followWriter(tester, harness);
    expect(backTo('Builder'), findsOneWidget);
    expect(find.text('Builder'), findsWidgets);

    // The chip goes back, without opening Writer's settings.
    await tester.tap(backTo('Builder'));
    await settle(tester);
    expect(selected(harness), 'a1');
    expect(find.text('Save'), findsNothing);
    expect(backTo('Builder'), findsNothing);
    expect(backTo('Writer'), findsNothing);
  });

  testWidgets('⌘[ and ⌘] go back and forward', (tester) async {
    final harness = await showMessage(tester);
    await followWriter(tester, harness);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.bracketLeft);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
    await settle(tester);
    expect(selected(harness), 'a1');

    await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.bracketRight);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
    await settle(tester);
    expect(selected(harness), 'a2');
    expect(backTo('Builder'), findsOneWidget);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));

  testWidgets('on Windows, Ctrl+[ and Alt+arrows go back and forward', (
    tester,
  ) async {
    final harness = await showMessage(tester);
    await followWriter(tester, harness);
    await tester.sendKeyDownEvent(LogicalKeyboardKey.controlLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.bracketLeft);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.controlLeft);
    await settle(tester);
    expect(selected(harness), 'a1');

    await tester.sendKeyDownEvent(LogicalKeyboardKey.altLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowRight);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.altLeft);
    await settle(tester);
    expect(selected(harness), 'a2');

    await tester.sendKeyDownEvent(LogicalKeyboardKey.altLeft);
    await tester.sendKeyEvent(LogicalKeyboardKey.arrowLeft);
    await tester.sendKeyUpEvent(LogicalKeyboardKey.altLeft);
    await settle(tester);
    expect(selected(harness), 'a1');
  }, variant: TargetPlatformVariant.only(TargetPlatform.windows));

  testWidgets("the mouse's back and forward buttons do too", (tester) async {
    final harness = await showMessage(tester);
    await followWriter(tester, harness);
    final at = tester.getCenter(find.byType(AgentView));
    final back = await tester.startGesture(
      at,
      kind: PointerDeviceKind.mouse,
      buttons: kBackMouseButton,
    );
    await back.up();
    await settle(tester);
    expect(selected(harness), 'a1');

    final forward = await tester.startGesture(
      at,
      kind: PointerDeviceKind.mouse,
      buttons: kForwardMouseButton,
    );
    await forward.up();
    await settle(tester);
    expect(selected(harness), 'a2');
  });

  testWidgets("the conversation is washed in its agent's color", (
    tester,
  ) async {
    final harness = await showMessage(tester);
    Color tint() {
      final box = tester.widget<DecoratedBox>(
        find
            .ancestor(
              of: find.byType(AgentView),
              matching: find.byType(DecoratedBox),
            )
            .first,
      );
      return (box.decoration as BoxDecoration).color!;
    }

    // Builder has no color of its own; Writer is green.
    final builder = harness.backend.workspace.agent('a1')!;
    expect(tint(), conversationTint(builder));
    expect(tint().a, closeTo(0.04, 0.001));
    await followWriter(tester, harness);
    await tester.pump(const Duration(milliseconds: 300));
    expect(tint(), agentColor('green').withValues(alpha: 0.04));
  });

  testWidgets('picking an agent in the sidebar starts over', (tester) async {
    final harness = await showMessage(tester);
    await followWriter(tester, harness);
    await tester.tap(find.text('Builder').first);
    await settle(tester);
    expect(selected(harness), 'a1');
    expect(harness.history.back, isNull);
    expect(harness.history.forward, isNull);
  });

  testWidgets('Settings opens beside the conversation, which keeps its way '
      'back', (tester) async {
    final harness = await showMessage(tester);
    harness.backend.appliesLayouts = true;
    await followWriter(tester, harness);
    final back = harness.history.back;
    expect(back, isNotNull);
    await tester.tap(find.bySemanticsLabel('Settings'));
    await settle(tester);
    expect(harness.history.back, back);
  });

  testWidgets('an agent deleted meanwhile has no chip', (tester) async {
    final harness = await showMessage(tester);
    await followWriter(tester, harness);
    harness.backend.workspace.apply(
      harness.backend.workspace.workspace.deepCopy()
        ..agents.removeWhere((agent) => agent.id == 'a1'),
    );
    await settle(tester);
    expect(backTo('Builder'), findsNothing);
  });
}
