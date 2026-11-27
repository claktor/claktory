// A move made in a phone's drawer keeps its Undo when the drawer closes
// before the Undo's time is up: at the foot of the screen, for the rest of
// that time, and back on the row only once the drawer is out again.
import 'package:factory_app/mobile/move_undo.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/phone.dart' show PhoneShell;
import 'package:factory_app/sidebar.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_test.dart' show menuButton, showPhone;

Workspace _workspace() => Workspace(
  projects: [Project(id: 'p1', name: 'Claktory')],
  agents: [
    Agent(id: 'mike', name: 'Mike', projectId: 'p1', modelAvailable: true),
    Agent(
      id: 'sam',
      name: 'Sam',
      projectId: 'p1',
      leadId: 'mike',
      parentId: 'mike',
      modelAvailable: true,
    ),
    Agent(id: 'solo', name: 'Solo', projectId: 'p1', modelAvailable: true),
  ],
  selectedAgentId: 'mike',
);

Finder _inSidebar(Finder finder) =>
    find.descendant(of: find.byType(Sidebar), matching: finder);

Finder _atFoot(Finder finder) =>
    find.descendant(of: find.byType(PhoneMoveUndo), matching: finder);

/// Opens the drawer and drags Solo onto [lead] with a finger: held, then
/// moved.
Future<void> _move(WidgetTester tester, String lead) async {
  await tester.tap(menuButton());
  await pumpFrames(tester);
  final from = tester.getCenter(_inSidebar(find.text('Solo')));
  final to = tester.getCenter(_inSidebar(find.text(lead)));
  final finger = await tester.startGesture(from, kind: PointerDeviceKind.touch);
  await tester.pump(const Duration(milliseconds: 700));
  for (var i = 1; i <= 10; i++) {
    await finger.moveTo(Offset.lerp(from, to, i / 10)!);
    await tester.pump(const Duration(milliseconds: 16));
  }
  await finger.up();
  await settle(tester);
}

/// A tap on the dimmed conversation beside the drawer.
Future<void> _closeDrawer(WidgetTester tester) async {
  final screen = tester.getSize(find.byType(PhoneShell));
  await tester.tapAt(Offset(screen.width - 8, screen.height / 2));
  await pumpFrames(tester);
}

void main() {
  testWidgets('the drawer closed within the time, Undo is at the foot', (
    tester,
  ) async {
    final harness = await showPhone(tester, _workspace());
    harness.backend.arranged = Arranged(token: 'tok-1');
    await _move(tester, 'Sam');
    expect(harness.last.arrangeAgent.parentId, 'sam');
    // In the drawer, the row has it; nothing at the foot yet.
    expect(_inSidebar(find.text('Now reports to Sam')), findsOneWidget);
    expect(_atFoot(find.text('Undo')), findsNothing);

    await _closeDrawer(tester);
    expect(find.byType(Sidebar), findsNothing);
    expect(_atFoot(find.text('Solo')), findsOneWidget);
    expect(_atFoot(find.text('Now reports to Sam')), findsOneWidget);
    final undo = tester.getSize(
      find
          .ancestor(
            of: _atFoot(find.text('Undo')),
            matching: find.byType(ConstrainedBox),
          )
          .first,
    );
    expect(undo.height, greaterThanOrEqualTo(44));
    expect(undo.width, greaterThanOrEqualTo(44));
    // At the foot, over nothing else: the menu is still in reach.
    final bar = tester.getRect(
      find
          .descendant(
            of: find.byType(PhoneMoveUndo),
            matching: find.byType(Container),
          )
          .first,
    );
    final screen = tester.getSize(find.byType(PhoneShell));
    expect(bar.bottom, greaterThan(screen.height - 60));
    expect(bar.height, lessThan(100));

    // Out again, it's the row's.
    await tester.tap(menuButton());
    await pumpFrames(tester);
    expect(_inSidebar(find.text('Now reports to Sam')), findsOneWidget);
    expect(_atFoot(find.text('Undo')).hitTestable(), findsNothing);
    await _closeDrawer(tester);

    await tester.tap(_atFoot(find.text('Undo')));
    await pumpFrames(tester);
    expect(harness.last.restoreArrangement.token, 'tok-1');
    expect(_atFoot(find.text('Undo')), findsNothing);
  });

  testWidgets('its time up, the foot lets it go', (tester) async {
    final harness = await showPhone(tester, _workspace());
    harness.backend.arranged = Arranged(token: 'tok-2');
    await _move(tester, 'Mike');
    await _closeDrawer(tester);
    expect(_atFoot(find.text('Now reports to Mike')), findsOneWidget);
    // The time counts from the drop, before the drawer closed.
    await tester.pump(const Duration(seconds: 5));
    expect(_atFoot(find.text('Undo')).hitTestable(), findsOneWidget);
    await tester.pump(const Duration(seconds: 3));
    await pumpFrames(tester);
    expect(_atFoot(find.text('Undo')), findsNothing);
    expect(
      harness.backend.sent.where((sent) => sent.hasRestoreArrangement()),
      isEmpty,
    );
  });

  testWidgets('a drawer left open to the end never shows it at the foot', (
    tester,
  ) async {
    final harness = await showPhone(tester, _workspace());
    harness.backend.arranged = Arranged(token: 'tok-3');
    await _move(tester, 'Sam');
    await tester.pump(const Duration(seconds: 8));
    await pumpFrames(tester);
    await _closeDrawer(tester);
    expect(_atFoot(find.text('Undo')), findsNothing);
  });

  testWidgets('at the largest text, the words are whole and Undo under them', (
    tester,
  ) async {
    tester.platformDispatcher.textScaleFactorTestValue = 53 / 17;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final harness = await showPhone(tester, _workspace());
    harness.backend.arranged = Arranged(token: 'tok-4');
    await _move(tester, 'Sam');
    await _closeDrawer(tester);
    final says = _atFoot(find.text('Now reports to Sam'));
    expect(says, findsOneWidget);
    expect(tester.widget<Text>(says).maxLines, isNull);
    final undo = tester.getRect(_atFoot(find.text('Undo')));
    expect(undo.top, greaterThanOrEqualTo(tester.getRect(says).bottom));
    expect(tester.takeException(), isNull);
  });

  testWidgets('a swipe down puts the bar away early', (tester) async {
    final harness = await showPhone(tester, _workspace());
    harness.backend.arranged = Arranged(token: 'tok-5');
    await _move(tester, 'Sam');
    await _closeDrawer(tester);
    final bar = _atFoot(find.text('Solo'));
    // A short pull springs back.
    await tester.drag(bar, const Offset(0, 10));
    await pumpFrames(tester);
    expect(_atFoot(find.text('Undo')).hitTestable(), findsOneWidget);
    await tester.drag(bar, const Offset(0, 60));
    await pumpFrames(tester);
    expect(_atFoot(find.text('Undo')), findsNothing);
    expect(
      harness.backend.sent.where((sent) => sent.hasRestoreArrangement()),
      isEmpty,
    );
    // The row still has it.
    await tester.tap(menuButton());
    await pumpFrames(tester);
    expect(_inSidebar(find.text('Now reports to Sam')), findsOneWidget);
  });
}
