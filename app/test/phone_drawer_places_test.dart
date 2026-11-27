// The production PhoneClaktories root, not Shell's fallback drawer.
// Real product navigation with a recording backend; not native acceptance.
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/account.dart' show SignInStep;
import 'package:factory_app/changelog.dart';
import 'package:factory_app/map.dart';
import 'package:factory_app/marketplace.dart';
import 'package:factory_app/monitors_overview.dart';
import 'package:factory_app/usage_view.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/device_account.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/main.dart';
import 'package:factory_app/name_step.dart';
import 'package:factory_app/mobile/device.dart';
import 'package:factory_app/mobile/local_network.dart';
import 'package:factory_app/mobile/drawer.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:factory_app/shell.dart' show NoAgentView;
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final _code = 'factory://?t=${'a' * 64}&name=Origin&e=${'c' * 64}';

class _App {
  _App({this.selected = true, this.named = true, this.signedIn = true});
  final bool selected;
  final bool named;
  final bool signedIn;
  final connections = Connections(
    MemoryConnectionStorage(),
    null,
    null,
    null,
    DeviceAccount(),
  );
  final drafts = DeviceDrafts(MemoryDraftStorage());
  late Claktories claktories;

  Future<void> show(WidgetTester tester) async {
    connections.use(parsePairingCode(_code));
    debugDeviceCapabilities = const DeviceCapabilities(idiom: Idiom.phone);
    final previousLocalNetwork = localNetwork;
    addTearDown(() {
      debugDeviceCapabilities = null;
      localNetwork = previousLocalNetwork;
    });
    tester.view.physicalSize = const Size(393, 852);
    tester.view.devicePixelRatio = 1;
    tester.view.display.size = const Size(393, 852);
    tester.view.display.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    addTearDown(tester.view.display.reset);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pumpWidget(
      FactoryApp(
        drafts: drafts,
        connections: connections,
        localBackend: false,
        open: (target) {
          final backend = RecordingBackend(localBackend: false)
            ..connectTo(target);
          backend.workspace.apply(
            Workspace(
              account: signedIn
                  ? null
                  : ClaktoryAccount(
                      state: ClaktoryAccountState
                          .CLAKTORY_ACCOUNT_STATE_SIGNED_OUT,
                    ),
              agents: [Agent(id: 'rivet', name: 'Rivet', modelAvailable: true)],
              selectedAgentId: selected ? 'rivet' : '',
              settings: Settings(userName: named ? 'QA' : '', name: 'Origin'),
            ),
          );
          backend.onSend = (message) {
            if (message.hasUpdateSettings() &&
                message.updateSettings.hasUserName()) {
              backend.workspace.apply(
                backend.workspace.workspace.deepCopy()
                  ..settings.userName = message.updateSettings.userName,
              );
            }
          };
          return backend;
        },
      ),
    );
    await pumpFrames(tester);
    expect(find.byType(PhoneClaktories), findsOneWidget);
    final root = find.byType(selected ? AgentView : NoAgentView);
    expect(root, findsOneWidget);
    claktories = ClaktoriesScope.of(tester.element(root))!;
  }
}

