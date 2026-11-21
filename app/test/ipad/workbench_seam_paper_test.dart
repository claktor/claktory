// The window's focus across the seam (docs/ipad-workbench.md §3, hook 2):
// an iPad window narrowed with the paper in front opens on the paper, over
// the conversation; widened again, the paper is still in front. Back on
// the phone's routes gives the conversation the focus. Nothing of this is
// the agent's layout: no SetLayout is sent.
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/pane_chrome.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/shell.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../panes_test.dart' show leaf, layoutsSent, row, snapshot, split, text;
import 'as_ipad.dart';

/// The window as main.dart builds it: the focus above the Shell.
Widget _window(WorkbenchFocus focus) => WorkbenchScope(
  focus: focus,
  child: const Shell(rail: SizedBox(width: 56)),
);

final _layout = split('s1', row, [
  leaf('chat', 'chat'),
  leaf('paper', 'text', '/w/paper.md'),
]);

const _wide = Size(1366, 1024);
const _narrow = Size(375, 1024);

Future<(Harness, WorkbenchFocus)> _paperInFront(WidgetTester tester) async {
  asIPad();
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  final harness = Harness(snapshot(layout: _layout));
  harness.backend.files['/w/paper.md'] = text(
    '/w/paper.md',
    '# Paper\n\nThe argument.\n',
    mime: 'text/markdown',
  );
  await harness.show(tester, size: _wide, home: _window(focus));
  await pumpFrames(tester);
  focus.focus('a1', 'paper');
  await pumpFrames(tester);
  expect(find.byType(Workbench), findsOneWidget);
  expect(focus.focusedIn('a1', _layout), 'paper');
  return (harness, focus);
}

/// The phone's viewer page in front, if any.
final _viewers = find.byWidgetPredicate(
  (widget) => widget is PaneChrome && widget.phone != null,
);

PhonePane _viewer(WidgetTester tester) =>
    tester.widget<PaneChrome>(_viewers).phone!;

void main() {
  testWidgets('narrowed with the paper focused, the window opens on it; '
      'widened again, the paper is still focused', (tester) async {
    final (harness, focus) = await _paperInFront(tester);

    await harness.show(tester, size: _narrow, home: _window(focus));
    await pumpFrames(tester);
    expect(find.byType(Workbench), findsNothing);
    expect(find.byType(PhoneShell), findsOneWidget);
    expect(_viewers, findsOneWidget);
    expect(_viewer(tester).title, 'paper.md');
    expect(focus.focusedIn('a1', _layout), 'paper');

    await harness.show(tester, size: _wide, home: _window(focus));
    await pumpFrames(tester);
    expect(find.byType(Workbench), findsOneWidget);
    expect(focus.focusedIn('a1', _layout), 'paper');
    expect(layoutsSent(harness), isEmpty);
  });

  testWidgets('Back from the paper on the narrow window gives the '
      'conversation the focus, kept when the window widens', (tester) async {
    final (harness, focus) = await _paperInFront(tester);

    await harness.show(tester, size: _narrow, home: _window(focus));
    await pumpFrames(tester);
    expect(_viewers, findsOneWidget);

    _viewer(tester).onBack();
    await pumpFrames(tester);
    expect(_viewers, findsNothing);
    expect(focus.focusedIn('a1', _layout), 'chat');

    await harness.show(tester, size: _wide, home: _window(focus));
    await pumpFrames(tester);
    expect(find.byType(Workbench), findsOneWidget);
    expect(focus.focusedIn('a1', _layout), 'chat');
    expect(layoutsSent(harness), isEmpty);
  });

  testWidgets('narrowed with the conversation focused, the window opens on '
      'the conversation', (tester) async {
    asIPad();
    final focus = WorkbenchFocus();
    addTearDown(focus.dispose);
    final harness = Harness(snapshot(layout: _layout));
    await harness.show(tester, size: _wide, home: _window(focus));
    await pumpFrames(tester);

    await harness.show(tester, size: _narrow, home: _window(focus));
    await pumpFrames(tester);
    expect(find.byType(PhoneShell), findsOneWidget);
    expect(_viewers, findsNothing);
    expect(layoutsSent(harness), isEmpty);
  });
}
