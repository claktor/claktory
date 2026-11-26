import 'package:factory_app/devices.dart' show QrCode;
import 'package:factory_app/controls.dart' show Button, Field, Select, Toggle;
import 'package:factory_app/decision.dart';
import 'package:factory_app/inbox.dart'
    show InboxView, NeedsYouKind, needsYouItems;
import 'package:factory_app/map.dart';
import 'package:factory_app/map_layout.dart';
import 'package:factory_app/pipe_timeline.dart' show pipeItemKey;
import 'package:factory_app/pipe_start.dart' show inviteLifetime;
import 'package:factory_app/pipes.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart' show CameraAccess;
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart' show Sidebar;
import 'package:factory_app/taint_label.dart';
import 'package:factory_app/update.dart';
import 'package:fixnum/fixnum.dart';
import 'package:factory_app/usage.dart' show formatUntil;
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart' show LogicalKeyboardKey;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_test.dart' show menuButton, phone;
import 'secret_clipboard_fake.dart';
import 'selection_test.dart' show watchClipboard;

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

  testWidgets("a liaison held up by its usage: the pipe says so in place of "
      "'Online', in the sidebar and the pipe's header (#343 U18)", (
    tester,
  ) async {
    final until = DateTime.now().add(const Duration(hours: 3));
    final snapshot = _workspace([
      _pipe(members: ['june'], lead: 'june'),
    ], selected: 'pipe1');
    snapshot.agents.first
      ..usageWait = 'Waiting for your Claude usage to reset at 18:00'
      ..usageWaitUntilMs = Int64(until.millisecondsSinceEpoch);
    await show(tester, snapshot);
    final line =
        'June is waiting until ${formatUntil(until.millisecondsSinceEpoch, DateTime.now())}';
    expect(find.text(line, findRichText: true), findsWidgets);
    expect(find.text('Online', findRichText: true), findsNothing);
    // Waiting for the person (out of credit): no time.
    final june = snapshot.agents.first;
    expect(
      pipeStatus(
        _pipe(members: ['june'], lead: 'june'),
        lead: june.deepCopy()..usageWaitUntilMs = Int64.ZERO,
      ),
      'June is waiting',
    );
    // Offline or not waiting: as before.
    expect(
      pipeStatus(_pipe(online: false, lead: 'june'), lead: june),
      'Offline',
    );
    expect(
      pipeStatus(_pipe(lead: 'june'), lead: june.deepCopy()..usageWait = ''),
      'Online',
    );
  });

  testWidgets("the other side's liaison waits: the pipe says so by this "
      "side's own name for it, never their words (#343 U18)", (tester) async {
    final until = DateTime.now().add(const Duration(hours: 2));
    final snapshot = _workspace([
      _pipe(members: ['june'], lead: 'june')
        ..peerWaiting = true
        ..peerWaitingUntilMs = Int64(until.millisecondsSinceEpoch),
    ], selected: 'pipe1');
    await show(tester, snapshot);
    final line =
        'Acme launch is waiting until '
        '${formatUntil(until.millisecondsSinceEpoch, DateTime.now())}';
    expect(find.text(line, findRichText: true), findsWidgets);
    expect(find.text('Online', findRichText: true), findsNothing);
    expect(find.textContaining('Ana is waiting'), findsNothing);
    // No known end, or one that passed: no time.
    final june = snapshot.agents.first;
    final noEnd = _pipe(lead: 'june')..peerWaiting = true;
    expect(pipeStatus(noEnd, lead: june), 'Acme launch is waiting');
    expect(
      pipeStatus(
        noEnd.deepCopy()..peerWaitingUntilMs = Int64(1000),
        lead: june,
      ),
      'Acme launch is waiting',
    );
    // Our own lead's wait comes first; offline, as before.
    expect(
      pipeStatus(
        noEnd,
        lead: june.deepCopy()
          ..usageWait = 'Waiting'
          ..usageWaitUntilMs = Int64.ZERO,
      ),
      'June is waiting',
    );
    expect(pipeStatus(noEnd.deepCopy()..online = false, lead: june), 'Offline');
  });

  testWidgets('its lead past its spending limit: the row says so, and a '
      'note under the header raises the limit (#357 L1)', (tester) async {
    final snapshot = _workspace([
      _pipe(members: ['june'], lead: 'june'),
    ], selected: 'pipe1');
    snapshot.agents.first.spend = ContainedSpend(
      spentUsd: 10.2,
      capUsd: 10,
      capTokens: Int64(500000),
      over: true,
    );
    final harness = await show(tester, snapshot);
    expect(find.text(pipeOverLimitLine, findRichText: true), findsWidgets);
    expect(find.text('Online', findRichText: true), findsNothing);
    expect(find.text('June reached today’s \$10 limit'), findsOneWidget);
    final raise = find.widgetWithText(Button, 'Raise limit');
    expect(tester.widget<Button>(raise).onPressed, isNull);
    final field = find.descendant(
      of: find.byType(PipeSpendNotice),
      matching: find.byType(EditableText),
    );
    // Not a raise: nothing to send.
    await tester.enterText(field, '8');
    await pumpFrames(tester);
    expect(tester.widget<Button>(raise).onPressed, isNull);
    await tester.enterText(field, '15');
    await pumpFrames(tester);
    await tester.tap(raise);
    await pumpFrames(tester);
    final cap = harness.last.setSpendCap;
    expect(
      (cap.agentId, cap.capUsd, cap.capTokens),
      ('june', 15.0, Int64(500000)),
    );
    // Under its limit again: as before.
    final under = harness.backend.workspace.workspace.deepCopy();
    under.agents.first.spend.over = false;
    harness.backend.workspace.apply(under);
    await pumpFrames(tester);
    expect(find.byType(PipeSpendNotice), findsOneWidget);
    expect(find.textContaining('reached today’s'), findsNothing);
    expect(find.text('Online', findRichText: true), findsWidgets);
  });

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

  testWidgets("on a Mac, Needs you opens from a pipe's view in its place, "
      "not out of sight beside the agent last chosen; again goes back to the "
      "pipe (Abbacchio's row 638)", (tester) async {
    final harness = await show(
      tester,
      _workspace([
        _pipe(waiting: [4]),
      ], selected: 'pipe1'),
    );
    expect(find.byType(PipeView), findsOneWidget);
    final row = find.descendant(
      of: find.byType(Sidebar),
      matching: find.text('Needs you'),
    );
    await tester.tap(row);
    await settle(tester);
    expect(harness.page.value, AppPage.inbox);
    expect(find.byType(InboxView), findsOneWidget);
    expect(find.byType(PipeView), findsNothing);
    expect(
      harness.backend.sent.where((m) => m.hasSetLayout()),
      isEmpty,
      reason: "nothing put in the agent's layout",
    );

    await tester.tap(row);
    await settle(tester);
    expect(harness.page.value, AppPage.agent);
    expect(find.byType(PipeView), findsOneWidget);
  });

  testWidgets("an open pipe with no lead and nothing yet says no agent hears "
      "what comes in (Abbacchio's row 638)", (tester) async {
    await show(tester, _workspace([_pipe()], selected: 'pipe1'));
    expect(
      find.text(
        'Nothing here yet. No agent hears what comes in until you choose a '
        'lead in the pipe’s settings. You can write here too.',
      ),
      findsOneWidget,
    );
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

  testWidgets("an agent's held message with a comment typed: one Submit, "
      'which sends it back unsent to its writer (#370)', (tester) async {
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
            'The launch is on the 3rd.',
            PipeAuthorKind.PIPE_AUTHOR_KIND_MY_AGENT,
            agentId: 'june',
            state: PipeMessageState.PIPE_MESSAGE_STATE_HELD,
          ),
        ],
      ),
    );
    await settle(tester);
    await tester.tap(find.text('Add a comment'));
    await settle(tester);
    final field = find.descendant(
      of: find.byType(DecisionComment),
      matching: find.byType(EditableText),
    );
    await tester.enterText(field, 'Fix the date first.');
    await tester.pump();
    expect(find.text('Discard'), findsNothing);
    expect(find.text('Send'), findsOneWidget, reason: "the composer's only");
    expect(
      find.text('Not sent. June gets your comment and writes it again.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Submit'));
    await settle(tester);
    final decided = harness.last.decidePipeMessage;
    expect(decided.seq.toInt(), 1);
    expect(decided.reply, isTrue);
    expect(decided.allow, isFalse);
    expect(decided.comment, 'Fix the date first.');

    // Back as Discarded, replied, with the comment as its note.
    harness.backend.pipeChat.update(
      PipeMessageUpdate(
        pipeId: 'pipe1',
        message:
            _message(
                1,
                'The launch is on the 3rd.',
                PipeAuthorKind.PIPE_AUTHOR_KIND_MY_AGENT,
                agentId: 'june',
                state: PipeMessageState.PIPE_MESSAGE_STATE_DISCARDED,
              )
              ..replied = true
              ..note = 'Fix the date first.',
      ),
    );
    await settle(tester);
    expect(
      find.textContaining('Sent back to June · “Fix the date first.”'),
      findsOneWidget,
    );
    expect(find.textContaining('Discarded'), findsNothing);
  });

  testWidgets("a held message its writer took back says why, in place of "
      "Discarded (FB19, Abbacchio)", (tester) async {
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
            'The launch is on the 3rd.',
            PipeAuthorKind.PIPE_AUTHOR_KIND_MY_AGENT,
            agentId: 'june',
            state: PipeMessageState.PIPE_MESSAGE_STATE_DISCARDED,
          )..note = 'June took it back: the date moved \u202Eto the 5th.',
        ],
      ),
    );
    await settle(tester);
    expect(
      find.textContaining(
        r'June took it back: the date moved \u{202E}to the 5th.',
      ),
      findsOneWidget,
    );
    expect(find.textContaining('Discarded'), findsNothing);
  });

  testWidgets("one held on its way in keeps Let in with a comment, and Return "
      "doesn't decide it", (tester) async {
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
    await tester.enterText(
      find.descendant(
        of: find.byType(DecisionComment),
        matching: find.byType(EditableText),
      ),
      'Only the public deck.',
    );
    await tester.pump();
    expect(find.text('Submit'), findsNothing);
    expect(find.text('Let in'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await settle(tester);
    expect(
      harness.backend.sent.where((m) => m.hasDecidePipeMessage()),
      isEmpty,
    );
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

    testWidgets("an invite not used yet: the settings' sentences start "
        "capitalized, \"The other side\" (Zucchero's row 175)", (tester) async {
      final pipe = _pipe(state: PipeState.PIPE_STATE_INVITED)
        ..peer = PipePeer();
      await show(tester, _workspace([pipe], selected: 'pipe1'));
      await tester.tap(find.text('Acme launch').last);
      await settle(tester);
      expect(find.text('Pipe settings'), findsOneWidget);
      expect(find.text('The other side sees this.'), findsOneWidget);
      expect(
        find.textContaining('own safety rules. The other side never sees it.'),
        findsOneWidget,
      );
      expect(find.textContaining('. the other side'), findsNothing);
      expect(pipeStatus(pipe), 'Waiting for your invite to be used');
      expect(
        pipeOfflineText(pipe..online = false),
        startsWith('The other side’s Claktory is offline.'),
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

    testWidgets('another account is in Needs you by itself, whoever asked, '
        'with the warning and Trust; the pipe counts it (#471 G7)', (
      tester,
    ) async {
      final pipe = _pipe()
        ..peer.verifiedEmail = 'bo@example.com'
        ..peer.identityChange =
            PipeIdentityChange.PIPE_IDENTITY_CHANGE_ACCOUNT_CHANGED
        ..peer.previousEmail = 'ana@acme.example';
      // From the pipe's view, Needs you opens in its place.
      final harness = await show(tester, signedIn([pipe]));
      expect(pipeWaiting(pipe), 1);
      await tester.tap(
        find.descendant(
          of: find.byType(Sidebar),
          matching: find.text('Needs you'),
        ),
      );
      await settle(tester);
      final item = find.text("Ana's Claktory is on another account now");
      expect(item, findsOneWidget);
      // Opened, its card, once it has unfolded.
      await tester.tap(item);
      await settle(tester);
      await tester.pump(const Duration(milliseconds: 400));
      await tester.tap(find.text('Trust this Claktory…'));
      await settle(tester);
      await tester.tap(find.text('Trust'));
      await settle(tester);
      expect(harness.last.acceptPipeIdentity.pipeId, 'pipe1');
    });

    test('only another account waits in Needs you: a lapse or a new '
        'email stays in the header, and a closed pipe waits for nothing', () {
      Pipe changed(PipeIdentityChange change, {bool closed = false}) =>
          _pipe(
              state: closed
                  ? PipeState.PIPE_STATE_CLOSED
                  : PipeState.PIPE_STATE_OPEN,
            )
            ..peer.identityChange = change
            ..peer.previousEmail = 'ana@acme.example';
      List<NeedsYouKind> kinds(Pipe pipe) => [
        for (final item in needsYouItems(_workspace([pipe]))) item.kind,
      ];
      expect(
        kinds(changed(PipeIdentityChange.PIPE_IDENTITY_CHANGE_ACCOUNT_CHANGED)),
        [NeedsYouKind.pipeIdentity],
      );
      expect(
        kinds(changed(PipeIdentityChange.PIPE_IDENTITY_CHANGE_LAPSED)),
        [],
      );
      expect(
        kinds(changed(PipeIdentityChange.PIPE_IDENTITY_CHANGE_EMAIL_CHANGED)),
        [],
      );
      expect(
        kinds(
          changed(
            PipeIdentityChange.PIPE_IDENTITY_CHANGE_ACCOUNT_CHANGED,
            closed: true,
          ),
        ),
        [],
      );
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
        // A new pipe: both reviews off, as the card reads them (#470).
        _pipe(
          state: PipeState.PIPE_STATE_APPROVAL,
          digits: '4821',
          review: false,
        )..suggestedLiaisonName = 'Kai',
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

  testWidgets('with no agent to reply to, Decline takes a comment for this '
      'side only', (tester) async {
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

  testWidgets('with an agent to hear it, a comment on the approval is a '
      'reply: Submit, and the pipe still waits (#370)', (tester) async {
    final harness = await show(
      tester,
      _workspace([
        _pipe(state: PipeState.PIPE_STATE_APPROVAL, digits: '4821')
          ..replyAgentId = 'june',
      ], selected: 'pipe1'),
    );
    final add = find.text('Add a comment');
    await tester.ensureVisible(add);
    await tester.tap(add);
    await settle(tester);
    final field = find.descendant(
      of: find.byType(DecisionComment),
      matching: find.byType(EditableText),
    );
    await tester.enterText(field, 'Which client is this for?');
    await tester.pump();
    expect(find.text('Approve'), findsNothing);
    expect(find.text('Decline'), findsNothing);
    expect(
      find.text('The pipe still waits for you. June gets your comment.'),
      findsOneWidget,
    );
    await tester.ensureVisible(find.text('Submit'));
    await tester.tap(find.text('Submit'));
    await settle(tester);
    final approve = harness.last.approvePipe;
    expect(approve.pipeId, 'pipe1');
    expect(approve.reply, isTrue);
    expect(approve.approve, isFalse);
    expect(approve.comment, 'Which client is this for?');
    expect(approve.memberIds, isEmpty);
    // Still to decide; said where it went.
    expect(tester.widget<EditableText>(field).controller.text, isEmpty);
    expect(find.text('Sent to June.'), findsOneWidget);
    expect(find.text('Approve'), findsOneWidget);
  });

  testWidgets('Review incoming, turned on at approval, goes with it', (
    tester,
  ) async {
    final harness = await show(
      tester,
      _workspace([
        _pipe(
          state: PipeState.PIPE_STATE_APPROVAL,
          digits: '4821',
          review: false,
        ),
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

  testWidgets('settings made before approval survive it: the card opens '
      "from the pipe's members, lead and reviews, and follows them until "
      'the person changes it (#470)', (tester) async {
    // Waiting, nothing set yet: the card shows a new pipe's defaults.
    final harness = await show(
      tester,
      _workspace([
        _pipe(
          state: PipeState.PIPE_STATE_APPROVAL,
          digits: '4821',
          review: false,
        ),
      ], selected: 'pipe1'),
    );
    // Rivet's settings card approved meanwhile: June and Omar, Omar
    // leading, June limited to the pipe, Review incoming on.
    harness.backend.workspace.apply(
      harness.backend.workspace.workspace.deepCopy()
        ..pipes.first.memberIds.addAll(['june', 'omar'])
        ..pipes.first.leadId = 'omar'
        ..pipes.first.quarantinedIds.add('june')
        ..pipes.first.reviewIncoming = true,
    );
    await settle(tester);
    await tester.ensureVisible(find.text('Approve'));
    await tester.tap(find.text('Approve'));
    await settle(tester);
    final approve = harness.last.approvePipe;
    expect(approve.memberIds, unorderedEquals(['june', 'omar']));
    expect(approve.leadId, 'omar');
    expect(approve.liaisonName, isEmpty);
    expect(approve.quarantined.agentIds, ['june']);
    expect(approve.reviewIncoming, isTrue);
    expect(approve.reviewOutgoing, isFalse);
  });

  testWidgets('a change to the settings after the person changed the card '
      'is read again and said, never approved over (Fugo, #470)', (
    tester,
  ) async {
    final harness = await show(
      tester,
      _workspace([
        _pipe(
          state: PipeState.PIPE_STATE_APPROVAL,
          digits: '4821',
          members: ['june', 'omar'],
          lead: 'omar',
          review: false,
        ),
      ], selected: 'pipe1'),
    );
    final incoming = find.text('Review messages before your agents read them');
    await tester.ensureVisible(incoming);
    await tester.tap(incoming);
    await tester.pump();
    // Rivet limits June to the pipe meanwhile.
    harness.backend.workspace.apply(
      harness.backend.workspace.workspace.deepCopy()
        ..pipes.first.quarantinedIds.add('june'),
    );
    await settle(tester);
    expect(
      find.text(
        "The pipe's settings changed since you started. Check them again.",
      ),
      findsOneWidget,
    );
    await tester.ensureVisible(find.text('Approve'));
    await tester.tap(find.text('Approve'));
    await settle(tester);
    final approve = harness.last.approvePipe;
    expect(approve.quarantined.agentIds, ['june']);
    expect(approve.memberIds, unorderedEquals(['june', 'omar']));
    expect(approve.reviewIncoming, isFalse);
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
    expect(find.textContaining('can scan it in Claktory'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('on a phone, a second tap where Make invite was copies the '
      "code rather than closing it unseen; Done opens the pipe (Zucchero's "
      'row 157)', (tester) async {
    final harness = await show(tester, _workspace([]), size: phone);
    harness.backend.pipeInvite = PipeInvite(
      pipeId: 'swap',
      code: 'claktory-pipe://?w=x&s=y',
    );
    final clipboard = FakeSecretClipboard.install();
    await tester.tap(menuButton());
    await pumpFrames(tester);
    await tester.longPress(find.text('Factory').first);
    await pumpFrames(tester);
    await tester.tap(find.text('Project settings'));
    await pumpFrames(tester);
    await tester.ensureVisible(find.text('Connect a pipe…'));
    await tester.tap(find.text('Connect a pipe…'));
    await pumpFrames(tester);
    await tester.tap(find.text('Make an invite'));
    await settle(tester);
    final where = tester.getCenter(find.text('Make invite'));
    await tester.tapAt(where);
    harness.backend.workspace.apply(
      harness.backend.workspace.workspace.deepCopy()
        ..pipes.add(
          Pipe(
            id: 'swap',
            projectId: 'p1',
            name: 'Factory ⇄',
            state: PipeState.PIPE_STATE_INVITED,
          ),
        ),
    );
    await settle(tester);
    expect(find.text('Send this code'), findsOneWidget);
    expect(find.text('claktory-pipe://?w=x&s=y'), findsOneWidget);

    // Tapped again where Make invite was: the code is copied, and stays.
    expect(tester.getCenter(find.text('Copy code')).dx, closeTo(where.dx, 40));
    await tester.tapAt(tester.getCenter(find.text('Copy code')));
    await settle(tester);
    expect(clipboard.copies.single.$1, 'claktory-pipe://?w=x&s=y');
    expect(find.text('Copied'), findsOneWidget);
    expect(find.text('claktory-pipe://?w=x&s=y'), findsOneWidget);
    expect(harness.backend.sent.where((m) => m.hasSelectPipe()), isEmpty);

    await tester.tap(find.text('Done'));
    await pumpFrames(tester);
    expect(find.text('Send this code'), findsNothing);
    expect(
      harness.backend.sent
          .lastWhere((m) => m.hasSelectPipe())
          .selectPipe
          .pipeId,
      'swap',
    );
    // Cleared once dead.
    await tester.pump(inviteLifetime);
    expect(clipboard.cleared, [1]);
  });

  testWidgets("an invite's code is copied marked sensitive, for as long as "
      "it works, and cleared once it's dead (Fugo's row 166)", (tester) async {
    final clipboard = FakeSecretClipboard.install();
    final expires = DateTime.now().add(const Duration(minutes: 10));
    final harness = await show(tester, _workspace([]));
    harness.backend.pipeInvite = PipeInvite(
      pipeId: 'swap',
      code: 'claktory-pipe://?w=x&s=y',
    );
    final copied = watchClipboard(tester);
    await tester.tap(find.text('Factory'), buttons: kSecondaryButton);
    await settle(tester);
    await tester.ensureVisible(find.text('Connect a pipe…'));
    await tester.tap(find.text('Connect a pipe…'));
    await settle(tester);
    await tester.tap(find.text('Make an invite'));
    await settle(tester);
    await tester.tap(find.text('Make invite'));
    harness.backend.workspace.apply(
      harness.backend.workspace.workspace.deepCopy()
        ..pipes.add(
          Pipe(
            id: 'swap',
            projectId: 'p1',
            name: 'Factory ⇄',
            state: PipeState.PIPE_STATE_INVITED,
            inviteExpiresMs: Int64(expires.millisecondsSinceEpoch),
          ),
        ),
    );
    await settle(tester);
    await tester.tap(find.text('Copy code'));
    await settle(tester);
    expect(clipboard.copies.single.$1, 'claktory-pipe://?w=x&s=y');
    expect(clipboard.copies.single.$2, inInclusiveRange(590, 600));
    expect(copied, isEmpty, reason: 'never on the plain clipboard');
    expect(find.text('Copied'), findsOneWidget);
    // Kept while it works, even with the form closed; cleared once dead.
    await tester.tap(find.text('Done'));
    await settle(tester);
    await tester.pump(const Duration(minutes: 9));
    expect(clipboard.cleared, isEmpty);
    await tester.pump(const Duration(minutes: 1, seconds: 1));
    expect(clipboard.cleared, [1]);
  });

  testWidgets('I have a code scans it where there is a camera, then shows '
      'its preview, to Connect (friend bug #2)', (tester) async {
    final harness = Harness(_workspace([]));
    harness.camera.state = CameraAccess.denied;
    harness.backend.pipeInvitePreview = PipeInvitePreview(
      code: 'claktory-pipe://?w=a',
      name: 'Acme launch',
      peer: PipePeer(ownerName: 'Ana', projectName: 'Acme Site'),
    );
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);
    await tester.tap(find.text('Factory'), buttons: kSecondaryButton);
    await settle(tester);
    await tester.ensureVisible(find.text('Connect a pipe…'));
    await tester.tap(find.text('Connect a pipe…'));
    await settle(tester);
    await tester.tap(find.text('I have a code'));
    await settle(tester);
    // A camera refused so far still offers Scan: the scanner says how to
    // allow it.
    expect(
      find.text('Paste the code you were sent, or scan it.'),
      findsOneWidget,
    );
    harness.camera.state = CameraAccess.granted;
    await tester.tap(find.text('Scan code'));
    await pumpFrames(tester);
    harness.camera.scan('claktory-pipe://?w=a');
    await pumpFrames(tester);
    await settle(tester);
    expect(harness.last.readPipeInvite.code, 'claktory-pipe://?w=a');
    expect(find.text('Connect Factory with Acme Site?'), findsOneWidget);
    expect(harness.backend.sent.where((m) => m.hasAcceptPipeInvite()), isEmpty);
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

  group('only sends (#414)', () {
    Workspace senders({List<String> sendOnly = const []}) =>
        _workspace([
            _pipe(members: ['june', 'omar', ...sendOnly], lead: 'omar')
              ..sendOnlyIds.addAll(sendOnly),
          ], selected: 'pipe1')
          ..agents.add(_agent('rivet', 'Rivet', project: ''))
          ..agents.add(_agent('elsewhere', 'Elsewhere', project: 'p2'));

    Future<Harness> settings(WidgetTester tester, Workspace snapshot) async {
      final harness = await show(tester, snapshot);
      harness.backend.pipeChat.load(PipeTranscript(pipeId: 'pipe1'));
      await settle(tester);
      await tester.tap(find.text('Acme launch').last);
      await settle(tester);
      return harness;
    }

    Finder choice(String name) => find.byWidgetPredicate(
      (widget) => widget is Select<PipeAccess> && widget.title == name,
    );

    testWidgets("each member's choice: reads and runs commands, limited, or "
        'only sends; one that only sends never leads', (tester) async {
      final harness = await settings(tester, senders());
      expect(
        tester
            .widget<Select<PipeAccess>>(choice('June'))
            .options
            .map((option) => option.label),
        ['Reads and runs commands', 'Limited to the pipe', 'Only sends'],
      );
      expect(find.text('Reads and runs commands'), findsNWidgets(2));
      tester
          .widget<Select<PipeAccess>>(choice('June'))
          .onChanged(PipeAccess.sendOnly);
      await settle(tester);
      expect(
        find.text(
          sendOnlyNote(
            'June',
            harness.backend.workspace.workspace.pipes.single,
          ),
        ),
        findsOneWidget,
      );
      final lead = tester.widget<Select<String>>(
        find.descendant(
          of: find.ancestor(
            of: find.text('Lead'),
            matching: find.byType(Field),
          ),
          matching: find.byType(Select<String>),
        ),
      );
      expect([for (final option in lead.options) option.value], ['omar']);
      await tester.ensureVisible(find.text('Save'));
      await tester.tap(find.text('Save'));
      await settle(tester);
      final update = harness.last.updatePipe;
      expect(update.sendOnly.agentIds, ['june']);
      expect(update.hasMembers(), isFalse);
      expect(update.quarantined.agentIds, isNot(contains('june')));
    });

    testWidgets("on a phone, the settings raise no keyboard, and a choice "
        "gives none back to the name (Sale's row 77)", (tester) async {
      final harness = await show(tester, senders(), size: phone);
      harness.backend.pipeChat.load(PipeTranscript(pipeId: 'pipe1'));
      await settle(tester);
      await tester.tap(find.text('Acme launch').last);
      await settle(tester);
      EditableText name() => tester.widget<EditableText>(
        find.descendant(
          of: find.ancestor(
            of: find.text('Name'),
            matching: find.byType(Field),
          ),
          matching: find.byType(EditableText),
        ),
      );
      expect(name().focusNode.hasFocus, isFalse);
      // Typed in, its keyboard then hidden: the focus stays.
      name().focusNode.requestFocus();
      await settle(tester);
      await tester.ensureVisible(choice('June'));
      await tester.tap(choice('June'));
      await pumpFrames(tester);
      await tester.tap(find.text('Only sends').last);
      await pumpFrames(tester);
      expect(name().focusNode.hasFocus, isFalse);
      expect(
        find.text(
          sendOnlyNote(
            'June',
            harness.backend.workspace.workspace.pipes.single,
          ),
        ),
        findsOneWidget,
      );
    });

    testWidgets("Cancel on the ask to run commands comes back to the settings "
        "where they were (Sale's row 79)", (tester) async {
      // Short, so the settings scroll; a pointer's layout, where the ask's
      // buttons fit the test's font.
      await show(tester, senders(), size: const Size(1000, 520));
      await tester.tap(find.text('Acme launch').last);
      await settle(tester);
      tester
          .widget<Select<PipeAccess>>(choice('June'))
          .onChanged(PipeAccess.limited);
      await settle(tester);
      await tester.ensureVisible(choice('June'));
      await settle(tester);
      ScrollPosition form() => tester
          .state<ScrollableState>(
            find
                .descendant(
                  of: find.byType(PipeSettingsForm),
                  matching: find.byType(Scrollable),
                )
                .first,
          )
          .position;
      final at = form().pixels;
      expect(at, greaterThan(0));
      tester
          .widget<Select<PipeAccess>>(choice('June'))
          .onChanged(PipeAccess.commands);
      await settle(tester);
      expect(
        find.text('Let June run commands on this computer?'),
        findsOneWidget,
      );
      await tester.tap(find.text('Cancel').last);
      await settle(tester);
      expect(find.text('Pipe settings'), findsOneWidget);
      expect(form().pixels, at);
      expect(
        tester.widget<Select<PipeAccess>>(choice('June')).value,
        PipeAccess.limited,
      );
    });

    testWidgets("Rivet and the workspace's agents can be let in, only to "
        "send; another project's agents aren't offered", (tester) async {
      final harness = await settings(tester, senders());
      expect(find.text('Elsewhere'), findsNothing);
      final rivet = find.byWidgetPredicate(
        (widget) => widget is Toggle && widget.label == 'Rivet',
      );
      await tester.ensureVisible(rivet);
      await tester.tap(rivet);
      await settle(tester);
      expect(choice('Rivet'), findsNothing, reason: 'nothing else to choose');
      expect(find.text('Only sends'), findsOneWidget);
      await tester.ensureVisible(find.text('Save'));
      await tester.tap(find.text('Save'));
      await settle(tester);
      final update = harness.last.updatePipe;
      expect(
        update.members.agentIds,
        unorderedEquals(['june', 'omar', 'rivet']),
      );
      expect(update.sendOnly.agentIds, ['rivet']);
    });

    testWidgets('one that only sends, let read again, is asked first; a '
        "refusal shows as it comes", (tester) async {
      final harness = await settings(tester, senders(sendOnly: ['rivet']));
      expect(find.text('Only sends'), findsWidgets);
      tester
          .widget<Select<PipeAccess>>(choice('June'))
          .onChanged(PipeAccess.sendOnly);
      await settle(tester);
      tester
          .widget<Select<PipeAccess>>(choice('June'))
          .onChanged(PipeAccess.commands);
      await settle(tester);
      expect(
        find.text('Let June run commands on this computer?'),
        findsOneWidget,
      );
      await tester.tap(find.text('Let it run commands'));
      await settle(tester);
      harness.backend.refuse = "Rivet can't only send: it's quarantined.";
      await tester.ensureVisible(find.text('Save'));
      await tester.tap(find.text('Save'));
      await settle(tester);
      expect(
        find.text("Rivet can't only send: it's quarantined."),
        findsOneWidget,
      );
    });
  });

  group("a pipe's liaison uses no other (#414)", () {
    const held =
        "Freya is Partners' liaison: it reads that pipe, so it can't use "
        'another.';
    const rivetHeld =
        "Rivet is Launch's liaison: it reads that pipe, so it can't use "
        'another.';

    Workspace boundary(Pipe pipe) =>
        _workspace([
            pipe
              ..heldAgents.addAll([
                PipeAgentHeld(agentId: 'freya', reason: held),
                PipeAgentHeld(agentId: 'rivet', reason: rivetHeld),
              ]),
          ], selected: 'pipe1')
          ..agents.addAll([
            _agent('freya', 'Freya', role: 'Liaison'),
            _agent('rivet', 'Rivet', project: ''),
          ]);

    Future<Harness> settings(WidgetTester tester, Workspace snapshot) async {
      final harness = await show(tester, snapshot);
      harness.backend.pipeChat.load(PipeTranscript(pipeId: 'pipe1'));
      await settle(tester);
      await tester.tap(find.text('Acme launch').last);
      await settle(tester);
      return harness;
    }

    Finder switchOf(String label) => find.byWidgetPredicate(
      (widget) => widget is Toggle && widget.label == label,
    );

    Toggle toggle(WidgetTester tester, String label) =>
        tester.widget<Toggle>(switchOf(label));

    Future<void> turn(WidgetTester tester, String label) async {
      await tester.ensureVisible(switchOf(label));
      await tester.tap(switchOf(label), warnIfMissed: false);
      await settle(tester);
    }

    testWidgets("in the settings, another pipe's liaison is off, can't be "
        'turned on, and says why', (tester) async {
      final harness = await settings(
        tester,
        boundary(_pipe(members: ['june', 'omar'], lead: 'june')),
      );
      expect(find.text(held), findsOneWidget);
      expect(find.text(rivetHeld), findsOneWidget);
      expect(toggle(tester, 'Freya').onChanged, isNull);
      await turn(tester, 'Freya');
      expect(toggle(tester, 'Freya').value, isFalse);
      // The others are as they were.
      expect(
        tester
            .widget<Select<PipeAccess>>(
              find.byWidgetPredicate(
                (widget) =>
                    widget is Select<PipeAccess> && widget.title == 'Omar',
              ),
            )
            .options
            .map((option) => option.label),
        ['Reads and runs commands', 'Limited to the pipe', 'Only sends'],
      );
      await tester.ensureVisible(find.text('Save'));
      await tester.tap(find.text('Save'));
      await settle(tester);
      expect(harness.last.updatePipe.hasMembers(), isFalse);
    });

    testWidgets("approving, another pipe's liaison is off and says why", (
      tester,
    ) async {
      final harness = await show(
        tester,
        boundary(_pipe(state: PipeState.PIPE_STATE_APPROVAL, digits: '4821')),
      );
      expect(find.text(held), findsOneWidget);
      expect(toggle(tester, 'Freya').onChanged, isNull);
      await turn(tester, 'Freya');
      await turn(tester, 'Omar');
      await tester.ensureVisible(find.text('Approve'));
      await tester.tap(find.text('Approve'));
      await settle(tester);
      expect(harness.last.approvePipe.memberIds, ['omar']);
    });
  });

  testWidgets("Needs you's History lists Rivet letting an agent only send, "
      'in Claktory\'s words, and opens the pipe (#414)', (tester) async {
    final now = DateTime.now();
    final snapshot = _workspace([
      _pipe(members: ['june'], lead: 'june'),
    ])..agents.add(_agent('rivet', 'Rivet', project: ''));
    final harness = Harness(snapshot)
      ..backend.needsYouHistory = [
        NeedsYouRecord(
          kind: NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_PIPE_MEMBER,
          outcome: NeedsYouOutcome.NEEDS_YOU_OUTCOME_CHANGED,
          agentId: 'rivet',
          agentName: 'Rivet',
          subjectAgentId: 'june',
          pipeId: 'pipe1',
          title: 'Rivet let June send to this pipe without reading it',
          decidedMs: Int64(
            now.subtract(const Duration(hours: 1)).millisecondsSinceEpoch,
          ),
        ),
      ];
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    await tester.tap(find.text('History'));
    await settle(tester);
    await settle(tester);
    final title = find.text(
      'Rivet let June send to this pipe without reading it',
    );
    expect(title, findsOneWidget);
    expect(
      find.textContaining('Rivet · Acme launch', findRichText: true),
      findsOneWidget,
    );
    await tester.tap(title);
    await settle(tester);
    expect(harness.last.selectPipe.pipeId, 'pipe1');
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
            choice: true,
          ),
        ),
        findsOneWidget,
      );
      // Each member's choice (#414): Omar's, the last.
      void choose(PipeAccess access) => tester
          .widget<Select<PipeAccess>>(find.byType(Select<PipeAccess>).last)
          .onChanged(access);
      expect(
        tester
            .widget<Select<PipeAccess>>(find.byType(Select<PipeAccess>).last)
            .value,
        PipeAccess.limited,
      );
      choose(PipeAccess.commands);
      await settle(tester);
      expect(
        find.text('Let Omar run commands on this computer?'),
        findsOneWidget,
      );
      // Having read the pipe means nothing any more (#346).
      expect(
        find.text('It will be able to reach your files, secrets and network.'),
        findsOneWidget,
      );
      // Cancel keeps it limited.
      await tester.tap(find.text('Cancel').last);
      await settle(tester);
      expect(find.text('Pipe settings'), findsOneWidget);
      choose(PipeAccess.commands);
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
      tester
          .widget<Select<PipeAccess>>(find.byType(Select<PipeAccess>).first)
          .onChanged(PipeAccess.limited);
      await settle(tester);
      expect(find.text('Pipe settings'), findsOneWidget);
      await tester.ensureVisible(find.text('Save'));
      await tester.tap(find.text('Save'));
      await settle(tester);
      expect(harness.last.updatePipe.quarantined.agentIds, ['june']);
    });

    testWidgets('the pipe no longer lists who read it (#346, Matt)', (
      tester,
    ) async {
      // An older backend's mark, as Omar's here.
      await show(tester, limits());
      expect(
        find.textContaining('Has read this pipe', findRichText: true),
        findsNothing,
      );
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
    test('only web pages count, in one line (#346, Matt)', () {
      PipeTaint page(String site) => PipeTaint(kind: 'browser', pipeName: site);
      final pipe = PipeTaint(pipeId: 'pipe1', pipeName: 'Acme launch');
      final dm = PipeTaint(
        kind: 'dm',
        pipeName: 'Direct messages with Ana · K7QF2M',
      );
      expect(taintLine([pipe, dm], agent: 'June'), isNull);
      expect(
        taintLine([pipe, page('https://docs.example.com')], agent: 'Ada'),
        'from Ada, which read a page on https://docs.example.com',
      );
      expect(
        taintLine([
          page('https://a.example'),
          page('https://b.example'),
          page('https://a.example'),
          page('https://c.example'),
        ], agent: 'Ada'),
        'from Ada, which read 3 pages',
      );
    });

    testWidgets('one line after a globe; nothing for pipes', (tester) async {
      final harness = Harness(_workspace([_pipe()]));
      await harness.show(
        tester,
        home: Column(
          children: [
            TaintLabel(
              taint: [
                PipeTaint(pipeId: 'pipe1', pipeName: 'Acme launch'),
                PipeTaint(kind: 'browser', pipeName: 'https://a.example'),
                PipeTaint(kind: 'browser', pipeName: 'https://b.example'),
              ],
              agent: 'June',
            ),
            TaintLabel(
              taint: [PipeTaint(pipeId: 'pipe1', pipeName: 'Acme launch')],
              agent: 'Omar',
            ),
          ],
        ),
      );
      expect(find.text('from June, which read 2 pages'), findsOneWidget);
      expect(find.textContaining('Omar'), findsNothing);
      expect(find.textContaining('the pipe'), findsNothing);
      expect(find.byType(PortalGlyph), findsNothing);
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

  group("an agent's card to approve a pipe (#471 G1)", () {
    PipeApprovalProposal proposal({
      List<String> members = const ['june', 'omar'],
      List<String> canRun = const ['omar'],
      String lead = 'june',
      List<String> sendOnly = const [],
      PipeApprovalBaseline? askedFrom,
    }) => PipeApprovalProposal(
      pipeId: 'pipe1',
      memberIds: members,
      canRunCommandsIds: canRun,
      leadId: lead,
      reviewIncoming: true,
      reviewOutgoing: false,
      sendOnlyIds: sendOnly,
      // The pipe as it was when asked: a new one, nothing set.
      askedFrom: askedFrom ?? PipeApprovalBaseline(),
    );

    Transcript conversation(
      PipeApprovalProposal proposal, {
      String reason = 'Matt asked me to set it up for Omar.',
      int updatedMs = 0,
      String updateReason = '',
    }) => Transcript(
      agentId: 'june',
      entries: [
        Entry(
          seq: Int64(9),
          timestampMs: Int64(1000),
          askPerson: AskPerson(
            id: 'ask9',
            title: 'Approve the pipe Acme launch',
            state: AskPersonState.ASK_PERSON_STATE_PENDING,
            actionDescription:
                'June asks you to approve the pipe "Acme launch", with '
                'the choices below.',
            actionReason: reason,
            pipeApproval: proposal,
            updatedMs: Int64(updatedMs),
            updateReason: updateReason,
          ),
        ),
      ],
    );

    Future<Harness> showCard(
      WidgetTester tester,
      PipeApprovalProposal proposal, {
      Pipe? pipe,
      List<Agent> more = const [],
      Transcript? transcript,
      Size size = const Size(1200, 1400),
    }) async {
      final snapshot = _workspace([
        pipe ??
            _pipe(
              state: PipeState.PIPE_STATE_APPROVAL,
              digits: '4821',
              review: false,
            ),
      ]);
      snapshot.agents.addAll(more);
      final harness = Harness(snapshot);
      harness.backend.transcript.load(transcript ?? conversation(proposal));
      await harness.show(tester, size: size);
      await settle(tester);
      return harness;
    }

    Future<void> approve(WidgetTester tester) async {
      await tester.ensureVisible(find.text('Approve'));
      await tester.tap(find.text('Approve'));
      await settle(tester);
    }

    testWidgets('it starts from what the agent proposes, shows the digits '
        'to compare yourself, and Approve sends exactly that through the '
        'card', (tester) async {
      final harness = await showCard(tester, proposal());
      expect(find.text('Ana wants to connect'), findsOneWidget);
      expect(
        find.text(
          'Compare these with Ana yourself, by voice or in person. '
          "Don't approve because an agent says they match.",
        ),
        findsOneWidget,
      );
      expect(find.textContaining('Approve only if'), findsNothing);
      // Claktory's words, then the agent's reason apart, as its.
      expect(find.text("June's reason"), findsOneWidget);
      // No Decline here: turning the pipe down stays in the pipe.
      expect(find.text('Decline'), findsNothing);
      expect(find.text('Dismiss'), findsOneWidget);

      await approve(tester);
      final sent = harness.last.approvePipe;
      expect(sent.pipeId, 'pipe1');
      expect(sent.approve, isTrue);
      expect(sent.cardId, 'ask9');
      expect(sent.memberIds, unorderedEquals(['june', 'omar']));
      expect(sent.leadId, 'june');
      expect(sent.liaisonName, '');
      // June can't run commands: limited to the pipe.
      expect(sent.quarantined.agentIds, ['june']);
      expect(sent.reviewIncoming, isTrue);
      expect(sent.reviewOutgoing, isFalse);
      expect(sent.sendOnly.agentIds, isEmpty);
    });

    testWidgets('each choice is the person\'s to change before Approve', (
      tester,
    ) async {
      final harness = await showCard(tester, proposal());
      // Omar's switch in the card; the sidebar's row comes first.
      await tester.ensureVisible(find.text('Omar').last);
      await tester.tap(find.text('Omar').last);
      await tester.pump();
      final incoming = find.text(
        'Review messages before your agents read them',
      );
      await tester.ensureVisible(incoming);
      await tester.tap(incoming);
      await tester.pump();
      await approve(tester);
      final sent = harness.last.approvePipe;
      expect(sent.memberIds, ['june']);
      expect(sent.reviewIncoming, isFalse);
      expect(sent.cardId, 'ask9');
    });

    testWidgets("Dismiss is the card's, and a comment alone is a reply to "
        'the agent, the pipe still waiting', (tester) async {
      final harness = await showCard(tester, proposal());
      await tester.ensureVisible(find.text('Dismiss'));
      await tester.tap(find.text('Dismiss'));
      await settle(tester);
      expect(harness.last.answerAsk.dismiss, isTrue);
      expect(harness.last.answerAsk.seq.toInt(), 9);
      expect(harness.backend.sent.where((m) => m.hasApprovePipe()), isEmpty);
    });

    testWidgets('a comment typed is a reply to the agent: Submit, nothing '
        'approved', (tester) async {
      final harness = await showCard(tester, proposal());
      final field = find.descendant(
        of: find.byType(DecisionComment),
        matching: find.byType(EditableText),
      );
      await tester.ensureVisible(find.text('Add a comment'));
      await tester.tap(find.text('Add a comment'));
      await tester.pump();
      await tester.enterText(field, 'Add Kai too');
      await tester.pump();
      expect(
        find.text('The pipe still waits for you. June gets your comment.'),
        findsOneWidget,
      );
      await tester.tap(find.text('Submit'));
      await settle(tester);
      expect(harness.last.answerAsk.reply, isTrue);
      expect(harness.last.answerAsk.text, 'Add Kai too');
      expect(harness.backend.sent.where((m) => m.hasApprovePipe()), isEmpty);
    });

    testWidgets('the agent changing what it proposes is read again and '
        'said', (tester) async {
      final harness = await showCard(tester, proposal());
      harness.backend.transcript.load(
        conversation(proposal(members: ['omar'], canRun: [], lead: 'omar')),
      );
      await settle(tester);
      expect(
        find.text('June changed what it proposes. Check it again.'),
        findsOneWidget,
      );
      await approve(tester);
      final sent = harness.last.approvePipe;
      expect(sent.memberIds, ['omar']);
      expect(sent.leadId, 'omar');
      expect(sent.quarantined.agentIds, ['omar']);
    });

    // Zucchero's row 535: on a 375 pt phone, the line beside the buttons
    // goes above them whole, the Verified mark under who; updated before
    // it's shown, it says so once and asks to check it again.
    testWidgets('on a phone it fits; updated before it shows, it says so '
        'once', (tester) async {
      final now = DateTime.now().millisecondsSinceEpoch;
      await showCard(
        tester,
        proposal(),
        size: const Size(375, 2400),
        transcript: conversation(
          proposal(),
          reason: 'Second try: Kai leads.',
          updatedMs: now,
          updateReason: 'Second try: Kai leads.',
        ),
      );
      expect(tester.takeException(), isNull);
      expect(find.text('Second try: Kai leads.'), findsOneWidget);
      expect(
        find.text('June changed what it proposes. Check it again.'),
        findsOneWidget,
      );
      final words = tester.getRect(
        find.text('June changed what it proposes. Check it again.'),
      );
      final approve = tester.getRect(find.text('Approve'));
      expect(words.bottom, lessThanOrEqualTo(approve.top));
      expect(words.width, greaterThan(200));
    });

    testWidgets('a narrowing of the pipe since the agent asked is said, '
        'and the choice still wider shows it; Approve stays the '
        "person's (Fugo)", (tester) async {
      // Asked with Omar in the pipe, able to run commands; limited to the
      // pipe at once, and Review incoming on, before Matt saw the card.
      final harness = await showCard(
        tester,
        proposal(askedFrom: PipeApprovalBaseline(memberIds: ['omar'])),
        pipe: _pipe(
          state: PipeState.PIPE_STATE_APPROVAL,
          digits: '4821',
          review: false,
          members: ['omar'],
          limited: ['omar'],
        )..reviewIncoming = true,
      );
      expect(
        find.text(
          "The pipe's settings changed since June asked. Check them again.",
        ),
        findsOneWidget,
      );
      expect(
        find.text('Since June asked, Omar was limited to the pipe.'),
        findsOneWidget,
      );
      // The card has Review incoming on already: not wider.
      expect(
        find.textContaining('Review incoming was turned on'),
        findsNothing,
      );
      await approve(tester);
      expect(harness.last.approvePipe.cardId, 'ask9');
    });

    testWidgets('the pipe as it was asked says nothing changed', (
      tester,
    ) async {
      await showCard(
        tester,
        proposal(askedFrom: PipeApprovalBaseline(memberIds: ['omar'])),
        pipe: _pipe(
          state: PipeState.PIPE_STATE_APPROVAL,
          digits: '4821',
          review: false,
          members: ['omar'],
        ),
      );
      expect(find.textContaining('since June asked'), findsNothing);
      expect(find.textContaining('Since June asked'), findsNothing);
    });

    testWidgets('a narrowing while the card shows is said as it comes', (
      tester,
    ) async {
      final harness = await showCard(tester, proposal());
      expect(find.textContaining('since June asked'), findsNothing);
      // Rivet turns the pipe's Review outgoing on at once meanwhile.
      harness.backend.workspace.apply(
        harness.backend.workspace.workspace.deepCopy()
          ..pipes.first.reviewOutgoing = true,
      );
      await settle(tester);
      expect(
        find.text(
          "The pipe's settings changed since June asked. Check them again.",
        ),
        findsOneWidget,
      );
      expect(
        find.text(
          'Since June asked, Review outgoing was turned on in the pipe.',
        ),
        findsOneWidget,
      );
    });

    testWidgets('one of another project that only sends is on the card, '
        'kept as it only sends, never offered as the lead', (tester) async {
      final harness = await showCard(
        tester,
        proposal(sendOnly: ['rivet'], members: ['june', 'rivet']),
        more: [_agent('rivet', 'Rivet', project: '')],
      );
      expect(find.text('Rivet'), findsWidgets);
      expect(find.text('Only sends'), findsOneWidget);
      await approve(tester);
      final sent = harness.last.approvePipe;
      expect(sent.memberIds, unorderedEquals(['june', 'rivet']));
      expect(sent.sendOnly.agentIds, ['rivet']);
      expect(sent.quarantined.agentIds, ['june']);
    });

    testWidgets("a pipe no longer waiting for approval can't be approved "
        'from the card; Dismiss can', (tester) async {
      final harness = await showCard(
        tester,
        proposal(),
        pipe: _pipe(state: PipeState.PIPE_STATE_OPEN),
      );
      expect(
        find.text("That pipe isn't waiting for your approval any more."),
        findsOneWidget,
      );
      expect(find.text('Approve'), findsNothing);
      await tester.tap(find.text('Dismiss'));
      await settle(tester);
      expect(harness.last.answerAsk.dismiss, isTrue);
    });
  });

  group("an agent's card to accept a change at a pipe's other end (#471 "
      'G7)', () {
    Transcript conversation(PipeIdentityChange change) => Transcript(
      agentId: 'june',
      entries: [
        Entry(
          seq: Int64(11),
          timestampMs: Int64(1000),
          askPerson: AskPerson(
            id: 'ask11',
            title: 'Trust the pipe Acme launch again',
            state: AskPersonState.ASK_PERSON_STATE_PENDING,
            actionDescription:
                'June asks you to accept the change on the pipe "Acme '
                'launch".',
            actionReason: 'Ana says she moved to her company account.',
            pipeTrust: PipeTrustProposal(
              pipeId: 'pipe1',
              identityChange: change,
            ),
          ),
        ),
      ],
    );

    Pipe changed(PipeIdentityChange change) => _pipe()
      ..peer.verifiedEmail = 'bo@example.com'
      ..peer.identityChange = change
      ..peer.previousEmail = 'ana@acme.example';

    Future<Harness> showCard(
      WidgetTester tester,
      PipeIdentityChange asked, {
      Pipe? pipe,
    }) async {
      final harness = Harness(_workspace([pipe ?? changed(asked)]));
      harness.backend.transcript.load(conversation(asked));
      await harness.show(tester, size: const Size(1200, 1400));
      await settle(tester);
      return harness;
    }

    const account = PipeIdentityChange.PIPE_IDENTITY_CHANGE_ACCOUNT_CHANGED;
    const email = PipeIdentityChange.PIPE_IDENTITY_CHANGE_EMAIL_CHANGED;
    const lapsed = PipeIdentityChange.PIPE_IDENTITY_CHANGE_LAPSED;

    testWidgets("another account: who it was and is, from the pipe; "
        "Claktory's warning, the agent's reason under it; Trust accepts "
        'through the card, asking nothing more', (tester) async {
      final harness = await showCard(tester, account);
      expect(
        find.text(
          "This pipe's Claktory is now confirmed as bo@example.com's, not "
          "ana@acme.example's.",
        ),
        findsOneWidget,
      );
      expect(
        find.text('Their Claktory now signs in with a different account.'),
        findsOneWidget,
      );
      final warning = find.text(
        'Check with Ana yourself that this change is theirs.',
      );
      expect(warning, findsOneWidget);
      // The reason, named as June's, under the warning, never above it.
      final reason = find.text("June's reason");
      expect(reason, findsOneWidget);
      expect(
        tester.getTopLeft(reason).dy,
        greaterThan(tester.getTopLeft(warning).dy),
      );
      await tester.ensureVisible(find.text('Trust'));
      await tester.tap(find.text('Trust'));
      await settle(tester);
      final accepted = harness.last.acceptPipeIdentity;
      expect(accepted.pipeId, 'pipe1');
      expect(accepted.cardId, 'ask11');
    });

    testWidgets('a new email: OK, through the card', (tester) async {
      final harness = await showCard(tester, email);
      expect(find.text('Email changed'), findsOneWidget);
      expect(find.text('Trust'), findsNothing);
      await tester.ensureVisible(find.text('OK'));
      await tester.tap(find.text('OK'));
      await settle(tester);
      expect(harness.last.acceptPipeIdentity.cardId, 'ask11');
    });

    testWidgets("a change that isn't the one asked about any more can't be "
        'accepted from the card; Dismiss can', (tester) async {
      final harness = await showCard(tester, account, pipe: changed(lapsed));
      expect(
        find.text("That change isn't waiting for you any more."),
        findsOneWidget,
      );
      expect(find.text('Trust'), findsNothing);
      await tester.tap(find.text('Dismiss'));
      await settle(tester);
      expect(harness.last.answerAsk.dismiss, isTrue);
      expect(
        harness.backend.sent.where((m) => m.hasAcceptPipeIdentity()),
        isEmpty,
      );
    });

    testWidgets('a comment typed is a reply to the agent: Submit, nothing '
        'accepted', (tester) async {
      final harness = await showCard(tester, account);
      await tester.ensureVisible(find.text('Add a comment'));
      await tester.tap(find.text('Add a comment'));
      await tester.pump();
      await tester.enterText(
        find.descendant(
          of: find.byType(DecisionComment),
          matching: find.byType(EditableText),
        ),
        'Did she really?',
      );
      await tester.pump();
      await tester.tap(find.text('Submit'));
      await settle(tester);
      expect(harness.last.answerAsk.reply, isTrue);
      expect(
        harness.backend.sent.where((m) => m.hasAcceptPipeIdentity()),
        isEmpty,
      );
    });
  });
}
