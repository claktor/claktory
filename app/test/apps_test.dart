import 'package:factory_app/apps.dart';
import 'package:factory_app/controls.dart' show Toggle;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/secrets.dart';
import 'package:factory_app/update.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

AppInfo _telegram({
  AppState state = AppState.APP_STATE_RUNNING,
  bool tokenSet = true,
  List<AppGrant> grants = const [],
  List<AppSubscription> subscriptions = const [],
}) => AppInfo(
  id: 'telegram',
  name: 'Telegram',
  version: '1.0.0',
  publisher: 'claktory',
  tier: AppTier.APP_TIER_LOCAL,
  publisherKey: 'ed25519:AbCdEf0123456789',
  icon: 'send',
  state: state,
  description: 'Talk to your agents from Telegram.',
  hosts: ['api.telegram.org'],
  scopes: [
    AppScopeSpec(
      id: 'send',
      label: 'Send you Telegram messages',
      tools: ['telegram_send'],
    ),
    AppScopeSpec(
      id: 'read',
      label: 'Read your chat with the bot',
      tools: ['telegram_history'],
    ),
  ],
  events: [
    AppEventKind(
      kind: 'message',
      label: 'A message you send the bot',
      scope: 'read',
    ),
  ],
  setup: [
    AppSetupItem(
      kind: AppSetupKind.APP_SETUP_KIND_SECRET,
      name: 'bot_token',
      label: 'Bot token',
      description: 'From @BotFather.',
      done: tokenSet,
    ),
  ],
  grants: grants,
  subscriptions: subscriptions,
);

Workspace _workspace({List<AppInfo>? apps, VaultState? vault}) => Workspace(
  agents: [
    Agent(id: 'june', name: 'June', color: 'pink', modelAvailable: true),
    Agent(id: 'rivet', name: 'Rivet', color: 'blue', modelAvailable: true),
    Agent(id: 'lea', name: 'Lea', color: 'teal', modelAvailable: true),
  ],
  selectedAgentId: 'june',
  settings: Settings(userName: 'Matt'),
  apps: apps ?? [_telegram()],
  appsAvailable: true,
  vault: vault,
);

Finder _labeled(String label) => find.byWidgetPredicate(
  (w) => w is Semantics && w.properties.label == label,
);

Future<void> _show(WidgetTester tester, Harness harness, Widget child) async {
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
              builder: (context) => SingleChildScrollView(
                padding: const EdgeInsets.all(24),
                child: child,
              ),
            ),
          ],
        ),
      ),
    ),
  );
  await tester.pump();
}

List<ClientMessage> _sent(Harness harness, bool Function(ClientMessage) of) => [
  for (final message in harness.backend.sent)
    if (of(message)) message,
];

