// Needs you's way into focus mode on an iPad (Lucy's needs-you-cards.md §4
// item 4, Bruno 46924; Polnareff's FocusModeButton): on a full-width iPad
// with no keyboard, ⌘⇧N isn't there, so the button is the way in. It's in
// Needs you's tabs row wherever Needs you shows in the workbench: the page,
// a pane beside the conversation, and a pane squeezed below a page's
// width. A finger's target (44 pt), read as "Focus on Needs you", and a
// tap opens the iPad's focus mode. In a narrow window Needs you is the
// list too (#599): a request tapped there opens alone, and Focus mode
// opens the iPad's one card at a time.
import 'package:factory_app/inbox.dart' show InboxView;
import 'package:factory_app/ipad/focus.dart' show IPadFocus;
import 'package:factory_app/mobile/header.dart' show HeaderButton;
import 'package:factory_app/ipad/workbench.dart'
    show WorkbenchFocus, WorkbenchScope;
import 'package:factory_app/mobile/needs_you_phone.dart' show PhoneNeedsYou;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart' show AppPage, AppScope;
import 'package:factory_app/shell.dart' show Shell;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import '../panes_test.dart' show beside, snapshot;
import 'as_ipad.dart';

Entry _ask(int seq) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(DateTime(2026, 11, 26, 9, seq).millisecondsSinceEpoch),
  askPerson: AskPerson(id: 'q$seq', title: 'Question $seq'),
);

/// Builder with [layout], and June waiting on two cards.
Future<Harness> _show(
  WidgetTester tester,
  Size size, {
  ViewNode? layout,
}) async {
  asIPad();
  final workspace = snapshot(layout: layout)
    ..agents.add(
      Agent(
        id: 'june',
        name: 'June',
        projectId: 'p1',
        modelAvailable: true,
        waitingSeqs: [Int64(1), Int64(2)],
      ),
    );
  final harness = Harness(workspace)..backend.appliesLayouts = true;
  harness.backend.entriesById
    ..['june/1'] = _ask(1)
    ..['june/2'] = _ask(2);
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  await harness.show(
    tester,
    size: size,
    home: WorkbenchScope(
      focus: focus,
      child: const Shell(rail: SizedBox(width: 56)),
    ),
  );
  await settle(tester);
  return harness;
}

final _button = find.text('Focus mode');

Future<void> _goesIn(WidgetTester tester) async {
  expect(_button, findsOneWidget);
  final target = find.bySemanticsLabel('Focus on Needs you');
  expect(target, findsOneWidget);
  final size = tester.getSize(target);
  expect(size.height, greaterThanOrEqualTo(44));
  expect(size.width, greaterThanOrEqualTo(44));
  await tester.ensureVisible(_button);
  await settle(tester);
  await tester.tap(_button);
  await settle(tester);
  expect(find.byType(IPadFocus), findsOneWidget);
}

void main() {
  testWidgets('Needs you as the page: Focus mode opens the iPad\'s focus '
      'mode', (tester) async {
    await _show(tester, const Size(1366, 1024));
    await tester.tap(find.text('Needs you').first);
    await settle(tester);
    await _goesIn(tester);
  });

  for (final size in [
    const Size(1366, 1024),
    const Size(1024, 768),
    const Size(834, 1194),
  ]) {
    testWidgets('Needs you as a pane beside the conversation at '
        '${size.width.toInt()} points', (tester) async {
      await _show(tester, size, layout: beside('inbox', ''));
      // Portrait shows one pane at a time: Needs you's, once it's read.
      WorkbenchScope.of(tester.element(find.byType(Shell))).focus('a1', 'v1');
      await settle(tester);
      await _goesIn(tester);
    });
  }

  testWidgets('a narrow window (#599): Needs you is the list, and its Focus '
      'mode opens the iPad\'s one card at a time; leaving comes back to the '
      'list', (tester) async {
    await _show(tester, const Size(504, 1024));
    AppScope.openPage(tester.element(find.byType(Shell)), AppPage.inbox);
    await settle(tester);
    expect(find.byType(InboxView), findsOneWidget);
    expect(find.byType(PhoneNeedsYou), findsNothing);
    await _goesIn(tester);
    expect(find.byType(PhoneNeedsYou), findsOneWidget);
    await tester.tap(
      find.descendant(
        of: find.byType(PhoneNeedsYou),
        matching: find.byWidgetPredicate(
          (w) => w is HeaderButton && w.label == 'Back',
        ),
      ),
    );
    await settle(tester);
    expect(find.byType(IPadFocus), findsNothing);
    expect(find.byType(InboxView), findsOneWidget);
  });

  testWidgets('narrowed from a wide window on the list: still the list, '
      'not one card', (tester) async {
    await _show(tester, const Size(1366, 1024));
    await tester.tap(find.text('Needs you').first);
    await settle(tester);
    expect(find.byType(InboxView), findsOneWidget);
    for (final width in [504.0, 375.0, 320.0]) {
      tester.view.physicalSize = Size(width, 1024);
      tester.view.display.size = Size(width, 1024);
      await settle(tester);
      expect(find.byType(InboxView), findsOneWidget, reason: '$width');
      expect(find.byType(PhoneNeedsYou), findsNothing, reason: '$width');
    }
  });

  testWidgets('narrow: a request tapped in the list opens alone, not '
      'focus mode; Back, the list', (tester) async {
    await _show(tester, const Size(504, 1024));
    AppScope.openPage(tester.element(find.byType(Shell)), AppPage.inbox);
    await pumpFrames(tester);
    await tester.pump();
    await pumpFrames(tester);
    final row = find.descendant(
      of: find.byType(InboxView),
      matching: find.text('Question 1'),
    );
    expect(row, findsOneWidget);
    await tester.tap(row);
    await pumpFrames(tester);
    expect(find.byType(PhoneNeedsYou), findsOneWidget);
    expect(find.byType(IPadFocus), findsNothing);
    // Back, the list (answered there, Pesci's mobile_needs_you_test).
    await tester.tap(
      find.descendant(
        of: find.byType(PhoneNeedsYou),
        matching: find.byWidgetPredicate(
          (w) => w is HeaderButton && w.label == 'Back',
        ),
      ),
    );
    await pumpFrames(tester);
    expect(find.byType(PhoneNeedsYou), findsNothing);
    expect(find.byType(InboxView), findsOneWidget);
  });
}
