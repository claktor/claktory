import 'dart:convert';

import 'package:flutter/rendering.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';
import 'package:markdown/markdown.dart' as md;
import 'package:url_launcher/url_launcher.dart';

import 'agent_name.dart';
import 'backend.dart' show fireAndLog;
import 'characters/character.dart';
import 'controls.dart';
import 'explorer.dart' show shortPath;
import 'extensions.dart' show StatusMark, statusLabel;
import 'forms.dart' show rivetIn;
import 'identity.dart';
import 'layout.dart';
import 'markdown.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'state.dart';
import 'transcript.dart' show AgentTints;

/// Compact views of extensions' tool calls in the conversation, by tool
/// name: what the call did in the app's words, with what a person scans for
/// (whom `agents_send` messaged, which items `checklist_add` added, what
/// `web_fetch` read). A tool with no view here keeps the generic row. Each
/// view reads the call's input, its output and its `details`, the JSON the
/// backend keeps with the result for the app (docs/EXTENSIONS.md); calls
/// from before details existed have none, so every view also makes do with
/// the input alone.
@immutable
class ToolView {
  const ToolView({
    required this.icon,
    required this.line,
    this.body,
    this.applies,
  });

  /// Its extension's Lucide icon, in front of [line].
  final String icon;

  /// One line: what the call did.
  final Widget Function(BuildContext context, ToolCallInfo call) line;

  /// What it did in more detail, under the line; null for nothing more.
  final Widget? Function(BuildContext context, ToolCallInfo call)? body;

  /// Whether this call gets the view; every call of the tool when null.
  final bool Function(ToolCallInfo call)? applies;
}

/// The view for [call], or null for the generic row.
ToolView? toolViewFor(ToolCallInfo call) {
  final view = toolViews[call.name];
  if (view == null) return null;
  final applies = view.applies;
  return applies == null || applies(call) ? view : null;
}

/// A tool call as its view reads it.
class ToolCallInfo {
  factory ToolCallInfo(ToolCall call, {int seq = 0}) {
    final input = _object(call.input);
    final details = _object(call.details) ?? const <String, Object?>{};
    // A tool of an extension turned on mid-context, called through
    // extension_call {tool, args}: shown as that tool.
    final inner = input?['tool'];
    if (call.name == 'extension_call' && inner is String) {
      final args = input?['args'];
      return ToolCallInfo._(
        call,
        seq,
        name: inner,
        input: args is Map<String, Object?> ? args : const {},
        details: details,
      );
    }
    return ToolCallInfo._(
      call,
      seq,
      name: call.name,
      input: input,
      details: details,
    );
  }

  ToolCallInfo._(
    this.call,
    this.seq, {
    required this.name,
    required this.input,
    required this.details,
  });

  final ToolCall call;

  /// The tool it calls: the one inside an extension_call.
  final String name;

  /// Its entry's place in the transcript.
  final int seq;

  /// Its arguments; null while the model is still writing them.
  final Map<String, Object?>? input;

  /// What the result says for the app; empty while it runs, when it failed,
  /// and for calls from before details.
  final Map<String, Object?> details;

  bool get running =>
      call.state == ToolCallState.TOOL_CALL_STATE_PENDING ||
      call.state == ToolCallState.TOOL_CALL_STATE_RUNNING;

  bool get failed => call.state == ToolCallState.TOOL_CALL_STATE_FAILED;

  bool get done => !running && !failed;

  /// The string argument [key], as far as it's written yet.
  String? argument(String key) {
    final input = this.input;
    if (input != null) {
      final value = input[key];
      return value is String ? value : value?.toString();
    }
    return partialJsonString(call.input, key);
  }

  /// The list argument [key]; empty while it's being written.
  List<Object?> list(String key) => _list(input?[key]);

  static Map<String, Object?>? _object(String json) {
    if (json.trim().isEmpty) return null;
    try {
      final decoded = jsonDecode(json);
      return decoded is Map<String, Object?> ? decoded : null;
    } on FormatException {
      return null;
    }
  }
}

List<Object?> _list(Object? value) => value is List ? value : const [];

Map<String, Object?> _map(Object? value) =>
    value is Map<String, Object?> ? value : const {};

String _string(Object? value) =>
    value is String ? value : (value == null ? '' : '$value');

int? _int(Object? value) => value is int ? value : null;

/// The views, by tool name.
final Map<String, ToolView> toolViews = {
  'agents_send': ToolView(
    icon: 'send',
    line: (context, call) => _SendLine(call),
    body: _sendBody,
  ),
  'propose_to_rivet': ToolView(
    icon: 'send',
    line: (context, call) => _ProposeLine(call),
  ),
  'checklist_add': ToolView(
    icon: 'list-checks',
    line: _checklistAddLine,
    body: _checklistAddBody,
  ),
  'checklist_update': ToolView(
    icon: 'list-checks',
    line: _checklistUpdateLine,
    body: _checklistUpdateBody,
  ),
  'memory_read': ToolView(
    icon: 'brain',
    line: _memoryReadLine,
    body: _memoryReadBody,
  ),
  'memory_update': ToolView(
    icon: 'brain',
    line: _memoryUpdateLine,
    body: _memoryUpdateBody,
  ),
  'monitors_add': ToolView(
    icon: 'alarm-clock',
    line: _monitorsAddLine,
    body: _monitorsAddBody,
  ),
  'monitors_change': ToolView(icon: 'alarm-clock', line: _monitorsChangeLine),
  'tasks_list': ToolView(
    icon: 'terminal',
    line: _tasksListLine,
    body: _tasksListBody,
  ),
  'tasks_stop': ToolView(icon: 'terminal', line: _tasksStopLine),
  // bash only once it runs in the background; otherwise the generic row.
  'bash': ToolView(
    icon: 'terminal',
    line: (context, call) => _BackgroundLine(call),
    applies: (call) => _string(call.details['task']).isNotEmpty,
  ),
  'show': ToolView(icon: 'panels-top-left', line: (_, call) => _ShowLine(call)),
  'web_search': ToolView(
    icon: 'globe',
    line: _webSearchLine,
    body: _webSearchBody,
  ),
  'web_fetch': ToolView(icon: 'globe', line: (_, call) => _FetchLine(call)),
  'db_query': ToolView(icon: 'database', line: _dbLine, body: _sqlBody),
  'db_exec': ToolView(icon: 'database', line: _dbLine, body: _sqlBody),
  'db_schema': ToolView(icon: 'database', line: _dbLine),
  'secrets_grant': ToolView(
    icon: 'key-round',
    line: (_, call) => _SecretLine(call, give: true),
  ),
  'secrets_revoke': ToolView(
    icon: 'key-round',
    line: (_, call) => _SecretLine(call, give: false),
    body: _secretRevokeBody,
  ),
  'secrets_list': ToolView(
    icon: 'key-round',
    line: _secretsListLine,
    body: _secretsListBody,
  ),
};

