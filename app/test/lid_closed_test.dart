// Keeping a laptop's agents working with its lid closed (the backend
// keeping the Mac awake itself, nothing installed): Settings → General →
// Sleep's second switch, only on a laptop that can; its own question before
// it's turned on, only on the Mac; where it stands under it; turning it off
// from anywhere; and STUCK, the Mac unable to sleep although Claktory isn't
// keeping it awake, shown everywhere with what fixes it.

import 'package:factory_app/controls.dart' show Toggle;
import 'package:factory_app/identity.dart' show Palette;
import 'package:factory_app/lid_closed.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';
import 'idle_compaction_test.dart' show reveal;
import 'selection_test.dart' show watchClipboard;

final _mac = TargetPlatformVariant.only(TargetPlatform.macOS);

const _fix =
    'In Terminal on Studio, run `sudo pmset -a disablesleep 0`, or restart '
    'it.';

KeepAwake _laptop({
  LidClosedState state = LidClosedState.LID_CLOSED_OFF,
  bool available = true,
  bool laptop = true,
  bool holding = false,
  String fix = '',
  bool? turnedOn,
}) => KeepAwake(
  available: true,
  laptop: laptop,
  answered: true,
  lidClosedAvailable: available,
  lidClosed: state,
  lidClosedHolding: holding,
  lidClosedFix: fix,
  lidClosedTurnedOn:
      turnedOn ??
      (state != LidClosedState.LID_CLOSED_OFF &&
          state != LidClosedState.LID_CLOSED_STUCK),
);

Workspace _workspace(KeepAwake keep) => workspace()..keepAwake = keep;

Future<Harness> _settings(
  WidgetTester tester,
  KeepAwake keep, {
  bool localBackend = true,
}) async {
  final harness = Harness(
    _workspace(keep)..settings.name = 'Studio',
    localBackend: localBackend,
  );
  await harness.show(tester, size: const Size(1200, 1600));
  harness.page.value = AppPage.settings;
  await settle(tester);
  await reveal(tester, find.text('Sleep'));
  await settle(tester);
  return harness;
}

Toggle _toggle(WidgetTester tester) => tester.widget<Toggle>(
  find.byWidgetPredicate((w) => w is Toggle && w.label == lidClosedLabel),
);

List<SetLidClosed> _told(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasSetLidClosed()) message.setLidClosed,
];

