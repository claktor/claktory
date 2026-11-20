import 'dart:convert';

import 'package:factory_app/characters/character.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/tool_views.dart';
import 'package:factory_app/transcript.dart' show TranscriptView;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'rivet_test.dart' show admin;

Workspace _workspace({ViewNode? layout}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/Users/me/factory')],
  agents: [
    admin(),
    Agent(
      id: 'a1',
      projectId: 'p1',
      name: 'Mike',
      role: 'Project Manager',
      modelAvailable: true,
      layout: layout,
    ),
    Agent(
      id: 'builder',
      projectId: 'p1',
      name: 'Sam',
      role: 'Backend',
      color: 'green',
      modelAvailable: true,
    ),
  ],
  selectedAgentId: 'a1',
  highlights: true,
);

/// A finished call of [name] with [input], [output] and [details].
Entry call(
  String name,
  Object input, {
  int seq = 1,
  String output = '',
  Object? details,
  ToolCallState state = ToolCallState.TOOL_CALL_STATE_DONE,
}) => Entry(
  seq: Int64(seq),
  reply: Int64(seq),
  toolCall: ToolCall(
    callId: 'c$seq',
    name: name,
    input: jsonEncode(input),
    output: output,
    details: details == null ? '' : jsonEncode(details),
    state: state,
  ),
);

Future<Harness> showing(
  WidgetTester tester,
  List<Entry> entries, {
  ViewNode? layout,
}) async {
  final harness = Harness(_workspace(layout: layout));
  harness.backend.transcript.load(Transcript(agentId: 'a1', entries: entries));
  await harness.show(tester);
  await settle(tester);
  return harness;
}

Finder inTranscript(Finder finder) =>
    find.descendant(of: find.byType(TranscriptView), matching: finder);

