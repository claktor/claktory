// Settings → General's Sleep row (#352 P1): keeping the Mac Claktory runs
// on awake while agents work (Workspace.keep_awake), shown only where the
// Claktory can; the switch is the person's answer (SetKeepAwake). And
// Needs you's question while keep_awake.ask: Keep it awake or Let it sleep.

import 'package:factory_app/controls.dart' show Button, Toggle;
import 'package:factory_app/inbox.dart';
import 'package:factory_app/keep_awake_card.dart' show keepAwakeAnswered;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';
import 'idle_compaction_test.dart' show reveal;

const _label = 'Keep this Mac awake while agents work';
const _note = "Only while it's plugged in with its lid open.";
const _holding = 'Keeping this Mac awake now';

Workspace _workspace(KeepAwake keep) => workspace()..keepAwake = keep;

Future<Harness> _settings(
  WidgetTester tester,
  Workspace snapshot, {
  Size size = const Size(1200, 1600),
}) async {
  final harness = Harness(snapshot);
  await harness.show(tester, size: size);
  harness.page.value = AppPage.settings;
  await settle(tester);
  await reveal(tester, find.text('Check for updates'));
  return harness;
}

Toggle _toggle(WidgetTester tester) => tester.widget<Toggle>(
  find.byWidgetPredicate((w) => w is Toggle && w.label == _label),
);

