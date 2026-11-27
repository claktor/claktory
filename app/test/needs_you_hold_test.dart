// Needs you's list holds still under the finger: a request coming in above
// goes in with the offset grown by its height, at the very top too, and a
// pill says what's above ("↑ 1 new", "↑ 1 urgent"), scrolling up to it; a
// row leaving fades where it was, the rows under it closing up only once
// no finger is down.
import 'package:factory_app/inbox.dart' show NeedsYouList, needsYouItems;
import 'package:factory_app/needs_you_entries.dart' show NeedsYouEntries;
import 'package:factory_app/needs_you_hold.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final _now = DateTime.now();

Entry _ask(int seq, int minutesAgo, String title) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(
    _now.subtract(Duration(minutes: minutesAgo)).millisecondsSinceEpoch,
  ),
  askPerson: AskPerson(id: 'q$seq', title: title),
);

const _titles = {
  'june': 'Ship the migration tonight?',
  'mista': 'Turn on the relay rule?',
  'lea': 'Pick a tagline',
  'tom': 'Name the release',
  'kai': 'Publish the notes',
};

/// Who waits, oldest last: [waiting] by agent id.
Workspace _workspace(List<String> waiting, {List<Alert> alerts = const []}) =>
    Workspace(
      agents: [
        for (final id in _titles.keys)
          Agent(
            id: id,
            name: id[0].toUpperCase() + id.substring(1),
            modelAvailable: true,
            waitingSeqs: [if (waiting.contains(id)) Int64(1)],
          ),
      ],
      alerts: alerts,
      selectedAgentId: 'june',
    );

Future<Harness> _open(
  WidgetTester tester,
  List<String> waiting, {
  Size size = const Size(393, 852),
}) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  final harness = Harness(_workspace(waiting));
  // Newest first: kai the newest, june the oldest.
  var minutes = 50;
  for (final id in _titles.keys) {
    harness.backend.entriesById['$id/1'] = _ask(1, minutes, _titles[id]!);
    minutes -= 10;
  }
  await harness.show(tester, size: size);
  harness.page.value = AppPage.inbox;
  await pumpFrames(tester);
  await tester.pump();
  await pumpFrames(tester);
  return harness;
}

Finder _row(String title) =>
    find.descendant(of: find.byType(NeedsYouList), matching: find.text(title));

Finder get _pill => find.byKey(const ValueKey('needs-you-new-above'));

bool _pillShown(WidgetTester tester) =>
    tester
        .widget<AnimatedOpacity>(
          find.ancestor(of: _pill, matching: find.byType(AnimatedOpacity)),
        )
        .opacity >
    0;

