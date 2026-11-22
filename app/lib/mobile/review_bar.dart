import 'dart:async';

import 'package:flutter/services.dart' show TextInputAction;
import 'package:flutter/widgets.dart';

import '../backend.dart';
import '../characters/character.dart';
import '../controls.dart';
import '../identity.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../scope.dart';
import 'drawer.dart' show largeText;
import 'haptics.dart';
import 'input.dart';
import 'metrics.dart' show TypeScale;
import 'presentation.dart';

/// An agent's question asking the person to review [path] (ask_person with
/// a review path, #275): its seq and what it asks.
typedef ReviewAsk = ({int seq, AskPerson ask});

/// At the foot of a file's viewer on a phone, while [agentId] waits for
/// the person's review of it: who asks and what, then Dismiss and
/// Approve at thumb reach. Approve opens a sheet for an optional note.
/// Comments on the file go to the agent as on any viewer; this answers the
/// question. Nothing shows when no question waits on the file.
class PhoneReviewBar extends StatefulWidget {
  const PhoneReviewBar({super.key, required this.agentId, required this.path});

  final String agentId;
  final String path;

  @override
  State<PhoneReviewBar> createState() => _PhoneReviewBarState();
}

class _PhoneReviewBarState extends State<PhoneReviewBar> {
  Backend? _backend;

  /// The agent's waiting entries fetched so far, by seq.
  final _entries = <int, Entry>{};
  final _fetching = <int>{};

