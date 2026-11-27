// Needs you on an iPhone: the list of what waits (#599, needs-you-cards.md
// §4.5), a request tapped there alone on its own screen, who asks opening
// their conversation, back to the list once answered, and no focus mode
// (#606). The same one-card screen is a narrow iPad's focus mode (#514
// §7): one card a screen, "3 of 7" opening the queue, ˄ ˅ between cards,
// and the answer pinned under the thumb.
import 'package:factory_app/card_answer.dart' show AnswerDock, CardAnswer;
import 'package:factory_app/card_files.dart' show CardAttachButton;
import 'package:factory_app/inbox.dart' show NeedsYouCard, NeedsYouList;
import 'package:factory_app/controls.dart' show Button, FocusGlyph;
import 'package:factory_app/mobile/needs_you_phone.dart';
import 'package:factory_app/needs_you_focus.dart'
    show FocusReviewBody, focusDismissReasons;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';

final _base = DateTime.now();

Entry _ask(int seq, int minutesAgo, String title) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(
    _base.subtract(Duration(minutes: minutesAgo)).millisecondsSinceEpoch,
  ),
  askPerson: AskPerson(id: 'q$seq', title: title),
);

Workspace _workspace({
  List<int> june = const [1],
  List<int> mista = const [7],
}) => Workspace(
  agents: [
    Agent(
      id: 'june',
      name: 'June',
      role: 'Backend',
      modelAvailable: true,
      waitingSeqs: [for (final seq in june) Int64(seq)],
    ),
    Agent(
      id: 'mista',
      name: 'Mista',
      modelAvailable: true,
      waitingSeqs: [for (final seq in mista) Int64(seq)],
    ),
  ],
  selectedAgentId: 'june',
);

Future<Harness> _open(
  WidgetTester tester, {
  Workspace? workspace,
  Size size = const Size(393, 852),
}) async {
  // An iPhone's: Android keeps Needs you's list for now.
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final harness = Harness(workspace ?? _workspace());
  harness.backend.entriesById
    ..['june/1'] = _ask(1, 60, 'Ship the migration tonight?')
    ..['mista/7'] = _ask(7, 5, 'Turn on the relay rule?');
  await harness.show(tester, size: size);
  harness.page.value = AppPage.inbox;
  await pumpFrames(tester);
  await tester.pump();
  await pumpFrames(tester);
  return harness;
}

/// Focus mode's sitting: the phone's one-card screen, as a narrow iPad's
/// focus mode shows it (ipad/focus.dart; Slide Over, ⅓ Split View). An
/// iPhone has none (#606). [onClose]: what its Back and Done do.
Future<Harness> _focus(
  WidgetTester tester, {
  Workspace? workspace,
  Size size = const Size(393, 852),
  VoidCallback? onClose,
}) async {
  asIPad();
  final harness = Harness(workspace ?? _workspace());
  harness.backend.entriesById
    ..['june/1'] = _ask(1, 60, 'Ship the migration tonight?')
    ..['mista/7'] = _ask(7, 5, 'Turn on the relay rule?');
  await harness.show(
    tester,
    size: size,
    home: PhoneNeedsYou(onClose: onClose),
  );
  await pumpFrames(tester);
  await tester.pump();
  await pumpFrames(tester);
  return harness;
}

/// The list's row for the request titled [title].
Finder _row(String title) =>
    find.descendant(of: find.byType(NeedsYouList), matching: find.text(title));

