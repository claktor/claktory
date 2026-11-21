import 'package:factory_app/android/back_host.dart';
import 'package:factory_app/android/behavior_policy.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/channel_fakes.dart';

class _Target {
  _Target(this.id, {this.page = true, this.canClose = true});
  final String id;
  final bool page;
  final bool canClose;
  String? title;
}

/// Only a fake of the shared registry contract. Actual shared PhoneShell paths
/// live in foreground_core_test.dart, not a second production phone shell.
class _Registry extends ChangeNotifier {
  final pages = <_Target>[];
  _Target? surface;
  _Target? get target => surface ?? pages.lastOrNull;
  var commits = 0;
  var confirmations = 0;
  bool showNavigator = true;
  String draft = 'Unsent 日本語 👩🏽‍💻';

  void push(String id) {
    pages.add(_Target(id));
    notifyListeners();
  }

  void dropNavigator() {
    showNavigator = false;
    notifyListeners();
  }

  void renameTop(String title) {
    pages.last.title = title;
    notifyListeners();
  }

  void overlay(String id, {bool dirty = false}) {
    surface = _Target(id, page: false, canClose: !dirty);
    notifyListeners();
  }

  void commit(_Target captured) {
    if (!identical(captured, target)) return;
    commits++;
    if (!captured.canClose) {
      confirmations++;
    } else if (identical(surface, captured)) {
      surface = null;
    } else {
      pages.remove(captured);
    }
    notifyListeners();
  }
}

Future<Object?> _gesture(String method, {int edge = 0, double progress = 0}) =>
    sendNativeMethod(
      SystemChannels.backGesture,
      MethodCall(
        method,
        method == 'startBackGesture' || method == 'updateBackGestureProgress'
            ? {
                'touchOffset': [edge == 0 ? 1.0 : 799.0, 300.0],
                'progress': progress,
                'swipeEdge': edge,
              }
            : null,
      ),
    );

Future<void> _button() async {
  await sendNativeMethod(
    SystemChannels.navigation,
    const MethodCall('popRoute'),
  );
}

