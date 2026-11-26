// Real shared blocked/onboarding screens under main's Android Back host.
// Channel-driven gestures prove routing, not native/emulator acceptance.
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/android/window_back_host.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/controls.dart';
import 'package:factory_app/device_account.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/mobile/back.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/name_step.dart';
import 'package:factory_app/pairing.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart';
import 'package:factory_app/update.dart';
import 'package:factory_app/your_claktories.dart';
import 'package:factory_app/zoom.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../tunnel_test.dart' show tunnelStatus;
import 'support/channel_fakes.dart';

final _origin = 'factory://?t=${'a' * 64}&name=Origin&e=${'c' * 64}';
final _other = 'factory://?t=${'b' * 64}&name=Other&e=${'d' * 64}';

enum _Screen { signInRequired, signedOut, otherAccount, pairing, name, signIn }

class _App {
  final connections = Connections(
    MemoryConnectionStorage(),
    null,
    null,
    null,
    DeviceAccount(),
  );
  final drafts = DeviceDrafts(MemoryDraftStorage());
  final backends = <String, RecordingBackend>{};
  final camera = FakeCamera();
  late final roots = Claktories(
    connections: connections,
    localBackend: false,
    drafts: drafts,
    open: (target) {
      final backend = RecordingBackend(localBackend: false)..connectTo(target);
      backend.workspace.apply(
        Workspace(
          agents: [Agent(id: 'agent', name: 'QA agent', modelAvailable: true)],
          selectedAgentId: 'agent',
          settings: Settings(userName: 'QA', name: target?.name ?? 'Unpaired'),
        ),
      );
      return backends[target?.code ?? ''] = backend;
    },
  );
  RecordingBackend get backend => backends[_origin]!;

  Future<void> show(WidgetTester tester, {_Screen? screen}) async {
    connections
      ..use(parsePairingCode(_other))
      ..use(parsePairingCode(_origin));
    final claktories = roots;
    if (screen != null) block(screen);
    tester.view.physicalSize = const Size(393, 852);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    addTearDown(claktories.dispose);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    final entry = OverlayEntry(
      builder: (context) => PhoneClaktories(
        shell: Shell(key: ObjectKey(ClaktoriesScope.of(context)!.selected)),
      ),
    );
    await tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xff202020),
        localizationsDelegates: PhonePlatformPolicy.current.localizations,
        builder: (_, _) => ClaktoriesScope(
          claktories: claktories,
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
  }

  void block(_Screen screen) {
    switch (screen) {
      case _Screen.signInRequired:
        backend.connected.value = false;
        backend.signInRequired.value = 'Sign in before this phone can connect.';
      case _Screen.signedOut:
        backend.connected.value = false;
        backend.signInRequired.value =
            'Origin was signed out of your Claktory account.';
      case _Screen.otherAccount:
        backend.connected.value = false;
        backend.otherAccount.value = 'This Claktory uses another account.';
        backend.refused.value = 'This Claktory uses another account.';
      case _Screen.pairing:
        backend.connected.value = false;
        backend.pairing.value = PairingWait(
          PairingPending(deviceId: 'qa-device', name: 'Origin'),
          '1234',
        );
      case _Screen.name:
        backend.workspace.apply(
          backend.workspace.workspace.deepCopy()
            ..ensureSettings().userName = '',
        );
      case _Screen.signIn:
        backend.workspace.apply(
          backend.workspace.workspace.deepCopy()
            ..account = ClaktoryAccount(
              state: ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_OUT,
            ),
        );
    }
  }
}

Future<void> _button() =>
    sendNativeMethod(SystemChannels.navigation, const MethodCall('popRoute'));
Future<Object?> _gesture(String method, {double progress = 0}) =>
    sendNativeMethod(
      SystemChannels.backGesture,
      MethodCall(
        method,
        method == 'startBackGesture' || method == 'updateBackGestureProgress'
            ? {
                'touchOffset': [1.0, 300.0],
                'progress': progress,
                'swipeEdge': 0,
              }
            : null,
      ),
    );

