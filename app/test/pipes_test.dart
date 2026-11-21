import 'package:factory_app/map.dart';
import 'package:factory_app/map_layout.dart';
import 'package:factory_app/pipes.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/update.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Agent _agent(String id, String name, {String project = 'p1'}) => Agent(
  id: id,
  name: name,
  projectId: project,
  modelAvailable: true,
  status: AgentStatus.AGENT_STATUS_IDLE,
);

Pipe _pipe({
  String id = 'pipe1',
  PipeState state = PipeState.PIPE_STATE_OPEN,
  PipePause paused = PipePause.PIPE_PAUSE_UNSPECIFIED,
  List<int> waiting = const [],
  bool online = true,
  String digits = '',
  List<String> members = const [],
  String lead = '',
  bool review = true,
}) => Pipe(
  id: id,
  name: 'Acme launch',
  projectId: 'p1',
  state: state,
  paused: paused,
  createdMs: Int64(1000),
  peer: PipePeer(
    claktoryName: "Ana's Claktory",
    ownerName: 'Ana',
    ownerEmail: 'ana@acme.example',
    projectName: 'Acme Site',
    purpose: 'We run the website.',
  ),
  purpose: 'Launch dates',
  memberIds: members,
  leadId: lead,
  limits: PipeLimits(
    messagesPerHour: 30,
    burst: 10,
    fileMbPerDay: 50,
    messagesLeft: 28,
    fileMbLeft: 50,
  ),
  reviewOutgoing: review,
  digits: digits,
  online: online,
  waitingSeqs: [for (final seq in waiting) Int64(seq)],
);

Workspace _workspace(List<Pipe> pipes, {String selected = ''}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/Users/me/factory')],
  agents: [_agent('june', 'June'), _agent('omar', 'Omar')],
  pipes: pipes,
  selectedPipeId: selected,
  selectedAgentId: 'june',
);

PipeMessage _message(
  int seq,
  String text,
  PipeAuthorKind kind, {
  PipeMessageState state = PipeMessageState.PIPE_MESSAGE_STATE_RECEIVED,
  String name = '',
  String role = '',
  String agentId = '',
}) => PipeMessage(
  seq: Int64(seq),
  timestampMs: Int64(1000 + seq),
  author: PipeAuthor(kind: kind, name: name, role: role, agentId: agentId),
  text: text,
  state: state,
);

Finder _labeled(String label) => find.byWidgetPredicate(
  (widget) => widget is Semantics && widget.properties.label == label,
);