/// Publishes a docked answer for the card showing, as a card under an
/// AnswerDock does, recording what's pressed ([keep]: one with something
/// to show once answered, a pipe invite's code).
Future<(TextEditingController, List<String>)> _dock(
  WidgetTester tester, {
  bool keep = false,
}) async {
  final field = TextEditingController();
  addTearDown(field.dispose);
  final pressed = <String>[];
  final card = tester.element(find.byType(NeedsYouCard));
  // The answer a card would publish, typed once its field has words.
  CardAnswer ours() => CardAnswer(
    docked: true,
    field: field,
    typed: field.text.trim().isNotEmpty,
    yes: 'Approve',
    no: 'Dismiss',
    keep: keep,
    onYes: () async => pressed.add('yes'),
    onSubmit: () async => pressed.add('submit ${field.text}'),
    onNo: (reason) async => pressed.add('no ${reason ?? '-'}'),
  );
  final answer = AnswerDock.of(card)!.answer;
  var current = ours();
  // It takes the real card's place again whenever that one republishes.
  void stand() {
    if (!identical(answer.value, current)) answer.value = current;
  }

  answer.addListener(stand);
  addTearDown(() => answer.removeListener(stand));
  field.addListener(() => answer.value = current = ours());
  answer.value = current;
  await pumpFrames(tester);
  // Past the 1 s a new card takes nothing.
  await tester.pump(const Duration(seconds: 2));
  return (field, pressed);
}

/// [text] in the answer at the foot, not in the card's own controls.
Finder _inDock(String text) => find.descendant(
  of: find.byKey(phoneThumbDockKey),
  matching: find.text(text),
);

