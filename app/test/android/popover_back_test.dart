// Actual microphone Before popover under Android's single window Back host.
// RecordingBackend/FakeRecorder and framework channel events only: these are
// not native permission, OS-gesture, IME or physical-phone acceptance.
import 'dart:async';

import 'package:factory_app/agent_view.dart';
import 'package:factory_app/android/back_host.dart';
import 'package:factory_app/android/window_back_host.dart';
import 'package:factory_app/mobile/back.dart';
import 'package:factory_app/mobile/drawer.dart';
import 'package:factory_app/mobile/permission.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/mobile/popover.dart';
import 'package:factory_app/mobile/voice_input.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recorder.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart';
import 'package:factory_app/update.dart';
import 'package:factory_app/voice.dart';
import 'package:factory_app/voice_session.dart';
import 'package:factory_app/zoom.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../voice_test.dart' show FakeRecorder;
import 'support/channel_fakes.dart';

const _draft = 'Unsent 日本語 👩🏽‍💻';
final _mic = find.byType(PhoneMicButton);
final _before = find.text('Talk instead of typing');

Future<void> _button() async {
  await sendNativeMethod(
    SystemChannels.navigation,
    const MethodCall('popRoute'),
  );
}

Future<Object?> _gesture(String method, {double progress = 0, int edge = 0}) =>
    sendNativeMethod(
      SystemChannels.backGesture,
      MethodCall(
        method,
        method == 'startBackGesture' || method == 'updateBackGestureProgress'
            ? {
                'touchOffset': [edge == 0 ? 1.0 : 392.0, 300.0],
                'progress': progress,
                'swipeEdge': edge,
              }
            : null,
      ),
    );

class _App {
  final window = ValueNotifier<int>(0);
  final harness = Harness(
    Workspace(
      settings: Settings(userName: 'QA'),
      projects: [Project(id: 'p', name: 'Claktory')],
      agents: [
        Agent(
          id: 'mike',
          name: 'Mike',
          role: 'Coordinator',
          projectId: 'p',
          modelAvailable: true,
        ),
      ],
      selectedAgentId: 'mike',
      speechModel: SpeechModel(
        state: SpeechModelState.SPEECH_MODEL_STATE_READY,
      ),
    ),
  );

  late PhoneBack back;

