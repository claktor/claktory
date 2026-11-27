// A page pane across the seam: an iPad window narrowed
// with a page in front (Needs you beside the conversation) opens on that
// page, not on a viewer that can't show it; Back goes to the conversation
// and leaves the pane in the agent's layout. A phone asked to show a page
// pane (showPane, as the app strip's chips do) opens the page the same way.
import 'package:factory_app/inbox.dart' show InboxView;
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/pane_chrome.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../panes_test.dart' show leaf, layoutsSent, row, snapshot, split;
import 'as_ipad.dart';

Widget _window(WorkbenchFocus focus) => WorkbenchScope(
  focus: focus,
  child: const Shell(rail: SizedBox(width: 56)),
);

final _layout = split('s1', row, [
  leaf('chat', 'chat'),
  leaf('needs', 'inbox'),
]);

const _wide = Size(1366, 1024);
const _narrow = Size(375, 1024);

/// The phone's viewer page in front, if any.
final _viewers = find.byWidgetPredicate(
  (widget) => widget is PaneChrome && widget.phone != null,
);

const _cantShow = "Claktory can't show this kind of file here yet.";

void main() {
  testWidgets('narrowed with Needs you focused, the window opens on the '
      'Needs you page; Back goes to the conversation and the pane stays', (
    tester,
  ) async {
    asIPad();
    final focus = WorkbenchFocus();
    addTearDown(focus.dispose);
    final harness = Harness(snapshot(layout: _layout));
    await harness.show(tester, size: _wide, home: _window(focus));
    await pumpFrames(tester);
    focus.focus('a1', 'needs');
    await pumpFrames(tester);
    expect(find.byType(Workbench), findsOneWidget);

    await harness.show(tester, size: _narrow, home: _window(focus));
    await pumpFrames(tester);
    expect(find.byType(PhoneShell), findsOneWidget);
    // The phone's Needs you: the list first.
    expect(find.byType(InboxView), findsOneWidget);
    expect(_viewers, findsNothing);
    expect(find.text(_cantShow), findsNothing);
    final page = AppScope.scope(tester.element(find.byType(Shell))).page;
    expect(page.value, AppPage.inbox);

    // Widened again, the workbench, Needs you in its pane; narrowed, the
    // page again.
    await harness.show(tester, size: _wide, home: _window(focus));
    await pumpFrames(tester);
    expect(find.byType(Workbench), findsOneWidget);
    expect(find.byType(InboxView), findsOneWidget);
    expect(focus.focusedIn('a1', _layout), 'needs');
    // In its pane, not the page over the workbench.
    expect(page.value, AppPage.agent);
    await harness.show(tester, size: _narrow, home: _window(focus));
    await pumpFrames(tester);
    expect(find.byType(InboxView), findsOneWidget);
    expect(find.text(_cantShow), findsNothing);

    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(page.value, AppPage.agent);
    expect(find.byType(InboxView), findsNothing);
    expect(_viewers, findsNothing);
    expect(layoutsSent(harness), isEmpty);
  });

  testWidgets('narrowed with a page focused, the page is in front from the '
      "first frame: never a frame of the conversation alone, which would let "
      "the page's state go (Sale's row 149)", (tester) async {
    asIPad();
    final focus = WorkbenchFocus();
    addTearDown(focus.dispose);
    final harness = Harness(snapshot(layout: _layout));
    await harness.show(tester, size: _wide, home: _window(focus));
    await pumpFrames(tester);
    focus.focus('a1', 'needs');
    await pumpFrames(tester);

    // Narrowed, one frame: the page is there in it.
    tester.view.physicalSize = _narrow;
    tester.view.display.size = _narrow;
    await tester.pump();
    expect(find.byType(PhoneShell), findsOneWidget);
    expect(find.byType(InboxView), findsOneWidget);
    final first = tester.state(find.byType(InboxView));
    await pumpFrames(tester);
    // The same page, kept: not one made again a frame later.
    expect(tester.state(find.byType(InboxView)), same(first));
    final page = AppScope.scope(tester.element(find.byType(Shell))).page;
    expect(page.value, AppPage.inbox);
  });

  testWidgets('on a phone, showPane on a page pane opens the page', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(snapshot(layout: _layout));
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    final routes = tester
        .widget<PhoneRoutesScope>(find.byType(PhoneRoutesScope))
        .routes;
    expect(routes.showPane('a1', 'needs'), isTrue);
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.byType(InboxView), findsOneWidget);
    expect(_viewers, findsNothing);
    expect(find.text(_cantShow), findsNothing);

    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.byType(InboxView), findsNothing);
    expect(layoutsSent(harness), isEmpty);
  });
}
