import 'package:factory_app/controls.dart' show ExtensionGlyph;
import 'package:factory_app/mobile/device.dart'
    show WindowControls, windowControls;
import 'package:factory_app/mobile/back.dart';
import 'package:factory_app/panes.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart'
    show
        beside,
        glyphButton,
        layoutsSent,
        leaf,
        reads,
        row,
        snapshot,
        split,
        text;

const _main = '/w/main.rs';
const _body = 'fn main() {\n}\n';

/// Builder's layout, the conversation and main.rs, with [expanded] (a
/// leaf's id) filling the window.
Future<Harness> _show(
  WidgetTester tester, {
  String expanded = '',
  ViewNode? layout,
}) async {
  final workspace = snapshot(layout: layout ?? beside());
  workspace.agents.last.layoutFocus = expanded;
  final harness = Harness(workspace)..backend.appliesLayouts = true;
  harness.backend.files[_main] = text(_main, _body);
  harness.backend.files['/w/notes.md'] = text('/w/notes.md', '# Notes\n');
  await harness.show(tester);
  await settle(tester);
  return harness;
}

final _expanded = find.byType(ExpandedPane);

SetLayout _lastLayout(Harness harness) => layoutsSent(harness).last.setLayout;

void main() {
  testWidgets('Expand fills the window with a pane, keeping what it shows', (
    tester,
  ) async {
    final harness = await _show(tester);
    expect(_expanded, findsNothing);
    // Every pane but the conversation has it, before its ×.
    expect(glyphButton('Expand main.rs'), findsOneWidget);
    expect(glyphButton('Expand Conversation'), findsNothing);
    final scroll = tester.state<ScrollableState>(
      find
          .descendant(
            of: find.byKey(const ValueKey('pane-v1')),
            matching: find.byType(Scrollable),
          )
          .first,
    );
    final read = reads(harness);

    await tester.tap(glyphButton('Expand main.rs'));
    await settle(tester);
    final sent = _lastLayout(harness);
    expect(sent.focus, 'v1');
    expect(sent.root, beside(), reason: 'the layout itself unchanged');

    // The whole window, over the sidebar and the conversation.
    expect(_expanded, findsOneWidget);
    expect(
      tester.getRect(_expanded),
      Offset.zero & tester.view.physicalSize / tester.view.devicePixelRatio,
    );
    expect(find.text('Back to Builder'), findsOneWidget);
    expect(
      find.descendant(of: _expanded, matching: find.text('fn main() {')),
      findsOneWidget,
    );
    expect(glyphButton('Collapse main.rs'), findsOneWidget);
    // The same viewer, moved: not read again, its scroll the same.
    expect(reads(harness), read);
    expect(
      tester.state<ScrollableState>(
        find.descendant(of: _expanded, matching: find.byType(Scrollable)).first,
      ),
      same(scroll),
    );
  });

  testWidgets('Back, Esc and Collapse put it back as it was', (tester) async {
    final harness = await _show(tester, expanded: 'v1');
    expect(_expanded, findsOneWidget);
    final chat = tester.getRect(find.byKey(const ValueKey('pane-chat')));

    await tester.tap(find.text('Back to Builder'));
    await settle(tester);
    expect(_lastLayout(harness).focus, '');
    expect(_expanded, findsNothing);
    expect(find.text('fn main() {'), findsOneWidget);
    expect(tester.getRect(find.byKey(const ValueKey('pane-chat'))), chat);

    await tester.tap(glyphButton('Expand main.rs'));
    await settle(tester);
    expect(_expanded, findsOneWidget);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(_lastLayout(harness).focus, '');
    expect(_expanded, findsNothing);

    await tester.tap(glyphButton('Expand main.rs'));
    await settle(tester);
    await tester.tap(glyphButton('Collapse main.rs'));
    await settle(tester);
    expect(_lastLayout(harness).focus, '');
    expect(_expanded, findsNothing);
  });

  testWidgets('a pane expanded before (a restart, another device) opens '
      'expanded', (tester) async {
    await _show(tester, expanded: 'v1');
    expect(_expanded, findsOneWidget);
    expect(
      find.descendant(of: _expanded, matching: find.text('fn main() {')),
      findsOneWidget,
    );
  });

  testWidgets('a pane expanded after the panes all closed shows (D1: '
      "Review's file was a blank until a relaunch)", (tester) async {
    final harness = await _show(tester);
    // Every pane closed: the conversation alone.
    final alone = snapshot(layout: beside());
    alone.agents.last.clearLayout();
    await harness.apply(tester, alone);
    await settle(tester);
    expect(find.byKey(const ValueKey('pane-v1')), findsNothing);
    // Review opens the file again, expanded.
    final reviewing = snapshot(layout: beside());
    reviewing.agents.last.layoutFocus = 'v1';
    await harness.apply(tester, reviewing);
    await settle(tester);
    expect(_expanded, findsOneWidget);
    expect(
      find.descendant(of: _expanded, matching: find.text('fn main() {')),
      findsOneWidget,
    );
  });

  testWidgets('a pane an agent opens meanwhile waits behind, and says so', (
    tester,
  ) async {
    final harness = await _show(tester, expanded: 'v1');
    expect(find.textContaining('opened'), findsNothing);

    // The backend adds it behind the expanded pane, which stays expanded.
    final now = harness.backend.workspace.workspace.deepCopy();
    now.agents.last.layout = split('s1', row, [
      leaf('chat', 'chat'),
      leaf('v1', 'text', _main),
      leaf('v2', 'text', '/w/notes.md'),
    ]);
    await harness.apply(tester, now);
    expect(_expanded, findsOneWidget);
    expect(find.text('Builder opened notes.md'), findsOneWidget);

    final again = now.deepCopy();
    again.agents.last.layout.split.children.add(leaf('v3', 'text', '/w/x'));
    await harness.apply(tester, again);
    expect(find.text('Builder opened 2 more'), findsOneWidget);

    // It's a way back too.
    await tester.tap(find.text('Builder opened 2 more'));
    await settle(tester);
    expect(_lastLayout(harness).focus, '');
    expect(_expanded, findsNothing);
    expect(find.text('notes.md'), findsOneWidget);
  });

  testWidgets('closing the expanded pane ends it', (tester) async {
    final harness = await _show(tester, expanded: 'v1');
    await tester.tap(
      find.descendant(of: _expanded, matching: glyphButton('Close')),
    );
    await settle(tester);
    final sent = _lastLayout(harness);
    expect(sent.hasFocus(), isFalse, reason: 'the backend ends it');
    expect(_expanded, findsNothing);
    expect(find.text('fn main() {'), findsNothing);
  });

  testWidgets("a windowed iPad's band sits beside its window controls, "
      'not under them', (tester) async {
    addTearDown(() => windowControls.value = WindowControls.none);
    tester.view.padding = const FakeViewPadding(top: 24 + 43);
    addTearDown(tester.view.resetPadding);
    windowControls.value = const WindowControls(leading: 70, top: 43);
    await _show(tester, expanded: 'v1');
    final chip = tester.getRect(find.text('Back to Builder'));
    final band = tester.getRect(
      find
          .ancestor(
            of: find.text('Back to Builder'),
            matching: find.byType(Row),
          )
          .last,
    );
    // Below the status bar only, beside the controls.
    expect(band.top, lessThan(24 + 1));
    expect(chip.left, greaterThan(70));

    // Full screen: no controls, nothing set aside for them.
    windowControls.value = WindowControls.none;
    tester.view.padding = const FakeViewPadding(top: 24);
    await tester.pump();
    expect(tester.getRect(find.text('Back to Builder')).left, lessThan(70));
  });

  // The Mac's traffic lights don't zoom with the app: the band keeps their
  // room in the window's points at any zoom.
  for (final zoom in [0.75, 0.8, 1.0, 1.25]) {
    testWidgets("the band's Back chip keeps clear of the traffic lights at "
        '${(zoom * 100).round()}%', (tester) async {
      final harness = await _show(tester, expanded: 'v1');
      final now = harness.backend.workspace.workspace.deepCopy();
      now.settings.zoom[harness.backend.zoomKey] = zoom;
      harness.backend.workspace.apply(now);
      await settle(tester);
      final chip = find.textContaining('Back to', findRichText: true);
      expect(chip, findsOneWidget);
      // The lights end about 70 points in; the arrow starts after them.
      final arrow = tester.getTopLeft(
        find.byWidgetPredicate((w) => w is ExtensionGlyph && w.name == 'Back'),
      );
      expect(arrow.dx, greaterThanOrEqualTo(78), reason: 'at $zoom');
    }, variant: TargetPlatformVariant.only(TargetPlatform.macOS));
  }

  // Android's Back (the window's registry) collapses an expanded
  // pane, as its Back chip does, and then closes the pane beside.
  testWidgets("the window's Back collapses an expanded pane, below a dialog "
      'still open over it', (tester) async {
    final back = PhoneBack.root();
    addTearDown(back.dispose);
    final workspace = snapshot(layout: beside());
    workspace.agents.last.layoutFocus = 'v1';
    final harness = Harness(workspace)..backend.appliesLayouts = true;
    harness.backend.files[_main] = text(_main, _body);
    await harness.show(
      tester,
      home: PhoneBackScope(back: back, child: const Shell()),
    );
    await settle(tester);
    expect(_expanded, findsOneWidget);
    expect(back.target, isNotNull);

    // A dialog opened over it closes first.
    var dialog = 0;
    final modal = back.register(CallbackBackToken(onBack: () => dialog++));
    expect(back.commit(back.target!), isTrue);
    expect(dialog, 1);
    expect(_expanded, findsOneWidget);
    modal.release();

    expect(back.commit(back.target!), isTrue);
    await settle(tester);
    expect(_lastLayout(harness).focus, '');
    expect(_expanded, findsNothing);
    // On Android the pane beside is the next target: Back
    // closes it, and the conversation alone is the root.
    expect(back.commit(back.target!), isTrue);
    await settle(tester);
    expect(_lastLayout(harness).hasRoot(), isFalse);
    expect(back.target, isNull);
  });
}
