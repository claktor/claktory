// The workbench with the real PaneLayout in it: the window's tier decides
// which of the agent's panes are laid out, the switch and Panes move the
// focus, touching a pane focuses it, and resizing the window sends
// nothing.
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/panes.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import 'as_ipad.dart';
import '../panes_test.dart' show leaf, row, snapshot, split;

/// The conversation with a paper beside it.
ViewNode _paper() => split('s1', row, [
  leaf('chat', 'chat'),
  leaf('paper', 'pdf', '/w/paper.pdf'),
]);

Future<(Harness, WorkbenchFocus)> _show(WidgetTester tester, Size size) async {
  asIPad();
  final harness = Harness(snapshot(layout: _paper()));
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
                chat: const SizedBox.expand(key: ValueKey('conversation')),
              ),
            ),
          );
        },
      ),
    ),
  );
  // The panes' projection reaches Panes after the frame.
  await tester.pump();
  return (harness, focus);
}

/// What was sent besides the paper's reads (its document host fetches it):
/// nothing, when only the window changed.
List<Object> _changes(Harness harness) => [
  for (final message in harness.backend.sent)
    if (!message.hasReadFile()) message,
];

Finder _pane(String id) => find.byKey(ValueKey('pane-$id'));

void main() {
  testWidgets('a wide window lays out the paper beside the conversation', (
    tester,
  ) async {
    final (harness, focus) = await _show(tester, const Size(1366, 1024));
    expect(_pane('chat'), findsOneWidget);
    expect(_pane('paper'), findsOneWidget);
    expect(focus.projectionOf('a1')!.hidden, isEmpty);
    expect(find.byKey(const ValueKey('workbench-panes')), findsNothing);

    // Touching the paper makes it the one being read.
    await tester.tap(_pane('paper'));
    await tester.pump();
    expect(focus.projectionOf('a1')!.focused, 'paper');
    expect(_changes(harness), isEmpty);
  });

  testWidgets('a focused window shows one pane, and the switch the other', (
    tester,
  ) async {
    final (harness, focus) = await _show(tester, const Size(744, 1133));
    expect(_pane('paper'), findsNothing);
    expect(find.byKey(const ValueKey('conversation')), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('workbench-switch-document')));
    await tester.pump();
    await tester.pump();
    expect(_pane('paper'), findsOneWidget);
    expect(find.byKey(const ValueKey('conversation')), findsNothing);
    expect(focus.projectionOf('a1')!.hidden, ['chat']);
    // Only the focus moved: the agent's layout is as it was.
    expect(_changes(harness), isEmpty);
  });

  testWidgets('narrowing and widening the window sends nothing and keeps '
      'the paper in front', (tester) async {
    final (harness, focus) = await _show(tester, const Size(1366, 1024));
    focus.focus('a1', 'paper');
    await tester.pump();

    tester.view.physicalSize = const Size(744, 1133);
    await tester.pump();
    await tester.pump();
    expect(_pane('paper'), findsOneWidget);
    expect(find.byKey(const ValueKey('conversation')), findsNothing);

    tester.view.physicalSize = const Size(1366, 1024);
    await tester.pump();
    await tester.pump();
    expect(_pane('paper'), findsOneWidget);
    expect(_pane('chat'), findsOneWidget);
    expect(_changes(harness), isEmpty);
  });
}
