// Settings → Claktory's browser and the Browser row in Agent settings, drawn
// dark on a Mac, an iPhone and an iPad. With SHOTS set, each is drawn to a
// PNG there:
//   SHOTS=/tmp/browser flutter test test/browser_settings_shots_test.dart
// Without it, each still lays out without an overflow or an exception.
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/controls.dart' show Toggle, showModal;
import 'package:factory_app/forms.dart' show AgentSettingsForm;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;

Agent _agent(
  String id,
  String name,
  String color, {
  bool browser = false,
  bool admin = false,
}) => Agent(
  id: id,
  name: name,
  color: color,
  modelAvailable: true,
  extensions: [
    AgentExtension(id: 'web', enabled: true),
    AgentExtension(id: 'browser', enabled: browser),
    AgentExtension(id: 'monitors', enabled: true),
    AgentExtension(
      id: 'agents',
      enabled: true,
      options: {if (admin) 'admin': OptionValue(on: true)}.entries,
    ),
  ],
);

Workspace _workspace({AgentBrowser? browser}) => Workspace(
  agents: [
    _agent('rivet', 'Rivet', 'blue', admin: true)..role = 'Admin',
    _agent('yukako', 'Yukako', 'pink', browser: true),
    _agent('koichi', 'Koichi', 'amber', browser: true),
  ],
  extensions: [
    Extension(
      id: 'web',
      name: 'Web',
      icon: 'globe',
      description: 'Searches and reads web pages.',
    ),
    Extension(id: 'browser', name: 'Browser', icon: 'monitor'),
    Extension(
      id: 'monitors',
      name: 'Monitors',
      icon: 'clock',
      description: 'Runs jobs on a schedule.',
    ),
    Extension(id: 'agents', name: 'Messaging', icon: 'users'),
  ],
  agentBrowser:
      browser ??
      AgentBrowser(
        state: AgentBrowserState.AGENT_BROWSER_STATE_WARM,
        chromeVersion: '142.0.7444.59',
        timeZone: 'Europe/Rome',
        languages: ['it-IT', 'en-US'],
      ),
  agentBrowsers: [
    AgentBrowser(
      agentId: 'yukako',
      state: AgentBrowserState.AGENT_BROWSER_STATE_WARM,
      signedInSites: ['https://x.com', 'https://www.linkedin.com'],
    ),
  ],
  settings: Settings(name: 'Studio'),
  selectedAgentId: 'yukako',
);

Future<void> _shot(WidgetTester tester, String name) async {
  expect(tester.takeException(), isNull);
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

const _devices = [
  ('mac', Size(1200, 800), TargetPlatform.macOS),
  ('iphone', Size(393, 852), TargetPlatform.iOS),
  ('ipad', Size(1180, 820), TargetPlatform.iOS),
];

void main() {
  setUpAll(loadGeist);
  setUp(() {
    final dispatcher = TestWidgetsFlutterBinding.instance.platformDispatcher;
    dispatcher.platformBrightnessTestValue = Brightness.dark;
    addTearDown(dispatcher.clearPlatformBrightnessTestValue);
    WidgetsApp.debugAllowBannerOverride = false;
    addTearDown(() => WidgetsApp.debugAllowBannerOverride = true);
  });

  Future<Harness> settings(
    WidgetTester tester,
    Size size,
    TargetPlatform platform, {
    Workspace? workspace,
  }) async {
    debugDefaultTargetPlatformOverride = platform;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(workspace ?? _workspace());
    await harness.show(tester, size: size);
    harness.page.value = AppPage.settings;
    await pumpFrames(tester);
    harness.backend.settingsSection.value = "claktory's browser";
    await pumpFrames(tester, const Duration(seconds: 2));
    return harness;
  }

  for (final (where, size, platform) in _devices) {
    testWidgets("Claktory's browser in Settings, on the $where", (
      tester,
    ) async {
      await settings(tester, size, platform);
      expect(find.text('Ready · Chrome 142'), findsOneWidget);
      await _shot(tester, 'settings-$where');
      await tester.tap(find.bySemanticsLabel("Yukako's browser"));
      await pumpFrames(tester);
      expect(find.text('Sign out of x.com'), findsOneWidget);
      await _shot(tester, 'settings-menu-$where');
    });

    testWidgets('Browser in Agent settings, on the $where', (tester) async {
      debugDefaultTargetPlatformOverride = platform;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final workspace = _workspace();
      final harness = Harness(workspace);
      await harness.show(tester, size: size);
      await pumpFrames(tester);
      for (final id in ['yukako', 'rivet']) {
        final agent = workspace.agents.firstWhere((a) => a.id == id);
        showModal(
          tester.element(
            find
                .descendant(
                  of: find.byType(Overlay).first,
                  matching: find.byType(Text),
                )
                .first,
          ),
          (context, close) => AgentSettingsForm(
            agent: agent,
            close: close,
            showExtensions: true,
          ),
        );
        await pumpFrames(tester);
        await tester.ensureVisible(find.text('Browser').last);
        await pumpFrames(tester);
        await _shot(tester, 'agent-$id-$where');
        if (id == 'yukako') {
          await tester.tap(
            find.byWidgetPredicate(
              (widget) => widget is Toggle && widget.label == 'Browser',
            ),
          );
          await pumpFrames(tester);
          await tester.tap(find.text('Save').last);
          await pumpFrames(tester);
          expect(find.text("Turn off Yukako's browser?"), findsOneWidget);
          await _shot(tester, 'agent-turn-off-$where');
          await tester.tap(find.text('Cancel').last);
          await pumpFrames(tester);
        }
        await tester.tap(find.text('Cancel').last);
        await pumpFrames(tester);
      }
    });
  }

  testWidgets('not set up yet, the command unfolded, on a Mac', (tester) async {
    await settings(
      tester,
      const Size(1200, 800),
      TargetPlatform.macOS,
      workspace: _workspace(
        browser: AgentBrowser(
          state: AgentBrowserState.AGENT_BROWSER_STATE_UNAVAILABLE,
          setup: AgentBrowserSetup(
            problem: "Apple's container tool isn't installed on this Mac.",
            command:
                'brew install container && container system start '
                '--enable-kernel-install',
          ),
        ),
      ),
    );
    await tester.tap(find.text('Set it up yourself'));
    await pumpFrames(tester);
    await _shot(tester, 'settings-not-set-up-mac');
  });

  testWidgets("a computer that can't run it, on an iPhone", (tester) async {
    await settings(
      tester,
      const Size(393, 852),
      TargetPlatform.iOS,
      workspace: _workspace(
        browser: AgentBrowser(
          state: AgentBrowserState.AGENT_BROWSER_STATE_UNAVAILABLE,
          setup: AgentBrowserSetup(unsupported: true),
        ),
      ),
    );
    await _shot(tester, 'settings-cant-run-iphone');
  });
}
