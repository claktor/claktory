// Needs you on a phone pulls down to refresh: what was answered while the
// list was open goes at once, and every card still waiting is fetched
// again; VoiceOver and TalkBack, which can't pull, have Refresh on the
// title. An iPhone's list bounces over a spinner, Android's drops a circle.
import 'package:cupertino_ui/cupertino_ui.dart'
    show CupertinoSliverRefreshControl;
import 'package:factory_app/inbox.dart' show NeedsYouList;
import 'package:factory_app/mobile/header.dart' show PhoneHeader;
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:material_ui/material_ui.dart' show RefreshIndicator;

import 'harness.dart';

Entry _ask(int seq, String title, {bool answered = false}) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(DateTime.now().millisecondsSinceEpoch),
  askPerson: AskPerson(
    id: 'q$seq',
    title: title,
    state: answered
        ? AskPersonState.ASK_PERSON_STATE_ANSWERED
        : AskPersonState.ASK_PERSON_STATE_UNSPECIFIED,
  ),
);

Workspace _workspace({bool mistaWaits = true}) => Workspace(
  agents: [
    Agent(
      id: 'june',
      name: 'June',
      modelAvailable: true,
      waitingSeqs: [Int64(1)],
    ),
    Agent(
      id: 'mista',
      name: 'Mista',
      modelAvailable: true,
      waitingSeqs: [if (mistaWaits) Int64(7)],
    ),
  ],
  selectedAgentId: 'june',
);

const _june = 'Ship the migration tonight?';
const _mista = 'Turn on the relay rule?';

Finder _row(String title) =>
    find.descendant(of: find.byType(NeedsYouList), matching: find.text(title));

Future<Harness> _open(WidgetTester tester, TargetPlatform platform) async {
  debugDefaultTargetPlatformOverride = platform;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  if (platform == TargetPlatform.android) {
    PhonePlatformPolicy.debugAndroidPolicy = true;
    addTearDown(() => PhonePlatformPolicy.debugAndroidPolicy = false);
  }
  final harness = Harness(_workspace());
  harness.backend.entriesById
    ..['june/1'] = _ask(1, _june)
    ..['mista/7'] = _ask(7, _mista);
  await harness.show(tester, size: const Size(393, 852));
  harness.page.value = AppPage.inbox;
  await pumpFrames(tester);
  await tester.pump();
  await pumpFrames(tester);
  return harness;
}

/// Mista's request opened from the list, then answered: on an iPhone alone
/// and back, on Android in place. It stays in the list, saying so.
Future<void> _answerMista(WidgetTester tester, Harness harness) async {
  await tester.tap(_row(_mista));
  await pumpFrames(tester);
  if (defaultTargetPlatform == TargetPlatform.iOS) {
    await tester.tap(find.bySemanticsLabel('Back').last);
    await pumpFrames(tester);
  }
  harness.backend.entriesById['mista/7'] = _ask(7, _mista, answered: true);
  await harness.apply(tester, _workspace(mistaWaits: false));
  await pumpFrames(tester);
  await tester.pump();
  await pumpFrames(tester);
  expect(_row(_mista), findsWidgets, reason: 'kept until you leave');
}

int _fetchesOf(Harness harness, String agentId) => harness.backend.sent
    .where(
      (message) =>
          message.hasLoadEntries() && message.loadEntries.agentId == agentId,
    )
    .length;

/// A finger pulling the list down from its top, slowly, then letting go.
Future<void> _pull(WidgetTester tester) async {
  // From the top, where a pull starts.
  tester
      .state<ScrollableState>(
        find.descendant(
          of: find.byType(NeedsYouList),
          matching: find.byType(Scrollable),
        ),
      )
      .position
      .jumpTo(0);
  await tester.pump();
  final gesture = await tester.startGesture(
    tester.getTopLeft(find.byType(NeedsYouList)) + const Offset(200, 40),
  );
  for (var step = 0; step < 8; step++) {
    await gesture.moveBy(const Offset(0, 40));
    await tester.pump(const Duration(milliseconds: 16));
  }
  await gesture.up();
  await pumpFrames(tester);
  await tester.pump(const Duration(seconds: 1));
  await pumpFrames(tester);
}

