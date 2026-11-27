import 'dart:async';

import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/controls.dart' show Button, showModal;
import 'package:factory_app/errands/browser_agent.dart';
import 'package:factory_app/errands/browser_session.dart';
import 'package:factory_app/errands/browser_settings.dart';
import 'package:factory_app/forms.dart' show AgentSettingsForm;
import 'package:factory_app/pipe_reports.dart' show containedKeeps;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart' show Sidebar;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Agent _agent(
  String id,
  String name, {
  bool browser = false,
  bool admin = false,
  bool quarantined = false,
  bool readsReports = false,
}) => Agent(
  id: id,
  name: name,
  color: 'blue',
  modelAvailable: true,
  quarantined: quarantined,
  readsReports: readsReports,
  extensions: [
    AgentExtension(id: 'web', enabled: true),
    AgentExtension(id: 'browser', enabled: browser),
    AgentExtension(
      id: 'agents',
      enabled: true,
      options: {if (admin) 'admin': OptionValue(on: true)}.entries,
    ),
  ],
);

Workspace _workspace({
  List<Agent>? agents,
  AgentBrowser? browser,
  List<AgentBrowser> browsers = const [],
}) => Workspace(
  agents:
      agents ??
      [
        _agent('rivet', 'Rivet', admin: true),
        _agent('yukako', 'Yukako', browser: true),
        _agent('koichi', 'Koichi', browser: true),
        _agent('june', 'June'),
      ],
  extensions: [
    Extension(id: 'agents', name: 'Messaging', icon: 'users'),
    Extension(id: 'web', name: 'Web', icon: 'globe'),
    Extension(
      id: 'browser',
      name: 'Browser',
      icon: 'globe',
      description: "Uses websites in Claktory's browser.",
    ),
  ],
  agentBrowser:
      browser ??
      AgentBrowser(
        state: AgentBrowserState.AGENT_BROWSER_STATE_OFF,
        chromeVersion: '142.0.7444.59',
      ),
  agentBrowsers: browsers,
  selectedAgentId: 'rivet',
);

final _yukakos = AgentBrowser(
  agentId: 'yukako',
  state: AgentBrowserState.AGENT_BROWSER_STATE_WARM,
  signedInSites: ['https://www.linkedin.com', 'https://x.com'],
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
      languages: () => ['it-IT', 'en-US'],
    );
  }

  final sent = <ClientMessage>[];
  String? refuse;
  late final AgentBrowserSession session;
}

_Session _attach(Harness harness) {
  final s = _Session();
  AgentBrowserSession.attach(harness.backend, s.session);
  return s;
}

/// Settings → Claktory's browser, alone.
Future<(Harness, _Session)> _settings(
  WidgetTester tester,
  Workspace workspace,
) async {
  final harness = Harness(workspace);
  final s = _attach(harness);
  await harness.show(
    tester,
    size: const Size(900, 1000),
    home: AgentBrowserSettings(workspace: harness.backend.workspace.workspace),
  );
  await tester.pump();
  return (harness, s);
}

/// [id]'s Agent settings, its extensions unfolded.
Future<Harness> _agentSettings(
  WidgetTester tester,
  Workspace workspace,
  String id,
) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final harness = Harness(workspace);
  _attach(harness);
  await harness.show(tester, size: const Size(1200, 1600));
  await settle(tester);
  final agent = workspace.agents.firstWhere((agent) => agent.id == id);
  unawaited(
    showModal(
      tester.element(find.byType(Sidebar).first),
      (context, close) =>
          AgentSettingsForm(agent: agent, close: close, showExtensions: true),
    ),
  );
  await settle(tester);
  return harness;
}

Iterable<UpdateAgent> _updates(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasUpdateAgent()) message.updateAgent,
];

/// The Browser row's switch.
Finder _browserSwitch() => find.descendant(
  of: find.ancestor(
    of: find.text('Browser'),
    matching: find.byWidgetPredicate(
      (widget) => widget.runtimeType.toString() == '_ExtensionSettings',
    ),
  ),
  matching: find.byWidgetPredicate(
    (widget) => widget.runtimeType.toString() == 'Toggle',
  ),
);

