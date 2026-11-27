// Claktory's browser's handoff cards: each asks only for the person's part.

import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/errands/browser_session.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _with(
  AgentBrowserState state, {
  String errandId = '',
  List<Errand> errands = const [],
  AgentBrowserSetup? setup,
  List<String> sites = const [],
  String timeZone = '',
  List<String> languages = const [],
  List<AgentBrowserAccount> accounts = const [],
}) => Workspace(
  agents: [Agent(id: 'a1', name: 'Ada', modelAvailable: true)],
  agentBrowser: AgentBrowser(
    state: state,
    errandId: errandId,
    setup: setup,
    signedInSites: sites,
    timeZone: timeZone,
    languages: languages,
    accounts: accounts,
  ),
  errands: errands,
);

/// A session whose requests are recorded.
class _Session {
  _Session() {
    session = AgentBrowserSession(
      send: (message) async {
        sent.add(message);
        final reason = refuse;
        if (reason != null) throw RequestFailure(reason);
      },
      timeZone: () async => 'Europe/Rome',
      languages: () => ['it-IT', 'en-GB'],
    );
  }

  final sent = <ClientMessage>[];
  String? refuse;
  late final AgentBrowserSession session;
}

/// The harness's backend's session.
_Session _attach(Harness harness) {
  final s = _Session();
  AgentBrowserSession.attach(harness.backend, s.session);
  return s;
}

void main() {
  group('a handoff card', () {
    testWidgets('shows in the conversation, and Done answers it', (
      tester,
    ) async {
      final harness = Harness(
        _with(
          AgentBrowserState.AGENT_BROWSER_STATE_PERSONS_TURN,
          errandId: 'e1',
          errands: [
            Errand(
              id: 'e1',
              agentId: 'a1',
              state: ErrandState.ERRAND_STATE_PERSONS_TURN,
            ),
          ],
        )..selectedAgentId = 'a1',
      );
      _attach(harness);
      harness.backend.transcript.load(
        Transcript(
          agentId: 'a1',
          entries: [
            Entry(
              seq: Int64(12),
              timestampMs: Int64(1000),
              askPerson: AskPerson(
                id: 'ask12',
                title: 'Pay the registration fee',
                body: 'The form is filled in; only the card is left.',
                state: AskPersonState.ASK_PERSON_STATE_PENDING,
                handoff: ErrandHandoff(
                  errandId: 'e1',
                  kind: ErrandHandoffKind.ERRAND_HANDOFF_KIND_OTHER,
                  origin: 'https://play.google.com',
                  title: 'Checkout',
                  seq: Int64(12),
                ),
              ),
            ),
          ],
        ),
      );
      await harness.show(tester, size: const Size(1400, 900));
      await tester.pump();
      expect(find.text('Ada needs your OK'), findsOneWidget);
      expect(
        find.textContaining('play.google.com', findRichText: true),
        findsWidgets,
      );
      expect(find.text('Ada waits until you choose Done.'), findsOneWidget);
      expect(find.text("Can't do this"), findsOneWidget);
      expect(find.text('Show me'), findsNothing);
      await tester.tap(find.text('Done').last);
      await tester.pump();
      final answer = harness.backend.sent.lastWhere((m) => m.hasAnswerAsk());
      expect(answer.answerAsk.seq, Int64(12));
      expect(answer.answerAsk.dismiss, isFalse);
    });

    testWidgets("Can't do this sends what's typed as its reason, and Done "
        'sends no words', (tester) async {
      final harness = Harness(
        _with(AgentBrowserState.AGENT_BROWSER_STATE_PERSONS_TURN)
          ..selectedAgentId = 'a1',
      );
      _attach(harness);
      harness.backend.transcript.load(
        Transcript(
          agentId: 'a1',
          entries: [
            Entry(
              seq: Int64(14),
              timestampMs: Int64(1000),
              askPerson: AskPerson(
                id: 'ask14',
                title: 'Your turn: sign in',
                state: AskPersonState.ASK_PERSON_STATE_PENDING,
                handoff: ErrandHandoff(
                  errandId: 'e1',
                  kind: ErrandHandoffKind.ERRAND_HANDOFF_KIND_SIGN_IN,
                  origin: 'https://accounts.example.com',
                  seq: Int64(14),
                ),
              ),
            ),
          ],
        ),
      );
      await harness.show(tester, size: const Size(1400, 900));
      await tester.pump();
      await tester.enterText(find.byType(EditableText).first, 'No phone here');
      await tester.pump();
      // Typed, Done stays Done: the words go only with Can't do this.
      expect(
        find.text("Your words go only with Can't do this."),
        findsOneWidget,
      );
      await tester.tap(find.text("Can't do this"));
      await tester.pump();
      final answer = harness.backend.sent.lastWhere((m) => m.hasAnswerAsk());
      expect(answer.answerAsk.dismiss, isTrue);
      expect(answer.answerAsk.text, 'No phone here');
    });

    // Abbacchio's row 1640: typed words went out with Done, kept as the
    // reply and shown under Done, though the agent never got them.
    testWidgets('typed, Done still sends no words', (tester) async {
      final harness = Harness(
        _with(AgentBrowserState.AGENT_BROWSER_STATE_PERSONS_TURN)
          ..selectedAgentId = 'a1',
      );
      _attach(harness);
      harness.backend.transcript.load(
        Transcript(
          agentId: 'a1',
          entries: [
            Entry(
              seq: Int64(15),
              timestampMs: Int64(1000),
              askPerson: AskPerson(
                id: 'ask15',
                title: 'Your turn: pay',
                state: AskPersonState.ASK_PERSON_STATE_PENDING,
                handoff: ErrandHandoff(
                  errandId: 'e1',
                  kind: ErrandHandoffKind.ERRAND_HANDOFF_KIND_OTHER,
                  origin: 'https://shop.example.com',
                  seq: Int64(15),
                ),
              ),
            ),
          ],
        ),
      );
      await harness.show(tester, size: const Size(1400, 900));
      await tester.pump();
      await tester.enterText(find.byType(EditableText).first, 'Card declined');
      await tester.pump();
      await tester.tap(find.text('Done').last);
      await tester.pump();
      final answer = harness.backend.sent.lastWhere((m) => m.hasAnswerAsk());
      expect(answer.answerAsk.seq, Int64(15));
      expect(answer.answerAsk.dismiss, isFalse);
      expect(answer.answerAsk.text, isEmpty);
      expect(answer.answerAsk.attachments, isEmpty);
    });
  });
}
