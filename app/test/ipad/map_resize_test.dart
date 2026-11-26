// The Map on an iPad keeps what it shows when the window is resized across
// the phone's layout (Sale's row 149): History, and where it is in it,
// come back as they were, whether the Map is a page or a pane beside a
// conversation, as a phone's rotation keeps them (Zucchero's row 525).
import 'package:factory_app/history/view.dart' show HistoryView, MapModeSwitch;
import 'package:factory_app/ipad/workbench.dart'
    show WorkbenchFocus, WorkbenchScope;
import 'package:factory_app/map.dart' show MapView;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../map_fixture.dart';
import '../panes_test.dart' show leaf, row, split;
import 'as_ipad.dart';

Future<Harness> _show(WidgetTester tester, Workspace workspace) async {
  asIPad();
  final harness = Harness(workspace);
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  addTearDown(tester.view.reset);
  _resize(tester, const Size(1366, 1024));
  await harness.show(
    tester,
    size: const Size(1366, 1024),
    home: WorkbenchScope(
      focus: focus,
      child: const Shell(rail: SizedBox(width: 56)),
    ),
  );
  await pumpFrames(tester);
  return harness;
}

void _resize(WidgetTester tester, Size size) {
  tester.view.physicalSize = size;
  tester.view.display.size = size;
}

bool _history(WidgetTester tester) =>
    tester.widget<MapModeSwitch>(find.byType(MapModeSwitch)).history;

Future<State> _toHistory(WidgetTester tester) async {
  await tester.tap(find.text('History').first);
  await pumpFrames(tester);
  expect(_history(tester), isTrue);
  return tester.state(find.byType(HistoryView));
}

void main() {
  testWidgets('the Map page: History and where it is come back after the '
      'phone layout and back', (tester) async {
    final harness = await _show(tester, claktoryWorkspace());
    harness.page.value = AppPage.map;
    await pumpFrames(tester);
    final history = await _toHistory(tester);

    for (final size in [const Size(321, 1024), const Size(984, 746)]) {
      _resize(tester, size);
      await pumpFrames(tester);
      expect(find.byType(MapView), findsOneWidget);
      expect(_history(tester), isTrue, reason: '$size');
      // The same History, at the same place.
      expect(tester.state(find.byType(HistoryView)), same(history));
    }
  });

  testWidgets('the Map beside a conversation, in front in the phone '
      'layout: History and where it is stay as the window widens', (
    tester,
  ) async {
    final workspace = claktoryWorkspace();
    final agent = workspace.agents.first;
    agent.layout = split('s1', row, [leaf('chat', 'chat'), leaf('map', 'map')]);
    workspace.selectedAgentId = agent.id;
    await _show(tester, workspace);
    // The Map read last, then the window narrowed: it's in front.
    await tester.tap(find.text('History').first);
    await pumpFrames(tester);
    _resize(tester, const Size(487, 1024));
    await pumpFrames(tester);
    expect(find.byType(MapView), findsOneWidget);
    final history = await _toHistory(tester);

    for (final size in [const Size(823, 1024), const Size(1366, 1024)]) {
      _resize(tester, size);
      await pumpFrames(tester);
      expect(find.byType(MapView), findsOneWidget);
      expect(_history(tester), isTrue, reason: '$size');
      expect(tester.state(find.byType(HistoryView)), same(history));
    }
  });
}