void main() {
  testWidgets('on the Mac: off, its note; turning it on asks first, then '
      'turns it on', (tester) async {
    final harness = await _settings(tester, _laptop());
    expect(find.text(lidClosedLabel), findsOneWidget);
    expect(find.text(lidClosedNote), findsOneWidget);
    expect(_toggle(tester).value, isFalse);

    await tester.tap(find.text(lidClosedLabel));
    await settle(tester);
    // Its own question: what it does, the heat and the bag.
    expect(find.text(lidClosedQuestionTitle), findsOneWidget);
    expect(
      find.text(
        "This Mac stays awake while agents work and it's plugged in, even "
        'with its lid closed.',
      ),
      findsOneWidget,
    );
    expect(find.text(lidClosedQuestionBag), findsOneWidget);
    // Nothing installed: no password.
    expect(find.textContaining('password'), findsNothing);

    // Not now: nothing.
    await tester.tap(find.text('Not now'));
    await settle(tester);
    expect(find.text(lidClosedQuestionTitle), findsNothing);
    expect(_told(harness), isEmpty);

    await tester.tap(find.text(lidClosedLabel));
    await settle(tester);
    await tester.tap(find.text('Turn on'));
    await settle(tester);
    expect(find.text(lidClosedQuestionTitle), findsNothing);
    expect(_told(harness).single.on, isTrue);

    await harness.apply(
      tester,
      _workspace(_laptop(state: LidClosedState.LID_CLOSED_ON)),
    );
    await settle(tester);
    expect(_toggle(tester).value, isTrue);
  }, variant: _mac);

  testWidgets('refused, the question says why and stays', (tester) async {
    final harness = await _settings(tester, _laptop());
    harness.backend.refuse = 'Turn it on from the Mac.';
    await tester.tap(find.text(lidClosedLabel));
    await settle(tester);
    await tester.tap(find.text('Turn on'));
    await settle(tester);
    expect(find.text(lidClosedQuestionTitle), findsOneWidget);
    expect(find.text('Turn it on from the Mac.'), findsOneWidget);
  }, variant: _mac);

  testWidgets("no switch on a desktop or a laptop that can't", (tester) async {
    await _settings(tester, _laptop(laptop: false));
    expect(find.text(lidClosedLabel), findsNothing);

    await _settings(tester, _laptop(available: false));
    expect(find.text('Sleep'), findsOneWidget);
    expect(find.text(lidClosedLabel), findsNothing);
  }, variant: _mac);

  testWidgets('where it stands, in words', (tester) async {
    final harness = await _settings(
      tester,
      _laptop(state: LidClosedState.LID_CLOSED_ON),
    );
    expect(
      find.text(
        'On: agents keep working while this Mac is plugged in, even with its '
        'lid closed.',
      ),
      findsOneWidget,
    );

    Future<void> show(KeepAwake keep) async {
      await harness.apply(tester, _workspace(keep));
      await settle(tester);
    }

    await show(_laptop(state: LidClosedState.LID_CLOSED_ON, holding: true));
    expect(
      find.text('Keeping this Mac awake now, even with its lid closed'),
      findsOneWidget,
    );
    await show(_laptop(state: LidClosedState.LID_CLOSED_PAUSED_HEAT));
    expect(
      find.text(
        'Paused: this Mac is too hot. It starts again 10 minutes after it '
        'cools down.',
      ),
      findsOneWidget,
    );
    // On battery it lets go: on, not holding.
    await show(_laptop(state: LidClosedState.LID_CLOSED_ON));
    expect(find.textContaining('Keeping'), findsNothing);
  }, variant: _mac);

  testWidgets('macOS refusing it says why, in the danger color', (
    tester,
  ) async {
    await _settings(
      tester,
      _laptop(state: LidClosedState.LID_CLOSED_FAILED)
        ..lidClosedError = 'pmset exited with status 1',
    );
    const words =
        "Couldn't keep this Mac awake with its lid closed: pmset exited with "
        'status 1';
    expect(find.text(words), findsOneWidget);
    final context = tester.element(find.text(words));
    expect(
      tester.widget<Text>(find.text(words)).style?.color,
      Palette.of(context).danger,
    );
    // Still the person's setting: on, so off is there.
    expect(_toggle(tester).value, isTrue);
  }, variant: _mac);

  testWidgets("the helper's states, never sent now, read as off", (
    tester,
  ) async {
    for (final state in [
      LidClosedState.LID_CLOSED_NEEDS_APPROVAL,
      LidClosedState.LID_CLOSED_PAUSED_BATTERY,
      LidClosedState.LID_CLOSED_PAUSED_LID_ON_BATTERY,
      LidClosedState.LID_CLOSED_MISSING,
      LidClosedState.LID_CLOSED_OUTDATED,
    ]) {
      await _settings(tester, _laptop(state: state, turnedOn: false));
      expect(_toggle(tester).value, isFalse, reason: '$state');
      expect(find.textContaining('Paused'), findsNothing, reason: '$state');
      expect(find.textContaining('helper'), findsNothing, reason: '$state');
    }
  }, variant: _mac);

  testWidgets('turning it off asks nothing and turns it off', (tester) async {
    final harness = await _settings(
      tester,
      _laptop(state: LidClosedState.LID_CLOSED_ON),
    );
    await tester.tap(find.text(lidClosedLabel));
    await settle(tester);
    expect(find.text(lidClosedQuestionTitle), findsNothing);
    expect([for (final each in _told(harness)) each.on], [false]);
  }, variant: _mac);

  testWidgets('from another device it shows where it stands; it turns on '
      'only from the Mac, and off from anywhere', (tester) async {
    final harness = await _settings(tester, _laptop(), localBackend: false);
    expect(find.text(lidClosedLabel), findsOneWidget);
    expect(find.text(lidClosedFromMac), findsOneWidget);
    expect(_toggle(tester).onChanged, isNull);
    await tester.tap(find.text(lidClosedLabel));
    await settle(tester);
    expect(find.text(lidClosedQuestionTitle), findsNothing);
    expect(_told(harness), isEmpty);

    await harness.apply(
      tester,
      _workspace(_laptop(state: LidClosedState.LID_CLOSED_ON)),
    );
    await settle(tester);
    await tester.tap(find.text(lidClosedLabel));
    await settle(tester);
    expect([for (final each in _told(harness)) each.on], [false]);
  }, variant: _mac);

  testWidgets("stuck: it says the Mac can't sleep and what fixes it, the "
      'command with Copy, never Off; even with the switch hidden, on any '
      'device', (tester) async {
    final copied = watchClipboard(tester);
    final harness = await _settings(
      tester,
      _laptop(state: LidClosedState.LID_CLOSED_STUCK, fix: _fix),
    );
    expect(
      find.text("This Mac can't sleep with its lid closed right now."),
      findsOneWidget,
    );
    // One sentence, nothing left alone after the command's box.
    expect(
      find.text(
        'In Terminal on Studio, run sudo pmset -a disablesleep 0, or restart '
        'it.',
      ),
      findsOneWidget,
    );
    expect(find.text(', or restart it.'), findsNothing);
    expect(find.text('sudo pmset -a disablesleep 0'), findsOneWidget);
    await tester.tap(find.text('Copy'));
    await tester.pump();
    expect(copied.last, 'sudo pmset -a disablesleep 0');
    // The switch is the person's setting (off here); stuck isn't on.
    expect(_toggle(tester).value, isFalse);
    expect(find.textContaining('On:'), findsNothing);

    // The switch hidden (a build that can't), still said.
    await harness.apply(
      tester,
      _workspace(
        _laptop(
          state: LidClosedState.LID_CLOSED_STUCK,
          available: false,
          fix: 'Restart Studio to let it sleep with the lid closed.',
        ),
      ),
    );
    await settle(tester);
    expect(find.text(lidClosedLabel), findsNothing);
    expect(
      find.text("This Mac can't sleep with its lid closed right now."),
      findsOneWidget,
    );
    expect(
      find.text('Restart Studio to let it sleep with the lid closed.'),
      findsOneWidget,
    );
    expect(find.text('Copy'), findsNothing);
  }, variant: _mac);

  testWidgets('stuck with the setting on: the switch shows it on, and turns '
      "it off, from the Mac or a phone, even where it can't be turned on "
      "(Abbacchio's row 872)", (tester) async {
    for (final local in [true, false]) {
      final harness = await _settings(
        tester,
        _laptop(
          state: LidClosedState.LID_CLOSED_STUCK,
          fix: _fix,
          turnedOn: true,
        ),
        localBackend: local,
      );
      expect(_toggle(tester).value, isTrue);
      expect(_toggle(tester).onChanged, isNotNull);
      expect(find.text(lidClosedNote), findsOneWidget);
      expect(find.textContaining("can't sleep with its lid closed"), findsOne);
      await tester.tap(find.text(lidClosedLabel));
      await settle(tester);
      // Straight off, no question.
      expect(find.text(lidClosedQuestionTitle), findsNothing);
      expect([for (final each in _told(harness)) each.on], [false]);

      // Off, still stuck until the Mac can sleep: the switch off, the fix
      // still said.
      await harness.apply(
        tester,
        _workspace(_laptop(state: LidClosedState.LID_CLOSED_STUCK, fix: _fix)),
      );
      await settle(tester);
      expect(_toggle(tester).value, isFalse);
      expect(find.textContaining("can't sleep with its lid closed"), findsOne);

      // Where it can't be turned on any more, the switch stays while it's
      // on, so off is still there.
      await harness.apply(
        tester,
        _workspace(
          _laptop(
            state: LidClosedState.LID_CLOSED_STUCK,
            available: false,
            fix: _fix,
            turnedOn: true,
          ),
        ),
      );
      await settle(tester);
      expect(_toggle(tester).value, isTrue);
      expect(_toggle(tester).onChanged, isNotNull);
      await tester.pumpWidget(const SizedBox());
    }
  }, variant: _mac);

  testWidgets("stuck, seen from a phone: the Mac's name", (tester) async {
    await _settings(
      tester,
      _laptop(state: LidClosedState.LID_CLOSED_STUCK, fix: _fix),
      localBackend: false,
    );
    expect(
      find.text("Studio can't sleep with its lid closed right now."),
      findsOneWidget,
    );
  }, variant: _mac);

  testWidgets("stuck is in Needs you, with what fixes it; opened, it says "
      "once the Mac can sleep (Abbacchio's row 1159); unopened, it goes", (
    tester,
  ) async {
    final harness = Harness(
      _workspace(_laptop(state: LidClosedState.LID_CLOSED_STUCK, fix: _fix)),
    );
    await harness.show(tester);
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    await settle(tester);
    const title = "This Mac can't sleep with its lid closed right now.";
    expect(find.text(title), findsWidgets);
    expect(
      find.text(
        'In Terminal on Studio, run sudo pmset -a disablesleep 0, or restart '
        'it.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text(title).first);
    await settle(tester);
    await tester.pump(const Duration(seconds: 1));
    expect(find.text('sudo pmset -a disablesleep 0'), findsOneWidget);
    expect(find.text('Copy'), findsOneWidget);

    await harness.apply(tester, _workspace(_laptop()));
    await settle(tester);
    expect(find.text(title), findsWidgets);
    expect(find.text('It can sleep again.'), findsWidgets);
    expect(find.text('Copy'), findsNothing);
  }, variant: _mac);

  testWidgets('stuck in Needs you, never opened, goes once the Mac can sleep', (
    tester,
  ) async {
    final harness = Harness(
      _workspace(_laptop(state: LidClosedState.LID_CLOSED_STUCK, fix: _fix)),
    );
    await harness.show(tester);
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    const title = "This Mac can't sleep with its lid closed right now.";
    expect(find.text(title), findsWidgets);
    await harness.apply(tester, _workspace(_laptop()));
    await settle(tester);
    expect(find.text(title), findsNothing);
  }, variant: _mac);

  test('the fix splits into words and the commands in backticks', () {
    expect(lidClosedFixParts(_fix), [
      (text: 'In Terminal on Studio, run ', command: false),
      (text: 'sudo pmset -a disablesleep 0', command: true),
      (text: ', or restart it.', command: false),
    ]);
    expect(lidClosedFixParts('Restart this Mac.'), [
      (text: 'Restart this Mac.', command: false),
    ]);
    // An unclosed backtick is words.
    expect(lidClosedFixParts('Run `this'), [
      (text: 'Run ', command: false),
      (text: 'this', command: false),
    ]);
  });
}
