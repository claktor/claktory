// Automated pipes in the app (pipes-automation.md §7–§8; H5 and H6, on
// Cioccolata's contract; Trish's words): where a pipe's finished reports
// go ("Forward reports to"), a project's default for new pipes, letting an
// agent read reports (it then keeps only its database, Ask and the report
// tools), and what was forwarded: the pipe's "Forwarded" strip, a report's
// own words, and a reader's "Reports". Everything is the backend's; the
// person reading reports here taints no agent (the backend's guarantee).
// On screen it's never a hand-off, record, curator, schema or taint.

import 'dart:async';
import 'dart:typed_data';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'forms.dart' show FormFrame;
import 'identity.dart';
import 'mobile/drawer.dart' show largeText;
import 'mobile/metrics.dart';
import 'pipe_timeline.dart' show pipeLiaison;
import 'pipes.dart' show PortalGlyph;
import 'report_types.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'resources.dart' show formatBytes;
import 'scope.dart';
import 'selection.dart' show SelectableArea;
import 'usage.dart' show formatAge;

/// Agents of [projectId] that read reports, in the sidebar's order.
List<Agent> reportReaders(Workspace workspace, String projectId) => [
  for (final agent in workspace.agents)
    if (agent.readsReports && agent.projectId == projectId) agent,
];

/// Agents of [projectId] that could be let read reports: not a pipe's
/// agent or member (the backend refuses them), not limited to a pipe, not
/// one already.
List<Agent> reportCandidates(Workspace workspace, String projectId) {
  final members = {for (final pipe in workspace.pipes) ...pipe.memberIds};
  return [
    for (final agent in workspace.agents)
      if (agent.projectId == projectId &&
          !agent.readsReports &&
          !agent.quarantined &&
          agent.pipeId.isEmpty &&
          !members.contains(agent.id))
        agent,
  ];
}

/// Whether [agent] can use extension [id]: any, unless it's limited to a
/// pipe (then Ask and Reports) or reads reports (then also its database).
bool containedKeeps(Agent agent, String id) {
  if (agent.quarantined) return id == 'ask' || id == 'handoff';
  if (agent.readsReports) {
    return id == 'ask' || id == 'handoff' || id == 'database';
  }
  return true;
}

/// The pipes whose reports go to [agentId].
List<Pipe> pipesFeeding(Workspace workspace, String agentId) => [
  for (final pipe in workspace.pipes)
    if (pipe.hasForward() && pipe.forward.recipientId == agentId) pipe,
];

/// "Reads reports from 3 pipes", "…from 1 pipe"; "Reads reports" with none.
String readsReportsText(int pipes) => pipes == 0
    ? 'Reads reports'
    : 'Reads reports from $pipes ${pipes == 1 ? 'pipe' : 'pipes'}';

String _nameOf(Workspace workspace, String id, {String none = 'no one'}) {
  if (id.isEmpty) return none;
  for (final agent in workspace.agents) {
    if (agent.id == id) return agent.name;
  }
  return 'an agent that’s gone';
}

/// The value a report's title comes from: its title, or the first text
/// field of its kind ([schema]) that has words (a question's question).
ReportValue? _titleValue(PipeReport report, ReportSchema? schema) {
  ReportValue? valueOf(String field) {
    for (final value in report.values) {
      if (value.field_1 == field && value.text.trim().isNotEmpty) return value;
    }
    return null;
  }

  if (valueOf('title') case final title?) return title;
  for (final field in schema?.fields ?? const <ReportField>[]) {
    if (field.type == ReportFieldType.REPORT_FIELD_TYPE_TEXT) {
      if (valueOf(field.name) case final value?) return value;
    }
  }
  return null;
}

/// A report's title: its own, or its kind's first text field's first
/// line, or its number.
String reportTitle(PipeReport report, {ReportSchema? schema}) =>
    _titleValue(report, schema)?.text.trim().split('\n').first ??
    '#${report.number}';

/// Why a report wasn't forwarded, from its code and the field it's about
/// ([field], as the person reads it; empty for none).
String reportRefusedText(String code, {String field = ''}) {
  final reason = switch (code) {
    'missing' => field.isEmpty ? 'something missing' : 'missing $field',
    'too_long' => field.isEmpty ? 'too long' : '$field too long',
    'too_many' => field.isEmpty ? 'too many items' : 'too many $field',
    'invalid' => field.isEmpty ? 'not valid' : '$field not valid',
    'version' => 'not a version Claktory knows',
    'unknown_field' => 'something a report can’t hold',
    'secret' => 'it held a secret',
    'over_limit' => 'over today’s limit',
    'cannot_revise' => 'it changes a report that’s already been read',
    'not_kept' => 'an attachment couldn’t be kept',
    'too_large' => 'an attachment over 2 MB',
    'no_forward' => 'this pipe forwards reports to no one',
    'type' => 'this pipe doesn’t forward this report type',
    'out_of_range' =>
      field.isEmpty ? 'a number out of range' : '$field out of range',
    'date' => field.isEmpty ? 'not a real day' : '$field not a real day',
    _ => '',
  };
  return reason.isEmpty ? 'Not forwarded' : 'Not forwarded: $reason';
}

