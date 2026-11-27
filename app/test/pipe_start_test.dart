import 'package:factory_app/devices.dart' show QrCode;
import 'package:factory_app/decision.dart';
import 'package:factory_app/pipe_start.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart' show CameraAccess;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'secret_clipboard_fake.dart';

PipeStart _start({
  PipeStartKind kind = PipeStartKind.PIPE_START_KIND_INVITE,
  PipeStartState state = PipeStartState.PIPE_START_STATE_WAITING,
  String pipeId = '',
}) => PipeStart(
  id: 'start1',
  agentId: 'rivet',
  kind: kind,
  projectId: 'p2',
  name: 'Launch with Acme',
  purpose: 'Launch dates and screenshots',
  state: state,
  pipeId: pipeId,
  endedMs: Int64(state == PipeStartState.PIPE_START_STATE_WAITING ? 0 : 5000),
);

Entry _entry(PipeStart start) =>
    Entry(seq: Int64(4), timestampMs: Int64(4000), pipeStart: start);

Workspace _workspace({List<Pipe> pipes = const []}) => Workspace(
  projects: [
    Project(id: 'p1', name: 'Factory', path: '/Users/me/factory'),
    Project(id: 'p2', name: 'Website', path: '/Users/me/site'),
  ],
  agents: [
    Agent(
      id: 'rivet',
      name: 'Rivet',
      role: 'Admin',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
      waitingSeqs: [Int64(4)],
    ),
  ],
  pipes: pipes,
  selectedAgentId: 'rivet',
);

/// Today at 14:20: the expiry reads as a time alone only on its own day.
DateTime _todayAt1420() {
  final now = DateTime.now();
  return DateTime(now.year, now.month, now.day, 14, 20);
}

