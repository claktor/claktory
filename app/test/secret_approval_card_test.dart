// The card a held secret use waits on (SecretApproval): under who asks, a
// line per pipe the command may come from (P1's taint), the agent's own
// read or heard through others.
import 'package:factory_app/decision.dart';
import 'package:factory_app/pipes.dart' show SecretApprovalCard;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart' show LogicalKeyboardKey;
import 'package:flutter/widgets.dart';
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
  testWidgets('it says where the command may come from, under who asks; a '
      'pipe read before #346 says nothing', (tester) async {
    await _card(
      tester,
      _approval(
        taint: [
          PipeTaint(pipeName: 'Acme launch', pipeId: 'pp'),
          PipeTaint(kind: 'browser', pipeName: 'https://docs.example.com'),
        ],
      ),
    );
    final read = find.text(
      'from June, which read a page on https://docs.example.com',
    );
    expect(read, findsOneWidget);
    expect(
      find.descendant(
        of: find.byType(SecretApprovalCard),
        matching: find.textContaining('Acme launch'),
      ),
      findsNothing,
    );
    final header = tester.getTopLeft(find.text('June wants to use AWS')).dy;
    final command = tester.getTopLeft(find.text('aws s3 ls')).dy;
    expect(tester.getTopLeft(read).dy, inExclusiveRange(header, command));
  });

  testWidgets('the command shows every hidden character by its code', (
    tester,
  ) async {
    await _card(tester, _approval()..command = 'aws s3 rm \u202Es3://x\u200B');
    expect(find.text(r'aws s3 rm \u{202E}s3://x\u{200B}'), findsOneWidget);
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

  testWidgets('with a page read too, it says both; a pipe read says '
      'nothing', (tester) async {
    await _card(
      tester,
      _approval(
        secrets: ['AWS', 'Stripe'],
        asks: ['AWS', 'Stripe'],
        taint: [
          PipeTaint(pipeName: 'Acme launch', pipeId: 'pp'),
          PipeTaint(kind: 'browser', pipeName: 'https://docs.example.com'),
        ],
      ),
    );
    expect(
      find.text(
        'June has read a page on https://docs.example.com, and you set AWS '
        'and Stripe to ask each time. Allow it only if this command is what '
        'you expect.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('an old card from a pipe read alone just says Claktory asks', (
    tester,
  ) async {
    await _card(tester, _approval());
    expect(
      find.text(
        'Claktory asks you before June uses a secret. Allow it only if this '
        'command is what you expect.',
      ),
      findsOneWidget,
    );
  });

  testWidgets('a page read, not a pipe, says so', (tester) async {
    await _card(
      tester,
      _approval(
        pipes: [],
        taint: [
          PipeTaint(kind: 'browser', pipeName: 'https://docs.example.com'),
        ],
      ),
    );
    expect(
      find.text(
        'June has read a page on https://docs.example.com, so Claktory asks '
        'you before it uses a secret. Allow it only if this command is what '
        'you expect.',
      ),
      findsOneWidget,
    );
    expect(
      find.text('from June, which read a page on https://docs.example.com'),
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

  testWidgets("a comment typed is one Submit, which doesn't allow it "
      '(#370); decided, the card shows the comment', (tester) async {
    final harness = await _card(tester, _approval());
    await tester.tap(find.text('Add a comment'));
    await settle(tester);
    expect(find.text('June gets it with your answer.'), findsOneWidget);
    final field = find.descendant(
      of: find.byType(DecisionComment),
      matching: find.byType(EditableText),
    );
    await tester.enterText(field, 'Use the staging bucket.');
    await tester.pump();
    expect(find.text("Don't allow"), findsNothing);
    expect(find.text('Allow once'), findsNothing);
    expect(
      find.text('Not allowed this time. June gets your comment.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Submit'));
    await settle(tester);
    var decided = harness.last.decideSecretApproval;
    expect(decided.reply, isTrue);
    expect(decided.allow, isFalse);
    expect(decided.comment, 'Use the staging bucket.');

    // Return is Submit too; cleared, the choices are back.
    final before = harness.backend.sent.length;
    await tester.enterText(field, 'Try again tomorrow.');
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    decided = harness.backend.sent
        .skip(before)
        .singleWhere((m) => m.hasDecideSecretApproval())
        .decideSecretApproval;
    expect(decided.reply, isTrue);
    expect(decided.allow, isFalse);
    await tester.enterText(field, '');
    await tester.pump();
    expect(find.text('Allow once'), findsOneWidget);

    // Not allowed, with the comment.
    harness.backend.transcript.load(
      Transcript(
        agentId: 'june',
        entries: [
          Entry(
            seq: Int64(7),
            secretApproval: _approval()
              ..state = SecretApprovalState.SECRET_APPROVAL_STATE_DENIED
              ..replied = true
              ..comment = 'Use the staging bucket.',
          ),
        ],
      ),
    );
    await settle(tester);
    expect(
      find.textContaining(
        'Not allowed · “Use the staging bucket.”',
        findRichText: true,
      ),
      findsOneWidget,
    );
  });
}
