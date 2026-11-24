import 'package:factory_app/controls.dart' show GlyphButton;
import 'package:factory_app/layout.dart' show leaves;
import 'package:factory_app/mobile/metrics.dart' show Touch;
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/mobile/back.dart';
import 'package:factory_app/panes.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show glyphButton, leaf, row, snapshot, split;

/// Sale's A4 and A3 (Bruno 23882): the conversation is never squeezed below
/// its least by the panes beside it. A pane that doesn't fit replaces the
/// oldest beside it; a page with no room for it beside opens in the
/// conversation's place; in a layout the window narrowed under, the
/// panes beside it are squeezed instead. On Android, Back closes the newest pane beside.
void main() {
  List<String> kindsOf(ViewNode root) => [
    for (final leaf in leaves(root)) leaf.leaf.kind,
  ];

  List<String> kinds(Harness harness) {
    final agent = harness.backend.workspace.agent('a1')!;
    if (!agent.hasLayout()) return ['chat'];
    return kindsOf(agent.layout);
  }

  double chatWidth(WidgetTester tester) =>
      tester.getSize(find.byKey(const ValueKey('pane-chat'))).width;

  group('fitting a new pane', () {
    final three = split('s1', row, [
      leaf('chat', 'chat'),
      leaf('n1', 'inbox'),
      leaf('g1', 'settings'),
    ]);

    test('the oldest pane beside gives way until it fits', () {
      final opened = withPage(
        three,
        AppPage.usage,
        fit: (room: 1100, chatLeast: 360),
      );
      // 360 + 3 × 320 + 3 won't go in 1100; without Needs you it does.
      expect(kindsOf(opened), ['chat', 'usage', 'settings']);
    });

    test('with room for all, none goes', () {
      final opened = withPage(
        three,
        AppPage.usage,
        fit: (room: 1400, chatLeast: 360),
      );
      expect(kindsOf(opened), ['chat', 'usage', 'inbox', 'settings']);
    });

    test('larger text makes the conversation wider, so more give way', () {
      final opened = withPage(
        three,
        AppPage.usage,
        fit: (room: 1100, chatLeast: 540),
      );
      expect(kindsOf(opened), ['chat', 'usage']);
    });

    test('too narrow for it even alone, the others stay', () {
      final opened = withPage(
        three,
        AppPage.usage,
        fit: (room: 600, chatLeast: 360),
      );
      expect(kindsOf(opened), ['chat', 'usage', 'inbox', 'settings']);
      expect(fitsBesideChat((room: 600, chatLeast: 360), 320), isFalse);
      expect(fitsBesideChat((room: 681, chatLeast: 360), 320), isTrue);
      expect(fitsBesideChat(null, 320), isTrue);
    });
  });

  testWidgets('in a window too narrow for a page beside the conversation, '
      'Settings opens in its place', (tester) async {
    final harness = Harness(snapshot())..backend.appliesLayouts = true;
    await harness.show(tester, size: const Size(900, 800));
    await settle(tester);
    await tester.tap(glyphButton('Settings'));
    await settle(tester);
    expect(harness.page.value, AppPage.settings);
    expect(harness.backend.sent.where((m) => m.hasSetLayout()), isEmpty);
    expect(find.byType(SettingsView), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('in a wide one, a page that would squeeze the conversation '
      'replaces the oldest pane beside it', (tester) async {
    final harness = Harness(
      snapshot(
        layout: split('s1', row, [
          leaf('chat', 'chat'),
          leaf('n1', 'inbox'),
          leaf('g1', 'settings'),
        ]),
      ),
    )..backend.appliesLayouts = true;
    await harness.show(tester, size: const Size(1400, 900));
    await settle(tester);
    await tester.tap(glyphButton('Usage'));
    await settle(tester);
    expect(kinds(harness), ['chat', 'usage', 'settings']);
    expect(chatWidth(tester), greaterThanOrEqualTo(chatMinWidth - 1));
    expect(tester.takeException(), isNull);
  });

  testWidgets('a layout the window narrowed under keeps the conversation at '
      'its least; the pages beside it scroll sideways', (tester) async {
    final harness = Harness(
      snapshot(
        layout: split('s1', row, [
          leaf('chat', 'chat'),
          leaf('n1', 'inbox'),
          leaf('g1', 'settings'),
          leaf('u1', 'usage'),
        ]),
      ),
    )..backend.appliesLayouts = true;
    await harness.show(tester, size: const Size(1000, 800));
    await settle(tester);
    expect(tester.takeException(), isNull);
    expect(chatWidth(tester), closeTo(chatMinWidth, 1));
    expect(find.byType(PageSideways), findsNWidgets(3));
  });

  testWidgets("on Android, the window's Back closes the newest pane beside "
      'the conversation, one at a time, then lets go', (tester) async {
    final back = PhoneBack.root();
    addTearDown(back.dispose);
    final harness = Harness(
      snapshot(
        layout: split('s1', row, [
          leaf('chat', 'chat'),
          leaf('g1', 'settings'),
        ]),
      ),
    )..backend.appliesLayouts = true;
    await harness.show(
      tester,
      size: const Size(1400, 900),
      home: PhoneBackScope(back: back, child: const Shell()),
    );
    await settle(tester);
    // Usage opens after Settings: it's the newest, though it's placed
    // nearer the conversation.
    AppScope.openPage(tester.element(find.byType(Shell)), AppPage.usage);
    await settle(tester);
    expect(kinds(harness), ['chat', 'usage', 'settings']);
    expect(back.target, isNotNull);

    // A dialog over it closes first.
    var dialog = 0;
    final modal = back.register(CallbackBackToken(onBack: () => dialog++));
    expect(back.commit(back.target!), isTrue);
    expect(dialog, 1);
    modal.release();
    expect(kinds(harness), ['chat', 'usage', 'settings']);

    expect(back.commit(back.target!), isTrue);
    await settle(tester);
    expect(kinds(harness), ['chat', 'settings']);
    expect(back.commit(back.target!), isTrue);
    await settle(tester);
    expect(kinds(harness), ['chat']);
    expect(back.target, isNull, reason: 'the conversation alone is the root');
  }, variant: TargetPlatformVariant.only(TargetPlatform.android));

  testWidgets("Back closes the pane opened last, by when it was opened, "
      'wherever it sits', (tester) async {
    final back = PhoneBack.root();
    addTearDown(back.dispose);
    ViewNode opened(String id, String path, int ms) =>
        leaf(id, 'text', path)..leaf.openedMs = Int64(ms);
    final harness = Harness(
      snapshot(
        layout: split('s1', row, [
          leaf('chat', 'chat'),
          opened('gamma', '/w/gamma.md', 3000),
          opened('alpha', '/w/alpha.md', 1000),
          opened('beta', '/w/beta.md', 2000),
        ]),
      ),
    )..backend.appliesLayouts = true;
    await harness.show(
      tester,
      size: const Size(1600, 900),
      home: PhoneBackScope(back: back, child: const Shell()),
    );
    await settle(tester);
    List<String> ids() => [
      for (final each in leaves(harness.backend.workspace.agent('a1')!.layout))
        each.id,
    ];
    expect(back.commit(back.target!), isTrue);
    await settle(tester);
    expect(ids(), ['chat', 'alpha', 'beta']);
    expect(back.commit(back.target!), isTrue);
    await settle(tester);
    expect(ids(), ['chat', 'alpha']);
  }, variant: TargetPlatformVariant.only(TargetPlatform.android));

  testWidgets('a pane squeezed to its least on touch keeps × whole: Expand '
      'and then its name give way', (tester) async {
    // Android's own policy, a finger's targets (Sale's tablet).
    PhonePlatformPolicy.debugAndroidPolicy = true;
    addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
    final harness = Harness(
      snapshot(
        layout: split('s1', row, [
          leaf('chat', 'chat'),
          leaf('n1', 'inbox'),
          leaf('g1', 'settings'),
          leaf('u1', 'usage'),
        ]),
      ),
    )..backend.appliesLayouts = true;
    await harness.show(tester, size: const Size(1000, 800));
    await settle(tester);
    expect(tester.takeException(), isNull);
    final closes = find.byWidgetPredicate(
      (w) => w is GlyphButton && w.label == 'Close',
    );
    expect(closes, findsNWidgets(3));
    for (final close in closes.evaluate()) {
      final box = close.renderObject! as RenderBox;
      expect(box.size.width, greaterThanOrEqualTo(Touch.target));
    }
  }, variant: TargetPlatformVariant.only(TargetPlatform.android));

  testWidgets("on the Mac, panes beside aren't Back targets", (tester) async {
    final back = PhoneBack.root();
    addTearDown(back.dispose);
    final harness = Harness(
      snapshot(
        layout: split('s1', row, [
          leaf('chat', 'chat'),
          leaf('g1', 'settings'),
        ]),
      ),
    )..backend.appliesLayouts = true;
    await harness.show(
      tester,
      size: const Size(1400, 900),
      home: PhoneBackScope(back: back, child: const Shell()),
    );
    await settle(tester);
    expect(back.target, isNull);
  }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
}