/// The transcript the views are in, for what happens after a call (a
/// background task's end), and whose it is.
class ToolViewScope extends InheritedWidget {
  const ToolViewScope({
    super.key,
    required this.transcript,
    required this.agentId,
    required super.child,
  });

  final TranscriptStore transcript;
  final String agentId;

  static ToolViewScope? of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<ToolViewScope>();

  @override
  bool updateShouldNotify(ToolViewScope old) =>
      old.transcript != transcript || old.agentId != agentId;
}

/// Opens web links; tests replace it.
@visibleForTesting
Future<bool> Function(Uri url) openWebLink = launchUrl;

// ---------------------------------------------------------------------------
// Pieces the views are built from.

/// A line's words: [lead] muted, then [subject] in the text color (mono
/// when it's [code]), then [after] faint, on one line.
class ToolLine extends StatelessWidget {
  const ToolLine(
    this.lead, {
    super.key,
    this.subject = '',
    this.code = false,
    this.after = '',
  });

  final String lead;
  final String subject;
  final bool code;
  final String after;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Text.rich(
      TextSpan(
        children: [
          TextSpan(text: lead),
          if (subject.isNotEmpty)
            TextSpan(
              text: lead.isEmpty ? subject : ' $subject',
              style: (code ? TextStyles.code : TextStyles.body).copyWith(
                color: palette.text,
              ),
            ),
          if (after.isNotEmpty)
            TextSpan(
              text: '  $after',
              style: TextStyles.caption.copyWith(color: palette.textFaint),
            ),
        ],
      ),
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: TextStyles.body.copyWith(color: palette.textMuted),
    );
  }
}

/// A faint caption at a line's end: "Sent", "3 rows".
class _Trailing extends StatelessWidget {
  const _Trailing(this.text, {this.color});

  final String text;
  final Color? color;

  @override
  Widget build(BuildContext context) => Text(
    text,
    maxLines: 1,
    style: TextStyles.caption.copyWith(
      color: color ?? Palette.of(context).textFaint,
    ),
  );
}

/// Text that opens something when clicked: underlined while hovered.
class _Link extends StatelessWidget {
  const _Link({required this.text, required this.onTap, this.color});

  final String text;
  final VoidCallback? onTap;
  final Color? color;

  @override
  Widget build(BuildContext context) {
    final color = this.color ?? Palette.of(context).text;
    return Pressable(
      onTap: onTap,
      builder: (context, hovered) => Text(
        text,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: TextStyles.body.copyWith(
          color: color,
          decoration: hovered && onTap != null
              ? TextDecoration.underline
              : null,
          decorationColor: color,
        ),
      ),
    );
  }
}

/// An agent by its character and name (with its role faint), opening its
/// conversation on a click when the app knows it.
class ToolAgent extends StatelessWidget {
  const ToolAgent({
    super.key,
    required this.id,
    required this.name,
    this.role,
    this.size = 16,
  });

  /// Its id, when known; empty for a name the app couldn't place.
  final String id;
  final String name;

  /// Its role then; its role now when null.
  final String? role;
  final double size;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final tints = AgentTints.of(context);
    final agent = tints?.agents[id];
    final color = tints?.byId[id];
    final open = tints?.open;
    final known = agent != null && open != null;
    final shownName = name.isNotEmpty ? name : (agent?.name ?? id);
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        if (agent != null) ...[
          AgentCharacter(agent: agent, size: size),
          const SizedBox(width: Space.xs),
        ],
        Flexible(
          child: Pressable(
            onTap: known ? () => open(id) : null,
            builder: (context, hovered) {
              final nameColor = color ?? palette.text;
              return NameThenRole(
                name: shownName,
                role: role ?? agent?.role ?? '',
                style: TextStyles.label.copyWith(
                  color: nameColor,
                  fontWeight: FontWeight.w600,
                  decoration: known && hovered
                      ? TextDecoration.underline
                      : null,
                  decorationColor: nameColor,
                ),
                roleColor: palette.textFaint,
              );
            },
          ),
        ),
      ],
    );
  }
}

/// [to], an agent's id or name as an agent wrote it, as the agent it names:
/// by id, else by name, preferring the agent in [self]'s project. Null when
/// no agent the app knows matches.
Agent? resolveAgent(BuildContext context, String to, {String self = ''}) {
  final agents = AgentTints.of(context)?.agents ?? const {};
  final wanted = to.trim();
  if (wanted.isEmpty) return null;
  if (agents[wanted] case final agent?) return agent;
  final lower = wanted.toLowerCase();
  final named = [
    for (final agent in agents.values)
      if (agent.name.toLowerCase() == lower) agent,
  ];
  if (named.length <= 1) return named.firstOrNull;
  final workspace = context
      .getInheritedWidgetOfExactType<AppScope>()
      ?.backend
      .workspace;
  final project = workspace?.agent(self)?.projectId;
  if (project == null) return null;
  final ours = [
    for (final agent in named)
      if (workspace?.agent(agent.id)?.projectId == project) agent,
  ];
  return ours.length == 1 ? ours.single : null;
}

/// Shows [child] cut to [height] with "Show more" under it when it's
/// taller, and whole after a click.
class ShowMore extends StatefulWidget {
  const ShowMore({super.key, required this.child, this.height = 76});

  final Widget child;

  /// Its height cut, in pixels: four lines of body text by default.
  final double height;

  @override
  State<ShowMore> createState() => _ShowMoreState();
}

