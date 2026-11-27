// The phone's drawer as a team, drawn to check by eye: the tree by lead
// with folder tags, a folded project speaking for its agents, its ⋯ sheet,
// a finger's drag, the moved row's Undo, the Undo at the foot once the
// drawer closes, and the confirm before a move into another project; on an
// iPhone SE and a Pro Max, at the default text size and AX5, dark (and SE
// light). Each checks what it draws fits; with SHOTS set, drawn to PNGs
// there:
//   SHOTS=/tmp/drawer-moves flutter test test/mobile_drawer_moves_shots_test.dart
// test-bundle: alone (loads the app's fonts)
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/mobile/move_undo.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
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
  bool working = false,
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
  color: color,
  status: working ? AgentStatus.AGENT_STATUS_WORKING : null,
  waitingSeqs: [for (var i = 0; i < waiting; i++) Int64(i + 1)],
);

Workspace _workspace() => Workspace(
  projects: [
    Project(
      id: 'p1',
      name: 'Claktory',
      path: '/Users/me/factory',
      folders: [
        ProjectFolder(id: 'research', name: 'Research', path: '/r'),
        ProjectFolder(id: 'ops', name: 'Ops', path: '/o'),
      ],
    ),
    Project(
      id: 'p2',
      name: 'Bakery Website',
      path: '/Users/me/.factory/workspace/bakery-website',
      collapsed: true,
    ),
  ],
  agents: [
    _agent('rivet', 'Rivet', 'Admin', project: '', color: 'blue'),
    _agent('bruno', 'Bruno', 'Project Manager', lead: 'rivet', color: 'blue'),
    _agent(
      'narancia',
      'Narancia',
      'Researcher',
      lead: 'bruno',
      folder: 'research',
      color: 'orange',
    ),
    _agent(
      'speedwagon',
      'Speedwagon',
      'Product Marketing',
      lead: 'bruno',
      folder: 'research',
      color: 'pink',
    ),
    _agent(
      'okuyasu',
      'Okuyasu',
      'Visitor Tester',
      lead: 'speedwagon',
      folder: 'research',
      color: 'purple',
    ),
    _agent('pesci', 'Pesci', 'Mobile', lead: 'bruno', color: 'teal'),
    _agent(
      'pericolo',
      'Pericolo',
      'DevOps',
      lead: 'bruno',
      folder: 'ops',
      color: 'green',
    ),
    _agent(
      'ana',
      'Ana',
      'Baker',
      project: 'p2',
      waiting: 2,
      working: true,
      color: 'yellow',
    ),
    _agent('june', 'June', 'Copywriter', project: 'p2', color: 'red'),
  ],
  selectedAgentId: 'pesci',
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

Finder _inSidebar(Finder finder) =>
    find.descendant(of: find.byType(Sidebar), matching: finder);

Future<void> _openDrawer(WidgetTester tester) async {
  await tester.tap(find.bySemanticsLabel('Menu'));
  await pumpFrames(tester);
  await settle(tester);
}

/// A finger holds [name]'s row and moves it to [to]; with [hold] it's held
/// there, still down, for a shot.
Future<TestGesture> _drag(
  WidgetTester tester,
  String name,
  Offset to, {
  bool hold = false,
}) async {
  final from = tester.getCenter(_inSidebar(find.text(name)).first);
  final finger = await tester.startGesture(from, kind: PointerDeviceKind.touch);
  await tester.pump(const Duration(milliseconds: 700));
  for (var i = 1; i <= 12; i++) {
    await finger.moveTo(Offset.lerp(from, to, i / 12)!);
    await tester.pump(const Duration(milliseconds: 16));
  }
  await tester.pump(const Duration(milliseconds: 300));
  if (!hold) {
    await finger.up();
    await pumpFrames(tester);
  }
  return finger;
}

/// Scrolls the drawer so [name]'s row is in the middle.
Future<void> _inView(WidgetTester tester, String name) async {
  await Scrollable.ensureVisible(
    tester.element(_inSidebar(find.text(name)).first),
    alignment: 0.5,
  );
  await pumpFrames(tester);
}

Offset _rowOf(WidgetTester tester, String name) => tester
    .getRect(
      find
          .ancestor(
            of: _inSidebar(find.text(name)).first,
            matching: find.byType(Row),
          )
          .first,
    )
    .center;

void main() {
  setUpAll(loadGeist);

  for (final (phone, size, padding) in const [
    ('se', Size(375, 667), FakeViewPadding(top: 20)),
    ('promax', Size(440, 956), FakeViewPadding(top: 62, bottom: 34)),
  ]) {
    for (final (text, factor) in const [
      ('1x', TextFactor.standard),
      ('ax5', TextFactor.ax5),
    ]) {
      for (final dark in [true, if (phone == 'se' && factor == 1.0) false]) {
        final name = '$phone-$text-${dark ? 'dark' : 'light'}';
        testWidgets('the drawer as a team ($name)', (tester) async {
          debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
          addTearDown(() => debugDefaultTargetPlatformOverride = null);
          tester.platformDispatcher.textScaleFactorTestValue = factor;
          addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
          tester.platformDispatcher.platformBrightnessTestValue = dark
              ? Brightness.dark
              : Brightness.light;
          addTearDown(
            tester.platformDispatcher.clearPlatformBrightnessTestValue,
          );
          tester.view.padding = padding;
          addTearDown(tester.view.resetPadding);
          final harness = Harness(_workspace());
          harness.backend.arrangePreview = ArrangePreview();
          await harness.show(tester, size: size);
          await settle(tester);

          await _openDrawer(tester);
          expect(tester.takeException(), isNull);
          await _shot(tester, '$name-1-tree');

          // The project's ⋯: its sheet.
          await tester.tap(find.bySemanticsLabel('Claktory options'));
          await pumpFrames(tester);
          await settle(tester);
          expect(tester.takeException(), isNull);
          expect(find.text('Project settings…'), findsOneWidget);
          await _shot(tester, '$name-2-project-menu');
          await tester.ensureVisible(find.text('Cancel'));
          await pumpFrames(tester);
          await _shot(tester, '$name-2b-project-menu-end');
          await tester.tap(find.text('Cancel'));
          await pumpFrames(tester);
          await settle(tester);

          // Pesci held over Speedwagon: what the drop would do.
          await _inView(tester, 'Okuyasu');
          final finger = await _drag(
            tester,
            'Pesci',
            _rowOf(tester, 'Speedwagon'),
            hold: true,
          );
          expect(tester.takeException(), isNull);
          await _shot(tester, '$name-3-drag');
          await finger.up();
          await pumpFrames(tester);
          expect(harness.last.arrangeAgent.parentId, 'speedwagon');
          expect(
            _inSidebar(find.text('Now reports to Speedwagon')),
            findsOneWidget,
          );
          expect(tester.takeException(), isNull);
          await _shot(tester, '$name-4-undo-on-row');

          // The backend's next snapshot has Pesci under Speedwagon: the
          // Undo line goes with its row.
          await harness.apply(
            tester,
            _workspace()
              ..agents.firstWhere((agent) => agent.id == 'pesci').leadId =
                  'speedwagon'
              ..agents.firstWhere((agent) => agent.id == 'pesci').parentId =
                  'speedwagon',
          );
          await pumpFrames(tester);
          expect(
            _inSidebar(find.text('Now reports to Speedwagon')),
            findsOneWidget,
          );
          expect(
            tester.getTopLeft(_inSidebar(find.text('Pesci'))).dx,
            tester.getTopLeft(_inSidebar(find.text('Okuyasu'))).dx,
          );
          expect(tester.takeException(), isNull);
          await _shot(tester, '$name-4b-moved');

          // The drawer closed within the time: Undo at the foot.
          await tester.tapAt(Offset(size.width - 8, size.height / 2));
          await pumpFrames(tester);
          await settle(tester);
          expect(
            find.descendant(
              of: find.byType(PhoneMoveUndo),
              matching: find.text('Undo'),
            ),
            findsOneWidget,
          );
          expect(tester.takeException(), isNull);
          await _shot(tester, '$name-5-undo-at-foot');
          await tester.pump(const Duration(seconds: 9));
          await settle(tester);

          // Into another project: asked first.
          await _openDrawer(tester);
          await _inView(tester, 'Pericolo');
          await _drag(tester, 'Pericolo', _rowOf(tester, 'Bakery Website'));
          await pumpFrames(tester);
          await settle(tester);
          expect(find.text('Move Pericolo to Bakery Website?'), findsOneWidget);
          expect(tester.takeException(), isNull);
          await _shot(tester, '$name-6-confirm');
        });
      }
    }
  }
}