/// Where a report stands, in the person's words.
String reportStateText(
  PipeReport report, {
  ReportSchema? schema,
}) => switch (report.state) {
  PipeReportState.PIPE_REPORT_STATE_ACCEPTED => 'Accepted',
  PipeReportState.PIPE_REPORT_STATE_DONE => 'Done',
  PipeReportState.PIPE_REPORT_STATE_DECLINED => 'Declined',
  PipeReportState.PIPE_REPORT_STATE_KNOWN =>
    report.alias.isEmpty ? 'Already known' : 'Already known as ${report.alias}',
  PipeReportState.PIPE_REPORT_STATE_FIXED =>
    report.fixedIn.isEmpty
        ? 'Fixed'
        // As the app writes versions: v0.1.16.
        : 'Fixed in ${report.fixedIn.startsWith(RegExp('[0-9]')) ? 'v' : ''}'
              '${report.fixedIn}',
  PipeReportState.PIPE_REPORT_STATE_WONT_FIX => 'Won’t fix',
  PipeReportState.PIPE_REPORT_STATE_NEEDS_INFO =>
    'Needs more: ${_fieldWord(report.needsField, schema)}',
  PipeReportState.PIPE_REPORT_STATE_REFUSED => reportRefusedText(
    report.refusedCode,
    field: report.refusedField.isEmpty
        ? ''
        : _fieldWord(report.refusedField, schema),
  ),
  PipeReportState.PIPE_REPORT_STATE_WAITING => 'Waiting for you',
  PipeReportState.PIPE_REPORT_STATE_DISCARDED => 'Discarded',
  _ => 'New',
};

/// A field's name as the person reads it, lowercase after "Needs more:".
String _fieldWord(String field, ReportSchema? schema) {
  for (final each in schema?.fields ?? const <ReportField>[]) {
    if (each.name == field && each.label.isNotEmpty) {
      return each.label.toLowerCase();
    }
  }
  return field.replaceAll('_', ' ');
}

/// [report]'s kind at the version it was written in, from [older] (its
/// page's definitions of versions no longer current) or the current one.
ReportSchema? _schemaOf(
  Workspace workspace,
  PipeReport report, [
  Iterable<ReportSchema> older = const [],
]) => reportTypeOf(
  workspace,
  report.schemaId,
  version: report.typeVersion,
  older: older,
);

/// "Forward reports to" in a pipe's settings ([pipe]), or "New pipes
/// forward reports to" in a project's ([projectId]): No one, the agents
/// that read reports, or Another agent…, which asks before letting it read
/// them. Each choice is sent as it's made.
class ForwardReportsField extends StatefulWidget {
  const ForwardReportsField({super.key, this.pipe, this.projectId = ''})
    : assert(pipe != null || projectId != '');

  final Pipe? pipe;
  final String projectId;

  @override
  State<ForwardReportsField> createState() => _ForwardReportsFieldState();
}

class _ForwardReportsFieldState extends State<ForwardReportsField> {
  static const _another = '+another';

  var _busy = false;
  String? _error;

  /// The agent picked under "Another agent…", to confirm; '' while
  /// picking.
  String? _picking;
  var _fresh = true;
  var _showFields = false;

  String get _projectId => widget.pipe?.projectId ?? widget.projectId;

  PipeForward _forward(Workspace workspace) {
    if (widget.pipe case final pipe?) {
      for (final now in workspace.pipes) {
        if (now.id == pipe.id) return now.forward;
      }
      return pipe.forward;
    }
    for (final project in workspace.projects) {
      if (project.id == widget.projectId) return project.reportForward;
    }
    return PipeForward();
  }

  /// Sends where reports go; with no kinds named, the pipe keeps its own.
  Future<void> _send(String recipient, {bool makeReader = false}) => _set(
    SetPipeForward(
      recipientId: recipient,
      makeReader: makeReader,
      fresh: makeReader && _fresh,
    ),
  );

  /// Sends the kinds it forwards, to the same agent.
  Future<void> _sendTypes(List<String> ids) {
    final workspace = AppScope.of(context).workspace.workspace;
    return _set(
      SetPipeForward(
        recipientId: _forward(workspace).recipientId,
        // For a Claktory from before kinds were lists.
        schemaId: ids.first,
        typeIds: ids,
      ),
    );
  }

