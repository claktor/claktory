// Kinds of report as data (pipes-automation.md §11, T4): Settings → Pipes
// lists them with what crosses; the editor sends a new kind or version with
// its caps clamped; a pipe chooses which kinds it forwards; a report is
// shown against the version it was written in, numbers and dates as read;
// and History shows a kind's change, with what widened.
import 'package:factory_app/controls.dart' show Select, showModal;
import 'package:factory_app/pipe_reports.dart';
import 'package:factory_app/report_types.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Agent _agent(String id, String name, {String pipe = '', bool reads = false}) =>
    Agent(
      id: id,
      name: name,
      projectId: 'p1',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
      pipeId: pipe,
      quarantined: pipe.isNotEmpty,
      readsReports: reads,
    );

const _text = ReportFieldType.REPORT_FIELD_TYPE_TEXT;
const _enum = ReportFieldType.REPORT_FIELD_TYPE_ENUM;

final _bug = ReportSchema(
  id: 'bug_report',
  name: 'Bug report',
  version: 1,
  builtin: true,
  fields: [
    ReportField(name: 'title', label: 'Title', type: _text, required: true),
    ReportField(
      name: 'severity',
      label: 'Severity',
      type: _enum,
      fact: true,
      choices: ['crash', 'cosmetic'],
      rule: 'One of crash, cosmetic',
    ),
  ],
  outcomes: [
    PipeReportState.PIPE_REPORT_STATE_ACCEPTED,
    PipeReportState.PIPE_REPORT_STATE_FIXED,
  ],
);

final _question = ReportSchema(
  id: 'question',
  name: 'Question',
  version: 2,
  byId: 'rivet',
  atMs: Int64(DateTime.now().millisecondsSinceEpoch - 3600000),
  fields: [
    ReportField(name: 'question', label: 'Question', type: _text),
    ReportField(
      name: 'votes',
      label: 'Votes',
      type: ReportFieldType.REPORT_FIELD_TYPE_NUMBER,
      fact: true,
    ),
    ReportField(
      name: 'due',
      label: 'Due',
      type: ReportFieldType.REPORT_FIELD_TYPE_DATE,
      fact: true,
    ),
  ],
  outcomes: [
    PipeReportState.PIPE_REPORT_STATE_DONE,
    PipeReportState.PIPE_REPORT_STATE_DECLINED,
  ],
  widenedFields: ['due'],
);

final _rules = ReportTypeRules(
  textBytes: 4096,
  listItems: 20,
  itemBytes: 1024,
  fields: 20,
  facts: 12,
  choices: 32,
  attachments: 3,
  attachmentMb: 2,
  numberAbs: Int64(1000000000),
  nameChars: 60,
  labelChars: 80,
  ruleChars: 200,
  types: 50,
  perPipe: 10,
  numbers: 1,
  dates: 2,
  reservedIds: ['to', 'recipient', 'url'],
  routingTokens: ['recipient', 'forward', 'cc', 'url', 'link', 'agent', 'pipe'],
  textKinds: [
    ReportFieldType.REPORT_FIELD_TYPE_TEXT,
    ReportFieldType.REPORT_FIELD_TYPE_TEXT_LIST,
    ReportFieldType.REPORT_FIELD_TYPE_ATTACHMENTS,
  ],
);

Workspace _workspace() => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/Users/someone/f')],
  agents: [
    _agent('kai', 'Kai', pipe: 'pipe1'),
    _agent('ingrid', 'Ingrid', reads: true),
    _agent('omar', 'Omar'),
    Agent(id: 'rivet', name: 'Rivet', role: 'Admin', modelAvailable: true),
  ],
  pipes: [
    Pipe(
      id: 'pipe1',
      name: 'Acme launch',
      projectId: 'p1',
      state: PipeState.PIPE_STATE_OPEN,
      createdMs: Int64(1000),
      peer: PipePeer(claktoryName: "Ana's Claktory", ownerName: 'Ana'),
      memberIds: ['kai'],
      quarantinedIds: ['kai'],
      leadId: 'kai',
      forward: PipeForward(
        recipientId: 'ingrid',
        schemaId: 'bug_report',
        typeIds: ['bug_report'],
        revision: 1,
      ),
      reports: PipeReportCounts(today: 1, roomToday: 29),
    ),
  ],
  reportSchemas: [_bug, _question],
  reportTypeRules: _rules,
  selectedAgentId: 'omar',
);

/// The editor's text fields, in order.
final _inputs = find.descendant(
  of: find.byType(ReportTypeEditor),
  matching: find.byType(EditableText),
);

