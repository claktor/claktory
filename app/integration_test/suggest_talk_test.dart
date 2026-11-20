// An agent suggests the person talk to another agent, end to end, against
// a real Claktory (the dev one), on an iPhone (the Simulator) already
// paired with it, or paired with PAIRING_CODE:
//
//   1. AGENT (Wren by default) is asked to suggest talking to TARGET.
//   2. Its card's "Talk to <TARGET>" opens TARGET's conversation with the
//      message in its box, not sent.
//   3. Send sends it, as the person's message.
//
//   cd app && flutter test integration_test/suggest_talk_test.dart \
//     -d <simulator> --dart-define=FACTORY_DEV=true \
//     --dart-define=AGENT=Wren --dart-define=TARGET=Rivet
//
// AGENT's model runs for real; TARGET only receives the message.
import 'package:factory_app/main.dart' as app;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart';
import 'package:factory_app/tool_views.dart' show TalkCard;
import 'package:flutter/widgets.dart' show EditableText;
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'phone_test.dart' show hold, waitFor;

const _code = String.fromEnvironment('PAIRING_CODE');
const _agent = String.fromEnvironment('AGENT', defaultValue: 'Wren');
const _target = String.fromEnvironment('TARGET', defaultValue: 'Rivet');

/// Holds the screen still at each step, for screenshots.
const _shots = bool.fromEnvironment('SHOTS');

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('suggesting the person talk to another agent', (tester) async {
    await app.main();
    await tester.pump(const Duration(seconds: 1));
    if (find.text('Connect to your Claktory').evaluate().isNotEmpty) {
      expect(_code, isNotEmpty, reason: 'pass --dart-define=PAIRING_CODE=…');
      for (var i = 0; i < 50; i++) {
        if (find.byType(EditableText).evaluate().isNotEmpty) break;
        if (find.text('Paste code instead').evaluate().isNotEmpty) {
          await tester.tap(find.text('Paste code instead'));
        }
        await tester.pump(const Duration(milliseconds: 200));
      }
      await tester.enterText(find.byType(EditableText), _code);
      await tester.pump();
      await tester.tap(find.text('Connect'));
    }
    await waitFor(
      tester,
      find.byType(Sidebar),
      timeout: const Duration(minutes: 1),
    );
    final scope = AppScope.scope(tester.element(find.byType(Sidebar).first));
    final backend = scope.backend;
    Workspace now() => backend.workspace.workspace;
    Future<void> until(
      String what,
      bool Function() done, {
      Duration timeout = const Duration(minutes: 1),
    }) async {
      final end = DateTime.now().add(timeout);
      while (!done()) {
        if (DateTime.now().isAfter(end)) fail('never $what');
        await tester.pump(const Duration(milliseconds: 200));
      }
    }

    await until('connected', () => backend.connected.value);
    final agent = now().agents.firstWhere((a) => a.name == _agent);
    final target = now().agents.firstWhere((a) => a.name == _target);
    final stamp = DateTime.now().millisecondsSinceEpoch;
    final draft = 'Test $stamp: please reply with only the word "ok".';
    scope.drafts.set(target.id, '');

    // 1. The suggestion.
    await tester.tap(find.text(_agent).first);
    await hold(tester, seconds: 1);
    await backend.sendMessage(
      agent.id,
      'This is a test of suggesting that I talk to another agent. Call '
      'agents_suggest_talk once, with agent "$_target", message "$draft" and '
      'reason "$_target can answer this test". Then stop; no need to reply.',
    );
    final card = find.byType(TalkCard);
    final button = find.text('Talk to $_target');
    // Earlier tries' cards may be there already: wait for a new one.
    final before = card.evaluate().length;
    await until(
      'the card',
      () => card.evaluate().length > before,
      timeout: const Duration(minutes: 2),
    );
    await tester.ensureVisible(button.last);
    await hold(tester, seconds: _shots ? 6 : 1);

    // 2. Talk to the target: its conversation, the message in its box.
    await tester.tap(button.last);
    await until(
      "the target's conversation",
      () => now().selectedAgentId == target.id,
    );
    await hold(tester, seconds: _shots ? 6 : 1);
    expect(scope.drafts.of(target.id), draft);
    final box = find.byWidgetPredicate(
      (w) => w is EditableText && w.controller.text == draft,
    );
    await waitFor(tester, box);

    // 3. Send.
    await tester.tap(find.text('Send').last);
    await until('sent', () {
      final transcript = backend.transcript;
      if (transcript.agentId != target.id) return false;
      for (var i = transcript.length - 1; i >= 0; i--) {
        final entry = transcript[i].entry;
        if (entry.hasUser() && entry.user.text.contains('Test $stamp')) {
          return true;
        }
      }
      return false;
    }, timeout: const Duration(seconds: 30));
    expect(scope.drafts.of(target.id), isEmpty);
    await hold(tester, seconds: _shots ? 6 : 2);
    // ignore: avoid_print
    print('suggested, opened with the draft, and sent');
  });
}