class _ShowMoreState extends State<ShowMore> {
  var _whole = false;
  var _taller = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _Clamped(
          height: widget.height,
          clamp: !_whole,
          onTaller: (taller) {
            if (mounted && taller != _taller) {
              setState(() => _taller = taller);
            }
          },
          // Cut, its last line fades out rather than ending mid-letter.
          child: _taller && !_whole
              ? ShaderMask(
                  blendMode: BlendMode.dstIn,
                  shaderCallback: (bounds) => LinearGradient(
                    begin: Alignment.topCenter,
                    end: Alignment.bottomCenter,
                    stops: [
                      (widget.height - 18) / bounds.height,
                      widget.height / bounds.height,
                    ],
                    colors: const [Color(0xFFFFFFFF), Color(0x00FFFFFF)],
                  ).createShader(bounds),
                  child: widget.child,
                )
              : widget.child,
        ),
        if (_taller)
          Align(
            alignment: Alignment.centerLeft,
            child: Pressable(
              onTap: () => setState(() => _whole = !_whole),
              builder: (context, hovered) => Padding(
                padding: const EdgeInsets.only(top: 2),
                child: Text(
                  _whole ? 'Show less' : 'Show more',
                  style: TextStyles.caption.copyWith(
                    color: hovered ? palette.text : palette.textMuted,
                  ),
                ),
              ),
            ),
          ),
      ],
    );
  }
}

/// Lays its child out whole and, when [clamp], shows only its top
/// [height], telling [onTaller] whether there's more.
class _Clamped extends SingleChildRenderObjectWidget {
  const _Clamped({
    required this.height,
    required this.clamp,
    required this.onTaller,
    super.child,
  });

  final double height;
  final bool clamp;
  final ValueChanged<bool> onTaller;

  @override
  _RenderClamped createRenderObject(BuildContext context) =>
      _RenderClamped(height, clamp, onTaller);

  @override
  void updateRenderObject(BuildContext context, _RenderClamped render) {
    render
      ..height = height
      ..clamp = clamp
      ..onTaller = onTaller;
  }
}

class _RenderClamped extends RenderProxyBox {
  _RenderClamped(this._height, this._clamp, this.onTaller);

  double _height;
  set height(double value) {
    if (value == _height) return;
    _height = value;
    markNeedsLayout();
  }

  bool _clamp;
  set clamp(bool value) {
    if (value == _clamp) return;
    _clamp = value;
    markNeedsLayout();
  }

  ValueChanged<bool> onTaller;
  bool? _told;
  var _cut = false;
  final _clip = LayerHandle<ClipRectLayer>();

  @override
  void performLayout() {
    final child = this.child;
    if (child == null) {
      size = constraints.smallest;
      return;
    }
    child.layout(
      constraints.copyWith(minHeight: 0, maxHeight: double.infinity),
      parentUsesSize: true,
    );
    final taller = child.size.height > _height + 0.5;
    _cut = _clamp && taller;
    size = constraints.constrain(
      Size(child.size.width, _cut ? _height : child.size.height),
    );
    if (taller != _told) {
      _told = taller;
      final tell = onTaller;
      SchedulerBinding.instance.addPostFrameCallback((_) => tell(taller));
    }
  }

  @override
  void paint(PaintingContext context, Offset offset) {
    if (!_cut) {
      _clip.layer = null;
      super.paint(context, offset);
      return;
    }
    _clip.layer = context.pushClipRect(
      needsCompositing,
      offset,
      Offset.zero & size,
      super.paint,
      oldLayer: _clip.layer,
    );
  }

  @override
  void dispose() {
    _clip.layer = null;
    super.dispose();
  }
}

/// A row in a view's body: a [lead] 14px mark or character, then [child].
class _Row extends StatelessWidget {
  const _Row({required this.child, this.lead, this.trailing});

  final Widget? lead;
  final Widget child;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 1),
    child: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        if (lead != null) ...[
          Padding(
            // Centers a 14px mark on the first line of body text.
            padding: const EdgeInsets.only(top: 2.5),
            child: SizedBox.square(dimension: 14, child: lead),
          ),
          const SizedBox(width: Space.s),
        ],
        Expanded(child: child),
        if (trailing != null) ...[const SizedBox(width: Space.s), trailing!],
      ],
    ),
  );
}

/// Text a view's body shows small and muted, at most [lines] lines.
class _Faint extends StatelessWidget {
  const _Faint(this.text, {this.lines = 2, this.color});

  final String text;
  final int lines;
  final Color? color;

  @override
  Widget build(BuildContext context) => Text(
    text,
    maxLines: lines,
    overflow: TextOverflow.ellipsis,
    style: TextStyles.caption.copyWith(
      color: color ?? Palette.of(context).textMuted,
    ),
  );
}

Widget _rows(List<Widget> rows) =>
    Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: rows);

/// "1 item", "3 items".
String _count(int count, String one, [String? many]) =>
    '$count ${count == 1 ? one : (many ?? '${one}s')}';

// ---------------------------------------------------------------------------
// Messaging.

class _SendLine extends StatelessWidget {
  const _SendLine(this.call);

  final ToolCallInfo call;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final to = call.argument('to') ?? '';
    final details = call.details;
    final self = ToolViewScope.of(context)?.agentId ?? '';
    final id = _string(details['to']);
    final agent = id.isNotEmpty
        ? AgentTints.of(context)?.agents[id]
        : resolveAgent(context, to, self: self);
    final String result;
    if (!call.done) {
      result = '';
    } else if (details['working'] == true) {
      result = 'Queued';
    } else {
      result = 'Sent';
    }
    return Row(
      children: [
        Text('→', style: TextStyles.body.copyWith(color: palette.textMuted)),
        const SizedBox(width: Space.s),
        Flexible(
          child: agent != null
              ? ToolAgent(
                  id: agent.id,
                  name: _string(details['name']),
                  role: details['role'] is String
                      ? details['role'] as String
                      : null,
                )
              : ToolLine('', subject: _string(details['name']).ifEmpty(to)),
        ),
        if (result.isNotEmpty) ...[
          const SizedBox(width: Space.s),
          _Trailing(result),
        ],
      ],
    );
  }
}

Widget? _sendBody(BuildContext context, ToolCallInfo call) {
  final text = call.argument('text') ?? '';
  if (text.trim().isEmpty) return null;
  return FirstLine(text.trim());
}

/// A message folded to its first line, as plain text cut with an ellipsis,
/// with "Show more" at its end when there's more; open, the whole message
/// as Markdown with "Show less" under it.
class FirstLine extends StatefulWidget {
  const FirstLine(this.text, {super.key});