  Future<void> _set(SetPipeForward ask) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).setPipeForward(
        ask
          ..pipeId = widget.pipe?.id ?? ''
          ..projectId = widget.pipe == null ? widget.projectId : '',
      );
      if (mounted) setState(() => _picking = null);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  void _choose(String id) {
    if (id == _another) {
      setState(() {
        _picking = '';
        _fresh = true;
        _error = null;
      });
      return;
    }
    unawaited(_send(id));
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final forward = _forward(workspace);
    final to = forward.recipientId;
    final readers = reportReaders(workspace, _projectId);
    final candidates = reportCandidates(workspace, _projectId);
    final recipient = _nameOf(workspace, to);
    final liaison = widget.pipe == null
        ? null
        : pipeLiaison(workspace, widget.pipe!);
    final setBy = forward.setById;
    final caption = TextStyle(color: palette.textFaint);
    final picking = _picking;
    final kinds = forwardTypeIds(forward);
    final types = [for (final id in kinds) ?reportTypeOf(workspace, id)];
    // Kinds to choose from, where Claktory has them.
    final choosing = reportTypesAvailable(workspace);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.l),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            widget.pipe == null
                ? 'New pipes forward reports to'
                : 'Forward reports to',
            style: TextStyles.caption.copyWith(
              color: palette.textMuted,
              fontWeight: FontWeight.w500,
            ),
          ),
          const SizedBox(height: Space.xs + 2),
          Select<String>(
            value: picking != null ? _another : to,
            options: [
              const SelectOption('', 'No one'),
              for (final agent in readers) SelectOption(agent.id, agent.name),
              if (to.isNotEmpty && !readers.any((agent) => agent.id == to))
                SelectOption(to, recipient),
              if (candidates.isNotEmpty)
                const SelectOption(_another, 'Another agent…'),
            ],
            onChanged: (id) {
              if (!_busy) _choose(id);
            },
          ),
          if (picking != null)
            _LetRead(
              candidates: candidates,
              picked: picking,
              fresh: _fresh,
              busy: _busy,
              onPick: (id) => setState(() => _picking = id),
              onFresh: (on) => setState(() => _fresh = on),
              onConfirm: () => _send(picking, makeReader: true),
              onCancel: () => setState(() => _picking = null),
            ),
          if (to.isNotEmpty && picking == null) ...[
            const SizedBox(height: Space.xs + 2),
            Text(
              widget.pipe == null
                  ? 'Each new pipe forwards its reports to $recipient from '
                        'the start.'
                  : '${liaison?.name ?? 'The pipe’s agent'} collects '
                        'reports from the other side and forwards each '
                        'finished one to $recipient. Reports hold the other '
                        'side’s own words.',
              style: type.caption.copyWith(color: palette.textFaint),
            ),
          ],
          // The report types it forwards, kept while it forwards to no one
          // (so one can be turned off before it's deleted).
          if (choosing && picking == null) ...[
            const SizedBox(height: Space.s),
            Text(
              'Can forward',
              style: TextStyles.caption.copyWith(
                color: palette.textMuted,
                fontWeight: FontWeight.w500,
              ),
            ),
            const SizedBox(height: Space.xs),
            ReportTypeChecks(
              chosen: kinds,
              busy: _busy,
              onChanged: (ids) => unawaited(_sendTypes(ids)),
            ),
          ],
          if (to.isNotEmpty && picking == null) ...[
            if (types.isNotEmpty) ...[
              const SizedBox(height: Space.xs),
              Align(
                alignment: Alignment.centerLeft,
                child: Pressable(
                  onTap: () => setState(() => _showFields = !_showFields),
                  builder: (context, hovered) => Text(
                    'See what a report holds',
                    style: type.caption.copyWith(
                      color: hovered ? palette.text : palette.signal,
                    ),
                  ),
                ),
              ),
              if (_showFields)
                Padding(
                  padding: const EdgeInsets.only(top: Space.xs),
                  child: _KindsFields(types: types),
                ),
            ],
          ],
          if (setBy.isNotEmpty && picking == null) ...[
            const SizedBox(height: Space.xs),
            Text(
              '${_nameOf(workspace, setBy, none: 'Rivet')} set '
              '${widget.pipe == null ? 'new pipes’' : 'this pipe’s'} reports '
              'to go to $recipient',
              style: type.caption.merge(caption),
            ),
          ],
          if (_error case final error?) ...[
            const SizedBox(height: Space.xs),
            Text(error, style: type.caption.copyWith(color: palette.danger)),
          ],
        ],
      ),
    );
  }
}

/// "Another agent…": which one, then what letting it read reports means,
/// with "Start Ingrid's conversation fresh", and Let Ingrid read reports.
class _LetRead extends StatelessWidget {
  const _LetRead({
    required this.candidates,
    required this.picked,
    required this.fresh,
    required this.busy,
    required this.onPick,
    required this.onFresh,
    required this.onConfirm,
    required this.onCancel,
  });

  final List<Agent> candidates;

  /// '' until one is picked.
  final String picked;
  final bool fresh;
  final bool busy;
  final ValueChanged<String> onPick;
  final ValueChanged<bool> onFresh;
  final VoidCallback onConfirm;
  final VoidCallback onCancel;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final agent = candidates.where((each) => each.id == picked).firstOrNull;
    final name = agent?.name ?? '';
    return Container(
      margin: const EdgeInsets.only(top: Space.s),
      padding: const EdgeInsets.all(Space.m),
      decoration: BoxDecoration(
        border: Border.all(color: palette.hairline),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Select<String>(
            value: picked,
            options: [
              if (picked.isEmpty) const SelectOption('', 'Choose an agent'),
              for (final each in candidates) SelectOption(each.id, each.name),
            ],
            onChanged: (id) {
              if (!busy && id.isNotEmpty) onPick(id);
            },
          ),
          if (agent != null) ...[
            const SizedBox(height: Space.m),
            Text(
              'Let $name read reports?',
              style: type.emphasis.copyWith(color: palette.text),
            ),
            const SizedBox(height: Space.xs),
            Text(
              'Reports hold words from people outside your Claktory. So '
              'that nothing in them can misuse $name, '
              'it keeps its database but can no '
              'longer run commands, use the web or secrets, or message '
              'other agents.',
              style: type.body.copyWith(color: palette.textMuted),
            ),
            const SizedBox(height: Space.s),
            _FreshCheck(
              label: 'Start $name’s conversation fresh',
              note:
                  'Nothing from its earlier work can then leak into what it '
                  'writes. The old conversation is kept.',
              on: fresh,
              onChanged: busy ? null : onFresh,
            ),
          ],
          const SizedBox(height: Space.m),
          // A long name wraps the confirm under Cancel.
          Wrap(
            alignment: WrapAlignment.end,
            spacing: Space.s,
            runSpacing: Space.xs,
            children: [
              Button(label: 'Cancel', onPressed: busy ? null : onCancel),
              Button(
                label: agent == null
                    ? 'Let it read reports'
                    : 'Let $name read reports',
                primary: true,
                onPressed: busy || agent == null ? null : onConfirm,
              ),
            ],
          ),
        ],
      ),
    );
  }
}

/// What each kind a pipe forwards holds: its name, then each field with
/// whose words it holds, those its last edit widened flagged.
class _KindsFields extends StatelessWidget {
  const _KindsFields({required this.types});

  final List<ReportSchema> types;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (final (i, kind) in types.indexed) ...[
          if (types.length > 1)
            Padding(
              padding: EdgeInsets.only(top: i == 0 ? 0 : Space.s, bottom: 2),
              child: Text(
                kind.name,
                style: type.caption.copyWith(
                  color: palette.text,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          ReportTypeFields(type: kind, widened: {...kind.widenedFields}),
        ],
      ],
    );
  }
}

