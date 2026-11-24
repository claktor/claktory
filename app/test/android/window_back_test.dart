// Root-registry/Flutter-dispatch contracts. The small surfaces below are
// fixtures, not a fork of product screens or native/emulator acceptance.
import 'package:factory_app/android/back_host.dart';
import 'package:factory_app/android/window_back_host.dart';
import 'package:factory_app/mobile/back.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/channel_fakes.dart';

Future<void> _button() async =>
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

class _Surface extends StatefulWidget {
  const _Surface({super.key, required this.closed, this.dirty = false});
  final VoidCallback closed;
  final bool dirty;
  @override
  State<_Surface> createState() => _SurfaceState();
}

class _SurfaceState extends State<_Surface> {
  final draft = TextEditingController(text: 'Unsent 日本語 👩🏽‍💻');
  final focus = FocusNode();
  BackHandle? handle;
  bool visible = true;
  int confirmations = 0;
  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    handle ??= PhoneBack.maybeOf(context)!.register(
      CallbackBackToken(
        canClose: () => !widget.dirty,
        onBack: () {
          if (widget.dirty) {
            confirmations++;
            setState(() {});
          } else {
            widget.closed();
            handle?.release();
            setState(() => visible = false);
          }
        },
      ),
    );
  }

  @override
  void dispose() {
    handle?.release();
    draft.dispose();
    focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => visible
      ? Column(
          children: [
            const Text('Wide modal fixture'),
            EditableText(
              controller: draft,
              focusNode: focus,
              style: const TextStyle(fontSize: 16),
              cursorColor: const Color(0xffffffff),
              backgroundCursorColor: const Color(0xff000000),
            ),
          ],
        )
      : const Text('Root fixture');
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUp(() {
    PhonePlatformPolicy.debugAndroidPolicy = true;
    addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
  });

  group('one window registry', () {
    test('overlay ordering survives navigation provider rebuilds', () {
      final root = PhoneBack.root();
      addTearDown(root.dispose);
      final window = root.branch('A');
      final phone = window.branch('conversation');
      var drawer = true;
      var closed = '';
      phone.attachNavigation(
        drawerOpen: () => drawer,
        closeDrawer: () => drawer = false,
        topPage: () => const PageTarget(id: 'settings', serial: 1),
        popPage: () => closed = 'phone page',
      );
      expect(root.target, isA<DrawerTarget>());
      final modal = phone.register(
        CallbackBackToken(onBack: () => closed = 'modal'),
      );
      final captured = root.target!;
      // A new wide page mounts under the still-visible modal on resize.
      window.registerNavigation(
        CallbackBackToken(onBack: () => closed = 'wide page'),
      );
      expect(root.target, captured);
      expect(root.commit(captured), isTrue);
      expect(closed, 'modal');
      modal.release();
      expect(root.commit(root.target!), isTrue);
      expect(closed, 'wide page');
    });

    test('new scene retires navigation, not a visible window overlay', () {
      final root = PhoneBack.root();
      addTearDown(root.dispose);
      final window = root.branch('A');
      final old = window.branch('conversation');
      var closed = '';
      old.registerNavigation(
        CallbackBackToken(onBack: () => closed = 'old route'),
      );
      final captured = root.target!;
      final modal = old.register(
        CallbackBackToken(onBack: () => closed = 'modal'),
      );
      final list = window.branch('list');
      list.registerNavigation(CallbackBackToken(onBack: () => closed = 'list'));
      expect(root.commit(captured), isFalse);
      expect(root.commit(root.target!), isTrue);
      expect(closed, 'modal');
      modal.release();
      expect(root.commit(root.target!), isTrue);
      expect(closed, 'list');
      final listTarget = root.target;
      // AnimatedSwitcher can retain the old scene for a frame. It must not
      // create a new window-level overlay after losing its navigation lease.
      final late = old.register(
        CallbackBackToken(onBack: () => closed = 'late old scene'),
      );
      expect(root.target, listTarget);
      late.release();
      expect(root.target, listTarget);
    });

    test(
      'replacement rejects old page even with identical local id/serial',
      () {
        final root = PhoneBack.root();
        addTearDown(root.dispose);
        var oldPops = 0, newPops = 0;
        final old = root.branch('A');
        final oldHandle = old.attachNavigation(
          drawerOpen: () => false,
          closeDrawer: () {},
          topPage: () => const PageTarget(id: 'settings', serial: 1),
          popPage: () => oldPops++,
        );
        final captured = root.target!;
        final next = root.branch('B');
        next.attachNavigation(
          drawerOpen: () => false,
          closeDrawer: () {},
          topPage: () => const PageTarget(id: 'settings', serial: 1),
          popPage: () => newPops++,
        );
        expect(root.target, isA<PageTarget>());
        expect(root.target, isNot(captured));
        oldHandle.changed();
        oldHandle.release();
        expect(root.commit(captured), isFalse);
        expect(old.commit(root.target!), isFalse);
        expect(root.commit(root.target!), isTrue);
        expect(oldPops, 0);
        expect(newPops, 1);
      },
    );

    test(
      'late going-window registrations and handle releases cannot reach B',
      () {
        final root = PhoneBack.root();
        addTearDown(root.dispose);
        final old = root.branch('A');
        final handle = old.register(
          CallbackBackToken(onBack: () => fail('old')),
        );
        final captured = root.target!;
        final next = root.branch('B');
        var closes = 0;
        next.register(CallbackBackToken(onBack: () => closes++));
        final current = root.target;
        old.register(CallbackBackToken(onBack: () => fail('late old')));
        old.registerNavigation(
          CallbackBackToken(onBack: () => fail('late old route')),
        );
        old.attachNavigation(
          drawerOpen: () => true,
          closeDrawer: () => fail('old drawer'),
          topPage: () => null,
          popPage: () => fail('old page'),
        );
        handle.changed();
        handle.release();
        handle.release();
        expect(root.target, current);
        expect(root.commit(captured), isFalse);
        expect(root.commit(current!), isTrue);
        expect(closes, 1);
      },
    );

    testWidgets('retirement is deferred, once, and distinct from Back', (
      tester,
    ) async {
      final root = PhoneBack.root();
      addTearDown(root.dispose);
      var closes = 0, retired = 0;
      root
          .branch('A')
          .register(
            CallbackBackToken(
              onBack: () => closes++,
              onRetired: () => retired++,
            ),
          );
      root.branch('B');
      expect(closes, 0);
      expect(retired, 0);
      expect(root.target, isNull);
      await tester.pump();
      expect(retired, 1);
      root.branch('C');
      await tester.pump();
      expect(retired, 1);
      expect(closes, 0);
    });

    testWidgets('ordinary release cancels deferred retirement cleanup', (
      tester,
    ) async {
      final root = PhoneBack.root();
      addTearDown(root.dispose);
      var retired = 0;
      final handle = root
          .branch('A')
          .register(
            CallbackBackToken(onBack: () {}, onRetired: () => retired++),
          );
      root.branch('B');
      handle.release();
      await tester.pump();
      expect(retired, 0);
    });

    test(
      'same owner branch is stable and navigation changes do not rebuild scope',
      () {
        final root = PhoneBack.root();
        addTearDown(root.dispose);
        final owner = root.branch('A');
        var notifications = 0;
        root.addListener(() => notifications++);
        expect(root.branch('A'), same(owner));
        expect(notifications, 0);
        final handle = owner.register(CallbackBackToken(onBack: () {}));
        expect(root.branch('A'), same(owner));
        expect(notifications, 1);
        handle.release();
        expect(notifications, 2);
        expect(root.branch('A'), same(owner));
      },
    );
  });

  group('root Flutter dispatch at every width', () {
    var systemPops = 0;
    setUp(() {
      systemPops = 0;
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      messenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
        if (call.method == 'SystemNavigator.pop') systemPops++;
        return null;
      });
      addTearDown(() {
        messenger.setMockMethodCallHandler(SystemChannels.platform, null);
      });
    });

    Future<void> show(
      WidgetTester tester,
      Widget child, {
      Size size = const Size(1000, 800),
    }) async {
      tester.view.physicalSize = size;
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await tester.pumpWidget(
        WidgetsApp(
          color: const Color(0xff202020),
          localizationsDelegates: PhonePlatformPolicy.current.localizations,
          builder: (_, _) => AndroidWindowBackHost(window: 'A', child: child),
        ),
      );
      await tester.pump();
    }

    testWidgets('wide cancellation preserves dirty state and never prompts', (
      tester,
    ) async {
      final key = GlobalKey<_SurfaceState>();
      var closed = 0;
      await show(
        tester,
        _Surface(key: key, dirty: true, closed: () => closed++),
      );
      expect(
        find.byWidgetPredicate((w) => w is AndroidBackHost),
        findsOneWidget,
      );
      final draft = key.currentState!.draft.text;
      expect(await _gesture('startBackGesture'), isTrue);
      await _gesture('updateBackGestureProgress', progress: .8);
      await _gesture('cancelBackGesture');
      await tester.pump();
      expect(key.currentState!.draft.text, draft);
      expect(key.currentState!.confirmations, 0);
      expect(closed, 0);
      expect(systemPops, 0);
      await _button();
      await tester.pump();
      expect(key.currentState!.confirmations, 1);
      expect(key.currentState!.draft.text, draft);
      expect(find.text('Wide modal fixture'), findsOneWidget);
    });

    testWidgets(
      'one Back hides IME only, even after inset drops before commit',
      (tester) async {
        final key = GlobalKey<_SurfaceState>();
        var closed = 0;
        await show(tester, _Surface(key: key, closed: () => closed++));
        tester.view.viewInsets = const FakeViewPadding(bottom: 300);
        await tester.pump();
        final draft = key.currentState!.draft.text;
        tester.testTextInput.log.clear();
        expect(await _gesture('startBackGesture'), isTrue);
        tester.view.viewInsets = FakeViewPadding.zero;
        await tester.pump();
        await _gesture('commitBackGesture');
        await tester.pump();
        expect(
          tester.testTextInput.log.where(
            (call) => call.method == 'TextInput.hide',
          ),
          hasLength(1),
        );
        expect(closed, 0);
        expect(key.currentState!.draft.text, draft);
        expect(find.text('Wide modal fixture'), findsOneWidget);
        await _button();
        await tester.pump();
        expect(closed, 1);
        expect(systemPops, 0);
        await _button();
        await tester.pump();
        expect(systemPops, 1);
      },
    );

    testWidgets('resize keeps the same visible overlay and its draft/Back', (
      tester,
    ) async {
      final key = GlobalKey<_SurfaceState>();
      var closed = 0;
      await show(
        tester,
        _Surface(key: key, closed: () => closed++),
        size: const Size(393, 852),
      );
      final draft = key.currentState!.draft.text;
      expect(await _gesture('startBackGesture'), isTrue);
      tester.view.physicalSize = const Size(1000, 800);
      await tester.pump();
      await _gesture('cancelBackGesture');
      await tester.pump();
      expect(
        find.byWidgetPredicate((w) => w is AndroidBackHost),
        findsOneWidget,
      );
      expect(key.currentState!.draft.text, draft);
      expect(closed, 0);
      await _button();
      await tester.pump();
      expect(closed, 1);
      expect(systemPops, 0);
    });
  });
}