void main() {
  test('the words, as Trish wrote them', () {
    expect(newAboveWords(count: 1, urgent: 0), '↑ 1 new');
    expect(newAboveWords(count: 3, urgent: 0), '↑ 3 new');
    expect(newAboveWords(count: 1, urgent: 1), '↑ 1 urgent');
    // Both, the urgent first.
    expect(newAboveWords(count: 3, urgent: 1), '↑ 1 urgent · 2 new');
    expect(
      newAboveSpoken(requests: 1, alerts: 0, urgent: 0),
      '1 new request above',
    );
    expect(
      newAboveSpoken(requests: 3, alerts: 0, urgent: 0),
      '3 new requests above',
    );
    expect(
      newAboveSpoken(requests: 0, alerts: 1, urgent: 0),
      '1 new alert above',
    );
    expect(newAboveSpoken(requests: 1, alerts: 1, urgent: 0), '2 new above');
    expect(
      newAboveSpoken(requests: 0, alerts: 1, urgent: 1),
      '1 urgent alert above',
    );
    expect(
      newAboveSpoken(requests: 1, alerts: 1, urgent: 1),
      '1 urgent alert and 1 new request above',
    );
    expect(
      newAboveSpoken(requests: 2, alerts: 3, urgent: 2),
      '2 urgent alerts and 3 new above',
    );
  });

  testWidgets('a request coming in above, at the very top: nothing in view '
      'moves; the pill says it, and takes you up to it', (tester) async {
    final harness = await _open(tester, ['june', 'mista', 'lea']);
    expect(_pillShown(tester), isFalse);
    final before = tester.getRect(_row('Pick a tagline'));
    final second = tester.getRect(_row('Turn on the relay rule?'));

    // Kai asks: the newest, so first in the list.
    await harness.apply(tester, _workspace(['june', 'mista', 'lea', 'kai']));
    await pumpFrames(tester);
    expect(tester.getRect(_row('Pick a tagline')), before);
    expect(tester.getRect(_row('Turn on the relay rule?')), second);
    expect(_pillShown(tester), isTrue);
    expect(find.text('↑ 1 new'), findsOneWidget);
    expect(find.bySemanticsLabel('1 new request above'), findsOneWidget);

    await tester.tap(_pill);
    await pumpFrames(tester);
    // Kai where the first row was; the rest under it.
    final kai = tester.getRect(_row('Publish the notes'));
    expect(kai.top, closeTo(before.top, 1));
    expect(tester.getRect(_row('Pick a tagline')).top, greaterThan(before.top));
    expect(_pillShown(tester), isFalse, reason: 'seen');
  });

  testWidgets('an urgent alert coming in above: "↑ 1 urgent"', (tester) async {
    final harness = await _open(tester, ['june', 'mista']);
    final before = tester.getRect(_row('Turn on the relay rule?'));
    await harness.apply(
      tester,
      _workspace(
        ['june', 'mista'],
        alerts: [
          Alert(
            id: 'a1',
            agentId: 'lea',
            fromName: 'Lea',
            title: "Claktory's disk is almost full",
            urgent: true,
            sentMs: Int64(_now.millisecondsSinceEpoch),
          ),
        ],
      ),
    );
    await pumpFrames(tester);
    expect(tester.getRect(_row('Turn on the relay rule?')), before);
    expect(find.text('↑ 1 urgent'), findsOneWidget);
    expect(find.bySemanticsLabel('1 urgent alert above'), findsOneWidget);
  });

  testWidgets('with an urgent alert first, a request coming in under it '
      "moves none of the rows under it (Zucchero's row 715)", (tester) async {
    final alert = Alert(
      id: 'a1',
      agentId: 'tom',
      fromName: 'Tom',
      title: "Claktory's disk is almost full",
      urgent: true,
      sentMs: Int64(_now.millisecondsSinceEpoch),
    );
    final harness = await _open(tester, ['june', 'mista', 'lea']);
    await harness.apply(
      tester,
      _workspace(['june', 'mista', 'lea'], alerts: [alert]),
    );
    await pumpFrames(tester);
    final lea = tester.getRect(_row('Pick a tagline'));
    final mista = tester.getRect(_row('Turn on the relay rule?'));
    // Kai asks: the newest request, so under the alert, above Lea.
    await harness.apply(
      tester,
      _workspace(['june', 'mista', 'lea', 'kai'], alerts: [alert]),
    );
    await pumpFrames(tester);
    expect(tester.getRect(_row('Pick a tagline')), lea);
    expect(tester.getRect(_row('Turn on the relay rule?')), mista);
    expect(
      tester.getRect(_row('Publish the notes')).bottom,
      lessThanOrEqualTo(lea.top),
    );
    // The alert went up past the top to make room, and Kai's top with it:
    // the pill says so, until they're in view.
    expect(find.text('↑ 1 urgent · 1 new'), findsOneWidget);
    await tester.tap(_pill);
    await pumpFrames(tester);
    expect(_pillShown(tester), isFalse);
  });

  testWidgets('a row that moves up from further down (its card changed by '
      'its agent) moves none in view; the pill counts it (Squalo R1)', (
    tester,
  ) async {
    final harness = await _open(tester, ['june', 'mista', 'lea', 'tom']);
    final tom = tester.getRect(_row('Name the release'));
    final lea = tester.getRect(_row('Pick a tagline'));
    // June's card, the oldest at the foot, changed by June just now: the
    // newest, so first.
    harness.backend.entriesById['june/1'] = _ask(1, 50, _titles['june']!)
      ..askPerson.updatedMs = Int64(DateTime.now().millisecondsSinceEpoch);
    final store = NeedsYouEntries.of(harness.backend);
    for (final item in needsYouItems(harness.backend.workspace.workspace)) {
      if (item.agentId == 'june') store.fetch(item);
    }
    await pumpFrames(tester);
    await tester.pump();
    await pumpFrames(tester);
    expect(tester.getRect(_row('Name the release')), tom);
    expect(tester.getRect(_row('Pick a tagline')), lea);
    expect(
      tester.getRect(_row(_titles['june']!)).bottom,
      lessThanOrEqualTo(tom.top),
    );
    expect(find.text('↑ 1 new'), findsOneWidget);

    // An older copy of June's card (fetched before the change): its row
    // stays first, and nothing moves.
    final june = tester.getRect(_row(_titles['june']!));
    harness.backend.entriesById['june/1'] = _ask(1, 50, _titles['june']!);
    for (final item in needsYouItems(harness.backend.workspace.workspace)) {
      if (item.agentId == 'june') store.fetch(item);
    }
    await pumpFrames(tester);
    await tester.pump();
    await pumpFrames(tester);
    expect(tester.getRect(_row(_titles['june']!)), june);
    expect(tester.getRect(_row('Name the release')), tom);
    expect(tester.getRect(_row('Pick a tagline')), lea);
  });

  testWidgets("a card changed while no conversation holds it: the Workspace's "
      'version brings it again, once; its row shows it as it is now and '
      "stays first (Squalo's B3)", (tester) async {
    Workspace versioned(int june) {
      final workspace = _workspace(['june', 'mista', 'lea', 'tom']);
      for (final agent in workspace.agents) {
        if (agent.waitingSeqs.isEmpty) continue;
        agent.waitingVersions[Int64(1)] = Int64(agent.id == 'june' ? june : 1);
      }
      return workspace;
    }

    final harness = await _open(tester, []);
    await harness.apply(tester, versioned(1));
    await pumpFrames(tester);
    int fetches(String agent) => harness.backend.sent
        .where(
          (message) =>
              message.hasLoadEntries() && message.loadEntries.agentId == agent,
        )
        .length;
    final before = fetches('june');
    expect(before, greaterThan(0));
    final tom = tester.getRect(_row('Name the release'));

    // Anything else in the Workspace: nothing fetched again.
    await harness.apply(tester, versioned(1)..selectedAgentId = 'mista');
    await pumpFrames(tester);
    expect(fetches('june'), before);

    // June changes her card: its version moves, and it's fetched again.
    harness.backend.entriesById['june/1'] = _ask(
      1,
      50,
      'Ship the migration on Friday?',
    )..askPerson.updatedMs = Int64(DateTime.now().millisecondsSinceEpoch);
    await harness.apply(tester, versioned(2));
    await pumpFrames(tester);
    await tester.pump();
    await pumpFrames(tester);
    expect(fetches('june'), before + 1);
    expect(_row('Ship the migration on Friday?'), findsOneWidget);
    expect(find.textContaining('Updated', findRichText: true), findsWidgets);
    expect(tester.getRect(_row('Name the release')), tom);

    // And it stays so: nothing fetched it back as it was.
    await harness.apply(tester, versioned(2)..selectedAgentId = 'june');
    await pumpFrames(tester);
    expect(fetches('june'), before + 1);
    expect(_row('Ship the migration on Friday?'), findsOneWidget);
  });

  testWidgets('scrolled down a long list, one coming in above moves nothing '
      'in view', (tester) async {
    final harness = await _open(tester, [
      'june',
      'mista',
      'lea',
      'tom',
    ], size: const Size(320, 400));
    await tester.drag(find.byType(NeedsYouList), const Offset(0, -150));
    await pumpFrames(tester);
    final before = tester.getRect(_row('Turn on the relay rule?'));
    await harness.apply(
      tester,
      _workspace(['june', 'mista', 'lea', 'tom', 'kai']),
    );
    await pumpFrames(tester);
    expect(tester.getRect(_row('Turn on the relay rule?')), before);
    expect(find.text('↑ 1 new'), findsOneWidget);
  });

  testWidgets('one answered elsewhere fades where it was; the rows under it '
      'close up only once no finger is down', (tester) async {
    final harness = await _open(tester, ['june', 'mista', 'lea']);
    final under = tester.getRect(_row('Turn on the relay rule?'));
    // A finger down on the list, about to tap.
    final finger = await tester.startGesture(
      tester.getCenter(_row('Ship the migration tonight?')),
    );
    await harness.apply(tester, _workspace(['june', 'mista']));
    await pumpFrames(tester);
    expect(tester.getRect(_row('Turn on the relay rule?')), under);
    // Faded, and takes no tap.
    expect(
      find.descendant(
        of: find.byType(IgnorePointer),
        matching: find.text('Pick a tagline'),
      ),
      findsOneWidget,
    );
    await tester.pump(const Duration(seconds: 2));
    expect(tester.getRect(_row('Turn on the relay rule?')), under);

    await finger.cancel();
    await pumpFrames(tester);
    expect(find.text('Pick a tagline'), findsNothing);
    expect(
      tester.getRect(_row('Turn on the relay rule?')).top,
      lessThan(under.top),
      reason: 'closed up',
    );
  });
}
