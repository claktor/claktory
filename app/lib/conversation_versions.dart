import 'dart:async';
import 'dart:io';

import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'drafts.dart';
import 'forms.dart';
import 'identity.dart';
import 'message_time.dart' show clockTime;
import 'mobile/metrics.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'state.dart';

// Going back in an agent's conversation, and its other versions (friend
// bug #9, research conversation-fork.md §7). "Go back to here…" in a
// message's menu takes the conversation back to just before that message
// (or, on one of the agent's replies, to the end of the run it belongs
// to); what came after is kept as another version, which the agent's
// "Other versions" lists to switch back to. The backend does all of it
// and says what it refuses; the app only asks first and, for the person's
// own message, puts it back in the message box.

/// Whether [entry], shown in [transcript], is a place its conversation can
/// go back to: the person's message or an event (not the conversation's
/// very first entry, which would keep nothing), or the agent's reply with
/// a message or an event after it (the cut comes there, so a reply with
/// nothing after it yet would set nothing aside).
bool canGoBackTo(TranscriptStore transcript, Entry entry) {
  if (transcript.agentId.isEmpty) return false;
  final boundary = entry.hasUser() || entry.hasEvent();
  if (!boundary && !entry.hasAssistant() && !entry.hasThinking()) {
    return false;
  }
  var at = -1;
  for (var i = transcript.length - 1; i >= 0; i--) {
    if (transcript[i].entry.seq == entry.seq) {
      at = i;
      break;
    }
  }
  if (at < 0) return false;
  if (boundary) return at > 0 || transcript.hasEarlier;
  for (var i = at + 1; i < transcript.length; i++) {
    final later = transcript[i].entry;
    if (later.hasUser() || later.hasEvent()) return true;
  }
  return false;
}

/// Asks before taking [transcript]'s agent's conversation back to
/// [entry], then does it; the person's own message goes back into the
/// message box, its files too.
Future<void> goBackTo(
  BuildContext context,
  TranscriptStore transcript,
  Entry entry,
) {
  final agentId = transcript.agentId;
  return showModal(
    context,
    (context, close) =>
        _GoBack(agentId: agentId, entry: entry.deepCopy(), close: close),
  );
}

class _GoBack extends StatefulWidget {
  const _GoBack({
    required this.agentId,
    required this.entry,
    required this.close,
  });

  final String agentId;
  final Entry entry;
  final VoidCallback close;

  @override
  State<_GoBack> createState() => _GoBackState();
}

class _GoBackState extends State<_GoBack> {
  var _busy = false;
  String? _error;

  Future<void> _go() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    final backend = AppScope.of(context);
    final drafts = AppScope.scope(context).drafts;
    final entry = widget.entry;
    try {
      await backend.forkConversation(
        widget.agentId,
        entry.seq,
        entry.timestampMs,
      );
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = failure.message;
        });
      }
      return;
    }
    if (entry.hasUser()) {
      putBack(backend, drafts, widget.agentId, entry.user);
    }
    widget.close();
  }

  @override
  Widget build(BuildContext context) {
    final agent = AppScope.of(context).workspace.agent(widget.agentId);
    final name = agent?.name ?? 'The agent';
    final working = agent?.status == AgentStatus.AGENT_STATUS_WORKING;
    return FormFrame(
      title: "Take $name's conversation back to here?",
      body: [
        "$name forgets what came after, but it's kept as another version "
            'you can switch back to. Files it changed and messages it sent '
            'stay as they are.',
        if (working) '$name stops first.',
        if (widget.entry.hasUser())
          'Your message goes back into the message box, so you can change '
              'it and send it again.',
      ].join(' '),
      fields: const [],
      error: _error,
      submitLabel: 'Go back',
      onSubmit: _busy ? null : _go,
      onCancel: widget.close,
    );
  }
}

/// Puts the person's [message] back into [agentId]'s message box, after
/// anything already typed there: its words at once, and its files as they
/// come back from Claktory (a voice message's words, without its
/// recording).
@visibleForTesting
void putBack(Backend backend, Drafts drafts, String agentId, UserText message) {
  final text = message.text;
  if (text.trim().isNotEmpty) {
    final draft = drafts.of(agentId);
    drafts.set(agentId, draft.trim().isEmpty ? text : '$draft\n\n$text');
  }
  final files = [
    for (final (i, file) in message.attachments.indexed)
      if (!(message.hasVoice() && i == 0)) file,
  ];
  if (files.isNotEmpty) unawaited(_fetchBack(backend, agentId, files));
}

