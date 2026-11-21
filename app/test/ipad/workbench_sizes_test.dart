import 'dart:io';

import 'package:factory_app/identity.dart';
import 'package:factory_app/ipad/columns.dart';
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/layout.dart' show chatKind;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/sidebar.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import 'as_ipad.dart';
import 'window_fixtures.dart';

/// Geist, as the app ships it, so words are as wide as on the iPad.
Future<void> _loadFonts() async {
  Future<void> family(String name, List<String> files) async {
    final loader = FontLoader(name);
    for (final file in files) {
      loader.addFont(
        Future.value(
          ByteData.sublistView(File('fonts/$file').readAsBytesSync()),
        ),
      );
    }
    await loader.load();
  }

  await family('Geist', [
    'Geist-Regular.ttf',
    'Geist-Medium.ttf',
    'Geist-SemiBold.ttf',
  ]);
}

ViewNode _leaf(String id, String kind, String title) => ViewNode(
  id: id,
  leaf: ViewLeaf(
    kind: kind,
    path: kind == chatKind ? '' : '/papers/$title',
    title: title,
  ),
);

/// Wren with a paper and its notes beside the conversation; the paper's
/// name long enough to need cutting short in a narrow switch.
final _wren = Agent(
  id: 'wren',
  name: 'Wren',
  color: 'teal',
  projectId: 'p1',
  modelAvailable: true,
  layout: ViewNode(
    id: 'root',
    split: ViewSplit(
      axis: ViewAxis.VIEW_AXIS_ROW,
      children: [
        _leaf('chat', chatKind, ''),
        _leaf(
          'paper',
          'pdf',
          'attention-is-all-you-need-with-appendices-v7.pdf',
        ),
        _leaf('notes', 'text', 'notes.md'),
      ],
    ),
  ),
);

Workspace _workspace() => Workspace(
  projects: [Project(id: 'p1', name: 'Papers to read this week', path: '/w')],
  agents: [
    Agent(id: 'rivet', name: 'Rivet', role: 'Admin', modelAvailable: true),
    _wren,
    Agent(
      id: 'june',
      name: 'June',
      role: 'Writer',
      projectId: 'p1',
      color: 'violet',
      modelAvailable: true,
    ),
  ],
  selectedAgentId: 'wren',
);

/// The workbench at [fixture], with [sidebar] as its projects sidebar.
Future<WorkbenchFocus> _show(
  WidgetTester tester,
  WindowFixture fixture, {
  required Widget Function(double width) sidebar,
}) async {
  asIPad();
  final harness = Harness(_workspace());
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  await harness.show(
    tester,
    size: fixture.size,
    home: MediaQuery.withClampedTextScaling(
      minScaleFactor: fixture.textFactor,
      maxScaleFactor: fixture.textFactor,
      child: WorkbenchScope(
        focus: focus,
        child: Builder(
          builder: (context) {
            final palette = Palette.of(context);
            return ColoredBox(
              color: palette.canvas,
              child: DefaultTextStyle(
                style: TextStyles.body.copyWith(color: palette.text),
                child: Workbench(
                  rail: const SizedBox(width: 40),
                  sidebar: sidebar,
                  sidebarWidth: 248,
                  content: const SizedBox.expand(key: ValueKey('content')),
                  agent: _wren,
                ),
              ),
            );
          },
        ),
      ),
    ),
  );
  return focus;
}

void main() {
  setUpAll(_loadFonts);

  // Every window the workbench takes, as on an iPad (a finger's sizes) at
  // its text size: nothing overflows with the real sidebar pinned or open
  // over the window, or with Panes open, and the content keeps its least
  // width.
  for (final fixture in windowFixtures) {
    if (fixture.tier == WorkbenchTier.compact) continue;
    testWidgets('$fixture lays out without overflowing', (tester) async {
      final focus = await _show(
        tester,
        fixture,
        sidebar: (width) => Sidebar(width: width),
      );
      final context = tester.element(find.byType(Workbench));
      expect(windowTier(context, fixture.size), fixture.tier);
      // As the panes would lay it out: with the paper hidden in narrow
      // windows, Panes shows.
      focus.project(
        'wren',
        _wren.layout,
        tier: fixture.tier,
        room: Size(
          contentWidth(fixture.tier, fixture.size.width),
          fixture.size.height,
        ),
        textFactor: fixture.textFactor,
      );
      await tester.pump();
      await tester.pump();
      expect(tester.takeException(), isNull);
      expect(
        tester.getSize(find.byKey(const ValueKey('content'))).width,
        greaterThanOrEqualTo(
          fixture.tier == WorkbenchTier.focused ? fixture.size.width - 1 : 440,
        ),
      );

      if (fixture.tier != WorkbenchTier.full) {
        await tester.tap(find.byKey(const ValueKey('workbench-sidebar')));
        await tester.pump();
        expect(find.byType(Sidebar), findsOneWidget);
        expect(tester.takeException(), isNull);
        await tester.tap(find.byKey(const ValueKey('workbench-scrim')));
        await tester.pump();
      }

      final panes = find.byKey(const ValueKey('workbench-panes'));
      if (panes.evaluate().isNotEmpty) {
        await tester.tap(panes);
        await tester.pump();
        expect(find.text('Panes'), findsWidgets);
        expect(tester.takeException(), isNull);
      }
    });
  }
}
