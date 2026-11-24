// #363, Sale's row 21: on Android with no Claktory yet, Back from a way to
// connect (Paste code instead, Sign in with your email) goes back to the
// first screen's choices, and only from there leaves the app.
import 'package:factory_app/android/window_back_host.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/device_account.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart';
import 'package:factory_app/scanner.dart' show CameraAccess;
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart';
import 'package:factory_app/update.dart';
import 'package:factory_app/zoom.dart';
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

/// The app as main.dart places it, with nothing paired.
Future<void> _show(WidgetTester tester) async {
  final connections = Connections(
    MemoryConnectionStorage(),
    null,
    null,
    null,
    DeviceAccount(),
  );
  final roots = Claktories(
    connections: connections,
    localBackend: false,
    open: (target) => RecordingBackend(localBackend: false)
      ..connectTo(target)
      ..workspace.apply(Workspace(settings: Settings(userName: 'QA'))),
    drafts: DeviceDrafts(MemoryDraftStorage()),
  );
  final camera = FakeCamera()..state = CameraAccess.granted;
  tester.view.physicalSize = const Size(393, 852);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  addTearDown(roots.dispose);
  final entry = OverlayEntry(
    builder: (context) {
      final selected = ClaktoriesScope.of(context)!.selected;
      return PhoneClaktories(shell: Shell(key: ObjectKey(selected)));
    },
  );
  await tester.pumpWidget(
    WidgetsApp(
      color: const Color(0xff202020),
      localizationsDelegates: PhonePlatformPolicy.current.localizations,
      builder: (_, _) => ClaktoriesScope(
        claktories: roots,
        child: Builder(
          builder: (context) {
            final selected = ClaktoriesScope.of(context)!.selected;
            return AppScope(
              backend: selected.backend,
              login: selected.login,
              page: selected.page,
              appBuild: AppBuild(version: 'dev', bundlePath: '', quit: () {}),
              drafts: selected.drafts,
              history: selected.history,
              connections: connections,
              camera: camera,
              child: AppZoom(
                child: AndroidWindowBackHost(
                  window: selected,
                  child: Overlay(initialEntries: [entry]),
                ),
              ),
            );
          },
        ),
      ),
    ),
  );
  await pumpFrames(tester);
  expect(find.byType(PairScreen), findsOneWidget);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
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

  for (final way in ['Paste code instead', 'Sign in with your email']) {
    testWidgets('Back from $way goes to the first screen, then leaves', (
      tester,
    ) async {
      await _show(tester);
      await tester.tap(find.text(way));
      await pumpFrames(tester);
      expect(find.text('Send me the link'), findsNothing);
      await _back(tester);
      expect(find.text('Send me the link'), findsOneWidget);
      expect(systemPops, 0);
      await _back(tester);
      expect(systemPops, 1);
    });
  }
}