/// The line a pipe's approval says when its reports will go somewhere
/// (its own forward, or its project's default for new pipes): "Kai
/// forwards this pipe's reports to Ingrid."; null when they won't.
/// [liaison] is the name the liaison will have, when it's made there.
String? forwardApprovalLine(
  Workspace workspace,
  Pipe pipe, {
  String liaison = '',
}) {
  var to = pipe.hasForward() ? pipe.forward.recipientId : '';
  if (to.isEmpty) {
    for (final project in workspace.projects) {
      if (project.id == pipe.projectId) to = project.reportForward.recipientId;
    }
  }
  if (to.isEmpty) return null;
  final name = liaison.trim().isNotEmpty
      ? liaison.trim()
      : pipeLiaison(workspace, pipe)?.name ?? 'The pipe’s agent';
  return '$name forwards this pipe’s reports to '
      '${_nameOf(workspace, to)}.';
}

/// What the Map's arrow from a pipe to its reports' reader says on hover:
/// "Reports from Kai to Ingrid: 3 today, room for 27 more", and how many
/// weren't forwarded today (0 for none).
(String, int) mapReportsText(Workspace workspace, Pipe pipe) {
  final from = pipeLiaison(workspace, pipe)?.name ?? pipe.name;
  final to = _nameOf(workspace, pipe.forward.recipientId);
  final counts = pipe.reports;
  return (
    'Reports from $from to $to: ${counts.today} today, room for '
        '${counts.roomToday} more',
    counts.refusedToday,
  );
}

/// The quiet counts about a pipe's reports and its agent, each a sentence:
/// over the limit, its spending cap, its held questions.
List<String> pipeReportNotes(Workspace workspace, Pipe pipe) {
  final liaison = pipeLiaison(workspace, pipe);
  final name = liaison?.name ?? 'The pipe’s agent';
  final over = pipe.reports.overLimitToday;
  return [
    if (over > 0)
      '$over ${over == 1 ? 'report' : 'reports'} over today’s limit '
          '${over == 1 ? 'wasn’t' : 'weren’t'} forwarded.',
    if (liaison != null && liaison.hasSpend() && liaison.spend.over)
      '$name reached today’s ${_dollars(liaison.spend.capUsd)} limit. It '
          'carries on tomorrow.',
    if (liaison != null && liaison.heldAsks > 0)
      '${liaison.heldAsks} more '
          '${liaison.heldAsks == 1 ? 'question' : 'questions'} from $name '
          '${liaison.heldAsks == 1 ? 'is' : 'are'} waiting.',
  ];
}

String _dollars(double usd) => usd == usd.roundToDouble()
    ? '\$${usd.round()}'
    : '\$${usd.toStringAsFixed(2)}';

/// Under a pipe's header while its reports go somewhere, or any wait or
/// were refused: "Forwarded", the newest reports with where they stand,
/// those waiting because where they go changed (Send to Ingrid, Discard),
/// the quiet counts, and Show all.
class PipeReportsStrip extends StatefulWidget {
  const PipeReportsStrip({super.key, required this.pipe});

  final Pipe pipe;

  @override
  State<PipeReportsStrip> createState() => _PipeReportsStripState();
}

class _PipeReportsStripState extends State<PipeReportsStrip> {
  List<PipeReport> _latest = const [];

  /// The kinds' versions [_latest] were written in, no longer current.
  List<ReportSchema> _older = const [];
  var _busy = false;
  String? _error;
  int _asked = 0;

  bool get _shown {
    final pipe = widget.pipe;
    final counts = pipe.reports;
    return (pipe.hasForward() && pipe.forward.recipientId.isNotEmpty) ||
        counts.today > 0 ||
        counts.refusedToday > 0 ||
        counts.waiting > 0;
  }

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _load());
  }

  @override
  void didUpdateWidget(PipeReportsStrip old) {
    super.didUpdateWidget(old);
    if (old.pipe.id != widget.pipe.id ||
        old.pipe.reports != widget.pipe.reports ||
        old.pipe.forward.revision != widget.pipe.forward.revision) {
      _load();
    }
  }

  Future<void> _load() async {
    if (!mounted || !_shown) return;
    final asked = ++_asked;
    try {
      final page = await AppScope.of(context)
          .listPipeReports(ListPipeReports(pipeId: widget.pipe.id, limit: 3));
      if (mounted && asked == _asked) {
        setState(() {
          _latest = page.reports;
          _older = page.schemas;
          _error = null;
        });
      }
    } on RequestFailure catch (failure) {
      if (mounted && asked == _asked) setState(() => _error = failure.message);
    }
  }

  Future<void> _decide({required bool send}) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).decidePipeReports(widget.pipe.id, send: send);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    if (!_shown) return const SizedBox.shrink();
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final pipe = widget.pipe;
    final counts = pipe.reports;
    final recipient = pipe.forward.recipientId;
    final notes = pipeReportNotes(workspace, pipe);
    final header = Text.rich(
      TextSpan(
        children: [
          TextSpan(
            text: 'Forwarded',
            style: TextStyle(
              color: palette.textMuted,
              fontWeight: FontWeight.w600,
            ),
          ),
          if (recipient.isNotEmpty)
            TextSpan(
              text:
                  '  to ${_nameOf(workspace, recipient)} · '
                  '${counts.today} today, room for '
                  '${counts.roomToday} more',
              style: TextStyle(color: palette.textFaint),
            ),
        ],
      ),
      style: type.caption,
    );
    final showAll = Button(
      label: 'Show all',
      onPressed: () => showReports(context, pipe: pipe),
    );
    return Container(
      padding: EdgeInsets.symmetric(
        horizontal: isPhoneLayout(context) ? Touch.gutter : Space.xl,
        vertical: Space.s,
      ),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: palette.hairline)),
      ),
      // Never more than a third of the screen, whatever the text size: past
      // it, the strip scrolls, and the conversation keeps its room.
      constraints: BoxConstraints(
        maxHeight: MediaQuery.sizeOf(context).height / 3,
      ),
      child: SingleChildScrollView(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // The counts wrap under "Forwarded" on a narrow screen, Show
            // all staying at the end; at an accessibility text size Show
            // all goes under them, so no word breaks.
            if (largeText(context))
              Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  header,
                  const SizedBox(height: Space.xs),
                  showAll,
                ],
              )
            else
              Row(
                children: [
                  Expanded(child: header),
                  const SizedBox(width: Space.s),
                  showAll,
                ],
              ),
            if (counts.waiting > 0)
              Padding(
                padding: const EdgeInsets.only(top: Space.xs),
                child: Wrap(
                  spacing: Space.s,
                  runSpacing: Space.xs,
                  crossAxisAlignment: WrapCrossAlignment.center,
                  children: [
                    Text(
                      '${counts.waiting} '
                      '${counts.waiting == 1 ? 'report is' : 'reports are'} '
                      'waiting because where they go changed.',
                      style: type.body.copyWith(color: palette.text),
                    ),
                    if (recipient.isNotEmpty)
                      Button(
                        label: 'Send to ${_nameOf(workspace, recipient)}',
                        primary: true,
                        onPressed: _busy ? null : () => _decide(send: true),
                      ),
                    Button(
                      label: 'Discard',
                      onPressed: _busy ? null : () => _decide(send: false),
                    ),
                  ],
                ),
              ),
            for (final report in _latest)
              PipeReportRow(
                key: ValueKey(report.id),
                report: report,
                schema: _schemaOf(workspace, report, _older),
                onOpen: () => showReport(context, report, older: _older),
              ),
            for (final note in notes)
              Padding(
                padding: const EdgeInsets.only(top: 2),
                child: Text(
                  note,
                  style: type.caption.copyWith(color: palette.textMuted),
                ),
              ),
            if (_error case final error?)
              Text(error, style: type.caption.copyWith(color: palette.danger)),
          ],
        ),
      ),
    );
  }
}

