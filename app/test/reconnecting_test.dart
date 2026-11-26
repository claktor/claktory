import 'package:factory_app/agent_view.dart';
import 'package:factory_app/connection.dart' show ConnectionDialog;
import 'package:factory_app/connections.dart';
import 'package:factory_app/rail.dart' show PhoneConnectionWord;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/tunnel/api/tunnel.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'rivet_test.dart' show snapshot;
import 'tunnel_test.dart' show tunnelStatus;

final _code =
    'factory://?t=${'a' * 64}&name=mac-mini&e=${'c' * 64}'
    '&r=https://relay-eu.example.com';

void main() {
  testWidgets('while the connection is down, the conversation says so', (
    tester,
  ) async {
    final harness = Harness(snapshot(selected: 'a1'));
    harness.backend.transcript.load(Transcript(agentId: 'a1'));
    await harness.show(tester);
    await settle(tester);
    expect(find.text(reconnectingText(touch: true)), findsNothing);

    harness.backend.connected.value = false;
    await tester.pump(const Duration(milliseconds: 500));
    expect(
      find.text(reconnectingText(touch: true)),
      findsNothing,
      reason: 'not at once',
    );
    await tester.pump(ReconnectingLine.after);
    expect(find.text(reconnectingText(touch: true)), findsOneWidget);
    // A tap reconnects at once.
    await tester.tap(find.byType(ReconnectingLine));
    expect(harness.backend.reconnectsAsked, 1);

    harness.backend.connected.value = true;
    await tester.pump();
    expect(find.text(reconnectingText(touch: true)), findsNothing);

    // A drop shorter than a second never shows.
    harness.backend.connected.value = false;
    await tester.pump(const Duration(milliseconds: 300));
    harness.backend.connected.value = true;
    await tester.pump(const Duration(seconds: 2));
    expect(find.text(reconnectingText(touch: true)), findsNothing);
  });

  for (final size in [const Size(1200, 900), const Size(393, 852)]) {
    testWidgets("once its tries fail, it says it can't connect and what to "
        "do, not that it's reconnecting (${size.width.toInt()} wide)", (
      tester,
    ) async {
      final harness = Harness(snapshot(selected: 'a1'), localBackend: false);
      // Paired with mac-mini (connecting starts it afresh: its snapshot
      // after).
      harness.connections.use(parsePairingCode(_code)!);
      harness.backend.workspace.apply(snapshot(selected: 'a1'));
      harness.backend.transcript.load(Transcript(agentId: 'a1'));
      await harness.show(tester, size: size);
      await settle(tester);
      // A phone opens on the list: Builder's conversation.
      if (find.byType(AgentView).evaluate().isEmpty) {
        await tester.tap(find.text('Builder').first);
        await settle(tester);
      }
      expect(find.byType(AgentView), findsOneWidget);
      final line = find.byType(ReconnectingLine);
      Finder inLine(String text) =>
          find.descendant(of: line, matching: find.textContaining(text));

      // Down, coming back quietly: reconnecting.
      harness.backend
        ..connected.value = false
        ..reconnecting.value = true;
      harness.backend.tunnel.value = tunnelStatus(
        phase: TunnelPhase.failed,
        path: TunnelPath.none,
        failure: TunnelFailure.unreachable,
        error: 'Nothing answered within 8 seconds.',
      );
      await tester.pump(ReconnectingLine.after);
      await tester.pump();
      expect(inLine(reconnectingText(touch: true)), findsOneWidget);
      expect(inLine("Can't connect"), findsNothing);

      // Its tries failing (the machine off): it says so, and what to do.
      harness.backend.reconnecting.value = false;
      await tester.pump();
      expect(inLine(reconnectingText(touch: true)), findsNothing);
      expect(inLine("Can't connect to mac-mini"), findsOneWidget);
      expect(
        inLine('Make sure mac-mini is on, awake and online'),
        findsOneWidget,
      );

      // A tap tries again at once; VoiceOver says what it does.
      expect(
        tester.getSize(line).height,
        greaterThanOrEqualTo(44),
        reason: "a finger's height",
      );
      expect(inLine('Tap to try again.'), findsOneWidget);
      expect(inLine('Trying again…'), findsNothing);
      final semantics = tester.ensureSemantics();
      expect(
        tester.getSemantics(
          find.bySemanticsLabel(RegExp("^Can't connect to mac-mini\\.")),
        ),
        matchesSemantics(
          label:
              "Can't connect to mac-mini. This "
              '${size.width < 600 ? 'phone' : 'tablet'} is online, but '
              "mac-mini doesn't answer. Make sure mac-mini is on, awake "
              'and online, with Claktory open. Tap to try again.',
          hint: reconnectHint,
          isButton: true,
          hasTapAction: true,
          isLiveRegion: true,
        ),
      );
      semantics.dispose();
      await tester.tap(line);
      expect(harness.backend.reconnectsAsked, 1);

      // Back: nothing.
      harness.backend.connected.value = true;
      await tester.pump();
      expect(inLine("Can't connect"), findsNothing);
      expect(inLine(reconnectingText(touch: true)), findsNothing);
    });
  }

  testWidgets("on a phone, tapping the menu's Can't connect reconnects at "
      'once and shows the connection', (tester) async {
    final harness = Harness(snapshot(selected: 'a1'), localBackend: false);
    harness.connections.use(parsePairingCode(_code)!);
    harness.backend.workspace.apply(snapshot(selected: 'a1'));
    harness.backend.transcript.load(Transcript(agentId: 'a1'));
    await harness.show(tester, size: const Size(393, 852));
    await settle(tester);
    harness.backend
      ..connected.value = false
      ..reconnecting.value = false;
    harness.backend.tunnel.value = tunnelStatus(
      phase: TunnelPhase.failed,
      path: TunnelPath.none,
      failure: TunnelFailure.unreachable,
      error: 'Nothing answered within 8 seconds.',
    );
    await tester.pump();
    if (find.byType(PhoneConnectionWord).hitTestable().evaluate().isEmpty) {
      await tester.tap(find.bySemanticsLabel('Menu').last);
      await pumpFrames(tester);
    }
    final word = find.byType(PhoneConnectionWord).hitTestable().first;
    expect(
      find.descendant(of: word, matching: find.text("Can't connect")),
      findsOneWidget,
    );
    // The drawer's dense line (Touch.denseLine): its height there.
    expect(tester.getSize(word).height, greaterThanOrEqualTo(36));
    await tester.tap(word);
    await settle(tester);
    expect(harness.backend.reconnectsAsked, 1);
    expect(find.byType(ConnectionDialog), findsOneWidget);
  });
}
