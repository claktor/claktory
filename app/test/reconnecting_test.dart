import 'package:factory_app/agent_view.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'rivet_test.dart' show snapshot;

void main() {
  testWidgets('while the connection is down, the conversation says so', (
    tester,
  ) async {
    final harness = Harness(snapshot(selected: 'a1'));
    harness.backend.transcript.load(Transcript(agentId: 'a1'));
    await harness.show(tester);
    await settle(tester);
    expect(find.text(reconnectingText), findsNothing);

    harness.backend.connected.value = false;
    await tester.pump(const Duration(milliseconds: 500));
    expect(find.text(reconnectingText), findsNothing, reason: 'not at once');
    await tester.pump(ReconnectingLine.after);
    expect(find.text(reconnectingText), findsOneWidget);

    harness.backend.connected.value = true;
    await tester.pump();
    expect(find.text(reconnectingText), findsNothing);

    // A drop shorter than a second never shows.
    harness.backend.connected.value = false;
    await tester.pump(const Duration(milliseconds: 300));
    harness.backend.connected.value = true;
    await tester.pump(const Duration(seconds: 2));
    expect(find.text(reconnectingText), findsNothing);
  });
}