  var _busy = false;
  String? _error;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final backend = AppScope.of(context);
    if (backend == _backend) return;
    _backend?.workspace.removeListener(_changed);
    _backend = backend..workspace.addListener(_changed);
    _entries.clear();
    _fetching.clear();
    _fetch();
  }

  @override
  void didUpdateWidget(PhoneReviewBar old) {
    super.didUpdateWidget(old);
    if (old.agentId == widget.agentId) return;
    _entries.clear();
    _fetching.clear();
    _fetch();
  }

  @override
  void dispose() {
    _backend?.workspace.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    if (!mounted) return;
    setState(() {});
    _fetch();
  }

  List<int> get _waiting => [
    for (final seq
        in _backend?.workspace.agent(widget.agentId)?.waitingSeqs ?? const [])
      seq.toInt(),
  ];

  /// Fetches the waiting entries not seen yet: one of them may be the
  /// question about this file.
  void _fetch() {
    final backend = _backend;
    if (backend == null) return;
    final agentId = widget.agentId;
    for (final seq in _waiting) {
      if (_entries.containsKey(seq) || !_fetching.add(seq)) continue;
      unawaited(() async {
        try {
          final entry = await backend.fetchEntry(agentId, seq);
          if (entry != null && mounted && agentId == widget.agentId) {
            setState(() => _entries[seq] = entry);
          }
        } on RequestFailure {
          // Tried again when the workspace next changes.
        } finally {
          _fetching.remove(seq);
        }
      }());
    }
  }

  /// The oldest question still waiting on this file.
  ReviewAsk? get _ask {
    for (final seq in _waiting) {
      final entry = _entries[seq];
      if (entry == null || entry.whichKind() != Entry_Kind.askPerson) {
        continue;
      }
      final ask = entry.askPerson;
      if (ask.reviewPath == widget.path &&
          ask.state == AskPersonState.ASK_PERSON_STATE_PENDING) {
        return (seq: seq, ask: ask);
      }
    }
    return null;
  }

  Future<bool> _answer(
    ReviewAsk ask, {
    String note = '',
    bool dismiss = false,
  }) async {
    if (_busy) return false;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await _backend!.answerAsk(
        widget.agentId,
        ask.seq,
        text: note.trim(),
        dismiss: dismiss,
      );
      unawaited(dismiss ? Haptics.light() : Haptics.success());
      return true;
    } on RequestFailure catch (failure) {
      unawaited(Haptics.warning());
      if (mounted) setState(() => _error = failure.message);
      return false;
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// Approve's or Dismiss's sheet: either way the person may say why, and
  /// the agent gets it with the answer (Matt's rule).
  Future<void> _decide(ReviewAsk ask, String name, {required bool dismiss}) =>
      presentSheet<void>(
        context,
        title: dismiss ? 'Dismiss' : 'Approve',
        builder: (context, close) => _DecisionNote(
          name: name,
          action: dismiss ? 'Dismiss' : 'Approve',
          primary: !dismiss,
          decide: (note) async {
            final ok = await _answer(ask, note: note, dismiss: dismiss);
            if (ok) close();
            return ok;
          },
          error: () => _error,
        ),
      );

  @override
  Widget build(BuildContext context) {
    final ask = _ask;
    final agent = _backend?.workspace.agent(widget.agentId);
    if (ask == null || agent == null) return const SizedBox.shrink();
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final dismiss = Button(
      label: 'Dismiss',
      onPressed: _busy
          ? null
          : () => unawaited(_decide(ask, agent.name, dismiss: true)),
    );
    final approve = Button(
      label: 'Approve',
      primary: true,
      onPressed: _busy
          ? null
          : () => unawaited(_decide(ask, agent.name, dismiss: false)),
    );
    final stacked = largeText(context);
    // A bar over the file, as iOS's toolbars: it grows with the text only
    // so far, so the file keeps its room at the largest sizes.
    return MediaQuery.withClampedTextScaling(
      maxScaleFactor: 2,
      child: Semantics(
        container: true,
        explicitChildNodes: true,
        child: Container(
          decoration: BoxDecoration(
            color: palette.canvas,
            border: Border(top: BorderSide(color: palette.hairline)),
          ),
          padding: const EdgeInsets.fromLTRB(
            Space.m,
            Space.s,
            Space.m,
            Space.s,
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              // At the largest sizes the question alone, so the file keeps
              // some room above it.
              if (!stacked || ask.ask.title.isEmpty)
                Row(
                  children: [
                    ExcludeSemantics(
                      child: AgentCharacter(agent: agent, size: 20),
                    ),
                    const SizedBox(width: Space.s),
                    Expanded(
                      child: Text(
                        '${agent.name} asks you to review this',
                        maxLines: stacked ? 1 : 2,
                        overflow: TextOverflow.ellipsis,
                        style: type.secondary.copyWith(
                          color: palette.textMuted,
                        ),
                      ),
                    ),
                  ],
                ),
              if (ask.ask.title.isNotEmpty) ...[
                if (!stacked) const SizedBox(height: Space.xs),
                Text(
                  ask.ask.title,
                  maxLines: stacked ? 2 : 3,
                  overflow: TextOverflow.ellipsis,
                  semanticsLabel: stacked
                      ? '${agent.name} asks you to review this: '
                            '${ask.ask.title}'
                      : null,
                  style: type.body.copyWith(color: palette.text),
                ),
              ],
              if (_error case final error?) ...[
                const SizedBox(height: Space.xs),
                Text(
                  error,
                  style: type.secondary.copyWith(color: palette.danger),
                ),
              ],
              const SizedBox(height: Space.s),
              if (stacked) ...[
                approve,
                const SizedBox(height: Space.s),
                dismiss,
              ] else
                Row(
                  children: [
                    Expanded(child: dismiss),
                    const SizedBox(width: Space.s),
                    // The primary action last, under the thumb.
                    Expanded(child: approve),
                  ],
                ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Approve's or Dismiss's sheet: a comment for the agent, if the person
/// has one, and the button that answers.
class _DecisionNote extends StatefulWidget {
  const _DecisionNote({
    required this.name,
    required this.action,
    required this.primary,
    required this.decide,
    required this.error,
  });

  final String name;
  final String action;
  final bool primary;
  final Future<bool> Function(String note) decide;
  final String? Function() error;

  @override
  State<_DecisionNote> createState() => _DecisionNoteState();
}

class _DecisionNoteState extends State<_DecisionNote> {
  final _note = TextEditingController();
  var _busy = false;
  String? _error;

  @override
  void dispose() {
    _note.dispose();
    super.dispose();
  }

  Future<void> _submit() async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    final ok = await widget.decide(_note.text);
    if (!mounted || ok) return;
    setState(() {
      _busy = false;
      _error = widget.error();
    });
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return SingleChildScrollView(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          TextField(
            controller: _note,
            placeholder: 'A comment for ${widget.name}, if you like',
            kind: InputKind.message,
            textInputAction: TextInputAction.send,
            onSubmitted: (_) => unawaited(_submit()),
            minLines: 2,
            maxLines: 6,
          ),
          const SizedBox(height: Space.xs),
          Text(
            '${widget.name} gets it with your answer.',
            style: type.caption.copyWith(color: palette.textFaint),
          ),
          if (_error case final error?)
            Padding(
              padding: const EdgeInsets.only(top: Space.s),
              child: Text(
                error,
                style: type.secondary.copyWith(color: palette.danger),
              ),
            ),
          const SizedBox(height: Space.m),
          Button(
            label: widget.action,
            primary: widget.primary,
            onPressed: _busy ? null : () => unawaited(_submit()),
          ),
        ],
      ),
    );
  }
}