/// Brings [files] from Claktory to this device and into [agentId]'s box,
/// each as it arrives; one that can't be read is left out.
Future<void> _fetchBack(
  Backend backend,
  String agentId,
  List<Attachment> files,
) async {
  final Directory folder;
  try {
    folder = await Directory.systemTemp.createTemp('claktory-back-');
  } on FileSystemException {
    return;
  }
  for (final file in files) {
    try {
      final out = File('${folder.path}/${file.name}');
      final sink = out.openWrite();
      var offset = 0;
      var ok = true;
      while (true) {
        final piece = await backend.readFile(file.path, offset: offset);
        if (piece.error.isNotEmpty) {
          ok = false;
          break;
        }
        sink.add(piece.data);
        offset += piece.data.length;
        if (piece.data.isEmpty || offset >= piece.size.toInt()) break;
      }
      await sink.close();
      if (ok) await backend.attachments.add(agentId, [out.path]);
    } on Object {
      continue;
    }
  }
}

/// The header's way to [agent]'s other versions, while it has any:
/// "Other versions (2)".
String versionsLabel(Agent agent) => 'Other versions (${agent.branches})';

/// Lists [agent]'s other versions of its conversation, to switch to one or
/// delete it.
Future<void> showVersions(BuildContext context, Agent agent) => showModal(
  context,
  (context, close) => _Versions(agentId: agent.id, close: close),
);

class _Versions extends StatefulWidget {
  const _Versions({required this.agentId, required this.close});

  final String agentId;
  final VoidCallback close;

  @override
  State<_Versions> createState() => _VersionsState();
}

/// What the versions sheet asks first: switching to [branch], or deleting
/// it.
typedef _Asking = ({Branch branch, bool delete});

class _VersionsState extends State<_Versions> {
  List<Branch>? _branches;
  String? _error;
  _Asking? _asking;
  var _busy = false;
  late final Backend _backend;

  /// The agent's count of versions as last seen, and a pause before
  /// asking again.
  var _count = -1;
  Timer? _again;

  @override
  void initState() {
    super.initState();
    _backend = AppScope.of(context);
    _count = _backend.workspace.agent(widget.agentId)?.branches ?? 0;
    // Another device or an agent's tool may go back or switch meanwhile:
    // the count changes (not on a switch, which sets one aside as it
    // brings one back), or the conversation comes again (Cioccolata).
    _backend.workspace.addListener(_workspaceChanged);
    _backend.transcript.addListener(_transcriptChanged);
    unawaited(_load());
  }

  @override
  void dispose() {
    _again?.cancel();
    _backend.workspace.removeListener(_workspaceChanged);
    _backend.transcript.removeListener(_transcriptChanged);
    super.dispose();
  }

  void _workspaceChanged() {
    final count = _backend.workspace.agent(widget.agentId)?.branches ?? 0;
    if (count == _count) return;
    _count = count;
    _askAgain();
  }

  void _transcriptChanged() {
    if (_backend.transcript.agentId == widget.agentId) _askAgain();
  }

  void _askAgain() {
    _again?.cancel();
    _again = Timer(const Duration(milliseconds: 300), () {
      if (mounted) unawaited(_load());
    });
  }

  Future<void> _load() async {
    try {
      final found = await AppScope.of(context).listBranches(widget.agentId);
      if (!mounted) return;
      setState(() {
        _branches = [...found.branches]
          ..sort((a, b) => b.setAsideMs.compareTo(a.setAsideMs));
        _error = null;
      });
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    }
  }

