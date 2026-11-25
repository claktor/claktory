// Friend bug #27: what quitting the Mac app does to Claktory on its Mac
// (Settings.when_app_quits), turning it off now (StopEngine), and starting
// it again (`factoryd install --start`): ⌘Q, the app menu's Stop or Start
// Claktory, Settings → General's choice and switch, and the first launch's
// question.

import 'package:factory_app/controls.dart' show Button, Select;
import 'package:factory_app/engine.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart' show stopEngineMessage;
import 'package:factory_app/setup.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';
import 'setup_test.dart' show FakeSetupChannel;

Workspace _workspace({
  bool canStop = true,
  bool stopping = false,
  QuitMode mode = QuitMode.QUIT_MODE_KEEP_RUNNING,
  OnboardingStep asks = OnboardingStep.ONBOARDING_STEP_DONE,
}) {
  final snapshot = workspace();
  snapshot.backendService = BackendService(
    state: BackendServiceState.BACKEND_SERVICE_STATE_RUNNING,
    canStop: canStop,
    stopping: stopping,
  );
  snapshot.settings.whenAppQuits = mode;
  snapshot.settings.onboarding = Onboarding(
    model: OnboardingStep.ONBOARDING_STEP_DONE,
    whenAppQuits: asks,
  );
  return snapshot;
}

Iterable<StopEngine> _stops(Harness harness) => harness.backend.sent
    .where((m) => m.hasStopEngine())
    .map((m) => m.stopEngine);

