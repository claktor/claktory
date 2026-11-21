// Matt's reconnect flash (#214): a Claktory connected afresh (opening the
// app, switching to another) knows nothing until its workspace arrives,
// and shows that: "Connecting…", never what an empty Claktory would ask
// (the name, a new project, signing in to a provider) in between.
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/connections.dart' show parsePairingCode;
import 'package:factory_app/name_step.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_test.dart' show phone, workspace;

final _home = parsePairingCode(
  'factory://?t=${'a' * 64}&name=home&e=${'c' * 64}',
)!;

/// Nothing a Claktory with no workspace yet should show.
void _nothingDerived() {
  expect(find.byType(NameStep), findsNothing);
  expect(find.text('No agents yet'), findsNothing);
  expect(find.text('New project'), findsNothing);
  expect(find.textContaining('Sign in'), findsNothing);
}

void main() {
  for (final (what, size) in [
    ('a phone', phone),
    ('a Mac', const Size(1200, 900)),
  ]) {
    testWidgets('on $what, a fresh connection says Connecting… until its '
        'workspace arrives, then shows the conversation', (tester) async {
      if (size == phone) {
        debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
      }
      final harness = Harness(
        workspace()..settings = Settings(userName: 'Matt'),
      );
      await harness.show(tester, size: size);
      await pumpFrames(tester);
      expect(find.byType(AgentView), findsOneWidget);

      // Connected afresh (Welcome in), its workspace still on its way.
      harness.backend.connectTo(_home);
      harness.backend.connected.value = true;
      await pumpFrames(tester);
      expect(find.text('Connecting…'), findsWidgets);
      _nothingDerived();

      await pumpFrames(tester, const Duration(seconds: 2));
      _nothingDerived();

      harness.backend.workspace.apply(
        workspace()..settings = Settings(userName: 'Matt'),
      );
      await pumpFrames(tester);
      expect(find.byType(AgentView), findsOneWidget);
      _nothingDerived();
    });
  }
}
