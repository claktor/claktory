// Main-like root placement is required for Android selection overlays.
import 'package:factory_app/drop.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart';
import 'package:factory_app/update.dart';
import 'package:factory_app/zoom.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../harness.dart';
import '../../phone_test.dart' show workspace;

Future<Harness> showAndroid(
  WidgetTester tester, {
  Workspace? snapshot,
  Harness? harness,
  Size size = const Size(393, 852),
  double? textScale,
  bool expectPhone = true,
  bool withWindowDrop = false,
}) async {
  if (textScale != null) {
    tester.platformDispatcher.textScaleFactorTestValue = textScale;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
  }
  final shown =
      harness ?? Harness(snapshot ?? (workspace()..selectedAgentId = 'mike'));
  tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  // Match main.dart's root delegate placement. An override inside the Shell
  // would not reach the root overlay used by EditableText's context menu.
  await tester.pumpWidget(
    WidgetsApp(
      color: const Color(0xff202020),
      localizationsDelegates: PhonePlatformPolicy.current.localizations,
      builder: (context, _) => AppScope(
        backend: shown.backend,
        login: shown.login,
        page: shown.page,
        appBuild: AppBuild(
          version: shown.appVersion,
          bundlePath: shown.bundlePath,
          quit: () => shown.quits++,
          updater: shown.updater,
        ),
        drafts: shown.drafts,
        history: shown.history,
        connections: shown.connections,
        camera: shown.camera,
        child: withWindowDrop
            ? const AppZoom(
                child: Stack(
                  children: [
                    Positioned.fill(child: Shell()),
                    Positioned.fill(child: WindowDropOverlay()),
                  ],
                ),
              )
            : const AppZoom(child: Shell()),
      ),
    ),
  );
  await pumpFrames(tester);
  expect(find.byType(PhoneShell), expectPhone ? findsOneWidget : findsNothing);
  return shown;
}
