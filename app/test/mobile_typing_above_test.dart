// On a phone the message box steps aside while a Needs you card's field in
// the conversation has the keyboard, and comes back as it was, draft and
// all (Lucy, needs-you-cards.md §5.2). On a Mac it stays.
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/message_box.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace() => Workspace(
  projects: [Project(id: 'p1', name: 'Factory')],
  agents: [
    Agent(
      id: 'june',
      name: 'June',
      projectId: 'p1',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
      waitingSeqs: [Int64(4)],
    ),
  ],
  selectedAgentId: 'june',
);

Entry _ask() => Entry(
  seq: Int64(4),
  timestampMs: Int64(1004),
  askPerson: AskPerson(
    id: 'ask4',
    title: 'Which region should the bucket live in?',
    wantsText: true,
    revision: 1,
  ),
);

Future<void> _show(WidgetTester tester, {required bool phone}) async {
  if (phone) debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final harness = Harness(_workspace());
  harness.backend.transcript.load(
    Transcript(agentId: 'june', entries: [_ask()]),
  );
  await harness.show(
    tester,
    size: phone ? const Size(375, 667) : const Size(1200, 900),
  );
  await settle(tester);
  await pumpFrames(tester);
}

Finder get _cardField => find.descendant(
  of: find.byType(AskPersonCard),
  matching: find.byType(EditableText),
);

Finder _boxField({bool skipOffstage = true}) => find.descendant(
  of: find.byType(MessageBox, skipOffstage: skipOffstage),
  matching: find.byType(EditableText, skipOffstage: skipOffstage),
  skipOffstage: skipOffstage,
);

/// The keyboard's height on screen, in points (0: none).
Future<void> _keyboard(WidgetTester tester, double height) async {
  tester.view.viewInsets = FakeViewPadding(bottom: height);
  addTearDown(tester.view.resetViewInsets);
  await tester.pump();
}

void main() {
  testWidgets('iPhone: the box steps aside while the card has the keyboard '
      'and comes back with its draft', (tester) async {
    await _show(tester, phone: true);
    expect(find.byType(MessageBox), findsOneWidget);

    // Half a message in the box first.
    await tester.showKeyboard(_boxField());
    await tester.enterText(_boxField(), 'half a thought');
    await tester.pump();

    // Then the card's field, the keyboard up: the box goes, kept.
    await tester.showKeyboard(_cardField);
    await _keyboard(tester, 300);
    expect(find.byType(MessageBox), findsNothing);
    expect(find.byType(MessageBox, skipOffstage: false), findsOneWidget);
    expect(tester.widget<EditableText>(_cardField).focusNode.hasFocus, isTrue);

    // The card lets go: the box is back, its draft as it was.
    FocusManager.instance.primaryFocus?.unfocus();
    await _keyboard(tester, 0);
    expect(find.byType(MessageBox), findsOneWidget);
    expect(
      tester.widget<EditableText>(_boxField()).controller.text,
      'half a thought',
    );

    // Typing in the box itself never hides it.
    await tester.showKeyboard(_boxField());
    await _keyboard(tester, 300);
    expect(find.byType(MessageBox), findsOneWidget);
  });

  testWidgets('iPhone or iPad: no keyboard on screen, no reason to hide the '
      'box, even while the card keeps the focus (Bruno 45806)', (tester) async {
    await _show(tester, phone: true);
    // A hardware keyboard: the card's field has the focus, nothing shows.
    await tester.showKeyboard(_cardField);
    await _keyboard(tester, 0);
    expect(find.byType(MessageBox), findsOneWidget);
    // An iPad's keyboard shrunk to its shortcut bar.
    await _keyboard(tester, 60);
    expect(find.byType(MessageBox), findsOneWidget);
    // The keyboard itself: the box steps aside.
    await _keyboard(tester, 300);
    expect(find.byType(MessageBox), findsNothing);
    // Put away while the field keeps the focus: the box comes back.
    await _keyboard(tester, 60);
    expect(tester.widget<EditableText>(_cardField).focusNode.hasFocus, isTrue);
    expect(find.byType(MessageBox), findsOneWidget);
  });

  testWidgets('Mac: the box stays while the card has the focus', (
    tester,
  ) async {
    await _show(tester, phone: false);
    await tester.showKeyboard(_cardField);
    await _keyboard(tester, 300);
    expect(find.byType(MessageBox), findsOneWidget);
  });
}