  Future<void> show(WidgetTester tester) async {
    harness.backend.transcript.load(Transcript(agentId: 'mike'));
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    tester.view.physicalSize = const Size(393, 852);
    tester.view.devicePixelRatio = 1;
    tester.view.display.size = const Size(393, 852);
    tester.view.display.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    addTearDown(tester.view.display.reset);
    addTearDown(window.dispose);
    // Main's placement: one host above the Shell's real window Overlay.
    // The drawer borrows its registry, including for overlay siblings.
    await tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xff202020),
        localizationsDelegates: PhonePlatformPolicy.current.localizations,
        builder: (context, _) => AppScope(
          backend: harness.backend,
          login: harness.login,
          page: harness.page,
          drafts: harness.drafts,
          appBuild: AppBuild(
            version: 'dev',
            bundlePath: '',
            quit: () {},
            updater: harness.updater,
          ),
          history: harness.history,
          connections: harness.connections,
          camera: harness.camera,
          child: AppZoom(
            child: VoiceBarHost(
              child: ValueListenableBuilder<int>(
                valueListenable: window,
                builder: (context, current, _) => AndroidWindowBackHost(
                  window: current,
                  child: Shell(key: ValueKey(current)),
                ),
              ),
            ),
          ),
        ),
      ),
    );
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsOneWidget);
    expect(_mic, findsOneWidget);
    expect(find.byWidgetPredicate((w) => w is AndroidBackHost), findsOneWidget);
    back = PhoneBack.maybeOf(_mic.evaluate().single)!;
    expect(back.target, isNull, reason: 'conversation is at its root');
  }

  Future<void> openMic(WidgetTester tester) async {
    await tester.tap(_mic);
    await pumpFrames(tester);
    expectBefore();
  }

  void expectBefore() {
    expect(_before, findsOneWidget);
    expect(find.text('Continue'), findsOneWidget);
    final content = find.byType(PermissionContent);
    expect(content, findsOneWidget);
    final widget = content.evaluate().single.widget as PermissionContent;
    expect(widget.compact, isTrue, reason: 'actual mic popover, not a sheet');
    expect(widget.flow.stage, PermissionStage.before);
  }

  void expectConversation() {
    expect(find.byType(AgentView), findsOneWidget);
    expect(
      testerDrawer.drawerOpen,
      isFalse,
      reason: 'Back did not reveal or open the drawer',
    );
    expect(harness.backend.sent.where((m) => m.hasSendMessage()), isEmpty);
  }

  PhoneDrawerHostState get testerDrawer =>
      (find.byType(PhoneDrawerHost).evaluate().single as StatefulElement).state
          as PhoneDrawerHostState;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final android = TargetPlatformVariant.only(TargetPlatform.android);
  late FakeRecorder recorder;
  late VoiceRecorder previousRecorder;
  late bool previousPolicy;
  late bool previousVoiceMessages;
  final platformCalls = <MethodCall>[];
  int systemPops() => platformCalls
      .where((call) => call.method == 'SystemNavigator.pop')
      .length;

  setUp(() {
    previousPolicy = PhonePlatformPolicy.debugAndroidPolicy;
    previousVoiceMessages = voiceMessagesOn;
    PhonePlatformPolicy.debugAndroidPolicy = true;
    voiceMessagesOn = true;
    previousRecorder = deviceRecorder;
    recorder = FakeRecorder(granted: MicAccess.undetermined);
    deviceRecorder = recorder;
    platformCalls.clear();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, (call) async {
          platformCalls.add(call);
          return null;
        });
  });
  tearDown(() {
    deviceRecorder = previousRecorder;
    PhonePlatformPolicy.debugAndroidPolicy = previousPolicy;
    voiceMessagesOn = previousVoiceMessages;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, null);
  });

  testWidgets(
    'button Back closes actual microphone Before, never the app or conversation',
    (tester) async {
      final app = _App();
      await app.show(tester);
      await app.openMic(tester);
      final occurrence = app.back.target;
      await _button();
      await pumpFrames(tester);
      expect(
        systemPops(),
        0,
        reason: 'the popover, not Android root, took Back',
      );
      expect(_before, findsNothing);
      expect(occurrence, isNotNull, reason: 'popover owns window Back');
      expect(
        platformCalls.where(
          (call) =>
              call.method == 'SystemNavigator.setFrameworkHandlesBack' &&
              call.arguments == true,
        ),
        isNotEmpty,
      );
      expect(app.back.target, isNull);
      app.expectConversation();
      expect(systemPops(), 0);
      expect(recorder.requests, isEmpty);
      expect(recorder.recordingInto, isNull);
      // Only a separate Back may now reach Android's root action.
      await _button();
      await tester.pump();
      expect(systemPops(), 1);
    },
    variant: android,
  );

  for (final edge in [0, 1]) {
    testWidgets(
      'cancelled edge $edge Back preserves microphone Before and draft',
      (tester) async {
        final app = _App();
        await app.show(tester);
        await tester.enterText(find.byType(EditableText).last, _draft);
        await app.openMic(tester);
        final occurrence = app.back.target;
        expect(await _gesture('startBackGesture', edge: edge), isTrue);
        await _gesture('updateBackGestureProgress', progress: 0.7, edge: edge);
        await _gesture('cancelBackGesture');
        await pumpFrames(tester);
        app.expectBefore();
        app.expectConversation();
        expect(app.back.target, equals(occurrence));
        expect(app.harness.drafts.of('mike'), _draft);
        expect(recorder.requests, isEmpty);
        expect(recorder.recordingInto, isNull);
        expect(systemPops(), 0);
        // Finish the deliberately preserved attempt while its composer is
        // alive, rather than leave an unresolved permission for teardown.
        await _button();
        await pumpFrames(tester);
        expect(_before, findsNothing);
        expect(find.text(_draft), findsOneWidget);
        expect(systemPops(), 0);
      },
      variant: android,
    );
  }

  testWidgets('predictive commit dismisses only microphone Before', (
    tester,
  ) async {
    final app = _App();
    await app.show(tester);
    await app.openMic(tester);
    expect(await _gesture('startBackGesture'), isTrue);
    await _gesture('updateBackGestureProgress', progress: 0.8);
    await _gesture('commitBackGesture');
    await pumpFrames(tester);
    expect(_before, findsNothing);
    app.expectConversation();
    expect(app.back.target, isNull);
    expect(systemPops(), 0);
    expect(recorder.requests, isEmpty);
  }, variant: android);

  testWidgets('Back closes only the top popover above microphone Before', (
    tester,
  ) async {
    final app = _App();
    await app.show(tester);
    await app.openMic(tester);
    final microphone = app.back.target;
    unawaited(
      presentPopover<void>(
        tester.element(find.byType(PermissionContent)),
        builder: (context, close) => const Text('Second popover'),
      ),
    );
    await pumpFrames(tester);
    expect(find.text('Second popover'), findsOneWidget);
    expect(app.back.target, isNot(equals(microphone)));
    await _button();
    await pumpFrames(tester);
    expect(find.text('Second popover'), findsNothing);
    app.expectBefore();
    expect(app.back.target, equals(microphone));
    expect(systemPops(), 0);
    await _button();
    await pumpFrames(tester);
    expect(_before, findsNothing);
    app.expectConversation();
    expect(systemPops(), 0);
  }, variant: android);

  testWidgets(
    'a Back captured before outside-close cannot dismiss its replacement',
    (tester) async {
      final app = _App();
      await app.show(tester);
      await app.openMic(tester);
      final first = app.back.target;
      expect(await _gesture('startBackGesture'), isTrue);
      await tester.tapAt(const Offset(20, 100));
      await pumpFrames(tester);
      expect(_before, findsNothing);
      expect(app.back.target, isNull);
      await app.openMic(tester);
      expect(app.back.target, isNot(equals(first)));
      await _gesture('commitBackGesture');
      await pumpFrames(tester);
      app.expectBefore();
      app.expectConversation();
      expect(systemPops(), 0);
      await _button();
      await pumpFrames(tester);
      expect(_before, findsNothing);
      expect(recorder.requests, isEmpty);
    },
    variant: android,
  );

  for (final predictive in [false, true]) {
    testWidgets(
      '${predictive ? 'predictive' : 'button'} IME Back cannot close microphone popover or page',
      (tester) async {
        final app = _App();
        await app.show(tester);
        await tester.enterText(find.byType(EditableText).last, _draft);
        tester.view.viewInsets = const FakeViewPadding(bottom: 280);
        await pumpFrames(tester);
        await app.openMic(tester);
        final occurrence = app.back.target;
        final hiddenBefore = tester.testTextInput.log
            .where((call) => call.method == 'TextInput.hide')
            .length;
        if (predictive) {
          expect(await _gesture('startBackGesture'), isTrue);
          // Android hides the keyboard before the gesture terminates. The
          // captured owner must remain the IME, not fall through to the UI.
          tester.view.viewInsets = FakeViewPadding.zero;
          await tester.pump();
          await _gesture('commitBackGesture');
        } else {
          await _button();
          // Framework inset only: no claim a real Android IME was hidden.
          tester.view.viewInsets = FakeViewPadding.zero;
        }
        await pumpFrames(tester);
        expect(
          tester.testTextInput.log
              .where((call) => call.method == 'TextInput.hide')
              .length,
          hiddenBefore + 1,
        );
        app.expectBefore();
        app.expectConversation();
        expect(app.back.target, equals(occurrence));
        expect(app.harness.drafts.of('mike'), _draft);
        expect(systemPops(), 0);
        await _button();
        await pumpFrames(tester);
        expect(_before, findsNothing);
        app.expectConversation();
        expect(find.text(_draft), findsOneWidget);
        expect(systemPops(), 0);
        expect(recorder.requests, isEmpty);
      },
      variant: android,
    );
  }

  testWidgets(
    'retiring the originating window removes its popover and captured Back',
    (tester) async {
      final app = _App();
      await app.show(tester);
      await app.openMic(tester);
      expect(await _gesture('startBackGesture'), isTrue);
      app.window.value++;
      await pumpFrames(tester);
      expect(_before, findsNothing, reason: 'going window retires its overlay');
      expect(app.back.target, isNull);
      await app.openMic(tester);
      await _gesture('commitBackGesture');
      await pumpFrames(tester);
      app.expectBefore();
      app.expectConversation();
      expect(systemPops(), 0);
      await _button();
      await pumpFrames(tester);
      expect(_before, findsNothing);
      expect(recorder.requests, isEmpty);
      expect(tester.takeException(), isNull);
    },
    variant: android,
  );
}
