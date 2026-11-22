import 'dart:ui' show Tristate;

import 'package:factory_app/ipad/columns.dart';
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/layout.dart' show chatKind, layoutOf;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import 'as_ipad.dart';

ViewNode _leaf(String id, String kind, [String title = '']) => ViewNode(
  id: id,
  leaf: ViewLeaf(
    kind: kind,
    path: kind == chatKind ? '' : '/papers/$id.$kind',
    title: title,
  ),
);

/// Wren, with a paper open beside the conversation.
Agent _wren({bool paper = true}) => Agent(
  id: 'wren',
  name: 'Wren',
  color: 'teal',
  layout: paper
      ? ViewNode(
          id: 'root',
          split: ViewSplit(
            axis: ViewAxis.VIEW_AXIS_ROW,
            children: [
              _leaf('chat', chatKind),
              _leaf('paper', 'pdf', 'paper.pdf'),
            ],
          ),
        )
      : null,
);

Agent _june() => Agent(id: 'june', name: 'June', color: 'violet');

/// Shows the workbench at [size], its parts stand-ins that say what they
/// are, with [agent] selected.
Future<WorkbenchFocus> _show(
  WidgetTester tester,
  Size size, {
  Agent? agent,
  double textScale = 1,
  bool panel = false,
  WorkbenchFocus? focus,
}) async {
  asIPad();
  final harness = Harness(Workspace());
  final kept = focus ?? WorkbenchFocus();
  addTearDown(kept.dispose);
  await harness.show(
    tester,
    size: size,
    home: MediaQuery.withClampedTextScaling(
      minScaleFactor: textScale,
      maxScaleFactor: textScale,
      child: WorkbenchScope(
        focus: kept,
        child: _Host(agent: agent ?? _wren(), panel: panel),
      ),
    ),
  );
  return kept;
}

/// Lets a test change the selected agent.
class _Host extends StatefulWidget {
  const _Host({required this.agent, required this.panel});

  final Agent agent;
  final bool panel;

  @override
  State<_Host> createState() => _HostState();
}

class _HostState extends State<_Host> {
  late Agent agent = widget.agent;

  @override
  Widget build(BuildContext context) => Workbench(
    rail: const SizedBox(key: ValueKey('rail'), width: 40, height: 100),
    sidebar: (width) => SizedBox(
      key: const ValueKey('sidebar'),
      width: width,
      child: GestureDetector(
        onTap: () => setState(() => agent = _june()),
        child: const Text('June'),
      ),
    ),
    sidebarWidth: 248,
    content: Focus(
      autofocus: true,
      child: SizedBox.expand(key: const ValueKey('content')),
    ),
    panel: widget.panel ? const SizedBox(key: ValueKey('panel')) : null,
    panelWidth: 320,
    agent: agent,
  );
}

Finder _key(String key) => find.byKey(ValueKey(key));