/// One report: its number and title, then where it stands (red when it
/// wasn't forwarded), and how long ago; it opens the report.
class PipeReportRow extends StatelessWidget {
  const PipeReportRow({
    super.key,
    required this.report,
    required this.onOpen,
    this.schema,
  });

  final PipeReport report;
  final ReportSchema? schema;
  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final refused = report.state == PipeReportState.PIPE_REPORT_STATE_REFUSED;
    final phone = isPhoneLayout(context);
    final state = reportStateText(report, schema: schema);
    return Pressable(
      onTap: refused ? null : onOpen,
      builder: (context, hovered) => Container(
        constraints: BoxConstraints(minHeight: phone ? Touch.target : 28),
        color: hovered && !refused ? palette.hover : null,
        padding: const EdgeInsets.symmetric(vertical: 2),
        child: Row(
          children: [
            Expanded(
              child: Text.rich(
                TextSpan(
                  children: [
                    // A refused report has no number, nor a title to show.
                    if (report.number > 0)
                      TextSpan(
                        text: '#${report.number}  ',
                        style: TextStyle(color: palette.textFaint),
                      ),
                    if (!refused)
                      TextSpan(
                        text: reportTitle(report, schema: schema),
                        style: TextStyle(color: palette.text),
                      ),
                    TextSpan(
                      // On a phone, where it stands on a line of its own.
                      text: refused ? state : '${phone ? '\n' : '  '}$state',
                      style: TextStyle(
                        color: refused ? palette.danger : palette.textMuted,
                      ),
                    ),
                  ],
                ),
                maxLines: phone ? 4 : 2,
                overflow: TextOverflow.ellipsis,
                style: type.caption,
              ),
            ),
            const SizedBox(width: Space.s),
            Text(
              formatAge(report.receivedMs.toInt(), DateTime.now()),
              style: type.caption.copyWith(color: palette.textFaint),
            ),
          ],
        ),
      ),
    );
  }
}

/// A report's own words: "Written by the other side, through Kai." over
/// each field, labelled as the schema says, then where it stands.
/// [older]: the definitions of kinds' versions no longer current, for one
/// written in an older version.
Future<void> showReport(
  BuildContext context,
  PipeReport report, {
  List<ReportSchema> older = const [],
}) => showModal(
  context,
  (context, close) => ReportView(report: report, older: older, close: close),
);

class ReportView extends StatelessWidget {
  const ReportView({
    super.key,
    required this.report,
    required this.close,
    this.older = const [],
  });

  final PipeReport report;
  final List<ReportSchema> older;
  final VoidCallback close;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final schema = _schemaOf(workspace, report, older);
    final liaison = _nameOf(
      workspace,
      report.liaisonId,
      none: 'the pipe’s agent',
    );
    final values = {for (final value in report.values) value.field_1: value};
    final fields = [
      ...?schema?.fields.where((field) => values.containsKey(field.name)),
      // A field the app's schema doesn't name still shows.
      for (final value in report.values)
        if (schema == null ||
            !schema.fields.any((field) => field.name == value.field_1))
          ReportField(name: value.field_1, label: value.field_1),
    ];
    final title = reportTitle(report, schema: schema);
    final titled = _titleValue(report, schema);
    final shownWhole = titled != null && !titled.text.trim().contains('\n')
        ? titled
        : null;
    // Its kind, with the version it was written in once there's a newer.
    final current = reportTypeOf(workspace, report.schemaId);
    final kind = schema == null
        ? ''
        : current != null &&
              report.typeVersion > 0 &&
              current.version != report.typeVersion
        ? '${schema.name} (version ${report.typeVersion}) · '
        : '${schema.name} · ';
    return FormFrame(
      title: '#${report.number}  $title',
      body: '${kind}Written by the other side, through $liaison.',
      fields: [
        for (final field in fields)
          // Not the one the title shows whole.
          if (field.name != shownWhole?.field_1)
            _ReportValue(field: field, value: values[field.name]!),
        if (report.attachments.isNotEmpty)
          _ReportAttachments(attachments: report.attachments),
        Text(
          reportStateText(report, schema: schema),
          style: type.body.copyWith(color: palette.textMuted),
        ),
      ],
      error: null,
      submitLabel: 'Done',
      onSubmit: close,
      onCancel: close,
    );
  }
}