void main() {
  testWidgets('not on a Mac that can, no row', (tester) async {
    await _settings(tester, _workspace(KeepAwake()));
    expect(find.text('Check for updates'), findsOneWidget);
    expect(find.text(_label), findsNothing);
    expect(find.text('Sleep'), findsNothing);
  });

  testWidgets('on a laptop: off, the battery note; the switch answers yes, '
      'and while held it says so', (tester) async {
    final harness = await _settings(
      tester,
      _workspace(KeepAwake(available: true, laptop: true, ask: true)),
    );
    await reveal(tester, find.text(_label));
    expect(find.text('Sleep'), findsOneWidget);
    expect(find.text(_note), findsOneWidget);
    expect(_toggle(tester).value, isFalse);
    expect(find.text(_holding), findsNothing);

    // A click anywhere on the row turns it on.
    await tester.tap(find.text(_label));
    await settle(tester);
    expect(harness.last.setKeepAwake.on, isTrue);

    await harness.apply(
      tester,
      _workspace(
        KeepAwake(available: true, laptop: true, on: true, answered: true),
      ),
    );
    expect(_toggle(tester).value, isTrue);
    expect(find.text(_holding), findsNothing);

    // An agent works: it's held.
    await harness.apply(
      tester,
      _workspace(
        KeepAwake(
          available: true,
          laptop: true,
          on: true,
          answered: true,
          holding: true,
        ),
      ),
    );
    expect(find.text(_holding), findsOneWidget);

    // Off again.
    await tester.tap(
      find.byWidgetPredicate((w) => w is Toggle && w.label == _label),
    );
    await settle(tester);
    expect(harness.last.setKeepAwake.on, isFalse);
  });

  testWidgets('a desktop Mac is on by default, with no note and no '
      'question', (tester) async {
    final snapshot = _workspace(
      KeepAwake(available: true, on: true, pluggedIn: true),
    );
    await _settings(tester, snapshot);
    await reveal(tester, find.text(_label));
    expect(find.text(_label), findsOneWidget);
    expect(_toggle(tester).value, isTrue);
    expect(find.text(_note), findsNothing);
    expect(
      needsYouItems(snapshot).where((i) => i.kind == NeedsYouKind.keepAwake),
      isEmpty,
    );
  });

  testWidgets('a laptop on battery while agents work says it can sleep', (
    tester,
  ) async {
    const battery =
        'On battery, so this Mac can sleep. Plug it in to keep agents working.';
    Workspace snapshot({required bool working, required bool plugged}) =>
        workspace(agents: [builder(working: working)])
          ..keepAwake = KeepAwake(
            available: true,
            laptop: true,
            on: true,
            answered: true,
            pluggedIn: plugged,
            holding: working && plugged,
          );
    final harness = await _settings(
      tester,
      snapshot(working: true, plugged: false),
    );
    await reveal(tester, find.text(_label));
    expect(find.text(battery), findsOneWidget);
    expect(find.text(_holding), findsNothing);

    // Plugged in: held.
    await harness.apply(tester, snapshot(working: true, plugged: true));
    expect(find.text(battery), findsNothing);
    expect(find.text(_holding), findsOneWidget);

    // No agent works: neither, whatever plugged_in last said.
    await harness.apply(tester, snapshot(working: false, plugged: false));
    expect(find.text(battery), findsNothing);
    expect(find.text(_holding), findsNothing);
  });

  testWidgets('a refusal says why under it', (tester) async {
    final harness = await _settings(
      tester,
      _workspace(KeepAwake(available: true, laptop: true)),
    );
    harness.backend.refuse = 'The Mac said no.';
    await reveal(tester, find.text(_label));
    await tester.tap(find.text(_label));
    await settle(tester);
    expect(find.textContaining('The Mac said no.'), findsOneWidget);
  });

  testWidgets('on a phone it shows in General and answers', (tester) async {
    final harness = Harness(
      _workspace(KeepAwake(available: true, laptop: true, holding: false)),
    );
    await harness.show(tester, size: const Size(390, 844));
    harness.page.value = AppPage.settings;
    await pumpFrames(tester);
    await tester.tap(find.text('General'));
    await pumpFrames(tester);
    await tester.ensureVisible(find.text(_label));
    await pumpFrames(tester);
    expect(find.text(_note), findsOneWidget);
    await tester.tap(find.text(_label));
    await pumpFrames(tester);
    expect(harness.last.setKeepAwake.on, isTrue);
    expect(tester.takeException(), isNull);
  });

  test('asked only while ask is on', () {
    expect(
      needsYouItems(_workspace(KeepAwake(available: true, laptop: true)))
          .where((item) => item.kind == NeedsYouKind.keepAwake),
      isEmpty,
    );
    expect(
      needsYouItems(
        _workspace(KeepAwake(available: true, laptop: true, ask: true)),
      ).where((item) => item.kind == NeedsYouKind.keepAwake).single.key,
      'keepAwake',
    );
  });

  for (final (yes, label) in [
    (true, 'Keep it awake'),
    (false, 'Let it sleep'),
  ]) {
    testWidgets('Needs you asks; $label answers, and it says what the Mac '
        'does now', (tester) async {
      final harness = Harness(
        _workspace(KeepAwake(available: true, laptop: true, ask: true)),
      );
      await harness.show(tester, size: const Size(1200, 900));
      await settle(tester);
      harness.page.value = AppPage.inbox;
      await settle(tester);
      expect(
        find.text("Keep your agents working while you're away?"),
        findsOneWidget,
      );
      expect(
        find.text(
          'Agents keep working while this Mac is plugged in with its lid open.',
        ),
        findsOneWidget,
      );
      await tester.tap(
        find.text("Keep your agents working while you're away?"),
      );
      await settle(tester);
      expect(
        find.text('You can change this in Settings → General.'),
        findsOneWidget,
      );
      // No conversation to open: the card is all there is.
      expect(find.text('Open conversation'), findsNothing);
      final button = find.byWidgetPredicate(
        (w) => w is Button && w.label == label,
      );
      await tester.ensureVisible(button);
      await settle(tester);
      await tester.tap(button);
      await settle(tester);
      expect(harness.last.setKeepAwake.on, yes);

      await harness.apply(
        tester,
        _workspace(
          KeepAwake(available: true, laptop: true, on: yes, answered: true),
        ),
      );
      await settle(tester);
      final now = yes
          ? 'Agents keep working while this Mac is plugged in with its lid open.'
          : 'This Mac sleeps as usual.';
      expect(find.text(now), findsWidgets);
      expect(find.text('Answered'), findsOneWidget);
      expect(find.text(label), findsNothing);
    });
  }

  testWidgets('on a phone the question fits and answers', (tester) async {
    final harness = Harness(
      _workspace(KeepAwake(available: true, laptop: true, ask: true)),
    );
    await harness.show(tester, size: const Size(390, 844));
    await pumpFrames(tester);
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await tester.tap(find.text("Keep your agents working while you're away?"));
    await pumpFrames(tester);
    await tester.ensureVisible(find.text('Let it sleep'));
    await pumpFrames(tester);
    await tester.tap(find.text('Let it sleep'));
    await pumpFrames(tester);
    expect(harness.last.setKeepAwake.on, isFalse);
    expect(tester.takeException(), isNull);
  });

  testWidgets('a refused answer says why in the card', (tester) async {
    final harness = Harness(
      _workspace(KeepAwake(available: true, laptop: true, ask: true)),
    );
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    await tester.tap(find.text("Keep your agents working while you're away?"));
    await settle(tester);
    harness.backend.refuse = 'The Mac said no.';
    await tester.ensureVisible(find.text('Keep it awake'));
    await settle(tester);
    await tester.tap(find.text('Keep it awake'));
    await settle(tester);
    expect(
      find.textContaining("Couldn't keep this Mac awake: The Mac said no."),
      findsOneWidget,
    );
  });

  testWidgets('seen from another device, the Mac goes by its Claktory\'s '
      'name, never "this Mac"', (tester) async {
    final harness = Harness(
      _workspace(KeepAwake(available: true, laptop: true, ask: true))
        ..settings.name = 'Studio',
      localBackend: false,
    );
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    expect(
      find.text(
        'Agents keep working while Studio is plugged in with its lid '
        'open.',
      ),
      findsOneWidget,
    );
    harness.page.value = AppPage.settings;
    await settle(tester);
    await reveal(tester, find.text('Keep Studio awake while agents work'));
    expect(find.text(_label), findsNothing);
  });

  test("a let-sleep answer keeps a Claktory's name as it was typed", () {
    final remote = Harness(
      _workspace(KeepAwake(available: true, laptop: true, answered: true))
        ..settings.name = 'studio',
      localBackend: false,
    ).backend;
    expect(
      keepAwakeAnswered(remote, remote.workspace.workspace.keepAwake),
      'studio sleeps as usual.',
    );
    final local = Harness(
      _workspace(KeepAwake(available: true, laptop: true, answered: true)),
    ).backend;
    expect(
      keepAwakeAnswered(local, local.workspace.workspace.keepAwake),
      'This Mac sleeps as usual.',
    );
  });
}