void main() {
  testWidgets('the tier is the window\'s, whatever room the workbench is '
      'given and whatever the keyboard covers', (tester) async {
    asIPad();
    // Less than a compact window's least height left either way: a banner
    // took the rest, or the keyboard (which the workbench keeps clear of).
    for (final (room, keyboard) in [(400.0, 0.0), (null, 700.0)]) {
      final focus = WorkbenchFocus();
      addTearDown(focus.dispose);
      final host = _Host(agent: _wren(), panel: false);
      await Harness(Workspace()).show(
        tester,
        size: const Size(744, 1133),
        home: MediaQuery(
          data: MediaQueryData(
            size: const Size(744, 1133),
            padding: const EdgeInsets.only(top: 32),
            viewPadding: const EdgeInsets.only(top: 32, bottom: 20),
            viewInsets: EdgeInsets.only(bottom: keyboard),
          ),
          child: WorkbenchScope(
            focus: focus,
            child: Align(
              alignment: Alignment.topCenter,
              child: room == null ? host : SizedBox(height: room, child: host),
            ),
          ),
        ),
      );
      expect(tester.takeException(), isNull);
      final context = tester.element(find.byType(Workbench));
      expect(
        windowTier(context, const Size(744, 1133)),
        WorkbenchTier.focused,
      );
      expect(_key('workbench-panes'), findsOneWidget);
    }
  });

  group('the full tier', () {
    testWidgets('pins the sidebar beside the content, and ⌃⌘S hides it', (
      tester,
    ) async {
      await _show(tester, const Size(1366, 1024));
      expect(_key('sidebar'), findsOneWidget);
      expect(_key('rail'), findsOneWidget);
      expect(_key('workbench-sidebar-over'), findsNothing);
      // Beside the content, not over it.
      expect(
        tester.getTopRight(_key('sidebar')).dx,
        lessThanOrEqualTo(tester.getTopLeft(_key('content')).dx),
      );
      expect(tester.getSize(_key('content')).width, greaterThan(900));

      await tester.sendKeyDownEvent(LogicalKeyboardKey.control);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyS);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.control);
      await tester.pump();
      expect(_key('sidebar'), findsNothing);

      await tester.tap(_key('workbench-sidebar'));
      await tester.pump();
      expect(_key('sidebar'), findsOneWidget);
      expect(_key('workbench-sidebar-over'), findsNothing);
    });

    testWidgets('keeps the panel beside the content when it fits', (
      tester,
    ) async {
      await _show(tester, const Size(1366, 1024), panel: true);
      expect(_key('panel'), findsOneWidget);
      expect(_key('workbench-panel-over'), findsNothing);
    });
  });

  group('the paired tier', () {
    testWidgets('opens the sidebar over the content; a tap on the dimmed '
        'window closes it', (tester) async {
      await _show(tester, const Size(1024, 1366));
      expect(_key('sidebar'), findsNothing);
      expect(_key('rail'), findsOneWidget);
      final content = tester.getSize(_key('content'));

      await tester.tap(_key('workbench-sidebar'));
      await tester.pump();
      expect(_key('workbench-sidebar-over'), findsOneWidget);
      expect(_key('sidebar'), findsOneWidget);
      // Over the content, which keeps its width.
      expect(tester.getSize(_key('content')), content);
      expect(
        tester.getSemantics(_key('workbench-sidebar')).label,
        'Hide sidebar',
      );

      await tester.tapAt(const Offset(1000, 700));
      await tester.pump();
      expect(_key('sidebar'), findsNothing);
    });

    testWidgets('choosing an agent in the sidebar closes it', (tester) async {
      await _show(tester, const Size(1024, 1366));
      await tester.tap(_key('workbench-sidebar'));
      await tester.pump();
      await tester.tap(find.text('June'));
      await tester.pump();
      expect(_key('workbench-sidebar-over'), findsNothing);
    });

    testWidgets('⌃⌘S closes the sidebar it opened over the window', (
      tester,
    ) async {
      await _show(tester, const Size(1024, 1366));
      await tester.tap(_key('workbench-sidebar'));
      await tester.pump();
      await tester.pump();
      expect(_key('workbench-sidebar-over'), findsOneWidget);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.control);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyS);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.control);
      await tester.pump();
      expect(_key('workbench-sidebar-over'), findsNothing);
    });

    testWidgets('Esc closes the sidebar', (tester) async {
      await _show(tester, const Size(1024, 1366));
      await tester.tap(_key('workbench-sidebar'));
      await tester.pump();
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pump();
      expect(_key('workbench-sidebar-over'), findsNothing);
    });

    testWidgets('keeps the panel beside the content at its narrowest', (
      tester,
    ) async {
      await _show(tester, const Size(840, 1000), panel: true);
      expect(_key('panel'), findsOneWidget);
      expect(_key('workbench-panel-over'), findsNothing);
      expect(tester.getSize(_key('content')).width, greaterThanOrEqualTo(440));
    });

    testWidgets('larger text makes a wide window paired', (tester) async {
      await _show(tester, const Size(1180, 820), textScale: 23 / 17);
      expect(_key('sidebar'), findsNothing);
      expect(_key('workbench-sidebar'), findsOneWidget);
    });
  });

  group('the focused tier', () {
    testWidgets('switches between the conversation and the paper', (
      tester,
    ) async {
      final focus = await _show(tester, const Size(744, 1133));
      expect(_key('rail'), findsNothing);
      final root = layoutOf(_wren());
      expect(focus.focusedIn('wren', root), 'chat');
      expect(find.text('Conversation'), findsOneWidget);
      expect(find.text('paper.pdf'), findsOneWidget);

      await tester.tap(_key('workbench-switch-document'));
      await tester.pump();
      expect(focus.focusedIn('wren', root), 'paper');
      expect(
        tester
            .getSemantics(_key('workbench-switch-document'))
            .flagsCollection
            .isSelected,
        Tristate.isTrue,
      );

      await tester.tap(_key('workbench-switch-conversation'));
      await tester.pump();
      expect(focus.focusedIn('wren', root), 'chat');
    });

    testWidgets('has no switch when there\'s only the conversation', (
      tester,
    ) async {
      await _show(tester, const Size(744, 1133), agent: _wren(paper: false));
      expect(_key('workbench-switch-document'), findsNothing);
      expect(_key('workbench-panes'), findsNothing);
    });

    testWidgets('opens the panel over the content', (tester) async {
      await _show(tester, const Size(744, 1133), panel: true);
      expect(_key('workbench-panel-over'), findsOneWidget);
    });

    testWidgets('opens the rail and sidebar together over the window', (
      tester,
    ) async {
      await _show(tester, const Size(744, 1133));
      await tester.tap(_key('workbench-sidebar'));
      await tester.pump();
      expect(_key('rail'), findsOneWidget);
      expect(_key('sidebar'), findsOneWidget);
    });

    testWidgets('⌃Tab moves to the next pane, and round again', (tester) async {
      final focus = await _show(tester, const Size(744, 1133));
      final root = layoutOf(_wren());
      Future<void> controlTab() async {
        await tester.sendKeyDownEvent(LogicalKeyboardKey.control);
        await tester.sendKeyEvent(LogicalKeyboardKey.tab);
        await tester.sendKeyUpEvent(LogicalKeyboardKey.control);
        await tester.pump();
      }

      await controlTab();
      expect(focus.focusedIn('wren', root), 'paper');
      await controlTab();
      expect(focus.focusedIn('wren', root), 'chat');
    });

    testWidgets('Panes lists every pane and focuses the one chosen', (
      tester,
    ) async {
      final focus = await _show(tester, const Size(744, 1133));
      final root = layoutOf(_wren());
      // What the panes laid out: the conversation, the paper hidden.
      focus.project(
        'wren',
        root,
        tier: WorkbenchTier.focused,
        room: const Size(744, 1080),
      );
      await tester.pump();
      await tester.tap(_key('workbench-panes'));
      await tester.pump();
      expect(find.text('Panes'), findsOneWidget);
      expect(find.text('Not shown'), findsOneWidget);
      await tester.tap(find.text('paper.pdf').last);
      await tester.pump();
      expect(find.text('Panes'), findsNothing);
      expect(focus.focusedIn('wren', root), 'paper');
    });

    testWidgets('Panes is a popover under its button; Esc closes it', (
      tester,
    ) async {
      final focus = await _show(tester, const Size(744, 1133));
      focus.project(
        'wren',
        layoutOf(_wren()),
        tier: WorkbenchTier.focused,
        room: const Size(744, 1080),
      );
      await tester.pump();
      await tester.tap(_key('workbench-panes'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));
      final button = tester.getRect(_key('workbench-panes'));
      final list = tester.getRect(find.byType(PanesList));
      expect(list.top, greaterThan(button.bottom));
      expect(list.left, lessThanOrEqualTo(button.center.dx));
      expect(list.right, greaterThanOrEqualTo(button.center.dx));
      await tester.sendKeyEvent(LogicalKeyboardKey.escape);
      await tester.pump();
      expect(find.byType(PanesList), findsNothing);
    });

    testWidgets('⌥⌘P with every pane laid out: Panes in the window\'s '
        'middle', (tester) async {
      final focus = await _show(tester, const Size(1366, 1024));
      focus.project(
        'wren',
        layoutOf(_wren()),
        tier: WorkbenchTier.full,
        room: const Size(1000, 1000),
      );
      await tester.pump();
      await tester.pump();
      expect(_key('workbench-panes'), findsNothing);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.alt);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.meta);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyP);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.meta);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.alt);
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 300));
      final list = tester.getRect(find.byType(PanesList));
      expect(list.center.dx, closeTo(683, 1));
      expect(list.center.dy, closeTo(512, 1));
    });
  });

  group('the wide tiers\' Panes', () {
    testWidgets('appears above the rail only when a pane isn\'t shown', (
      tester,
    ) async {
      final focus = await _show(tester, const Size(1024, 1366));
      expect(_key('workbench-panes'), findsNothing);
      focus.project(
        'wren',
        layoutOf(_wren()),
        tier: WorkbenchTier.paired,
        // Too narrow for both: only the focused conversation fits.
        room: const Size(500, 1300),
      );
      // The panes hear of it after the frame.
      await tester.pump();
      await tester.pump();
      expect(_key('workbench-panes'), findsOneWidget);
      expect(
        tester.getSemantics(_key('workbench-panes')).label,
        'Panes, 1 not shown',
      );
    });
  });

  test('the focus is the session\'s: a closed pane falls back to the '
      'conversation', () {
    final focus = WorkbenchFocus();
    final root = layoutOf(_wren());
    focus.focus('wren', 'paper');
    expect(focus.focusedIn('wren', root), 'paper');
    expect(focus.focusedIn('wren', layoutOf(_wren(paper: false))), 'chat');
    expect(focus.documentIn('wren', root)!.id, 'paper');
    expect(focus.documentIn('wren', layoutOf(_wren(paper: false))), isNull);
    focus.dispose();
  });
}