  /// Markdown.
  final String text;

  @override
  State<FirstLine> createState() => _FirstLineState();
}

class _FirstLineState extends State<FirstLine> {
  var _open = false;

  /// The first line with something on it, without its Markdown.
  static String _first(String text) {
    for (final line in text.split('\n')) {
      final plain = md.Document(extensionSet: md.ExtensionSet.gitHubFlavored)
          .parseInline(
            line.trim().replaceFirst(RegExp(r'^(#+|[-*+>]|\d+\.)\s+'), ''),
          )
          .map((node) => node.textContent)
          .join()
          .trim();
      if (plain.isNotEmpty) return plain;
    }
    return text.trim();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    Widget toggle(String label) => Pressable(
      onTap: () => setState(() => _open = !_open),
      builder: (context, hovered) => Text(
        label,
        style: TextStyles.caption.copyWith(
          color: hovered ? palette.text : palette.textMuted,
        ),
      ),
    );
    if (_open) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Markdown(widget.text, color: palette.textMuted),
          Align(
            alignment: Alignment.centerLeft,
            child: Padding(
              padding: const EdgeInsets.only(top: 2),
              child: toggle('Show less'),
            ),
          ),
        ],
      );
    }
    final first = _first(widget.text);
    final style = TextStyles.body.copyWith(color: palette.textMuted);
    final lines = widget.text
        .trim()
        .split('\n')
        .where((l) => l.trim().isNotEmpty);
    return LayoutBuilder(
      builder: (context, constraints) {
        final painter = TextPainter(
          text: TextSpan(text: first, style: style),
          maxLines: 1,
          textDirection: TextDirection.ltr,
          textScaler: MediaQuery.textScalerOf(context),
        )..layout(maxWidth: constraints.maxWidth);
        // Something left out: more lines, Markdown that reads differently
        // whole, or a line too long for the width.
        final more =
            lines.length > 1 ||
            first != widget.text.trim() ||
            painter.didExceedMaxLines;
        painter.dispose();
        return Row(
          children: [
            Flexible(
              child: Text(
                first,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: style,
              ),
            ),
            if (more) ...[const SizedBox(width: Space.s), toggle('Show more')],
          ],
        );
      },
    );
  }
}

class _ProposeLine extends StatelessWidget {
  const _ProposeLine(this.call);

  final ToolCallInfo call;

  @override
  Widget build(BuildContext context) {
    final scope = context.getInheritedWidgetOfExactType<AppScope>();
    final rivet = scope == null
        ? null
        : rivetIn(scope.backend.workspace.workspace);
    return Row(
      children: [
        const ToolLine('Proposed a message to'),
        const SizedBox(width: Space.s),
        Flexible(
          child: rivet == null
              ? const ToolLine('', subject: 'Rivet')
              : ToolAgent(id: rivet.id, name: rivet.name, role: ''),
        ),
      ],
    );
  }
}

// ---------------------------------------------------------------------------
// Checklist.

ChecklistStatus? _status(Object? name) => switch (name) {
  'open' => ChecklistStatus.CHECKLIST_STATUS_OPEN,
  'in_progress' => ChecklistStatus.CHECKLIST_STATUS_IN_PROGRESS,
  'blocked' => ChecklistStatus.CHECKLIST_STATUS_BLOCKED,
  'done' => ChecklistStatus.CHECKLIST_STATUS_DONE,
  'dropped' => ChecklistStatus.CHECKLIST_STATUS_DROPPED,
  _ => null,
};

/// One checklist item as a view shows it: its mark, its number, its text
/// (struck through once done or dropped), a note under it, and its owner's
/// character.
class _ItemRow extends StatelessWidget {
  const _ItemRow({
    required this.status,
    required this.id,
    required this.text,
    this.owner = '',
    this.note = '',
    this.label = '',
    this.problem = '',
  });

  final ChecklistStatus? status;
  final int? id;
  final String text;
  final String owner;
  final String note;

  /// What changed, said after the text: "Done", "In progress".
  final String label;
  final String problem;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final status = this.status;
    final finished =
        status == ChecklistStatus.CHECKLIST_STATUS_DONE ||
        status == ChecklistStatus.CHECKLIST_STATUS_DROPPED;
    final owner = this.owner.isEmpty
        ? null
        : AgentTints.of(context)?.agents[this.owner];
    final words = <InlineSpan>[
      if (id != null)
        TextSpan(
          text: '#$id ',
          style: TextStyle(
            color: palette.textFaint,
            fontFeatures: const [FontFeature.tabularFigures()],
          ),
        ),
      if (text.isNotEmpty)
        TextSpan(
          text: text,
          style: TextStyle(
            color: finished ? palette.textMuted : palette.text,
            decoration: finished ? TextDecoration.lineThrough : null,
            decorationColor: palette.textMuted,
          ),
        ),
      if (label.isNotEmpty)
        TextSpan(
          text: '  $label',
          style: TextStyles.caption.copyWith(color: palette.textMuted),
        ),
    ];
    return _Row(
      lead: status == null ? null : StatusMark(status: status),
      trailing: this.owner.isEmpty
          ? null
          : Semantics(
              label: owner?.name ?? this.owner,
              child: owner == null
                  ? const UnknownCharacter(size: 18)
                  : AgentCharacter(agent: owner, size: 18),
            ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text.rich(TextSpan(children: words), style: TextStyles.body),
          if (note.isNotEmpty) _Faint(note),
          if (problem.isNotEmpty) _Faint(problem, color: palette.danger),
        ],
      ),
    );
  }
}

Widget _checklistAddLine(BuildContext context, ToolCallInfo call) {
  final added = _list(call.details['items']);
  final count = added.isNotEmpty ? added.length : call.list('items').length;
  if (call.running || count == 0) {
    return const ToolLine('Adding to the checklist…');
  }
  return ToolLine('Added ${_count(count, 'item')} to the checklist');
}

