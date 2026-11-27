// Lucy's Needs you cards (needs-you-cards.md), drawn as they look, for her
// to check against the mock-ups: each card alone in June's conversation,
// on a Mac window and on an iPhone, before and after words are typed. With
// SHOTS set, each is drawn to a PNG there:
//   SHOTS=/tmp/cards flutter test test/needs_you_cards_shots_test.dart
// Without it, each still lays out without an overflow or an exception.
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/controls.dart' show TextField;
import 'package:factory_app/decision.dart' show DecisionComment;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;

Workspace _workspace() => Workspace(
  projects: [Project(id: 'p1', name: 'Claktory')],
  vault: VaultState(
    exists: true,
    unlocked: true,
    secrets: [SecretInfo(id: 's1', name: 'Hetzner')],
  ),
  agents: [
    Agent(
      id: 'rivet',
      name: 'Rivet',
      role: 'Admin',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
    ),
    Agent(
      id: 'june',
      name: 'June',
      role: 'Backend',
      projectId: 'p1',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
      waitingSeqs: [Int64(4)],
    ),
  ],
  selectedAgentId: 'june',
);

Entry _ask(AskPerson ask) =>
    Entry(seq: Int64(4), timestampMs: Int64(1004), askPerson: ask);

final _cards = <String, (Entry, String?)>{
  'approval': (
    _ask(
      AskPerson(
        id: 'a1',
        title: 'Deploy the account service?',
        body:
            'The migration is tested on staging. It takes about two '
            'minutes and the sign-in page is down meanwhile.\n\n'
            'Shall I deploy it now?',
        state: AskPersonState.ASK_PERSON_STATE_PENDING,
      ),
    ),
    'After 18:00, please',
  ),
  'written-answer': (
    _ask(
      AskPerson(
        id: 'a2',
        title: 'Which domain should the mail come from?',
        wantsText: true,
        state: AskPersonState.ASK_PERSON_STATE_PENDING,
      ),
    ),
    'mail.claktory.com',
  ),
  'step': (
    _ask(
      AskPerson(
        id: 'a3',
        title: 'Set up the printer',
        state: AskPersonState.ASK_PERSON_STATE_PENDING,
        steps: [
          AskStep(
            text: 'Plug the printer in',
            state: AskStepState.ASK_STEP_STATE_DONE,
          ),
          AskStep(text: 'Open System Settings → Printers & Scanners.'),
          AskStep(text: 'Add the printer'),
          AskStep(text: 'Print a test page'),
        ],
      ),
    ),
    "The menu isn't there, it's called Printers in Sequoia",
  ),
  'execute': (
    _ask(
      AskPerson(
        id: 'a4',
        title: 'List the build folder',
        body: 'To see why the **build** is so big.',
        state: AskPersonState.ASK_PERSON_STATE_PENDING,
        command: AskCommand(
          command: 'du -sh target/*',
          cwd: '/Users/me/factory',
          machine: "Matt's Mac Studio",
          timeoutSeconds: 120,
        ),
      ),
    ),
    'Not now, a build is running',
  ),
  'secret': (
    Entry(
      seq: Int64(4),
      timestampMs: Int64(1004),
      secretRequest: SecretRequest(
        id: 'r1',
        agentId: 'june',
        adminAgentId: 'rivet',
        secretName: 'Throwaway Google account',
        newSecret: NewSecret(
          name: 'Throwaway Google account',
          description: 'Signs in to Google from the browser',
          variables: [
            SecretVariable(name: 'GOOGLE_TEST_EMAIL'),
            SecretVariable(name: 'GOOGLE_TEST_PASSWORD'),
          ],
        ),
        reason:
            'I need an account with nothing on it, to test signing in '
            "to Google from Claktory's browser.",
        state: SecretRequestState.SECRET_REQUEST_STATE_PENDING,
      ),
    ),
    'I just gave you my super old gmail',
  ),
};

Future<void> _shot(WidgetTester tester, String name) async {
  final folder = Platform.environment['SHOTS'];
  if (folder == null) return;
  final boundary = tester.binding.renderViews.first;
  final layer = boundary.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(
    () => layer.toImage(Offset.zero & boundary.size, pixelRatio: 2),
  );
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}