  Future<void> _do(_Asking asking) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    final backend = AppScope.of(context);
    try {
      if (asking.delete) {
        await backend.deleteBranch(widget.agentId, asking.branch.id);
      } else {
        await backend.switchBranch(widget.agentId, asking.branch.id);
      }
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = failure.message;
        });
      }
      return;
    }
    if (!mounted) return;
    if (!asking.delete) {
      widget.close();
      return;
    }
    setState(() {
      _busy = false;
      _asking = null;
      _branches?.removeWhere((branch) => branch.id == asking.branch.id);
    });
    unawaited(_load());
  }

  @override
  Widget build(BuildContext context) {
    final agent = AppScope.of(context).workspace.agent(widget.agentId);
    final name = agent?.name ?? 'The agent';
    final asking = _asking;
    if (asking != null) {
      void back() => setState(() {
        _asking = null;
        _error = null;
      });
      return asking.delete
          ? FormFrame(
              title: 'Delete this version?',
              body: "It can't be brought back.",
              fields: const [],
              error: _error,
              submitLabel: 'Delete',
              onSubmit: _busy ? null : () => _do(asking),
              onCancel: back,
            )
          : FormFrame(
              title: 'Switch $name to this version?',
              body: 'The conversation as it is now is kept as another version.',
              fields: const [],
              error: _error,
              submitLabel: 'Switch',
              onSubmit: _busy ? null : () => _do(asking),
              onCancel: back,
            );
    }
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final branches = _branches;
    final error = _error;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Semantics(
          header: true,
          namesRoute: true,
          child: Text(
            "$name's other versions",
            style: type.emphasis.copyWith(color: palette.text),
          ),
        ),
        const SizedBox(height: Space.m),
        Flexible(
          child: SingleChildScrollView(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (branches == null && error == null)
                  Text(
                    'Loading…',
                    style: type.body.copyWith(color: palette.textMuted),
                  )
                else if (branches != null && branches.isEmpty)
                  Text(
                    '$name has no other versions.',
                    style: type.body.copyWith(color: palette.textMuted),
                  ),
                for (final branch in branches ?? const <Branch>[])
                  _VersionRow(
                    branch: branch,
                    onSwitch: () => setState(
                      () => _asking = (branch: branch, delete: false),
                    ),
                    onDelete: () => setState(
                      () => _asking = (branch: branch, delete: true),
                    ),
                  ),
              ],
            ),
          ),
        ),
        if (error != null)
          Padding(
            padding: const EdgeInsets.only(top: Space.m),
            child: Text(
              error,
              style: type.secondary.copyWith(color: palette.danger),
            ),
          ),
        const SizedBox(height: Space.xl),
        Row(
          children: [
            const Spacer(),
            Button(label: 'Done', primary: true, onPressed: widget.close),
          ],
        ),
      ],
    );
  }
}

/// One other version: its label ("From 14:02" when it has none), when it
/// was set aside and how many messages it has past where it parts, then
/// Switch to this version and Delete….
class _VersionRow extends StatelessWidget {
  const _VersionRow({
    required this.branch,
    required this.onSwitch,
    required this.onDelete,
  });

  final Branch branch;
  final VoidCallback onSwitch;
  final VoidCallback onDelete;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final label = branch.label.isNotEmpty
        ? branch.label
        : 'From ${clockTime(branch.forkedAtMs.toInt())}';
    final count = branch.messages == 1
        ? '1 message'
        : '${branch.messages} messages';
    return Container(
      padding: const EdgeInsets.symmetric(vertical: Space.m),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: palette.hairline)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            label,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: type.body.copyWith(color: palette.text),
          ),
          const SizedBox(height: Space.xs),
          Text(
            'Set aside ${setAsideAt(branch.setAsideMs.toInt(), DateTime.now())}'
            ' · $count',
            style: type.secondary.copyWith(color: palette.textMuted),
          ),
          const SizedBox(height: Space.s),
          Wrap(
            spacing: Space.s,
            runSpacing: Space.s,
            children: [
              Button(label: 'Switch to this version', onPressed: onSwitch),
              Button(label: 'Delete…', onPressed: onDelete),
            ],
          ),
        ],
      ),
    );
  }
}

/// When a version was set aside: "at 15:31" today, "on Mon 17 Nov at
/// 15:31" before.
@visibleForTesting
String setAsideAt(int ms, DateTime now) {
  final at = DateTime.fromMillisecondsSinceEpoch(ms);
  final clock = clockTime(ms);
  if (at.year == now.year && at.month == now.month && at.day == now.day) {
    return 'at $clock';
  }
  const weekdays = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
  const months = [
    'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', //
    'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
  ];
  final year = at.year == now.year ? '' : ' ${at.year}';
  return 'on ${weekdays[at.weekday - 1]} ${at.day} ${months[at.month - 1]}'
      '$year at $clock';
}
