// Android's Back on a wide window (a tablet, a resizable window): a dialog
// first, then a page shown in place of the conversation, then the system.
// The host sits where main.dart puts it, above the window's own overlay.
import 'package:factory_app/agent_view.dart' show AgentView;
import 'package:factory_app/android/window_back_host.dart';
import 'package:factory_app/controls.dart' show showModal;
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:factory_app/shell.dart';
import 'package:factory_app/update.dart';
import 'package:factory_app/zoom.dart';
import 'package:cupertino_ui/cupertino_ui.dart'
    show DefaultCupertinoLocalizations;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import 'support/channel_fakes.dart';

Future<void> _back(WidgetTester tester) async {
  await sendNativeMethod(
    SystemChannels.navigation,
    const MethodCall('popRoute'),
  );
  await pumpFrames(tester);
}

Future<void> _show(WidgetTester tester, Harness harness, Size size) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  tester.view.display.size = size;
  tester.view.display.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  addTearDown(tester.view.display.reset);
  await tester.pumpWidget(
    WidgetsApp(
      color: const Color(0xFF000000),
      localizationsDelegates: const [DefaultCupertinoLocalizations.delegate],
      builder: (context, _) => AppScope(
        backend: harness.backend,
        login: harness.login,
        page: harness.page,
        appBuild: AppBuild(version: 'dev', bundlePath: '', quit: () {}),
        drafts: harness.drafts,
        history: harness.history,
        connections: harness.connections,
        camera: harness.camera,
        // As main.dart places it: after AppZoom, above the window.
        child: AppZoom(
          child: AndroidWindowBackHost(window: 'A', child: const Shell()),
        ),
      ),
    ),
  );
  await pumpFrames(tester);
}

Workspace _workspace() => Workspace(
  agents: [Agent(id: 'a1', name: 'June', modelAvailable: true)],
  selectedAgentId: 'a1',
  settings: Settings(userName: 'QA', name: 'Mac'),
);

void main() {
  var systemPops = 0;
  setUp(() {
    PhonePlatformPolicy.debugAndroidPolicy = true;
    systemPops = 0;
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'SystemNavigator.pop') systemPops++;
      return null;
    });
    addTearDown(() {
      PhonePlatformPolicy.debugAndroidPolicy = false;
      messenger.setMockMethodCallHandler(SystemChannels.platform, null);
    });
  });

  testWidgets('Back closes a dialog, then the page under it, then leaves '
      'it to the system', (tester) async {
    final harness = Harness(_workspace());
    harness.page.value = AppPage.settings;
    await _show(tester, harness, const Size(1200, 900));
    expect(find.byType(SettingsView), findsOneWidget);

    var closed = false;
    showModal(
      tester.element(find.byType(SettingsView)),
      (context, close) => const Text('A dialog'),
    ).then((_) => closed = true);
    await pumpFrames(tester);
    expect(find.text('A dialog'), findsOneWidget);

    // A resize keeps the dialog: only another Claktory takes it away.
    tester.view.physicalSize = const Size(1000, 800);
    tester.view.display.size = const Size(1000, 800);
    await pumpFrames(tester);
    expect(find.text('A dialog'), findsOneWidget);

    await _back(tester);
    expect(find.text('A dialog'), findsNothing);
    expect(closed, isTrue);
    expect(harness.page.value, AppPage.settings, reason: 'dialog first');
    expect(systemPops, 0);

    await _back(tester);
    expect(harness.page.value, AppPage.agent);
    expect(find.byType(SettingsView), findsNothing);
    expect(systemPops, 0);

    // The conversation is the root: Back is the system's.
    await _back(tester);
    expect(systemPops, 1);
  });

  testWidgets('a dialog closed its own way no longer takes Back', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    await _show(tester, harness, const Size(1200, 900));
    late VoidCallback closeIt;
    showModal(tester.element(find.byType(AgentView)), (context, close) {
      closeIt = close;
      return const Text('A dialog');
    });
    await pumpFrames(tester);
    expect(find.text('A dialog'), findsOneWidget);
    closeIt();
    await pumpFrames(tester);
    await _back(tester);
    expect(systemPops, 1);
  });
}