void main() {
  for (final platform in [TargetPlatform.android, TargetPlatform.iOS]) {
    testWidgets('${platform.name}: drawer Settings is visible and Back returns '
        'to the conversation', (tester) async {
      PhonePlatformPolicy.debugAndroidPolicy = true;
      addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
      final app = _App();
      await app.show(tester);
      final host = tester.state<PhoneDrawerHostState>(
        find.byType(PhoneDrawerHost),
      );
      await tester.tap(
        find.descendant(
          of: find.byType(AgentView),
          matching: find.bySemanticsLabel('Menu'),
        ),
      );
      await pumpFrames(tester);
      expect(host.drawerOpen, isTrue);
      final settings = find.descendant(
        of: find.byType(Sidebar),
        matching: find.text('Settings'),
      );
      await tester.ensureVisible(settings);
      await tester.tap(settings);
      await pumpFrames(tester);
      expect(app.claktories.selected.page.value, AppPage.settings);
      expect(find.byType(SettingsView), findsOneWidget);
      expect(host.drawerOpen, isFalse);
      expect(find.byType(Sidebar), findsNothing);
      expect(tester.takeException(), isNull);
      host.pop();
      await pumpFrames(tester);
      expect(app.claktories.selected.page.value, AppPage.agent);
      expect(find.byType(AgentView), findsOneWidget);
      expect(host.drawerOpen, isFalse);
    }, variant: TargetPlatformVariant.only(platform));
    testWidgets('${platform.name}: Settings from the initially offered drawer '
        'closes it with no conversation chosen', (tester) async {
      PhonePlatformPolicy.debugAndroidPolicy = true;
      addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
      final app = _App(selected: false);
      await app.show(tester);
      final host = tester.state<PhoneDrawerHostState>(
        find.byType(PhoneDrawerHost),
      );
      expect(host.drawerOpen, isTrue);
      final settings = find.descendant(
        of: find.byType(Sidebar),
        matching: find.text('Settings'),
      );
      await tester.ensureVisible(settings);
      await tester.tap(settings);
      await pumpFrames(tester);
      expect(app.claktories.selected.page.value, AppPage.settings);
      expect(find.byType(SettingsView), findsOneWidget);
      expect(host.drawerOpen, isFalse);
      expect(find.byType(Sidebar), findsNothing);
      expect(tester.takeException(), isNull);
    }, variant: TargetPlatformVariant.only(platform));

    for (final (page, label, type) in [
      (AppPage.settings, 'Settings', SettingsView),
      (AppPage.map, 'Map', MapView),
      (AppPage.usage, 'Usage', UsageView),
      (AppPage.marketplace, 'Marketplace', MarketplaceView),
      (AppPage.monitors, 'Monitors', MonitorsOverview),
      (AppPage.changelog, "What's new", ChangelogView),
    ]) {
      testWidgets('${platform.name}: $label chosen before the name step '
          'finishes is revealed when that step goes', (tester) async {
        PhonePlatformPolicy.debugAndroidPolicy = true;
        addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
        final app = _App(selected: false, named: false);
        await app.show(tester);
        expect(find.byType(NameStep), findsOneWidget);
        final host = tester.state<PhoneDrawerHostState>(
          find.byType(PhoneDrawerHost),
        );
        expect(host.drawerOpen, isTrue);
        final place = find.descendant(
          of: find.byType(Sidebar),
          matching: find.text(label),
        );
        await tester.ensureVisible(place);
        await tester.tap(place);
        await pumpFrames(tester);
        expect(app.claktories.selected.page.value, page);
        // What's new deliberately bypasses setup; the other pages wait.
        if (page == AppPage.changelog) {
          expect(find.byType(NameStep), findsNothing);
          expect(find.byType(ChangelogView), findsOneWidget);
          expect(host.drawerOpen, isFalse);
          expect(find.byType(Sidebar), findsNothing);
          return;
        }
        expect(find.byType(NameStep), findsOneWidget);
        await tester.enterText(
          find.descendant(
            of: find.byType(NameStep),
            matching: find.byType(EditableText),
          ),
          'Pat',
        );
        await tester.pump(); // Rebuild Continue from its changed field.
        await tester.tap(
          find.descendant(
            of: find.byType(NameStep),
            matching: find.text('Continue'),
          ),
        );
        await pumpFrames(tester);
        expect(find.byType(NameStep), findsNothing);
        expect(find.byType(type), findsOneWidget);
        expect(host.drawerOpen, isFalse);
        expect(find.byType(Sidebar), findsNothing);
        expect(tester.takeException(), isNull);
      }, variant: TargetPlatformVariant.only(platform));
    }

    testWidgets('${platform.name}: Settings chosen before sign-in is revealed '
        'after the Claktory signs in', (tester) async {
      PhonePlatformPolicy.debugAndroidPolicy = true;
      addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
      final app = _App(selected: false, signedIn: false);
      await app.show(tester);
      expect(find.byType(SignInStep), findsOneWidget);
      final host = tester.state<PhoneDrawerHostState>(
        find.byType(PhoneDrawerHost),
      );
      expect(host.drawerOpen, isTrue);
      final settings = find.descendant(
        of: find.byType(Sidebar),
        matching: find.text('Settings'),
      );
      await tester.ensureVisible(settings);
      await tester.tap(settings);
      await pumpFrames(tester);
      expect(app.claktories.selected.page.value, AppPage.settings);
      expect(find.byType(SignInStep), findsOneWidget);
      // The successful account response arrives from the backend. No real
      // account or HTTP service is used by this navigation regression.
      final backend = app.claktories.selected.backend;
      backend.workspace.apply(
        backend.workspace.workspace.deepCopy()
          ..account.state =
              ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_IN,
      );
      await pumpFrames(tester);
      expect(find.byType(SignInStep), findsNothing);
      expect(find.byType(SettingsView), findsOneWidget);
      expect(host.drawerOpen, isFalse);
      expect(find.byType(Sidebar), findsNothing);
      expect(tester.takeException(), isNull);
    }, variant: TargetPlatformVariant.only(platform));
  }
}
