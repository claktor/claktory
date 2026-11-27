// #435 W7 (Ghiaccio 41158): where Claktory's browser may fill a secret in
// for an agent's errand (its sites, exact https origins), giving it to an
// agent to fill in only, and how such a grant and a fill read.
import 'package:factory_app/backend.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/secrets.dart';
import 'package:factory_app/update.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

SecretInfo _shop({
  List<String> sites = const [],
  List<SecretGrant> grants = const [],
  bool askEachTime = false,
}) => SecretInfo(
  id: 's1',
  name: 'Shop login',
  description: 'For buying parts',
  variables: [SecretVariable(name: 'SHOP_PASSWORD', hint: 'x9')],
  sites: sites,
  grants: grants,
  askEachTime: askEachTime,
);

Workspace _workspace(SecretInfo secret) => Workspace(
  agents: [
    Agent(id: 'june', name: 'June', color: 'pink', modelAvailable: true),
    Agent(id: 'mike', name: 'Mike', color: 'blue', modelAvailable: true),
  ],
  selectedAgentId: 'june',
  settings: Settings(userName: 'Matt'),
  vault: VaultState(exists: true, unlocked: true, secrets: [secret]),
);

Finder _labeled(String label) => find.byWidgetPredicate(
  (w) => w is Semantics && w.properties.label == label,
);

Future<Harness> _show(WidgetTester tester, SecretInfo secret) async {
  final harness = Harness(_workspace(secret));
  await _showWith(tester, harness);
  return harness;
}

Future<void> _open(WidgetTester tester) async {
  await tester.tap(_labeled('Shop login'));
  await pumpFrames(tester);
}

List<ClientMessage> _sent(Harness harness, bool Function(ClientMessage) of) => [
  for (final message in harness.backend.sent)
    if (of(message)) message,
];

Finder get _siteField => find.descendant(
  of: _labeled('A site to fill it in on'),
  matching: find.byType(EditableText),
);