void _expectScreen(_Screen screen) {
  expect(find.byType(ClaktoryList), findsNothing);
  expect(find.byType(ClaktoriesBackStrip), findsWidgets);
  expect(
    find.bySemanticsLabel(RegExp(r'^Back to Claktories\b')),
    findsOneWidget,
  );
  switch (screen) {
    case _Screen.signInRequired:
      expect(find.byType(SignInRequiredView), findsOneWidget);
      expect(find.text('Sign in to connect to Origin'), findsOneWidget);
    case _Screen.signedOut:
      expect(find.text('Origin was signed out'), findsOneWidget);
    case _Screen.otherAccount:
      expect(find.byType(OtherAccountView), findsOneWidget);
      expect(
        find.text('Origin is on another Claktory account'),
        findsOneWidget,
      );
    case _Screen.pairing:
      expect(find.byType(PairingWaitView), findsOneWidget);
    case _Screen.name:
      expect(find.byType(NameStep), findsOneWidget);
    case _Screen.signIn:
      expect(find.text('Sign in to Claktory'), findsOneWidget);
  }
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

  for (final screen in _Screen.values) {
    for (final gesture in [false, true]) {
      testWidgets(
        '${screen.name} ${gesture ? 'gesture' : 'button'} returns to Claktories without forgetting it',
        (tester) async {
          final app = _App();
          await app.show(tester, screen: screen);
          _expectScreen(screen);
          final saved = app.connections.saved.map((s) => s.code).toList();
          if (gesture) {
            expect(await _gesture('startBackGesture'), isTrue);
            await _gesture('updateBackGestureProgress', progress: .8);
            await _gesture('cancelBackGesture');
            await pumpFrames(tester);
            _expectScreen(screen);
            expect(systemPops, 0);
            expect(await _gesture('startBackGesture'), isTrue);
            await _gesture('commitBackGesture');
          } else {
            await _button();
          }
          await pumpFrames(tester);
          expect(find.byType(ClaktoryList), findsOneWidget);
          expect(find.text('Add a Claktory'), findsOneWidget);
          expect(app.connections.saved.map((s) => s.code), saved);
          expect(app.connections.current?.code, _origin);
          expect(systemPops, 0);
        },
      );
    }
  }

  testWidgets(
    'IME Back hides only the keyboard; cancelled gesture keeps unsent name',
    (tester) async {
      final app = _App();
      await app.show(tester, screen: _Screen.name);
      final field = find.descendant(
        of: find.byType(NameStep),
        matching: find.byType(EditableText),
      );
      expect(field, findsOneWidget);
      await tester.enterText(field, 'Unsent 日本語');
      await tester.pump();
      tester.view.viewInsets = const FakeViewPadding(bottom: 300);
      await tester.pump();
      tester.testTextInput.log.clear();
      await _button();
      await tester.pump();
      expect(
        tester.testTextInput.log.where((c) => c.method == 'TextInput.hide'),
        hasLength(1),
      );
      expect(find.byType(NameStep), findsOneWidget);
      expect(tester.widget<EditableText>(field).controller.text, 'Unsent 日本語');
      expect(find.byType(ClaktoryList), findsNothing);
      expect(systemPops, 0);
      tester.view.viewInsets = const FakeViewPadding();
      await tester.pump();
      expect(await _gesture('startBackGesture'), isTrue);
      await _gesture('cancelBackGesture');
      await tester.pump();
      expect(tester.widget<EditableText>(field).controller.text, 'Unsent 日本語');
      await _button();
      await pumpFrames(tester);
      expect(find.byType(ClaktoryList), findsOneWidget);
      expect(systemPops, 0);
    },
  );

  testWidgets('a sheet stays above a rebuilt full-screen strip', (
    tester,
  ) async {
    final app = _App();
    await app.show(tester, screen: _Screen.signInRequired);
    final context = tester.element(find.byType(SignInRequiredView));
    showModal(
      context,
      (context, close) => Button(label: 'Sheet close', onPressed: close),
    );
    await pumpFrames(tester);
    app.backend.signInRequired.value = 'Changed explanation.';
    await pumpFrames(tester);
    await _button();
    await pumpFrames(tester);
    expect(find.text('Sheet close'), findsNothing);
    expect(find.byType(SignInRequiredView), findsOneWidget);
    expect(find.byType(ClaktoryList), findsNothing);
    expect(systemPops, 0);
    await _button();
    await pumpFrames(tester);
    expect(find.byType(ClaktoryList), findsOneWidget);
    expect(systemPops, 0);
  });

  testWidgets('retired strip cannot navigate a recovered conversation', (
    tester,
  ) async {
    final app = _App();
    await app.show(tester, screen: _Screen.signInRequired);
    expect(await _gesture('startBackGesture'), isTrue);
    app.backend.signInRequired.value = null;
    app.backend.connected.value = true;
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsOneWidget);
    await _gesture('commitBackGesture');
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsOneWidget);
    expect(find.byType(ClaktoryList), findsNothing);
    expect(systemPops, 0);
    // Ordinary root Back still belongs to Android after the strip goes.
    await _button();
    await tester.pump();
    expect(systemPops, 1);
  });

  testWidgets("can't connect keeps Menu for switching Claktories", (
    tester,
  ) async {
    final app = _App();
    await app.show(tester);
    app.backend.workspace.apply(
      app.backend.workspace.workspace.deepCopy()..selectedAgentId = '',
    );
    app.backend.connected.value = false;
    app.backend.lastFailure.value = tunnelStatus();
    await pumpFrames(tester);
    expect(find.textContaining("Can't connect"), findsWidgets);
    expect(find.bySemanticsLabel('Menu'), findsOneWidget);
    await tester.tap(find.bySemanticsLabel('Menu'));
    await pumpFrames(tester);
    final other = find.byWidgetPredicate(
      (widget) =>
          widget is Semantics &&
          (widget.properties.label?.startsWith('Other') ?? false),
    );
    expect(other, findsOneWidget);
    await tester.tap(other);
    await pumpFrames(tester);
    expect(app.connections.current?.code, _other);
    expect(app.connections.saved.map((s) => s.code), contains(_origin));
    await _button();
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsOneWidget);
    expect(find.bySemanticsLabel('Close menu'), findsNothing);
    expect(systemPops, 0);
  });

  testWidgets('a rebuilt blocked screen keeps the current gesture target', (
    tester,
  ) async {
    final app = _App();
    await app.show(tester, screen: _Screen.signInRequired);
    expect(await _gesture('startBackGesture'), isTrue);
    app.backend.signInRequired.value = 'A newer explanation.';
    await pumpFrames(tester);
    await _gesture('commitBackGesture');
    await pumpFrames(tester);
    expect(find.byType(ClaktoryList), findsOneWidget);
    expect(systemPops, 0);
  });

  testWidgets(
    'a stale blocked-screen gesture cannot leave a replacement Claktory',
    (tester) async {
      final app = _App();
      await app.show(tester, screen: _Screen.signInRequired);
      expect(await _gesture('startBackGesture'), isTrue);
      app.connections.use(parsePairingCode(_other));
      await pumpFrames(tester);
      expect(find.byType(AgentView), findsOneWidget);
      await _gesture('commitBackGesture');
      await pumpFrames(tester);
      expect(app.connections.current?.code, _other);
      expect(find.byType(ClaktoryList), findsNothing);
      expect(find.byType(AgentView), findsOneWidget);
      expect(systemPops, 0);
    },
  );

  testWidgets('iPhone strip does not register a system Back target', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final app = _App();
    await app.show(tester, screen: _Screen.signInRequired);
    final context = tester.element(find.byType(SignInRequiredView));
    expect(PhoneBack.maybeOf(context)?.target, isNull);
    expect(find.byType(ClaktoriesBackStrip), findsOneWidget);
  });
}
