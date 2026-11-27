// Pinned agents drawn for Lucy (ux/pinned-agents.md): the Mac's sidebar with
// four pins, a right click's menu on a pin, a drag pinning an agent; the
// iPhone's drawer with pins, at the default text and at AX5. Each checks
// what it draws fits; with SHOTS set, drawn to PNGs there:
//   SHOTS=/tmp/pins flutter test test/pinned_agents_shots_test.dart
// test-bundle: alone (loads the app's fonts)
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/window_fixtures.dart' show TextFactor;
import 'phone_sizes_test.dart' show loadGeist;

Agent _agent(
  String id,
  String name,
  String role, {
  String project = 'p1',
  String lead = '',
  String folder = '',
  int waiting = 0,
  bool unseen = false,
  String color = '',
}) => Agent(
  id: id,
  name: name,
  role: role,
  projectId: project,
  leadId: lead,
  parentId: project.isEmpty || lead == 'rivet' ? '' : lead,
  folderId: folder,
  modelAvailable: true,
  unseen: unseen,
  color: color,
  waitingSeqs: [for (var i = 0; i < waiting; i++) Int64(i + 1)],
);

Workspace _workspace() => Workspace(
  projects: [
    Project(id: 'p1', name: 'Claktory', path: '/w'),
    Project(id: 'p2', name: 'Insights', path: '/i'),
    Project(id: 'p3', name: 'YouTube Videos', path: '/y'),
  ],
  agents: [
    _agent('rivet', 'Rivet', 'Admin', project: '', color: 'blue'),
    _agent(
      'bruno',
      'Bruno',
      'Project Manager',
      lead: 'rivet',
      unseen: true,
      color: 'blue',
    ),
    _agent('polnareff', 'Polnareff', 'Frontend', lead: 'bruno', color: 'teal'),
    _agent(
      'speedwagon',
      'Speedwagon',
      'Product Marketing',
      lead: 'bruno',
      color: 'orange',
    ),
    _agent('lucy', 'Lucy', 'UX Designer', lead: 'bruno', color: 'green'),
    _agent(
      'dana',
      'Dana',
      'Product Insights',
      project: 'p2',
      waiting: 1,
      color: 'pink',
    ),
    _agent(
      'marco',
      'Marco',
      'Animation Researcher',
      project: 'p3',
      color: 'purple',
    ),
  ],
  selectedAgentId: 'speedwagon',
  settings: Settings(pinnedAgents: ['bruno', 'speedwagon', 'dana', 'marco']),
);

Future<void> _shot(WidgetTester tester, String name) async {
  final folder = Platform.environment['SHOTS'];
  if (folder == null) return;
  final boundary = tester.binding.renderViews.first;
  final layer = boundary.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(
    () => layer.toImage(Offset.zero & boundary.size, pixelRatio: 2),
  );
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}

void main() {
  setUpAll(loadGeist);

  testWidgets('the Mac: four pins, a right click on one, a drag pinning', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    await harness.show(tester, size: const Size(1100, 760));
    await settle(tester);
    expect(tester.takeException(), isNull);
    expect(find.text(' · YouTube Videos'), findsOneWidget);
    await _shot(tester, 'mac');

    await tester.tap(
      find.text('Speedwagon').first,
      buttons: kSecondaryMouseButton,
      kind: PointerDeviceKind.mouse,
    );
    await settle(tester);
    expect(find.text('Unpin'), findsOneWidget);
    await _shot(tester, 'mac-menu');
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);
    expect(find.text('Unpin'), findsNothing);

    // Polnareff, dragged up from Claktory under the last pin.
    final from = tester.getCenter(find.text('Polnareff'));
    final marco = tester.getRect(
      find
          .ancestor(of: find.text('Marco').first, matching: find.byType(Row))
          .first,
    );
    final to = marco.bottomCenter - const Offset(0, 3);
    final mouse = await tester.startGesture(
      from,
      kind: PointerDeviceKind.mouse,
    );
    for (var i = 1; i <= 10; i++) {
      await mouse.moveTo(Offset.lerp(from, to, i / 10)!);
      await tester.pump(const Duration(milliseconds: 16));
    }
    // Held there a moment, as a hand does.
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text('Pins Polnareff here'), findsOneWidget);
    await _shot(tester, 'mac-drag');
    await mouse.up();
    await settle(tester);
    expect(harness.last.pinAgent.agentId, 'polnareff');
    expect(harness.last.pinAgent.beforeId, '');
  });

  for (final (name, factor) in [
    ('iphone', TextFactor.standard),
    ('iphone-ax5', TextFactor.ax5),
  ]) {
    testWidgets("the iPhone's drawer with pins ($name)", (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      tester.platformDispatcher.textScaleFactorTestValue = factor;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final harness = Harness(_workspace());
      await harness.show(tester, size: const Size(393, 852));
      await settle(tester);
      await tester.tap(find.bySemanticsLabel('Menu'));
      await pumpFrames(tester);
      await settle(tester);
      expect(tester.takeException(), isNull);
      // At AX5 the role and the project take a line each.
      expect(find.text(factor > 2 ? 'Claktory' : ' · Claktory'), findsWidgets);
      await _shot(tester, name);

      // Touch and hold, let go: the sheet, titled with the agent.
      final pin = find.text('Dana').first;
      final finger = await tester.startGesture(
        tester.getCenter(pin),
        kind: PointerDeviceKind.touch,
      );
      await tester.pump(const Duration(milliseconds: 700));
      await finger.up();
      await settle(tester);
      await tester.pump(const Duration(seconds: 1));
      // At most two lines: at AX5 the role gives way (Lucy 47558).
      expect(
        find.text(
          factor > 2 ? 'Dana · Insights' : 'Dana · Product Insights · Insights',
        ),
        findsOneWidget,
      );
      expect(tester.takeException(), isNull);
      await _shot(tester, '$name-sheet');
    });
  }
}
