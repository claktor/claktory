// ⌘F on an iPad finds in what's being read (Lucy's agent view §4): with
// the conversation the pane read, the system's menu (hold ⌘) lists ⌘F as
// "Find" (Trish 43450), and running it opens the conversation's find bar,
// as ⌘F does on a Mac.
import 'package:factory_app/conversation_find.dart' show ConversationFind;
import 'package:factory_app/find.dart' show FindSessions;
import 'package:factory_app/ipad/commands.dart' show CommandMenu;
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/shell.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../panes_test.dart' show leaf, snapshot;
import 'as_ipad.dart';

const _channel = MethodChannel(CommandMenu.channelName);

void main() {
  testWidgets("the menu's ⌘F opens the conversation's find bar", (
    tester,
  ) async {
    asIPad();
    final titles = <String, String>{};
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(_channel, (call) async {
      if (call.method != 'setCommands') return null;
      titles.clear();
      for (final command in (call.arguments as Map)['commands'] as List) {
        final map = command as Map;
        titles[map['id'] as String] = map['title'] as String;
      }
      return true;
    });
    addTearDown(() => messenger.setMockMethodCallHandler(_channel, null));
    final focus = WorkbenchFocus();
    addTearDown(focus.dispose);
    final harness = Harness(snapshot(layout: leaf('chat', 'chat')));
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(
            seq: Int64(1),
            timestampMs: Int64(1000),
            assistant: AssistantText(text: 'Shipped.'),
          ),
        ],
      ),
    );
    await harness.show(
      tester,
      size: const Size(1376, 1032),
      home: WorkbenchScope(
        focus: focus,
        child: const Shell(rail: SizedBox(width: 56)),
      ),
    );
    await pumpFrames(tester);
    expect(titles['find'], 'Find');

    final session = FindSessions.instance.sessions.value
        .where((session) => session.target is ConversationFind)
        .single;
    expect(session.isOpen.value, isFalse);
    await messenger.handlePlatformMessage(
      CommandMenu.channelName,
      const StandardMethodCodec().encodeMethodCall(
        MethodCall('run', {'id': 'find'}),
      ),
      (_) {},
    );
    await pumpFrames(tester);
    expect(session.isOpen.value, isTrue);
  });
}
