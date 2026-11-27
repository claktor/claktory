// Pictures of Settings as area 3 and 5a lay it out (ux/settings.md,
// ux/permissions.md §2.1), for Lucy to check against her mock-ups. With
// SHOTS set they're drawn to PNGs there:
//   SHOTS=/tmp/settings flutter test test/settings_shots_test.dart
// Without it they only check that each scene lays out without an error.
// test-bundle: alone (loads the app's fonts)
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/app_source.dart';
import 'package:factory_app/connections.dart' show RemoteFactory;
import 'package:factory_app/device_account.dart' show AccountClaktory;
import 'package:factory_app/highlights.dart' show HighlightStore;
import 'package:factory_app/notification_settings.dart'
    show DeviceNotifications;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recorder.dart';
import 'package:factory_app/scanner.dart' show CameraAccess;
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'permissions_settings_test.dart' show FakeNotifications;
import 'phone_sizes_test.dart' show loadGeist;
import 'voice_test.dart' show FakeRecorder;

Future<void> _shot(WidgetTester tester, String name) async {
  expect(tester.takeException(), isNull, reason: name);
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

Workspace _workspace({bool models = true}) {
  final snapshot = workspace();
  if (!models) snapshot.models.clear();
  snapshot.settings.notifications = NotificationSettings(
    needsYou: true,
    alerts: true,
  );
  // Unnamed, so Settings goes by its computer's name; a released version.
  snapshot
    ..remote = RemoteAccess(machine: 'Mac Studio')
    ..version = '0.1.24'
    // Report types shows only where Claktory has their rules.
    ..reportTypeRules = ReportTypeRules(fields: 20, types: 50)
    ..reportSchemas.add(
      ReportSchema(
        id: 'bug_report',
        name: 'Bug report',
        version: 1,
        builtin: true,
        fields: [
          ReportField(
            name: 'title',
            label: 'Title',
            type: ReportFieldType.REPORT_FIELD_TYPE_TEXT,
            required: true,
          ),
        ],
      ),
    );
  return snapshot;
}

void main() {
  late VoiceRecorder was;
  setUpAll(loadGeist);
  setUp(() {
    final dispatcher = TestWidgetsFlutterBinding.instance.platformDispatcher;
    dispatcher.platformBrightnessTestValue = Brightness.dark;
    addTearDown(dispatcher.clearPlatformBrightnessTestValue);
    WidgetsApp.debugAllowBannerOverride = false;
    addTearDown(() => WidgetsApp.debugAllowBannerOverride = true);
    was = deviceRecorder;
    deviceRecorder = FakeRecorder(granted: MicAccess.denied);
  });
  tearDown(() {
    deviceRecorder = was;
    DeviceNotifications.debugOverride = null;
  });

  Future<Harness> open(
    WidgetTester tester,
    Size size, {
    Workspace? snapshot,
    bool local = true,
  }) async {
    final harness = Harness(snapshot ?? _workspace(), localBackend: local)
      ..appVersion = '0.1.23';
    await harness.show(tester, size: size);
    if (!local) {
      // Connected to the Claktory on Mac Studio, as a phone or an iPad is.
      harness.connections.use(
        RemoteFactory.fromAccount(
          AccountClaktory(id: 's', name: 'Mac Studio', endpointId: 'e' * 64),
        ),
      );
      harness.backend.workspace.apply(snapshot ?? _workspace());
      await pumpFrames(tester);
    }
    harness.page.value = AppPage.settings;
    await pumpFrames(tester);
    return harness;
  }

  Future<void> at(
    WidgetTester tester,
    Harness harness,
    String place,
    String name,
  ) async {
    harness.backend.settingsSection.value = place;
    await pumpFrames(tester, const Duration(seconds: 2));
    await _shot(tester, name);
  }

  testWidgets('a Mac', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    DeviceNotifications.debugOverride = FakeNotifications('denied');
    final harness = await open(tester, const Size(1200, 800));
    await _shot(tester, 'mac-1-profile');
    await at(tester, harness, 'general', 'mac-2-general');
    await at(tester, harness, 'defaults', 'mac-3-defaults');
    await at(tester, harness, 'pipes', 'mac-4-report-types');
    await at(tester, harness, 'device#permissions', 'mac-5-this-mac');
    await tester.enterText(find.byType(EditableText).first, 'pair');
    await pumpFrames(tester);
    await _shot(tester, 'mac-6-search-pair');
    await tester.enterText(find.byType(EditableText).first, 'wallpaper');
    await pumpFrames(tester);
    await _shot(tester, 'mac-7-search-nothing');
    await tester.enterText(find.byType(EditableText).first, '');
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await pumpFrames(tester);

    // No model: Defaults' link to Providers.
    await harness.apply(tester, _workspace(models: false));
    harness.page.value = AppPage.settings;
    await pumpFrames(tester);
    await at(tester, harness, 'defaults', 'mac-8-defaults-no-model');
  });

  testWidgets("a Mac, Secrets: the account's recovery key (#239)", (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    DeviceNotifications.debugOverride = FakeNotifications('allowed');
    final now = Int64(DateTime.now().millisecondsSinceEpoch);
    Workspace vault({bool change = false}) => _workspace()
      ..vault = VaultState(
        exists: true,
        unlocked: true,
        recoveryKeyCovered: true,
        recoveryKeyMadeOn: 'Mac Studio',
        recoveryKeyMadeMs: now,
        recoveryKeyChangeWaiting: change,
        recoveryKeyChangeMadeOn: change ? 'Work Laptop' : '',
        recoveryKeyChangeMadeMs: change ? now : Int64.ZERO,
        secrets: [
          SecretInfo(
            id: 's1',
            name: 'AWS',
            description: "Matt's deploy account, eu-west-1",
          ),
        ],
      );
    final harness = await open(
      tester,
      const Size(1200, 800),
      snapshot: vault(),
    );
    await at(tester, harness, 'secrets', 'mac-10-secrets-covered');
    await harness.apply(tester, vault(change: true));
    await at(tester, harness, 'secrets', 'mac-11-secrets-new-key');
  });

  testWidgets("an iPhone, Secrets with a new recovery key (#239)", (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final now = Int64(DateTime.now().millisecondsSinceEpoch);
    final snapshot = _workspace()
      ..vault = VaultState(
        exists: true,
        unlocked: true,
        recoveryKeyCovered: true,
        recoveryKeyMadeOn: 'Mac Studio',
        recoveryKeyMadeMs: now,
        recoveryKeyChangeWaiting: true,
        recoveryKeyChangeMadeOn: 'Work Laptop',
        recoveryKeyChangeMadeMs: now,
      );
    final harness = await open(
      tester,
      const Size(393, 852),
      snapshot: snapshot,
      local: false,
    );
    await at(tester, harness, 'secrets', 'iphone-3-secrets-new-key');
  });

  testWidgets('a Mac, places macOS protects (item 12)', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    DeviceNotifications.debugOverride = FakeNotifications('allowed');
    final snapshot = _workspace()
      ..protectedPlaces.addAll([
        ProtectedPlace(
          root: '/Users/me/Documents',
          name: 'Documents',
          state: ProtectedPlaceState.PROTECTED_PLACE_STATE_REFUSED,
          checkedMs: Int64(1000),
          usedBy: ['nixos-site'],
          needsYou: true,
          title: "Your agents can't work in Documents",
          text:
              "macOS hasn't let Claktory Agents into Documents, so commands "
              'in nixos-site fail. Turn it on for Claktory Agents in System '
              'Settings.',
        ),
        ProtectedPlace(
          root: '/Users/me/Desktop',
          name: 'Desktop',
          state: ProtectedPlaceState.PROTECTED_PLACE_STATE_ALLOWED,
          checkedMs: Int64(1000),
          usedBy: ['blog'],
        ),
      ]);
    final harness = await open(
      tester,
      const Size(1200, 800),
      snapshot: snapshot,
    );
    await at(tester, harness, 'device#permissions', 'mac-12-this-mac-places');
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await tester.tap(find.text("Your agents can't work in Documents").first);
    await pumpFrames(tester, const Duration(seconds: 1));
    await _shot(tester, 'mac-13-needs-you-place');
  });

  testWidgets('a Mac, Settings in a pane beside the conversation', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    DeviceNotifications.debugOverride = FakeNotifications('allowed');
    final harness = Harness(_workspace()..highlights = true)
      ..backend.appliesLayouts = true;
    await harness.show(tester, size: const Size(1200, 800));
    await pumpFrames(tester);
    AppScope.openSettingsAt(
      tester.element(find.byType(Shell)),
      'general',
      setting: 'move',
    );
    await pumpFrames(tester, const Duration(milliseconds: 400));
    await _shot(tester, 'mac-9-pane-general-move-glowing');
    harness.page.value = AppPage.agent;
    await tester.pump(HighlightStore.duration);
  });

  testWidgets('an iPhone', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    DeviceNotifications.debugOverride = FakeNotifications('allowed');
    debugAppSource = AppSource.testFlight;
    addTearDown(() => debugAppSource = null);
    final harness = await open(tester, const Size(393, 852), local: false);
    harness.camera.state = CameraAccess.undetermined;
    await _shot(tester, 'iphone-1-list');
    await at(tester, harness, 'device#permissions', 'iphone-2-this-iphone');
  });

  testWidgets('an iPad, full screen', (tester) async {
    asIPad();
    DeviceNotifications.debugOverride = FakeNotifications('not_determined');
    debugAppSource = AppSource.appStore;
    addTearDown(() => debugAppSource = null);
    final harness = await open(tester, const Size(1194, 834), local: false);
    harness.camera.state = CameraAccess.denied;
    await at(tester, harness, 'device#permissions', 'ipad-1-this-ipad');
    await at(tester, harness, 'general', 'ipad-2-general');
  });
}