void main() {
  tearDown(() => appFilePicker = null);

  testWidgets('an app shows how it runs and to whom it is given; its switch '
      'turns it off', (tester) async {
    final harness = Harness(
      _workspace(
        apps: [
          _telegram(
            grants: [
              AppGrant(agentId: 'june', scopes: ['send', 'read']),
            ],
          )..status = 'To link your Telegram, send /start 4821 to the bot.',
        ],
      ),
    );
    await _show(tester, harness, const AppsSettings());
    expect(find.text('Telegram'), findsOneWidget);
    expect(find.text('Running · Given to June'), findsOneWidget);
    expect(
      find.text('To link your Telegram, send /start 4821 to the bot.'),
      findsOneWidget,
    );
    final toggle = find.byWidgetPredicate(
      (w) => w is Toggle && w.label == 'Run Telegram',
    );
    expect(tester.widget<Toggle>(toggle).value, isTrue);
    await tester.tap(toggle);
    await pumpFrames(tester);
    final running = _sent(harness, (m) => m.hasSetAppRunning()).single;
    expect(running.setAppRunning.id, 'telegram');
    expect(running.setAppRunning.running, isFalse);
  });

  testWidgets('a paused app says why in red, and its switch starts it', (
    tester,
  ) async {
    final harness = Harness(
      _workspace(
        apps: [
          _telegram(state: AppState.APP_STATE_PAUSED_CRASHED)
            ..stateDetail = 'It stopped 3 times in 10 minutes: out of memory.',
        ],
      ),
    );
    await _show(tester, harness, const AppsSettings());
    expect(
      find.text('It stopped 3 times in 10 minutes: out of memory.'),
      findsOneWidget,
    );
    expect(find.textContaining('Paused'), findsOneWidget);
    await tester.tap(
      find.byWidgetPredicate((w) => w is Toggle && w.label == 'Run Telegram'),
    );
    await pumpFrames(tester);
    expect(
      _sent(harness, (m) => m.hasSetAppRunning()).single.setAppRunning.running,
      isTrue,
    );
  });

  testWidgets('setup that waits shows with the app; its secret is pasted '
      'for the app alone', (tester) async {
    final harness = Harness(
      _workspace(
        apps: [
          _telegram(state: AppState.APP_STATE_NEEDS_SETUP, tokenSet: false),
        ],
      ),
    );
    await _show(tester, harness, const AppsSettings());
    expect(find.textContaining('Needs setup'), findsOneWidget);
    expect(find.text('Bot token'), findsOneWidget);
    expect(find.text('Not set yet'), findsOneWidget);
    await tester.tap(find.text('Set…'));
    await pumpFrames(tester);
    expect(find.text('Telegram: Bot token'), findsOneWidget);
    expect(find.text('From @BotFather.'), findsOneWidget);
    await tester.enterText(find.byType(EditableText), '123:abc');
    await tester.tap(find.text('Save'));
    await pumpFrames(tester);
    final set = _sent(harness, (m) => m.hasSetAppSecret()).single.setAppSecret;
    expect(set.appId, 'telegram');
    expect(set.name, 'bot_token');
    expect(set.value, '123:abc');
  });

  test("only Claktory's and verified publishers' names stand as who made "
      'an app', () {
    expect(publisherLine(AppTier.APP_TIER_CLAKTORY, 'claktory'), 'By claktory');
    expect(publisherLine(AppTier.APP_TIER_VERIFIED, 'Acme'), 'By Acme');
    for (final tier in [
      AppTier.APP_TIER_COMMUNITY,
      AppTier.APP_TIER_LOCAL,
      AppTier.APP_TIER_UNSPECIFIED,
    ]) {
      expect(
        publisherLine(tier, 'claktory'),
        "Says it's by claktory",
        reason: '$tier',
      );
    }
  });

  testWidgets('installing from a file shows what it can reach, then installs '
      'the file reviewed', (tester) async {
    final harness = Harness(_workspace(apps: []));
    harness.backend.appPermissions = AppPermissions(
      sha256: 'ab12',
      id: 'telegram',
      name: 'Telegram',
      version: '1.0.0',
      publisher: 'claktory',
      tier: AppTier.APP_TIER_LOCAL,
      publisherKey: 'ed25519:AbCdEf0123456789',
      description: 'Talk to your agents from Telegram.',
      hosts: ['api.telegram.org'],
      scopes: _telegram().scopes,
      events: _telegram().events,
      secrets: [AppSetupItem(name: 'bot_token', label: 'Bot token')],
      memoryMb: 64,
      storageMb: 16,
    );
    appFilePicker = () async => [1, 2, 3];
    await _show(tester, harness, const AppsSettings());
    expect(find.text('No apps yet.'), findsOneWidget);
    await tester.tap(find.text('Install from a file…'));
    await pumpFrames(tester);
    final review = _sent(
      harness,
      (m) => m.hasInstallLocalApp(),
    ).single.installLocalApp;
    expect(review.content, [1, 2, 3]);
    expect(review.approve, isFalse);
    expect(find.text('Install Telegram?'), findsOneWidget);
    expect(find.textContaining("Claktory hasn't reviewed"), findsOneWidget);
    // Unchecked, its publisher is only what it says (even "claktory").
    expect(find.textContaining("Says it's by claktory"), findsOneWidget);
    expect(find.textContaining('By claktory'), findsNothing);
    expect(find.text('api.telegram.org'), findsOneWidget);
    expect(find.text('Send you Telegram messages'), findsOneWidget);
    expect(find.text('A message you send the bot'), findsOneWidget);
    expect(find.text('Bot token'), findsOneWidget);
    expect(
      find.text('Up to 64 MB of memory and 16 MB of storage'),
      findsOneWidget,
    );
    expect(find.text('SHA-256 ab12'), findsOneWidget);
    await tester.tap(find.text('Install'));
    await pumpFrames(tester);
    final install = _sent(harness, (m) => m.hasInstallLocalApp()).last;
    expect(install.installLocalApp.approve, isTrue);
    expect(install.installLocalApp.sha256, 'ab12');
    expect(install.installLocalApp.content, [1, 2, 3]);
    expect(find.text('Install Telegram?'), findsNothing);
  });

  testWidgets("a file Claktory can't install says why", (tester) async {
    final harness = Harness(_workspace(apps: []))
      ..backend.refuse = 'The signature doesn’t match the file.';
    appFilePicker = () async => [1];
    await _show(tester, harness, const AppsSettings());
    await tester.tap(find.text('Install from a file…'));
    await pumpFrames(tester);
    expect(find.text('The signature doesn’t match the file.'), findsOneWidget);
  });

  testWidgets('giving an app to an agent: all of it to start with, and the '
      'events it hears', (tester) async {
    final harness = Harness(
      _workspace(
        apps: [
          _telegram(
            grants: [
              AppGrant(agentId: 'rivet', scopes: ['read']),
            ],
            subscriptions: [
              AppSubscription(eventKind: 'message', agentIds: ['rivet']),
            ],
          ),
        ],
      ),
    );
    await _show(tester, harness, const AppsSettings());
    await tester.tap(_labeled('Telegram'));
    await pumpFrames(tester);
    expect(find.text('Rivet'), findsOneWidget);
    expect(find.text('Read your chat with the bot'), findsOneWidget);
    expect(find.text('Hears: a message you send the bot'), findsOneWidget);
    await tester.tap(find.text('Give to an agent…'));
    await pumpFrames(tester);
    expect(find.text('Give Telegram to an agent'), findsOneWidget);
    // June is the one without it.
    expect(find.text('June can'), findsOneWidget);
    await tester.tap(
      find.byWidgetPredicate(
        (w) => w is Toggle && w.label == 'A message you send the bot',
      ),
    );
    await pumpFrames(tester);
    await tester.tap(find.text('Give'));
    await pumpFrames(tester);
    final grant = _sent(harness, (m) => m.hasGrantApp()).single.grantApp;
    expect(grant.agentId, 'june');
    expect(grant.scopes, ['send', 'read']);
    final subscribe = _sent(
      harness,
      (m) => m.hasSubscribeApp(),
    ).single.subscribeApp;
    expect(subscribe.eventKind, 'message');
    expect(subscribe.agentIds, ['rivet', 'june']);
  });

  testWidgets("an event needs its scope; Take back takes the app back", (
    tester,
  ) async {
    final harness = Harness(
      _workspace(
        apps: [
          _telegram(
            grants: [
              AppGrant(agentId: 'june', scopes: ['send']),
            ],
          ),
        ],
      ),
    );
    await _show(tester, harness, const AppsSettings());
    await tester.tap(_labeled('Telegram'));
    await pumpFrames(tester);
    await tester.tap(find.text('Change…'));
    await pumpFrames(tester);
    expect(find.text('Telegram for June'), findsOneWidget);
    expect(
      find.text('Turn on “Read your chat with the bot” first.'),
      findsOneWidget,
    );
    final event = find.byWidgetPredicate(
      (w) => w is Toggle && w.label == 'A message you send the bot',
    );
    expect(tester.widget<Toggle>(event).onChanged, isNull);
    await tester.tap(find.text('Take back'));
    await pumpFrames(tester);
    final grant = _sent(harness, (m) => m.hasGrantApp()).single.grantApp;
    expect(grant.agentId, 'june');
    expect(grant.scopes, isEmpty);
    expect(_sent(harness, (m) => m.hasSubscribeApp()), isEmpty);
  });

  testWidgets('uninstalling can keep its data', (tester) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, const AppsSettings());
    await tester.tap(_labeled('Telegram'));
    await pumpFrames(tester);
    await tester.tap(find.text('Uninstall…'));
    await pumpFrames(tester);
    expect(find.text('Uninstall Telegram?'), findsOneWidget);
    await tester.tap(
      find.byWidgetPredicate((w) => w is Toggle && w.label == 'Keep its data'),
    );
    await pumpFrames(tester);
    await tester.tap(find.text('Uninstall'));
    await pumpFrames(tester);
    final uninstall = _sent(harness, (m) => m.hasUninstallApp()).single;
    expect(uninstall.uninstallApp.id, 'telegram');
    expect(uninstall.uninstallApp.keepData, isTrue);
  });

  testWidgets('a secret given to an app shows as given to the app', (
    tester,
  ) async {
    final harness = Harness(
      _workspace(
        vault: VaultState(
          exists: true,
          unlocked: true,
          secrets: [
            SecretInfo(
              id: 's1',
              name: 'Telegram Bot token',
              grants: [SecretGrant(agentId: 'app:telegram')],
            ),
          ],
        ),
      ),
    );
    await _show(tester, harness, const SecretsSettings());
    expect(find.textContaining('Given to Telegram'), findsOneWidget);
  });

  group('data sources', () {
    AppInfo charts({bool records = false, bool lea = false}) =>
        _telegram().deepCopy()
          ..id = 'charts'
          ..name = 'Charts'
          ..sources.addAll([
            AppDataSource(
              id: 'factory-records',
              kind: AppDataKind.APP_DATA_KIND_FACTORY_RECORDS,
              label: "Claktory's records",
              reason: 'To chart your usage.',
              granted: records,
            ),
            AppDataSource(
              id: 'agent:lea',
              kind: AppDataKind.APP_DATA_KIND_AGENT_DATABASE,
              agentId: 'lea',
              label: "Lea's database",
              granted: lea,
            ),
          ]);

    testWidgets('the install review lists them, granting nothing', (
      tester,
    ) async {
      final harness = Harness(_workspace(apps: []));
      harness.backend.appPermissions = AppPermissions(
        sha256: 'ab12',
        id: 'charts',
        name: 'Charts',
        version: '1.0.0',
        tier: AppTier.APP_TIER_CLAKTORY,
        hosts: ['api.example.com', 'cdn.example.com'],
        sources: [
          AppDataSource(
            id: 'factory-records',
            label: "Claktory's records",
            reason: 'To chart your usage.',
          ),
          AppDataSource(
            id: 'agent:*',
            agentId: '*',
            label: "Agents' own databases (you choose which)",
          ),
        ],
      );
      appFilePicker = () async => [1];
      await _show(tester, harness, const AppsSettings());
      await tester.tap(find.text('Install from a file…'));
      await pumpFrames(tester);
      expect(find.text('It asks to read'), findsOneWidget);
      expect(find.text("Claktory's records"), findsOneWidget);
      // What each holds, plainly, and where the app can send it.
      expect(
        find.text(
          "Charts can read all your agents' conversations, including your "
          'answers to their questions, and their tasks, checklists, monitors '
          'and usage.',
        ),
        findsOneWidget,
      );
      expect(
        find.text(
          'Charts can read everything each agent you choose keeps in its '
          'database.',
        ),
        findsOneWidget,
      );
      expect(
        find.text(
          'Charts can send anything it reads to api.example.com and '
          'cdn.example.com.',
        ),
        findsOneWidget,
      );
      expect(find.text('To chart your usage.'), findsOneWidget);
      expect(
        find.text("Agents' own databases (you choose which)"),
        findsOneWidget,
      );
      expect(
        find.text('Nothing is shared until you turn it on in Settings → Apps.'),
        findsOneWidget,
      );
      await tester.tap(find.text('Install'));
      await pumpFrames(tester);
      expect(_sent(harness, (m) => m.hasGrantAppSource()), isEmpty);
    });

    testWidgets('each is turned on and off on its own in Settings', (
      tester,
    ) async {
      final harness = Harness(_workspace(apps: [charts()]));
      await _show(tester, harness, const AppsSettings());
      await tester.tap(_labeled('Charts'));
      await pumpFrames(tester);
      expect(find.text('It can read'), findsOneWidget);
      expect(
        find.text('Charts can read everything Lea keeps in its database.'),
        findsOneWidget,
      );
      expect(
        find.text('Charts can send anything it reads to api.telegram.org.'),
        findsOneWidget,
      );
      expect(
        find.text("Turn on what Charts can read. It can't read anything else."),
        findsOneWidget,
      );
      Finder toggle(String label) =>
          find.byWidgetPredicate((w) => w is Toggle && w.label == label);
      expect(tester.widget<Toggle>(toggle("Lea's database")).value, isFalse);
      await tester.tap(toggle("Lea's database"));
      await pumpFrames(tester);
      final grant = _sent(
        harness,
        (m) => m.hasGrantAppSource(),
      ).single.grantAppSource;
      expect(grant.appId, 'charts');
      expect(grant.sourceId, 'agent:lea');
      expect(grant.granted, isTrue);

      // As the backend has it now: on, and turned off again.
      await harness.apply(tester, _workspace(apps: [charts(lea: true)]));
      expect(tester.widget<Toggle>(toggle("Lea's database")).value, isTrue);
      expect(find.textContaining('Turn on what Charts'), findsNothing);
      await tester.tap(toggle("Lea's database"));
      await pumpFrames(tester);
      expect(
        _sent(
          harness,
          (m) => m.hasGrantAppSource(),
        ).last.grantAppSource.granted,
        isFalse,
      );
    });

    testWidgets('giving the app to an agent says what it can then read', (
      tester,
    ) async {
      final harness = Harness(
        _workspace(apps: [charts(records: true, lea: true)]),
      );
      await _show(tester, harness, const AppsSettings());
      await tester.tap(_labeled('Charts'));
      await pumpFrames(tester);
      await tester.tap(find.text('Give to an agent…'));
      await pumpFrames(tester);
      expect(
        find.text(
          "Through Charts, June can also read Claktory's records and "
          "Lea's database.",
        ),
        findsOneWidget,
      );
    });

    testWidgets('with nothing granted, giving it warns of nothing', (
      tester,
    ) async {
      final harness = Harness(_workspace(apps: [charts()]));
      await _show(tester, harness, const AppsSettings());
      await tester.tap(_labeled('Charts'));
      await pumpFrames(tester);
      await tester.tap(find.text('Give to an agent…'));
      await pumpFrames(tester);
      expect(find.textContaining('can also read'), findsNothing);
    });
  });

  testWidgets("Claktory's own apps show with Install, reviewed and installed "
      'as a file is; an older one installed offers its update', (tester) async {
    final harness = Harness(
      _workspace(apps: [])
        ..bundledApps.add(
          BundledApp(
            id: 'charts',
            name: 'Charts',
            description: 'Live charts of what your agents do.',
            icon: 'chart-column',
            version: '0.2.1',
            publisher: 'claktory',
          ),
        ),
    );
    harness.backend.appPermissions = AppPermissions(
      sha256: 'abc',
      id: 'charts',
      name: 'Charts',
      version: '0.2.1',
      publisher: 'claktory',
      tier: AppTier.APP_TIER_CLAKTORY,
    );
    await _show(tester, harness, const AppsSettings());

    expect(find.text('No apps yet.'), findsNothing);
    expect(find.text('Charts'), findsOneWidget);
    expect(find.text('Live charts of what your agents do.'), findsOneWidget);
    expect(find.text('Made by Claktory · Version 0.2.1'), findsOneWidget);
    await tester.tap(find.text('Install'));
    await tester.pumpAndSettle();
    final review = _sent(harness, (m) => m.hasInstallLocalApp()).single;
    expect(review.installLocalApp.bundled, 'charts');
    expect(review.installLocalApp.approve, isFalse);
    expect(review.installLocalApp.content, isEmpty);
    expect(find.text('Install Charts?'), findsOneWidget);
    // Claktory's own says so once, not "By claktory" too.
    expect(find.textContaining('By claktory'), findsNothing);
    expect(find.textContaining('Made by Claktory'), findsWidgets);

    await tester.tap(find.text('Install').last);
    await tester.pumpAndSettle();
    final install = _sent(harness, (m) => m.hasInstallLocalApp()).last;
    expect(install.installLocalApp.bundled, 'charts');
    expect(install.installLocalApp.approve, isTrue);
    expect(install.installLocalApp.sha256, 'abc');

    // An older version installed: its update.
    harness.backend.workspace.apply(
      _workspace()
        ..bundledApps.add(
          BundledApp(
            id: 'charts',
            name: 'Charts',
            version: '0.2.1',
            installedVersion: '0.2.0',
          ),
        ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Update to v0.2.1'), findsOneWidget);
    expect(
      find.text('Made by Claktory · Version 0.2.0 installed'),
      findsOneWidget,
    );
    // Installed and listed: one row, the installed app's, with its update.
    harness.backend.workspace.apply(
      _workspace(
          apps: [
            _telegram(),
            AppInfo(
              id: 'charts',
              name: 'Charts',
              version: '0.2.0',
              tier: AppTier.APP_TIER_CLAKTORY,
              state: AppState.APP_STATE_RUNNING,
            ),
          ],
        )
        ..bundledApps.add(
          BundledApp(
            id: 'charts',
            name: 'Charts',
            version: '0.2.1',
            installedVersion: '0.2.0',
          ),
        ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Charts'), findsOneWidget);
    expect(
      find.text('Made by Claktory · Version 0.2.0 installed'),
      findsNothing,
    );
    expect(find.text('v0.2.0 installed'), findsOneWidget);
    harness.backend.appPermissions = AppPermissions(
      sha256: 'abd',
      id: 'charts',
      name: 'Charts',
      version: '0.2.1',
      installedVersion: '0.2.0',
      publisher: 'claktory',
      tier: AppTier.APP_TIER_CLAKTORY,
    );
    await tester.tap(find.text('Update to v0.2.1'));
    await tester.pumpAndSettle();
    // The same version form as the button's.
    expect(find.text('Update Charts to v0.2.1?'), findsOneWidget);
  });
}
