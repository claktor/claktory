import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

Entry assistant(int seq, String text) => Entry(
  seq: Int64(seq),
  assistant: AssistantText(text: text),
);

Entry thinking(int seq, String text) => Entry(
  seq: Int64(seq),
  thinking: Thinking(text: text),
);

Entry tool(int seq, ToolCallState state, {String output = ''}) => Entry(
  seq: Int64(seq),
  toolCall: ToolCall(
    callId: 'c$seq',
    name: 'bash',
    output: output,
    state: state,
  ),
);

Entry user(int seq, String text) => Entry(
  seq: Int64(seq),
  user: UserText(text: text),
);

EntryUpdate update(String agentId, Entry entry) =>
    EntryUpdate(agentId: agentId, entry: entry);

EntryDelta delta(String agentId, int seq, String text) =>
    EntryDelta(agentId: agentId, seq: Int64(seq), text: text);

List<int> seqs(TranscriptStore store) => [
  for (var i = 0; i < store.length; i++) store[i].seq,
];

/// Counts notifications from a listenable.
class Counter {
  Counter(Listenable listenable) {
    listenable.addListener(() => count++);
  }

  var count = 0;
}

void main() {
  test('a transcript replaces everything loaded before it', () {
    final store = TranscriptStore();
    store.load(
      Transcript(agentId: 'a', entries: [user(1, 'hi'), assistant(2, 'hello')]),
    );
    final first = store[0];
    final list = Counter(store);

    store.load(Transcript(agentId: 'b', entries: [user(7, 'other agent')]));

    expect(store.agentId, 'b');
    expect(seqs(store), [7]);
    expect(store[0].entry.user.text, 'other agent');
    expect(store.loads, 2);
    expect(list.count, 1);
    // Models from the previous transcript are disposed with it.
    expect(() => first.addListener(() {}), throwsStateError);
    // Updates for the previous agent no longer apply.
    store.update(update('a', assistant(3, 'late')));
    expect(seqs(store), [7]);
  });

  test('an update with a new seq adds the entry in seq order', () {
    final store = TranscriptStore()
      ..load(
        Transcript(agentId: 'a', entries: [user(1, 'go'), assistant(4, 'x')]),
      );
    final list = Counter(store);
    final revision = Counter(store.revision);

    store.update(update('a', tool(6, ToolCallState.TOOL_CALL_STATE_PENDING)));
    store.update(update('a', thinking(3, 'late arrival')));

    expect(seqs(store), [1, 3, 4, 6]);
    expect(store[1].entry.thinking.text, 'late arrival');
    expect(list.count, 2);
    expect(revision.count, 2);
  });

  test('an update with a known seq replaces that entry in place', () {
    final store = TranscriptStore()
      ..load(
        Transcript(
          agentId: 'a',
          entries: [
            user(1, 'run it'),
            tool(2, ToolCallState.TOOL_CALL_STATE_RUNNING),
          ],
        ),
      );
    final model = store[1];
    final list = Counter(store);
    final entry = Counter(model);
    final revision = Counter(store.revision);

    store.update(
      update('a', tool(2, ToolCallState.TOOL_CALL_STATE_DONE, output: 'ok')),
    );

    expect(seqs(store), [1, 2]);
    expect(identical(store[1], model), isTrue);
    expect(model.entry.toolCall.state, ToolCallState.TOOL_CALL_STATE_DONE);
    expect(model.entry.toolCall.output, 'ok');
    // Only the entry's own widget hears about it; the list is unchanged.
    expect(list.count, 0);
    expect(entry.count, 1);
    expect(revision.count, 1);
  });

  test('deltas append to assistant and thinking text', () {
    final store = TranscriptStore()
      ..load(
        Transcript(
          agentId: 'a',
          entries: [thinking(1, 'Let'), assistant(2, '')],
        ),
      );
    final list = Counter(store);
    final entry = Counter(store[1]);

    store.delta(delta('a', 1, ' me think'));
    store.delta(delta('a', 2, '# Plan'));
    store.delta(delta('a', 2, '\n\n1. Read'));
    expect(store[1].entry.assistant.text, '# Plan\n\n1. Read');
    store.delta(delta('a', 2, ' the code'));

    expect(store[0].entry.thinking.text, 'Let me think');
    expect(store[1].entry.assistant.text, '# Plan\n\n1. Read the code');
    expect(entry.count, 3);
    expect(list.count, 0);
  });

  test('a replace after deltas carries the final text without doubling it', () {
    final store = TranscriptStore()
      ..load(Transcript(agentId: 'a', entries: [assistant(5, 'Hel')]));
    store.delta(delta('a', 5, 'lo'));
    store.delta(delta('a', 5, ' wor'));

    store.update(update('a', assistant(5, 'Hello world')));
    store.delta(delta('a', 5, '!'));

    expect(store[0].entry.assistant.text, 'Hello world!');
  });

  test('deltas for entries that do not stream text are ignored', () {
    final store = TranscriptStore()
      ..load(
        Transcript(
          agentId: 'a',
          entries: [
            user(1, 'hi'),
            tool(2, ToolCallState.TOOL_CALL_STATE_RUNNING),
          ],
        ),
      );
    final revision = Counter(store.revision);

    store.delta(delta('a', 1, 'x'));
    store.delta(delta('a', 2, 'x'));
    store.delta(delta('a', 9, 'x'));

    expect(store[0].entry.user.text, 'hi');
    expect(store[1].entry.toolCall.output, '');
    expect(revision.count, 0);
  });

  test("other agents' updates and deltas are ignored", () {
    final store = TranscriptStore()
      ..load(Transcript(agentId: 'a', entries: [assistant(1, 'mine')]));
    final list = Counter(store);
    final entry = Counter(store[0]);
    final revision = Counter(store.revision);

    store.update(update('b', assistant(1, 'theirs')));
    store.update(update('b', user(2, 'theirs')));
    store.delta(delta('b', 1, ' and theirs'));

    expect(seqs(store), [1]);
    expect(store[0].entry.assistant.text, 'mine');
    expect(list.count + entry.count + revision.count, 0);
  });

  test('the workspace store answers lookups from the latest snapshot', () {
    final store = WorkspaceStore();
    final notified = Counter(store);
    store.apply(
      Workspace(
        projects: [
          Project(id: 'p1', name: 'Factory'),
          Project(id: 'p2', name: 'Site'),
        ],
        agents: [
          Agent(id: 'a1', projectId: 'p1', name: 'Builder'),
          Agent(id: 'a2', projectId: 'p2', name: 'Writer'),
          Agent(id: 'a3', projectId: 'p1', name: 'Reviewer'),
        ],
        models: [Model(id: 'anthropic/claude-opus-5-5', name: 'Opus 5.5')],
        selectedAgentId: 'a3',
      ),
    );

    expect(notified.count, 1);
    expect(store.selectedAgent?.name, 'Reviewer');
    expect([for (final a in store.agentsIn('p1')) a.id], ['a1', 'a3']);
    expect(store.model('anthropic/claude-opus-5-5')?.name, 'Opus 5.5');

    store.apply(
      Workspace(
        projects: [Project(id: 'p1', name: 'Factory')],
      ),
    );
    expect(store.selectedAgent, isNull);
    expect(store.agentsIn('p1'), isEmpty);
  });
}
