// A phone's Back (mobile/back.dart): what it would close, in order (the
// newest sheet, the drawer, the page on top), and closing only what it
// read, still there. The iPhone has no Back of its own; Android's host
// reads and commits it. And the platform policy (mobile/platform_policy.dart)
// that keeps the iPhone as it is.
import 'dart:async';

import 'package:factory_app/agent_view.dart';
import 'package:factory_app/mobile/back.dart';
import 'package:factory_app/mobile/drawer.dart';
import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/mobile/presentation.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_test.dart' show phone, workspace;

Future<PhoneDrawerHostState> _show(WidgetTester tester) async {
  final harness = Harness(workspace());
  await harness.show(tester, size: phone);
  await pumpFrames(tester);
  return tester.state<PhoneDrawerHostState>(find.byType(PhoneDrawerHost));
}

PhoneRoutes _routes(WidgetTester tester) =>
    PhoneRoutes.of(tester.element(find.byType(AgentView)))!;

void main() {
  group('the registry, alone', () {
    late bool open;
    late List<PageTarget> pages;
    late PhoneBack back;
    var serial = 0;

    void push(String id) => pages.add(PageTarget(id: id, serial: ++serial));

    setUp(() {
      open = false;
      pages = [];
      back = PhoneBack(
        drawerOpen: () => open,
        closeDrawer: () => open = false,
        topPage: () => pages.isEmpty ? null : pages.last,
        popPage: () => pages.removeLast(),
      );
    });
    tearDown(() => back.dispose());

    test('the newest token, then the drawer, then the page on top; none '
        'at the root', () {
      expect(back.target, isNull);
      push('route:settings');
      expect(back.target, pages.last);

      open = true;
      back.update();
      expect(back.target, isA<DrawerTarget>());

      var closed = 0;
      final first = back.register(CallbackBackToken(onBack: () => closed++));
      final second = back.register(
        CallbackBackToken(onBack: () => closed += 10),
      );
      final top = back.target;
      expect(top, isA<TokenTarget>());
      expect(back.commit(top!), isTrue);
      expect(closed, 10);

      second.release();
      expect(back.target, isA<TokenTarget>());
      expect(back.target, isNot(top));
      first.release();
      first.release();
      expect(back.target, isA<DrawerTarget>());
    });

    test('a target read before something covered it, or went, does '
        'nothing', () {
      push('route:settings');
      final page = back.target!;

      // A sheet rose meanwhile.
      final sheet = back.register(CallbackBackToken(onBack: () {}));
      expect(back.commit(page), isFalse);
      expect(pages, hasLength(1));
      sheet.release();

      // The same page pushed again is another.
      pages
        ..removeLast()
        ..add(PageTarget(id: 'route:settings', serial: ++serial));
      expect(back.commit(page), isFalse);
      expect(back.commit(back.target!), isTrue);
      expect(pages, isEmpty);
    });

    test('the drawer closed and opened again is another target', () {
      open = true;
      back.update();
      final first = back.target!;
      open = false;
      back.update();
      open = true;
      back.update();
      expect(back.target, isNot(first));
      expect(back.commit(first), isFalse);
      expect(open, isTrue);
      expect(back.commit(back.target!), isTrue);
      expect(open, isFalse);
    });

    test('a token that would drop something is still reached: it asks', () {
      var dirty = true;
      var asked = 0;
      final handle = back.register(
        CallbackBackToken(onBack: () => asked++, canClose: () => !dirty),
      );
      final target = back.target!;
      expect(target.canClose, isFalse);
      expect(back.commit(target), isTrue);
      expect(asked, 1);

      var told = 0;
      back.addListener(() => told++);
      dirty = false;
      handle.changed();
      expect(told, 1);
      expect(back.target!.canClose, isTrue);
      handle.changed();
      expect(told, 1, reason: 'nothing new to tell');
    });
  });

  testWidgets('on a phone: a page, the drawer over it, a sheet over that, '
      'each closed by Back in turn', (tester) async {
    final host = await _show(tester);
    final back = host.back;
    expect(back.target, isNull, reason: 'the conversation is the root');

    unawaited(
      _routes(tester)
          .push(PhoneRoute(id: 'test', builder: (_) => const Text('A page'))),
    );
    await pumpFrames(tester);
    expect(back.target, isA<PageTarget>());
    expect((back.target! as PageTarget).id, 'route:test');

    host.openDrawer();
    await pumpFrames(tester);
    expect(back.target, isA<DrawerTarget>());

    unawaited(
      presentSheet<void>(
        tester.element(find.byType(Sidebar)),
        builder: (context, close) => const Text('A sheet'),
      ),
    );
    await pumpFrames(tester);
    expect(back.target, isA<TokenTarget>());
    expect(sheetsPresented.value, 1);

    expect(back.commit(back.target!), isTrue);
    await pumpFrames(tester);
    expect(find.text('A sheet'), findsNothing);
    expect(sheetsPresented.value, 0);
    expect(back.target, isA<DrawerTarget>());

    expect(back.commit(back.target!), isTrue);
    await pumpFrames(tester);
    expect(find.byType(Sidebar), findsNothing);
    expect(back.target, isA<PageTarget>());

    expect(back.commit(back.target!), isTrue);
    await pumpFrames(tester);
    expect(find.text('A page'), findsNothing);
    expect(back.target, isNull);
  });

  testWidgets('a sheet with changes asks before Back drops them', (
    tester,
  ) async {
    final host = await _show(tester);
    final back = host.back;
    unawaited(
      presentSheet<void>(
        tester.element(find.byType(AgentView)),
        builder: (context, close) => Builder(
          builder: (context) {
            guardSheet(context, () => confirmDiscard(context));
            return const Text('A form');
          },
        ),
      ),
    );
    await pumpFrames(tester);
    final form = back.target!;
    expect(form.canClose, isFalse);

    expect(back.commit(form), isTrue);
    await pumpFrames(tester);
    expect(find.text('Discard changes'), findsOneWidget);
    expect(find.text('A form'), findsOneWidget);
    // The question is a sheet of its own, over the form.
    expect(back.target, isNot(form));
    await tester.tap(find.text('Discard changes'));
    await pumpFrames(tester);
    expect(find.text('A form'), findsNothing);
    expect(back.target, isNull);
  });

  testWidgets('the extension panel sheet is a target while it is up', (
    tester,
  ) async {
    final harness = Harness(
      workspace(openPanel: 'checklist')..selectedAgentId = 'mike',
    );
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    final back = tester
        .state<PhoneDrawerHostState>(find.byType(PhoneDrawerHost))
        .back;
    expect(find.byType(PhoneSheet), findsOneWidget);
    expect(back.target, isA<TokenTarget>());
    expect(back.commit(back.target!), isTrue);
    await pumpFrames(tester);
    expect(harness.last.openPanel.agentId, 'mike');
    expect(harness.last.openPanel.extensionId, isEmpty);
  });

  group('the platform policy', () {
    test("the iPhone's: 44pt, the edge opens the drawer, its own "
        'selection, no Back host', () {
      final policy = PhonePlatformPolicy.forPlatform(TargetPlatform.iOS);
      expect(policy.touchTarget, 44);
      expect(policy.drawerEdgeSwipe, isTrue);
      expect(policy.nativeSelection, isTrue);
      expect(policy.selectableMenu(), isNotNull);
      final page = policy.page<void>(
        key: const ValueKey('p'),
        child: const SizedBox(),
      );
      expect(page.runtimeType.toString(), startsWith('CupertinoPage'));
      const child = SizedBox();
      final back = PhoneBack(
        drawerOpen: () => false,
        closeDrawer: () {},
        topPage: () => null,
        popPage: () {},
      );
      addTearDown(back.dispose);
      expect(policy.backHost(back: back, child: child), same(child));
    });

    test("a Mac's: the iPhone's sizes, the desktop's selection", () {
      final policy = PhonePlatformPolicy.forPlatform(TargetPlatform.macOS);
      expect(policy.touchTarget, 44);
      expect(policy.nativeSelection, isFalse);
      expect(policy.selectableMenu(), isNull);
    });

    test("Android's, where it's asked for", () {
      PhonePlatformPolicy.debugAndroidPolicy = true;
      addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
      final policy = PhonePlatformPolicy.forPlatform(TargetPlatform.android);
      expect(policy.touchTarget, 48);
      expect(policy.drawerEdgeSwipe, isFalse);
      expect(policy.nativeSelection, isTrue);
      final page = policy.page<void>(
        key: const ValueKey('p'),
        child: const SizedBox(),
      );
      // Android's own (android/behavior_policy.dart), not the iPhone's.
      expect(page.runtimeType.toString(), isNot(startsWith('CupertinoPage')));
    });

    testWidgets("on Android, the leading edge doesn't open the drawer; Menu "
        'does', (tester) async {
      PhonePlatformPolicy.debugAndroidPolicy = true;
      addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
      final host = await _show(tester);
      await tester.dragFrom(const Offset(6, 420), const Offset(300, 0));
      await pumpFrames(tester);
      expect(host.drawerOpen, isFalse);
      host.openDrawer();
      await pumpFrames(tester);
      expect(find.byType(Sidebar), findsOneWidget);
    });
  });
}
