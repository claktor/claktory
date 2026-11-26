// The permission sheet (mobile/permission.dart, area 5a): one way to ask
// for every permission. Before the system's prompt, why, with Continue
// only; declined, what stops and Open Settings with a quiet way out;
// restricted, the way out only; back from Settings with it on, done by
// itself. The microphone's explainer is a popover over the mic.
import 'dart:async';

import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/mobile/device_permissions.dart';
import 'package:factory_app/mobile/permission.dart';
import 'package:factory_app/mobile/scroll_fade.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recorder.dart' show MicAccess, VoiceRecorder;
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';

const _phone = Size(390, 844);

/// A permission as the test answers it: [access] says [now]; a request
/// waits for [answer].
class _Fake extends Permission {
  _Fake(this.now);

  PermissionAccess now;
  var requests = 0;
  var settingsOpened = 0;
  Completer<PermissionAccess>? _pending;

  bool get asking => _pending != null;

  /// The system's answer; [then] is what access says after it.
  void answer(PermissionAccess answer, {PermissionAccess? then}) {
    now = then ?? answer;
    final pending = _pending!;
    _pending = null;
    pending.complete(answer);
  }

  @override
  PermissionWords words(BuildContext context) => const PermissionWords(
    glyph: 'bell',
    title: 'Before title',
    body: 'Before body. Your iPhone asks next.',
    offTitle: 'Off title',
    offBody: 'What stops.',
    offCaption: 'To turn it on: here.',
  );

  @override
  Future<PermissionAccess> access() async => now;

  @override
  Future<PermissionAccess> request() {
    requests++;
    return (_pending = Completer()).future;
  }

  @override
  Future<void> openSettings() async => settingsOpened++;
}

/// One whose Before words run past a small screen at large text.
class _Long extends _Fake {
  _Long(super.now);

  @override
  PermissionWords words(BuildContext context) => PermissionWords(
    glyph: 'bell',
    title: 'Before title',
    body: List.filled(30, 'Before body.').join(' '),
    offTitle: 'Off title',
    offBody: 'What stops.',
  );
}

class _FakeRecorder implements VoiceRecorder {
  _FakeRecorder(this.now);

  MicAccess now;
  var requests = 0;
  Completer<bool>? pending;

  @override
  Future<MicAccess> access() async => now;

  @override
  Future<bool> request() {
    requests++;
    return (pending = Completer()).future;
  }

