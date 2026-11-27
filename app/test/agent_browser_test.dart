// Claktory's browser in the app (#435 P5), after Matt's card 222: the
// person never drives it. An agent's errand shows in its header (its panel,
// with the log, and Stop); Settings has how it is, Close browser, its time
// zone and languages, its sign-ins and Delete browser data; a handoff card
// asks only for the person's part.

import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/errands/browser_session.dart';
import 'package:factory_app/errands/browser_settings.dart';
import 'package:factory_app/errands/errand_controls.dart';
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
  test('the session says why a Stop failed', () {
    final s = _Session();
    s.refuse = "Claktory's browser isn't available yet.";
    s.session.stop();
    return Future<void>.delayed(Duration.zero, () {
      expect(s.sent.single.hasStopAgentBrowser(), isTrue);
      expect(s.session.error, "Claktory's browser isn't available yet.");
    });
  });

  group('in the app', () {
    final errand = Errand(
      id: 'e1',
      agentId: 'a1',
      purpose: 'Publish the app on Play',
      sites: ['https://play.google.com'],
      state: ErrandState.ERRAND_STATE_RUNNING,
    );

    testWidgets("hides everything where there's no browser", (tester) async {
      final harness = Harness(
        Workspace(
          agents: [Agent(id: 'a1', name: 'Ada')],
          selectedAgentId: 'a1',
        ),
      );
      _attach(harness);
      await harness.show(tester);
      expect(find.byType(ErrandHeaderControls), findsOneWidget);
      expect(
        find.descendant(
          of: find.byType(ErrandHeaderControls),
          matching: find.byType(Text),
        ),
        findsNothing,
      );
    });

    testWidgets('the agent header has its errand and Stop, never Take over '
        'or Watch', (tester) async {
      final harness = Harness(
        _with(
          AgentBrowserState.AGENT_BROWSER_STATE_DRIVEN,
          errandId: 'e1',
          errands: [errand],
        )..selectedAgentId = 'a1',
      );
      final s = _attach(harness);
      await harness.show(tester, size: const Size(1400, 900));
      expect(find.text('Take over'), findsNothing);
      expect(find.text('Watch'), findsNothing);
      await tester.tap(find.text('Stop'));
      await tester.pump();
      expect(s.sent.last.hasStopAgentBrowser(), isTrue);

      // The errand's panel: what it's for, how it goes, its log.
      await tester.tap(find.bySemanticsLabel('Errand'));
      await tester.pumpAndSettle();
      expect(find.text("Ada's errand"), findsOneWidget);
      expect(find.text('Publish the app on Play'), findsOneWidget);
      expect(find.textContaining('Only on play.google.com'), findsOneWidget);
    });

    testWidgets('in a narrow window the header gives way around them, '
        'never overflowing', (tester) async {
      final harness = Harness(
        _with(
          AgentBrowserState.AGENT_BROWSER_STATE_DRIVEN,
          errandId: 'e1',
          errands: [errand],
        )..selectedAgentId = 'a1',
      );
      _attach(harness);
      for (var width = 1400.0; width >= 800; width -= 50) {
        await harness.show(tester, size: Size(width, 900));
        expect(tester.takeException(), isNull, reason: '$width');
        expect(find.text('Stop'), findsOneWidget, reason: '$width');
        expect(
          find.bySemanticsLabel('More options for Ada'),
          findsOneWidget,
          reason: '$width',
        );
      }
    });
  });

  group('the browser in Settings', () {
    testWidgets('running, it says so and closes; nothing opens it', (
      tester,
    ) async {
      final harness = Harness(
        _with(AgentBrowserState.AGENT_BROWSER_STATE_DRIVEN),
      );
      final s = _attach(harness);
      await harness.show(
        tester,
        home: AgentBrowserSettings(
          workspace: harness.backend.workspace.workspace,
        ),
      );
      expect(find.textContaining('An agent is using it'), findsOneWidget);
      expect(find.text('Open'), findsNothing);
      await tester.tap(find.text('Close browser'));
      await tester.pump();
      expect(s.sent.last.hasStopAgentBrowser(), isTrue);
    });

    testWidgets('closed, it has nothing to close', (tester) async {
      final harness = Harness(_with(AgentBrowserState.AGENT_BROWSER_STATE_OFF));
      _attach(harness);
      await harness.show(
        tester,
        home: AgentBrowserSettings(
          workspace: harness.backend.workspace.workspace,
        ),
      );
      expect(find.text('Closed'), findsOneWidget);
      expect(find.text('Close browser'), findsNothing);
      expect(find.text('Open'), findsNothing);
    });

    // Abbacchio's nit: "Closed · Chrome Chrome/158.0.8127.37".
    testWidgets("Chrome's version reads once, as a number", (tester) async {
      final harness = Harness(
        _with(AgentBrowserState.AGENT_BROWSER_STATE_OFF)
          ..agentBrowser.chromeVersion = 'Chrome/158.0.8127.37',
      );
      _attach(harness);
      await harness.show(
        tester,
        home: AgentBrowserSettings(
          workspace: harness.backend.workspace.workspace,
        ),
      );
      expect(find.text('Closed · Chrome 158.0.8127.37'), findsOneWidget);
    });
  });

  group('settings', () {
    testWidgets('show the setup command while it can’t run', (tester) async {
      final harness = Harness(
        _with(
          AgentBrowserState.AGENT_BROWSER_STATE_UNAVAILABLE,
          setup: AgentBrowserSetup(
            problem: "Chrome isn't installed on this computer.",
            command: 'sudo claktory-browser-setup',
          ),
        ),
      );
      await harness.show(
        tester,
        home: AgentBrowserSettings(
          workspace: harness.backend.workspace.workspace,
        ),
      );
      expect(find.text("Chrome isn't installed on this computer."), findsOne);
      expect(find.textContaining('sudo claktory-browser-setup'), findsOne);
      expect(find.textContaining("administrator's password"), findsOne);
    });

    testWidgets("a Mac's setup command asks for no password", (tester) async {
      final harness = Harness(
        _with(
          AgentBrowserState.AGENT_BROWSER_STATE_UNAVAILABLE,
          setup: AgentBrowserSetup(
            problem: "Apple's container tool isn't running on this Mac.",
            command: 'container system start --enable-kernel-install',
          ),
        ),
      );
      await harness.show(
        tester,
        home: AgentBrowserSettings(
          workspace: harness.backend.workspace.workspace,
        ),
      );
      expect(
        find.text('Run this once in a terminal on this computer.'),
        findsOne,
      );
      expect(find.textContaining('password'), findsNothing);
    });

    testWidgets('show the time zone and languages, and change both', (
      tester,
    ) async {
      final harness = Harness(
        _with(AgentBrowserState.AGENT_BROWSER_STATE_OFF, languages: ['en-GB']),
      );
      _attach(harness);
      await harness.show(
        tester,
        home: AgentBrowserSettings(
          workspace: harness.backend.workspace.workspace,
        ),
      );
      expect(find.text("Time zone: this computer's"), findsOneWidget);
      expect(find.text('Languages: en-GB'), findsOneWidget);
      await tester.tap(find.text('Change…'));
      await tester.pumpAndSettle();
      expect(find.text('Time zone and languages'), findsWidgets);
    });

    testWidgets('the form finds a zone by its words, orders languages, and '
        'shows a refusal', (tester) async {
      final harness = Harness(_with(AgentBrowserState.AGENT_BROWSER_STATE_OFF));
      final s = _attach(harness);
      var closed = 0;
      await harness.show(
        tester,
        home: AgentBrowserLocaleForm(
          browser: AgentBrowser(languages: ['en-GB']),
          close: () => closed++,
          listTimeZones: () async => [
            'America/New_York',
            'Europe/London',
            'Europe/Rome',
          ],
        ),
      );
      await tester.pump();
      await tester.tap(find.byType(EditableText).first);
      await tester.enterText(find.byType(EditableText).first, 'new york');
      await tester.pump();
      await tester.tap(find.text('America/New_York'));
      await tester.pump();
      await tester.enterText(find.byType(EditableText).last, 'fr');
      await tester.tap(find.text('Add'));
      await tester.pump();
      await tester.tap(find.bySemanticsLabel('Move fr up'));
      await tester.pump();
      s.refuse = 'Choose at most 8 languages.';
      await tester.tap(find.text('Save'));
      await tester.pump();
      expect(find.text('Choose at most 8 languages.'), findsOneWidget);
      expect(closed, 0);
      s.refuse = null;
      await tester.tap(find.text('Save'));
      await tester.pump();
      final set = s.sent.last.setAgentBrowserLocale;
      expect(set.timeZone, 'America/New_York');
      expect(set.languages, ['fr', 'en-GB']);
      expect(closed, 1);
    });

    testWidgets('list its sign-ins and delete them on request', (tester) async {
      final harness = Harness(
        _with(
          AgentBrowserState.AGENT_BROWSER_STATE_OFF,
          sites: ['https://accounts.google.com'],
        ),
      );
      final s = _attach(harness);
      await harness.show(
        tester,
        home: AgentBrowserSettings(
          workspace: harness.backend.workspace.workspace,
        ),
      );
      expect(find.textContaining('accounts.google.com'), findsOneWidget);
      await tester.tap(find.text('Delete browser data…'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Delete').last);
      await tester.pumpAndSettle();
      expect(s.sent.last.hasDeleteAgentBrowserData(), isTrue);
    });
  });

  group('identity providers', () {
    final google = AgentBrowserAccount(
      provider: 'google',
      name: 'Google',
      secretId: 's1',
      secretName: 'Work Gmail',
      signedInMs: Int64(1764000000000),
    );

    Future<_Session> show(
      WidgetTester tester,
      List<AgentBrowserAccount> accounts,
    ) async {
      final harness = Harness(
        _with(AgentBrowserState.AGENT_BROWSER_STATE_OFF, accounts: accounts),
      );
      final s = _attach(harness);
      await harness.show(
        tester,
        home: AgentBrowserSettings(
          workspace: harness.backend.workspace.workspace,
        ),
      );
      return s;
    }

    testWidgets('each says which secret it signed in with, and signs out '
        'once confirmed', (tester) async {
      final s = await show(tester, [google]);
      expect(find.text('Google as Work Gmail'), findsOneWidget);
      expect(find.text('No sites yet.'), findsNothing);
      await tester.tap(find.text('Sign out'));
      await tester.pumpAndSettle();
      expect(
        find.text("Sign Claktory's browser out of Google?"),
        findsOneWidget,
      );
      await tester.tap(find.text('Sign out').last);
      await tester.pumpAndSettle();
      expect(s.sent.last.signOutAgentBrowserAccount.provider, 'google');
    });

    testWidgets("a refusal shows Claktory's sentence", (tester) async {
      final s = await show(tester, [google]);
      s.refuse = "Claktory's browser isn't signed in to Google.";
      await tester.tap(find.text('Sign out'));
      await tester.pumpAndSettle();
      await tester.tap(find.text('Sign out').last);
      await tester.pumpAndSettle();
      expect(
        find.textContaining("Claktory's browser isn't signed in to Google."),
        findsOneWidget,
      );
    });

    testWidgets('its secret gone, just the provider; signing out, no button', (
      tester,
    ) async {
      await show(tester, [
        AgentBrowserAccount(provider: 'apple', name: 'Apple'),
        google.deepCopy()..signingOut = true,
      ]);
      expect(find.text('Apple'), findsOneWidget);
      expect(find.text('Google as Work Gmail'), findsOneWidget);
      expect(
        find.text('Signing out when the errand using the browser ends.'),
        findsOneWidget,
      );
      // Apple's alone.
      expect(find.text('Sign out'), findsOneWidget);
    });
  });

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
                  kind: ErrandHandoffKind.ERRAND_HANDOFF_KIND_PAYMENT,
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
      // The cards' own heading and row (Illuso 48684): Done and Can't do
      // this, the field open, never Show me.
      expect(find.text("Ada's errand needs your OK"), findsOneWidget);
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