Iterable<UpdateSettings> _updates(Harness harness) => harness.backend.sent
    .where((m) => m.hasUpdateSettings())
    .map((m) => m.updateSettings);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('quitting and the app menu', () {
    const channel = MethodChannel('factory/engine-test');
    late List<Object?> menus;
    late Harness harness;
    late FakeSetupChannel setupChannel;
    late MacSetup setup;
    late EngineControl engine;

    setUp(() {
      menus = [];
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(channel, (call) async {
            if (call.method == 'menu') menus.add(call.arguments);
            return null;
          });
      harness = Harness(_workspace());
      setupChannel = FakeSetupChannel(
        results: {
          'install --check': const InstallResult(
            result: 'already_installed',
            running: true,
          ),
          'install --start': const InstallResult(result: 'started'),
        },
      );
      setup = MacSetup(
        channel: setupChannel,
        connected: harness.backend.connected,
        reconnect: harness.backend.reconnectNow,
      );
      engine = EngineControl(
        backend: harness.backend,
        setup: setup,
        channel: channel,
      );
    });

    tearDown(() {
      engine.dispose();
      setup.dispose();
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(channel, null);
    });

    test('⌘Q stops it only when the person chose that', () async {
      await engine.quitting();
      expect(_stops(harness), isEmpty, reason: 'keeps running');

      harness.backend.workspace.apply(
        _workspace(mode: QuitMode.QUIT_MODE_STOP),
      );
      await engine.quitting();
      expect(_stops(harness).single.quitting, isTrue);
    });

    test('⌘Q asks nothing of a Claktory its service doesn\'t run, or one '
        'not answering', () async {
      harness.backend.workspace.apply(
        _workspace(canStop: false, mode: QuitMode.QUIT_MODE_STOP),
      );
      await engine.quitting();
      harness.backend.workspace.apply(
        _workspace(mode: QuitMode.QUIT_MODE_STOP),
      );
      harness.backend.connected.value = false;
      await engine.quitting();
      expect(_stops(harness), isEmpty);
    });

    test('the menu: Stop Claktory while it runs, Stopping… once taken, '
        'Start Claktory once turned off, hidden otherwise', () async {
      await pumpEventQueue();
      expect(engine.item, ('Stop Claktory', true));
      expect(menus.last, {'title': 'Stop Claktory', 'enabled': true});

      expect(await engine.menuChosen(), isNull);
      expect(_stops(harness).single.quitting, isFalse);

      harness.backend.workspace.apply(_workspace(stopping: true));
      await pumpEventQueue();
      expect(menus.last, {'title': 'Stopping Claktory…', 'enabled': false});

      // Gone, and --check says the person turned it off.
      setupChannel.results['install --check'] = const InstallResult(
        result: 'stopped',
        held: true,
      );
      await setup.start();
      harness.backend.connected.value = false;
      await pumpEventQueue();
      expect(setup.stage, SetupStage.stopped);
      expect(menus.last, {'title': 'Start Claktory', 'enabled': true});

      expect(await engine.menuChosen(), isNull);
      expect(setupChannel.runs.last, 'install --start');

      harness.backend.workspace.apply(_workspace(canStop: false));
      harness.backend.connected.value = true;
      await pumpEventQueue();
      expect(engine.item, isNull);
      expect(menus.last, isNull);
    });

    test('a refused stop comes back in words, for an alert', () async {
      harness.backend.refuse =
          'Claktory is finishing an update. Try again in a minute.';
      expect(
        await engine.menuChosen(),
        'Claktory is finishing an update. Try again in a minute.',
      );
    });
  });

  group('Settings → General', () {
    Future<Harness> general(WidgetTester tester, Workspace snapshot) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final harness = Harness(snapshot);
      await harness.show(tester, size: const Size(1200, 900));
      await pumpFrames(tester);
      harness.page.value = AppPage.settings;
      await pumpFrames(tester);
      return harness;
    }

    testWidgets('where its service runs it: the choice and the switch; '
        'choosing sends it, turning off asks first', (tester) async {
      final harness = await general(tester, _workspace());
      expect(find.text('When I quit Claktory'), findsOneWidget);
      expect(find.text('Keep agents running'), findsOneWidget);
      expect(find.text('Claktory on this Mac'), findsOneWidget);
      expect(find.text('Running'), findsOneWidget);

      await tester.tap(find.byType(Select<QuitMode>));
      await pumpFrames(tester);
      await tester.tap(find.text('Stop agents').last);
      await pumpFrames(tester);
      expect(_updates(harness).last.whenAppQuits, QuitMode.QUIT_MODE_STOP);

      await tester.tap(find.text('Running'));
      await pumpFrames(tester);
      expect(find.text(stopEngineMessage), findsOneWidget);
      expect(_stops(harness), isEmpty, reason: 'asks first');
      await tester.tap(find.text('Turn Off'));
      await pumpFrames(tester);
      expect(_stops(harness).single.quitting, isFalse);

      harness.backend.workspace.apply(_workspace(stopping: true));
      await pumpFrames(tester);
      expect(find.text('Stopping…'), findsOneWidget);
    });

    testWidgets('nothing where its service doesn\'t run it', (tester) async {
      await general(tester, _workspace(canStop: false));
      expect(find.text('When I quit Claktory'), findsNothing);
      expect(find.text('Claktory on this Mac'), findsNothing);
    });
  });

  group("the first launch's question", () {
    testWidgets('a new Claktory on this Mac asks; each answer is sent', (
      tester,
    ) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final harness = Harness(
        _workspace(asks: OnboardingStep.ONBOARDING_STEP_NEEDED),
      );
      await harness.show(tester, size: const Size(1200, 900));
      await pumpFrames(tester);
      expect(find.byType(WhenAppQuitsStep), findsOneWidget);
      expect(find.text('When you quit Claktory'), findsOneWidget);
      await tester.tap(
        find.byWidgetPredicate(
          (w) => w is Button && w.label == 'Keep agents running',
        ),
      );
      await pumpFrames(tester);
      expect(
        _updates(harness).single.whenAppQuits,
        QuitMode.QUIT_MODE_KEEP_RUNNING,
      );

      // Answered: done on every device.
      harness.backend.workspace.apply(_workspace());
      await pumpFrames(tester);
      expect(find.byType(WhenAppQuitsStep), findsNothing);
    });

    testWidgets('not on a phone, nor once answered', (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final harness = Harness(
        _workspace(asks: OnboardingStep.ONBOARDING_STEP_NEEDED),
      );
      await harness.show(tester);
      await pumpFrames(tester);
      expect(find.byType(WhenAppQuitsStep), findsNothing);
    });
  });
}