void main() {
  group('the list first (#599)', () {
    // A narrow iPad keeps its button (ipad/focus_way_in_test.dart).
    testWidgets('Needs you is the list: every request, who asks; no focus '
        'mode on a phone (#606)', (tester) async {
      final harness = await _open(tester);
      expect(_row('Turn on the relay rule?'), findsOneWidget);
      expect(_row('Ship the migration tonight?'), findsOneWidget);
      expect(find.text('Waiting'), findsOneWidget);
      expect(find.text('1 of 2'), findsNothing);
      expect(find.byKey(phoneThumbDockKey), findsNothing);
      expect(find.bySemanticsLabel('Focus on Needs you'), findsNothing);
      expect(find.byType(FocusGlyph), findsNothing);
      expect(harness.page.value, AppPage.inbox);
    });

    testWidgets("a long press selects none of a row's words: the rows are "
        "places to go (Zucchero's row 715)", (tester) async {
      await _open(tester);
      expect(
        find.ancestor(
          of: _row('Turn on the relay rule?'),
          matching: find.byType(SelectableRegion),
        ),
        findsNothing,
      );
      final gesture = await tester.startGesture(
        tester.getCenter(find.textContaining('min ago').first),
      );
      await tester.pump(const Duration(seconds: 1));
      await gesture.up();
      await pumpFrames(tester);
      expect(find.text('Select All'), findsNothing);
      expect(find.text('Copy'), findsNothing);
    });

    testWidgets('a request tapped opens alone: titled with who asks, no '
        'place among the rest nor ˄ ˅; Back, the list', (tester) async {
      final harness = await _open(tester);
      await tester.tap(_row('Turn on the relay rule?'));
      await pumpFrames(tester);
      expect(find.byType(NeedsYouList), findsNothing);
      expect(find.text('Turn on the relay rule?'), findsOneWidget);
      expect(find.text('Mista'), findsWidgets);
      expect(find.text('1 of 2'), findsNothing);
      expect(find.bySemanticsLabel('Next'), findsNothing);
      expect(find.bySemanticsLabel('Previous'), findsNothing);
      expect(find.byKey(phoneThumbDockKey), findsOneWidget);

      await tester.tap(find.bySemanticsLabel('Back').last);
      await pumpFrames(tester);
      expect(harness.page.value, AppPage.inbox);
      expect(_row('Turn on the relay rule?'), findsOneWidget);
    });

    testWidgets('a request changed by its agent since the list fetched it '
        "opens as it is now (Squalo's B4)", (tester) async {
      final harness = await _open(tester);
      // Mista changes the card; the list still has the copy it fetched.
      harness.backend.entriesById['mista/7'] = _ask(
        7,
        60,
        'Turn on the relay rule for every pipe?',
      )..askPerson.updatedMs = Int64(_base.millisecondsSinceEpoch);
      await tester.tap(_row('Turn on the relay rule?'));
      await pumpFrames(tester);
      await tester.pump();
      await pumpFrames(tester);
      expect(
        find.text('Turn on the relay rule for every pipe?'),
        findsOneWidget,
      );
      expect(find.text('Turn on the relay rule?'), findsNothing);
    });

    testWidgets('answered there, the list comes back after a moment, the '
        'row saying how it ended', (tester) async {
      final harness = await _open(tester);
      await tester.tap(_row('Turn on the relay rule?'));
      await pumpFrames(tester);
      final (_, pressed) = await _dock(tester);
      await tester.tap(_inDock('Approve'));
      await pumpFrames(tester);
      expect(pressed, ['yes']);
      // Claktory accepts it: it stops waiting.
      harness.backend.entriesById['mista/7'] = _ask(
        7,
        60,
        'Turn on the relay rule?',
      )..askPerson.state = AskPersonState.ASK_PERSON_STATE_ANSWERED;
      await harness.apply(tester, _workspace(mista: const []));
      await tester.pump();
      // The receipt, a moment, over this request: never the next one, nor
      // buttons a second tap could answer it with (Squalo's B1).
      expect(find.text('Turn on the relay rule?'), findsOneWidget);
      expect(find.text('Ship the migration tonight?'), findsNothing);
      expect(find.byKey(phoneThumbDockKey), findsNothing);
      // Opened alone: nothing to go Next to, only Done back to the list
      // (Zucchero 49234).
      expect(find.widgetWithText(Button, 'Next'), findsNothing);
      await tester.pump(phoneAnsweredFor ~/ 2);
      expect(find.text('Ship the migration tonight?'), findsNothing);
      expect(find.byKey(phoneThumbDockKey), findsNothing);
      await tester.pump(phoneAnsweredFor);
      await pumpFrames(tester);
      expect(find.byKey(phoneThumbDockKey), findsNothing);
      expect(_row('Turn on the relay rule?'), findsOneWidget);
      expect(
        find.descendant(
          of: find.byType(NeedsYouList),
          // In the card's own words: an approval's yes (#533).
          matching: find.text('Approved'),
        ),
        findsOneWidget,
        reason: 'its own outcome, never only "Answered"',
      );
      expect(_row('Ship the migration tonight?'), findsOneWidget);
    });

    testWidgets('one with something to show once answered stays until Back '
        '(Squalo L3)', (tester) async {
      final harness = await _open(tester);
      await tester.tap(_row('Turn on the relay rule?'));
      await pumpFrames(tester);
      await _dock(tester, keep: true);
      await tester.tap(_inDock('Approve'));
      await pumpFrames(tester);
      await harness.apply(tester, _workspace(mista: const []));
      await tester.pump(const Duration(seconds: 2));
      await pumpFrames(tester);
      expect(find.byType(NeedsYouList), findsNothing);
      expect(find.text('Turn on the relay rule?'), findsOneWidget);
    });

    testWidgets('a press it still waits after (a step done, the next to '
        'come) stays on it (Squalo L4)', (tester) async {
      await _open(tester);
      await tester.tap(_row('Turn on the relay rule?'));
      await pumpFrames(tester);
      final (_, pressed) = await _dock(tester);
      await tester.tap(_inDock('Approve'));
      await tester.pump(const Duration(seconds: 2));
      await pumpFrames(tester);
      expect(pressed, ['yes']);
      expect(find.byType(NeedsYouList), findsNothing);
      expect(find.byKey(phoneThumbDockKey), findsOneWidget);
    });

    testWidgets('Back keeps what was typed for when it opens again '
        '(Squalo L6)', (tester) async {
      await _open(tester);
      await tester.tap(_row('Turn on the relay rule?'));
      await pumpFrames(tester);
      final field = find.descendant(
        of: find.byKey(phoneThumbDockKey),
        matching: find.byType(EditableText),
      );
      await tester.enterText(field, 'only on staging');
      await pumpFrames(tester);
      await tester.tap(find.bySemanticsLabel('Back').last);
      await pumpFrames(tester);
      await tester.tap(_row('Turn on the relay rule?'));
      await pumpFrames(tester);
      expect(
        find.descendant(
          of: find.byKey(phoneThumbDockKey),
          matching: find.text('only on staging'),
        ),
        findsOneWidget,
      );
    });
  });

  testWidgets('one card a screen, the newest first; ˄ ˅ go between them', (
    tester,
  ) async {
    await _focus(tester);
    expect(find.text('1 of 2'), findsOneWidget);
    expect(find.text('Turn on the relay rule?'), findsOneWidget);
    expect(find.text('Ship the migration tonight?'), findsNothing);
    // A finger's target each.
    for (final label in ['Previous', 'Next', 'Back']) {
      final button = find.bySemanticsLabel(label);
      expect(button, findsOneWidget, reason: label);
      expect(tester.getSize(button).height, greaterThanOrEqualTo(44));
    }

    await tester.tap(find.bySemanticsLabel('Next'));
    await pumpFrames(tester);
    expect(find.text('2 of 2'), findsOneWidget);
    expect(find.text('Ship the migration tonight?'), findsOneWidget);
    expect(find.text('Turn on the relay rule?'), findsNothing);
    // Whose it is, over the card.
    expect(find.textContaining('June'), findsWidgets);
    expect(find.textContaining('Backend'), findsWidgets);
  });

  testWidgets('the title opens the queue; a row there shows its card', (
    tester,
  ) async {
    await _focus(tester);
    await tester.tap(find.text('1 of 2'));
    await pumpFrames(tester);
    expect(find.text('Waiting'), findsOneWidget);
    expect(find.text('History'), findsOneWidget);
    await tester.tap(find.bySemanticsLabel(RegExp('^June, ')));
    await pumpFrames(tester);
    expect(find.text('Waiting'), findsNothing, reason: 'the sheet closed');
    expect(find.text('2 of 2'), findsOneWidget);
    expect(find.text('Ship the migration tonight?'), findsOneWidget);
  });

  testWidgets("who asks opens their conversation; Back, the card again", (
    tester,
  ) async {
    await _open(tester);
    await tester.tap(_row('Turn on the relay rule?'));
    await pumpFrames(tester);
    await tester.tap(
      find.byWidgetPredicate(
        (widget) =>
            widget is Semantics &&
            widget.properties.hint == "Opens Mista's conversation",
      ),
    );
    await pumpFrames(tester);
    expect(find.byKey(phoneThumbDockKey), findsNothing);
    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester);
    expect(find.byKey(phoneThumbDockKey), findsOneWidget);
    expect(find.text('Turn on the relay rule?'), findsOneWidget);
  });

  testWidgets("a notification's request alone, over the list; Back, the "
      'list (§4.5)', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_workspace());
    harness.backend.entriesById
      ..['june/1'] = _ask(1, 60, 'Ship the migration tonight?')
      ..['mista/7'] = _ask(7, 5, 'Turn on the relay rule?');
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    // As a tapped notification does (notifications.dart).
    harness.backend.needsYouCard.value = 'agent:june/1';
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await tester.pump();
    await pumpFrames(tester);
    expect(find.text('Ship the migration tonight?'), findsOneWidget);
    expect(find.byKey(phoneThumbDockKey), findsOneWidget);
    expect(find.text('1 of 2'), findsNothing);
    expect(find.text('June'), findsWidgets, reason: 'titled with who asks');
    expect(harness.backend.needsYouCard.value, isNull, reason: 'once');

    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester);
    expect(harness.page.value, AppPage.inbox);
    expect(find.byKey(phoneThumbDockKey), findsNothing);
    expect(_row('Ship the migration tonight?'), findsOneWidget);
  });

  testWidgets('a notification for a request answered elsewhere meanwhile: '
      'the list (Squalo L5)', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_workspace());
    harness.backend.entriesById
      ..['june/1'] = _ask(1, 60, 'Ship the migration tonight?')
      ..['mista/7'] = _ask(7, 5, 'Turn on the relay rule?');
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    harness.backend.needsYouCard.value = 'agent:june/9';
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await tester.pump();
    await pumpFrames(tester);
    expect(find.byKey(phoneThumbDockKey), findsNothing);
    expect(_row('Ship the migration tonight?'), findsOneWidget);
    expect(harness.backend.needsYouCard.value, isNull);
  });

  testWidgets('Back leaves Needs you', (tester) async {
    final harness = await _open(tester);
    await tester.tap(find.bySemanticsLabel('Back'));
    await pumpFrames(tester);
    expect(harness.page.value, AppPage.agent);
  });

  testWidgets('ended while it shows: it says so, and Next moves on', (
    tester,
  ) async {
    final harness = await _focus(tester);
    expect(find.text('Turn on the relay rule?'), findsOneWidget);
    await harness.apply(tester, _workspace(mista: const []));
    await pumpFrames(tester);
    // It never moves on by itself (§8).
    expect(find.text('Turn on the relay rule?'), findsOneWidget);
    final next = find.widgetWithText(Button, 'Next');
    expect(next, findsOneWidget);
    await tester.tap(next);
    await pumpFrames(tester);
    expect(find.text('Ship the migration tonight?'), findsOneWidget);
  });

  group('on an iPad, as its focus mode', () {
    Future<Harness> mount(
      WidgetTester tester,
      PhoneNeedsYou screen, {
      Entry? card,
    }) async {
      final harness = Harness(_workspace(mista: const []));
      harness.backend.entriesById['june/1'] =
          card ?? _ask(1, 5, 'Ship the migration tonight?');
      await harness.show(tester, size: const Size(393, 852), home: screen);
      await pumpFrames(tester);
      await tester.pump();
      await pumpFrames(tester);
      return harness;
    }

    testWidgets('Back is what the iPad gives', (tester) async {
      var left = 0;
      final harness = await mount(tester, PhoneNeedsYou(onClose: () => left++));
      harness.page.value = AppPage.inbox;
      await tester.tap(find.bySemanticsLabel('Back'));
      await pumpFrames(tester);
      expect(left, 1);
      expect(harness.page.value, AppPage.inbox, reason: 'the iPad leaves');
    });

    testWidgets("who asks opens the iPad's Context", (tester) async {
      var opened = 0;
      await mount(tester, PhoneNeedsYou(onContext: () => opened++));
      await tester.tap(find.bySemanticsLabel(RegExp('^June, ')));
      await pumpFrames(tester);
      expect(opened, 1);
    });

    testWidgets("a review the iPad answers takes all under who asks", (
      tester,
    ) async {
      var docked = false;
      final card = _ask(1, 5, 'Read the plan?')
        ..askPerson.reviewPath = 'docs/plan.md';
      await mount(
        tester,
        PhoneNeedsYou(
          reviewBody: (context, item, entry) => FocusReviewBody(
            Builder(
              builder: (context) {
                final dock = AnswerDock.of(context);
                docked = dock != null;
                // Docked on purpose: the no asks why (Sorbet 38658).
                WidgetsBinding.instance.addPostFrameCallback((_) {
                  dock?.answer.value = CardAnswer(
                    key: item.key,
                    docked: true,
                    field: TextEditingController(),
                    yes: 'Approve',
                    no: 'Dismiss',
                  );
                });
                return const Text('the iPad\'s document');
              },
            ),
            answers: true,
          ),
        ),
        card: card,
      );
      expect(find.text("the iPad's document"), findsOneWidget);
      expect(docked, isTrue, reason: 'it answers through the dock');
      expect(find.text('Read the plan?'), findsNothing, reason: 'no card');
      expect(find.textContaining('June'), findsWidgets);
      // Its own bar answers, docked or not: no thumb dock under it.
      expect(find.byKey(phoneThumbDockKey), findsNothing);
    });

    testWidgets('one that only shows the file sits above the card', (
      tester,
    ) async {
      final card = _ask(1, 5, 'Read the plan?')
        ..askPerson.reviewPath = 'docs/plan.md';
      await mount(
        tester,
        PhoneNeedsYou(
          reviewBody: (context, item, entry) =>
              const FocusReviewBody(Text('the file')),
        ),
        card: card,
      );
      expect(find.text('the file'), findsOneWidget);
      expect(find.text('Read the plan?'), findsOneWidget);
    });
  });

  testWidgets('nothing waiting: the list says so, with no Focus mode', (
    tester,
  ) async {
    await _open(
      tester,
      workspace: _workspace(june: [], mista: []),
    );
    expect(find.text('Nothing is waiting for your answer.'), findsOneWidget);
    expect(find.bySemanticsLabel('Focus on Needs you'), findsNothing);
  });

  testWidgets('focus mode with nothing left: "Nothing needs you." and Done, '
      'which goes back (F3)', (tester) async {
    var left = 0;
    await Harness(_workspace(june: [], mista: [])).show(
      tester,
      size: const Size(393, 852),
      home: PhoneNeedsYou(onClose: () => left++),
    );
    await pumpFrames(tester);
    expect(find.text('Nothing needs you.'), findsOneWidget);
    final done = find.widgetWithText(Button, 'Done');
    expect(tester.getSize(done).height, greaterThanOrEqualTo(44));
    expect(tester.widget<Button>(done).primary, isTrue);
    await tester.tap(done);
    await pumpFrames(tester);
    expect(left, 1);
  });

  testWidgets('the last card answered on another device while it shows: '
      'it stays, saying so, with Done back to the list (F3)', (tester) async {
    var left = 0;
    final harness = await _focus(
      tester,
      workspace: _workspace(mista: []),
      onClose: () => left++,
    );
    expect(find.text('Ship the migration tonight?'), findsOneWidget);
    await harness.apply(tester, _workspace(june: [], mista: []));
    await pumpFrames(tester);
    // Never yanked away.
    expect(find.text('Ship the migration tonight?'), findsOneWidget);
    expect(find.widgetWithText(Button, 'Next'), findsNothing);
    final done = find.widgetWithText(Button, 'Done');
    expect(tester.widget<Button>(done).primary, isTrue);
    await tester.tap(done);
    await pumpFrames(tester);
    expect(left, 1, reason: 'back to the list');
  });

  for (final (scale, glyph) in const [(1.0, 16.0), (3.1, 32.0)]) {
    testWidgets('Focus mode\'s glyph grows with its label at ${scale}x, to '
        'where the label stops', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = scale;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      // A narrow iPad's: a phone has none.
      asIPad();
      await _open(tester);
      final box = find.ancestor(
        of: find.byType(FocusGlyph),
        matching: find.byType(FittedBox),
      );
      expect(tester.getSize(box), Size.square(glyph));
    });
  }

  group('the answer under the thumb', () {
    testWidgets('the no at the leading side, the yes wider at the trailing; '
        'both a finger tall, at the foot', (tester) async {
      await _focus(tester);
      await _dock(tester);
      final yes = tester.getRect(_inDock('Approve'));
      final no = tester.getRect(_inDock('Dismiss'));
      expect(no.center.dx, lessThan(yes.center.dx));
      expect(yes.center.dy, closeTo(no.center.dy, 1));
      // Their buttons 2:3 (Lucy's spec; Zucchero 49328).
      final yesButton = tester.getRect(
        find.ancestor(of: _inDock('Approve'), matching: find.byType(Button)),
      );
      final noButton = tester.getRect(
        find.ancestor(of: _inDock('Dismiss'), matching: find.byType(Button)),
      );
      expect(noButton.width / yesButton.width, closeTo(2 / 3, 0.02));
      expect(yes.bottom, greaterThan(852 - 120), reason: 'near the thumb');
      expect(find.text('Your answer to Mista'), findsOneWidget);
    });

    testWidgets('at the largest text the field says only "Answer", read in '
        'full (Zucchero 48776)', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 3.1;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await _focus(tester);
      await _dock(tester);
      expect(find.text('Answer'), findsOneWidget);
      expect(find.text('Your answer to Mista'), findsNothing);
      expect(find.bySemanticsLabel('Your answer to Mista'), findsWidgets);
    });

    testWidgets('a card that takes files: the paperclip beside the field '
        '(Zucchero row 651)', (tester) async {
      await _focus(tester);
      final card = tester.element(find.byType(NeedsYouCard));
      final answer = AnswerDock.of(card)!.answer;
      final ours = CardAnswer(
        docked: true,
        field: TextEditingController(),
        yes: 'Approve',
        no: 'Dismiss',
        filesKey: 'files/mista/7',
        onYes: () async {},
        onNo: (_) async {},
      );
      void stand() {
        if (!identical(answer.value, ours)) answer.value = ours;
      }

      answer.addListener(stand);
      addTearDown(() => answer.removeListener(stand));
      answer.value = ours;
      await pumpFrames(tester);
      expect(
        find.descendant(
          of: find.byKey(phoneThumbDockKey),
          matching: find.byType(CardAttachButton),
        ),
        findsOneWidget,
      );
    });

    testWidgets('a card with no "no" has only its yes', (tester) async {
      await _focus(tester);
      final card = tester.element(find.byType(NeedsYouCard));
      AnswerDock.of(card)!.answer.value = CardAnswer(
        docked: true,
        field: TextEditingController(),
        yes: 'Continue',
        no: '',
        onYes: () async {},
      );
      await pumpFrames(tester);
      expect(_inDock('Continue'), findsOneWidget);
      expect(
        find.descendant(
          of: find.byKey(phoneThumbDockKey),
          matching: find.byType(Button),
        ),
        findsOneWidget,
      );
    });

    testWidgets('Approve answers; words typed make it Submit', (tester) async {
      await _focus(tester);
      final (field, pressed) = await _dock(tester);
      await tester.tap(_inDock('Approve'));
      await pumpFrames(tester);
      expect(pressed, ['yes']);

      field.text = 'only the staging one';
      await pumpFrames(tester);
      expect(_inDock('Submit'), findsOneWidget);
      expect(_inDock('Dismiss'), findsNothing);
      await tester.tap(_inDock('Submit'));
      await pumpFrames(tester);
      expect(pressed.last, 'submit only the staging one');
    });

    testWidgets('Dismiss with nothing typed asks why, one tap each', (
      tester,
    ) async {
      await _focus(tester);
      final (_, pressed) = await _dock(tester);
      await tester.tap(_inDock('Dismiss'));
      await pumpFrames(tester);
      for (final reason in focusDismissReasons) {
        expect(find.text(reason), findsOneWidget, reason: reason);
      }
      await tester.tap(find.text('Unclear'));
      await pumpFrames(tester);
      expect(pressed, ['no Unclear']);

      await tester.tap(_inDock('Dismiss'));
      await pumpFrames(tester);
      await tester.tap(find.text('Dismiss without a reason'));
      await pumpFrames(tester);
      expect(pressed.last, 'no -');
    });
  });

  for (final size in const [Size(320, 568), Size(440, 956)]) {
    testWidgets('at the largest text on a ${size.width.toInt()}pt phone, '
        'nothing is cut off', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 3.1;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await _open(tester, size: size);
      expect(tester.takeException(), isNull);
      expect(_row('Turn on the relay rule?'), findsOneWidget);
      // A request alone…
      await tester.ensureVisible(_row('Turn on the relay rule?'));
      await pumpFrames(tester);
      await tester.tap(_row('Turn on the relay rule?'));
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      expect(find.byKey(phoneThumbDockKey), findsOneWidget);
    });

    testWidgets("at the largest text, a narrow iPad's focus mode on a "
        '${size.width.toInt()}pt window: nothing is cut off', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 3.1;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await _focus(tester, size: size);
      expect(tester.takeException(), isNull);
      expect(find.text('1/2'), findsOneWidget);
    });
  }
}