void main() {
  group('agents_send', () {
    testWidgets('names the recipient, says it was sent, shows the message', (
      tester,
    ) async {
      final harness = await showing(tester, [
        call(
          'agents_send',
          {'to': 'Sam', 'text': 'Please add **details**.'},
          output: 'Sent.',
          details: {
            'to': 'builder',
            'name': 'Sam',
            'role': 'Backend',
            'working': false,
          },
        ),
      ]);
      expect(find.text('→'), findsOneWidget);
      expect(find.text('Sam  Backend'), findsOneWidget);
      expect(find.text('Sent'), findsOneWidget);
      expect(find.textContaining('Please add'), findsOneWidget);
      expect(find.text('agents_send'), findsNothing, reason: 'no generic row');

      // The recipient opens their conversation.
      await tester.tap(find.text('Sam  Backend'));
      await settle(tester);
      expect(harness.last.selectAgent.agentId, 'builder');
    });

    testWidgets('says Queued when the recipient was working', (tester) async {
      await showing(tester, [
        call(
          'agents_send',
          {'to': 'builder', 'text': 'Hi'},
          output: "Sent; it's working and reads it after its current step.",
          details: {'to': 'builder', 'name': 'Sam', 'working': true},
        ),
      ]);
      expect(find.text('Queued'), findsOneWidget);
    });

    testWidgets('resolves a name without details, or shows it as written', (
      tester,
    ) async {
      await showing(tester, [
        call('agents_send', {'to': 'sam', 'text': 'One'}, seq: 1),
        call('agents_send', {'to': 'ghost', 'text': 'Two'}, seq: 2),
      ]);
      expect(find.text('Sam  Backend'), findsOneWidget);
      expect(find.text('ghost'), findsOneWidget);
    });

    testWidgets('a message shows its first line, Show more opens it', (
      tester,
    ) async {
      final long = List.generate(12, (i) => 'Line $i of the **message**.');
      await showing(tester, [
        call('agents_send', {'to': 'builder', 'text': long.join('\n\n')}),
      ]);
      expect(find.text('Line 0 of the message.'), findsOneWidget);
      expect(find.textContaining('Line 1 of'), findsNothing);
      await tester.tap(find.text('Show more'));
      await settle(tester);
      expect(find.textContaining('Line 11 of'), findsOneWidget);
      await tester.tap(find.text('Show less'));
      await settle(tester);
      expect(find.textContaining('Line 11 of'), findsNothing);
      expect(find.text('Show more'), findsOneWidget);
    });

    testWidgets('a short plain message has nothing to show more of', (
      tester,
    ) async {
      await showing(tester, [
        call('agents_send', {'to': 'builder', 'text': 'Done.'}),
      ]);
      expect(find.text('Done.'), findsOneWidget);
      expect(find.text('Show more'), findsNothing);
    });

    testWidgets('a failed send shows its error', (tester) async {
      await showing(tester, [
        call(
          'agents_send',
          {'to': 'x', 'text': 'Hi'},
          output: 'No agent you can message is called "x".',
          state: ToolCallState.TOOL_CALL_STATE_FAILED,
        ),
      ]);
      expect(find.text('Failed'), findsOneWidget);
      expect(
        find.text('No agent you can message is called "x".'),
        findsOneWidget,
      );
      expect(find.text('Sent'), findsNothing);
    });
  });

  testWidgets('a click on the line still shows the raw input and output', (
    tester,
  ) async {
    await showing(tester, [
      call(
        'agents_send',
        {'to': 'builder', 'text': 'Hi'},
        output: 'Sent.',
        details: {'to': 'builder', 'name': 'Sam', 'working': false},
      ),
    ]);
    expect(find.text('Input'), findsNothing);
    await tester.tap(find.text('→'));
    await settle(tester);
    expect(find.text('Input'), findsOneWidget);
    expect(find.text('Output'), findsOneWidget);
    expect(find.text('Sent.'), findsOneWidget);
  });

  testWidgets('a tool called through extension_call gets its view', (
    tester,
  ) async {
    await showing(tester, [
      call(
        'extension_call',
        {
          'tool': 'agents_send',
          'args': {'to': 'builder', 'text': 'Through the extension.'},
        },
        output: 'Sent.',
        details: {'to': 'builder', 'name': 'Sam', 'working': false},
      ),
    ]);
    expect(find.text('Sam  Backend'), findsOneWidget);
    expect(find.text('Through the extension.'), findsOneWidget);
    expect(find.text('extension_call'), findsNothing);
  });

  testWidgets('a tool with no view keeps the generic row', (tester) async {
    await showing(tester, [
      call('read_file', {'path': '/tmp/a.txt'}, output: 'hello'),
    ]);
    expect(find.text('read_file'), findsOneWidget);
    expect(find.text('/tmp/a.txt'), findsOneWidget);
  });

  testWidgets('propose_to_rivet names Rivet', (tester) async {
    await showing(tester, [
      call('propose_to_rivet', {
        'message': 'Add an agent.',
      }, output: 'Proposed.'),
    ]);
    expect(find.text('Proposed a message to'), findsOneWidget);
    expect(tester.widget<ToolAgent>(find.byType(ToolAgent)).id, 'rivet');
  });

  group('checklist', () {
    testWidgets('checklist_add lists the items added, with their owner', (
      tester,
    ) async {
      await showing(tester, [
        call(
          'checklist_add',
          {
            'items': ['Fix login', 'Dark mode'],
            'owner': 'builder',
          },
          output: 'Added #1-#2.',
          details: {
            'agent': 'a1',
            'items': [
              {'id': 1, 'text': 'Fix login', 'owner': 'builder'},
              {'id': 2, 'text': 'Dark mode', 'owner': 'builder'},
            ],
          },
        ),
      ]);
      expect(find.text('Added 2 items to the checklist'), findsOneWidget);
      expect(find.text('#1 Fix login'), findsOneWidget);
      expect(find.text('#2 Dark mode'), findsOneWidget);
      // Their owner's character on each.
      expect(inTranscript(find.byType(AgentCharacter)), findsNWidgets(2));
    });

    testWidgets('checklist_add without details lists the input', (
      tester,
    ) async {
      await showing(tester, [
        call('checklist_add', {
          'items': ['Only one'],
        }),
      ]);
      expect(find.text('Added 1 item to the checklist'), findsOneWidget);
      expect(find.text('Only one'), findsOneWidget);
    });

    testWidgets('checklist_update strikes done items and says what changed', (
      tester,
    ) async {
      await showing(tester, [
        call(
          'checklist_update',
          {
            'updates': [
              {'id': 1, 'status': 'done', 'note': 'shipped'},
              {'id': 9, 'status': 'done'},
            ],
          },
          details: {
            'agent': 'a1',
            'items': [
              {
                'id': 1,
                'text': 'Fix login',
                'status': 'done',
                'owner': '',
                'changed': ['status', 'note'],
                'note': 'shipped',
              },
              {'id': 9, 'problem': 'no such item'},
            ],
          },
        ),
      ]);
      expect(find.text('Updated 2 items on the checklist'), findsOneWidget);
      expect(find.text('#1 Fix login  Done'), findsOneWidget);
      expect(find.text('shipped'), findsOneWidget);
      expect(find.text('No such item.'), findsOneWidget);
      final text = tester.widget<Text>(find.text('#1 Fix login  Done'));
      final spans = <TextSpan>[];
      text.textSpan!.visitChildren((span) {
        if (span is TextSpan) spans.add(span);
        return true;
      });
      final item = spans.firstWhere((span) => span.text == 'Fix login');
      expect(item.style!.decoration, TextDecoration.lineThrough);
    });
  });

  group('memory', () {
    testWidgets('memory_read of a note says which and shows its text', (
      tester,
    ) async {
      await showing(tester, [
        call(
          'memory_read',
          {'note': 'release'},
          output:
              'release: How to release (updated 11-19)\n\nStamp, then release.',
          details: {
            'instance': '',
            'slug': 'release',
            'title': 'How to release',
          },
        ),
      ]);
      expect(find.text('Read note release  How to release'), findsOneWidget);
      expect(find.text('Stamp, then release.'), findsOneWidget);
    });

    testWidgets('a long note folds to its first line', (tester) async {
      await showing(tester, [
        call(
          'memory_read',
          {'note': 'release'},
          output:
              'release: How to release (updated 11-19)\n\n'
              '# Steps\n\nStamp the changelog.\n\nThen run the release.',
          details: {
            'instance': '',
            'slug': 'release',
            'title': 'How to release',
          },
        ),
      ]);
      expect(find.text('Steps'), findsOneWidget);
      expect(find.textContaining('Stamp the changelog'), findsNothing);
      await tester.tap(find.text('Show more'));
      await settle(tester);
      expect(find.textContaining('Stamp the changelog'), findsOneWidget);
    });

    testWidgets('memory_read of the manual says so', (tester) async {
      await showing(tester, [
        call(
          'memory_read',
          {'q': 'tunnel', 'instance': 'factory'},
          details: {'instance': 'factory', 'query': 'tunnel', 'hits': 2},
        ),
      ]);
      expect(
        find.text('Searched the manual for “tunnel”  2 matches'),
        findsOneWidget,
      );
    });

    testWidgets('memory_update of a note says Remembered, with its text', (
      tester,
    ) async {
      await showing(tester, [
        call(
          'memory_update',
          {
            'ops': [
              {
                'op': 'note_write',
                'slug': 'release',
                'summary': 'How to release',
                'body': 'Stamp, then release.',
              },
            ],
          },
          details: {
            'changes': [
              {
                'kind': 'note',
                'change': 'written',
                'slug': 'release',
                'title': 'How to release',
              },
            ],
          },
        ),
      ]);
      expect(find.text('Remembered: How to release'), findsOneWidget);
      expect(find.text('Stamp, then release.'), findsOneWidget);
    });

    testWidgets('several memory changes are listed', (tester) async {
      await showing(tester, [
        call('memory_update', {
          'ops': [
            {'op': 'pref_add', 'rule': 'Short answers', 'why': 'Phone'},
            {'op': 'note_remove', 'slug': 'old'},
          ],
        }),
      ]);
      expect(find.text('Updated memory  2 changes'), findsOneWidget);
      expect(find.text('Remembered: Short answers'), findsOneWidget);
      expect(find.text('Removed note old'), findsOneWidget);
    });
  });

  group('monitors', () {
    testWidgets('monitors_add says its name and when it fires', (tester) async {
      await showing(tester, [
        call(
          'monitors_add',
          {
            'monitors': [
              {
                'name': 'Nightly',
                'prompt': 'Check the build.',
                'when': {'every': '24h'},
              },
            ],
          },
          details: {
            'monitors': [
              {
                'id': 'm1',
                'agent': 'a1',
                'name': 'Nightly',
                'schedule': 'every 1d',
                'next': 1,
              },
            ],
          },
        ),
      ]);
      expect(find.text('Set a monitor Nightly  every 1d'), findsOneWidget);
      expect(find.text('Check the build.'), findsOneWidget);
    });

    testWidgets('without details, from its when', (tester) async {
      await showing(tester, [
        call('monitors_add', {
          'monitors': [
            {
              'name': 'CI',
              'prompt': 'p',
              'when': {'check': 'gh run view', 'every': '5m'},
            },
          ],
        }),
      ]);
      expect(find.text('Set a monitor CI  when gh run view'), findsOneWidget);
    });

    testWidgets('monitors_change says what it did', (tester) async {
      await showing(tester, [
        call(
          'monitors_change',
          {
            'changes': [
              {'id': 'm1', 'action': 'pause'},
            ],
          },
          details: {
            'monitors': [
              {'id': 'm1', 'name': 'Nightly', 'action': 'paused'},
            ],
          },
        ),
      ]);
      expect(find.text('Paused Nightly'), findsOneWidget);
    });
  });

  group('tasks', () {
    testWidgets('bash in the background runs until its task finishes', (
      tester,
    ) async {
      final harness = await showing(tester, [
        call(
          'bash',
          {'command': 'npm run dev', 'run_in_background': true},
          output: 'Started task-7.',
          details: {'task': 'task-7', 'pid': 42, 'log': '/tmp/task-7.log'},
        ),
      ]);
      expect(find.text('Background task task-7: npm run dev'), findsOneWidget);
      expect(find.text('Running'), findsOneWidget);
      harness.backend.transcript.update(
        EntryUpdate(
          agentId: 'a1',
          entry: Entry(
            seq: Int64(2),
            event: Event(
              source: EventSource.EVENT_SOURCE_TASK,
              fromId: 'task-7',
              from: 'task-7',
              summary: 'task-7 finished (exit 0, 4.2s): npm run dev',
              text: 'task-7 finished',
            ),
          ),
        ),
      );
      await settle(tester);
      expect(find.text('Running'), findsNothing);
      expect(find.text('exit 0, 4.2s'), findsOneWidget);
    });

    testWidgets('bash in the foreground keeps the generic row', (tester) async {
      await showing(tester, [
        call('bash', {'command': 'ls'}, output: 'a', details: {}),
      ]);
      expect(find.text('bash'), findsOneWidget);
    });

    testWidgets('tasks_list and tasks_stop', (tester) async {
      await showing(tester, [
        call(
          'tasks_list',
          {},
          seq: 1,
          details: {
            'agent': 'a1',
            'tasks': [
              {
                'task': 'task-3',
                'agent': 'a1',
                'command': 'cargo build',
                'running': true,
              },
              {
                'task': 'task-2',
                'agent': 'a1',
                'command': 'ls',
                'running': false,
                'status': 'exit 1',
              },
            ],
          },
        ),
        call(
          'tasks_stop',
          {'task': 'task-4', 'agent': 'builder'},
          seq: 2,
          details: {'task': 'task-4', 'agent': 'builder'},
        ),
      ]);
      expect(
        find.text('Listed background tasks  2 tasks, 1 running'),
        findsOneWidget,
      );
      expect(find.text('task-3  cargo build'), findsOneWidget);
      expect(find.text('exit 1'), findsOneWidget);
      expect(find.text('Stopped task-4  Sam'), findsOneWidget);
    });
  });

  group('show', () {
    final layout = ViewNode(
      id: 'root',
      split: ViewSplit(
        axis: ViewAxis.VIEW_AXIS_ROW,
        children: [
          ViewNode(
            id: 'chat',
            leaf: ViewLeaf(kind: 'chat'),
          ),
          ViewNode(
            id: 'v1',
            leaf: ViewLeaf(kind: 'text', path: '/Users/me/factory/README.md'),
          ),
        ],
      ),
    );

    testWidgets('names the file, and a click makes its pane glow', (
      tester,
    ) async {
      final harness = await showing(tester, [
        call(
          'show',
          {'path': 'README.md'},
          output: 'Showing README.md to Matt.',
          details: {
            'path': '/Users/me/factory/README.md',
            'kind': 'text',
            'pane': 'v1',
            'agent': 'a1',
          },
        ),
      ], layout: layout);
      expect(find.text('Showing'), findsOneWidget);
      await tester.tap(inTranscript(find.text('README.md')));
      await tester.pump();
      expect(
        harness.backend.highlights.glows.map((glow) => glow.key),
        contains('pane:a1:v1'),
      );
      await tester.pump(const Duration(seconds: 2));
    });

    testWidgets('once its pane is closed, a click opens it again', (
      tester,
    ) async {
      final harness = await showing(tester, [
        call(
          'show',
          {'path': '/tmp/notes.md'},
          details: {
            'path': '/tmp/notes.md',
            'kind': 'text',
            'pane': 'gone',
            'agent': 'a1',
          },
        ),
      ]);
      await tester.tap(find.text('notes.md'));
      await tester.pump();
      expect(harness.last.hasSetLayout(), isTrue);
      expect(harness.last.setLayout.agentId, 'a1');
    });
  });

  group('web', () {
    testWidgets('web_search lists result titles that open', (tester) async {
      final opened = <Uri>[];
      openWebLink = (url) async {
        opened.add(url);
        return true;
      };
      addTearDown(() => openWebLink = (url) async => false);
      await showing(tester, [
        call(
          'web_search',
          {
            'queries': ['flutter rendering'],
          },
          details: {
            'queries': [
              {
                'q': 'flutter rendering',
                'engine': 'brave',
                'results': [
                  {
                    'title': 'Flutter architectural overview',
                    'url': 'https://docs.flutter.dev/resources/architecture',
                    'snippet': '…',
                  },
                ],
              },
            ],
          },
        ),
      ]);
      expect(
        find.text('Searched “flutter rendering”  1 result'),
        findsOneWidget,
      );
      expect(find.text('docs.flutter.dev'), findsOneWidget);
      await tester.tap(find.text('Flutter architectural overview'));
      await tester.pump();
      expect(opened.single.host, 'docs.flutter.dev');
    });

    testWidgets('web_fetch names the page read', (tester) async {
      await showing(tester, [
        call(
          'web_fetch',
          {'url': 'https://www.example.com/a'},
          details: {
            'url': 'https://www.example.com/a',
            'title': 'Example',
            'lines': 40,
          },
        ),
      ]);
      expect(find.text('Read'), findsOneWidget);
      expect(find.text('Example'), findsOneWidget);
      expect(find.text('example.com'), findsOneWidget);
    });
  });

  testWidgets('database calls show their SQL and row counts', (tester) async {
    await showing(tester, [
      call(
        'db_query',
        {'sql': 'SELECT name FROM leads'},
        seq: 1,
        details: {
          'instance': '',
          'columns': ['name'],
          'rows': 2,
          'more': 0,
        },
      ),
      call(
        'db_exec',
        {'sql': 'DELETE FROM leads', 'instance': 'factory'},
        seq: 2,
        details: {'instance': 'factory', 'changed': 1},
      ),
      call('db_schema', {}, seq: 3, details: {'instance': '', 'tables': 3}),
    ]);
    expect(find.text('Queried the database  2 rows'), findsOneWidget);
    expect(find.text('SELECT name FROM leads'), findsOneWidget);
    expect(
      find.text("Changed Claktory's records  1 row changed"),
      findsOneWidget,
    );
    expect(find.text("Read the database's tables  3 tables"), findsOneWidget);
  });

  group('secrets', () {
    testWidgets('secrets_grant says what it gave to whom', (tester) async {
      final harness = await showing(tester, [
        call(
          'secrets_grant',
          {
            'secret': 'Cloudflare',
            'agents': ['Sam'],
          },
          output: 'Gave Cloudflare to Sam (builder).',
          details: {
            'secret': 'Cloudflare',
            'agents': [
              {'id': 'builder', 'name': 'Sam'},
            ],
          },
        ),
      ]);
      expect(find.text('Gave Cloudflare'), findsOneWidget);
      expect(find.text('to'), findsOneWidget);
      await tester.tap(inTranscript(find.text('Sam')));
      await settle(tester);
      expect(harness.last.selectAgent.agentId, 'builder');
    });

    testWidgets('without details, from the input', (tester) async {
      await showing(tester, [
        call('secrets_grant', {
          'secret': 'AWS',
          'agents': ['sam', 'nobody'],
        }),
      ]);
      expect(find.text('Gave AWS'), findsOneWidget);
      expect(inTranscript(find.text('Sam')), findsOneWidget);
      expect(find.text('nobody'), findsOneWidget);
    });

    testWidgets('secrets_revoke says whom and the tasks it stopped', (
      tester,
    ) async {
      await showing(tester, [
        call(
          'secrets_revoke',
          {
            'secret': 'Cloudflare',
            'agents': ['builder'],
            'stop_tasks': true,
          },
          details: {
            'secret': 'Cloudflare',
            'agents': [
              {
                'id': 'builder',
                'name': 'Sam',
                'stopped': ['task-1'],
              },
            ],
          },
        ),
      ]);
      expect(find.text('Took Cloudflare'), findsOneWidget);
      expect(find.text('back from'), findsOneWidget);
      expect(find.text("Stopped Sam's task-1"), findsOneWidget);
    });

    testWidgets('secrets_list counts and lists them', (tester) async {
      await showing(tester, [
        call(
          'secrets_list',
          {},
          details: {
            'secrets': [
              {
                'id': 's1',
                'name': 'AWS',
                'variables': ['AWS_ACCESS_KEY_ID', 'AWS_SECRET_ACCESS_KEY'],
                'agents': ['builder'],
              },
              {
                'id': 's2',
                'name': 'Resend',
                'variables': ['RESEND_API_KEY'],
                'agents': <String>[],
              },
            ],
          },
        ),
      ]);
      expect(find.text('Listed 2 secrets'), findsOneWidget);
      expect(
        find.text('AWS  AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY'),
        findsOneWidget,
      );
      expect(find.text('Resend  RESEND_API_KEY'), findsOneWidget);
      expect(inTranscript(find.byType(AgentCharacter)), findsOneWidget);
    });
  });

  test('describeWhen', () {
    expect(describeWhen({'every': '3h'}), 'every 3h');
    expect(describeWhen({'at': '09:00'}), 'at 09:00');
    expect(describeWhen({'in': '45m'}), 'in 45m');
    expect(
      describeWhen({
        'files': ['src/', 'a.csv'],
      }),
      'when src/, a.csv change',
    );
    expect(describeWhen({'watch': 'tail -F x'}), 'when tail -F x prints');
  });
}
