// Needs you on an iPhone (#514 §7): focus mode itself, one card a screen,
// "3 of 7" opening the queue, ˄ ˅ between cards, who asks opening their
// conversation, and the answer pinned under the thumb.
import 'package:factory_app/card_answer.dart' show AnswerDock, CardAnswer;
import 'package:factory_app/card_files.dart' show CardAttachButton;
import 'package:factory_app/inbox.dart' show NeedsYouCard;
import 'package:factory_app/controls.dart' show Button;
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
    ..['june/1'] = _ask(1, 5, 'Ship the migration tonight?')
    ..['mista/7'] = _ask(7, 60, 'Turn on the relay rule?');
  await harness.show(tester, size: size);
  harness.page.value = AppPage.inbox;
  await pumpFrames(tester);
  await tester.pump();
  await pumpFrames(tester);
  return harness;
}

/// [text] in the answer at the foot, not in the card's own controls.
Finder _inDock(String text) => find.descendant(
  of: find.byKey(phoneThumbDockKey),
  matching: find.text(text),
);

void main() {
  testWidgets('one card a screen, the oldest first; ˄ ˅ go between them', (
    tester,
  ) async {
    await _open(tester);
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
    await _open(tester);
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
    await tester.tap(find.bySemanticsLabel(RegExp('^Mista, ')));
    await pumpFrames(tester);
    expect(find.text('1 of 2'), findsNothing);
    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester);
    expect(find.text('1 of 2'), findsOneWidget);
    expect(find.text('Turn on the relay rule?'), findsOneWidget);
  });

  testWidgets("a notification's card first, whatever its place; Back goes "
      'back to what showed (#514 §7.2)', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_workspace());
    harness.backend.entriesById
      ..['june/1'] = _ask(1, 5, 'Ship the migration tonight?')
      ..['mista/7'] = _ask(7, 60, 'Turn on the relay rule?');
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    // As a tapped notification does (notifications.dart).
    harness.backend.needsYouCard.value = 'agent:june/1';
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await tester.pump();
    await pumpFrames(tester);
    expect(find.text('Ship the migration tonight?'), findsOneWidget);
    // In its place among them, oldest first.
    expect(find.text('2 of 2'), findsOneWidget);
    expect(harness.backend.needsYouCard.value, isNull, reason: 'once');

    await tester.tap(find.bySemanticsLabel('Back'));
    await pumpFrames(tester);
    expect(harness.page.value, AppPage.agent);
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
    final harness = await _open(tester);
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

  testWidgets('nothing waiting: it says so, and stays until Back', (
    tester,
  ) async {
    await _open(
      tester,
      workspace: _workspace(june: [], mista: []),
    );
    expect(find.text('Nothing needs you.'), findsOneWidget);
    expect(find.bySemanticsLabel('Next'), findsNothing);
  });

  group('the answer under the thumb', () {
    /// Publishes a docked answer for the card showing, as a card under an
    /// AnswerDock does, recording what's pressed.
    Future<(TextEditingController, List<String>)> dock(
      WidgetTester tester,
    ) async {
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

    testWidgets('the no at the leading side, the yes wider at the trailing; '
        'both a finger tall, at the foot', (tester) async {
      await _open(tester);
      await dock(tester);
      final yes = tester.getRect(_inDock('Approve'));
      final no = tester.getRect(_inDock('Dismiss'));
      expect(no.center.dx, lessThan(yes.center.dx));
      expect(yes.center.dy, closeTo(no.center.dy, 1));
      expect(yes.bottom, greaterThan(852 - 120), reason: 'near the thumb');
      expect(find.text('Your answer to Mista'), findsOneWidget);
    });

    testWidgets('a card that takes files: the paperclip beside the field '
        '(Zucchero row 651)', (tester) async {
      await _open(tester);
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
      await _open(tester);
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
      await _open(tester);
      final (field, pressed) = await dock(tester);
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
      await _open(tester);
      final (_, pressed) = await dock(tester);
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
      expect(find.text('1/2'), findsOneWidget);
    });
  }
}
