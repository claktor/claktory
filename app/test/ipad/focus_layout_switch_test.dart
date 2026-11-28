// Focus mode through the window crossing into the phone's layout and back,
// with the window built as main.dart builds it:
// the phone's layout puts the Shell under PhoneClaktories, a wider one
// gives it the rail, so the Shell is a new one each time the window
// crosses. The sitting and its card must live through that, and in the
// narrow window a tap on who asks must slide Context over the card.
import 'package:factory_app/identity.dart' show isPhoneLayout;
import 'package:factory_app/inbox.dart' show NeedsYouCard;
import 'package:factory_app/ipad/focus.dart';
import 'package:factory_app/ipad/workbench.dart'
    show WorkbenchFocus, WorkbenchScope;
import 'package:factory_app/mobile/needs_you_phone.dart' show PhoneNeedsYou;
import 'package:factory_app/needs_you_focus.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart' show PhoneClaktories;
import 'package:factory_app/shell.dart' show Shell;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import 'as_ipad.dart';

final _base = DateTime(2026, 11, 26, 9);

Entry _ask(int seq, int minutesAgo) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(
    _base.subtract(Duration(minutes: minutesAgo)).millisecondsSinceEpoch,
  ),
  askPerson: AskPerson(id: 'q$seq', title: 'Question $seq'),
);

Workspace _waiting() => Workspace(
  agents: [
    Agent(
      id: 'june',
      name: 'June',
      projectId: 'p1',
      waitingSeqs: [Int64(1), Int64(2)],
    ),
    Agent(id: 'mista', name: 'Mista', waitingSeqs: [Int64(7)]),
  ],
  projects: [Project(id: 'p1', name: 'Factory')],
);

/// The window as main.dart's _buildWindow lays it out.
class _Window extends StatelessWidget {
  const _Window();

  @override
  Widget build(BuildContext context) => isPhoneLayout(context)
      ? const PhoneClaktories(shell: Shell(key: ValueKey('shown')))
      : const Shell(key: ValueKey('shown'), rail: SizedBox(width: 56));
}

Future<Harness> _show(WidgetTester tester, Size size) async {
  asIPad();
  final harness = Harness(_waiting());
  harness.backend.entriesById
    ..['june/1'] = _ask(1, 30)
    ..['june/2'] = _ask(2, 60)
    ..['mista/7'] = _ask(7, 5);
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  await harness.show(
    tester,
    size: size,
    home: WorkbenchScope(focus: focus, child: const _Window()),
  );
  await settle(tester);
  return harness;
}

Future<void> _resize(WidgetTester tester, Size size) async {
  tester.view.physicalSize = size;
  tester.view.display.size = size;
  await settle(tester);
}

String? _card(WidgetTester tester) =>
    tester.widget<IPadFocus>(find.byType(IPadFocus)).session.current?.key;

void main() {
  for (final (from, to) in [
    (const Size(809, 1024), const Size(504, 1024)),
    (const Size(504, 1024), const Size(809, 1024)),
    (const Size(1194, 834), const Size(375, 1024)),
  ]) {
    testWidgets('the card stays from ${from.width.toInt()} to '
        '${to.width.toInt()} points and back', (tester) async {
      final harness = await _show(tester, from);
      setFocusMode(harness.backend, true);
      await settle(tester);
      final session = tester.widget<IPadFocus>(find.byType(IPadFocus)).session;
      session.next();
      await settle(tester);
      final card = _card(tester);
      expect(card, isNot(session.waiting.first));
      await _resize(tester, to);
      expect(find.byType(IPadFocus), findsOneWidget);
      expect(_card(tester), card);
      await _resize(tester, from);
      expect(_card(tester), card);
      expect(session.disposed, isFalse);
      // Left: the sitting ends with it, whichever window showed it.
      setFocusMode(harness.backend, false);
      await settle(tester);
      expect(find.byType(IPadFocus), findsNothing);
      expect(session.disposed, isTrue);
    });
  }

  for (final from in [
    const Size(504, 1024),
    const Size(809, 1024),
    const Size(320, 1024),
  ]) {
    testWidgets('narrow, from ${from.width.toInt()}: a tap on who asks slides '
        'Context over the card', (tester) async {
      final harness = await _show(tester, from);
      setFocusMode(harness.backend, true);
      await settle(tester);
      await _resize(tester, from.width > 600 ? const Size(504, 1024) : from);
      expect(find.byType(PhoneNeedsYou), findsOneWidget);
      expect(find.byType(FocusContext), findsNothing);
      await tester.tap(
        find
            .descendant(
              of: find.byType(PhoneNeedsYou),
              matching: find.text('Mista'),
            )
            .first,
      );
      await settle(tester);
      expect(find.byType(FocusContext), findsOneWidget);
      // Wide enough to read: over the card, a strip left.
      final panel = tester.getRect(find.byType(FocusContext));
      expect(panel.width, greaterThan(200));
      expect(panel.left, greaterThan(0));
      expect(find.byType(NeedsYouCard), findsWidgets);
    });
  }
}
