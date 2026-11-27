// Needs you's focus mode on an iPad (#514, the spec's §6): the queue, the
// stage and Context laid out for the window's tier, a finger's way to
// everything the keys do, and focus mode's ⌘-commands in the system's
// menu while it's on (the workbench's, with ⌘⇧N to enter, while it's off).
import 'package:factory_app/ipad/focus.dart';
import 'package:factory_app/ipad/focus_columns.dart';
import 'package:factory_app/mobile/header.dart' show HeaderButton;
import 'package:factory_app/mobile/needs_you_phone.dart' show PhoneNeedsYou;
import 'package:factory_app/ipad/workbench.dart'
    show WorkbenchFocus, WorkbenchScope;
import 'package:factory_app/card_answer.dart' show AnswerDock, CardAnswer;
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/identity.dart' show Palette;
import 'package:factory_app/inbox.dart' show NeedsYouCard;
import 'package:factory_app/needs_you_focus.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/rendering.dart' show RenderParagraph;
import 'package:flutter/services.dart';
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

/// June waits on two cards, Mista on one (the newest).
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

const _channel = MethodChannel('factory/commands');

/// What the system's menu was last given, by id: title and enabled.
final _menu = <String, (String, bool)>{};

Future<Harness> _show(WidgetTester tester, Size size) async {
  asIPad();
  _menu.clear();
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  messenger.setMockMethodCallHandler(_channel, (call) async {
    if (call.method != 'setCommands') return null;
    _menu.clear();
    for (final command in (call.arguments as Map)['commands'] as List) {
      final map = command as Map;
      _menu[map['id'] as String] = (
        map['title'] as String,
        map['enabled'] as bool,
      );
    }
    return true;
  });
  addTearDown(() => messenger.setMockMethodCallHandler(_channel, null));
  final harness = Harness(_waiting());
  harness.backend.entriesById
    ..['june/1'] = _ask(1, 30)
    ..['june/2'] = _ask(2, 60)
    ..['mista/7'] = _ask(7, 5);
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  // The window as main.dart builds it: the focus above the Shell, which
  // has a rail, so an iPad's is the workbench.
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

Future<void> _enter(WidgetTester tester, Harness harness) async {
  setFocusMode(harness.backend, true);
  await settle(tester);
}

Finder _onStage(String text) =>
    find.descendant(of: find.byType(FocusStage), matching: find.text(text));

final _contextButton = find.byWidgetPredicate(
  (widget) => widget is Button && widget.label == 'Context',
);

void main() {
  testWidgets('13" landscape: queue, stage and Context side by side; the '
      'Context button hides it and shows it again', (tester) async {
    final harness = await _show(tester, const Size(1376, 1032));
    await _enter(tester, harness);
    expect(find.byType(IPadFocus), findsOneWidget);
    expect(tester.getSize(find.byType(FocusQueue)).width, FocusGeometry.queue);
    expect(_onStage('Question 7'), findsOneWidget);
    expect(find.byType(FocusContext), findsOneWidget);
    // The button looks on while its pane is open, by touch too.
    bool on() {
      final fill = find
          .descendant(of: _contextButton, matching: find.byType(Container))
          .first;
      final color =
          (tester.widget<Container>(fill).decoration! as BoxDecoration).color;
      return color == Palette.of(tester.element(fill)).selected;
    }

    expect(on(), isTrue);
    await tester.tap(find.bySemanticsLabel(RegExp(r'^Context$')).first);
    await settle(tester);
    expect(find.byType(FocusContext), findsNothing);
    expect(on(), isFalse);
    await tester.tap(find.bySemanticsLabel(RegExp(r'^Context$')).first);
    await settle(tester);
    expect(find.byType(FocusContext), findsOneWidget);
    expect(on(), isTrue);
  });

  testWidgets('13" portrait: Context opens over the stage, and a tap '
      'beside it closes it', (tester) async {
    final harness = await _show(tester, const Size(1032, 1376));
    await _enter(tester, harness);
    expect(
      tester.getSize(find.byType(FocusQueue)).width,
      FocusGeometry.queueNarrow,
    );
    final session = tester.widget<IPadFocus>(find.byType(IPadFocus)).session;
    session.contextOpen = false;
    await settle(tester);
    expect(find.byType(FocusContext), findsNothing);
    await tester.tap(find.bySemanticsLabel(RegExp(r'^Context$')).first);
    await settle(tester);
    expect(find.byType(FocusContext), findsOneWidget);
    // Over the stage, at its right; the card still shows at its left.
    final panel = tester.getRect(find.byType(FocusContext));
    expect(panel.right, lessThanOrEqualTo(1032));
    expect(panel.width, lessThanOrEqualTo(FocusGeometry.contextPanel));
    await tester.tapAt(const Offset(400, 700));
    await settle(tester);
    expect(find.byType(FocusContext), findsNothing);
    expect(_onStage('Question 7'), findsOneWidget);

    // Open, it leaves the stage's header in reach: its Context button
    // hides it again (Zucchero's row 666), and so does its own close.
    await tester.tap(find.bySemanticsLabel(RegExp(r'^Context$')).first);
    await settle(tester);
    final header = find.descendant(
      of: find.byType(FocusStage),
      matching: find.text('Open conversation'),
    );
    expect(tester.getRect(header).bottom, lessThan(panel.top));
    expect(
      tester
          .hitTestOnBinding(tester.getCenter(header))
          .path
          .any((entry) => entry.target is RenderParagraph),
      isTrue,
    );
    await tester.tap(find.bySemanticsLabel(RegExp(r'^Context$')).first);
    await settle(tester);
    expect(find.byType(FocusContext), findsNothing);
    await tester.tap(find.bySemanticsLabel(RegExp(r'^Context$')).first);
    await settle(tester);
    await tester.tap(find.bySemanticsLabel('Hide Context'));
    await settle(tester);
    expect(find.byType(FocusContext), findsNothing);
  });

  testWidgets('11" portrait: the stage alone; "1 of 3" shows the queue, a '
      "pick closes it, and ˄ ˅ step through", (tester) async {
    final harness = await _show(tester, const Size(834, 1194));
    await _enter(tester, harness);
    expect(find.byType(FocusQueue), findsNothing);
    expect(find.text('1 of 3'), findsOneWidget);
    await tester.tap(find.text('1 of 3'));
    await settle(tester);
    expect(find.byType(FocusQueue), findsOneWidget);
    // June's first card, from the queue.
    await tester.tap(
      find.descendant(
        of: find.byType(FocusQueue),
        matching: find.textContaining('Question 1'),
      ),
    );
    await settle(tester);
    expect(find.byType(FocusQueue), findsNothing);
    expect(_onStage('Question 1'), findsOneWidget);
    await tester.tap(find.bySemanticsLabel('Next request'));
    await settle(tester);
    expect(_onStage('Question 2'), findsOneWidget);
    await tester.tap(find.bySemanticsLabel('Previous request'));
    await settle(tester);
    expect(_onStage('Question 1'), findsOneWidget);
  });

  testWidgets('a resize keeps the card on the stage', (tester) async {
    final harness = await _show(tester, const Size(1376, 1032));
    await _enter(tester, harness);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyJ);
    await settle(tester);
    expect(_onStage('Question 1'), findsOneWidget);
    // Stage Manager narrows the window (Harness.show resets it after).
    tester.view.physicalSize = const Size(834, 1194);
    tester.view.display.size = const Size(834, 1194);
    await settle(tester);
    expect(find.byType(FocusQueue), findsNothing);
    expect(_onStage('Question 1'), findsOneWidget);
    expect(find.text('2 of 3'), findsOneWidget);
  });

  testWidgets('Slide Over: the phone\'s one-card screen on the same '
      'sitting; the card stays through the resize there and back, and Back '
      'leaves focus mode', (tester) async {
    final harness = await _show(tester, const Size(1376, 1032));
    await _enter(tester, harness);
    await tester.sendKeyEvent(LogicalKeyboardKey.keyJ);
    await settle(tester);
    final session = tester.widget<IPadFocus>(find.byType(IPadFocus)).session;
    final card = session.current?.key;
    expect(_onStage('Question 1'), findsOneWidget);
    // Narrowed into Slide Over's width: still focus mode, the same sitting
    // and card, on the phone's screen.
    tester.view.physicalSize = const Size(375, 1032);
    tester.view.display.size = const Size(375, 1032);
    await settle(tester);
    expect(find.byType(PhoneNeedsYou), findsOneWidget);
    expect(find.byType(FocusQueue), findsNothing);
    // Context was beside the card: it folds away rather than cover it;
    // a tap on who asks slides it over, and a tap beside it closes it.
    expect(find.byType(FocusContext), findsNothing);
    await tester.tap(
      find
          .descendant(
            of: find.byType(PhoneNeedsYou),
            matching: find.text('June'),
          )
          .first,
    );
    await settle(tester);
    expect(find.byType(FocusContext), findsOneWidget);
    await tester.tapAt(const Offset(20, 1000));
    await settle(tester);
    expect(find.byType(FocusContext), findsNothing);
    expect(tester.widget<IPadFocus>(find.byType(IPadFocus)).session, session);
    expect(session.current?.key, card);
    expect(
      find.descendant(
        of: find.byType(PhoneNeedsYou),
        matching: find.text('Question 1'),
      ),
      findsWidgets,
    );
    // And wide again: the same card, with the queue.
    tester.view.physicalSize = const Size(1376, 1032);
    tester.view.display.size = const Size(1376, 1032);
    await settle(tester);
    expect(find.byType(PhoneNeedsYou), findsNothing);
    expect(find.byType(FocusQueue), findsOneWidget);
    // Context, folded away by the narrowing, is beside the card again.
    expect(find.byType(FocusContext), findsOneWidget);
    expect(session.current?.key, card);
    expect(_onStage('Question 1'), findsOneWidget);
    // Narrow once more: Back leaves focus mode.
    tester.view.physicalSize = const Size(375, 1032);
    tester.view.display.size = const Size(375, 1032);
    await settle(tester);
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
    expect(focusModeOf(harness.backend).value, isFalse);
  });

  testWidgets("the menu: ⌘⇧N to enter while it's off; focus mode's "
      'commands while on, standing down in a field', (tester) async {
    final harness = await _show(tester, const Size(1376, 1032));
    expect(_menu['focus.toggle'], ('Focus on Needs You', true));
    expect(_menu.containsKey('sidebar'), isTrue);

    await _enter(tester, harness);
    expect(_menu.containsKey('sidebar'), isFalse);
    expect(_menu['focus.toggle'], ('Leave Focus Mode', true));
    expect(_menu['focus.next']?.$2, isTrue);
    expect(_menu['focus.context']?.$2, isTrue);

    // A field has the keys: ⌘⌫, ⌥⌘↓ and ⌘⇧C reach it instead. The card
    // docks its field in the stage's answer bar (Illuso's CardAnswer; a
    // stand-in here, as in needs_you_focus_test).
    final field = TextEditingController();
    addTearDown(field.dispose);
    tester
        .element(find.byType(NeedsYouCard))
        .getInheritedWidgetOfExactType<AnswerDock>()!
        .answer
        .value = CardAnswer(
      docked: true,
      field: field,
      yes: 'Approve',
      no: 'Dismiss',
      onYes: () async {},
      onSubmit: () async {},
      onNo: (reason) async {},
    );
    await settle(tester);
    expect(_menu['focus.yes'], ('Approve', true));
    expect(_menu['focus.no'], ('Dismiss', true));
    await tester.tap(
      find.descendant(
        of: find.byType(FocusStage),
        matching: find.byType(EditableText),
      ),
    );
    await settle(tester);
    expect(_menu['focus.no']?.$2, isFalse);
    expect(_menu['focus.next']?.$2, isFalse);
    expect(_menu['focus.context']?.$2, isFalse);

    // Leaving: the workbench's again.
    await tester.tap(find.text('Done'));
    await settle(tester);
    expect(find.byType(IPadFocus), findsNothing);
    expect(_menu['focus.toggle'], ('Focus on Needs You', true));
    expect(_menu.containsKey('sidebar'), isTrue);
  });
}
