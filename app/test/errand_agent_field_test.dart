// "Use for errands" in Agent settings (#435; Bruno 49824, Lucy 49837,
// Trish 49866, Fugo 49863): the person contains an agent for errands in
// place, at once, after a question that says what changes, and lets it out
// the same way.
import 'dart:async';

import 'package:factory_app/controls.dart' show Button, showModal;
import 'package:factory_app/forms.dart' show AgentSettingsForm;
import 'package:factory_app/pipe_reports.dart' show containedKeeps;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart' show Sidebar;
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace({
  bool browser = true,
  bool contained = false,
  bool admin = false,
  bool errandsOn = false,
}) => Workspace(
  agents: [
    Agent(id: 'bruno', name: 'Bruno', color: 'blue', modelAvailable: true),
    Agent(
      id: 'june',
      name: 'June',
      color: 'green',
      leadId: 'bruno',
      modelAvailable: true,
      contained: contained,
      extensions: [
        AgentExtension(id: 'web', enabled: true),
        AgentExtension(id: 'ask', enabled: true),
        AgentExtension(id: 'errands', enabled: errandsOn),
        AgentExtension(
          id: 'agents',
          enabled: true,
          options: {if (admin) 'admin': OptionValue(on: true)}.entries,
        ),
      ],
    ),
  ],
  extensions: [
    Extension(id: 'agents', name: 'Messaging', icon: 'users'),
    Extension(id: 'web', name: 'Web', icon: 'globe'),
    Extension(id: 'ask', name: 'Ask', icon: 'circle-help'),
    Extension(id: 'errands', name: 'Errands', icon: 'globe'),
  ],
  agentBrowser: browser
      ? AgentBrowser(state: AgentBrowserState.AGENT_BROWSER_STATE_OFF)
      : null,
  selectedAgentId: 'bruno',
);

Future<Harness> _open(
  WidgetTester tester,
  Workspace workspace, {
  Size size = const Size(1200, 1600),
}) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final harness = Harness(workspace);
  await harness.show(tester, size: size);
  await settle(tester);
  final june = workspace.agents.firstWhere((agent) => agent.id == 'june');
  unawaited(
    showModal(
      tester.element(find.byType(Sidebar).first),
      (context, close) => AgentSettingsForm(agent: june, close: close),
    ),
  );
  await settle(tester);
  return harness;
}

Iterable<SetAgentContained> _sent(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasSetAgentContained()) message.setAgentContained,
];