Widget? _checklistAddBody(BuildContext context, ToolCallInfo call) {
  final added = _list(call.details['items']);
  if (added.isNotEmpty) {
    return _rows([
      for (final item in added.map(_map))
        _ItemRow(
          status: ChecklistStatus.CHECKLIST_STATUS_OPEN,
          id: _int(item['id']),
          text: _string(item['text']),
          owner: _string(item['owner']),
        ),
    ]);
  }
  final items = call.list('items');
  if (items.isEmpty) return null;
  // The owner as written; the app shows a character only for an id.
  final owner = call.argument('owner') ?? '';
  final known = AgentTints.of(context)?.agents.containsKey(owner) ?? false;
  return _rows([
    for (final item in items)
      _ItemRow(
        status: ChecklistStatus.CHECKLIST_STATUS_OPEN,
        id: null,
        text: _string(item),
        owner: known ? owner : '',
      ),
  ]);
}

Widget _checklistUpdateLine(BuildContext context, ToolCallInfo call) {
  final count = call.list('updates').length;
  if (call.running || count == 0) {
    return const ToolLine('Updating the checklist…');
  }
  return ToolLine('Updated ${_count(count, 'item')} on the checklist');
}

Widget? _checklistUpdateBody(BuildContext context, ToolCallInfo call) {
  final updated = _list(call.details['items']);
  final updates = call.list('updates').map(_map).toList();
  if (updated.isNotEmpty) {
    return _rows([
      for (final item in updated.map(_map))
        _ItemRow(
          status: _status(item['status']),
          id: _int(item['id']),
          text: _string(item['text']),
          owner: _string(item['owner']),
          note: _string(item['note']),
          label: _list(item['changed']).contains('status')
              ? statusLabel(_status(item['status'])!)
              : '',
          problem: _problem(_string(item['problem'])),
        ),
    ]);
  }
  if (updates.isEmpty) return null;
  return _rows([
    for (final update in updates)
      _ItemRow(
        status: _status(update['status']),
        id: _int(update['id']),
        text: _string(update['text']),
        note: _string(update['note']),
        label: _status(update['status']) == null
            ? ''
            : statusLabel(_status(update['status'])!),
      ),
  ]);
}

/// A checklist update's problem, as a sentence.
String _problem(String problem) => problem.isEmpty
    ? ''
    : '${problem[0].toUpperCase()}${problem.substring(1)}.';

// ---------------------------------------------------------------------------
// Memory.

bool _manual(ToolCallInfo call) =>
    _string(call.details['instance']) == 'factory' ||
    call.argument('instance') == 'factory';

Widget _memoryReadLine(BuildContext context, ToolCallInfo call) {
  final details = call.details;
  final manual = _manual(call);
  final slug = _string(details['slug']).ifEmpty(call.argument('note') ?? '');
  final query = _string(details['query']).ifEmpty(call.argument('q') ?? '');
  if (slug.isNotEmpty) {
    return ToolLine(
      manual ? 'Read the manual on' : 'Read note',
      subject: slug,
      after: _string(details['title']),
    );
  }
  if (query.isNotEmpty) {
    final hits = _int(details['hits']);
    return ToolLine(
      manual ? 'Searched the manual for' : 'Searched memory for',
      subject: '“$query”',
      after: hits == null
          ? ''
          : hits == 0
          ? 'Nothing found'
          : _count(hits, 'match', 'matches'),
    );
  }
  final notes = _int(details['notes']);
  final preferences = _int(details['preferences']);
  return ToolLine(
    manual ? 'Read the manual' : 'Read memory',
    after: [
      if (preferences != null) _count(preferences, 'preference'),
      if (notes != null) _count(notes, 'note'),
    ].join(', '),
  );
}

Widget? _memoryReadBody(BuildContext context, ToolCallInfo call) {
  final slug = _string(call.details['slug']).ifEmpty(call.argument('note'));
  if (slug.isEmpty || !call.done) return null;
  // The note's text follows its heading line and a blank line.
  final output = call.call.output;
  final at = output.indexOf('\n\n');
  final text = at < 0 ? '' : output.substring(at + 2).trim();
  if (text.isEmpty) return null;
  return FirstLine(text);
}

/// What one memory op did, for its line.
String _memoryChange(Map<String, Object?> op, Map<String, Object?> change) {
  final title = _string(change['title']);
  final slug = _string(op['slug']).ifEmpty(_string(change['slug']));
  final id = _string(op['id']).ifEmpty(_string(change['id']));
  return switch (_string(op['op'])) {
    'pref_add' => 'Remembered: ${title.ifEmpty(_string(op['rule']))}',
    'pref_replace' =>
      'Changed a preference: ${title.ifEmpty(_string(op['rule']))}',
    'pref_remove' =>
      title.isEmpty ? 'Forgot preference #$id' : 'Forgot: $title',
    'note_write' => 'Remembered: ${title.ifEmpty(_string(op['summary']))}',
    'note_edit' => 'Edited note $slug',
    'note_remove' => 'Removed note $slug',
    _ => 'Changed memory',
  };
}

Widget _memoryUpdateLine(BuildContext context, ToolCallInfo call) {
  final ops = call.list('ops').map(_map).toList();
  final changes = _list(call.details['changes']).map(_map).toList();
  if (ops.isEmpty) {
    return const ToolLine('Updating memory…');
  }
  if (ops.length == 1) {
    final change = changes.isEmpty ? const <String, Object?>{} : changes.first;
    final words = _memoryChange(ops.single, change);
    final colon = words.indexOf(': ');
    if (colon < 0) return ToolLine(words);
    return ToolLine(
      words.substring(0, colon + 1),
      subject: words.substring(colon + 2),
    );
  }
  return ToolLine('Updated memory', after: _count(ops.length, 'change'));
}

Widget? _memoryUpdateBody(BuildContext context, ToolCallInfo call) {
  final ops = call.list('ops').map(_map).toList();
  final changes = _list(call.details['changes']).map(_map).toList();
  if (ops.length == 1) {
    final op = ops.single;
    return switch (_string(op['op'])) {
      'note_write' when _string(op['body']).trim().isNotEmpty => FirstLine(
        _string(op['body']).trim(),
      ),
      'pref_add' || 'pref_replace' when _string(op['why']).isNotEmpty => _Faint(
        _string(op['why']),
      ),
      _ => null,
    };
  }
  if (ops.isEmpty) return null;
  return _rows([
    for (var i = 0; i < ops.length; i++)
      _Row(
        child: _Faint(
          _memoryChange(
            ops[i],
            i < changes.length ? changes[i] : const <String, Object?>{},
          ),
          lines: 1,
        ),
      ),
  ]);
}

// ---------------------------------------------------------------------------
// Monitors.

