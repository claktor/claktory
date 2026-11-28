// An app's view on an iPad fills the window in the focused tier: Claktory's
// own pane header names the app there ("Charts app · Days"), so the view
// drops its "From Charts" frame. Beside the conversation, at the wider
// tiers, the view keeps it.
import 'package:factory_app/app_views/app_view.dart';
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/panes.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../panes_test.dart' show leaf, row, snapshot, split;
import 'as_ipad.dart';

Future<WorkbenchFocus> _show(WidgetTester tester, Size size) async {
  asIPad();
  final workspace = snapshot(
    layout: split('s1', row, [
      leaf('chat', 'chat'),
      ViewNode(
        id: 'v1',
        leaf: ViewLeaf(
          kind: appKind,
          title: 'Days',
          appId: 'charts',
          view: 'day-by-day',
        ),
      ),
    ]),
  );
  workspace.apps.add(
    AppInfo(id: 'charts', name: 'Charts', icon: 'chart-column'),
  );
  final harness = Harness(workspace);
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  await harness.show(
    tester,
    size: size,
    home: WorkbenchScope(
      focus: focus,
      child: Builder(
        builder: (context) {
          final store = AppScope.of(context).workspace;
          return ListenableBuilder(
            listenable: store,
            builder: (context, _) => Workbench(
              rail: const SizedBox(width: 40),
              sidebar: (width) => SizedBox(width: width),
              sidebarWidth: 248,
              agent: store.selectedAgent,
              content: PaneLayout(
                agent: store.selectedAgent!,
                chat: const SizedBox.expand(),
              ),
            ),
          );
        },
      ),
    ),
  );
  await settle(tester);
  return focus;
}

/// Whether Claktory's chrome names the app around the view shown.
bool _named(WidgetTester tester) =>
    ChromeNamesApp.of(tester.element(find.byType(AppView)));

Finder _header(String text) =>
    find.descendant(of: find.byType(PaneHeader), matching: find.text(text));

void main() {
  testWidgets('filling the window in the focused tier, the pane header '
      'names the app and the view goes without its frame', (tester) async {
    final focus = await _show(tester, const Size(744, 1133));
    focus.focus('a1', 'v1');
    await settle(tester);
    expect(_header('Charts app · Days'), findsOneWidget);
    expect(_named(tester), isTrue);
  });

  testWidgets('beside the conversation the view keeps its own frame', (
    tester,
  ) async {
    await _show(tester, const Size(1366, 1024));
    expect(_header('Charts app · Days'), findsOneWidget);
    expect(_named(tester), isFalse);
  });
}
