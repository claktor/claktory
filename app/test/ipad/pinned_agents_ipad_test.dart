// Pinned agents on an iPad (Lucy's ux/pinned-agents.md; Bruno 48346):
// touch and hold a pin in the workbench's sidebar and let go, and its menu
// is a popover beside the row, as iPadOS shows a sidebar row's (Sorbet's
// review point); touch, hold and move, and the pins reorder by finger. At
// 13" landscape (the sidebar docked), 11" portrait and a Stage Manager
// window (the sidebar over the window), and at the largest text. With
// SHOTS set, drawn to PNGs there:
//   SHOTS=/tmp/pins flutter test test/ipad/pinned_agents_ipad_test.dart
// test-bundle: alone (loads the app's fonts)
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/ipad/workbench.dart'
    show WorkbenchFocus, WorkbenchScope;
import 'package:factory_app/mobile/popover.dart' show popoverWidth;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../phone_sizes_test.dart' show loadGeist;
import 'as_ipad.dart';
import 'window_fixtures.dart' show TextFactor;

Agent _agent(String id, String name, String role, {String project = 'p1'}) =>
    Agent(
      id: id,
      name: name,
      role: role,
      projectId: project,
      leadId: project.isEmpty ? '' : 'rivet',
      modelAvailable: true,
    );

Workspace _workspace() => Workspace(
  projects: [
    Project(id: 'p1', name: 'Claktory', path: '/w'),
    Project(id: 'p2', name: 'Insights', path: '/i'),
  ],
  agents: [
    _agent('rivet', 'Rivet', 'Admin', project: ''),
    _agent('bruno', 'Bruno', 'Project Manager'),
    _agent('polnareff', 'Polnareff', 'Frontend'),
    _agent('lucy', 'Lucy', 'UX Designer'),
    _agent('dana', 'Dana', 'Product Insights', project: 'p2'),
  ],
  selectedAgentId: 'polnareff',
  settings: Settings(pinnedAgents: ['bruno', 'lucy', 'dana']),
);

Future<void> _shot(WidgetTester tester, String name) async {
  final folder = Platform.environment['SHOTS'];
  if (folder == null) return;
  final view = tester.binding.renderViews.first;
  final layer = view.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(
    () => layer.toImage(Offset.zero & view.size, pixelRatio: 1),
  );
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}

/// The workbench as main.dart builds it, [size] wide, the sidebar shown
/// (over the window where it isn't docked).
Future<Harness> _show(
  WidgetTester tester,
  Size size, {
  double text = TextFactor.standard,
}) async {
  asIPad();
  tester.platformDispatcher.textScaleFactorTestValue = text;
  addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  final harness = Harness(_workspace());
  await harness.show(
    tester,
    size: size,
    home: WorkbenchScope(
      focus: focus,
      child: const Shell(rail: SizedBox(width: 56)),
    ),
  );
  await settle(tester);
  if (find.text('Dana').evaluate().isEmpty) {
    await tester.tap(find.byKey(const ValueKey('workbench-sidebar')));
    await settle(tester);
  }
  expect(find.text('Dana'), findsWidgets);
  return harness;
}

/// A pin's row: the sidebar's first [name], the pins being on top.
Rect _row(WidgetTester tester, String name) => tester.getRect(
  find.ancestor(of: find.text(name).first, matching: find.byType(Row)).first,
);

Rect _panel(WidgetTester tester) => tester.getRect(
  find
      .ancestor(
        of: find.text('Unpin'),
        matching: find.byWidgetPredicate(
          (widget) => widget is SizedBox && widget.width == popoverWidth,
        ),
      )
      .first,
);

void main() {
  setUpAll(loadGeist);

  for (final (name, size, text) in [
    ('13in-landscape', const Size(1376, 1032), TextFactor.standard),
    ('11in-portrait', const Size(834, 1194), TextFactor.standard),
    ('stage-manager-671', const Size(671, 900), TextFactor.standard),
    ('13in-landscape-ax', const Size(1376, 1032), TextFactor.ax5),
  ]) {
    testWidgets(
      '$name: touch and hold a pin, let go: its menu beside the row; Unpin '
      'unpins',
      (tester) async {
        final harness = await _show(tester, size, text: text);
        await _shot(tester, 'ipad-pins-$name');
        final row = _row(tester, 'Dana');
        final finger = await tester.startGesture(
          tester.getCenter(find.text('Dana').first),
          kind: PointerDeviceKind.touch,
        );
        await tester.pump(const Duration(milliseconds: 700));
        await finger.up();
        await settle(tester);
        await tester.pump(const Duration(seconds: 1));
        expect(find.text('Unpin'), findsOneWidget);
        expect(find.text('Move up'), findsOneWidget);
        expect(find.text('Agent settings…'), findsOneWidget);
        final panel = _panel(tester);
        expect(panel.left, greaterThan(row.right - 1), reason: 'beside it');
        expect(panel.right, lessThanOrEqualTo(size.width - 8));
        expect(panel.bottom, lessThanOrEqualTo(size.height - 8));
        expect(tester.takeException(), isNull);
        await _shot(tester, 'ipad-pins-$name-menu');

        await tester.tap(find.text('Unpin'));
        await settle(tester);
        expect(harness.last.unpinAgent.agentId, 'dana');
      },
    );
  }

  for (final (name, text) in [
    ('13in-landscape', TextFactor.standard),
    ('13in-landscape-ax', TextFactor.ax5),
  ]) {
    testWidgets('$name: touch, hold and move a pin up: it reorders by '
        'finger', (tester) async {
      final harness = await _show(tester, const Size(1376, 1032), text: text);
      final from = _row(tester, 'Dana').center;
      final bruno = _row(tester, 'Bruno');
      final to = bruno.topCenter + const Offset(0, 3);
      final finger = await tester.startGesture(
        from,
        kind: PointerDeviceKind.touch,
      );
      await tester.pump(const Duration(milliseconds: 700));
      for (var i = 1; i <= 12; i++) {
        await finger.moveTo(Offset.lerp(from, to, i / 12)!);
        await tester.pump(const Duration(milliseconds: 16));
      }
      await tester.pump(const Duration(milliseconds: 400));
      await _shot(tester, 'ipad-pins-$name-drag');
      await finger.up();
      await settle(tester);
      expect(harness.last.pinAgent.agentId, 'dana');
      expect(harness.last.pinAgent.beforeId, 'bruno');
      expect(tester.takeException(), isNull);
    });
  }
}