/// One field of a report: its label, then the value as it is (a list as
/// lines, a yes or no).
class _ReportValue extends StatelessWidget {
  const _ReportValue({required this.field, required this.value});

  final ReportField field;
  final ReportValue value;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final shown = switch (field.type) {
      ReportFieldType.REPORT_FIELD_TYPE_BOOL => value.flag ? 'Yes' : 'No',
      ReportFieldType.REPORT_FIELD_TYPE_NUMBER => _number(value.number.toInt()),
      ReportFieldType.REPORT_FIELD_TYPE_DATE => reportDate(value.text),
      ReportFieldType.REPORT_FIELD_TYPE_ENUM => choiceLabel(value.text),
      ReportFieldType.REPORT_FIELD_TYPE_TEXT_LIST => [
        for (final (i, item) in value.items.indexed) '${i + 1}. $item',
      ].join('\n'),
      _ =>
        value.items.isNotEmpty && value.text.isEmpty
            ? value.items.join('\n')
            : value.text,
    };
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.m),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            field.label.isEmpty ? field.name : field.label,
            style: type.caption.copyWith(
              color: palette.textMuted,
              fontWeight: FontWeight.w500,
            ),
          ),
          const SizedBox(height: 2),
          SelectableArea(
            child: Text(
              shown.isEmpty ? '—' : shown,
              style: type.body.copyWith(color: palette.text),
            ),
          ),
        ],
      ),
    );
  }
}

/// 12,345 and -1,200, as numbers read.
String _number(int value) {
  final digits = value.abs().toString();
  final grouped = digits.replaceAllMapped(
    RegExp(r'\B(?=(\d{3})+(?!\d))'),
    (_) => ',',
  );
  return value < 0 ? '-$grouped' : grouped;
}

/// "2026-11-22" as "22 Nov 2026"; anything else as it is.
String reportDate(String text) {
  final day = RegExp(r'^(\d{4})-(\d{2})-(\d{2})$').firstMatch(text.trim());
  if (day == null) return text;
  const months = [
    'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', //
    'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
  ];
  final month = int.parse(day[2]!);
  if (month < 1 || month > 12) return text;
  return '${int.parse(day[3]!)} ${months[month - 1]} ${day[1]}';
}

/// A report's attachments, the other side's files: an image shown from
/// the bytes Claktory reads for the app, decoded here; any other file by
/// its name and size only. None is ever opened with another app, and the
/// name is Claktory's cleaned one, never the file's path.
class _ReportAttachments extends StatelessWidget {
  const _ReportAttachments({required this.attachments});

  final List<ReportAttachment> attachments;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.m),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'Attachments',
            style: type.caption.copyWith(
              color: palette.textMuted,
              fontWeight: FontWeight.w500,
            ),
          ),
          for (final attachment in attachments) ...[
            const SizedBox(height: Space.xs),
            if (attachment.image) _AttachedImage(attachment: attachment),
            Row(
              children: [
                ExtensionGlyph(
                  icon: attachment.image ? 'image' : 'paperclip',
                  name: '',
                  color: palette.textFaint,
                  size: 12,
                ),
                const SizedBox(width: Space.xs),
                Expanded(
                  child: Text(
                    attachment.name,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: type.body.copyWith(color: palette.text),
                  ),
                ),
                const SizedBox(width: Space.s),
                Text(
                  formatBytes(attachment.size),
                  style: type.caption.copyWith(color: palette.textFaint),
                ),
              ],
            ),
          ],
        ],
      ),
    );
  }
}

/// An attached image, read through Claktory and decoded in the app; while
/// it loads, or if it can't be shown, nothing but its name below.
class _AttachedImage extends StatefulWidget {
  const _AttachedImage({required this.attachment});

  final ReportAttachment attachment;

  @override
  State<_AttachedImage> createState() => _AttachedImageState();
}

/// An attached image's largest size on screen, in points.
const _imageMaxWidth = 480.0;
const _imageMaxHeight = 240.0;

class _AttachedImageState extends State<_AttachedImage> {
  Future<FileContent>? _read;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _read ??= AppScope.of(context).readFile(widget.attachment.path);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final scale = MediaQuery.devicePixelRatioOf(context);
    return FutureBuilder<FileContent>(
      future: _read,
      builder: (context, snapshot) {
        final content = snapshot.data;
        if (content == null ||
            content.error.isNotEmpty ||
            content.data.isEmpty ||
            !content.mime.startsWith('image/')) {
          return const SizedBox.shrink();
        }
        return Padding(
          padding: const EdgeInsets.only(bottom: Space.xs),
          child: Align(
            alignment: Alignment.centerLeft,
            child: Container(
              constraints: const BoxConstraints(
                maxWidth: _imageMaxWidth,
                maxHeight: _imageMaxHeight,
              ),
              decoration: BoxDecoration(
                border: Border.all(color: palette.hairline),
                borderRadius: BorderRadius.circular(Radii.control),
              ),
              clipBehavior: Clip.antiAlias,
              // Decoded no larger than it's shown, whatever size the image
              // claims: one from the other side can't take the memory.
              child: Image(
                image: ResizeImage(
                  MemoryImage(Uint8List.fromList(content.data)),
                  width: (_imageMaxWidth * scale).round(),
                  height: (_imageMaxHeight * scale).round(),
                  policy: ResizeImagePolicy.fit,
                ),
                fit: BoxFit.contain,
                semanticLabel: widget.attachment.name,
                errorBuilder: (context, _, _) => const SizedBox.shrink(),
              ),
            ),
          ),
        );
      },
    );
  }
}