/// When a monitor fires, from its `when` as written: "every 3h", "at
/// 09:00", "when `gh run view` succeeds".
String describeWhen(Object? when) {
  final spec = _map(when);
  if (spec['every'] != null && spec['check'] == null) {
    return 'every ${_string(spec['every'])}';
  }
  if (spec['at'] != null) return 'at ${_string(spec['at'])}';
  if (spec['in'] != null) return 'in ${_string(spec['in'])}';
  if (spec['cron'] != null) return 'on schedule ${_string(spec['cron'])}';
  if (spec['check'] != null) return 'when ${_string(spec['check'])}';
  if (spec['files'] != null) {
    final files = _list(spec['files']).map(_string).join(', ');
    return 'when $files change';
  }
  if (spec['watch'] != null) return 'when ${_string(spec['watch'])} prints';
  return '';
}

/// Monitor [index] of [call]: its name and when it fires.
({String name, String schedule, String agent}) _monitor(
  ToolCallInfo call,
  int index,
) {
  final asked = call.list('monitors');
  final added = _list(call.details['monitors']);
  final input = index < asked.length ? _map(asked[index]) : const {};
  final made = index < added.length ? _map(added[index]) : const {};
  return (
    name: _string(made['name']).ifEmpty(_string(input['name'])),
    schedule: _string(made['schedule']).ifEmpty(describeWhen(input['when'])),
    agent: _string(made['agent']).ifEmpty(_string(input['agent'])),
  );
}

Widget _monitorsAddLine(BuildContext context, ToolCallInfo call) {
  final count = call.list('monitors').length;
  if (count == 0) return const ToolLine('Adding a monitor…');
  if (count > 1) return ToolLine('Added ${_count(count, 'monitor')}');
  final monitor = _monitor(call, 0);
  return ToolLine(
    'Set a monitor',
    subject: monitor.name,
    after: monitor.schedule,
  );
}

Widget? _monitorsAddBody(BuildContext context, ToolCallInfo call) {
  final asked = call.list('monitors');
  if (asked.length == 1) {
    final prompt = _string(_map(asked.single)['prompt']);
    return prompt.isEmpty ? null : FirstLine(prompt);
  }
  if (asked.isEmpty) return null;
  return _rows([
    for (var i = 0; i < asked.length; i++)
      _Row(
        child: ToolLine(
          '',
          subject: _monitor(call, i).name,
          after: _monitor(call, i).schedule,
        ),
      ),
  ]);
}

Widget _monitorsChangeLine(BuildContext context, ToolCallInfo call) {
  final changed = _list(call.details['monitors']).map(_map).toList();
  final asked = call.list('changes').map(_map).toList();
  String past(String action) => switch (action) {
    'pause' => 'paused',
    'resume' => 'resumed',
    'cancel' => 'cancelled',
    'fire' => 'fired',
    _ => action,
  };
  final words = changed.isNotEmpty
      ? [
          for (final monitor in changed)
            '${past(_string(monitor['action']))} ${_string(monitor['name']).ifEmpty(_string(monitor['id']))}',
        ]
      : [
          for (final change in asked)
            '${past(_string(change['action']))} ${_string(change['id'])}',
        ];
  if (words.isEmpty) return const ToolLine('Changing monitors…');
  final line = words.join(', ');
  return ToolLine('${line[0].toUpperCase()}${line.substring(1)}');
}

// ---------------------------------------------------------------------------
// Tasks.

Widget _tasksListLine(BuildContext context, ToolCallInfo call) {
  final tasks = _list(call.details['tasks']);
  final running = tasks.where((task) => _map(task)['running'] == true).length;
  return ToolLine(
    'Listed background tasks',
    after: call.details.containsKey('tasks')
        ? (tasks.isEmpty
              ? 'None'
              : '${_count(tasks.length, 'task')}, $running running')
        : '',
  );
}

Widget? _tasksListBody(BuildContext context, ToolCallInfo call) {
  final tasks = _list(call.details['tasks']).map(_map).toList();
  if (tasks.isEmpty) return null;
  final palette = Palette.of(context);
  return _rows([
    for (final task in tasks)
      _Row(
        trailing: task['running'] == true
            ? const _Trailing('Running')
            : _Trailing(_string(task['status'])),
        child: Text.rich(
          TextSpan(
            children: [
              TextSpan(
                text: '${_string(task['task'])}  ',
                style: TextStyle(color: palette.textFaint),
              ),
              TextSpan(text: _string(task['command'])),
            ],
          ),
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: TextStyles.code.copyWith(color: palette.textMuted),
        ),
      ),
  ]);
}

Widget _tasksStopLine(BuildContext context, ToolCallInfo call) {
  final task = _string(call.details['task']).ifEmpty(call.argument('task'));
  final owner = _string(call.details['agent']).ifEmpty(call.argument('agent'));
  final self = ToolViewScope.of(context)?.agentId ?? '';
  final name = AgentTints.of(context)?.agents[owner]?.name ?? owner;
  return ToolLine(
    call.done ? 'Stopped' : 'Stopping',
    subject: task,
    after: owner.isEmpty || owner == self ? '' : name,
  );
}

/// bash moved to (or started in) the background: its task and command,
/// and whether it still runs, from its "finished" event in the transcript.
class _BackgroundLine extends StatelessWidget {
  const _BackgroundLine(this.call);

  final ToolCallInfo call;

  @override
  Widget build(BuildContext context) {
    final task = _string(call.details['task']);
    final command = (call.argument('command') ?? '').trim().split('\n').first;
    final transcript = ToolViewScope.of(context)?.transcript;
    return Row(
      children: [
        Flexible(
          child: ToolLine(
            'Background task $task:',
            subject: command,
            code: true,
          ),
        ),
        if (transcript != null) ...[
          const SizedBox(width: Space.s),
          ListenableBuilder(
            listenable: transcript,
            builder: (context, _) {
              final ended = taskEnded(transcript, task, after: call.seq);
              if (ended == null) {
                return const Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    LiveDot(),
                    SizedBox(width: Space.xs),
                    _Trailing('Running'),
                  ],
                );
              }
              return _Trailing(
                ended,
                color: ended.startsWith('exit 0')
                    ? null
                    : Palette.of(context).danger,
              );
            },
          ),
        ],
      ],
    );
  }
}

