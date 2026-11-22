// The card a held secret use waits on (SecretApproval): under who asks, a
// line per pipe the command may come from (P1's taint), the agent's own
// read or heard through others.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace() => Workspace(
  projects: [Project(id: 'p1', name: 'Factory')],
  agents: [
    Agent(id: 'june', name: 'June', projectId: 'p1', modelAvailable: true),
  ],
  selectedAgentId: 'june',
  pipes: [
    Pipe(
      id: 'pp',
      name: 'Acme launch',
      projectId: 'p1',
      peer: PipePeer(claktoryName: "Ana's Claktory"),
    ),
  ],
);

Future<Harness> _card(WidgetTester tester, SecretApproval approval) async {
  final harness = Harness(_workspace());
  harness.backend.transcript.load(
    Transcript(
      agentId: 'june',
      entries: [Entry(seq: Int64(7), secretApproval: approval)],
    ),
  );
  await harness.show(tester);
  await settle(tester);
  return harness;
}

SecretApproval _approval({
  List<PipeTaint> taint = const [],
  List<String> pipes = const ['Acme launch'],
  List<String> secrets = const ['AWS'],
  List<String> asks = const [],
  bool keepsRunning = false,
}) => SecretApproval(
  agentId: 'june',
  secretNames: secrets,
  pipeNames: pipes,
  command: 'aws s3 ls',
  state: SecretApprovalState.SECRET_APPROVAL_STATE_PENDING,
  taint: taint,
  askEachTimeNames: asks,
  keepsRunning: keepsRunning,
);

void main() {
  testWidgets('it says where the command may come from, under who asks', (
    tester,
  ) async {
    await _card(
      tester,
      _approval(
        taint: [
          PipeTaint(pipeName: 'Acme launch', pipeId: 'pp'),
          PipeTaint(
            pipeName: 'Acme launch',
            pipeId: 'pp',
            viaNames: ['Liaison', 'Lea'],
          ),
        ],
      ),
    );
    final read = find.text(
      "from June, which reads the pipe with Ana's Claktory",
    );
    expect(read, findsOneWidget);
    expect(
      find.text(
        "from Lea, which heard from the pipe with Ana's Claktory via Liaison",
      ),
      findsOneWidget,
    );
    final header = tester.getTopLeft(find.text('June wants to use AWS')).dy;
    final command = tester.getTopLeft(find.text('aws s3 ls')).dy;
    expect(tester.getTopLeft(read).dy, inExclusiveRange(header, command));
  });

  testWidgets('with no taint, no such line', (tester) async {
    await _card(tester, _approval());
    expect(find.text('June wants to use AWS'), findsOneWidget);
    expect(find.textContaining('which reads the pipe'), findsNothing);
  });

  testWidgets('a secret set to ask each time says so', (tester) async {
    await _card(tester, _approval(pipes: [], asks: ['AWS']));
    expect(
      find.text(
        'You set AWS to ask each time. Allow it only if this command is '
        'what you expect.',
      ),
      findsOneWidget,
    );
    expect(find.text('Allow once'), findsOneWidget);
  });

  testWidgets('with a pipe read too, it says both', (tester) async {
    await _card(
      tester,
      _approval(secrets: ['AWS', 'Stripe'], asks: ['AWS', 'Stripe']),
    );
    expect(
      find.text(
        'June has read the pipe Acme launch, and you set AWS and Stripe to '
        'ask each time. Allow it only if this command is what you expect.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('a command that keeps running says it can use the secret the '
      'whole time', (tester) async {
    await _card(tester, _approval(asks: ['AWS'], keepsRunning: true));
    expect(
      find.text(
        'It keeps running after it starts, and can use AWS the whole time.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('one that ends when done says nothing of it', (tester) async {
    await _card(tester, _approval(asks: ['AWS']));
    expect(find.textContaining('keeps running'), findsNothing);
  });
}