void main() {
  Future<Harness> show(
    WidgetTester tester,
    PipeStart start, {
    List<Pipe> pipes = const [],
    Size size = const Size(1200, 900),
  }) async {
    final harness = Harness(_workspace(pipes: pipes));
    await harness.show(tester, size: size);
    harness.backend.transcript.load(
      Transcript(agentId: 'rivet', entries: [_entry(start)]),
    );
    await settle(tester);
    return harness;
  }

  Finder field(String label) => find.descendant(
    of: find
        .ancestor(of: find.text(label), matching: find.byType(Column))
        .first,
    matching: find.byType(EditableText),
  );

  testWidgets('an invite card: the suggestion to check, Make invite, then '
      'the code once, with when it stops working', (tester) async {
    final harness = await show(
      tester,
      _start(),
      pipes: [
        Pipe(
          id: 'p1',
          name: 'Acme invite',
          projectId: 'p2',
          state: PipeState.PIPE_STATE_INVITED,
          inviteExpiresMs: Int64(_todayAt1420().millisecondsSinceEpoch),
        ),
      ],
    );
    expect(
      find.text('Rivet suggests inviting someone to a pipe'),
      findsOneWidget,
    );
    expect(find.text('Website'), findsWidgets);
    expect(find.text('Launch with Acme'), findsOneWidget);
    expect(find.text('Launch dates and screenshots'), findsOneWidget);

    await tester.enterText(field('Your name for the pipe'), 'Acme launch');
    await tester.pump();
    await tester.tap(find.text('Make invite'));
    await settle(tester);
    final made = harness.last.createPipeInvite;
    expect(made.startId, 'start1');
    expect(made.projectId, 'p2');
    expect(made.name, 'Acme launch');
    expect(made.purpose, 'Launch dates and screenshots');

    expect(find.text('claktory-pipe://?w=x&s=y'), findsOneWidget);
    expect(
      find.text(
        'It works once, until 14:20. It won’t show again, so copy it now.',
      ),
      findsOneWidget,
    );
    // The backend closes the card; the code stays while it shows.
    harness.backend.transcript.load(
      Transcript(
        agentId: 'rivet',
        entries: [
          _entry(
            _start(state: PipeStartState.PIPE_START_STATE_DONE, pipeId: 'p1'),
          ),
        ],
      ),
    );
    await settle(tester);
    expect(find.text('claktory-pipe://?w=x&s=y'), findsOneWidget);

    // Marked sensitive, for as long as it works.
    final clipboard = FakeSecretClipboard.install();
    // No share sheet on a Mac: Copy code alone.
    expect(find.text('Share'), findsNothing);
    await tester.tap(find.text('Copy code'));
    await settle(tester);
    expect(clipboard.copies.single.$1, 'claktory-pipe://?w=x&s=y');
    expect(find.text('Copied'), findsOneWidget);
    // To scan in person, the code as a QR code, once asked for (friend
    // bug #2).
    expect(find.byType(QrCode), findsNothing);
    await tester.tap(find.text('Show QR code'));
    await settle(tester);
    expect(
      tester.widget<QrCode>(find.byType(QrCode)).text,
      'claktory-pipe://?w=x&s=y',
    );
    expect(find.text('Show QR code'), findsNothing);
    // Cleared once dead (14:20 today, or 10 minutes when that's past).
    await tester.pump(Duration(seconds: clipboard.copies.single.$2));
    expect(clipboard.cleared, [1]);
  });

  testWidgets("on an iPhone the code goes out through the share sheet, "
      'with Copy code beside it', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final shared = <Object?>[];
    const device = MethodChannel('factory/device');
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(device, (
      call,
    ) async {
      if (call.method == 'share') shared.add(call.arguments);
      return true;
    });
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        device,
        null,
      ),
    );
    await show(tester, _start(), size: const Size(390, 844));
    await tester.enterText(field('Your name for the pipe'), 'Acme launch');
    await tester.pump();
    await tester.ensureVisible(find.text('Make invite'));
    await pumpFrames(tester);
    await tester.tap(find.text('Make invite'));
    await settle(tester);
    expect(find.text('Copy code'), findsOneWidget);
    await tester.ensureVisible(find.text('Share'));
    await pumpFrames(tester);
    await tester.tap(find.text('Share'));
    await settle(tester);
    expect(shared, hasLength(1));
    expect((shared.single! as Map)['text'], 'claktory-pipe://?w=x&s=y');
    debugDefaultTargetPlatformOverride = null;
  });

  testWidgets('a code card scans the code where there is a camera, then '
      'reads it as a pasted one, still waiting for Connect (friend bug #2)', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    harness.camera.state = CameraAccess.granted;
    harness.backend.pipeInvitePreview = PipeInvitePreview(
      code: 'claktory-pipe://?w=a&s=b',
      name: 'Website for Ana',
      peer: PipePeer(ownerName: 'Ana', projectName: 'Acme Site'),
    );
    await harness.show(tester, size: const Size(1200, 900));
    harness.backend.transcript.load(
      Transcript(
        agentId: 'rivet',
        entries: [_entry(_start(kind: PipeStartKind.PIPE_START_KIND_ACCEPT))],
      ),
    );
    // Asked as it opens, whether there's a camera.
    await settle(tester);
    await settle(tester);
    expect(
      find.text('Paste the code you were sent, or scan it.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Scan code'));
    await pumpFrames(tester);
    expect(find.text('Scan a pipe invite'), findsOneWidget);
    harness.camera.scan('https://example.com');
    await pumpFrames(tester);
    expect(find.text("This isn't a pipe invite."), findsOneWidget);
    harness.camera.scan('claktory-pipe://?w=a&s=b');
    await pumpFrames(tester);
    await settle(tester);
    expect(find.text('Scan a pipe invite'), findsNothing);
    expect(harness.last.readPipeInvite.code, 'claktory-pipe://?w=a&s=b');
    expect(find.text('The code says'), findsOneWidget);
    // Nothing connects until the person chooses Connect.
    expect(harness.backend.sent.where((m) => m.hasAcceptPipeInvite()), isEmpty);
    expect(find.text('Connect'), findsOneWidget);
  });

  testWidgets('without a camera, a code card has no Scan code', (tester) async {
    await show(tester, _start(kind: PipeStartKind.PIPE_START_KIND_ACCEPT));
    expect(find.text('Scan code'), findsNothing);
    expect(find.text('Paste the code you were sent.'), findsOneWidget);
  });

  testWidgets('a code card: what the code says sits apart, marked not '
      "verified, from the person's own name for the pipe; Connect uses it", (
    tester,
  ) async {
    final harness = await show(
      tester,
      _start(kind: PipeStartKind.PIPE_START_KIND_ACCEPT),
    );
    harness.backend.pipeInvitePreview = PipeInvitePreview(
      code: 'claktory-pipe://?w=a&s=b',
      name: 'Website for Ana',
      peer: PipePeer(
        ownerName: 'Ana',
        claktoryName: "Ana's Claktory",
        projectName: 'Acme Site',
        purpose: 'We run the website.',
      ),
    );
    expect(
      find.text('Rivet suggests joining a pipe with a code you were sent'),
      findsOneWidget,
    );
    await tester.enterText(field('Code'), ' claktory-pipe://?w=a&s=b ');
    await tester.pump();
    await tester.tap(find.text('Continue'));
    await settle(tester);
    expect(harness.last.readPipeInvite.code, 'claktory-pipe://?w=a&s=b');

    expect(find.text('The code says'), findsOneWidget);
    expect(find.text('Not verified'), findsOneWidget);
    expect(find.text("From Ana · Ana's Claktory"), findsOneWidget);
    expect(find.text('“We run the website.”'), findsOneWidget);
    // The person's own label, as suggested; the code's name stays in its box.
    expect(
      tester
          .widget<EditableText>(field('Your name for the pipe'))
          .controller
          .text,
      'Launch with Acme',
    );
    expect(find.text('Ana sees this.'), findsOneWidget);

    await tester.tap(find.text('Connect'));
    await settle(tester);
    final accepted = harness.last.acceptPipeInvite;
    expect(accepted.code, 'claktory-pipe://?w=a&s=b');
    expect(accepted.startId, 'start1');
    expect(accepted.projectId, 'p2');
    expect(accepted.name, 'Launch with Acme');
  });

  testWidgets('Dismiss closes the card', (tester) async {
    final harness = await show(tester, _start());
    await tester.tap(find.text('Dismiss'));
    await settle(tester);
    expect(harness.last.dismissPipeStart.agentId, 'rivet');
    expect(harness.last.dismissPipeStart.startId, 'start1');
  });

  testWidgets('Dismiss takes a comment for the agent', (tester) async {
    final harness = await show(tester, _start());
    expect(
      find.text('Why not? Rivet sees this only if you dismiss.'),
      findsOneWidget,
    );
    await tester.enterText(
      find.descendant(
        of: find.byType(DecisionComment),
        matching: find.byType(EditableText),
      ),
      'Not with Acme',
    );
    await tester.tap(find.text('Dismiss'));
    await settle(tester);
    expect(harness.last.dismissPipeStart.comment, 'Not with Acme');
  });

  testWidgets('a closed card says how it ended, and Open pipe opens what it '
      'made', (tester) async {
    final harness = await show(
      tester,
      _start(state: PipeStartState.PIPE_START_STATE_DONE, pipeId: 'pipe9'),
      pipes: [
        Pipe(
          id: 'pipe9',
          name: 'Acme launch',
          projectId: 'p2',
          state: PipeState.PIPE_STATE_INVITED,
        ),
      ],
    );
    expect(
      find.textContaining('Invite made: Acme launch', findRichText: true),
      findsOneWidget,
    );
    expect(find.text('Make invite'), findsNothing);
    await tester.tap(find.text('Open pipe'));
    await settle(tester);
    expect(harness.last.selectPipe.pipeId, 'pipe9');
  });

  for (final kind in [
    PipeStartKind.PIPE_START_KIND_INVITE,
    PipeStartKind.PIPE_START_KIND_ACCEPT,
  ]) {
    testWidgets('the ${kind.name} card fits a phone at the largest text', (
      tester,
    ) async {
      tester.platformDispatcher.textScaleFactorTestValue = 2;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await show(tester, _start(kind: kind), size: const Size(390, 844));
      expect(find.byType(PipeStartCard), findsOneWidget);
      expect(
        find.text(
          kind == PipeStartKind.PIPE_START_KIND_ACCEPT
              ? 'Continue'
              : 'Make invite',
        ),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
    });
  }

  test('Needs you says what the card asks', () {
    expect(pipeStartSummary(_start()), 'Invite someone to a pipe');
    expect(
      pipeStartSummary(_start(kind: PipeStartKind.PIPE_START_KIND_ACCEPT)),
      'Join a pipe',
    );
  });
}
