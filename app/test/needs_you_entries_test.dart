// Needs you's store of cards: one per Claktory, shared by the
// Needs you page and focus mode, fetching each waiting card once, keeping
// held ones up to date as they stop waiting.
import 'package:factory_app/inbox.dart';
import 'package:factory_app/needs_you_entries.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace({bool waiting = true}) => Workspace(
  agents: [
    Agent(
      id: 'kai',
      name: 'Kai',
      modelAvailable: true,
      waitingSeqs: [if (waiting) Int64(7)],
    ),
  ],
);

Entry _ask({
  AskPersonState state = AskPersonState.ASK_PERSON_STATE_PENDING,
}) => Entry(
  seq: Int64(7),
  timestampMs: Int64(1000),
  askPerson: AskPerson(id: 'q7', title: 'Ship it?', state: state),
);

int _loads(Harness harness) =>
    harness.backend.sent.where((m) => m.hasLoadEntries()).length;

void main() {
  const item = NeedsYouItem.entry('kai', 7);

  testWidgets('one store per Claktory: two readers, one fetch', (
    tester,
  ) async {
    final harness = Harness(_workspace());
    harness.backend.entriesById['kai/7'] = _ask();
    final store = NeedsYouEntries.of(harness.backend);
    expect(NeedsYouEntries.of(harness.backend), same(store));
    void page() {}
    void focus() {}
    store
      ..addListener(page)
      ..addListener(focus);
    await tester.pump();
    expect(store.entryOf(item)?.askPerson.title, 'Ship it?');
    expect(store.timeOf(item, harness.backend.workspace.workspace), 1000);
    expect(_loads(harness), 1);
    store
      ..removeListener(page)
      ..removeListener(focus);
  });

  testWidgets('a held card that stops waiting is fetched once more, to show '
      'how it ended; one not held is not', (tester) async {
    final harness = Harness(_workspace());
    harness.backend.entriesById['kai/7'] = _ask();
    final store = NeedsYouEntries.of(harness.backend);
    void listener() {}
    store.addListener(listener);
    await tester.pump();
    store.hold(item);
    harness.backend.entriesById['kai/7'] = _ask(
      state: AskPersonState.ASK_PERSON_STATE_ANSWERED,
    );
    harness.backend.workspace.apply(_workspace(waiting: false));
    await tester.pump();
    expect(
      store.entryOf(item)?.askPerson.state,
      AskPersonState.ASK_PERSON_STATE_ANSWERED,
    );
    expect(_loads(harness), 2);
    // Released, and waiting again then not: not fetched for it.
    store.release(item.key);
    harness.backend.workspace.apply(_workspace());
    await tester.pump();
    final before = _loads(harness);
    harness.backend.workspace.apply(_workspace(waiting: false));
    await tester.pump();
    expect(_loads(harness), before);
    store.removeListener(listener);
  });

  testWidgets("no one listening: nothing's fetched meanwhile", (tester) async {
    final harness = Harness(_workspace(waiting: false));
    harness.backend.entriesById['kai/7'] = _ask();
    final store = NeedsYouEntries.of(harness.backend);
    void listener() {}
    store
      ..addListener(listener)
      ..removeListener(listener);
    harness.backend.workspace.apply(_workspace());
    await tester.pump();
    expect(_loads(harness), 0);
    // Listened to again: brought up to date.
    store.addListener(listener);
    await tester.pump();
    expect(_loads(harness), 1);
    expect(store.entryOf(item), isNotNull);
    store.removeListener(listener);
  });
}