void main() {
  testWidgets("a secret with no sites says it's filled in nowhere; a site "
      'added is sent, and a refusal shows as the backend says it', (
    tester,
  ) async {
    final harness = await _show(tester, _shop());
    await _open(tester);
    expect(
      find.textContaining("Claktory's browser doesn't fill it in anywhere"),
      findsOneWidget,
    );
    expect(find.text('Let errands fill it in…'), findsNothing);

    harness.backend.onSend = (sent) {
      if (sent.hasSetSecretSites() && sent.setSecretSites.sites.first == 'x') {
        throw const RequestFailure(
          'https://*.x: give each site exactly, such as '
          'https://shop.example, not a pattern.',
        );
      }
    };
    await tester.enterText(_siteField, 'x');
    await pumpFrames(tester);
    await tester.ensureVisible(find.text('Add site'));
    await tester.tap(find.text('Add site'));
    await pumpFrames(tester);
    expect(find.textContaining('not a pattern'), findsOneWidget);

    await tester.enterText(_siteField, 'https://shop.example');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await pumpFrames(tester);
    final set = _sent(harness, (m) => m.hasSetSecretSites()).last;
    expect(set.setSecretSites.id, 's1');
    expect(set.setSecretSites.sites, ['https://shop.example']);
    expect(
      tester.widget<EditableText>(_siteField).controller.text,
      isEmpty,
      reason: 'added, the field empties',
    );
  });

  testWidgets('sites are listed with Remove; asking each time, it says '
      "they're filled nowhere", (tester) async {
    final harness = await _show(
      tester,
      _shop(
        sites: ['https://store.example', 'https://parts.example'],
        askEachTime: true,
      ),
    );
    await _open(tester);
    expect(find.text('https://store.example'), findsOneWidget);
    expect(find.text('https://parts.example'), findsOneWidget);
    expect(
      find.textContaining("It asks each time, so it isn't filled in anywhere"),
      findsOneWidget,
    );
    await tester.ensureVisible(find.text('Remove').first);
    await tester.tap(find.text('Remove').first);
    await pumpFrames(tester);
    expect(
      _sent(harness, (m) => m.hasSetSecretSites()).single.setSecretSites.sites,
      ['https://parts.example'],
    );
  });

  testWidgets('Let errands fill it in… gives it for errands alone, an agent '
      'that has it in full can be picked; the grant says so', (tester) async {
    final harness = await _show(
      tester,
      _shop(
        sites: ['https://shop.example'],
        grants: [SecretGrant(agentId: 'june')],
      ),
    );
    await _open(tester);
    await tester.tap(find.text('Let errands fill it in…'));
    await pumpFrames(tester);
    expect(find.text('Whose errands may fill in Shop login?'), findsOneWidget);
    await tester.tap(_labeled('June').last);
    await pumpFrames(tester);
    final given = _sent(harness, (m) => m.hasGrantSecret()).single.grantSecret;
    expect((given.agentId, given.fillOnly), ('june', true));

    // As the backend has it now: June fills it in only.
    await harness.apply(
      tester,
      _workspace(
        _shop(
          sites: ['https://shop.example'],
          grants: [SecretGrant(agentId: 'june', fillOnly: true)],
        ),
      ),
    );
    await pumpFrames(tester);
    expect(find.textContaining('June (fill in only)'), findsOneWidget);
    expect(
      find.textContaining(
        'can fill it in on its sites only',
        findRichText: true,
      ),
      findsOneWidget,
    );

    // Give to… gives it in full again, June among those to pick.
    await tester.tap(find.text('Give to…'));
    await pumpFrames(tester);
    await tester.tap(_labeled('June').last);
    await pumpFrames(tester);
    expect(
      _sent(harness, (m) => m.hasGrantSecret()).last.grantSecret.fillOnly,
      isFalse,
    );
  });

  testWidgets('a fill reads as one: filled in by whom, on which site', (
    tester,
  ) async {
    final harness = Harness(_workspace(_shop()));
    harness.backend.secretUses = [
      SecretUse(
        timeMs: Int64(DateTime(2026, 11, 26, 10, 4).millisecondsSinceEpoch),
        agentId: 'june',
        kind: 'fill',
        detail: 'filled into a password field on https://shop.example',
      ),
    ];
    await _showWith(tester, harness);
    await _open(tester);
    expect(
      find.textContaining(
        'Filled in by June on https://shop.example',
        findRichText: true,
      ),
      findsOneWidget,
    );
  });

  testWidgets('a new secret can have its sites from the start', (tester) async {
    final harness = Harness(
      Workspace(
        agents: [Agent(id: 'june', name: 'June', modelAvailable: true)],
        vault: VaultState(exists: true, unlocked: true),
      ),
    );
    await _showWith(tester, harness);
    await tester.tap(find.text('Add a secret…'));
    await pumpFrames(tester);
    final fields = find.byType(EditableText);
    await tester.enterText(fields.at(0), 'Shop login');
    await tester.enterText(fields.at(2), 'SHOP_PASSWORD');
    await tester.enterText(fields.at(3), 'hunter22');
    await tester.enterText(
      fields.at(4),
      'https://shop.example\nhttps://parts.example',
    );
    await tester.tap(find.text('Add'));
    await pumpFrames(tester);
    final made = _sent(harness, (m) => m.hasCreateSecret()).single.createSecret;
    expect(made.sites, ['https://shop.example', 'https://parts.example']);
  });
}

/// [harness]'s Settings → Secrets, as [_show] does.
Future<void> _showWith(WidgetTester tester, Harness harness) async {
  tester.view.physicalSize = const Size(1000, 1400);
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
        child: Overlay(
          initialEntries: [
            OverlayEntry(
              builder: (context) => const SingleChildScrollView(
                padding: EdgeInsets.all(24),
                child: SecretsSettings(),
              ),
            ),
          ],
        ),
      ),
    ),
  );
  await tester.pump();
}