/// Every report of [pipe], or every report forwarded to [reader] grouped
/// by pipe ("Reports"), newest first, a page at a time.
Future<void> showReports(BuildContext context, {Pipe? pipe, Agent? reader}) =>
    showModal(
      context,
      (context, close) => ReportsList(pipe: pipe, reader: reader, close: close),
    );

class ReportsList extends StatefulWidget {
  const ReportsList({super.key, this.pipe, this.reader, required this.close})
    : assert(pipe != null || reader != null);

  final Pipe? pipe;
  final Agent? reader;
  final VoidCallback close;

  @override
  State<ReportsList> createState() => _ReportsListState();
}

class _ReportsListState extends State<ReportsList> {
  var _reports = <PipeReport>[];

  /// The kinds' versions they were written in, no longer current.
  var _older = <ReportSchema>[];
  var _more = false;
  var _loading = true;
  String? _error;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _load());
  }

  Future<void> _load({bool earlier = false}) async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final page = await AppScope.of(context).listPipeReports(
        ListPipeReports(
          pipeId: widget.pipe?.id ?? '',
          recipientId: widget.pipe == null ? widget.reader!.id : '',
          beforeId: earlier && _reports.isNotEmpty
              ? _reports.last.id
              : Int64.ZERO,
          limit: 100,
        ),
      );
      if (!mounted) return;
      setState(() {
        _reports = earlier ? [..._reports, ...page.reports] : page.reports;
        _older = earlier ? [..._older, ...page.schemas] : page.schemas;
        _more = page.more;
      });
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _loading = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final pipeNames = {for (final pipe in workspace.pipes) pipe.id: pipe.name};
    // By pipe, in the order their newest came, for a reader.
    final groups = <String, List<PipeReport>>{};
    for (final report in _reports) {
      groups.putIfAbsent(report.pipeId, () => []).add(report);
    }
    return FormFrame(
      title: widget.pipe == null ? 'Reports' : 'Forwarded',
      body: widget.pipe == null
          ? '${widget.reader!.name} · '
                '${readsReportsText(pipesFeeding(workspace, widget.reader!.id).length)}'
          : null,
      fields: [
        if (_reports.isEmpty)
          Text(
            _loading ? 'Loading…' : 'No reports yet.',
            style: type.body.copyWith(color: palette.textFaint),
          ),
        for (final MapEntry(key: pipeId, value: reports) in groups.entries) ...[
          if (widget.pipe == null)
            Padding(
              padding: const EdgeInsets.only(top: Space.s, bottom: 2),
              child: Text(
                pipeNames[pipeId] ?? 'A pipe that’s gone',
                style: type.caption.copyWith(
                  color: palette.textMuted,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          for (final report in reports)
            PipeReportRow(
              key: ValueKey(report.id),
              report: report,
              schema: _schemaOf(workspace, report, _older),
              onOpen: () => showReport(context, report, older: _older),
            ),
        ],
        if (_more)
          Align(
            alignment: Alignment.centerLeft,
            child: Button(
              label: 'Show earlier',
              onPressed: _loading ? null : () => _load(earlier: true),
            ),
          ),
      ],
      error: _error,
      submitLabel: 'Done',
      onSubmit: widget.close,
      onCancel: widget.close,
    );
  }
}

/// Beside a reader's name: "Reads reports from 3 pipes" in a hairline
/// capsule, like "Limited to the pipe…"; it opens its Reports. Nothing for
/// an agent that doesn't read reports.
class ReadsReportsMark extends StatelessWidget {
  const ReadsReportsMark({super.key, required this.agent});

  final Agent agent;

  @override
  Widget build(BuildContext context) {
    if (!agent.readsReports) return const SizedBox.shrink();
    final palette = Palette.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final text = readsReportsText(pipesFeeding(workspace, agent.id).length);
    return Semantics(
      button: true,
      label: text,
      excludeSemantics: true,
      child: Pressable(
        onTap: () => showReports(context, reader: agent),
        builder: (context, hovered) => Container(
          padding: const EdgeInsets.symmetric(
            horizontal: Space.s - 2,
            vertical: 1,
          ),
          decoration: BoxDecoration(
            color: hovered ? palette.hover : null,
            border: Border.all(color: palette.hairline),
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              const PortalGlyph(size: 12),
              const SizedBox(width: Space.xs),
              Flexible(
                child: Text(
                  text,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyles.caption.copyWith(color: palette.textMuted),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// "A and B", "A, B and C".
String _andList(List<String> items) => items.length < 2
    ? items.join()
    : '${items.sublist(0, items.length - 1).join(', ')} and ${items.last}';

/// In a reader's settings: what reading reports keeps it from, and "Stop
/// reading reports…". While a pipe still forwards to it, a tap says which
/// and where to change that; otherwise it asks first, offering to start its
/// conversation fresh (ticked), since that conversation holds the reports'
/// words. Nothing for an agent that doesn't read reports.
class StopReadingReportsField extends StatefulWidget {
  const StopReadingReportsField({super.key, required this.agent});

  final Agent agent;

  @override
  State<StopReadingReportsField> createState() =>
      _StopReadingReportsFieldState();
}

class _StopReadingReportsFieldState extends State<StopReadingReportsField> {
  var _confirming = false;
  var _refused = false;
  var _fresh = true;
  var _busy = false;
  String? _error;

  void _start(List<Pipe> feeding) => setState(() {
    _error = null;
    _refused = feeding.isNotEmpty;
    _confirming = feeding.isEmpty;
    _fresh = true;
  });

  Future<void> _stop() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await AppScope.of(context).setReportReader(
        SetReportReader(agentId: widget.agent.id, reads: false, fresh: _fresh),
      );
      if (mounted) setState(() => _confirming = false);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    if (!widget.agent.readsReports) return const SizedBox.shrink();
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final name = widget.agent.name;
    final feeding = pipesFeeding(workspace, widget.agent.id);
    final caption = type.caption.copyWith(color: palette.textMuted);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.l),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Padding(
                padding: EdgeInsets.only(top: 2),
                child: ExcludeSemantics(child: PortalGlyph(size: 12)),
              ),
              const SizedBox(width: Space.xs + 2),
              Expanded(
                child: Text(
                  // "Ingrid reads reports from 3 pipes, so…"
                  '$name ${readsReportsText(feeding.length).toLowerCase()}, '
                  'so it can’t run commands, use the web or secrets, or '
                  'message other agents.',
                  style: caption,
                ),
              ),
            ],
          ),
          if (!_confirming) ...[
            const SizedBox(height: Space.s),
            Align(
              alignment: Alignment.centerLeft,
              child: Button(
                label: 'Stop reading reports…',
                onPressed: () => _start(feeding),
              ),
            ),
          ],
          if (_refused && feeding.isNotEmpty && !_confirming) ...[
            const SizedBox(height: Space.xs),
            Text(
              '$name still gets reports from '
              '${_andList([for (final pipe in feeding) pipe.name])}. Choose '
              'another agent for them in each pipe’s settings first.',
              style: type.caption.copyWith(color: palette.danger),
            ),
          ],
          if (_confirming)
            Container(
              margin: const EdgeInsets.only(top: Space.s),
              padding: const EdgeInsets.all(Space.m),
              decoration: BoxDecoration(
                border: Border.all(color: palette.hairline),
                borderRadius: BorderRadius.circular(Radii.panel),
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Text(
                    'Stop $name reading reports?',
                    style: type.emphasis.copyWith(color: palette.text),
                  ),
                  const SizedBox(height: Space.xs),
                  Text(
                    '$name gets back commands, the web, secrets and '
                    'messaging. Its conversation holds words from reports it '
                    'has read, which could try to misuse those.',
                    style: type.body.copyWith(color: palette.textMuted),
                  ),
                  const SizedBox(height: Space.s),
                  _FreshCheck(
                    label: 'Start $name’s conversation fresh',
                    note: _fresh
                        ? 'The words it has read then can’t steer what it '
                              'does next. The old conversation is kept.'
                        : 'Its conversation keeps the reports’ words, so each '
                              'use of a secret asks you first until you start '
                              'it fresh.',
                    on: _fresh,
                    onChanged: _busy
                        ? null
                        : (on) => setState(() => _fresh = on),
                  ),
                  if (_error case final error?) ...[
                    const SizedBox(height: Space.xs),
                    Text(
                      error,
                      style: type.caption.copyWith(color: palette.danger),
                    ),
                  ],
                  const SizedBox(height: Space.m),
                  Wrap(
                    alignment: WrapAlignment.end,
                    spacing: Space.s,
                    runSpacing: Space.xs,
                    children: [
                      Button(
                        label: 'Cancel',
                        onPressed: _busy
                            ? null
                            : () => setState(() => _confirming = false),
                      ),
                      Button(
                        label: 'Stop reading reports',
                        primary: true,
                        onPressed: _busy ? null : _stop,
                      ),
                    ],
                  ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// "Start X's conversation fresh", a tick, with what it means under it.
class _FreshCheck extends StatelessWidget {
  const _FreshCheck({
    required this.label,
    required this.note,
    required this.on,
    required this.onChanged,
  });

  final String label;
  final String note;
  final bool on;
  final ValueChanged<bool>? onChanged;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final onChanged = this.onChanged;
    return Semantics(
      checked: on,
      child: Pressable(
        onTap: onChanged == null ? null : () => onChanged(!on),
        builder: (context, hovered) => Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Padding(
              padding: const EdgeInsets.only(top: 5),
              child: CheckMark(on: on),
            ),
            const SizedBox(width: Space.s),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(label, style: type.body.copyWith(color: palette.text)),
                  Text(
                    note,
                    style: type.caption.copyWith(color: palette.textFaint),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// The Reports extension's panel: what it does, and the reports themselves
/// (a reader's from every pipe, by pipe; a pipe's agent's, its pipe's),
/// so they can be reached from the extension bar on any device.
class ReportsPanel extends StatelessWidget {
  const ReportsPanel({super.key, required this.extension, required this.agent});

  final Extension extension;
  final Agent agent;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final pipe = workspace.pipes
        .where((pipe) => pipe.id == agent.pipeId)
        .firstOrNull;
    return ListView(
      padding: const EdgeInsets.fromLTRB(Space.l, Space.m, Space.l, Space.l),
      children: [
        if (extension.description.isNotEmpty)
          Text(
            extension.description,
            style: type.body.copyWith(color: palette.textMuted),
          ),
        const SizedBox(height: Space.m),
        if (agent.readsReports) ...[
          Text(
            readsReportsText(pipesFeeding(workspace, agent.id).length),
            style: type.body.copyWith(color: palette.text),
          ),
          const SizedBox(height: Space.s),
          Align(
            alignment: Alignment.centerLeft,
            child: Button(
              label: 'Show reports',
              onPressed: () => showReports(context, reader: agent),
            ),
          ),
        ] else if (pipe != null && pipe.hasForward())
          Align(
            alignment: Alignment.centerLeft,
            child: Button(
              label: 'Show reports',
              onPressed: () => showReports(context, pipe: pipe),
            ),
          )
        else
          Text(
            'No reports go through ${agent.name}.',
            style: type.body.copyWith(color: palette.textFaint),
          ),
      ],
    );
  }
}