void main() {
  test("the sites an agent's signed in to, as a person reads them", () {
    expect(sitesText(['x.com']), 'x.com');
    expect(sitesText(['x.com', 'linkedin.com']), 'x.com and linkedin.com');
    expect(
      sitesText(['x.com', 'linkedin.com', 'github.com']),
      'x.com, linkedin.com and github.com',
    );
    expect(
      sitesText(['a.com', 'b.com', 'c.com', 'd.com']),
      'a.com, b.com, '
      'c.com and d.com',
    );
    expect(
      sitesText(['a.com', 'b.com', 'c.com', 'd.com', 'e.com', 'f.com']),
      'a.com, b.com, c.com, d.com and 2 more',
    );
    expect(signedInText(const []), 'Not signed in anywhere');
    expect(siteName('https://www.linkedin.com'), 'linkedin.com');
  });

  test('the browser is never for an admin, a pipe or a report reader', () {
    expect(browserRefusal(_agent('a', 'A')), isNull);
    expect(
      browserRefusal(_agent('a', 'A', admin: true)),
      "Admin agents can't use the browser.",
    );
    expect(
      browserRefusal(_agent('a', 'A', quarantined: true)),
      "Agents limited to a pipe can't use the browser.",
    );
    expect(
      browserRefusal(_agent('a', 'A', readsReports: true)),
      "Agents that read reports can't use the browser.",
    );
    // No contained mode any more: a limit keeps Ask, never the browser.
    expect(containedKeeps(Agent(id: 'june'), 'browser'), isTrue);
    expect(
      containedKeeps(Agent(id: 'june', quarantined: true), 'browser'),
      isFalse,
    );
    expect(
      containedKeeps(Agent(id: 'june', readsReports: true), 'browser'),
      isFalse,
    );
  });

  group('Agent settings', () {
    testWidgets('Browser is an ordinary extension: its line names the agent, '
        'and turning it on is saved with Save; no errands', (tester) async {
      final harness = await _agentSettings(tester, _workspace(), 'june');
      expect(
        find.text(
          "Lets June use websites in Claktory's browser, signing in with "
          'your logins from Secrets.',
        ),
        findsOneWidget,
      );
      expect(find.textContaining('errand'), findsNothing);
      expect(find.textContaining('Errand'), findsNothing);
      await tester.ensureVisible(_browserSwitch());
      await tester.tap(_browserSwitch());
      await settle(tester);
      expect(_updates(harness), isEmpty, reason: 'only with Save');
      await tester.tap(find.widgetWithText(Button, 'Save'));
      await settle(tester);
      final change = _updates(harness).single.extension_8;
      expect(change.id, 'browser');
      expect(change.enabled, isTrue);
    });

    for (final (what, agent, why) in [
      (
        'an admin',
        _agent('june', 'June', admin: true),
        "Admin agents can't use the browser.",
      ),
      (
        'a pipe-limited agent',
        _agent('june', 'June', quarantined: true),
        "Agents limited to a pipe can't use the browser.",
      ),
      (
        'a report reader',
        _agent('june', 'June', readsReports: true),
        "Agents that read reports can't use the browser.",
      ),
    ]) {
      testWidgets('for $what it says why, and the switch is off', (
        tester,
      ) async {
        final harness = await _agentSettings(
          tester,
          _workspace(agents: [_agent('rivet', 'Rivet', admin: true), agent]),
          'june',
        );
        expect(find.text(why), findsOneWidget);
        await tester.ensureVisible(_browserSwitch());
        await tester.tap(_browserSwitch());
        await settle(tester);
        await tester.tap(find.widgetWithText(Button, 'Save'));
        await settle(tester);
        expect(
          _updates(harness).where((update) => update.hasExtension_8()),
          isEmpty,
        );
      });
    }

    testWidgets('turned off, Save asks first, saying where it signs out; '
        'Cancel there keeps the form as it is', (tester) async {
      final harness = await _agentSettings(
        tester,
        _workspace(browsers: [_yukakos]),
        'yukako',
      );
      await tester.ensureVisible(_browserSwitch());
      await tester.tap(_browserSwitch());
      await settle(tester);
      expect(find.text("Turn off Yukako's browser?"), findsNothing);
      await tester.tap(find.widgetWithText(Button, 'Save'));
      await settle(tester);
      expect(find.text("Turn off Yukako's browser?"), findsOneWidget);
      expect(
        find.text(
          'Yukako is signed out of linkedin.com and x.com, and the '
          "browser's data is deleted. Your logins stay in Secrets.",
        ),
        findsOneWidget,
      );
      // Cancel there sends nothing, and the form stays open to change.
      await tester.tap(find.text('Cancel').last);
      await settle(tester);
      expect(_updates(harness), isEmpty);
      expect(find.text("Turn off Yukako's browser?"), findsNothing);
      expect(find.byType(AgentSettingsForm), findsOneWidget);

      await tester.tap(find.widgetWithText(Button, 'Save'));
      await settle(tester);
      await tester.tap(find.text('Turn off').last);
      await settle(tester);
      final change = _updates(harness).single;
      expect(change.agentId, 'yukako');
      expect(change.extension_8.id, 'browser');
      expect(change.extension_8.enabled, isFalse);
    });

    testWidgets("a refusal shows the backend's words", (tester) async {
      final harness = await _agentSettings(tester, _workspace(), 'yukako');
      harness.backend.refuse = "Claktory couldn't turn off Yukako's browser.";
      await tester.ensureVisible(_browserSwitch());
      await tester.tap(_browserSwitch());
      await settle(tester);
      await tester.tap(find.widgetWithText(Button, 'Save'));
      await settle(tester);
      expect(
        find.text(
          "Yukako's browser data is deleted. Your logins stay in "
          'Secrets.',
        ),
        findsOneWidget,
      );
      await tester.tap(find.text('Turn off').last);
      await settle(tester);
      expect(
        find.textContaining("Claktory couldn't turn off Yukako's browser."),
        findsOneWidget,
      );
    });
  });

  group("Settings → Claktory's browser", () {
    testWidgets('ready, with Chrome as a number', (tester) async {
      await _settings(
        tester,
        _workspace(
          browser: AgentBrowser(
            state: AgentBrowserState.AGENT_BROWSER_STATE_WARM,
            chromeVersion: 'Chrome/142.0.7444.59',
          ),
        ),
      );
      expect(find.text('Ready · Chrome 142'), findsOneWidget);
      expect(find.text('Close browser'), findsNothing);
      expect(find.textContaining('errand'), findsNothing);
    });

    testWidgets('not set up yet: an agent sets it up, or you do, folded', (
      tester,
    ) async {
      await _settings(
        tester,
        _workspace(
          browser: AgentBrowser(
            state: AgentBrowserState.AGENT_BROWSER_STATE_UNAVAILABLE,
            setup: AgentBrowserSetup(
              problem: "Apple's container tool isn't installed on this Mac.",
              command: 'brew install container',
            ),
          ),
        ),
      );
      expect(
        find.text(
          'Not set up yet. An agent sets it up the first time it needs it.',
        ),
        findsOneWidget,
      );
      expect(find.textContaining('brew install container'), findsNothing);
      await tester.tap(find.text('Set it up yourself'));
      await tester.pump();
      expect(find.textContaining('brew install container'), findsOneWidget);
      expect(
        find.text('Run this once in a terminal on this computer.'),
        findsOneWidget,
      );
      // Said once already, above.
      expect(
        find.text("Apple's container tool isn't installed on this Mac."),
        findsNothing,
      );
    });

    testWidgets("a computer that can't run it, named", (tester) async {
      await _settings(
        tester,
        _workspace(
          browser: AgentBrowser(
            state: AgentBrowserState.AGENT_BROWSER_STATE_UNAVAILABLE,
            setup: AgentBrowserSetup(unsupported: true),
          ),
        )..settings = Settings(name: 'Studio'),
      );
      expect(
        find.text(
          "Studio can't run it. It needs a Mac with Apple silicon and "
          'macOS 26 or later.',
        ),
        findsOneWidget,
      );
      expect(find.text('Set it up yourself'), findsNothing);
    });

    testWidgets('a row per agent with the browser, saying where it signs in', (
      tester,
    ) async {
      await _settings(tester, _workspace(browsers: [_yukakos]));
      expect(find.text('Yukako'), findsOneWidget);
      expect(find.text('Signed in to linkedin.com and x.com'), findsOneWidget);
      expect(find.text('Koichi'), findsOneWidget);
      expect(find.text('Not signed in anywhere'), findsOneWidget);
      // Neither the admin nor an agent without it.
      expect(find.text('Rivet'), findsNothing);
      expect(find.text('June'), findsNothing);
      // The one shared list and the errands are gone.
      expect(find.text('Signed in to'), findsNothing);
      expect(find.textContaining('Errands'), findsNothing);
    });

    testWidgets('none yet: how one gets it', (tester) async {
      await _settings(tester, _workspace(agents: [_agent('june', 'June')]));
      expect(
        find.text(
          'No agent has the browser yet. Ask Rivet to give it to one, or '
          "turn on Browser in an agent's settings.",
        ),
        findsOneWidget,
      );
    });

    testWidgets('⋯ signs out of a site, at once', (tester) async {
      final (_, s) = await _settings(tester, _workspace(browsers: [_yukakos]));
      await tester.tap(find.bySemanticsLabel("Yukako's browser"));
      await settle(tester);
      expect(find.text('Sign out of linkedin.com'), findsOneWidget);
      expect(find.text('Sign out of x.com'), findsOneWidget);
      expect(find.text('Delete browser data…'), findsOneWidget);
      expect(find.text('Turn off browser…'), findsOneWidget);
      await tester.tap(find.text('Sign out of linkedin.com'));
      await settle(tester);
      final out = s.sent.single.signOutAgentBrowserAccount;
      expect(out.agentId, 'yukako');
      expect(out.site, 'https://www.linkedin.com');
    });

    testWidgets('⋯ deletes its data once confirmed', (tester) async {
      final (_, s) = await _settings(tester, _workspace(browsers: [_yukakos]));
      await tester.tap(find.bySemanticsLabel("Yukako's browser"));
      await settle(tester);
      await tester.tap(find.text('Delete browser data…'));
      await settle(tester);
      expect(find.text("Delete Yukako's browser data?"), findsOneWidget);
      expect(s.sent, isEmpty);
      await tester.tap(find.text('Delete').last);
      await settle(tester);
      expect(s.sent.single.deleteAgentBrowserData.agentId, 'yukako');
    });

    testWidgets('⋯ turns its browser off once confirmed; a refusal shows', (
      tester,
    ) async {
      final (harness, _) = await _settings(
        tester,
        _workspace(browsers: [_yukakos]),
      );
      await tester.tap(find.bySemanticsLabel("Yukako's browser"));
      await settle(tester);
      await tester.tap(find.text('Turn off browser…'));
      await settle(tester);
      expect(find.text("Turn off Yukako's browser?"), findsOneWidget);
      harness.backend.refuse = 'Yukako is busy.';
      await tester.tap(find.text('Turn off').last);
      await settle(tester);
      final change = _updates(harness).single;
      expect(change.agentId, 'yukako');
      expect(change.extension_8.enabled, isFalse);
      expect(find.textContaining('Yukako is busy.'), findsOneWidget);
    });

    testWidgets("the time zone and languages say when they're this device's", (
      tester,
    ) async {
      await _settings(
        tester,
        _workspace(
          browser: AgentBrowser(
            state: AgentBrowserState.AGENT_BROWSER_STATE_OFF,
            timeZone: 'Europe/Rome',
            languages: ['it-IT', 'en-US'],
          ),
        ),
      );
      await tester.pump();
      expect(
        find.text("This device's: Europe/Rome · it-IT, en-US"),
        findsOneWidget,
      );
      expect(
        find.text(
          'Sites see these. They match this device, so the browser looks '
          'like yours.',
        ),
        findsOneWidget,
      );
      await tester.tap(find.text('Change…'));
      await settle(tester);
      expect(find.text('Time zone and languages'), findsWidgets);
    });

    testWidgets('other ones are just what sites see', (tester) async {
      await _settings(
        tester,
        _workspace(
          browser: AgentBrowser(
            state: AgentBrowserState.AGENT_BROWSER_STATE_OFF,
            languages: ['en-GB'],
          ),
        ),
      );
      await tester.pump();
      expect(find.text("This computer's time zone · en-GB"), findsOneWidget);
      expect(find.text('Sites see these.'), findsOneWidget);
    });

    testWidgets('the browser log, everyone’s or one agent’s', (tester) async {
      final (harness, _) = await _settings(
        tester,
        _workspace(browsers: [_yukakos]),
      );
      harness.backend.errandLog = ErrandLog(
        entries: [
          ErrandLogEntry(
            atMs: Int64(1764000000000),
            agentId: 'yukako',
            origin: 'https://x.com',
            path: '/compose/post',
            action: 'Clicked',
            target: 'button "Post"',
          ),
        ],
      );
      await tester.tap(find.text('Open'));
      await settle(tester);
      await tester.pump(); // the log, answered after the first frame
      expect(
        harness.backend.sent.last.listErrandLog.agentId,
        '',
        reason: "everyone's first",
      );
      expect(find.textContaining('Yukako: Clicked'), findsOneWidget);
      expect(find.textContaining('x.com/compose/post'), findsOneWidget);
      await tester.tap(find.text('All agents'));
      await settle(tester);
      await tester.tap(find.text('Yukako').last);
      await settle(tester);
      expect(harness.backend.sent.last.listErrandLog.agentId, 'yukako');
    });
  });
}