void main() {
  Future<Harness> show(WidgetTester tester, Workspace snapshot) async {
    final harness = Harness(snapshot);
    await harness.show(tester);
    await settle(tester);
    return harness;
  }

  testWidgets('a pipe shows under its project, with a portal, where it '
      'stands and what waits for the person; a click opens it', (tester) async {
    final harness = await show(
      tester,
      _workspace([
        _pipe(waiting: [4, 5], online: false),
      ]),
    );

    expect(find.byType(PortalGlyph), findsOneWidget);
    expect(find.text('Acme launch'), findsOneWidget);
    expect(find.text('Offline'), findsOneWidget);
    expect(
      _labeled('Acme launch has 2 things waiting for your answer'),
      findsOneWidget,
    );

    await tester.tap(find.text('Acme launch'));
    await settle(tester);
    expect(harness.last.selectPipe.pipeId, 'pipe1');
  });

  testWidgets('an open pipe shows its chat in place of the agent: their '
      'text as it is, held messages with Send and Discard', (tester) async {
    final harness = await show(
      tester,
      _workspace([_pipe()], selected: 'pipe1'),
    );
    harness.backend.pipeChat.load(
      PipeTranscript(
        pipeId: 'pipe1',
        messages: [
          _message(
            1,
            '**Launch** is [here](https://evil.example)',
            PipeAuthorKind.PIPE_AUTHOR_KIND_THEIR_AGENT,
            name: 'Bo',
            role: 'Marketing',
          ),
          _message(
            2,
            'Thanks, noted',
            PipeAuthorKind.PIPE_AUTHOR_KIND_MY_AGENT,
            agentId: 'june',
            state: PipeMessageState.PIPE_MESSAGE_STATE_HELD,
          ),
          _message(
            3,
            'Paused for review',
            PipeAuthorKind.PIPE_AUTHOR_KIND_CLAKTORY,
          ),
        ],
      ),
    );
    await settle(tester);

    expect(find.byType(PipeView), findsOneWidget);
    // Not Markdown: what they wrote, as they wrote it.
    expect(
      find.text('**Launch** is [here](https://evil.example)'),
      findsOneWidget,
    );
    expect(find.textContaining('Bo', findRichText: true), findsWidgets);
    expect(find.textContaining('Waiting for you to send it'), findsOneWidget);
    expect(find.textContaining('Paused for review'), findsOneWidget);
    // The other side isn't verified, and the header says so.
    expect(find.text('Not verified'), findsOneWidget);

    // The held message's Send; the composer's is below it.
    await tester.tap(find.text('Send').first);
    await settle(tester);
    final decided = harness.last.decidePipeMessage;
    expect(decided.pipeId, 'pipe1');
    expect(decided.seq.toInt(), 2);
    expect(decided.allow, isTrue);

    // An update replaces the message in place.
    harness.backend.pipeChat.update(
      PipeMessageUpdate(
        pipeId: 'pipe1',
        message: _message(
          2,
          'Thanks, noted',
          PipeAuthorKind.PIPE_AUTHOR_KIND_MY_AGENT,
          agentId: 'june',
          state: PipeMessageState.PIPE_MESSAGE_STATE_DELIVERED,
        ),
      ),
    );
    await settle(tester);
    expect(find.textContaining('Delivered'), findsOneWidget);
    expect(find.textContaining('Waiting for you to send it'), findsNothing);
  });

  testWidgets("on a phone, the pipe's header leads with Menu", (tester) async {
    final harness = Harness(_workspace([_pipe()], selected: 'pipe1'));
    var menus = 0;
    tester.view.physicalSize = const Size(390, 844);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xFF000000),
        builder: (context, _) => AppScope(
          backend: harness.backend,
          login: harness.login,
          page: harness.page,
          appBuild: AppBuild(
            version: 'dev',
            bundlePath: '/Applications/Factory.app',
            quit: () {},
            updater: harness.updater,
          ),
          drafts: harness.drafts,
          history: harness.history,
          connections: harness.connections,
          camera: harness.camera,
          child: PipeView(pipe: _pipe(), onMenu: () => menus++),
        ),
      ),
    );
    await settle(tester);
    await tester.tap(_labeled('Menu'));
    await settle(tester);
    expect(menus, 1);
    // Menu isn't the header's own click, which opens the settings.
    expect(find.text('Pipe settings'), findsNothing);
  });

  testWidgets('the person writes in an open pipe', (tester) async {
    final harness = await show(
      tester,
      _workspace([_pipe()], selected: 'pipe1'),
    );
    harness.backend.pipeChat.load(PipeTranscript(pipeId: 'pipe1'));
    await settle(tester);

    await tester.enterText(find.byType(EditableText).last, 'Hello Ana');
    await tester.pump();
    await tester.tap(find.text('Send'));
    await settle(tester);
    final sent = harness.last.sendPipeMessage;
    expect(sent.pipeId, 'pipe1');
    expect(sent.text, 'Hello Ana');
  });

  testWidgets('a pipe waiting for approval shows the digits and who may use '
      'it; Approve sends the members and a new liaison', (tester) async {
    final harness = await show(
      tester,
      _workspace([
        _pipe(state: PipeState.PIPE_STATE_APPROVAL, digits: '4821'),
      ], selected: 'pipe1'),
    );

    expect(find.text('Ana wants to connect'), findsOneWidget);
    expect(find.text('“We run the website.”'), findsOneWidget);
    expect(find.textContaining('4'), findsWidgets);
    // Needs-you in the sidebar for the approval.
    expect(
      _labeled('Acme launch has something waiting for your answer'),
      findsOneWidget,
    );

    // Omar's toggle; the sidebar's row comes first.
    await tester.tap(find.text('Omar').last);
    await tester.pump();
    await tester.ensureVisible(find.text('Approve'));
    await tester.tap(find.text('Approve'));
    await settle(tester);
    final approve = harness.last.approvePipe;
    expect(approve.pipeId, 'pipe1');
    expect(approve.approve, isTrue);
    expect(approve.memberIds, ['omar']);
    expect(approve.leadId, '');
    expect(approve.liaisonName, 'Liaison');
  });

  testWidgets('a loop pauses a pipe; Continue resumes it', (tester) async {
    final harness = await show(
      tester,
      _workspace([
        _pipe(
          state: PipeState.PIPE_STATE_PAUSED,
          paused: PipePause.PIPE_PAUSE_LOOP,
        ),
      ], selected: 'pipe1'),
    );
    expect(find.textContaining('with no person writing'), findsOneWidget);
    await tester.tap(find.text('Continue'));
    await settle(tester);
    expect(harness.last.setPipeState.action, PipeAction.PIPE_ACTION_RESUME);
  });

  testWidgets('pipe settings send only what changed', (tester) async {
    final harness = await show(
      tester,
      _workspace([
        _pipe(members: ['june'], lead: 'june'),
      ], selected: 'pipe1'),
    );
    harness.backend.pipeChat.load(PipeTranscript(pipeId: 'pipe1'));
    await settle(tester);

    // The header opens them.
    await tester.tap(find.text('Acme launch').last);
    await settle(tester);
    expect(find.text('Pipe settings'), findsOneWidget);

    await tester.tap(find.text('Review messages before they go out'));
    await tester.pump();
    await tester.tap(find.text('Save'));
    await settle(tester);
    final update = harness.last.updatePipe;
    expect(update.pipeId, 'pipe1');
    expect(update.hasReviewOutgoing(), isTrue);
    expect(update.reviewOutgoing, isFalse);
    expect(update.hasName(), isFalse);
    expect(update.hasMembers(), isFalse);
    expect(update.hasLimits(), isFalse);
  });

  testWidgets('Connect a pipe… in project settings makes an invite and '
      'shows its code once', (tester) async {
    final harness = await show(tester, _workspace([]));
    await tester.tap(find.text('Factory'), buttons: kSecondaryButton);
    await settle(tester);
    await tester.ensureVisible(find.text('Connect a pipe…'));
    await tester.tap(find.text('Connect a pipe…'));
    await settle(tester);

    await tester.tap(find.text('Make an invite'));
    await settle(tester);
    await tester.tap(find.text('Make invite'));
    await settle(tester);
    final invite = harness.backend.sent
        .lastWhere((message) => message.hasCreatePipeInvite())
        .createPipeInvite;
    expect(invite.projectId, 'p1');
    expect(invite.name, 'Factory ⇄');
    expect(find.text('claktory-pipe://?w=x&s=y'), findsOneWidget);
  });

  testWidgets('a code from someone else: its preview, then Connect', (
    tester,
  ) async {
    final harness = await show(tester, _workspace([]));
    harness.backend.pipeInvitePreview = PipeInvitePreview(
      code: 'claktory-pipe://?w=a',
      name: 'Acme launch',
      peer: PipePeer(
        ownerName: 'Ana',
        claktoryName: "Ana's Claktory",
        projectName: 'Acme Site',
        purpose: 'We run the website.',
      ),
    );
    await tester.tap(find.text('Factory'), buttons: kSecondaryButton);
    await settle(tester);
    await tester.ensureVisible(find.text('Connect a pipe…'));
    await tester.tap(find.text('Connect a pipe…'));
    await settle(tester);
    await tester.tap(find.text('I have a code'));
    await settle(tester);
    await tester.enterText(
      find.byType(EditableText).last,
      'claktory-pipe://?w=a',
    );
    await tester.pump();
    await tester.tap(find.text('Continue'));
    await settle(tester);

    expect(find.text('Connect Factory with Acme Site?'), findsOneWidget);
    expect(find.textContaining('not verified'), findsOneWidget);
    await tester.tap(find.text('Connect'));
    await settle(tester);
    final accept = harness.backend.sent
        .lastWhere((message) => message.hasAcceptPipeInvite())
        .acceptPipeInvite;
    expect(accept.code, 'claktory-pipe://?w=a');
    expect(accept.projectId, 'p1');
    expect(accept.name, 'Acme launch');
  });

  testWidgets("an agent that read a pipe asks before using a secret: the "
      'card in its conversation', (tester) async {
    final harness = Harness(_workspace([]));
    harness.backend.transcript.load(
      Transcript(
        agentId: 'june',
        entries: [
          Entry(
            seq: Int64(7),
            secretApproval: SecretApproval(
              agentId: 'june',
              secretNames: ['AWS'],
              pipeNames: ['Acme launch'],
              command: 'aws s3 ls',
              state: SecretApprovalState.SECRET_APPROVAL_STATE_PENDING,
            ),
          ),
        ],
      ),
    );
    await harness.show(tester);
    await settle(tester);

    expect(find.text('June wants to use AWS'), findsOneWidget);
    expect(find.text('aws s3 ls'), findsOneWidget);
    await tester.tap(find.text('Allow once'));
    await settle(tester);
    final decided = harness.last.decideSecretApproval;
    expect(decided.agentId, 'june');
    expect(decided.seq.toInt(), 7);
    expect(decided.allow, isTrue);
  });

  test("the Map's portal pads sit at the top right of their project's box, "
      "named for the other Claktory", () {
    const box = Rect.fromLTWH(0, 0, 400, 200);
    const layout = MapLayout(
      size: Size(400, 200),
      boxes: [
        MapBox(
          id: 'project:p1',
          kind: MapBoxKind.project,
          rect: box,
          label: 'Factory',
        ),
      ],
      chips: [],
      wires: [],
      dots: [],
      related: {},
      reads: {},
      writes: {},
    );
    final pads = portalPads(layout, [
      _pipe(),
      _pipe(id: 'other')..projectId = 'p2',
    ]);
    expect(pads, hasLength(1));
    expect(pads.single.label, "Ana's Claktory");
    expect(pads.single.rect.right, box.right - mapCell);
    expect(pads.single.rect.top, greaterThan(box.top));
    expect(pads.single.rect.left, greaterThan(box.center.dx));
  });
}
