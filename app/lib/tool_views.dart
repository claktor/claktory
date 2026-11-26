import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:flutter/rendering.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';
import 'package:markdown/markdown.dart' as md;

import 'agent_name.dart';
import 'backend.dart' show RequestFailure, fireAndLog;
import 'characters/character.dart';
import 'controls.dart';
import 'explorer.dart' show openFile, shortPath;
import 'extensions.dart' show StatusMark, statusLabel;
import 'find.dart' show FindHighlight;
import 'forms.dart' show rivetIn;
import 'identity.dart';
import 'mobile/metrics.dart';
import 'mobile/navigation.dart' show PhoneRoutesScope;
import 'paced.dart' show PreviewBox;
import 'layout.dart';
import 'loopback_forward.dart' show openLink;
import 'markdown.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart' show Unselectable;
import 'share_files.dart' show ShareHold;
import 'state.dart';
import 'transcript.dart' show AgentTints;
import 'visible_codes.dart';

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

/// A tool call as its view reads it. Every string in its input and
/// details shows what could make it read as something else by its code
/// ([visibleCodes]; the words under [proseKeys] what reorders them,
/// [proseCodes]): an agent chose them (Fugo).
class ToolCallInfo {
  factory ToolCallInfo(ToolCall call, {int seq = 0}) {
    final rawInput = _raw(call.input);
    final rawDetails = _raw(call.details) ?? const <String, Object?>{};
    final input = _shown(rawInput);
    final details = _shown(rawDetails) ?? const <String, Object?>{};
    // A tool of an extension turned on mid-context, called through
    // extension_call {tool, args}: shown as that tool.
    final inner = input?['tool'];
    if (call.name == 'extension_call' && inner is String) {
      final args = input?['args'];
      final rawArgs = rawInput?['args'];
      return ToolCallInfo._(
        call,
        seq,
        name: inner,
        input: args is Map<String, Object?> ? args : const {},
        details: details,
        rawInput: rawArgs is Map<String, Object?> ? rawArgs : const {},
        rawDetails: rawDetails,
      );
    }
    return ToolCallInfo._(
      call,
      seq,
      name: call.name,
      input: input,
      details: details,
      rawInput: rawInput,
      rawDetails: rawDetails,
    );
  }

