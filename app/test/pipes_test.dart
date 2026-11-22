import 'package:factory_app/controls.dart' show Field, Toggle;
import 'package:factory_app/decision.dart';
import 'package:factory_app/map.dart';
import 'package:factory_app/map_layout.dart';
import 'package:factory_app/pipe_timeline.dart' show pipeItemKey;
import 'package:factory_app/pipes.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/taint_label.dart';
import 'package:factory_app/update.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Agent _agent(
  String id,
  String name, {
  String project = 'p1',
  String role = '',
  bool limited = false,
  List<String> read = const [],
}) => Agent(
  id: id,
  name: name,
  role: role,
  quarantined: limited,
  taintedByPipes: read,
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
  List<String> limited = const [],
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
  quarantinedIds: limited,
  canRunCommandsIds: [
    for (final member in members)
      if (!limited.contains(member)) member,
  ],
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
  Future<Harness> show(
    WidgetTester tester,
    Workspace snapshot, {
    Size size = const Size(1200, 900),
  }) async {
    final harness = Harness(snapshot);
    await harness.show(tester, size: size);
    await settle(tester);
    return harness;
  }

  testWidgets('a pipe shows under its project, with a portal, where it '
      'stands and what waits for the person; a click opens it', (tester) async {
    final harness = await show(
      tester,
      _workspace([
        _pipe(waiting: [4, 5], online: false, members: ['june'], lead: 'june'),
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

  testWidgets('a message held on its way in says why, and Let in lets it '
      'in', (tester) async {
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
            'Can you send the deck?',
            PipeAuthorKind.PIPE_AUTHOR_KIND_THEIR_AGENT,
            name: 'Bo',
            state: PipeMessageState.PIPE_MESSAGE_STATE_HELD_INCOMING,
          )..note = 'Waiting for you: let it in, or discard it.',
          _message(
            2,
            'And the logo',
            PipeAuthorKind.PIPE_AUTHOR_KIND_THEIR_AGENT,
            name: 'Bo',
            state: PipeMessageState.PIPE_MESSAGE_STATE_HELD_INCOMING,
          ),
        ],
      ),
    );
    await settle(tester);
    // The note says it all; with none, the limits held it.
    expect(
      find.textContaining(
        RegExp(r'^Waiting for you: let it in, or discard it\. · '),
      ),
      findsOneWidget,
    );
    expect(
      find.textContaining(
        RegExp('^Over this pipe’s limits: let it in, or discard it\\. · '),
      ),
      findsOneWidget,
    );
    expect(find.textContaining('Held:'), findsNothing);

    // The older message's (the list builds the newest first).
    await tester.tap(find.text('Let in').last);
    await settle(tester);
    final decided = harness.last.decidePipeMessage;
    expect(decided.seq.toInt(), 1);
    expect(decided.allow, isTrue);
  });

  testWidgets('a held message takes a comment for this side\'s agent only: '
      'the lead for one coming in, never the other side', (tester) async {
    final harness = await show(
      tester,
      _workspace([
        _pipe(members: ['june'], lead: 'june'),
      ], selected: 'pipe1'),
    );
    harness.backend.pipeChat.load(
      PipeTranscript(
        pipeId: 'pipe1',
        messages: [
          _message(
            1,
            'Can you send the deck?',
            PipeAuthorKind.PIPE_AUTHOR_KIND_THEIR_AGENT,
            name: 'Bo',
            state: PipeMessageState.PIPE_MESSAGE_STATE_HELD_INCOMING,
          ),
        ],
      ),
    );
    await settle(tester);
    await tester.tap(find.text('Add a comment'));
    await settle(tester);
    expect(find.text("Only June sees it, not Ana's Claktory."), findsOneWidget);
    await tester.enterText(
      find.descendant(
        of: find.byType(DecisionComment),
        matching: find.byType(EditableText),
      ),
      'Only the public deck.',
    );
    await tester.tap(find.text('Let in'));
    await settle(tester);
    final decided = harness.last.decidePipeMessage;
    expect(decided.allow, isTrue);
    expect(decided.comment, 'Only the public deck.');
  });

  testWidgets("the person's own held message has no one to comment to", (
    tester,
  ) async {
    final harness = await show(
      tester,
      _workspace([
        _pipe(members: ['june'], lead: 'june'),
      ], selected: 'pipe1'),
    );
    harness.backend.pipeChat.load(
      PipeTranscript(
        pipeId: 'pipe1',
        messages: [
          _message(
            1,
            'Hello Ana',
            PipeAuthorKind.PIPE_AUTHOR_KIND_ME,
            state: PipeMessageState.PIPE_MESSAGE_STATE_HELD,
          ),
        ],
      ),
    );
    await settle(tester);
    expect(find.textContaining('Waiting for you to send it'), findsOneWidget);
    expect(find.text('Add a comment'), findsNothing);
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

  group('whose Claktory is at the other end', () {
    const unconfirmed =
        'Messages here come only from the Claktory you connected to, but '
        "its name and email are only what it says about itself. Claktory "
        "hasn't confirmed whose it is. ";
    const bothSignIn =
        "${unconfirmed}When you're both signed in to claktory.com, Claktory "
        'confirms it and shows their email here.';
    const anaSignsIn =
        '${unconfirmed}When Ana signs in to claktory.com, Claktory confirms '
        'it and shows their email here.';

    Workspace signedIn(List<Pipe> pipes) => _workspace(pipes, selected: 'pipe1')
      ..account = ClaktoryAccount(
        state: ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_IN,
        email: 'matt@example.com',
      );

    Pipe verified() => _pipe()
      ..peer.verified = true
      ..peer.verifiedEmail = 'ana@acme.example';

    testWidgets("the header's Not verified opens what it means, beside it", (
      tester,
    ) async {
      await show(tester, signedIn([_pipe()]));
      expect(find.text(anaSignsIn), findsNothing);
      await tester.tap(find.text('Not verified'));
      await settle(tester);
      expect(find.text(anaSignsIn), findsOneWidget);
      expect(find.text('Sign in…'), findsNothing, reason: 'signed in');
      // Not the settings, as the rest of the header opens.
      expect(find.text('Pipe settings'), findsNothing);
      // A tap anywhere else closes it.
      await tester.tapAt(const Offset(600, 700));
      await settle(tester);
      expect(find.text(anaSignsIn), findsNothing);
    });

    testWidgets('this Claktory not signed in: both, and Sign in…', (
      tester,
    ) async {
      await show(tester, _workspace([_pipe()], selected: 'pipe1'));
      await tester.tap(find.text('Not verified'));
      await settle(tester);
      expect(find.text(bothSignIn), findsOneWidget);
      expect(find.text('Sign in…'), findsOneWidget);
    });

    testWidgets("the approval card's mark opens it too", (tester) async {
      await show(
        tester,
        signedIn([_pipe(state: PipeState.PIPE_STATE_APPROVAL, digits: '4821')]),
      );
      await tester.tap(find.text('Not verified').last);
      await settle(tester);
      expect(find.text(anaSignsIn), findsOneWidget);
    });

    testWidgets("the pipe's settings say it first", (tester) async {
      await show(tester, signedIn([_pipe()]));
      await tester.tap(find.text('Acme launch').last);
      await settle(tester);
      expect(find.text('Pipe settings'), findsOneWidget);
      expect(find.text(anaSignsIn), findsOneWidget);
      expect(
        tester.getTopLeft(find.text(anaSignsIn)).dy,
        lessThan(tester.getTopLeft(find.text('Name')).dy),
      );
    });

    testWidgets('verified: the email, and what that means', (tester) async {
      await show(tester, signedIn([verified()]));
      expect(find.text('Not verified'), findsNothing);
      await tester.tap(find.text('Verified · ana@acme.example'));
      await settle(tester);
      expect(
        find.text(
          "Claktory confirmed this pipe is with ana@acme.example's Claktory.",
        ),
        findsOneWidget,
      );
    });

    test('with no name, "the other side"', () {
      final pipe = _pipe()..peer = PipePeer();
      expect(
        notVerifiedNote(pipe, signedIn: true),
        contains('When the other side signs in'),
      );
    });

    testWidgets('no longer vouched for: a quiet notice, and OK dismisses it', (
      tester,
    ) async {
      final pipe = _pipe()
        ..peer.identityChange = PipeIdentityChange.PIPE_IDENTITY_CHANGE_LAPSED
        ..peer.previousEmail = 'ana@acme.example';
      final harness = await show(tester, signedIn([pipe]));
      expect(find.text('No longer verified'), findsOneWidget);
      expect(
        find.text(
          "Claktory can no longer confirm this pipe is with ana@acme.example's "
          'Claktory, perhaps because they signed out of claktory.com. '
          'Messages still come only from the same Claktory.',
        ),
        findsOneWidget,
      );
      await tester.tap(find.text('OK'));
      await settle(tester);
      expect(harness.last.acceptPipeIdentity.pipeId, 'pipe1');
    });

    testWidgets('a new email on the same account: a quiet notice, OK '
        'accepts it', (tester) async {
      final pipe = verified()
        ..peer.verifiedEmail = 'ana@newco.example'
        ..peer.identityChange =
            PipeIdentityChange.PIPE_IDENTITY_CHANGE_EMAIL_CHANGED
        ..peer.previousEmail = 'ana@acme.example';
      final harness = await show(tester, signedIn([pipe]));
      expect(find.text('Email changed'), findsOneWidget);
      expect(
        find.text(
          "This pipe's Claktory is now ana@newco.example, on the same account "
          "as ana@acme.example. It's still verified.",
        ),
        findsOneWidget,
      );
      expect(find.text('Verified · ana@newco.example'), findsOneWidget);
      await tester.tap(find.text('OK'));
      await settle(tester);
      expect(harness.last.acceptPipeIdentity.pipeId, 'pipe1');
    });

    testWidgets('another account: a warning, and Trust asks first', (
      tester,
    ) async {
      final pipe = _pipe()
        ..peer.verifiedEmail = 'bo@example.com'
        ..peer.identityChange =
            PipeIdentityChange.PIPE_IDENTITY_CHANGE_ACCOUNT_CHANGED
        ..peer.previousEmail = 'ana@acme.example';
      final harness = await show(tester, signedIn([pipe]));
      expect(
        find.text(
          "This pipe's Claktory is now confirmed as bo@example.com's, not "
          "ana@acme.example's.",
        ),
        findsOneWidget,
      );
      expect(
        find.text(
          'Check with Ana before you go on. Until you check, their messages '
          "wait for you in Needs you, and your agents can't send here.",
        ),
        findsOneWidget,
      );
      expect(find.text('Not verified'), findsOneWidget);
      await tester.tap(find.text('Trust this Claktory…'));
      await settle(tester);
      expect(
        find.text("Trust this Claktory as bo@example.com's?"),
        findsOneWidget,
      );
      expect(
        find.text(
          'Messages waiting from it go through, and your agents can send here '
          'again. Do this only if Ana told you about the change.',
        ),
        findsOneWidget,
      );
      await tester.tap(find.text('Cancel'));
      await settle(tester);
      expect(
        harness.backend.sent.where((m) => m.hasAcceptPipeIdentity()),
        isEmpty,
      );
      await tester.tap(find.text('Trust this Claktory…'));
      await settle(tester);
      await tester.tap(find.text('Trust'));
      await settle(tester);
      expect(harness.last.acceptPipeIdentity.pipeId, 'pipe1');
    });

    testWidgets('another account with none vouched: "Trust this Claktory?"', (
      tester,
    ) async {
      final pipe = _pipe()
        ..peer.identityChange =
            PipeIdentityChange.PIPE_IDENTITY_CHANGE_ACCOUNT_CHANGED
        ..peer.previousEmail = 'ana@acme.example';
      await show(tester, signedIn([pipe]));
      expect(
        find.text(
          "This pipe's Claktory is no longer confirmed as "
          "ana@acme.example's.",
        ),
        findsOneWidget,
      );
      await tester.tap(find.text('Trust this Claktory…'));
      await settle(tester);
      expect(find.text('Trust this Claktory?'), findsOneWidget);
      expect(
        find.text(
          'Messages waiting from it go through, and your agents can send here '
          'again. The pipe then shows Not verified, since nothing confirms '
          'whose Claktory it is now. Do this only if Ana told you about the '
          'change.',
        ),
        findsOneWidget,
      );
    });
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
        _pipe(state: PipeState.PIPE_STATE_APPROVAL, digits: '4821')
          ..suggestedLiaisonName = 'Kai',
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
    // Claktory's suggestion, prefilled.
    expect(approve.liaisonName, 'Kai');
    // Both reviews are off for a new pipe unless the person turns them on.
    expect(approve.hasReviewIncoming(), isTrue);
    expect(approve.reviewIncoming, isFalse);
    expect(approve.hasReviewOutgoing(), isTrue);
    expect(approve.reviewOutgoing, isFalse);
  });

  testWidgets('Decline takes a comment for this side only', (tester) async {
    final harness = await show(
      tester,
      _workspace([
        _pipe(state: PipeState.PIPE_STATE_APPROVAL, digits: '4821'),
      ], selected: 'pipe1'),
    );
    final add = find.text('Add a comment');
    await tester.ensureVisible(add);
    await tester.tap(add);
    await settle(tester);
    expect(
      find.text("Only your agents see it, not Ana's Claktory."),
      findsOneWidget,
    );
    await tester.enterText(
      find.descendant(
        of: find.byType(DecisionComment),
        matching: find.byType(EditableText),
      ),
      'Not this client.',
    );
    await tester.ensureVisible(find.text('Decline'));
    await tester.tap(find.text('Decline'));
    await settle(tester);
    final approve = harness.last.approvePipe;
    expect(approve.approve, isFalse);
    expect(approve.comment, 'Not this client.');
  });

  testWidgets('Review incoming, turned on at approval, goes with it', (
    tester,
  ) async {
    final harness = await show(
      tester,
      _workspace([
        _pipe(state: PipeState.PIPE_STATE_APPROVAL, digits: '4821'),
      ], selected: 'pipe1'),
    );
    expect(
      find.text(
        'Messages from the other side reach your agents as they arrive.',
      ),
      findsOneWidget,
    );
    final incoming = find.text('Review messages before your agents read them');
    await tester.ensureVisible(incoming);
    await tester.tap(incoming);
    await tester.pump();
    expect(
      find.text(
        'Each message from the other side waits here until you let it in.',
      ),
      findsOneWidget,
    );
    await tester.ensureVisible(find.text('Approve'));
    await tester.tap(find.text('Approve'));
    await settle(tester);
    final approve = harness.last.approvePipe;
    expect(approve.reviewIncoming, isTrue);
    expect(approve.reviewOutgoing, isFalse);
  });

  testWidgets("a join that can't reach the other side says why, since when, "
      'and what to check', (tester) async {
    final harness = await show(
      tester,
      _workspace([
        _pipe(state: PipeState.PIPE_STATE_JOINING)
          ..offlineReason = 'no answer in time'
          ..offlineSinceMs = Int64(1000),
      ], selected: 'pipe1'),
    );
    expect(find.text('Can’t reach Ana yet'), findsOneWidget);
    expect(
      find.textContaining(
        RegExp(
          r'^Claktory keeps trying until the code expires\. '
          r'Last try: no answer in time\.$',
        ),
      ),
      findsOneWidget,
    );
    // Only when the backend says it runs on a Mac.
    expect(find.text(pipeLocalNetworkHint), findsNothing);
    await harness.apply(
      tester,
      harness.backend.workspace.workspace.deepCopy()..os = 'macos',
    );
    await settle(tester);
    expect(find.text(pipeLocalNetworkHint), findsOneWidget);
  });

  testWidgets('an open pipe whose other side is offline says why', (
    tester,
  ) async {
    final harness = await show(
      tester,
      _workspace([
        _pipe(online: false)
          ..offlineReason = 'the connection was lost'
          ..offlineSinceMs = Int64(1000),
      ], selected: 'pipe1'),
    );
    harness.backend.pipeChat.load(PipeTranscript(pipeId: 'pipe1'));
    await settle(tester);
    expect(
      find.textContaining(
        RegExp(
          r'^Ana’s Claktory has been offline since .*: the connection was '
          r'lost\. Messages wait until it’s back\.$',
        ),
      ),
      findsOneWidget,
    );
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
      // Tall enough to hold the whole form, so nothing scrolls between taps.
      size: const Size(1200, 1500),
    );
    harness.backend.pipeChat.load(PipeTranscript(pipeId: 'pipe1'));
    await settle(tester);

    // The header opens them.
    await tester.tap(find.text('Acme launch').last);
    await settle(tester);
    expect(find.text('Pipe settings'), findsOneWidget);

    await tester.tap(find.text('Review messages before your agents read them'));
    await tester.pump();
    await tester.tap(find.text('Review messages before they go out'));
    await tester.pump();
    await tester.ensureVisible(find.text('Save'));
    await tester.tap(find.text('Save'));
    await settle(tester);
    final update = harness.last.updatePipe;
    expect(update.pipeId, 'pipe1');
    expect(update.hasReviewOutgoing(), isTrue);
    expect(update.reviewOutgoing, isFalse);
    expect(update.hasReviewIncoming(), isTrue);
    expect(update.reviewIncoming, isTrue);
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

    // A pipe's liaison sits on its pad, which grows to hold its face.
    final kai = _agent('kai', 'Kai')..pipeId = 'pipe1';
    final withKai = portalPads(layout, [_pipe()], liaisons: {'pipe1': kai});
    expect(withKai.single.liaison?.id, 'kai');
    expect(withKai.single.rect.width, greaterThan(pads.single.rect.width));
  });

  test('a liaison leaves the Map as a chip, with its lines', () {
    final map = ClaktoryMap(
      nodes: [
        MapNode(id: 'agent:june', kind: MapNodeKind.MAP_NODE_KIND_AGENT),
        MapNode(id: 'agent:kai', kind: MapNodeKind.MAP_NODE_KIND_AGENT),
      ],
      edges: [
        MapEdge(fromId: 'agent:june', toId: 'agent:kai'),
        MapEdge(fromId: 'agent:june', toId: 'secret:aws'),
      ],
    );
    final without = mapWithout(map, {'agent:kai'});
    expect([for (final node in without.nodes) node.id], ['agent:june']);
    expect([for (final edge in without.edges) edge.toId], ['secret:aws']);
    expect(identical(mapWithout(map, {}), map), isTrue);

    final snapshot = _workspace([_pipe()])
      ..agents.add(_agent('kai', 'Kai')..pipeId = 'pipe1')
      ..agents.add(_agent('lost', 'Lost')..pipeId = 'gone');
    expect(pipeLiaisons(snapshot).keys, ['pipe1']);
  });

  group('limited to the pipe', () {
    Workspace limits({
      List<String> members = const ['june', 'omar'],
      List<String> limited = const ['omar'],
      List<String> omarRead = const ['pipe1'],
    }) =>
        _workspace([
            _pipe(members: members, lead: 'omar', limited: limited),
          ], selected: 'pipe1')
          ..agents.clear()
          ..agents.addAll([
            _agent('june', 'June'),
            _agent(
              'omar',
              'Omar',
              role: 'Liaison',
              limited: limited.contains('omar'),
              read: omarRead,
            ),
          ]);

    testWidgets('approving, each member says whether it can run commands, '
        'with the warning while it can; a liaison starts limited', (
      tester,
    ) async {
      final snapshot = _workspace([
        _pipe(state: PipeState.PIPE_STATE_APPROVAL, digits: '4821'),
      ], selected: 'pipe1');
      snapshot.agents[1].role = 'Liaison';
      final harness = await show(tester, snapshot);
      expect(find.text(liaisonNote), findsOneWidget);
      for (final name in ['June', 'Omar']) {
        final line = find.byWidgetPredicate(
          (widget) => widget is Toggle && widget.label == name,
        );
        await tester.ensureVisible(line);
        await tester.tap(line);
        await tester.pump();
      }
      expect(find.text('Can run commands on this computer'), findsNWidgets(2));
      final warning = canRunCommandsWarning('June', snapshot.pipes.single);
      expect(
        warning,
        startsWith(
          'June can run commands on this computer. '
          "If a message from Ana's Claktory misleads it",
        ),
      );
      expect(find.text(warning), findsOneWidget);
      expect(
        find.text(canRunCommandsWarning('Omar', snapshot.pipes.single)),
        findsNothing,
        reason: 'a liaison is limited unless the person lets it',
      );

      // June limited too: no warning left.
      await tester.ensureVisible(
        find.text('Can run commands on this computer').first,
      );
      await tester.tap(find.text('Can run commands on this computer').first);
      await tester.pump();
      expect(find.text(warning), findsNothing);
      await tester.ensureVisible(find.text('Approve'));
      await tester.tap(find.text('Approve'));
      await settle(tester);
      final approve = harness.last.approvePipe;
      expect(approve.memberIds, unorderedEquals(['june', 'omar']));
      expect(approve.quarantined.agentIds, unorderedEquals(['june', 'omar']));
    });

    testWidgets('the header says how many can run commands; letting one run '
        'them asks first and says to clear what it read', (tester) async {
      final harness = await show(tester, limits());
      harness.backend.pipeChat.load(PipeTranscript(pipeId: 'pipe1'));
      await settle(tester);
      expect(find.text('1 agent can run commands'), findsOneWidget);

      await tester.tap(find.text('Acme launch').last);
      await settle(tester);
      // June can; Omar is limited.
      expect(
        find.text(
          canRunCommandsWarning(
            'June',
            harness.backend.workspace.workspace.pipes.single,
          ),
        ),
        findsOneWidget,
      );
      final omarsSwitch = find.text('Can run commands on this computer').last;
      await tester.ensureVisible(omarsSwitch);
      await tester.tap(omarsSwitch);
      await settle(tester);
      expect(
        find.text('Let Omar run commands on this computer?'),
        findsOneWidget,
      );
      expect(
        find.text(
          'It will be able to reach your files, secrets and network. It has '
          "read messages from Ana's Claktory, so clear its conversation "
          'first to start it afresh.',
        ),
        findsOneWidget,
      );
      // Cancel keeps it limited.
      await tester.tap(find.text('Cancel').last);
      await settle(tester);
      expect(find.text('Pipe settings'), findsOneWidget);
      await tester.ensureVisible(omarsSwitch);
      await tester.tap(omarsSwitch);
      await settle(tester);
      await tester.tap(find.text('Let it run commands'));
      await settle(tester);
      await tester.ensureVisible(find.text('Save'));
      await tester.tap(find.text('Save'));
      await settle(tester);
      final update = harness.last.updatePipe;
      // Released: the whole set, without it.
      expect(update.hasQuarantined(), isTrue);
      expect(update.quarantined.agentIds, isEmpty);
      expect(update.hasMembers(), isFalse);
    });

    testWidgets('limiting one needs no asking; one that never read the pipe '
        'is let out without the clearing line', (tester) async {
      final harness = await show(
        tester,
        limits(limited: const [], omarRead: const []),
      );
      harness.backend.pipeChat.load(PipeTranscript(pipeId: 'pipe1'));
      await settle(tester);
      expect(find.text('2 agents can run commands'), findsOneWidget);
      await tester.tap(find.text('Acme launch').last);
      await settle(tester);
      await tester.tap(find.text('Can run commands on this computer').first);
      await settle(tester);
      expect(find.text('Pipe settings'), findsOneWidget);
      await tester.ensureVisible(find.text('Save'));
      await tester.tap(find.text('Save'));
      await settle(tester);
      expect(harness.last.updatePipe.quarantined.agentIds, ['june']);
    });

    testWidgets('the pipe lists the agents that read it, to clear on '
        'purpose', (tester) async {
      final harness = await show(tester, limits());
      harness.backend.pipeChat.load(PipeTranscript(pipeId: 'pipe1'));
      await settle(tester);
      expect(find.text('Has read this pipe', findRichText: true), findsNothing);
      expect(
        find.textContaining('Has read this pipe', findRichText: true),
        findsOneWidget,
      );
      await tester.tap(find.text('Clear conversation…'));
      await settle(tester);
      expect(find.text("Clear Omar's conversation?"), findsOneWidget);
      await tester.tap(find.text('Clear conversation'));
      await settle(tester);
      final clear = harness.last.runCommand;
      expect((clear.agentId, clear.name), ('omar', 'clear'));
    });

    testWidgets("a limited agent's mark names its pipes", (tester) async {
      final snapshot = limits();
      final harness = Harness(snapshot);
      final omar = snapshot.agents[1];
      await harness.show(tester, home: PipeLimitMark(agent: omar));
      expect(
        find.text("Limited to the pipe with Ana's Claktory"),
        findsOneWidget,
      );
      expect(pipesLimiting(snapshot, 'june'), isEmpty);
      // In the agent's header.
      final shown = Harness(
        limits()
          ..selectedPipeId = ''
          ..selectedAgentId = 'omar',
      );
      await shown.show(tester);
      await settle(tester);
      expect(
        find.text("Limited to the pipe with Ana's Claktory"),
        findsOneWidget,
      );
      final second = _pipe(id: 'pipe2', members: ['omar'], limited: ['omar'])
        ..peer.claktoryName = "Lea's Claktory";
      expect(
        pipeLimitText([snapshot.pipes.single, second]),
        "Limited to the pipes with Ana's Claktory and Lea's Claktory",
      );
    });
  });

  group('taint label', () {
    final pipes = [_pipe()];
    PipeTaint read(List<String> via, {String pipe = 'pipe1'}) =>
        PipeTaint(pipeId: pipe, pipeName: 'Acme launch', viaNames: via);

    test('who read which pipe, and through whom', () {
      expect(taintLines([read(const [])], agent: 'June', pipes: pipes), [
        "from June, which reads the pipe with Ana's Claktory",
      ]);
      expect(
        taintLines(
          [
            read(const ['Liaison']),
          ],
          agent: 'June',
          pipes: pipes,
        ),
        ["from Liaison, which reads the pipe with Ana's Claktory"],
      );
      expect(
        taintLines(
          [
            read(const ['Liaison', 'Bruno', 'Lea']),
          ],
          agent: 'June',
          pipes: pipes,
        ),
        [
          "from Lea, which heard from the pipe with Ana's Claktory via "
              'Liaison → Bruno',
        ],
      );
      // A pipe that's gone, by its name.
      expect(
        taintLines(
          [read(const [], pipe: 'gone')],
          agent: 'June',
          pipes: pipes,
        ),
        ['from June, which reads the pipe “Acme launch”'],
      );
    });

    test('a site, direct messages, and a kind still to come', () {
      expect(
        taintLines(
          [
            PipeTaint(kind: 'browser', pipeName: 'https://docs.example.com'),
            PipeTaint(
              kind: 'dm',
              pipeName: 'Direct messages with Ana · K7QF2M',
              viaNames: ['Ada', 'Lea'],
            ),
            PipeTaint(kind: 'later', pipeName: 'something new'),
          ],
          agent: 'Ada',
          pipes: pipes,
        ),
        [
          'from Ada, which read a page on https://docs.example.com',
          'from Lea, which heard from direct messages with Ana · K7QF2M '
              'via Ada',
          'from Ada, which read something new',
        ],
      );
    });

    testWidgets('a line per pipe; nothing when none', (tester) async {
      final harness = Harness(_workspace([_pipe()]));
      await harness.show(
        tester,
        home: Column(
          children: [
            TaintLabel(
              taint: [
                read(const ['Liaison']),
                read(const [], pipe: 'gone'),
              ],
              agent: 'June',
            ),
            const TaintLabel(taint: [], agent: 'June'),
          ],
        ),
      );
      expect(
        find.text("from Liaison, which reads the pipe with Ana's Claktory"),
        findsOneWidget,
      );
      expect(
        find.text('from June, which reads the pipe “Acme launch”'),
        findsOneWidget,
      );
      expect(find.byType(PortalGlyph), findsNWidgets(2));
    });
  });

  testWidgets("a pipe with no liaison opens at a message it's asked to, "
      'loading back to it', (tester) async {
    final harness = await show(
      tester,
      _workspace([_pipe()], selected: 'pipe1'),
    );
    List<PipeMessage> page(int from, int to) => [
      for (var seq = from; seq <= to; seq++)
        _message(
          seq,
          'Message $seq',
          PipeAuthorKind.PIPE_AUTHOR_KIND_THEIR_AGENT,
          name: 'Bo',
        ),
    ];
    harness.backend.pipeChat.load(
      PipeTranscript(pipeId: 'pipe1', messages: page(41, 80), hasEarlier: true),
    );
    await settle(tester);
    bool inView(String text) {
      final found = find.text(text);
      if (found.evaluate().isEmpty) return false;
      final y = tester.getCenter(found).dy;
      return y > 0 && y < 900;
    }

    expect(inView('Message 80'), isTrue);

    // Asked for one in a page not loaded yet: it goes up, loading it.
    harness.backend.reveal.reachExtra(
      pipeChatRevealId('pipe1'),
      pipeItemKey(7),
    );
    await pumpFrames(tester);
    expect(harness.backend.sent.any((m) => m.hasLoadPipeMessages()), isTrue);
    harness.backend.pipeChat.prepend(
      PipeMessagePage(pipeId: 'pipe1', messages: page(1, 40)),
    );
    await pumpFrames(tester);
    expect(inView('Message 7'), isTrue);
    expect(inView('Message 80'), isFalse);
    expect(harness.backend.reveal.extra, isNull, reason: 'used once');
  });

  group('a pipe with agents always has a lead', () {
    test('the lead Claktory gives it: its liaison, else the first member '
        'limited to the pipe; never one that can run commands', () {
      expect(
        pipeLeadFor(
          ['june', 'omar', 'kai'],
          limited: {'omar', 'kai'},
          liaisonId: 'kai',
        ),
        'kai',
      );
      expect(pipeLeadFor(['june', 'kai'], liaisonId: 'kai'), '');
      expect(pipeLeadFor(['june', 'omar'], limited: {'omar'}), 'omar');
      expect(pipeLeadFor(['june', 'omar']), '');
      expect(pipeLeadFor(const []), '');
    });

    Future<Harness> settings(WidgetTester tester, Pipe pipe) async {
      final harness = await show(
        tester,
        _workspace([pipe], selected: 'pipe1'),
        size: const Size(1200, 1500),
      );
      harness.backend.pipeChat.load(PipeTranscript(pipeId: 'pipe1'));
      await settle(tester);
      await tester.tap(find.text('Acme launch').last);
      await settle(tester);
      expect(find.text('Pipe settings'), findsOneWidget);
      return harness;
    }

    Finder lead(String name) => find.descendant(
      of: find.ancestor(of: find.text('Lead'), matching: find.byType(Field)),
      matching: find.text(name),
    );

    testWidgets('unticking the lead shows who leads instead, never No lead; '
        'ticking it back brings it back', (tester) async {
      final harness = await settings(
        tester,
        _pipe(members: ['june', 'omar'], lead: 'omar', limited: ['june']),
      );
      expect(lead('Omar'), findsOneWidget);
      // Omar's line under Who may use it; the sidebar's row comes first.
      final omar = find.text('Omar').at(1);
      await tester.tap(omar);
      await settle(tester);
      expect(lead('June'), findsOneWidget);
      expect(find.text('No lead'), findsNothing);
      await tester.tap(omar);
      await settle(tester);
      expect(lead('Omar'), findsOneWidget);

      await tester.ensureVisible(find.text('Save'));
      await tester.tap(find.text('Save'));
      await settle(tester);
      final update = harness.last.updatePipe;
      expect(update.hasLeadId(), isFalse, reason: 'nothing changed');
      expect(update.hasMembers(), isFalse);
    });

    testWidgets('saved without its lead, the pipe gets the one it shows', (
      tester,
    ) async {
      final harness = await settings(
        tester,
        _pipe(members: ['june', 'omar'], lead: 'omar', limited: ['june']),
      );
      await tester.tap(find.text('Omar').at(1));
      await settle(tester);
      await tester.ensureVisible(find.text('Save'));
      await tester.tap(find.text('Save'));
      await settle(tester);
      final update = harness.last.updatePipe;
      expect(update.members.agentIds, ['june']);
      expect(update.leadId, 'june');
    });

    testWidgets('with no agent limited to the pipe left, it has no lead '
        'until the person chooses one; open, it says no one answers it', (
      tester,
    ) async {
      final harness = await settings(
        tester,
        _pipe(members: ['june', 'omar'], lead: 'omar'),
      );
      await tester.tap(find.text('Omar').at(1));
      await settle(tester);
      expect(lead('No lead'), findsOneWidget);
      expect(find.text(pipeNoLeadNote), findsOneWidget);
      await tester.ensureVisible(find.text('Save'));
      await tester.tap(find.text('Save'));
      await settle(tester);
      expect(harness.last.updatePipe.leadId, '');
    });

    testWidgets('a pipe with no agents has No lead, and says no one '
        'answers it', (tester) async {
      await settings(tester, _pipe());
      expect(lead('No lead'), findsOneWidget);
      expect(find.text(pipeNoLeadNote), findsNothing);
      expect(find.text(pipeUnanswered), findsWidgets);
    });

    testWidgets('the person may choose one that can run commands', (
      tester,
    ) async {
      final harness = await settings(
        tester,
        _pipe(members: ['june', 'omar'], lead: ''),
      );
      expect(lead('No lead'), findsOneWidget);
      await tester.tap(lead('No lead'));
      await settle(tester);
      await tester.tap(find.text('June').last);
      await settle(tester);
      expect(lead('June'), findsOneWidget);
      expect(find.text('No lead'), findsNothing);
      await tester.ensureVisible(find.text('Save'));
      await tester.tap(find.text('Save'));
      await settle(tester);
      expect(harness.last.updatePipe.leadId, 'june');
    });

    testWidgets('approving, the lead unticked goes back to a new liaison '
        'and comes back when ticked', (tester) async {
      final harness = await show(
        tester,
        _workspace([
          _pipe(state: PipeState.PIPE_STATE_APPROVAL, digits: '4821'),
        ], selected: 'pipe1'),
      );
      final omar = find.text('Omar').last;
      await tester.tap(omar);
      await tester.pump();
      await tester.tap(lead('A new liaison agent'));
      await settle(tester);
      await tester.tap(find.text('Omar').last);
      await settle(tester);
      expect(lead('Omar'), findsOneWidget);
      // Omar's toggle, under Who may use it.
      await tester.tap(find.text('Omar').at(1));
      await settle(tester);
      expect(lead('A new liaison agent'), findsOneWidget);
      await tester.tap(find.text('Omar').at(1));
      await settle(tester);
      expect(lead('Omar'), findsOneWidget);
      await tester.ensureVisible(find.text('Approve'));
      await tester.tap(find.text('Approve'));
      await settle(tester);
      expect(harness.last.approvePipe.leadId, 'omar');
    });
  });
}