/// How background [task] ended, from its "finished" event after entry
/// [after]: "exit 0, 4.2s". Null while there's none, as it still runs.
String? taskEnded(TranscriptStore transcript, String task, {int after = 0}) {
  for (var i = transcript.length - 1; i >= 0; i--) {
    final model = transcript[i];
    if (model.seq <= after) break;
    final entry = model.entry;
    if (!entry.hasEvent() ||
        entry.event.source != EventSource.EVENT_SOURCE_TASK ||
        entry.event.fromId != task) {
      continue;
    }
    final how = RegExp(r'\(([^)]*)\)').firstMatch(entry.event.summary);
    return how?.group(1) ?? 'Ended';
  }
  return null;
}

// ---------------------------------------------------------------------------
// Viewer.

/// `show`: the file it showed, which brings its pane forward on a click
/// (or opens it again once closed).
class _ShowLine extends StatelessWidget {
  const _ShowLine(this.call);

  final ToolCallInfo call;

  @override
  Widget build(BuildContext context) {
    final path = _string(call.details['path']).ifEmpty(call.argument('path'));
    final slash = path.lastIndexOf('/');
    final name = slash < 0 ? path : path.substring(slash + 1);
    final folder = slash <= 0 ? '' : shortPath(path.substring(0, slash));
    final palette = Palette.of(context);
    return Row(
      children: [
        Text(
          call.done ? 'Showing' : 'Opening',
          style: TextStyles.body.copyWith(color: palette.textMuted),
        ),
        const SizedBox(width: Space.xs),
        Flexible(
          child: _Link(
            text: name,
            onTap: call.done && path.isNotEmpty
                ? () => _focus(context, call, path)
                : null,
          ),
        ),
        if (folder.isNotEmpty) ...[
          const SizedBox(width: Space.s),
          Flexible(child: _Trailing(folder)),
        ],
      ],
    );
  }

  static void _focus(BuildContext context, ToolCallInfo call, String path) {
    final scope = context.getInheritedWidgetOfExactType<AppScope>();
    if (scope == null) return;
    final backend = scope.backend;
    final agentId = _string(call.details['agent'])
        .ifEmpty(ToolViewScope.of(context)?.agentId);
    final agent = backend.workspace.agent(agentId);
    if (agent == null) return;
    final root = layoutOf(agent);
    final pane = _string(call.details['pane']);
    final shown = pane.isNotEmpty && find(root, pane) != null
        ? (root: root, paneId: pane)
        : open(root, path);
    if (identical(shown.root, root)) {
      backend.highlights.glow('pane:${agent.id}:${shown.paneId}', [
        agentColor(agent.color),
      ]);
      return;
    }
    fireAndLog(backend.setLayout(agent.id, shown.root));
  }
}

// ---------------------------------------------------------------------------
// Web.

String _host(String url) {
  final host = Uri.tryParse(url)?.host ?? '';
  return host.startsWith('www.') ? host.substring(4) : host;
}

Widget _webSearchLine(BuildContext context, ToolCallInfo call) {
  final queries = call.list('queries').map(_string).toList();
  final answers = _list(call.details['queries']).map(_map).toList();
  final results = answers.fold<int>(
    0,
    (sum, answer) => sum + _list(answer['results']).length,
  );
  return ToolLine(
    call.done ? 'Searched' : 'Searching',
    subject: queries.map((query) => '“$query”').join(', '),
    after: call.details.containsKey('queries')
        ? (results == 0 ? 'No results' : _count(results, 'result'))
        : '',
  );
}

/// The most results a search view lists per query.
const searchResultsShown = 5;

Widget? _webSearchBody(BuildContext context, ToolCallInfo call) {
  final answers = _list(call.details['queries']).map(_map).toList();
  if (answers.isEmpty) return null;
  final palette = Palette.of(context);
  return _rows([
    for (final answer in answers) ...[
      if (answers.length > 1)
        Padding(
          padding: const EdgeInsets.only(top: Space.xs),
          child: _Faint('“${_string(answer['q'])}”', lines: 1),
        ),
      for (final hit in _list(
        answer['results'],
      ).map(_map).take(searchResultsShown))
        _Row(
          trailing: _Trailing(_host(_string(hit['url']))),
          child: _Link(
            text: _string(hit['title']).ifEmpty(_string(hit['url'])),
            color: palette.text,
            onTap: () {
              final url = Uri.tryParse(_string(hit['url']));
              if (url != null) openWebLink(url);
            },
          ),
        ),
      if (_list(answer['results']).length > searchResultsShown)
        _Faint(
          '${_list(answer['results']).length - searchResultsShown} more',
          lines: 1,
        ),
    ],
  ]);
}

class _FetchLine extends StatelessWidget {
  const _FetchLine(this.call);

  final ToolCallInfo call;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final url = _string(call.details['url']).ifEmpty(call.argument('url'));
    final title = _string(call.details['title']);
    final link = Uri.tryParse(url);
    return Row(
      children: [
        Text(
          call.done ? 'Read' : 'Reading',
          style: TextStyles.body.copyWith(color: palette.textMuted),
        ),
        const SizedBox(width: Space.xs),
        Flexible(
          child: _Link(
            text: title.ifEmpty(url),
            onTap: link == null || !link.hasScheme
                ? null
                : () => openWebLink(link),
          ),
        ),
        if (title.isNotEmpty) ...[
          const SizedBox(width: Space.s),
          _Trailing(_host(url)),
        ],
      ],
    );
  }
}

// ---------------------------------------------------------------------------
// Database.

Widget _dbLine(BuildContext context, ToolCallInfo call) {
  final details = call.details;
  final records = _string(details['instance'])
      .ifEmpty(call.argument('instance'))
      .isNotEmpty;
  final what = records ? "Claktory's records" : 'the database';
  switch (call.name) {
    case 'db_query':
      final rows = _int(details['rows']);
      final more = _int(details['more']) ?? 0;
      final total = rows == null ? null : rows + more;
      return ToolLine(
        'Queried $what',
        after: total == null
            ? ''
            : '${_count(total, 'row')}${details['capped'] == true ? ' or more' : ''}',
      );
    case 'db_exec':
      final changed = _int(details['changed']);
      return ToolLine(
        'Changed $what',
        after: changed == null ? '' : '${_count(changed, 'row')} changed',
      );
    default:
      final table = _string(details['table']).ifEmpty(call.argument('table'));
      final tables = _int(details['tables']);
      if (table.isNotEmpty) {
        return ToolLine('Read the table', subject: table, code: true);
      }
      return ToolLine(
        "Read $what's tables",
        after: tables == null ? '' : _count(tables, 'table'),
      );
  }
}