void main() {
  testWidgets('an iPhone: a pull drops what was answered here at once and '
      'fetches what waits again', (tester) async {
    final harness = await _open(tester, TargetPlatform.iOS);
    await _answerMista(tester, harness);
    final fetched = _fetchesOf(harness, 'june');

    await _pull(tester);
    expect(_row(_mista), findsNothing);
    expect(_row(_june), findsOneWidget);
    expect(_fetchesOf(harness, 'june'), greaterThan(fetched));
    expect(harness.backend.reconnectsAsked, 0);
    expect(
      find.byType(CupertinoSliverRefreshControl, skipOffstage: false),
      findsOneWidget,
    );
    expect(find.byType(RefreshIndicator), findsNothing);
  });

  testWidgets('Android: the same, with its own circle', (tester) async {
    final harness = await _open(tester, TargetPlatform.android);
    expect(find.byType(RefreshIndicator), findsOneWidget);
    await _answerMista(tester, harness);
    final fetched = _fetchesOf(harness, 'june');

    await _pull(tester);
    expect(_row(_mista), findsNothing);
    expect(_row(_june), findsOneWidget);
    expect(_fetchesOf(harness, 'june'), greaterThan(fetched));
  });

  testWidgets('VoiceOver and TalkBack refresh from the title', (tester) async {
    final semantics = tester.ensureSemantics();
    final harness = await _open(tester, TargetPlatform.iOS);
    await _answerMista(tester, harness);
    final fetched = _fetchesOf(harness, 'june');

    final title = tester.getSemantics(
      find.descendant(
        of: find.byType(PhoneHeader),
        matching: find.text('Needs you'),
      ),
    );
    const refresh = CustomSemanticsAction(label: 'Refresh');
    final id = CustomSemanticsAction.getIdentifier(refresh);
    expect(title.getSemanticsData().customSemanticsActionIds, contains(id));
    title.owner!.performAction(title.id, SemanticsAction.customAction, id);
    await pumpFrames(tester);
    expect(_row(_mista), findsNothing);
    expect(_row(_june), findsOneWidget);
    expect(_fetchesOf(harness, 'june'), greaterThan(fetched));
    semantics.dispose();
  });

  testWidgets('with the connection down, a pull reconnects first', (
    tester,
  ) async {
    final harness = await _open(tester, TargetPlatform.iOS);
    harness.backend.connected.value = false;
    await _pull(tester);
    expect(harness.backend.reconnectsAsked, 1);
    // Given up on after a while: the spinner never stays.
    await tester.pump(const Duration(seconds: 11));
    await pumpFrames(tester);
    expect(_row(_june), findsOneWidget);
  });

  testWidgets('nothing waiting: the words pull down too', (tester) async {
    final harness = await _open(tester, TargetPlatform.iOS);
    await harness.apply(
      tester,
      Workspace(
        agents: [Agent(id: 'june', name: 'June', modelAvailable: true)],
      ),
    );
    await tester.pump(const Duration(seconds: 1));
    await pumpFrames(tester);
    expect(find.text('Nothing is waiting for your answer.'), findsOneWidget);
    harness.backend.connected.value = false;
    final gesture = await tester.startGesture(
      tester.getCenter(find.text('Nothing is waiting for your answer.')),
    );
    for (var step = 0; step < 8; step++) {
      await gesture.moveBy(const Offset(0, 40));
      await tester.pump(const Duration(milliseconds: 16));
    }
    await gesture.up();
    await pumpFrames(tester);
    expect(harness.backend.reconnectsAsked, 1);
    await tester.pump(const Duration(seconds: 11));
    await pumpFrames(tester);
  });

  testWidgets('a Mac has no pull, nor Refresh', (tester) async {
    final semantics = tester.ensureSemantics();
    final harness = Harness(_workspace());
    harness.backend.entriesById['june/1'] = _ask(1, _june);
    await harness.show(tester, size: const Size(393, 852));
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    expect(find.byType(NeedsYouList), findsOneWidget);
    final title = tester.getSemantics(find.text('Needs you').first);
    final refresh = CustomSemanticsAction.getIdentifier(
      const CustomSemanticsAction(label: 'Refresh'),
    );
    expect(
      (title.getSemanticsData().customSemanticsActionIds ?? const <int>[])
          .contains(refresh),
      isFalse,
    );
    expect(find.byType(CupertinoSliverRefreshControl), findsNothing);
    expect(find.byType(RefreshIndicator), findsNothing);
    semantics.dispose();
  });
}
