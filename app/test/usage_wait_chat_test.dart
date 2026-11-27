// A chat waiting out its provider's usage shows what it
// waits for, as an agent does, and doesn't wave as if it replied.
import 'package:factory_app/controls.dart' show LiveDot, WorkingWave;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart';
import 'package:factory_app/usage_wait.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart' show PointerDeviceKind;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'chats_test.dart' show chat, showChat;
import 'harness.dart';

const _said = 'Waiting for your Claude usage to reset at Fri 18:57';

Chat _waiting() => chat(status: AgentStatus.AGENT_STATUS_WORKING)
  ..usageWait = _said
  ..usageWaitUntilMs = Int64(1795000000000);

final _wait = chatUsageWaitLine(_waiting());

Workspace _workspace(Chat chat) => Workspace(
  projects: [Project(id: 'p1', name: 'Claktory', path: '/w')],
  agents: [
    Agent(id: 'mike', projectId: 'p1', name: 'Mike', modelAvailable: true),
  ],
  selectedAgentId: 'mike',
  chats: [chat],
);

Finder _inSidebar(Finder finder) =>
    find.descendant(of: find.byType(Sidebar), matching: finder);

void main() {
  test('a chat waiting is not replying', () {
    expect(chatAtWork(_waiting()), isFalse);
    expect(chatAtWork(chat(status: AgentStatus.AGENT_STATUS_WORKING)), isTrue);
    expect(chatAtWork(chat()), isFalse);
    expect(_wait, startsWith('Waiting for your Claude usage to reset at '));
  });

  testWidgets(
    'its sidebar row says the wait under its title, without the wave; '
    'Try now under the pointer',
    (tester) async {
      final harness = Harness(_workspace(_waiting()));
      await harness.show(tester);
      await pumpFrames(tester);
      expect(_inSidebar(find.text('Trains to Lyon')), findsOneWidget);
      expect(_inSidebar(find.text(_wait)), findsOneWidget);
      expect(
        tester.getTopLeft(_inSidebar(find.text(_wait))).dy,
        greaterThan(tester.getTopLeft(find.text('Trains to Lyon')).dy),
      );
      expect(_inSidebar(find.byType(WorkingWave)), findsNothing);

      final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
      await mouse.addPointer(location: Offset.zero);
      addTearDown(mouse.removePointer);
      await mouse.moveTo(tester.getCenter(find.text('Trains to Lyon')));
      await pumpFrames(tester);
      await tester.tap(_inSidebar(find.text('Try now')));
      await pumpFrames(tester);
      expect(harness.last.tryAgainNow.agentId, 'c1');

      // Replying again: the wave, and no wait.
      await harness.apply(
        tester,
        _workspace(chat(status: AgentStatus.AGENT_STATUS_WORKING)),
      );
      await pumpFrames(tester);
      expect(_inSidebar(find.text(_wait)), findsNothing);
      expect(_inSidebar(find.byType(WorkingWave)), findsOneWidget);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.macOS),
  );

  testWidgets('open, its bar has no wave; the wait line, with Stop, does '
      'the talking', (tester) async {
    await showChat(tester, with_: _waiting());
    expect(find.byType(WorkingWave), findsNothing);
    expect(find.byType(LiveDot), findsNothing);
    expect(find.text(_wait), findsOneWidget);
    expect(find.text('Try now'), findsOneWidget);
    expect(find.textContaining('Stop'), findsOneWidget);
  });
}