Widget? _sqlBody(BuildContext context, ToolCallInfo call) {
  final sql = (call.argument('sql') ?? '').trim();
  if (sql.isEmpty) return null;
  // Three lines of code.
  return ShowMore(
    height: 3 * TextStyles.code.fontSize! * TextStyles.code.height!,
    child: Text(
      sql,
      style: TextStyles.code.copyWith(color: Palette.of(context).textMuted),
    ),
  );
}

// ---------------------------------------------------------------------------
// Secrets.

/// The agents a secrets call names: from its details, each `{id, name}`,
/// else as the input wrote them.
List<({String id, String name})> _secretAgents(
  BuildContext context,
  ToolCallInfo call,
) {
  final given = _list(call.details['agents']).map(_map).toList();
  if (given.isNotEmpty) {
    return [
      for (final agent in given)
        (id: _string(agent['id']), name: _string(agent['name'])),
    ];
  }
  final self = ToolViewScope.of(context)?.agentId ?? '';
  return [
    for (final asked in call.list('agents').map(_string))
      if (resolveAgent(context, asked, self: self) case final agent?)
        (id: agent.id, name: agent.name)
      else
        (id: '', name: asked),
  ];
}

/// `secrets_grant` and `secrets_revoke`: "Gave AWS to June", "Took
/// Cloudflare back from Sam", each agent a link to its conversation.
class _SecretLine extends StatelessWidget {
  const _SecretLine(this.call, {required this.give});

  final ToolCallInfo call;
  final bool give;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final secret = _string(call.details['secret'])
        .ifEmpty(call.argument('secret'));
    final agents = _secretAgents(context, call);
    final done = call.done;
    final lead = give ? (done ? 'Gave' : 'Giving') : (done ? 'Took' : 'Taking');
    final words = TextStyles.body.copyWith(color: palette.textMuted);
    return Row(
      children: [
        Flexible(child: ToolLine(lead, subject: secret)),
        const SizedBox(width: Space.xs),
        Text(give ? 'to' : 'back from', style: words),
        for (var i = 0; i < agents.length; i++) ...[
          SizedBox(width: i == 0 ? Space.xs : 0),
          if (i > 0) Text(', ', style: words),
          Flexible(
            child: agents[i].id.isEmpty
                ? ToolLine('', subject: agents[i].name)
                : ToolAgent(id: agents[i].id, name: agents[i].name, role: ''),
          ),
        ],
      ],
    );
  }
}

/// The tasks a revoke stopped, per agent.
Widget? _secretRevokeBody(BuildContext context, ToolCallInfo call) {
  final agents = _list(call.details['agents']).map(_map).toList();
  final lines = [
    for (final agent in agents)
      if (_list(agent['stopped']).isNotEmpty)
        'Stopped ${_string(agent['name']).ifEmpty(_string(agent['id']))}\'s '
            '${_list(agent['stopped']).map(_string).join(', ')}',
  ];
  if (lines.isEmpty) return null;
  return _rows([for (final line in lines) _Faint(line, lines: 1)]);
}

Widget _secretsListLine(BuildContext context, ToolCallInfo call) {
  final query = call.argument('q') ?? '';
  final secrets = call.details.containsKey('secrets')
      ? _list(call.details['secrets']).length
      : null;
  return ToolLine(
    secrets == null
        ? 'Listed secrets'
        : secrets == 0
        ? 'Listed secrets: none'
        : 'Listed ${_count(secrets, 'secret')}',
    after: query.isEmpty ? '' : '“$query”',
  );
}

Widget? _secretsListBody(BuildContext context, ToolCallInfo call) {
  final secrets = _list(call.details['secrets']).map(_map).toList();
  if (secrets.isEmpty) return null;
  final palette = Palette.of(context);
  final agents = AgentTints.of(context)?.agents ?? const {};
  return _rows([
    for (final secret in secrets)
      _Row(
        trailing: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            for (final id in _list(secret['agents']).map(_string))
              Padding(
                padding: const EdgeInsets.only(left: 2),
                child: Semantics(
                  label: agents[id]?.name ?? id,
                  child: agents[id] == null
                      ? const UnknownCharacter(size: 18)
                      : AgentCharacter(agent: agents[id]!, size: 18),
                ),
              ),
          ],
        ),
        child: Text.rich(
          TextSpan(
            children: [
              TextSpan(
                text: _string(secret['name']),
                style: TextStyle(color: palette.text),
              ),
              TextSpan(
                text: '  ${_list(secret['variables']).map(_string).join(', ')}',
                style: TextStyles.code.copyWith(color: palette.textFaint),
              ),
            ],
          ),
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: TextStyles.body,
        ),
      ),
  ]);
}

// ---------------------------------------------------------------------------

/// The string value of [key] in [json] as far as it's written, decoding its
/// escapes; with no [key], the first string value. Null when there's none
/// yet.
String? partialJsonString(String json, String? key) {
  final pattern = key == null
      ? RegExp(r'"[^"\\]*"\s*:\s*"')
      : RegExp('"${RegExp.escape(key)}"\\s*:\\s*"');
  final match = pattern.firstMatch(json);
  if (match == null) return null;
  final out = StringBuffer();
  var i = match.end;
  while (i < json.length) {
    final char = json[i];
    if (char == '"') break;
    if (char == r'\') {
      if (i + 1 >= json.length) break;
      final next = json[i + 1];
      switch (next) {
        case 'n':
          out.write('\n');
        case 't':
          out.write('\t');
        case 'r':
          break;
        case 'u':
          if (i + 6 > json.length) return out.toString();
          final code = int.tryParse(json.substring(i + 2, i + 6), radix: 16);
          if (code != null) out.writeCharCode(code);
          i += 6;
          continue;
        default:
          out.write(next);
      }
      i += 2;
      continue;
    }
    out.write(char);
    i++;
  }
  return out.toString();
}

extension on String {
  /// This, or [other] when this is empty.
  String ifEmpty(String? other) => isEmpty ? (other ?? '') : this;
}
