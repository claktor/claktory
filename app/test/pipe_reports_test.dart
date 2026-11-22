// Automated pipes in the app (pipes-automation.md §7–8, H5/H6): where a
// pipe's reports go, in its settings and its project's; the agent that reads
// them; the "Forwarded" strip under an open pipe with what's waiting; a
// report's own words; the reader's Reports; the Map's line from a pipe to
// its reader; and changes Rivet or the person made, in History.
import 'package:factory_app/controls.dart' show Select, showModal;
import 'package:factory_app/forms.dart'
    show AgentSettingsForm, ProjectSettingsForm;
import 'package:factory_app/extensions.dart' show enabledExtensions;
import 'package:factory_app/map.dart';
import 'package:factory_app/map_layout.dart';
import 'package:factory_app/needs_you_history.dart';
import 'package:factory_app/pipe_reports.dart';
import 'package:factory_app/pipes.dart' show PipeSettingsForm;
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

final _schemaOf = ReportSchema(
  id: 'bug_report',
  name: 'Bug report',
  fields: [
    ReportField(name: 'title', label: 'Title'),
    ReportField(name: 'happened', label: 'What happened'),
    ReportField(
      name: 'steps',
      label: 'Steps',
      type: ReportFieldType.REPORT_FIELD_TYPE_TEXT_LIST,
    ),
    ReportField(
      name: 'blocking',
      label: 'Blocking',
      type: ReportFieldType.REPORT_FIELD_TYPE_BOOL,
    ),
  ],
);

Pipe _pipe({
  String to = 'ingrid',
  String setBy = '',
  PipeReportCounts? counts,
}) => Pipe(
  id: 'pipe1',
  name: 'Acme launch',
  projectId: 'p1',
  state: PipeState.PIPE_STATE_OPEN,
  createdMs: Int64(1000),
  peer: PipePeer(claktoryName: "Ana's Claktory", ownerName: 'Ana'),
  memberIds: ['kai', 'bo'],
  quarantinedIds: ['kai'],
  forward: to.isEmpty
      ? null
      : PipeForward(
          recipientId: to,
          schemaId: 'bug_report',
          revision: 1,
          setById: setBy,
        ),
  reports: counts ?? PipeReportCounts(today: 3, roomToday: 27),
);

Workspace _workspace({Pipe? pipe, String selectedPipe = ''}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/Users/me/factory')],
  agents: [
    _agent('kai', 'Kai', pipe: 'pipe1'),
    _agent('ingrid', 'Ingrid', reads: true),
    _agent('omar', 'Omar'),
    _agent('bo', 'Bo'),
    // In the workspace, outside the project.
    Agent(id: 'rivet', name: 'Rivet', role: 'Admin', modelAvailable: true),
  ],
  pipes: [pipe ?? _pipe()],
  reportSchemas: [_schemaOf],
  selectedAgentId: 'omar',
  selectedPipeId: selectedPipe,
);

PipeReport _report(
  int id,
  String title, {
  PipeReportState state = PipeReportState.PIPE_REPORT_STATE_NEW,
  String refused = '',
}) => PipeReport(
  id: Int64(id),
  pipeId: 'pipe1',
  number: id,
  state: state,
  refusedCode: refused,
  schemaId: 'bug_report',
  liaisonId: 'kai',
  recipientId: 'ingrid',
  receivedMs: Int64(DateTime.now().millisecondsSinceEpoch - 60000),
  values: [
    if (title.isNotEmpty) ReportValue(field_1: 'title', text: title),
    ReportValue(field_1: 'happened', text: 'The page went blank.'),
    ReportValue(field_1: 'steps', items: ['Open it', 'Click Save']),
    ReportValue(field_1: 'blocking', flag: true),
  ],
);

Select<String> _select(WidgetTester tester, {int index = 0}) =>
    tester.widget<Select<String>>(find.byType(Select<String>).at(index));

