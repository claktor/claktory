// The iPhone conversation header's pipe marks (mobile/agent_marks.dart):
// "Limited to the pipe with …" and "Reads reports from N pipes" under the
// agent's name, as the Mac's header shows them beside it; the second opens
// its Reports. Whole at the largest text sizes, and a finger's size.
import 'package:factory_app/agent_view.dart';
import 'package:factory_app/mobile/agent_marks.dart';
import 'package:factory_app/mobile/needs_you_button.dart';
import 'package:factory_app/mobile/header.dart' show PhoneHeader;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Agent _agent(
  String id,
  String name, {
  bool limited = false,
  bool reads = false,
}) => Agent(
  id: id,
  name: name,
  role: 'Liaison',
  projectId: 'p1',
  modelAvailable: true,
  status: AgentStatus.AGENT_STATUS_IDLE,
  pipeId: limited ? 'pipe1' : '',
  quarantined: limited,
  readsReports: reads,
);

Workspace _workspace(String selected) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory')],
  agents: [
    _agent('kai', 'Kai', limited: true),
    _agent('ingrid', 'Ingrid', reads: true),
    // Both at once: limited to a pipe, and reading another's reports.
    _agent('bo', 'Bo', limited: true, reads: true),
    _agent('omar', 'Omar'),
  ],
  pipes: [
    Pipe(
      id: 'pipe1',
      name: 'Acme launch',
      projectId: 'p1',
      state: PipeState.PIPE_STATE_OPEN,
      createdMs: Int64(1000),
      peer: PipePeer(claktoryName: "Ana's Claktory", ownerName: 'Ana'),
      memberIds: ['kai', 'bo'],
      quarantinedIds: ['kai', 'bo'],
      forward: PipeForward(
        recipientId: 'ingrid',
        schemaId: 'bug_report',
        revision: 1,
      ),
      reports: PipeReportCounts(today: 1, roomToday: 29),
    ),
    Pipe(
      id: 'pipe2',
      name: 'Beta',
      projectId: 'p1',
      state: PipeState.PIPE_STATE_OPEN,
      createdMs: Int64(2000),
      peer: PipePeer(claktoryName: "Lu's Claktory", ownerName: 'Lu'),
      memberIds: ['omar'],
      forward: PipeForward(
        recipientId: 'bo',
        schemaId: 'bug_report',
        revision: 1,
      ),
    ),
  ],
  reportSchemas: [ReportSchema(id: 'bug_report', name: 'Bug report')],
  selectedAgentId: selected,
);

const _phone = Size(390, 844);
const _se = Size(375, 667);

Future<Harness> _show(
  WidgetTester tester,
  String agent, {
  Size size = _phone,
  double scale = 1,
}) async {
  tester.platformDispatcher.textScaleFactorTestValue = scale;
  addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
  final harness = Harness(_workspace(agent));
  await harness.show(tester, size: size);
  await settle(tester);
  return harness;
}

Finder _inHeader(Finder finder) =>
    find.descendant(of: find.byType(AgentView), matching: finder);

/// Whether every line of [text] shows: nothing cut off or ellipsized.
void _expectWhole(WidgetTester tester, String text) {
  final paragraph = tester.renderObject<RenderParagraph>(
    _inHeader(find.text(text)),
  );
  expect(paragraph.didExceedMaxLines, isFalse, reason: text);
  expect(
    paragraph.size.width,
    lessThanOrEqualTo(paragraph.constraints.maxWidth),
  );
}

void main() {
  testWidgets('an agent limited to a pipe says so under its name', (
    tester,
  ) async {
    await _show(tester, 'kai');
    final mark = _inHeader(
      find.text("Limited to the pipe with Ana's Claktory"),
    );
    expect(mark, findsOneWidget);
    // Under the name, not beside it.
    expect(
      tester.getTopLeft(mark).dy,
      greaterThan(tester.getBottomLeft(_inHeader(find.text('Kai'))).dy),
    );
    expect(_inHeader(find.textContaining('Reads reports')), findsNothing);
    // A label for VoiceOver, not a button: on the Mac it opens nothing too.
    expect(
      tester.getSemantics(
        find.bySemanticsLabel("Limited to the pipe with Ana's Claktory"),
      ),
      isNot(matchesSemantics(isButton: true)),
    );
  });

  testWidgets("a reader's mark opens its Reports and is a finger's size", (
    tester,
  ) async {
    await _show(tester, 'ingrid');
    final mark = find.bySemanticsLabel('Reads reports from 1 pipe');
    expect(mark, findsOneWidget);
    expect(_inHeader(find.textContaining('Limited to')), findsNothing);
    expect(tester.getSize(mark).height, greaterThanOrEqualTo(44));
    expect(tester.getSize(mark).width, greaterThanOrEqualTo(44));

    await tester.tap(mark);
    await settle(tester);
    await settle(tester);
    expect(find.text('Ingrid · Reads reports from 1 pipe'), findsOneWidget);
  });

  testWidgets('quiet copies and waiting messages add nothing to the header: '
      'it keeps its height on an iPhone SE (Matt)', (tester) async {
    Future<double> header({bool quiet = false}) async {
      final workspace = _workspace('omar');
      workspace.extensions.add(Extension(id: 'agents', name: 'Messaging'));
      if (quiet) {
        workspace.agents[3]
          ..unreadCopies = 2
          ..quietWaiting = 1
          ..waitingSeqs.add(Int64(4))
          ..extensions.add(AgentExtension(id: 'agents', enabled: true));
      }
      await Harness(workspace).show(tester, size: _se);
      await settle(tester);
      expect(tester.takeException(), isNull);
      return tester.getSize(find.byType(PhoneHeader)).height;
    }

    final plain = await header();
    final quiet = await header(quiet: true);
    expect(find.byType(PhoneAgentMarks), findsNothing);
    expect(_inHeader(find.textContaining('cop')), findsNothing);
    expect(_inHeader(find.textContaining('quiet')), findsNothing);
    expect(quiet, plain);
    expect(find.byType(PhoneNeedsYouButton), findsOneWidget);
  });

  testWidgets('an agent with neither shows no second line', (tester) async {
    await _show(tester, 'omar');
    expect(find.byType(PhoneAgentMarks), findsNothing);
  });

  for (final (name, size) in [('iPhone SE', _se), ('iPhone 16', _phone)]) {
    testWidgets('both marks stay whole at the largest text size on $name', (
      tester,
    ) async {
      await _show(tester, 'bo', size: size, scale: 3.1);
      expect(tester.takeException(), isNull);
      _expectWhole(tester, "Limited to the pipe with Ana's Claktory");
      _expectWhole(tester, 'Reads reports from 1 pipe');
      final reports = find.bySemanticsLabel('Reads reports from 1 pipe');
      expect(tester.getSize(reports).height, greaterThanOrEqualTo(44));
      // Inside the screen.
      expect(tester.getRect(reports).right, lessThanOrEqualTo(size.width));
    });
  }
}
