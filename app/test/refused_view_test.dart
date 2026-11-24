import 'package:factory_app/agent_view.dart' show AgentView;
import 'package:factory_app/connections.dart' show parsePairingCode;
import 'package:factory_app/message_box.dart' show MessageBox;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

// Zucchero's row 220: a phone its Claktory turned away (a pairing code
// that expired) said "Connecting…" forever in the main view, while the
// drawer said Not allowed.

void main() {
  testWidgets('turned away, the main view says so, with Switch Claktory…, '
      'never Connecting…', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace(), localBackend: false);
    harness.connections.use(
      parsePairingCode('factory://?t=${'b' * 64}&name=mac-mini&e=${'d' * 64}'),
    );
    await harness.show(tester, size: const Size(375, 667));
    await pumpFrames(tester);
    harness.backend.connected.value = false;
    harness.backend.refused.value =
        "This device isn't allowed on this Claktory. Use a new code.";
    await pumpFrames(tester);
    expect(find.text('Connecting…'), findsNothing);
    // The app's own words, not the reason the tunnel gave (Trish).
    expect(
      find.text("This device isn't allowed on this Claktory. Use a new code."),
      findsNothing,
    );
    expect(
      find.text(
        'To use it again, add this iPhone in Settings → Devices on mac-mini.',
      ),
      findsOneWidget,
    );
    expect(find.text("mac-mini doesn't allow this device"), findsOneWidget);
    await tester.tap(find.text('Switch Claktory…'));
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);

    // Picked again and let in: the usual view.
    harness.backend.refused.value = null;
    harness.backend.connected.value = true;
    await pumpFrames(tester);
    expect(find.textContaining('To use it again'), findsNothing);
  });

  // Zucchero's row 232: with an agent open, the conversation stayed, with
  // a message box that couldn't send.
  testWidgets('turned away with an agent open, the main view says so '
      'instead of the conversation', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace(), localBackend: false);
    harness.connections.use(
      parsePairingCode('factory://?t=${'b' * 64}&name=studio&e=${'d' * 64}'),
    );
    await harness.show(tester, size: const Size(1200, 800));
    // Connected, with Eve open.
    harness.backend.connected.value = true;
    harness.backend.workspace.apply(
      Workspace(
        agents: [Agent(id: 'eve', name: 'Eve', color: 'green')],
        selectedAgentId: 'eve',
        models: [Model(id: 'm', name: 'M')],
      ),
    );
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsOneWidget);
    harness.backend.connected.value = false;
    harness.backend.refused.value = 'The machine turned this device away';
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsNothing);
    expect(
      find.text(
        'To use it again, add this Mac in Settings → Devices on studio.',
      ),
      findsOneWidget,
    );
    expect(find.text('Switch Claktory…'), findsWidgets);

    harness.backend.refused.value = null;
    harness.backend.connected.value = true;
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsOneWidget);
  });

  // On a phone, the same; the draft in the message box waits in the
  // drafts kept on the device until it's back (Prosciutto). The smallest
  // phone, at the largest text.
  testWidgets('a phone turned away with an agent open says so, and keeps '
      'the draft', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = 53 / 17;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final harness = Harness(Workspace(), localBackend: false);
    harness.connections.use(
      parsePairingCode('factory://?t=${'b' * 64}&name=studio&e=${'d' * 64}'),
    );
    await harness.show(tester, size: const Size(320, 568));
    harness.backend.connected.value = true;
    harness.backend.transcript.load(Transcript(agentId: 'eve'));
    harness.backend.workspace.apply(
      Workspace(
        agents: [
          Agent(
            id: 'eve',
            name: 'Eve',
            color: 'green',
            modelAvailable: true,
            status: AgentStatus.AGENT_STATUS_IDLE,
          ),
        ],
        selectedAgentId: 'eve',
        models: [Model(id: 'm', name: 'M')],
      ),
    );
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsOneWidget);
    final box = find.descendant(
      of: find.byType(MessageBox),
      matching: find.byType(EditableText),
    );
    await tester.enterText(box.first, 'Half a thought');
    await pumpFrames(tester);

    harness.backend.connected.value = false;
    harness.backend.refused.value = 'The machine turned this device away';
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    expect(find.byType(AgentView), findsNothing);
    expect(find.text("studio doesn't allow this device"), findsWidgets);
    expect(
      find.text(
        'To use it again, add this iPhone in Settings → Devices on studio.',
      ),
      findsOneWidget,
    );
    final switchTo = find.text('Switch Claktory…').last;
    await tester.ensureVisible(switchTo);
    await pumpFrames(tester);
    final at = tester.getRect(switchTo);
    expect(at.top, greaterThanOrEqualTo(0));
    expect(at.bottom, lessThanOrEqualTo(568));
    expect(harness.drafts.of('eve'), 'Half a thought');

    harness.backend.refused.value = null;
    harness.backend.connected.value = true;
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsOneWidget);
    expect(find.text('Half a thought'), findsOneWidget);
  });
}