Future<void> _show(
  WidgetTester tester,
  _Registry registry, {
  bool reduceMotion = false,
}) async {
  const policy = AndroidBehaviorPolicy();
  tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
  await tester.pumpWidget(
    WidgetsApp(
      color: const Color(0xff202020),
      localizationsDelegates: policy.localizations,
      builder: (context, _) => MediaQuery(
        data: MediaQuery.of(context).copyWith(disableAnimations: reduceMotion),
        child: AndroidBackHost<_Target>(
          changes: registry,
          readTarget: () => registry.target,
          canClose: (target) => target.canClose,
          commitTarget: registry.commit,
          pageKey: (target) => target.page ? ValueKey(target.id) : null,
          child: ListenableBuilder(
            listenable: registry,
            builder: (context, _) => Stack(
              children: [
                if (registry.showNavigator)
                  Navigator(
                    pages: [
                      policy.page<void>(
                        key: const ValueKey('root'),
                        child: const Center(child: Text('Conversation')),
                      ),
                      for (final page in registry.pages)
                        policy.page<void>(
                          key: ValueKey(page.id),
                          child: Center(
                            child: Text(
                              page.title ?? page.id,
                              key: ValueKey('title/${page.id}'),
                            ),
                          ),
                        ),
                    ],
                    onDidRemovePage: (_) {},
                  ),
                if (registry.surface case final surface?)
                  Center(
                    child: Text(surface.id, key: const ValueKey('overlay')),
                  ),
              ],
            ),
          ),
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

// Reuse these exact widget instances across stages: removing the old host must
// not be rescued by an unrelated rebuild/publication from the new one.
Widget _hostFor(_Registry registry, String id) => AndroidBackHost<_Target>(
  key: ValueKey(id),
  changes: registry,
  readTarget: () => registry.target,
  canClose: (target) => target.canClose,
  commitTarget: registry.commit,
  pageKey: (_) => null,
  child: const SizedBox.expand(),
);

Future<void> _stage(WidgetTester tester, List<Widget> hosts) async {
  tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
  await tester.pumpWidget(
    WidgetsApp(
      color: const Color(0xff202020),
      builder: (_, _) => Stack(children: hosts),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  final platform = <MethodCall>[];
  setUp(() {
    platform.clear();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, (call) async {
          platform.add(call);
          return null;
        });
  });
  tearDown(
    () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(SystemChannels.platform, null),
  );

  for (final edge in [0, 1]) {
    testWidgets(
      'native codec edge $edge cancellation restores route and draft',
      (tester) async {
        final registry = _Registry()..push('Settings');
        await _show(tester, registry);
        final route = ModalRoute.of(tester.element(find.text('Settings')))!;
        expect(route.isCurrent, isTrue);
        expect(await _gesture('startBackGesture', edge: edge), isTrue);
        await _gesture('updateBackGestureProgress', edge: edge, progress: 0.6);
        await tester.pump();
        expect(route.animation!.value, closeTo(0.4, 0.001));
        await _gesture('cancelBackGesture');
        await tester.pumpAndSettle();
        expect(route.animation!.value, 1);
        expect(route.navigator!.userGestureInProgress, isFalse);
        expect(find.text('Settings'), findsOneWidget);
        expect(registry.commits, 0);
        expect(registry.draft, 'Unsent 日本語 👩🏽‍💻');
      },
      variant: TargetPlatformVariant.only(TargetPlatform.android),
    );
  }

  testWidgets('native commit pops once, no independent Material observer pop', (
    tester,
  ) async {
    final registry = _Registry()
      ..push('Files')
      ..push('Viewer');
    await _show(tester, registry);
    expect(await _gesture('startBackGesture'), isTrue);
    await _gesture('updateBackGestureProgress', progress: 0.5);
    await _gesture('commitBackGesture');
    await tester.pumpAndSettle();
    expect(registry.commits, 1);
    expect(registry.pages.map((p) => p.id), ['Files']);
    expect(find.text('Files'), findsOneWidget);
    expect(find.text('Viewer'), findsNothing);
    // Flutter can deliver another terminal callback to the retained observer.
    await _gesture('commitBackGesture');
    await tester.pumpAndSettle();
    expect(registry.commits, 1);
  }, variant: TargetPlatformVariant.only(TargetPlatform.android));

  for (final predictive in [false, true]) {
    testWidgets(
      '${predictive ? 'predictive' : 'three-button'} IME Back never pops page',
      (tester) async {
        final registry = _Registry()..push('Editor');
        await _show(tester, registry);
        tester.view.viewInsets = const FakeViewPadding(bottom: 300);
        addTearDown(tester.view.resetViewInsets);
        await tester.pump();
        tester.testTextInput.log.clear();
        if (predictive) {
          expect(await _gesture('startBackGesture'), isTrue);
          // Insets shrink before the same attempt's terminal callback.
          tester.view.viewInsets = const FakeViewPadding();
          await tester.pump();
          await _gesture('commitBackGesture');
        } else {
          await _button();
        }
        await tester.pumpAndSettle();
        expect(registry.commits, 0);
        expect(find.text('Editor'), findsOneWidget);
        expect(
          tester.testTextInput.log.where((c) => c.method == 'TextInput.hide'),
          hasLength(1),
        );
        tester.view.viewInsets = const FakeViewPadding();
        await tester.pump();
        await _button();
        await tester.pumpAndSettle();
        expect(registry.commits, 1);
        expect(find.text('Conversation'), findsOneWidget);
        expect(registry.draft, 'Unsent 日本語 👩🏽‍💻');
      },
      variant: TargetPlatformVariant.only(TargetPlatform.android),
    );
  }

  testWidgets(
    'overlay arising during progress restores page; commit cannot pop beneath it',
    (tester) async {
      final registry = _Registry()..push('Viewer');
      await _show(tester, registry);
      final route = ModalRoute.of(tester.element(find.text('Viewer')))!;
      await _gesture('startBackGesture');
      await _gesture('updateBackGestureProgress', progress: 0.4);
      registry.overlay('Confirmation', dirty: true);
      await tester.pumpAndSettle();
      await _gesture('commitBackGesture');
      await tester.pumpAndSettle();
      expect(registry.commits, 0);
      expect(registry.pages, hasLength(1));
      expect(find.text('Confirmation'), findsOneWidget);
      expect(route.animation!.value, 1);
      await _button();
      await tester.pumpAndSettle();
      expect(registry.confirmations, 1);
      expect(registry.pages, hasLength(1));
    },
    variant: TargetPlatformVariant.only(TargetPlatform.android),
  );

  testWidgets(
    'drawer/overlay button Back publishes eligibility but does not pop page',
    (tester) async {
      final registry = _Registry()
        ..push('Viewer')
        ..overlay('Menu');
      await _show(tester, registry);
      expect(
        platform
            .where((c) => c.method == 'SystemNavigator.setFrameworkHandlesBack')
            .last
            .arguments,
        isTrue,
      );
      await _button();
      await tester.pumpAndSettle();
      expect(registry.pages, hasLength(1));
      expect(registry.surface, isNull);
      await _button();
      await tester.pumpAndSettle();
      expect(
        platform
            .where((c) => c.method == 'SystemNavigator.setFrameworkHandlesBack')
            .last
            .arguments,
        isFalse,
      );
      await _button();
      await tester.pumpAndSettle();
      expect(
        platform.where((c) => c.method == 'SystemNavigator.pop'),
        hasLength(1),
      );
      expect(registry.commits, 2);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.android),
  );

  testWidgets('reduced motion keeps preview still', (tester) async {
    final registry = _Registry()..push('Settings');
    await _show(tester, registry, reduceMotion: true);
    final route = ModalRoute.of(tester.element(find.text('Settings')))!;
    await _gesture('startBackGesture');
    await _gesture('updateBackGestureProgress', progress: 0.6);
    await tester.pump();
    expect(route.animation!.value, 1);
    await _gesture('cancelBackGesture');
    await tester.pumpAndSettle();
    expect(registry.commits, 0);
  }, variant: TargetPlatformVariant.only(TargetPlatform.android));

  testWidgets(
    'same-key Page updates display new content rather than constructor child',
    (tester) async {
      final registry = _Registry()..push('Settings');
      await _show(tester, registry);
      final route = ModalRoute.of(tester.element(find.text('Settings')))!;
      registry.renameTop('Updated Settings');
      await tester.pumpAndSettle();
      expect(find.text('Settings'), findsNothing);
      expect(find.text('Updated Settings'), findsOneWidget);
      expect(
        ModalRoute.of(tester.element(find.text('Updated Settings'))),
        same(route),
      );
    },
    variant: TargetPlatformVariant.only(TargetPlatform.android),
  );

  testWidgets(
    'inactive/resume preserves the gesture and its committed intent',
    (tester) async {
      final registry = _Registry()..push('Settings');
      await _show(tester, registry);
      await _gesture('startBackGesture');
      await _gesture('updateBackGestureProgress', progress: 0.4);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await _gesture('commitBackGesture');
      await tester.pumpAndSettle();
      expect(registry.commits, 1);
      expect(find.text('Conversation'), findsOneWidget);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.android),
  );

  testWidgets('backgrounding abandons preview, never navigates on resume', (
    tester,
  ) async {
    final registry = _Registry()..push('Editor');
    await _show(tester, registry);
    await _gesture('startBackGesture');
    await _gesture('updateBackGestureProgress', progress: 0.5);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await _gesture('commitBackGesture');
    await tester.pumpAndSettle();
    expect(registry.commits, 0);
    expect(find.text('Editor'), findsOneWidget);
    expect(
      ModalRoute.of(tester.element(find.text('Editor')))!.animation!.value,
      1,
    );
    await _button();
    await tester.pumpAndSettle();
    expect(registry.commits, 1);
  }, variant: TargetPlatformVariant.only(TargetPlatform.android));

  testWidgets(
    'button during a swipe is consumed, never commits a canceled swipe',
    (tester) async {
      final registry = _Registry()..push('Settings');
      await _show(tester, registry);
      await _gesture('startBackGesture');
      await _gesture('updateBackGestureProgress', progress: 0.5);
      await _button();
      expect(registry.commits, 0);
      await _gesture('cancelBackGesture');
      await tester.pumpAndSettle();
      expect(registry.commits, 0);
      expect(find.text('Settings'), findsOneWidget);
      await _button();
      await tester.pumpAndSettle();
      expect(registry.commits, 1);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.android),
  );

  for (final wholeHost in [false, true]) {
    testWidgets(
      'active preview tolerates ${wholeHost ? 'host' : 'Navigator'} subtree disposal',
      (tester) async {
        final registry = _Registry()..push('Viewer');
        await _show(tester, registry);
        await _gesture('startBackGesture');
        await _gesture('updateBackGestureProgress', progress: 0.6);
        if (wholeHost) {
          await tester.pumpWidget(const SizedBox.shrink());
          expect(
            platform
                .where(
                  (c) => c.method == 'SystemNavigator.setFrameworkHandlesBack',
                )
                .last
                .arguments,
            isFalse,
            reason: 'the disposed owner must release native Back eligibility',
          );
        } else {
          // Drop only the Navigator, keeping the host and its target unchanged.
          registry.dropNavigator();
          await tester.pumpAndSettle();
        }
        await _gesture('cancelBackGesture');
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull);
        expect(registry.commits, 0);
      },
      variant: TargetPlatformVariant.only(TargetPlatform.android),
    );
  }

  testWidgets('old host disposal cannot clear a newer host eligibility', (
    tester,
  ) async {
    final old = _Registry()..overlay('Old sheet');
    final next = _Registry()..overlay('New sheet');
    final oldHost = _hostFor(old, 'old');
    final newHost = _hostFor(next, 'new');
    await _stage(tester, [oldHost]);
    await _stage(tester, [oldHost, newHost]);
    final beforeRemoval = platform.length;
    await _stage(tester, [newHost]);
    expect(
      platform
          .skip(beforeRemoval)
          .where(
            (c) =>
                c.method == 'SystemNavigator.setFrameworkHandlesBack' &&
                c.arguments == false,
          ),
      isEmpty,
    );
    expect(
      platform
          .where((c) => c.method == 'SystemNavigator.setFrameworkHandlesBack')
          .last
          .arguments,
      isTrue,
    );
    await _button();
    await tester.pumpAndSettle();
    expect(old.commits, 0);
    expect(next.commits, 1);
  }, variant: TargetPlatformVariant.only(TargetPlatform.android));

  testWidgets(
    'obsolete host cannot republish or consume Back over the new root',
    (tester) async {
      final old = _Registry()..overlay('Old sheet');
      final root = _Registry();
      final oldHost = _hostFor(old, 'old');
      final newHost = _hostFor(root, 'root');
      await _stage(tester, [oldHost]);
      await _stage(tester, [oldHost, newHost]);
      final beforeOldChange = platform.length;
      old.overlay('Late old sheet');
      await tester.pumpAndSettle();
      expect(
        platform
            .skip(beforeOldChange)
            .where(
              (c) =>
                  c.method == 'SystemNavigator.setFrameworkHandlesBack' &&
                  c.arguments == true,
            ),
        isEmpty,
      );
      expect(
        platform
            .where((c) => c.method == 'SystemNavigator.setFrameworkHandlesBack')
            .last
            .arguments,
        isFalse,
      );
      await _button();
      await tester.pumpAndSettle();
      expect(old.commits, 0);
      expect(root.commits, 0);
      expect(
        platform.where((c) => c.method == 'SystemNavigator.pop'),
        hasLength(1),
      );
    },
    variant: TargetPlatformVariant.only(TargetPlatform.android),
  );

  testWidgets(
    'captured swipe cancels across host handoff without retargeting',
    (tester) async {
      final old = _Registry()..overlay('Old sheet');
      final next = _Registry()..overlay('New sheet');
      final oldHost = _hostFor(old, 'old');
      final newHost = _hostFor(next, 'new');
      await _stage(tester, [oldHost]);
      expect(await _gesture('startBackGesture'), isTrue);
      await _stage(tester, [oldHost, newHost]);
      await _gesture('commitBackGesture');
      await tester.pumpAndSettle();
      expect(old.commits, 0);
      expect(next.commits, 0);
      expect(old.surface?.id, 'Old sheet');
      expect(next.surface?.id, 'New sheet');
      await _stage(tester, [newHost]);
      await _button();
      await tester.pumpAndSettle();
      expect(next.commits, 1);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.android),
  );

  testWidgets(
    'route tripwire disables independent native predictive observers',
    (tester) async {
      final registry = _Registry()..push('Viewer');
      await _show(tester, registry);
      final route = ModalRoute.of(tester.element(find.text('Viewer')))!;
      // The SDK's Android default theme has a predictive builder. It must never
      // independently claim our page; eligibility/progress belongs to the host.
      expect(route.popGestureEnabled, isFalse);
      expect(await _gesture('startBackGesture'), isTrue);
      await _gesture('updateBackGestureProgress', progress: 0.6);
      expect(route.animation!.value, closeTo(0.4, 0.001));
      await _gesture('cancelBackGesture');
      await tester.pumpAndSettle();
      expect(registry.commits, 0);
      expect(route.navigator!.userGestureInProgress, isFalse);
    },
    variant: TargetPlatformVariant.only(TargetPlatform.android),
  );
}