void main() {
  testWidgets('nowhere without a browser', (tester) async {
    await _open(tester, _workspace(browser: false));
    expect(find.text('Use for errands…'), findsNothing);
  });

  testWidgets('not for an admin agent', (tester) async {
    await _open(tester, _workspace(admin: true));
    expect(find.text('Use for errands…'), findsNothing);
  });

  testWidgets('Use for errands asks first, says what changes, then sends it '
      'at once and unfolds Errands, the next step', (tester) async {
    final harness = await _open(tester, _workspace());
    expect(
      find.text(
        "June can do things on websites for you in Claktory's browser.",
      ),
      findsOneWidget,
    );
    await tester.ensureVisible(find.text('Use for errands…'));
    await settle(tester);
    await tester.tap(find.text('Use for errands…'));
    await settle(tester);
    expect(find.text('Use June for errands?'), findsOneWidget);
    expect(
      find.text(
        "June will do things on websites for you in Claktory's browser. "
        "While it runs errands, it can't run commands, search the web, use "
        'secrets or apps, or message anyone but Bruno. It works only in a '
        'folder of its own, with its other extensions off, and the browser '
        'fills in secrets only on their own sites.',
      ),
      findsOneWidget,
    );
    expect(_sent(harness), isEmpty, reason: 'nothing before the answer');
    // Cancel changes nothing (the question's, before the form's own).
    await tester.tap(find.widgetWithText(Button, 'Cancel').first);
    await settle(tester);
    expect(find.text('Use June for errands?'), findsNothing);
    expect(_sent(harness), isEmpty);

    harness.backend.onSend = (message) {
      if (!message.hasSetAgentContained()) return;
      harness.backend.workspace.apply(_workspace(contained: true));
    };
    await tester.tap(find.text('Use for errands…'));
    await settle(tester);
    await tester.tap(find.widgetWithText(Button, 'Use for errands'));
    await settle(tester);
    expect(_sent(harness).single.agentId, 'june');
    expect(_sent(harness).single.contained, isTrue);
    // Never through Save: the form has nothing of its own to save.
    expect(harness.backend.sent.where((m) => m.hasUpdateAgent()), isEmpty);
    // Contained: its limit's note, and its extensions open at Errands.
    expect(
      find.text(
        "June runs errands, so it can't run commands, search the web, use "
        'secrets or message anyone but Bruno.',
      ),
      findsOneWidget,
    );
    expect(find.text('Change…'), findsNothing, reason: 'unfolded');
    expect(find.text('Errands'), findsWidgets);
  });

  testWidgets('Stop errands… asks, says what it gets back, and sends false', (
    tester,
  ) async {
    final harness = await _open(tester, _workspace(contained: true));
    await tester.ensureVisible(find.text('Stop errands…'));
    await settle(tester);
    await tester.tap(find.text('Stop errands…'));
    await settle(tester);
    expect(find.text('Stop using June for errands?'), findsOneWidget);
    expect(
      find.text(
        'Its errand ends, Errands turns off in its extensions, and June gets '
        'back commands, the web, secrets, messaging, apps and its other '
        'extensions. Its conversation still holds words from pages it read, '
        'so each use of a secret waits for your OK until you clear its '
        'conversation.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.widgetWithText(Button, 'Stop errands'));
    await settle(tester);
    expect(_sent(harness).single.contained, isFalse);
  });

  testWidgets('a refusal says so in the question, which stays', (tester) async {
    final harness = await _open(tester, _workspace());
    harness.backend.refuse = 'June is already limited to a pipe.';
    await tester.ensureVisible(find.text('Use for errands…'));
    await settle(tester);
    await tester.tap(find.text('Use for errands…'));
    await settle(tester);
    await tester.tap(find.widgetWithText(Button, 'Use for errands'));
    await settle(tester);
    expect(
      find.text(
        "Couldn't change errands for June. June is already limited to a pipe.",
      ),
      findsOneWidget,
    );
    expect(find.text('Use June for errands?'), findsOneWidget);
  });

  testWidgets('its secrets stay given, each saying what it can still do '
      'with it while it runs errands (row 1711, Ghiaccio 50734, 51137)', (
    tester,
  ) async {
    const fills =
        "While June runs errands, it can fill this in only on the secret's "
        'sites.';
    const cant =
        "While June runs errands, it can't use this. To let it fill this in, "
        'add sites to the secret and turn off Ask each time.';
    Workspace withSecrets({required bool contained}) =>
        _workspace(contained: contained, errandsOn: contained)
          ..vault = VaultState(
            exists: true,
            unlocked: true,
            secrets: [
              SecretInfo(
                id: 'gh',
                name: 'GitHub',
                grants: [SecretGrant(agentId: 'june')],
                warnings: [
                  fills,
                  "Bruno can't use it while it's limited to a "
                      'pipe.',
                ],
              ),
              SecretInfo(
                id: 'aws',
                name: 'AWS',
                grants: [SecretGrant(agentId: 'june')],
                warnings: [cant],
              ),
            ],
          );
    await _open(tester, withSecrets(contained: true));
    expect(find.text(fills), findsOneWidget);
    expect(find.text(cant), findsOneWidget);
    // Only its own lines.
    expect(find.textContaining('Bruno can'), findsNothing);
  });

  test(
    'Errands is only a contained agent\'s, and all it keeps besides Ask',
    () {
      final free = Agent(id: 'june');
      expect(containedKeeps(free, 'errands'), isFalse);
      expect(containedKeeps(free, 'web'), isTrue);
      final contained = Agent(id: 'june', contained: true);
      expect(containedKeeps(contained, 'errands'), isTrue);
      expect(containedKeeps(contained, 'ask'), isTrue);
      expect(containedKeeps(contained, 'web'), isFalse);
      expect(containedKeeps(contained, 'agents'), isFalse);
    },
  );
}