void main() {
  test('a field id reads as routing as Claktory refuses it', () {
    for (final id in [
      'to',
      'send_to',
      'forward_to',
      'cc_list',
      'link',
      'agent_name',
      'my_pipe',
      'web_url',
      'recipient',
      'urls',
      'recipients',
      'web_links',
    ]) {
      expect(readsAsRouting(id, _rules), isTrue, reason: id);
    }
    for (final id in [
      'time_to_fix',
      'toast',
      'title',
      'linked_issue',
      'status',
      'notify',
      'file_path',
      '',
    ]) {
      expect(readsAsRouting(id, _rules), isFalse, reason: id);
    }
  });

  Future<Harness> show(WidgetTester tester, {Workspace? workspace}) async {
    final harness = Harness(workspace ?? _workspace());
    await harness.show(tester, size: const Size(1200, 1600));
    await settle(tester);
    return harness;
  }

  Future<void> open(WidgetTester tester, Widget form) async {
    showModal(tester.element(find.text('Omar').first), (context, close) {
      return SingleChildScrollView(child: form);
    });
    await settle(tester);
  }

  testWidgets('Settings → Pipes lists the kinds, built-in ones marked; one '
      'opens to what crosses, whose words each field holds', (tester) async {
    await show(tester);
    await open(tester, const ReportTypesSettings());

    expect(find.text('Report types'), findsOneWidget);
    expect(find.text('Bug report  Built in', findRichText: true), findsOne);
    expect(find.text('2 fields · v1 · used by 1 pipe'), findsOneWidget);
    expect(find.text('3 fields · v2'), findsOneWidget);

    await tester.tap(find.text('Question', findRichText: true).first);
    await settle(tester);
    expect(find.text('Version 2 · changed by Rivet 1 h ago'), findsOneWidget);
    // The question is their words; the number and the date only choices.
    expect(
      find.text('Their words · like reading their messages'),
      findsOneWidget,
    );
    expect(
      find.text('Safe to read · can’t carry their words'),
      findsNWidgets(2),
    );
    // Its last edit widened Due.
    expect(find.text('Due  Wider', findRichText: true), findsOneWidget);
    expect(
      find.text('Its reader can answer: Done and Declined.'),
      findsOneWidget,
    );
    expect(find.text('Delete…'), findsOneWidget);
  });

  testWidgets('a built-in kind can be edited, never deleted', (tester) async {
    await show(tester);
    await open(tester, const ReportTypesSettings());
    await tester.tap(find.text('Bug report  Built in', findRichText: true));
    await settle(tester);
    expect(find.text('Version 1 · built in'), findsOneWidget);
    expect(find.text('Edit…'), findsOneWidget);
    expect(find.text('Delete…'), findsNothing);
  });

  testWidgets('deleting a kind asks first', (tester) async {
    final harness = await show(tester);
    showReportType(tester.element(find.text('Omar').first), _question);
    await settle(tester);
    await tester.tap(find.text('Delete…'));
    await settle(tester);
    expect(find.text('Delete “Question”?'), findsOneWidget);
    await tester.tap(find.text('Delete'));
    await settle(tester);
    expect(harness.last.deleteReportType.id, 'question');
  });

  testWidgets('a new kind: a name, fields with their caps clamped to '
      "Claktory's ceilings, a list's choices, and the answers", (tester) async {
    final harness = await show(tester);
    editReportType(tester.element(find.text('Omar').first));
    await settle(tester);
    expect(find.text('New report type'), findsOneWidget);
    expect(find.text('Nothing yet.'), findsOneWidget);

    await tester.enterText(_inputs.first, 'Task request');
    await tester.tap(find.text('Add a field'));
    await settle(tester);
    await tester.tap(find.text('Add a field'));
    await settle(tester);
    expect(find.text('2 of 20 fields · 0 of 12 safe to read'), findsOneWidget);

    // The first: text, up to 9999 bytes, clamped to 4096.
    final fields = _inputs;
    await tester.enterText(fields.at(1), 'Needed');
    await settle(tester);
    // Label, bytes, hint for the first; then the second's label.
    await tester.enterText(fields.at(2), '9999');
    await settle(tester);

    expect(tester.widget<EditableText>(_inputs.at(2)).controller.text, '4096');

    // The second: a choice from a list.
    final kinds = find.byType(Select<ReportFieldType>);
    tester.widget<Select<ReportFieldType>>(kinds.at(1)).onChanged(_enum);
    await settle(tester);
    expect(find.text('1 of 20 fields · 1 of 12 safe to read'), findsNothing);
    expect(find.text('2 of 20 fields · 1 of 12 safe to read'), findsOneWidget);
    final second = _inputs;
    // Its label, then Add a choice.
    await tester.enterText(second.at(4), 'Priority');
    await tester.enterText(second.at(5), 'Very High');
    await tester.tap(find.text('Add'));
    await settle(tester);
    expect(find.text('Very high'), findsOneWidget);
    expect(find.text('1 of 32 choices'), findsOneWidget);

    await tester.tap(find.text('Save'));
    await settle(tester);
    final sent = harness.last.saveReportType;
    expect(sent.baseVersion, 0);
    expect(sent.type.id, '');
    expect(sent.type.name, 'Task request');
    expect(sent.type.fields[0].label, 'Needed');
    expect(sent.type.fields[0].type, _text);
    expect(sent.type.fields[0].maxBytes, 4096);
    expect(sent.type.fields[0].name, '');
    expect(sent.type.fields[1].type, _enum);
    expect(sent.type.fields[1].choices, ['very_high']);
    expect(sent.type.outcomes, [
      PipeReportState.PIPE_REPORT_STATE_ACCEPTED,
      PipeReportState.PIPE_REPORT_STATE_DONE,
      PipeReportState.PIPE_REPORT_STATE_NEEDS_INFO,
    ]);
    // Saved, it closes.
    expect(find.text('New report type'), findsNothing);
  });

  testWidgets('editing a kind sends a new version from the one it began '
      'with, its fields keeping their ids; a refusal stays on screen; a '
      'label that reads as routing says so', (tester) async {
    final harness = await show(tester);
    harness.backend.refuse =
        'A field\'s id can\'t be "to": it would read as where the report goes.';
    editReportType(tester.element(find.text('Omar').first), type: _question);
    await settle(tester);
    expect(find.text('Edit “Question”'), findsOneWidget);
    expect(find.textContaining('Saving makes version 3.'), findsOneWidget);

    await tester.tap(find.text('Add a field'));
    await settle(tester);
    // Its one number field is as many as it may have: a new field isn't
    // offered Number, the one there keeps it.
    List<ReportFieldType> offered(int i) => [
      for (final option
          in tester
              .widget<Select<ReportFieldType>>(
                find.byType(Select<ReportFieldType>).at(i),
              )
              .options)
        option.value,
    ];
    expect(
      offered(3),
      isNot(contains(ReportFieldType.REPORT_FIELD_TYPE_NUMBER)),
    );
    expect(offered(3), contains(ReportFieldType.REPORT_FIELD_TYPE_DATE));
    expect(offered(1), contains(ReportFieldType.REPORT_FIELD_TYPE_NUMBER));
    // Name, then three fields' label and hint, then the new one's label.
    final last = _inputs;
    await tester.enterText(last.at(last.evaluate().length - 3), 'To');
    await settle(tester);
    expect(
      find.text(
        'A field can’t be called “To”: it would read as where the '
        'report goes.',
      ),
      findsOneWidget,
    );

    await tester.tap(find.text('Save'));
    await settle(tester);
    final sent = harness.last.saveReportType;
    expect(sent.baseVersion, 2);
    expect(sent.type.id, 'question');
    expect(
      [for (final f in sent.type.fields) f.name],
      ['question', 'votes', 'due', ''],
    );
    expect(
      find.textContaining('would read as where the report goes'),
      findsNWidgets(2),
    );
    expect(find.text('Edit “Question”'), findsOneWidget);
  });

  testWidgets("a pipe's settings choose the kinds it forwards, to the same "
      'agent, never none', (tester) async {
    final harness = await show(tester);
    await open(tester, ForwardReportsField(pipe: _workspace().pipes.single));
    expect(find.text('Can forward'), findsOneWidget);
    expect(
      find.text(
        'Kai collects reports from the other side and forwards each '
        'finished one to Ingrid. Reports hold the other side’s own words.',
      ),
      findsOneWidget,
    );
    final before = harness.backend.sent.length;
    // The only one ticked can't be unticked.
    await tester.tap(find.text('Bug report'));
    await settle(tester);
    expect(harness.backend.sent.length, before);

    await tester.tap(find.text('Question'));
    await settle(tester);
    final sent = harness.last.setPipeForward;
    expect(sent.pipeId, 'pipe1');
    expect(sent.recipientId, 'ingrid');
    expect(sent.typeIds, ['bug_report', 'question']);
    expect(sent.schemaId, 'bug_report');

    // Choosing where they go leaves the kinds as they are.
    tester
        .widget<Select<String>>(find.byType(Select<String>).first)
        .onChanged('');
    await settle(tester);
    expect(harness.last.setPipeForward.typeIds, isEmpty);
    expect(harness.last.setPipeForward.schemaId, '');
  });

  testWidgets('a pipe that forwards to no one still shows the report types '
      'it keeps, so one can be turned off before it is deleted', (
    tester,
  ) async {
    final workspace = _workspace();
    workspace.pipes.single.forward
      ..recipientId = ''
      ..typeIds.add('question');
    final harness = await show(tester, workspace: workspace);
    await open(tester, ForwardReportsField(pipe: workspace.pipes.single));
    expect(find.text('Can forward'), findsOneWidget);
    await tester.tap(find.text('Question'));
    await settle(tester);
    expect(harness.last.setPipeForward.recipientId, '');
    expect(harness.last.setPipeForward.typeIds, ['bug_report']);
  });

  testWidgets('a report written in an older version shows against it, '
      'titled by its first text field, numbers and dates as read', (
    tester,
  ) async {
    final older = ReportSchema(
      id: 'question',
      name: 'Question',
      version: 1,
      fields: [
        ReportField(name: 'question', label: 'Asked', type: _text),
        ReportField(
          name: 'votes',
          label: 'Votes',
          type: ReportFieldType.REPORT_FIELD_TYPE_NUMBER,
        ),
        ReportField(
          name: 'due',
          label: 'Due',
          type: ReportFieldType.REPORT_FIELD_TYPE_DATE,
        ),
        ReportField(name: 'state', label: 'State', type: _enum),
        ReportField(name: 'on', label: 'On', type: _enum),
      ],
    );
    final report = PipeReport(
      id: Int64(7),
      pipeId: 'pipe1',
      number: 7,
      state: PipeReportState.PIPE_REPORT_STATE_DONE,
      schemaId: 'question',
      typeVersion: 1,
      liaisonId: 'kai',
      recipientId: 'ingrid',
      receivedMs: Int64(DateTime.now().millisecondsSinceEpoch - 60000),
      values: [
        ReportValue(field_1: 'question', text: 'Can we ship Friday?'),
        ReportValue(field_1: 'votes', number: Int64(12345)),
        ReportValue(field_1: 'due', text: '2026-11-27'),
        ReportValue(field_1: 'state', text: 'on_track'),
        ReportValue(field_1: 'on', text: 'iphone'),
      ],
    );
    final harness = await show(tester);
    harness.backend
      ..pipeReports = [report]
      ..pipeReportSchemas = [older];
    showReports(
      tester.element(find.text('Omar').first),
      pipe: _workspace().pipes.single,
    );
    await settle(tester);
    await settle(tester);
    expect(
      find.text('#7  Can we ship Friday?  Done', findRichText: true),
      findsOneWidget,
    );
    await tester.tap(find.textContaining('Can we ship', findRichText: true));
    await settle(tester);
    expect(find.text('#7  Can we ship Friday?'), findsOneWidget);
    expect(
      find.text(
        'Question (version 1) · Written by the other side, through Kai.',
      ),
      findsOneWidget,
    );
    // The old version's label, not the current one's.
    expect(find.text('Votes'), findsOneWidget);
    expect(find.text('12,345'), findsOneWidget);
    expect(find.text('27 Nov 2026'), findsOneWidget);
    // A list's choices as words.
    expect(find.text('On track'), findsOneWidget);
    expect(find.text('iPhone'), findsOneWidget);
  });

  testWidgets("History: a kind's change by the person, saying it lets more "
      'cross; it opens to what widened and what went', (tester) async {
    final harness = await show(tester);
    final before = ReportSchema(
      id: 'question',
      name: 'Question',
      version: 1,
      fields: [
        ReportField(name: 'question', label: 'Question', type: _text),
        ReportField(name: 'context', label: 'Context', type: _text),
      ],
    );
    harness.backend.needsYouHistory = [
      NeedsYouRecord(
        kind: NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_REPORT_TYPE,
        title: 'You changed the Question kind',
        outcome: NeedsYouOutcome.NEEDS_YOU_OUTCOME_CHANGED,
        decidedMs: Int64(DateTime.now().millisecondsSinceEpoch - 60000),
        reportType: ReportSchema(
          id: 'question',
          name: 'Question',
          version: 2,
          fields: [
            ReportField(name: 'question', label: 'Question', type: _text),
            ReportField(
              name: 'due',
              label: 'Due',
              type: ReportFieldType.REPORT_FIELD_TYPE_DATE,
            ),
          ],
          widenedFields: ['due'],
        ),
        reportTypeBefore: before,
        widens: true,
      ),
    ];
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await tester.tap(find.text('History'));
    await settle(tester);
    await settle(tester);
    expect(find.text('You changed the Question kind'), findsOneWidget);
    expect(find.text('Lets more of their words through'), findsOneWidget);
    expect(find.textContaining('You', findRichText: true), findsWidgets);

    await tester.tap(find.text('You changed the Question kind'));
    await settle(tester);
    expect(
      find.text('This change lets more of the other side’s words through.'),
      findsOneWidget,
    );
    expect(find.text('Due  New', findRichText: true), findsOneWidget);
    expect(find.text('Context  Removed'), findsOneWidget);
  });
}