  @override
  Future<void> openSettings() async {}

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

Finder _button(String label) => find.widgetWithText(Button, label);

void main() {
  void onIphone() {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
  }

  /// A screen to ask from: its context.
  Future<BuildContext> screen(
    WidgetTester tester, {
    Size size = _phone,
    double text = 1,
  }) async {
    late BuildContext context;
    await Harness(Workspace()).show(
      tester,
      size: size,
      home: MediaQuery(
        data: MediaQueryData(size: size, textScaler: TextScaler.linear(text)),
        child: Builder(
          builder: (c) {
            context = c;
            return const Align(
              alignment: Alignment.bottomRight,
              child: SizedBox(width: 44, height: 44),
            );
          },
        ),
      ),
    );
    await pumpFrames(tester);
    return context;
  }

  group('the flow', () {
    test('where each answer starts it', () {
      expect(
        PermissionFlow.stageFor(PermissionAccess.undetermined),
        PermissionStage.before,
      );
      expect(
        PermissionFlow.stageFor(PermissionAccess.unknown),
        PermissionStage.before,
      );
      expect(
        PermissionFlow.stageFor(PermissionAccess.denied),
        PermissionStage.declined,
      );
      expect(
        PermissionFlow.stageFor(PermissionAccess.restricted),
        PermissionStage.restricted,
      );
      expect(PermissionFlow.stageFor(PermissionAccess.granted), isNull);
      expect(PermissionFlow.stageFor(PermissionAccess.unavailable), isNull);
    });

    test('Continue asks once; allowed, done', () async {
      final permission = _Fake(PermissionAccess.undetermined);
      final flow = PermissionFlow(permission);
      final going = flow.proceed();
      expect(flow.stage, PermissionStage.asking);
      expect(flow.held, isTrue);
      unawaited(flow.proceed());
      expect(permission.requests, 1, reason: 'one prompt at a time');
      permission.answer(PermissionAccess.granted);
      await going;
      expect(flow.stage, PermissionStage.done);
      expect(await flow.done, PermissionAccess.granted);
    });

    test('a no reads access again: declined, or Before again where it '
        'can still ask (Android after a first no)', () async {
      final permission = _Fake(PermissionAccess.undetermined);
      final flow = PermissionFlow(permission);
      var going = flow.proceed();
      permission.answer(
        PermissionAccess.denied,
        then: PermissionAccess.undetermined,
      );
      await going;
      expect(flow.stage, PermissionStage.before);
      going = flow.proceed();
      permission.answer(PermissionAccess.denied);
      await going;
      expect(flow.stage, PermissionStage.declined);
      expect(flow.held, isFalse);
    });

    test('back from Settings with it on, done by itself', () async {
      final permission = _Fake(PermissionAccess.denied);
      final flow = PermissionFlow(permission, from: PermissionAccess.denied);
      await flow.recheck();
      expect(flow.stage, PermissionStage.declined);
      permission.now = PermissionAccess.granted;
      await flow.recheck();
      expect(flow.stage, PermissionStage.done);
      expect(flow.result, PermissionAccess.granted);
    });

    test('unreadable (Local Network): a check answers on coming back, and '
        'an unsure answer is taken as on', () async {
      final permission = _Fake(PermissionAccess.unknown);
      final flow = PermissionFlow(permission, from: PermissionAccess.unknown);
      var going = flow.proceed();
      permission.answer(
        PermissionAccess.denied,
        then: PermissionAccess.unknown,
      );
      await going;
      expect(flow.stage, PermissionStage.declined);
      going = flow.recheck();
      await Future<void>.delayed(Duration.zero);
      expect(permission.requests, 2, reason: 'the check, after Settings');
      permission.answer(PermissionAccess.unknown);
      await going;
      expect(flow.stage, PermissionStage.declined, reason: 'still unsure');
      final again = PermissionFlow(permission, from: PermissionAccess.unknown);
      going = again.proceed();
      permission.answer(PermissionAccess.unknown);
      await going;
      expect(again.result, PermissionAccess.granted);
    });
  });

  testWidgets('allowed already, or nothing to ask for: no sheet', (
    tester,
  ) async {
    onIphone();
    final context = await screen(tester);
    for (final access in [
      PermissionAccess.granted,
      PermissionAccess.unavailable,
    ]) {
      final answer = await askPermission(context, _Fake(access));
      expect(answer, access);
      await pumpFrames(tester);
      expect(find.text('Before title'), findsNothing);
    }
  });

  testWidgets('on a phone: Before is held with Continue only; declined, '
      'Open Settings and Not now; back from Settings with it on, it closes', (
    tester,
  ) async {
    onIphone();
    final semantics = tester.ensureSemantics();
    final context = await screen(tester);
    final permission = _Fake(PermissionAccess.undetermined);
    PermissionAccess? answer;
    unawaited(askPermission(context, permission).then((a) => answer = a));
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Before title'), findsOneWidget);
    expect(_button('Continue'), findsOneWidget);
    expect(find.text('Not now'), findsNothing);
    expect(find.bySemanticsLabel('Close'), findsNothing, reason: 'held');

    await tester.tap(_button('Continue'));
    await pumpFrames(tester);
    expect(permission.asking, isTrue);
    expect(tester.widget<Button>(_button('Continue')).onPressed, isNull);
    // Dimmed, Continue is read as itself, not over the whole sheet.
    final dimmed = tester.getSemantics(_button('Continue'));
    expect(dimmed.rect.height, lessThan(100));

    permission.answer(PermissionAccess.denied);
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Off title'), findsOneWidget);
    expect(find.text('To turn it on: here.'), findsOneWidget);
    await tester.tap(_button('Open Settings'));
    await pumpFrames(tester);
    expect(permission.settingsOpened, 1);

    // Turned on in Settings, back in the app.
    permission.now = PermissionAccess.granted;
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Off title'), findsNothing);
    expect(answer, PermissionAccess.granted);
    semantics.dispose();
  });

  testWidgets('scrolled down in Before at a large size, Declined opens at '
      'its top, title in sight (Zucchero row 619)', (tester) async {
    onIphone();
    final context = await screen(
      tester,
      size: const Size(320, 568),
      text: 2.35,
    );
    final permission = _Long(PermissionAccess.undetermined);
    unawaited(askPermission(context, permission));
    await pumpFrames(tester, const Duration(seconds: 1));
    final scroll = find.descendant(
      of: find.byType(ScrollsWithFade),
      matching: find.byType(Scrollable),
    );
    final before = tester.state<ScrollableState>(scroll).position;
    expect(before.maxScrollExtent, greaterThan(0), reason: 'it scrolls');
    before.jumpTo(before.maxScrollExtent);
    await pumpFrames(tester);
    await tester.tap(_button('Continue'));
    await pumpFrames(tester);
    permission.answer(PermissionAccess.denied);
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Off title'), findsOneWidget);
    expect(tester.state<ScrollableState>(scroll).position.pixels, 0);
  });

  testWidgets('refused already: straight to Declined; Not now goes on', (
    tester,
  ) async {
    onIphone();
    final context = await screen(tester);
    final permission = _Fake(PermissionAccess.denied);
    PermissionAccess? answer;
    unawaited(askPermission(context, permission).then((a) => answer = a));
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Before title'), findsNothing);
    expect(find.text('Off title'), findsOneWidget);
    await tester.tap(find.text('Not now'));
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Off title'), findsNothing);
    expect(answer, PermissionAccess.denied);
    expect(permission.requests, 0);
  });

  testWidgets('restricted: no Open Settings, only the way out', (
    tester,
  ) async {
    onIphone();
    final context = await screen(tester);
    var pasted = 0;
    unawaited(
      askPermission(
        context,
        _Fake(PermissionAccess.restricted),
        wayOut: PermissionWayOut('Paste code instead', () => pasted++),
      ),
    );
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Off title'), findsOneWidget);
    expect(find.textContaining("so it can't be changed here"), findsOneWidget);
    expect(find.text('Open Settings'), findsNothing);
    await tester.tap(find.text('Paste code instead'));
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(pasted, 1);
    expect(find.text('Off title'), findsNothing);
  });

  testWidgets('on an iPad, a dialog: Return presses the way on; Esc does '
      'nothing while it explains, and is Not now once declined', (
    tester,
  ) async {
    asIPad();
    final context = await screen(tester, size: const Size(1024, 1366));
    final permission = _Fake(PermissionAccess.undetermined);
    unawaited(askPermission(context, permission));
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Before title'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await pumpFrames(tester);
    expect(find.text('Before title'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await pumpFrames(tester);
    expect(permission.requests, 1);
    permission.answer(PermissionAccess.denied);
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Off title'), findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await pumpFrames(tester);
    expect(permission.settingsOpened, 1);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('Off title'), findsNothing);
  });

  group('the microphone', () {
    testWidgets('allowed already: record, nothing shown', (tester) async {
      onIphone();
      final context = await screen(tester);
      final recorder = _FakeRecorder(MicAccess.granted);
      expect(
        await askMicrophone(context, recorder: recorder),
        MicReady.ready,
      );
      expect(recorder.requests, 0);
    });

    testWidgets('the first time, a popover over the mic; Continue asks, and '
        'allowed, the next hold records', (tester) async {
      onIphone();
      final context = await screen(tester);
      final recorder = _FakeRecorder(MicAccess.undetermined);
      MicReady? ready;
      unawaited(
        askMicrophone(context, recorder: recorder).then((r) => ready = r),
      );
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(find.text('Talk instead of typing'), findsOneWidget);
      expect(
        find.text(
          'Your Claktory turns what you say into text itself. Your iPhone '
          'asks next.',
        ),
        findsOneWidget,
      );
      // Over the mic, at the window's foot.
      expect(
        tester.getBottomLeft(find.text('Continue')).dy,
        lessThan(_phone.height - 44),
      );
      await tester.tap(_button('Continue'));
      await pumpFrames(tester);
      expect(recorder.requests, 1);
      recorder.now = MicAccess.granted;
      recorder.pending!.complete(true);
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(find.text('Talk instead of typing'), findsNothing);
      expect(ready, MicReady.allowedNow);
    });

    testWidgets('refused in the popover: it closes, and the mic says so', (
      tester,
    ) async {
      onIphone();
      final context = await screen(tester);
      final recorder = _FakeRecorder(MicAccess.undetermined);
      MicReady? ready;
      unawaited(
        askMicrophone(context, recorder: recorder).then((r) => ready = r),
      );
      await pumpFrames(tester, const Duration(seconds: 1));
      await tester.tap(_button('Continue'));
      await pumpFrames(tester);
      recorder.now = MicAccess.denied;
      recorder.pending!.complete(false);
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(find.text('Talk instead of typing'), findsNothing);
      expect(find.text('The microphone is off for Claktory'), findsNothing);
      expect(ready, MicReady.declined);
    });

    testWidgets('a tap beside the popover sets it aside, asking nothing', (
      tester,
    ) async {
      onIphone();
      final context = await screen(tester);
      final recorder = _FakeRecorder(MicAccess.undetermined);
      MicReady? ready;
      unawaited(
        askMicrophone(context, recorder: recorder).then((r) => ready = r),
      );
      await pumpFrames(tester, const Duration(seconds: 1));
      await tester.tapAt(const Offset(20, 100));
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(find.text('Talk instead of typing'), findsNothing);
      expect(recorder.requests, 0);
      expect(ready, MicReady.setAside);
    });

    testWidgets('the keyboard docking moves the mic: the popover follows it, '
        'clear of the keyboard', (tester) async {
      onIphone();
      late BuildContext mic;
      await Harness(Workspace()).show(
        tester,
        size: _phone,
        // The message box rides on the keyboard, as the app's does.
        home: Builder(
          builder: (context) => Padding(
            padding: EdgeInsets.only(
              bottom: MediaQuery.viewInsetsOf(context).bottom,
            ),
            child: Align(
              alignment: Alignment.bottomRight,
              child: Builder(
                builder: (c) {
                  mic = c;
                  return const SizedBox(width: 44, height: 44);
                },
              ),
            ),
          ),
        ),
      );
      await pumpFrames(tester);
      unawaited(
        askMicrophone(mic, recorder: _FakeRecorder(MicAccess.undetermined)),
      );
      await pumpFrames(tester, const Duration(seconds: 1));
      final before = tester.getBottomLeft(find.text('Continue')).dy;
      tester.view.viewInsets = const FakeViewPadding(bottom: 300);
      addTearDown(tester.view.resetViewInsets);
      await pumpFrames(tester, const Duration(seconds: 1));
      final after = tester.getBottomLeft(find.text('Continue')).dy;
      expect(after, lessThan(before - 250), reason: 'it rose with the mic');
      expect(after, lessThan(_phone.height - 300 - 44));
    });

    testWidgets('at the accessibility text sizes, the sheet instead', (
      tester,
    ) async {
      onIphone();
      final context = await screen(tester, text: 2);
      final recorder = _FakeRecorder(MicAccess.undetermined);
      unawaited(askMicrophone(context, recorder: recorder));
      await pumpFrames(tester, const Duration(seconds: 1));
      expect(find.text('Talk instead of typing'), findsOneWidget);
      // The sheet's full-width Continue, not the popover's 320pt.
      expect(tester.getSize(_button('Continue')).width, greaterThan(320));
      expect(tester.takeException(), isNull);
    });
  });
}
