// Pipe groups (#359): a project's pipes gathered under named rows in the
// sidebar, folded away the same on every device. A pipe moves to a group
// from its right click or touch and hold (Move to group…, New group…,
// Remove from group), or by a pointer dragging it onto the group's row; a
// group renames and deletes from its own (its pipes stay). Each is a
// request: the rows only show what the Claktory says.
import 'package:factory_app/pipe_groups.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';

Pipe _pipe(String id, String name, {String group = '', bool unseen = false}) =>
    Pipe(
      id: id,
      name: name,
      projectId: 'p1',
      groupId: group,
      unseen: unseen,
      createdMs: Int64(int.parse(id.substring(1))),
      state: PipeState.PIPE_STATE_OPEN,
      online: true,
      peer: PipePeer(claktoryName: '$name Claktory'),
    );

Workspace _workspace({bool folded = false, bool unseen = false}) => Workspace(
  projects: [Project(id: 'p1', name: 'Friend Feedback', path: '/w')],
  agents: [
    Agent(id: 'mike', name: 'Mike', projectId: 'p1', modelAvailable: true),
  ],
  pipeGroups: [
    // Out of order on the wire: the rows follow position.
    PipeGroup(id: 'g2', projectId: 'p1', name: 'Launch', position: 1),
    PipeGroup(
      id: 'g1',
      projectId: 'p1',
      name: 'Friend feedback',
      position: 0,
      collapsed: folded,
    ),
  ],
  pipes: [
    _pipe('p3', 'Feedback · Ana', group: 'g1', unseen: unseen),
    _pipe('p4', 'Feedback · Ben', group: 'g1'),
    _pipe('p5', 'Acme launch'),
    // Its group is gone: it shows with the pipes in none.
    _pipe('p6', 'Old pipe', group: 'g9'),
  ],
  selectedAgentId: 'mike',
);

Finder _row(String id) => find.byKey(ValueKey('pipe-row/$id'));
Finder _group(String id) => find.byKey(ValueKey('pipe-group/$id'));

double _top(WidgetTester tester, Finder finder) => tester.getTopLeft(finder).dy;

