import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'rivet_test.dart' show admin;

const _hint =
    "Tell Wren what you'd like done. It remembers this conversation, so you "
    'can pick up anytime.';

Harness _harness({String role = 'Researcher', List<Entry> entries = const []}) {
  final harness = Harness(
    Workspace(
      agents: [
        admin(),
        Agent(id: 'wren', name: 'Wren', role: role, modelAvailable: true),
      ],
      selectedAgentId: 'wren',
    ),
  );
  harness.backend.transcript.load(
    Transcript(agentId: 'wren', entries: entries),
  );
  return harness;
}

void main() {
  testWidgets('a new agent says what it is for and how to start', (
    tester,
  ) async {
    await _harness().show(tester);
    await settle(tester);
    expect(find.text('Researcher'), findsWidgets);
    expect(find.text(_hint), findsOneWidget);
  });

  testWidgets('without a role, just the name and how to start', (tester) async {
    await _harness(role: '').show(tester);
    await settle(tester);
    expect(find.text(_hint), findsOneWidget);
  });

  testWidgets('gone once there is a conversation', (tester) async {
    await _harness(
      entries: [
        Entry(
          seq: Int64(1),
          user: UserText(text: 'Look into caching'),
        ),
      ],
    ).show(tester);
    await settle(tester);
    expect(find.text(_hint), findsNothing);
    expect(find.text('Look into caching'), findsOneWidget);
  });
}
