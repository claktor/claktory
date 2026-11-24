// Actual shared tablet panes under the window dispatcher. Framework channel
// events prove ordering; this is not Android OS/device acceptance.
import 'dart:async';

import 'package:factory_app/agent_view.dart';
import 'package:factory_app/android/window_back_host.dart';
import 'package:factory_app/controls.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/settings_view.dart';
import 'package:factory_app/shell.dart';
import 'package:factory_app/usage_view.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../panes_test.dart' show leaf, row, snapshot, split;
import '../phone_sizes_test.dart' show loadGeist;
import 'support/channel_fakes.dart';

Future<void> _button() async =>
    sendNativeMethod(SystemChannels.navigation, const MethodCall('popRoute'));

Future<Object?> _gesture(String method) => sendNativeMethod(
  SystemChannels.backGesture,
  MethodCall(
    method,
    method == 'startBackGesture'
        ? {
            'touchOffset': [1.0, 300.0],
            'progress': 0.0,
            'swipeEdge': 0,
          }
        : null,
  ),
);

void main() {
  setUpAll(loadGeist);

  testWidgets('tablet Back: keyboard, dialog, newest side pane, then system; '
      'cancel preserves panes and the unsent draft', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    PhonePlatformPolicy.debugAndroidPolicy = true;
    var systemPops = 0;
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'SystemNavigator.pop') systemPops++;
      return null;
    });
    addTearDown(() {
      debugDefaultTargetPlatformOverride = null;
      PhonePlatformPolicy.debugAndroidPolicy = false;
      messenger.setMockMethodCallHandler(SystemChannels.platform, null);
    });
    const draft = 'Unsent 日本語 👩🏽‍💻';
    final harness = Harness(
      snapshot(
        layout: split('side', row, [
          leaf('chat', 'chat'),
          leaf('settings', 'settings'),
          leaf('usage', 'usage'),
        ]),
      ),
    );
    harness.backend.appliesLayouts = true;
    harness.drafts.set('a1', draft);
    await harness.show(
      tester,
      size: const Size(1600, 900),
      home: const AndroidWindowBackHost(window: 'tablet', child: Shell()),
    );
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsOneWidget);
    expect(find.byType(SettingsView), findsOneWidget);
    expect(find.byType(UsageView), findsOneWidget);

    var dialogClosed = false;
    unawaited(
      showModal(
        tester.element(find.byType(UsageView)),
        (_, close) => const Text('Tablet dialog'),
      ).then((_) => dialogClosed = true),
    );
    await pumpFrames(tester);
    tester.view.viewInsets = const FakeViewPadding(bottom: 368);
    await pumpFrames(tester);
    tester.testTextInput.log.clear();
    await _button();
    await tester.pump();
    expect(
      tester.testTextInput.log.where((c) => c.method == 'TextInput.hide'),
      hasLength(1),
    );
    expect(dialogClosed, isFalse);
    expect(find.text('Tablet dialog'), findsOneWidget);
    expect(find.byType(UsageView), findsOneWidget);
    expect(find.byType(SettingsView), findsOneWidget);
    expect(systemPops, 0);

    tester.view.viewInsets = FakeViewPadding.zero;
    await pumpFrames(tester);
    await _button();
    await pumpFrames(tester);
    expect(dialogClosed, isTrue);
    expect(find.text('Tablet dialog'), findsNothing);
    expect(find.byType(UsageView), findsOneWidget);
    expect(find.byType(SettingsView), findsOneWidget);
    expect(systemPops, 0);

    final before = harness.backend.sent.length;
    expect(await _gesture('startBackGesture'), isTrue);
    await _gesture('cancelBackGesture');
    await pumpFrames(tester);
    expect(harness.backend.sent.length, before);
    expect(harness.drafts.of('a1'), draft);
    expect(find.byType(UsageView), findsOneWidget);
    expect(find.byType(SettingsView), findsOneWidget);

    expect(await _gesture('startBackGesture'), isTrue);
    await _gesture('commitBackGesture');
    await pumpFrames(tester);
    expect(find.byType(UsageView), findsNothing);
    expect(find.byType(SettingsView), findsOneWidget);
    expect(systemPops, 0);
    await _button();
    await pumpFrames(tester);
    expect(find.byType(SettingsView), findsNothing);
    expect(find.byType(AgentView), findsOneWidget);
    expect(harness.drafts.of('a1'), draft);
    expect(systemPops, 0);
    expect(tester.takeException(), isNull);
    await _button();
    await tester.pump();
    expect(systemPops, 1);
    expect(
      harness.backend.sent.where(
        (m) => m.hasSendMessage() || m.hasSendChatMessage(),
      ),
      isEmpty,
    );
  });
}