void main() {
  Future<Harness> show(
    WidgetTester tester,
    Workspace workspace, {
    Size size = const Size(1200, 900),
  }) async {
    final harness = Harness(workspace);
    await harness.show(tester, size: size);
    await settle(tester);
    return harness;
  }

  Future<void> open(
    WidgetTester tester,
    Widget Function(VoidCallback close) form,
  ) async {
    showModal(
      tester.element(find.text('Omar').first),
      (context, close) => form(close),
    );
    await settle(tester);
  }

  testWidgets("a pipe's settings say where its reports go: No one, the "
      'agents that read them, or another one, sent at once', (tester) async {
    final harness = await show(tester, _workspace());
    await open(
      tester,
      (close) => ForwardReportsField(pipe: _workspace().pipes.single),
    );

    expect(find.text('Forward reports to'), findsOneWidget);
    expect(
      [for (final option in _select(tester).options) option.label],
      ['No one', 'Ingrid', 'Another agent…'],
    );
    expect(_select(tester).value, 'ingrid');
    expect(
      find.text(
        'Kai collects reports from the other side and forwards each '
        'finished one to Ingrid. Reports hold the other side’s own words.',
      ),
      findsOneWidget,
    );

    // What a report holds, on asking.
    expect(find.text('What happened', findRichText: true), findsNothing);
    await tester.tap(find.text('See what a report holds'));
    await settle(tester);
    expect(find.text('What happened', findRichText: true), findsOneWidget);

    _select(tester).onChanged('');
    await settle(tester);
    final sent = harness.last.setPipeForward;
    expect(sent.pipeId, 'pipe1');
    expect(sent.recipientId, '');
    // The pipe keeps the kinds it forwards.
    expect(sent.schemaId, '');
    expect(sent.typeIds, isEmpty);
    expect(sent.makeReader, isFalse);
  });

  testWidgets('another agent reads reports only once the person confirms '
      'what that means, fresh unless they say otherwise', (tester) async {
    final harness = await show(tester, _workspace());
    await open(
      tester,
      (close) => ForwardReportsField(pipe: _workspace().pipes.single),
    );

    _select(tester).onChanged('+another');
    await settle(tester);
    // Kai is the pipe's agent, Ingrid already reads them and Bo is in a
    // pipe: only Omar.
    final pick = _select(tester, index: 1);
    expect(
      [for (final option in pick.options) option.label],
      ['Choose an agent', 'Omar'],
    );
    pick.onChanged('omar');
    await settle(tester);
    expect(find.text('Let Omar read reports?'), findsOneWidget);
    expect(find.textContaining('can no longer run commands'), findsOneWidget);
    final sentBefore = harness.backend.sent.length;

    // Not fresh this time.
    await tester.tap(find.text('Start Omar’s conversation fresh'));
    await settle(tester);
    await tester.tap(find.text('Let Omar read reports'));
    await settle(tester);
    expect(harness.backend.sent.length, sentBefore + 1);
    final sent = harness.last.setPipeForward;
    expect(sent.recipientId, 'omar');
    expect(sent.makeReader, isTrue);
    expect(sent.fresh, isFalse);
    expect(find.text('Let Omar read reports?'), findsNothing);

    // Cancel sends nothing.
    _select(tester).onChanged('+another');
    await settle(tester);
    _select(tester, index: 1).onChanged('omar');
    await settle(tester);
    await tester.tap(find.text('Cancel'));
    await settle(tester);
    expect(harness.backend.sent.length, sentBefore + 1);
  });

  testWidgets("Rivet's change says so under the choice", (tester) async {
    await show(tester, _workspace(pipe: _pipe(setBy: 'rivet')));
    await open(
      tester,
      (close) => ForwardReportsField(pipe: _workspace().pipes.single),
    );
    expect(
      find.text('Rivet set this pipe’s reports to go to Ingrid'),
      findsOneWidget,
    );
  });

  testWidgets("a project's settings say where its new pipes' reports go", (
    tester,
  ) async {
    final workspace = _workspace()
      ..projects.single.reportForward = PipeForward(
        recipientId: 'ingrid',
        schemaId: 'bug_report',
      );
    final harness = await show(tester, workspace);
    await open(
      tester,
      (close) =>
          ProjectSettingsForm(project: workspace.projects.single, close: close),
    );
    await tester.ensureVisible(find.text('New pipes forward reports to'));
    await settle(tester);
    expect(
      find.text('Each new pipe forwards its reports to Ingrid from the start.'),
      findsOneWidget,
    );
    final select = tester.widget<Select<String>>(
      find.descendant(
        of: find.byType(ForwardReportsField),
        matching: find.byType(Select<String>),
      ),
    );
    select.onChanged('');
    await settle(tester);
    final sent = harness.last.setPipeForward;
    expect(sent.projectId, 'p1');
    expect(sent.pipeId, '');
  });

  testWidgets('an open pipe shows what it forwarded and what waits, with '
      "Send and Discard; a report opens in the other side's words", (
    tester,
  ) async {
    final pipe = _pipe(
      counts: PipeReportCounts(
        today: 3,
        roomToday: 27,
        refusedToday: 1,
        overLimitToday: 2,
        waiting: 2,
      ),
    );
    final harness = Harness(_workspace(pipe: pipe, selectedPipe: 'pipe1'))
      ..backend.pipeReports = [
        _report(3, 'Save button does nothing'),
        _report(
          2,
          '',
          state: PipeReportState.PIPE_REPORT_STATE_REFUSED,
          refused: 'too_long',
        ),
        _report(
          1,
          'Login loops',
          state: PipeReportState.PIPE_REPORT_STATE_FIXED,
        )..fixedIn = '1.4.2',
      ];
    await harness.show(tester, size: const Size(1200, 900));
    harness.backend.pipeChat.load(PipeTranscript(pipeId: 'pipe1'));
    await settle(tester);
    await settle(tester);

    expect(find.byType(PipeReportsStrip), findsOneWidget);
    expect(
      find.text(
        'Forwarded  to Ingrid · 3 today, room for 27 more',
        findRichText: true,
      ),
      findsOneWidget,
    );
    // A refused report has no number to show.
    expect(find.textContaining('#0', findRichText: true), findsNothing);
    expect(
      find.text('2 reports are waiting because where they go changed.'),
      findsOneWidget,
    );
    expect(
      find.text('2 reports over today’s limit weren’t forwarded.'),
      findsOneWidget,
    );
    expect(
      find.textContaining('Save button does nothing', findRichText: true),
      findsOneWidget,
    );
    expect(
      find.textContaining('Not forwarded: too long', findRichText: true),
      findsOneWidget,
    );
    expect(
      find.textContaining('Fixed in v1.4.2', findRichText: true),
      findsOneWidget,
    );

    await tester.tap(find.text('Send to Ingrid'));
    await settle(tester);
    expect(harness.last.decidePipeReports.pipeId, 'pipe1');
    expect(harness.last.decidePipeReports.send, isTrue);
    await tester.tap(find.text('Discard'));
    await settle(tester);
    expect(harness.last.decidePipeReports.send, isFalse);

    await tester.tap(
      find.textContaining('Save button does nothing', findRichText: true),
    );
    await settle(tester);
    expect(
      find.text('Bug report · Written by the other side, through Kai.'),
      findsOneWidget,
    );
    expect(find.text('The page went blank.'), findsOneWidget);
    expect(find.text('1. Open it\n2. Click Save'), findsOneWidget);
    expect(find.text('Yes'), findsOneWidget);
  });

  testWidgets("a report's attachments show by Claktory's name and size; an "
      'image is read through Claktory, any other file never', (tester) async {
    final harness = await show(tester, _workspace());
    final report = _report(4, 'Broken chart')
      ..attachments.addAll([
        ReportAttachment(
          name: 'chart.png',
          size: Int64(2048),
          image: true,
          path: '/home/pipes/reports/pipe1/a1',
        ),
        ReportAttachment(
          name: 'crash log.txt',
          size: Int64(512),
          path: '/home/pipes/reports/pipe1/a2',
        ),
      ]);
    showReport(tester.element(find.text('Omar').first), report);
    await settle(tester);

    expect(find.text('Attachments'), findsOneWidget);
    expect(find.text('chart.png'), findsOneWidget);
    expect(find.text('crash log.txt'), findsOneWidget);
    final reads = [
      for (final message in harness.backend.sent)
        if (message.hasReadFile()) message.readFile.path,
    ];
    expect(reads, ['/home/pipes/reports/pipe1/a1']);
  });

  testWidgets("a reader's mark opens its Reports, by pipe, with no title "
      'past those it has', (tester) async {
    final harness = Harness(_workspace()..selectedAgentId = 'ingrid')
      ..backend.pipeReports = [_report(1, 'Login loops')];
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);

    expect(find.byType(ReadsReportsMark), findsOneWidget);
    await tester.tap(find.byType(ReadsReportsMark));
    await settle(tester);
    await settle(tester);
    expect(find.text('Reports'), findsWidgets);
    expect(find.text('Ingrid · Reads reports from 1 pipe'), findsOneWidget);
    expect(find.text('Acme launch'), findsWidgets);
    expect(
      find.textContaining('Login loops', findRichText: true),
      findsOneWidget,
    );
    expect(harness.last.listPipeReports.recipientId, 'ingrid');
  });

  testWidgets("a reader's settings say what reading reports keeps it from; "
      'it stops only once no pipe forwards to it, and after asking', (
    tester,
  ) async {
    final workspace = _workspace();
    final harness = await show(tester, workspace, size: const Size(1200, 1600));
    final ingrid = workspace.agents.firstWhere((agent) => agent.id == 'ingrid');
    await open(
      tester,
      (close) => AgentSettingsForm(agent: ingrid, close: close),
    );
    const note =
        "Ingrid reads reports from 1 pipe, so it can’t run commands, use the "
        'web or secrets, or message other agents.';
    await tester.ensureVisible(find.text(note));
    await settle(tester);
    await tester.ensureVisible(find.text('Stop reading reports…'));
    await settle(tester);
    await tester.tap(find.text('Stop reading reports…'));
    await settle(tester);
    expect(
      find.text(
        'Ingrid still gets reports from Acme launch. Choose another agent '
        'for them in each pipe’s settings first.',
      ),
      findsOneWidget,
    );
    expect(find.text('Stop Ingrid reading reports?'), findsNothing);

    // The pipe forwards elsewhere now: it asks, fresh unless untold.
    harness.backend.workspace.apply(_workspace(pipe: _pipe(to: '')));
    await settle(tester);
    expect(
      find.text(
        'Ingrid reads reports, so it can’t run commands, use the web or '
        'secrets, or message other agents.',
      ),
      findsOneWidget,
    );
    await tester.ensureVisible(find.text('Stop reading reports…'));
    await settle(tester);
    await tester.tap(find.text('Stop reading reports…'));
    await settle(tester);
    expect(find.text('Stop Ingrid reading reports?'), findsOneWidget);
    expect(
      find.text(
        'The words it has read then can’t steer what it does next. The old '
        'conversation is kept.',
      ),
      findsOneWidget,
    );
    final before = harness.backend.sent.length;
    await tester.ensureVisible(find.text('Start Ingrid’s conversation fresh'));
    await settle(tester);
    await tester.tap(find.text('Start Ingrid’s conversation fresh'));
    await settle(tester);
    expect(
      find.text(
        'Its conversation keeps the reports’ words, so each use of a secret '
        'asks you first until you start it fresh.',
      ),
      findsOneWidget,
    );
    await tester.ensureVisible(find.text('Stop reading reports'));
    await settle(tester);
    await tester.tap(find.text('Stop reading reports'));
    await settle(tester);
    expect(harness.backend.sent.length, before + 1);
    final sent = harness.last.setReportReader;
    expect(sent.agentId, 'ingrid');
    expect(sent.reads, isFalse);
    expect(sent.fresh, isFalse);
  });

  testWidgets("a pipe's settings set its liaison's daily spending limit, "
      'in dollars, with Save', (tester) async {
    final workspace = _workspace();
    workspace.agents.first.spend = ContainedSpend(
      spentUsd: 3.2,
      capUsd: 10,
      capTokens: Int64(2000000),
    );
    workspace.pipes.single.limits = PipeLimits(
      messagesPerHour: 30,
      burst: 10,
      fileMbPerDay: 50,
    );
    final harness = await show(tester, workspace, size: const Size(1200, 1600));
    await open(
      tester,
      (close) => PipeSettingsForm(pipeId: 'pipe1', close: close),
    );
    const note =
        'Kai has used about \$3.20 today. Past the limit, messages from the '
        'other side wait until midnight; yours still reach Kai.';
    await tester.ensureVisible(find.text(note));
    await settle(tester);
    expect(find.text('Daily spending limit'), findsOneWidget);
    // The field holds the limit as it is, after the pipe's limits (whose
    // In a row is 10 too).
    final field = find
        .byWidgetPredicate(
          (widget) => widget is EditableText && widget.controller.text == '10',
        )
        .last;
    await tester.enterText(field, '2.5');
    await tester.pump();
    await tester.tap(find.text('Save'));
    await settle(tester);
    final cap = harness.backend.sent.firstWhere((m) => m.hasSetSpendCap());
    expect(cap.setSpendCap.agentId, 'kai');
    expect(cap.setSpendCap.capUsd, 2.5);
    // Its token cap kept as it was.
    expect(cap.setSpendCap.capTokens.toInt(), 2000000);
    expect(harness.last.hasUpdatePipe(), isTrue);
  });

  testWidgets("on a phone at the largest text, the strip's counts wrap and "
      "each report's state shows on its own line", (tester) async {
    final harness = Harness(
      _workspace(
        pipe: _pipe(
          counts: PipeReportCounts(today: 3, roomToday: 27, refusedToday: 1),
        ),
        selectedPipe: 'pipe1',
      ),
    )..backend.pipeReports = [_report(3, 'Save button does nothing')];
    tester.platformDispatcher.textScaleFactorTestValue = 2.5;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    await harness.show(tester, size: const Size(375, 812));
    harness.backend.pipeChat.load(PipeTranscript(pipeId: 'pipe1'));
    await settle(tester);
    await settle(tester);
    // No overflow (it would fail the test), and the counts are there.
    expect(
      find.textContaining('room for 27 more', findRichText: true),
      findsOneWidget,
    );
    expect(find.textContaining('\nNew', findRichText: true), findsOneWidget);
    // Show all under them, so "Forwarded" keeps its whole width (Zucchero,
    // row 58: it broke mid-word beside it).
    final header = find.textContaining('room for 27 more', findRichText: true);
    expect(
      tester.getTopLeft(find.text('Show all')).dy,
      greaterThanOrEqualTo(tester.getBottomLeft(header).dy),
    );
  });

  testWidgets("a reader's Reports panel opens its reports, and its extension "
      'bar and settings leave out what it can no longer use', (tester) async {
    final workspace = _workspace()..selectedAgentId = 'ingrid';
    workspace.extensions.addAll([
      Extension(id: 'handoff', name: 'Reports', icon: 'inbox'),
      Extension(id: 'web', name: 'Web', icon: 'globe'),
      Extension(id: 'database', name: 'Database', icon: 'database'),
    ]);
    final ingrid = workspace.agents.firstWhere((a) => a.id == 'ingrid');
    for (final id in ['handoff', 'web', 'database']) {
      ingrid.extensions.add(AgentExtension(id: id, enabled: true));
    }
    expect(
      [
        for (final extension in enabledExtensions(workspace, ingrid))
          extension.id,
      ],
      ['handoff', 'database'],
    );

    final harness = await show(tester, workspace);
    harness.backend.pipeReports = [_report(1, 'Login loops')];
    await open(
      tester,
      (close) => SizedBox(
        height: 300,
        child: ReportsPanel(
          extension: workspace.extensions.first,
          agent: ingrid,
        ),
      ),
    );
    await settle(tester);
    expect(find.text('Reads reports from 1 pipe'), findsWidgets);
    await tester.tap(find.text('Show reports'));
    await settle(tester);
    await settle(tester);
    expect(
      find.textContaining('Login loops', findRichText: true),
      findsOneWidget,
    );
  });

  test('where a report stands, and why one was not forwarded', () {
    String state(PipeReport report) =>
        reportStateText(report, schema: _schemaOf);
    expect(state(_report(1, 'x')), 'New');
    expect(
      state(
        _report(1, 'x', state: PipeReportState.PIPE_REPORT_STATE_KNOWN)
          ..alias = 'A3',
      ),
      'Already known as A3',
    );
    expect(
      state(
        _report(1, 'x', state: PipeReportState.PIPE_REPORT_STATE_NEEDS_INFO)
          ..needsField = 'steps',
      ),
      'Needs more: steps',
    );
    for (final (code, field, text) in [
      ('too_long', '', 'Not forwarded: too long'),
      ('too_long', 'what happened', 'Not forwarded: what happened too long'),
      ('missing', 'what happened', 'Not forwarded: missing what happened'),
      ('over_limit', '', 'Not forwarded: over today’s limit'),
      ('secret', '', 'Not forwarded: it held a secret'),
      ('too_large', '', 'Not forwarded: an attachment over 2 MB'),
      ('type', '', 'Not forwarded: this pipe doesn’t forward this report type'),
      ('out_of_range', 'votes', 'Not forwarded: votes out of range'),
      ('date', 'due', 'Not forwarded: due not a real day'),
      ('nonsense', '', 'Not forwarded'),
    ]) {
      expect(reportRefusedText(code, field: field), text);
    }
    expect(
      state(
        _report(1, 'x', state: PipeReportState.PIPE_REPORT_STATE_REFUSED)
          ..refusedCode = 'too_many'
          ..refusedField = 'steps',
      ),
      'Not forwarded: too many steps',
    );
    expect(readsReportsText(0), 'Reads reports');
    expect(readsReportsText(3), 'Reads reports from 3 pipes');
  });

  test("a pipe's approval says where its reports will go: its own, or its "
      "project's for new pipes", () {
    final workspace = _workspace();
    expect(
      forwardApprovalLine(workspace, workspace.pipes.single),
      'Kai forwards this pipe’s reports to Ingrid.',
    );
    final fresh = Pipe(id: 'new', projectId: 'p1');
    expect(forwardApprovalLine(workspace, fresh), isNull);
    workspace.projects.single.reportForward = PipeForward(
      recipientId: 'ingrid',
    );
    expect(
      forwardApprovalLine(workspace, fresh, liaison: 'Bea'),
      'Bea forwards this pipe’s reports to Ingrid.',
    );
  });

  test("the Map draws a line from a pipe's pad to the agent reading its "
      'reports, and says on hover what went', () {
    const box = Rect.fromLTWH(0, 0, 400, 300);
    final ingrid = MapChip(
      node: MapNode(id: 'agent:ingrid', kind: MapNodeKind.MAP_NODE_KIND_AGENT),
      rect: const Rect.fromLTWH(40, 200, 140, 36),
    );
    final layout = MapLayout(
      size: const Size(400, 300),
      boxes: const [
        MapBox(
          id: 'project:p1',
          kind: MapBoxKind.project,
          rect: box,
          label: 'Factory',
        ),
      ],
      chips: [ingrid],
      wires: const [],
      dots: const [],
      related: const {},
      reads: const {},
      writes: const {},
    );
    final pads = portalPads(layout, [_pipe()]);
    final arrows = reportArrows(layout, pads);
    expect(arrows, hasLength(1));
    final arrow = arrows.single;
    expect(arrow.to, 'agent:ingrid');
    // From under the pad, down the box's inner edge, along the gap above
    // Ingrid's row and into her top, the arrowhead pointing her way.
    expect(
      arrow.points.first,
      Offset(pads.single.rect.center.dx, pads.single.rect.bottom),
    );
    expect(arrow.points[2].dx, box.right - mapCell * 1.5);
    expect(arrow.points.last.dy, ingrid.rect.top);
    expect(
      arrow.points.last.dx,
      inInclusiveRange(ingrid.rect.left, ingrid.rect.right),
    );
    expect(arrow.points[4].dy, lessThan(ingrid.rect.top));
    expect(arrow.arrow.direction, const Offset(-1, 0));

    // None for a pipe that forwards nothing, or to an agent not shown.
    expect(reportArrows(layout, portalPads(layout, [_pipe(to: '')])), isEmpty);
    expect(
      reportArrows(layout, portalPads(layout, [_pipe(to: 'gone')])),
      isEmpty,
    );

    final (text, refused) = mapReportsText(
      _workspace(),
      _pipe(counts: PipeReportCounts(today: 3, roomToday: 27, refusedToday: 2)),
    );
    expect(text, 'Reports from Kai to Ingrid: 3 today, room for 27 more');
    expect(refused, 2);
  });

  testWidgets('History shows who changed where reports go: Rivet, or you', (
    tester,
  ) async {
    final now = DateTime.now().millisecondsSinceEpoch;
    final harness = Harness(_workspace())
      ..backend.needsYouHistory = [
        NeedsYouRecord(
          kind: NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_PIPE_FORWARD,
          outcome: NeedsYouOutcome.NEEDS_YOU_OUTCOME_CHANGED,
          agentId: 'rivet',
          agentName: 'Rivet',
          pipeId: 'pipe1',
          title: 'Rivet set this pipe’s reports to go to Ingrid',
          askedMs: Int64(now - 7200000),
          decidedMs: Int64(now - 3600000),
        ),
        NeedsYouRecord(
          kind: NeedsYouRecordKind.NEEDS_YOU_RECORD_KIND_PIPE_FORWARD,
          outcome: NeedsYouOutcome.NEEDS_YOU_OUTCOME_CHANGED,
          title: 'New pipes in Factory forward reports to Ingrid',
          askedMs: Int64(now - 7200000),
          decidedMs: Int64(now - 3600000),
        ),
      ];
    await harness.show(tester, size: const Size(1200, 900));
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await tester.tap(find.text('History'));
    await settle(tester);
    await settle(tester);

    expect(find.byType(HistoryRow), findsNWidgets(2));
    expect(
      find.textContaining('Rivet set this pipe’s reports', findRichText: true),
      findsOneWidget,
    );
    final yours = find.ancestor(
      of: find.textContaining('New pipes in Factory', findRichText: true),
      matching: find.byType(HistoryRow),
    );
    expect(
      find.descendant(
        of: yours,
        matching: find.textContaining(RegExp(r'^You\b'), findRichText: true),
      ),
      findsOneWidget,
    );
  });
}