  ToolCallInfo._(
    this.call,
    this.seq, {
    required this.name,
    required this.input,
    required this.details,
    required this.rawInput,
    required this.rawDetails,
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

  /// [input] and [details] as the agent wrote them, hidden characters and
  /// all: never to show, only to act on (a file's path, to open or share
  /// it, Zucchero's row 343).
  final Map<String, Object?>? rawInput;
  final Map<String, Object?> rawDetails;

  /// The string [key] of the details, else of the input, as written: what
  /// a file's path really is, where the shown one has codes for what's
  /// hidden in it.
  String actual(String key) {
    final value = rawDetails[key];
    if (value is String && value.isNotEmpty) return value;
    final written = rawInput?[key];
    return written is String ? written : '';
  }

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
    final partial = partialJsonString(call.input, key);
    if (partial == null) return null;
    return proseKeys.contains(key)
        ? proseCodes(partial)
        : visibleCodes(partial);
  }

  /// The list argument [key]; empty while it's being written.
  List<Object?> list(String key) => _list(input?[key]);

  static Map<String, Object?>? _raw(String json) {
    if (json.trim().isEmpty) return null;
    try {
      final decoded = jsonDecode(json);
      return decoded is Map<String, Object?> ? decoded : null;
    } on FormatException {
      return null;
    }
  }

  static Map<String, Object?>? _shown(Map<String, Object?>? raw) =>
      raw == null ? null : visibleCodesIn(raw) as Map<String, Object?>;
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
  'agents_suggest_talk': ToolView(
    icon: 'message-square',
    line: (context, call) => _SuggestTalkLine(call),
    body: (context, call) =>
        _suggestTalk(call).message.isEmpty && _suggestTalk(call).reason.isEmpty
        ? null
        : TalkCard(call),
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
  'image_generate': ToolView(icon: 'image', line: _imageLine, body: _imageBody),
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

// ---------------------------------------------------------------------------
// Pieces the views are built from.

/// Whether a find went to this entry (the [FindHighlight] above it is
/// current, with a query), and, given [text], whether the query is in it.
bool findWentHere(BuildContext context, [String? text]) {
  final find = context.dependOnInheritedWidgetOfExactType<FindHighlight>();
  if (find == null || !find.current || find.query.isEmpty) return false;
  return text == null || text.toLowerCase().contains(find.query.toLowerCase());
}

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
      FindHighlight.mark(
        context,
        TextSpan(
          children: [
            TextSpan(text: lead),
            if (subject.isNotEmpty)
              TextSpan(
                text: lead.isEmpty ? subject : ' $subject',
                style:
                    (code
                            ? TypeScale.of(context).code
                            : TypeScale.of(context).body)
                        .copyWith(color: palette.text),
              ),
            if (after.isNotEmpty)
              TextSpan(
                text: '  $after',
                style: TypeScale.of(context).caption
                    .copyWith(color: palette.textFaint),
              ),
          ],
        ),
      ),
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: TypeScale.of(context).body.copyWith(color: palette.textMuted),
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
    style: TypeScale.of(context).caption
        .copyWith(color: color ?? Palette.of(context).textFaint),
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
      builder: (context, hovered) => Text.rich(
        FindHighlight.markText(
          context,
          text,
          TypeScale.of(context).body.copyWith(
            color: color,
            decoration: hovered && onTap != null
                ? TextDecoration.underline
                : null,
            decorationColor: color,
          ),
        ),
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
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
                style: TypeScale.of(context).label.copyWith(
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

  /// Its height cut, in pixels at the standard text size: four lines of
  /// body text by default. It grows with the person's text size, so larger
  /// text still shows as many lines (Zucchero's row 140).
  final double height;

  @override
  State<ShowMore> createState() => _ShowMoreState();
}

class _ShowMoreState extends State<ShowMore> {
  var _whole = false;
  var _taller = false;

  /// Whether a find had gone to its entry at the last build.
  var _found = false;

  @override
  Widget build(BuildContext context) {
    // A find going to its entry shows it whole, once; the person's own
    // Show less wins after.
    final found = findWentHere(context);
    if (found && !_found) _whole = true;
    _found = found;
    final scaler = MediaQuery.textScalerOf(context);
    final height = scaler.scale(widget.height);
    final fade = scaler.scale(18);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _Clamped(
          height: height,
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
                      (height - fade) / bounds.height,
                      height / bounds.height,
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
            child: _Toggle(
              top: 2,
              label: _whole ? 'Show less' : 'Show more',
              onTap: () => setState(() => _whole = !_whole),
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
    // A pixel's leeway: text lays out to whole pixels, so three lines at
    // a larger size measure a little over three times one.
    final taller = child.size.height > _height + 1;
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
  Widget build(BuildContext context) => Text.rich(
    FindHighlight.markText(
      context,
      text,
      TypeScale.of(context).caption
          .copyWith(color: color ?? Palette.of(context).textMuted),
    ),
    maxLines: lines,
    overflow: TextOverflow.ellipsis,
  );
}

Widget _rows(List<Widget> rows) =>
    Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: rows);

/// "1 item", "3 items".
String _count(int count, String one, [String? many]) =>
    '$count ${count == 1 ? one : (many ?? '${one}s')}';

// ---------------------------------------------------------------------------
// Messaging.

/// One agent an agents_send call is for, and how it went.
typedef _Recipient = ({
  String id,
  String name,
  String role,
  String outcome,
  String why,
});

/// Whom [call] messaged: from its details' recipients, its single-send
/// details, or else its input (`to`, a string or a list), resolved against
/// the agents the app knows.
List<_Recipient> _recipients(BuildContext context, ToolCallInfo call) {
  final details = call.details;
  final listed = _list(details['recipients']).map(_map).toList();
  if (listed.isNotEmpty) {
    return [
      for (final r in listed)
        (
          id: _string(r['id']),
          name: _string(r['name']).ifEmpty(_string(r['to'])),
          role: _string(r['role']),
          outcome: _string(r['outcome']),
          why: _string(r['why']),
        ),
    ];
  }
  if (_string(details['to']).isNotEmpty) {
    return [
      (
        id: _string(details['to']),
        name: _string(details['name']),
        role: _string(details['role']),
        outcome: details['working'] == true ? 'working' : 'sent',
        why: '',
      ),
    ];
  }
  final self = ToolViewScope.of(context)?.agentId ?? '';
  var to = call.input?['to'];
  // An agent on the old schema writes a list as a string holding JSON.
  if (to is String && to.trim().startsWith('[')) {
    try {
      to = jsonDecode(to);
    } on FormatException {
      // Not a list after all: a name.
    }
  }
  final asked = to is List
      ? to.map(_string).toList()
      : [call.argument('to') ?? ''];
  return [
    for (final name in asked)
      if (name.trim().isNotEmpty)
        if (resolveAgent(context, name, self: self) case final agent?)
          (
            id: agent.id,
            name: agent.name,
            role: agent.role,
            outcome: '',
            why: '',
          )
        else
          (id: '', name: name, role: '', outcome: '', why: ''),
  ];
}

/// At most this many recipients are named on the line; more, and it says
/// how many, with each under it.
const recipientsOnLine = 3;

/// agents_send: "→" and whom it went to, each a link to its conversation,
/// then Sent or Queued.
class _SendLine extends StatelessWidget {
  const _SendLine(this.call);

  final ToolCallInfo call;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final all = _recipients(context, call);
    final reached = [
      for (final r in all)
        if (r.outcome != 'failed') r,
    ];
    final String result;
    if (!call.done || reached.isEmpty || reached.first.outcome.isEmpty) {
      result = '';
    } else if (call.details['quiet'] == true) {
      // Read whenever each next works: when doesn't matter, so one word.
      result = 'Sent quietly';
    } else if (reached.every((r) => r.outcome == 'working')) {
      result = 'Queued';
    } else {
      result = 'Sent';
    }
    final single = all.length == 1;
    Widget who(_Recipient r) => r.id.isEmpty
        ? ToolLine('', subject: r.name)
        : ToolAgent(
            id: r.id,
            name: r.name,
            // Its role then, or now when the call didn't say.
            role: !single ? '' : (r.role.isEmpty ? null : r.role),
          );
    final words = TypeScale.of(context).body.copyWith(color: palette.textMuted);
    return Row(
      children: [
        Text('→', style: words),
        const SizedBox(width: Space.s),
        if (reached.length > recipientsOnLine)
          Flexible(child: ToolLine('', subject: '${reached.length} agents'))
        else
          for (var i = 0; i < reached.length; i++) ...[
            if (i > 0) Text(', ', style: words),
            Flexible(child: who(reached[i])),
          ],
        if (reached.isEmpty && all.isNotEmpty)
          Flexible(
            child: ToolLine('', subject: all.map((r) => r.name).join(', ')),
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
  final all = _recipients(context, call);
  final reached = [
    for (final r in all)
      if (r.outcome != 'failed') r,
  ];
  final failed = [
    for (final r in all)
      if (r.outcome == 'failed') r,
  ];
  final text = (call.argument('text') ?? _string(call.details['text'])).trim();
  final rows = <Widget>[
    // Too many to name on the line: each here.
    if (reached.length > recipientsOnLine)
      Padding(
        padding: const EdgeInsets.only(bottom: 2),
        child: Wrap(
          spacing: Space.s,
          runSpacing: 2,
          children: [
            for (final r in reached)
              r.id.isEmpty
                  ? ToolLine('', subject: r.name)
                  : ToolAgent(id: r.id, name: r.name, role: ''),
          ],
        ),
      ),
    for (final r in failed)
      _Faint(
        r.why.isEmpty
            ? 'Not sent to ${r.name}'
            : 'Not sent to ${r.name}: ${r.why}',
        lines: 2,
        color: Palette.of(context).textFaint,
      ),
    if (text.isNotEmpty) FirstLine(text),
  ];
  return rows.isEmpty ? null : _rows(rows);
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

  /// Whether a find had gone to a match in it at the last build.
  var _found = false;

  /// The first line with something on it, without its Markdown.
  static String _first(String text) {
    for (final line in text.split('\n')) {
      final plain =
          md.Document(
                extensionSet: md.ExtensionSet.gitHubFlavored,
                encodeHtml: false,
              )
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
    // A find going to a match in it opens it, once.
    final found = findWentHere(context, widget.text);
    if (found && !_found) _open = true;
    _found = found;
    Widget toggle(String label) =>
        _Toggle(label: label, onTap: () => setState(() => _open = !_open));
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
    final style = TypeScale.of(context).body.copyWith(color: palette.textMuted);
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
              child: Text.rich(
                FindHighlight.markText(context, first, style),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
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

/// Whom an agents_suggest_talk call suggests the person talk to, and what
/// to say: from its details, else its input.
typedef _SuggestTalk = ({
  String id,
  String name,
  String message,
  String reason,
});

_SuggestTalk _suggestTalk(ToolCallInfo call) {
  final details = call.details;
  return (
    id: _string(details['agent_id']),
    name: _string(details['agent_name']).ifEmpty(call.argument('agent') ?? ''),
    message: _string(details['message'])
        .ifEmpty(call.argument('message') ?? '')
        .trim(),
    reason: _string(details['reason'])
        .ifEmpty(call.argument('reason') ?? '')
        .trim(),
  );
}

class _SuggestTalkLine extends StatelessWidget {
  const _SuggestTalkLine(this.call);

  final ToolCallInfo call;

  @override
  Widget build(BuildContext context) {
    final talk = _suggestTalk(call);
    return Row(
      children: [
        const ToolLine('Suggests you talk to'),
        const SizedBox(width: Space.s),
        Flexible(
          child: talk.id.isEmpty
              ? ToolLine('', subject: talk.name)
              : ToolAgent(id: talk.id, name: talk.name, role: ''),
        ),
      ],
    );
  }
}

/// An agent's suggestion that the person talk to another agent: why, the
/// message it drafted, and "Talk to Arjun", in that agent's color, which
/// opens its conversation with the message in its message box, not sent.
class TalkCard extends StatelessWidget {
  const TalkCard(this.call, {super.key});

  final ToolCallInfo call;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final talk = _suggestTalk(call);
    final tints = AgentTints.of(context);
    // Without details, the name as written, matched as agents_send matches.
    final agent =
        tints?.agents[talk.id] ??
        (talk.id.isNotEmpty
            ? null
            : tints?.agents.values
                  .where(
                    (a) =>
                        a.id == talk.name ||
                        a.name.toLowerCase() == talk.name.toLowerCase(),
                  )
                  .firstOrNull);
    final open = tints?.open;
    final name = agent?.name ?? talk.name;
    final rows = <Widget>[
      if (talk.reason.isNotEmpty)
        Text.rich(
          FindHighlight.markText(
            context,
            talk.reason,
            TypeScale.of(context).body.copyWith(color: palette.text),
          ),
        ),
      if (talk.message.isNotEmpty) ...[
        const SizedBox(height: Space.xs),
        FirstLine(talk.message),
      ],
    ];
    if (call.done) {
      rows.add(const SizedBox(height: Space.s));
      if (agent == null || open == null) {
        rows.add(
          _Faint(
            talk.id.isEmpty
                ? "Claktory can't find $name."
                : '$name was deleted.',
            color: palette.textFaint,
          ),
        );
      } else {
        rows.add(
          Align(
            alignment: Alignment.centerLeft,
            child: _TalkButton(
              agent: agent,
              onTap: () {
                final drafts = AppScope.scope(context).drafts;
                // Once: a second click only opens it.
                if (!drafts.of(agent.id).contains(talk.message)) {
                  drafts.compose(agent.id, talk.message);
                }
                open(agent.id);
              },
            ),
          ),
        );
      }
    }
    return _rows(rows);
  }
}

/// "Show more" or "Show less" under something cut short: small print on
/// a Mac; on a phone a finger's height, still reading as small print.
class _Toggle extends StatelessWidget {
  const _Toggle({required this.label, required this.onTap, this.top = 0});

  final String label;
  final VoidCallback onTap;

  /// Space over it on a Mac.
  final double top;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = isPhoneLayout(context);
    return Semantics(
      container: true,
      button: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => Container(
          constraints: BoxConstraints(minHeight: phone ? Touch.target : 0),
          alignment: phone ? AlignmentDirectional.centerStart : null,
          padding: EdgeInsets.only(top: phone ? 0 : top),
          child: Text(
            label,
            style: TypeScale.of(context).caption
                .copyWith(color: hovered ? palette.text : palette.textMuted),
          ),
        ),
      ),
    );
  }
}

/// "Talk to Arjun": the agent's resting character and its name on a tint
/// of its color, stronger while hovered.
class _TalkButton extends StatelessWidget {
  const _TalkButton({required this.agent, required this.onTap});

  final Agent agent;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final color = agentColor(agent.color);
    final label = 'Talk to ${agent.name}';
    return Semantics(
      button: true,
      label: label,
      excludeSemantics: true,
      child: Unselectable(
        child: Pressable(
          onTap: onTap,
          builder: (context, hovered) => AnimatedContainer(
            duration: Motion.fast,
            // A finger's height on a phone; both grow with larger text
            // rather than cutting it (Zucchero's row 140, iPad at AX5).
            constraints: BoxConstraints(
              minHeight: isPhoneLayout(context) ? Touch.target : 28,
            ),
            padding: const EdgeInsets.symmetric(horizontal: Space.m),
            decoration: BoxDecoration(
              color: color.withValues(alpha: hovered ? 0.28 : 0.16),
              border: Border.all(color: color.withValues(alpha: 0.5)),
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                AgentCharacter(agent: agent, size: 14),
                const SizedBox(width: Space.s),
                Text(
                  label,
                  style: TypeScale.of(context).label.copyWith(
                    color: palette.text,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
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
  'parked' => ChecklistStatus.CHECKLIST_STATUS_PARKED,
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
          style: TypeScale.of(context).caption
              .copyWith(color: palette.textMuted),
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
          Text.rich(
            FindHighlight.mark(context, TextSpan(children: words)),
            style: TypeScale.of(context).body,
          ),
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
  final output = proseCodes(call.call.output);
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
  if (spec['pipes'] != null) return _pipesWhen(spec['pipes'], spec['on']);
  return '';
}

/// A pipes monitor's `when` (friend bug #34): which pipes ("*", a name or
/// id, or a list) and on which changes (all when left out), in words:
/// "when pipe Website opens or closes", "when any pipe changes".
String _pipesWhen(Object? pipes, Object? on) {
  final named = [
    for (final pipe in pipes is List ? pipes : [pipes])
      if (_string(pipe).trim() case final name when name.isNotEmpty)
        if (name != '*') name,
  ];
  final any = named.isEmpty || _list(pipes).contains('*') || pipes == '*';
  final which = any ? 'any pipe' : 'pipe ${_or(named)}';
  final changes = [
    for (final change in on is List ? on : [?on])
      switch (_string(change)) {
        'opened' => 'opens',
        'closed' => 'closes',
        'paused' => 'is paused',
        'resumed' => 'is resumed',
        'shared' => 'has a website shared',
        'unshared' => 'has a website stopped',
        'online' => 'comes online',
        'offline' => 'goes offline',
        final other => other,
      },
  ];
  return 'when $which ${changes.isEmpty ? 'changes' : _or(changes)}';
}

/// "a", "a or b", "a, b or c".
String _or(List<String> items) => items.length < 2
    ? items.join()
    : '${items.sublist(0, items.length - 1).join(', ')} or ${items.last}';

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
          style: TypeScale.of(context).code.copyWith(color: palette.textMuted),
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
    // Shown with codes for what's hidden in it; opened and shared as it is.
    final path = _string(call.details['path']).ifEmpty(call.argument('path'));
    final actual = call.actual('path');
    final slash = path.lastIndexOf('/');
    final name = slash < 0 ? path : path.substring(slash + 1);
    final folder = slash <= 0 ? '' : shortPath(path.substring(0, slash));
    final palette = Palette.of(context);
    return Row(
      children: [
        Text(
          call.done ? 'Showing' : 'Opening',
          style: TypeScale.of(context).body.copyWith(color: palette.textMuted),
        ),
        const SizedBox(width: Space.xs),
        // The file's name first; its folder gives way before it does (a
        // narrow pane, Abbacchio's row 544, showed "Showing" and no name).
        Expanded(
          child: LayoutBuilder(
            builder: (context, constraints) => Row(
              children: [
                ConstrainedBox(
                  constraints: BoxConstraints(maxWidth: constraints.maxWidth),
                  // Touched and held: Share… (#418).
                  child: ShareHold(
                    path: call.done ? actual : '',
                    child: _Link(
                      text: name,
                      onTap: call.done && actual.isNotEmpty
                          ? () => _focus(context, call, actual)
                          : null,
                    ),
                  ),
                ),
                if (folder.isNotEmpty)
                  Flexible(
                    child: Padding(
                      padding: const EdgeInsets.only(left: Space.s),
                      child: _Trailing(folder),
                    ),
                  ),
              ],
            ),
          ),
        ),
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
    // The pane it opened in, while that still shows this file: a later
    // show in place of the oldest pane keeps the pane's id for another file
    // or page (Zucchero's row 136: Needs you came forward). Else the pane
    // that shows it now, or a new one.
    final pane = _string(call.details['pane']);
    final kept = pane.isEmpty ? null : find(root, pane);
    final shown = kept != null && kept.hasLeaf() && kept.leaf.path == path
        ? (root: root, paneId: pane)
        : open(root, path);
    // A phone shows no panes beside the conversation: the file as a page
    // over it, as Files in ⋯ opens it (Zucchero's #379 find), put in the
    // layout first if it isn't there.
    final phone = context
        .getInheritedWidgetOfExactType<PhoneRoutesScope>()
        ?.routes;
    if (phone != null &&
        (identical(shown.root, root)
            ? phone.showPane(agent.id, shown.paneId)
            : phone.showFile(agent.id, path))) {
      return;
    }
    if (identical(shown.root, root)) {
      final key = 'pane:${agent.id}:${shown.paneId}';
      // Brought forward even with highlights off (an iPad's hidden pane);
      // it glows when they're on.
      backend.highlights.pointedAt.value = (keys: [key], at: DateTime.now());
      backend.highlights.glow(key, [agentColor(agent.color)]);
      return;
    }
    fireAndLog(backend.setLayout(agent.id, shown.root));
  }
}

// ---------------------------------------------------------------------------
// Images (#403).

/// `image_generate`: what it was asked for, and the picture it made.
Widget _imageLine(BuildContext context, ToolCallInfo call) => ToolLine(
  call.running
      ? 'Making an image'
      : call.failed
      ? "Couldn't make an image"
      : 'Made an image',
  subject: (call.argument('prompt') ?? '').replaceAll(RegExp(r'\s+'), ' '),
);

/// While it works, how long it can take; once made, the picture. A refusal
/// is the call's output, which every failed view shows under its line.
Widget? _imageBody(BuildContext context, ToolCallInfo call) {
  if (call.running) {
    final palette = Palette.of(context);
    return Text(
      'It can take up to 2 minutes.',
      style: TypeScale.of(context).caption.copyWith(color: palette.textFaint),
    );
  }
  // The file itself, every character as it is (Zucchero's row 343).
  final path = call.actual('path');
  if (!call.done || path.isEmpty) return null;
  return Align(
    alignment: AlignmentDirectional.centerStart,
    // Touched and held: Share… (#418), to save it to Photos say.
    child: ShareHold(
      path: path,
      child: MadeImage(
        path: path,
        width: _int(call.details['width']) ?? 0,
        height: _int(call.details['height']) ?? 0,
        label: _string(call.details['prompt']).ifEmpty(call.argument('prompt')),
        agentId: ToolViewScope.of(context)?.agentId ?? '',
      ),
    ),
  );
}

/// A picture an agent made, at most [side] points either way, the room
/// for it kept from its size before it comes; a click or tap opens it in
/// the image viewer. It's read as a preview made for its size (#379), so a
/// phone doesn't fetch the whole PNG to show a thumbnail.
class MadeImage extends StatefulWidget {
  const MadeImage({
    super.key,
    required this.path,
    required this.width,
    required this.height,
    required this.label,
    required this.agentId,
  });

  final String path;

  /// Its pixels, from the call's details; 0 when they don't say.
  final int width;
  final int height;

  /// What it shows, for VoiceOver: the prompt.
  final String label;

  /// Whose conversation it's in, whose panes it opens in.
  final String agentId;

  /// The most it takes either way.
  static const side = 320.0;

  /// Pictures read, by path, the latest last: a conversation scrolled back
  /// and forth doesn't read them again.
  static final _cache = <String, Uint8List>{};
  static const _kept = 40;

  @override
  State<MadeImage> createState() => _MadeImageState();
}

class _MadeImageState extends State<MadeImage> {
  Uint8List? _bytes;
  String? _error;

  @override
  void initState() {
    super.initState();
    final cached = MadeImage._cache.remove(widget.path);
    if (cached != null) {
      MadeImage._cache[widget.path] = cached;
      _bytes = cached;
    }
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_bytes == null && _error == null && !_reading) {
      _reading = true;
      unawaited(_read(MediaQuery.devicePixelRatioOf(context)));
    }
  }

  var _reading = false;

  Future<void> _read(double ratio) async {
    final side = (MadeImage.side * ratio).round();
    final PreviewBox box = (width: side, height: side);
    try {
      final content = await AppScope.of(context)
          .readFile(widget.path, preview: box);
      if (content.error.isNotEmpty) throw RequestFailure(content.error);
      if (content.truncated) {
        throw const RequestFailure("This image is too big to show here.");
      }
      final bytes = Uint8List.fromList(content.data);
      MadeImage._cache[widget.path] = bytes;
      while (MadeImage._cache.length > MadeImage._kept) {
        MadeImage._cache.remove(MadeImage._cache.keys.first);
      }
      if (mounted) setState(() => _bytes = bytes);
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(
          () => _error = failure.message.ifEmpty("Couldn't load the image."),
        );
      }
    }
  }

  void _open() {
    final scope = context.getInheritedWidgetOfExactType<AppScope>();
    final agent = scope?.backend.workspace.agent(widget.agentId);
    if (scope == null || agent == null) {
      openFile(context, widget.path);
      return;
    }
    final backend = scope.backend;
    final phone = context
        .getInheritedWidgetOfExactType<PhoneRoutesScope>()
        ?.routes;
    if (phone != null && phone.showFile(agent.id, widget.path)) return;
    final root = layoutOf(agent);
    final shown = open(root, widget.path);
    if (identical(shown.root, root)) {
      final key = 'pane:${agent.id}:${shown.paneId}';
      backend.highlights.pointedAt.value = (keys: [key], at: DateTime.now());
      backend.highlights.glow(key, [agentColor(agent.color)]);
      return;
    }
    fireAndLog(backend.setLayout(agent.id, shown.root));
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final error = _error;
    if (error != null) {
      return Text(
        error,
        style: TypeScale.of(context).caption.copyWith(color: palette.textMuted),
      );
    }
    final bytes = _bytes;
    final ratio = widget.width > 0 && widget.height > 0
        ? widget.width / widget.height
        : 1.0;
    // Fitted in side × side, its shape kept.
    final width = ratio >= 1 ? MadeImage.side : MadeImage.side * ratio;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: Space.xs),
      child: Semantics(
        image: true,
        button: true,
        label: widget.label,
        excludeSemantics: true,
        child: Pressable(
          onTap: _open,
          builder: (context, hovered) => ConstrainedBox(
            constraints: BoxConstraints(maxWidth: width),
            child: AspectRatio(
              aspectRatio: ratio,
              child: Container(
                decoration: BoxDecoration(
                  color: palette.sidebar,
                  borderRadius: BorderRadius.circular(Radii.panel),
                  border: Border.all(color: palette.hairline),
                ),
                clipBehavior: Clip.antiAlias,
                child: bytes == null
                    ? const SizedBox.expand()
                    : Image.memory(
                        bytes,
                        fit: BoxFit.cover,
                        gaplessPlayback: true,
                        errorBuilder: (context, error, stack) => Center(
                          child: Text(
                            "Couldn't show the image.",
                            style: TypeScale.of(context).caption
                                .copyWith(color: palette.textMuted),
                          ),
                        ),
                      ),
              ),
            ),
          ),
        ),
      ),
    );
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
            color: palette.link,
            onTap: () {
              final url = Uri.tryParse(_string(hit['url']));
              if (url != null) openLink(context, url);
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
          style: TypeScale.of(context).body.copyWith(color: palette.textMuted),
        ),
        const SizedBox(width: Space.xs),
        Flexible(
          child: _Link(
            text: title.ifEmpty(url),
            color: link == null || !link.hasScheme ? null : palette.link,
            onTap: link == null || !link.hasScheme
                ? null
                : () => openLink(context, link),
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
    height:
        3 *
        TypeScale.of(context).code.fontSize! *
        TypeScale.of(context).code.height!,
    child: Text.rich(
      FindHighlight.markText(
        context,
        sql,
        TypeScale.of(context).code
            .copyWith(color: Palette.of(context).textMuted),
      ),
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
    final words = TypeScale.of(context).body.copyWith(color: palette.textMuted);
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
                style: TypeScale.of(context).code
                    .copyWith(color: palette.textFaint),
              ),
            ],
          ),
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: TypeScale.of(context).body,
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