void main() {
  setUpAll(loadGeist);

  for (final phone in [false, true]) {
    for (final MapEntry(key: name, value: (entry, words)) in _cards.entries) {
      final where = phone ? 'iphone' : 'mac';
      testWidgets('$name on the $where', (tester) async {
        if (phone) {
          debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
        }
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        final harness = Harness(_workspace());
        harness.backend.transcript.load(
          Transcript(agentId: 'june', entries: [entry]),
        );
        await harness.show(
          tester,
          size: phone ? const Size(393, 852) : const Size(1100, 800),
        );
        await settle(tester);
        expect(tester.takeException(), isNull);
        await _shot(tester, '$name-$where');
        if (words == null) return;
        final field = find.descendant(
          of: find.byType(DecisionComment),
          matching: find.byType(EditableText),
        );
        await tester.ensureVisible(field.first);
        await tester.enterText(field.first, words);
        await settle(tester);
        expect(tester.takeException(), isNull);
        await _shot(tester, '$name-$where-typed');
      });
    }

    // History's words for where a procedure ended and how a secret was
    // given (§7, N12).
    testWidgets('history on the ${phone ? 'iphone' : 'mac'}', (tester) async {
      if (phone) debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final now = DateTime.now();
      var hours = 1;
      NeedsYouRecord record(
        String title, {
        NeedsYouRecordKind kind =
            NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_QUESTION,
        required NeedsYouOutcome outcome,
      }) => NeedsYouRecord(
        kind: kind,
        agentId: 'june',
        agentName: 'June',
        seq: Int64(4),
        title: title,
        outcome: outcome,
        askedMs: Int64(
          now.subtract(Duration(hours: hours + 1)).millisecondsSinceEpoch,
        ),
        decidedMs: Int64(
          now.subtract(Duration(hours: hours++)).millisecondsSinceEpoch,
        ),
      );
      const secret = NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_SECRET_REQUEST;
      const approved = NeedsYouOutcome.NEEDS_YOU_OUTCOME_APPROVED;
      final harness = Harness(_workspace())
        ..backend.needsYouHistory = [
          record(
              'Set up the printer',
              outcome: NeedsYouOutcome.NEEDS_YOU_OUTCOME_RESOLVED,
            )
            ..step = 2
            ..steps = 4
            ..stepStopped = true
            ..agentReason = "I'll print from the other Mac instead.",
          record(
              'Sign the release',
              outcome: NeedsYouOutcome.NEEDS_YOU_OUTCOME_DISMISSED,
            )
            ..step = 2
            ..steps = 4,
          record(
              'Pair the keyboard',
              outcome: NeedsYouOutcome.NEEDS_YOU_OUTCOME_REPLIED,
            )
            ..step = 3
            ..steps = 4,
          record('Reset the router', outcome: approved)
            ..step = 4
            ..steps = 4,
          record('Hetzner', kind: secret, outcome: approved)
            ..secretOutcome =
                NeedsYouSecretOutcome.NEEDS_YOU_SECRET_OUTCOME_GAVE
            ..secretGivenName = 'Hetzner production',
          record('Resend', kind: secret, outcome: approved)
            ..secretOutcome =
                NeedsYouSecretOutcome.NEEDS_YOU_SECRET_OUTCOME_ADDED,
          record('Deploy key', kind: secret, outcome: approved)
            ..secretOutcome =
                NeedsYouSecretOutcome.NEEDS_YOU_SECRET_OUTCOME_GENERATED
            ..secretGivenName = 'deploy-key',
          record('Cloudflare', kind: secret, outcome: approved)
            ..secretOutcome =
                NeedsYouSecretOutcome.NEEDS_YOU_SECRET_OUTCOME_ADMIN
            ..subjectAgentId = 'rivet',
        ];
      await harness.show(
        tester,
        size: phone ? const Size(393, 852) : const Size(1100, 800),
      );
      await settle(tester);
      harness.page.value = AppPage.inbox;
      await pumpFrames(tester);
      // Needs you opens on its list everywhere (#599), History a tab.
      await tester.tap(find.text('History'));
      await settle(tester);
      await settle(tester);
      expect(
        find.textContaining('Left to Rivet', findRichText: true),
        findsOneWidget,
      );
      await _shot(tester, 'history-${phone ? 'iphone' : 'mac'}');
    });

    // The secret's line as its values are pasted (§3.5): one missing, then
    // all in.
    testWidgets('secret values on the ${phone ? 'iphone' : 'mac'}', (
      tester,
    ) async {
      if (phone) debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final where = phone ? 'iphone' : 'mac';
      final harness = Harness(_workspace());
      harness.backend.transcript.load(
        Transcript(agentId: 'june', entries: [_cards['secret']!.$1]),
      );
      await harness.show(
        tester,
        size: phone ? const Size(393, 852) : const Size(1100, 800),
      );
      await settle(tester);
      final values = find.descendant(
        of: find.byWidgetPredicate(
          (w) => w is TextField && w.placeholder == 'Paste it here',
        ),
        matching: find.byType(EditableText),
      );
      await tester.enterText(values.first, 'matt.test@gmail.com');
      await settle(tester);
      expect(find.text('Enter GOOGLE_TEST_PASSWORD first.'), findsOneWidget);
      expect(find.text('Other ways…'), findsOneWidget);
      await _shot(tester, 'secret-$where-one-value');
      await tester.enterText(values.last, 'correct horse');
      await settle(tester);
      expect(find.text('Dismiss discards what you pasted.'), findsOneWidget);
      await _shot(tester, 'secret-$where-values');
    });
  }
}