void main() {
  testWidgets('groups first, each with its pipes, then the pipes in none; '
      'a tap folds a group away', (tester) async {
    final harness = Harness(_workspace());
    await harness.show(tester);
    await settle(tester);
    final order = [
      _group('g1'),
      _row('p3'),
      _row('p4'),
      _group('g2'),
      find.text('No pipes'),
      _row('p5'),
      _row('p6'),
    ];
    for (var i = 1; i < order.length; i++) {
      expect(
        _top(tester, order[i]),
        greaterThan(_top(tester, order[i - 1])),
        reason: '$i',
      );
    }
    // Its pipes sit in under it.
    expect(
      tester.getTopLeft(_row('p3')).dx,
      greaterThan(tester.getTopLeft(_group('g1')).dx),
    );
    expect(
      find.descendant(of: _group('g1'), matching: find.text('2')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: _group('g1'), matching: find.byType(PipeGroupGlyph)),
      findsOneWidget,
    );

    await tester.tap(_group('g1'));
    await pumpFrames(tester);
    final folded = harness.last.setPipeGroupCollapsed;
    expect(folded.groupId, 'g1');
    expect(folded.collapsed, isTrue);
  });

  testWidgets('folded, a group hides its pipes and says what is new', (
    tester,
  ) async {
    final semantics = tester.ensureSemantics();
    final harness = Harness(_workspace(folded: true, unseen: true));
    await harness.show(tester);
    await settle(tester);
    expect(_row('p3'), findsNothing);
    expect(_row('p5'), findsOneWidget);
    expect(
      find.bySemanticsLabel(
        'Friend feedback, group of 2 pipes, collapsed, new messages',
      ),
      findsOneWidget,
    );
    expect(
      find.bySemanticsLabel(RegExp('^Launch, group of 0 pipes, expanded')),
      findsOneWidget,
    );
    semantics.dispose();
  });

  testWidgets("a pipe's right click moves it to a group, a new one, or out "
      'of its own', (tester) async {
    final harness = Harness(_workspace());
    await harness.show(tester);
    await settle(tester);

    // Into another group: the one it's in isn't offered.
    await tester.tap(_row('p3'), buttons: kSecondaryButton);
    await settle(tester);
    expect(find.text('Remove from group'), findsOneWidget);
    await tester.tap(find.text('Move to group…'));
    await settle(tester);
    expect(find.text('New group…'), findsOneWidget);
    expect(find.text('Friend feedback'), findsOneWidget); // its row only
    await tester.tap(find.text('Launch').last);
    await settle(tester);
    var moved = harness.last.setPipeGroup;
    expect((moved.pipeId, moved.groupId), ('p3', 'g2'));

    // Out of its group.
    await tester.tap(_row('p4'), buttons: kSecondaryButton);
    await settle(tester);
    await tester.tap(find.text('Remove from group'));
    await settle(tester);
    moved = harness.last.setPipeGroup;
    expect((moved.pipeId, moved.groupId), ('p4', ''));

    // A pipe in none has nothing to leave; New group… takes it along.
    await tester.tap(_row('p5'), buttons: kSecondaryButton);
    await settle(tester);
    expect(find.text('Remove from group'), findsNothing);
    await tester.tap(find.text('Move to group…'));
    await settle(tester);
    await tester.tap(find.text('New group…'));
    await settle(tester);
    expect(find.text('New group'), findsOneWidget);
    // A name the Claktory refuses says why, and the form stays.
    harness.backend.refuse = 'This project already has a group named Launch.';
    await tester.enterText(find.byType(EditableText).last, 'Launch');
    await tester.tap(find.text('Create'));
    await settle(tester);
    expect(
      find.text('This project already has a group named Launch.'),
      findsOneWidget,
    );
    harness.backend.refuse = null;
    await tester.enterText(find.byType(EditableText).last, ' Customers ');
    await tester.tap(find.text('Create'));
    await settle(tester);
    final created = harness.last.createPipeGroup;
    expect(
      (created.projectId, created.name, created.pipeId),
      ('p1', 'Customers', 'p5'),
    );
    expect(find.text('New group'), findsNothing);
  });

  testWidgets('a move that fails says so even after the rows shift under '
      'its open menu (Abbacchio, row 890)', (tester) async {
    // Delta first, above the others.
    Workspace withDelta() {
      final workspace = _workspace();
      for (final group in workspace.pipeGroups) {
        group.position += 1;
      }
      return workspace
        ..pipeGroups.add(
          PipeGroup(id: 'g0', projectId: 'p1', name: 'Delta', position: 0),
        );
    }

    final harness = Harness(withDelta());
    await harness.show(tester);
    await settle(tester);
    Future<void> openMove(String pipe) async {
      await tester.tap(_row(pipe), buttons: kSecondaryButton);
      await settle(tester);
      await tester.tap(find.text('Move to group…'));
      await settle(tester);
    }

    // A group above it deleted from another device while the menu's open.
    await openMove('p3');
    harness.backend.workspace.apply(
      _workspace()..pipeGroups.removeWhere((group) => group.id == 'g0'),
    );
    await settle(tester);
    harness.backend.refuse = 'That group is gone.';
    await tester.tap(find.text('Launch').last);
    await settle(tester);
    expect(harness.last.setPipeGroup.pipeId, 'p3');
    expect(find.text("Couldn't move Feedback · Ana"), findsOneWidget);
    expect(find.text('That group is gone.'), findsOneWidget);
    await tester.tapAt(const Offset(1100, 40));
    await settle(tester);

    // Its own group deleted: the row is another one now, among the pipes
    // in none; the list says it.
    harness.backend.refuse = null;
    await openMove('p4');
    final gone = _workspace()..pipeGroups.removeWhere((g) => g.id == 'g1');
    for (final pipe in gone.pipes) {
      if (pipe.groupId == 'g1') pipe.groupId = '';
    }
    harness.backend.workspace.apply(gone);
    await settle(tester);
    harness.backend.refuse = 'That group is gone.';
    await tester.tap(find.text('Launch').last);
    await settle(tester);
    expect(harness.last.setPipeGroup.pipeId, 'p4');
    expect(find.text("Couldn't move Feedback · Ben"), findsOneWidget);
    harness.backend.refuse = null;
  });

  testWidgets("a group's right click renames it, or deletes it saying its "
      'pipes stay', (tester) async {
    final harness = Harness(_workspace());
    await harness.show(tester);
    await settle(tester);

    await tester.tap(_group('g1'), buttons: kSecondaryButton);
    await settle(tester);
    await tester.tap(find.text('Rename…'));
    await settle(tester);
    expect(find.text('Rename group'), findsOneWidget);
    await tester.enterText(find.byType(EditableText).last, 'Friends');
    await tester.tap(find.text('Save'));
    await settle(tester);
    final renamed = harness.last.renamePipeGroup;
    expect((renamed.groupId, renamed.name), ('g1', 'Friends'));

    await tester.tap(_group('g1'), buttons: kSecondaryButton);
    await settle(tester);
    await tester.tap(find.text('Delete group…'));
    await settle(tester);
    expect(find.text('Delete the group “Friend feedback”?'), findsOneWidget);
    expect(
      find.text(
        'Its pipes stay, just outside any group. Nothing about them changes.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Delete group'));
    await settle(tester);
    expect(harness.last.deletePipeGroup.groupId, 'g1');
  });

  testWidgets("a pointer drags a pipe onto a group's row", (tester) async {
    final harness = Harness(_workspace());
    await harness.show(tester);
    await settle(tester);
    final gesture = await tester.startGesture(
      tester.getCenter(_row('p5')),
      kind: PointerDeviceKind.mouse,
    );
    await gesture.moveBy(const Offset(0, -10));
    await tester.pump();
    await gesture.moveTo(tester.getCenter(_group('g2')));
    await tester.pump();
    await gesture.up();
    await settle(tester);
    final moved = harness.last.setPipeGroup;
    expect((moved.pipeId, moved.groupId), ('p5', 'g2'));
  });

  testWidgets("on an iPad a touch and hold is the pipe's menu, not a drag", (
    tester,
  ) async {
    asIPad();
    final harness = Harness(_workspace());
    await harness.show(tester, size: const Size(1032, 1376));
    await settle(tester);
    await tester.longPress(_row('p5'));
    await settle(tester);
    expect(find.text('Move to group…'), findsOneWidget);
    expect(harness.backend.sent.where((m) => m.hasSetPipeGroup()), isEmpty);
  });

  testWidgets("in a phone's drawer: the group's row, a finger's size, its "
      'pipes under it', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_workspace());
    await harness.show(tester, size: const Size(390, 844));
    await settle(tester);
    await tester.tap(find.bySemanticsLabel('Menu'));
    await pumpFrames(tester);
    expect(tester.getSize(_group('g1')).height, greaterThanOrEqualTo(36));
    expect(_top(tester, _row('p3')), greaterThan(_top(tester, _group('g1'))));
    await tester.longPress(_group('g1'));
    await settle(tester);
    expect(find.text('Delete group…'), findsOneWidget);
    expect(harness.backend.sent.where((m) => m.hasSetPipeGroup()), isEmpty);
  });
}
