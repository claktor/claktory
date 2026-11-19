import 'dart:async';
import 'dart:convert';
import 'dart:math' as math;

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';

import 'agent_name.dart';
import 'backend.dart' show RequestFailure;
import 'characters/character.dart';
import 'controls.dart';
import 'extensions.dart' show logTime;
import 'forms.dart' show rivetIn;
import 'identity.dart';
import 'markdown.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'state.dart';
import 'tell_rivet.dart';
import 'selection.dart';

/// The selected agent's transcript.
///
/// Entries present when the transcript loads are laid out lazily upward from
/// a center line; entries that arrive later (and the last loaded one, which
/// may still be streaming) grow downward below it. Opening a long transcript
/// therefore lays out only what's on screen, and growth never shifts what's
/// above it. While the view is at the bottom it follows new content; once the
/// user scrolls up it stays put.
///
/// A new entry glides into view: the view scrolls to the new bottom over
/// [_glideDuration] while the entry fades and rises in. Text streaming into
/// an entry is followed instantly, so tokens never start an animation, and
/// a load lands at the bottom at once.
///
/// Key this widget by [TranscriptStore.loads] so each load starts fresh.
class TranscriptView extends StatefulWidget {
  const TranscriptView({
    super.key,
    required this.transcript,
    required this.working,
    required this.follow,
    this.queued,
    this.agentId = '',
  });

  final TranscriptStore transcript;

  /// Messages waiting for the agent, shown faded after the last entry until
  /// it takes them; [agentId] is whose.
  final QueuedStore? queued;
  final String agentId;

  /// Whether the agent is running; the last entry is then live.
  final bool working;

  /// Ticks when the view should jump to the bottom and follow again, such as
  /// after the user sends a message.
  final Listenable follow;

  @override
  State<TranscriptView> createState() => _TranscriptViewState();
}

/// How long the view takes to glide to a new entry.
const _glideDuration = Duration(milliseconds: 300);

/// How faded a queued message is.
const _queuedOpacity = 0.5;

/// How long a message that left the queue stays, waiting for its entry,
/// before it fades out (as it does when Stop clears the queue).
const _leaveGrace = Duration(milliseconds: 1500);

/// A queued message on show.
class _Waiting {
  _Waiting(this.message, this.shown);

  final QueuedMessage message;

  /// When it was first shown, to ease it in then.
  final DateTime shown;

  /// Running once it left the queue: then it fades out unless its entry
  /// arrives first.
  Timer? leaving;

  /// It left without its entry arriving, and is fading out.
  var gone = false;
}

class _TranscriptViewState extends State<TranscriptView>
    with SingleTickerProviderStateMixin {
  final _liveKey = GlobalKey();

  /// Whether the person has text selected here.
  var _selected = false;
  late final _controller = _FollowController(_liveExtent);
  late int _split;

  /// Moves the view during a glide, a frame at a time.
  late final _glide = createTicker(_glided);

  /// Where the glide started, and where it last put the view; any other
  /// position means the user scrolled, which ends it.
  double _glideFrom = 0;
  double? _glidePixels;

  /// How many entries the view has seen, to tell new ones from growth.
  late int _known;

  /// When entries that arrived after the load came in, by seq: they ease in
  /// if they're built right after, and never again.
  final _arrivals = <int, DateTime>{};

  /// Entries the user has expanded, so they stay open when scrolled away.
  final _expanded = <int>{};

  /// Queued messages shown after the last entry, oldest first, including
  /// ones that just left the queue while their entry is on its way.
  final _waiting = <_Waiting>[];

  /// Queued messages whose entry has arrived, so they're never shown again.
  final _delivered = <Int64>{};

  /// Entries that replaced a queued bubble, by seq: they brighten in place
  /// rather than arrive.
  final _fromQueue = <int>{};

  @override
  void initState() {
    super.initState();
    _split = math.max(0, widget.transcript.length - 1);
    _known = widget.transcript.length;
    _controller.addListener(_scrolled);
    widget.follow.addListener(_followAgain);
    widget.transcript.addListener(_entriesChanged);
    widget.queued?.addListener(_queueChanged);
    _queueChanged(initial: true);
  }

  /// The queue changed: show what's new, and note what left it.
  void _queueChanged({bool initial = false}) {
    final queued = widget.queued;
    if (queued == null) return;
    final current = queued.of(widget.agentId);
    final ids = {for (final message in current) message.id};
    final now = DateTime.now();
    var added = false;
    for (final message in current) {
      if (_delivered.contains(message.id)) continue;
      final shown = _waiting.where((w) => w.message.id == message.id);
      if (shown.isEmpty) {
        _waiting.add(_Waiting(message, initial ? DateTime(0) : now));
        added = true;
      } else {
        // Back in the queue: it stays.
        shown.first
          ..leaving?.cancel()
          ..leaving = null
          ..gone = false;
      }
    }
    for (final waiting in _waiting) {
      if (!ids.contains(waiting.message.id)) _leave(waiting);
    }
    if (initial) return;
    setState(() {});
    if (added && _controller.pinned) _startGlide();
  }

  /// [waiting] left the queue. Its entry normally arrives at once and takes
  /// its place; if it doesn't (Stop cleared the queue), it fades out, then
  /// goes.
  void _leave(_Waiting waiting) {
    if (waiting.leaving != null) return;
    waiting.leaving = Timer(_leaveGrace, () {
      if (!mounted || !_waiting.contains(waiting)) return;
      setState(() => waiting.gone = true);
      waiting.leaving = Timer(Motion.base, () {
        if (mounted) setState(() => _waiting.remove(waiting));
      });
    });
  }

  @override
  void didUpdateWidget(TranscriptView old) {
    super.didUpdateWidget(old);
    if (old.follow != widget.follow) {
      old.follow.removeListener(_followAgain);
      widget.follow.addListener(_followAgain);
    }
    if (old.transcript != widget.transcript) {
      old.transcript.removeListener(_entriesChanged);
      widget.transcript.addListener(_entriesChanged);
    }
    if (old.queued != widget.queued) {
      old.queued?.removeListener(_queueChanged);
      widget.queued?.addListener(_queueChanged);
    }
  }

  @override
  void dispose() {
    widget.follow.removeListener(_followAgain);
    widget.transcript.removeListener(_entriesChanged);
    widget.queued?.removeListener(_queueChanged);
    for (final waiting in _waiting) {
      waiting.leaving?.cancel();
    }
    _glide.dispose();
    _controller.dispose();
    super.dispose();
  }

  bool get _reducedMotion => MediaQuery.disableAnimationsOf(context);

  /// New entries arrived: note when, and glide to them if following.
  void _entriesChanged() {
    final transcript = widget.transcript;
    final length = transcript.length;
    if (length <= _known) {
      _known = length;
      return;
    }
    final now = DateTime.now();
    for (var i = _known; i < length; i++) {
      _arrivals[transcript[i].seq] = now;
      _takeFromQueue(transcript[i]);
    }
    _known = length;
    if (_controller.pinned) _startGlide();
  }

  /// A delivered message takes the place of its queued bubble.
  void _takeFromQueue(EntryModel model) {
    final entry = model.entry;
    final String text;
    final bool person;
    if (entry.hasUser()) {
      (text, person) = (entry.user.text, true);
    } else if (entry.hasEvent()) {
      (text, person) = (entry.event.text, false);
    } else {
      return;
    }
    for (final waiting in _waiting) {
      final message = waiting.message;
      if (!waiting.gone &&
          (message.source == 'person') == person &&
          message.text == text) {
        waiting.leaving?.cancel();
        _waiting.remove(waiting);
        _delivered.add(message.id);
        _fromQueue.add(model.seq);
        setState(() {});
        return;
      }
    }
  }

  /// Glides to the bottom, or with reduced motion, goes there at once.
  void _startGlide() {
    if (!_controller.hasClients || _reducedMotion) {
      _controller.gliding = false;
      return;
    }
    _glideFrom = _controller.position.pixels;
    _glidePixels = _glideFrom;
    _controller.gliding = true;
    _glide
      ..stop()
      ..start();
  }

  /// One frame of a glide. The bottom is read each frame, so text streaming
  /// in meanwhile moves the target rather than jumping after.
  void _glided(Duration elapsed) {
    if (!_controller.hasClients || !_controller.gliding) {
      _glide.stop();
      return;
    }
    final position = _controller.position;
    final t = math.min(
      1.0,
      elapsed.inMicroseconds / _glideDuration.inMicroseconds,
    );
    final target = position.maxScrollExtent;
    final pixels = t >= 1
        ? target
        : _glideFrom + (target - _glideFrom) * Curves.easeOutCubic.transform(t);
    _glidePixels = pixels;
    position.jumpTo(pixels);
    if (t >= 1) {
      _controller.gliding = false;
      _glide.stop();
    }
  }

  double? _liveExtent() {
    final sliver = _liveKey.currentContext?.findRenderObject();
    return sliver is RenderSliver ? sliver.geometry?.scrollExtent : null;
  }

  void _scrolled() {
    if (!_controller.hasClients) return;
    final position = _controller.position;
    if (_controller.gliding) {
      // The glide's own step; anything else is the user taking over.
      if ((position.pixels - (_glidePixels ?? -1)).abs() < 0.5) return;
      _controller.gliding = false;
      _glide.stop();
    }
    _controller.pinned = position.pixels >= position.maxScrollExtent - 8;
  }

  void _followAgain() {
    _controller.pinned = true;
    if (!_controller.hasClients) return;
    _startGlide();
    if (!_controller.gliding) {
      _controller.jumpTo(_controller.position.maxScrollExtent);
    }
  }

  void _toggled(int seq, bool open) {
    if (open) {
      _expanded.add(seq);
    } else {
      _expanded.remove(seq);
    }
  }

  Widget _item(int index) {
    final transcript = widget.transcript;
    if (index >= transcript.length) {
      return _queuedItem(_waiting[index - transcript.length]);
    }
    final model = transcript[index];
    final fromQueue = _fromQueue.contains(model.seq);
    return Align(
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: contentMaxWidth),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: Space.xl),
          child: Arrival(
            // A delivered message brightens where its queued bubble was.
            from: fromQueue ? _queuedOpacity : 0,
            rise: !fromQueue,
            // Only an entry built as it arrives eases in; one built later,
            // as the user scrolls to it, is just there.
            animate:
                !_reducedMotion &&
                DateTime.now().difference(_arrivals[model.seq] ?? DateTime(0)) <
                    _glideDuration,
            child: EntryView(
              model: model,
              block: model.entry.hasToolCall()
                  ? toolBlockAt(transcript, index)
                  : const ToolBlock(first: true, last: true),
              live: widget.working && index == transcript.length - 1,
              expanded: _expanded.contains(model.seq),
              onToggle: (open) => _toggled(model.seq, open),
            ),
          ),
        ),
      ),
    );
  }

  /// A message waiting for the agent: as it will look once delivered, but
  /// faded and labeled "Queued".
  Widget _queuedItem(_Waiting waiting) {
    final palette = Palette.of(context);
    final message = waiting.message;
    final (Widget bubble, CrossAxisAlignment side) = switch (message.source) {
      'person' => (_UserText(message.text), CrossAxisAlignment.end),
      'agent' => (
        _AgentMessage(
          Event(
            source: EventSource.EVENT_SOURCE_AGENT,
            from: message.sender,
            fromId: message.senderId,
            text: message.text,
          ),
        ),
        CrossAxisAlignment.start,
      ),
      _ => (
        Text(
          message.summary.isEmpty ? message.text : message.summary,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          textAlign: TextAlign.center,
          style: TextStyles.caption.copyWith(color: palette.textFaint),
        ),
        CrossAxisAlignment.center,
      ),
    };
    return Align(
      key: ValueKey(('queued', message.id)),
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: contentMaxWidth),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(Space.xl, Space.l, Space.xl, 0),
          child: AnimatedOpacity(
            opacity: waiting.gone ? 0 : _queuedOpacity,
            duration: Motion.fast,
            curve: Curves.easeOut,
            child: Arrival(
              animate:
                  !_reducedMotion &&
                  DateTime.now().difference(waiting.shown) < _glideDuration,
              child: Column(
                crossAxisAlignment: side,
                children: [
                  bubble,
                  Padding(
                    padding: const EdgeInsets.only(
                      top: 2,
                      left: Space.m,
                      right: Space.m,
                    ),
                    child: Text(
                      'Queued',
                      style: TextStyles.caption.copyWith(
                        color: palette.textFaint,
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: widget.transcript,
      builder: (context, _) {
        final length = widget.transcript.length;
        final split = math.min(_split, length);
        return SelectableArea(
          onSelectionChanged: (content) =>
              _selected = content != null && content.plainText.isNotEmpty,
          child: _TranscriptSelection(
            selected: () => _selected,
            child: CustomScrollView(
              controller: _controller,
              center: _liveKey,
              slivers: [
                SliverList(
                  delegate: SliverChildBuilderDelegate(
                    (context, index) => _item(split - 1 - index),
                    childCount: split,
                  ),
                ),
                SliverPadding(
                  key: _liveKey,
                  padding: const EdgeInsets.only(bottom: Space.xl),
                  sliver: SliverList(
                    delegate: SliverChildBuilderDelegate(
                      (context, index) => _item(split + index),
                      childCount: length - split + _waiting.length,
                    ),
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}

/// A scroll controller that keeps its position at the bottom while [pinned].
class _FollowController extends ScrollController {
  _FollowController(this.liveExtent);

  /// The extent of the content below the center line, read during layout.
  final double? Function() liveExtent;

  var pinned = true;

  /// The view is gliding to a new entry, so layout leaves it be.
  var gliding = false;

  @override
  ScrollPosition createScrollPosition(
    ScrollPhysics physics,
    ScrollContext context,
    ScrollPosition? oldPosition,
  ) {
    return _FollowPosition(
      this,
      physics: physics,
      context: context,
      oldPosition: oldPosition,
      debugLabel: debugLabel,
    );
  }
}

class _FollowPosition extends ScrollPositionWithSingleContext {
  _FollowPosition(
    this.owner, {
    required super.physics,
    required super.context,
    super.oldPosition,
    super.debugLabel,
  }) : super(initialPixels: 0, keepScrollOffset: false);

  final _FollowController owner;

  @override
  bool applyContentDimensions(double minScrollExtent, double maxScrollExtent) {
    // With the center line at the top, the viewport reports a max of 0 when
    // the content below it is shorter than the viewport, which would allow
    // scrolling into empty space. The true bottom is where that content ends
    // at the viewport's bottom edge.
    var max = maxScrollExtent;
    if (max <= 0) {
      final live = owner.liveExtent();
      if (live != null) {
        max = math.max(minScrollExtent, live - viewportDimension);
      }
    }
    if (owner.pinned &&
        !owner.gliding &&
        hasPixels &&
        (pixels - max).abs() > precisionErrorTolerance) {
      correctPixels(max);
      super.applyContentDimensions(minScrollExtent, max);
      return false;
    }
    return super.applyContentDimensions(minScrollExtent, max);
  }
}

/// One transcript entry. It rebuilds only when its own entry changes, so a
/// streaming delta never rebuilds its neighbors.
class EntryView extends StatefulWidget {
  const EntryView({
    super.key,
    required this.model,
    required this.live,
    required this.expanded,
    required this.onToggle,
    this.block = const ToolBlock(first: true, last: true),
  });

  final EntryModel model;

  /// The agent is still working on this entry.
  final bool live;
  final bool expanded;
  final ValueChanged<bool> onToggle;

  /// For a tool call, where it sits in the block of its reply's calls.
  final ToolBlock block;

  @override
  State<EntryView> createState() => _EntryViewState();
}

class _EntryViewState extends State<EntryView> {
  Widget? _built;

  @override
  void initState() {
    super.initState();
    widget.model.addListener(_changed);
  }

  @override
  void didUpdateWidget(EntryView old) {
    super.didUpdateWidget(old);
    if (old.model != widget.model) {
      old.model.removeListener(_changed);
      widget.model.addListener(_changed);
      _built = null;
    } else if (old.live != widget.live || old.block != widget.block) {
      _built = null;
    }
  }

  @override
  void dispose() {
    widget.model.removeListener(_changed);
    super.dispose();
  }

  void _changed() => setState(() => _built = null);

  @override
  Widget build(BuildContext context) {
    // Returning the same widget instance lets Flutter skip the subtree when
    // the parent rebuilds for an unrelated reason.
    return _built ??= _build();
  }

  Widget _build() {
    final entry = widget.model.entry;
    final seq = widget.model.seq;
    final (Widget child, double gap) = switch (entry.whichKind()) {
      Entry_Kind.user => (_UserText(entry.user.text), Space.l),
      Entry_Kind.proposal => (
        _ProposalCard(key: ValueKey(entry.proposal.id), entry.proposal),
        Space.l,
      ),
      Entry_Kind.event =>
        entry.event.source == EventSource.EVENT_SOURCE_AGENT
            ? (_AgentMessage(entry.event), Space.l)
            : (
                _EventLine(
                  key: ValueKey(seq),
                  event: entry.event,
                  expanded: widget.expanded,
                  onToggle: widget.onToggle,
                ),
                Space.l,
              ),
      Entry_Kind.assistant => (
        entry.assistant.text.isEmpty
            ? const SizedBox.shrink()
            : Markdown(entry.assistant.text),
        Space.l,
      ),
      Entry_Kind.thinking => (
        _ThinkingView(
          key: ValueKey(seq),
          text: entry.thinking.text,
          live: widget.live,
          expanded: widget.expanded,
          onToggle: widget.onToggle,
        ),
        Space.m,
      ),
      Entry_Kind.toolCall => (
        _ToolBlockPiece(
          block: widget.block,
          child: _ToolCallView(
            key: ValueKey(seq),
            call: entry.toolCall,
            expanded: widget.expanded,
            onToggle: widget.onToggle,
          ),
        ),
        // Calls of one reply touch; a new block starts a little apart.
        widget.block.first ? Space.s : 0.0,
      ),
      Entry_Kind.compaction => (
        _CompactionView(
          key: ValueKey(seq),
          compaction: entry.compaction,
          expanded: widget.expanded,
          onToggle: widget.onToggle,
        ),
        Space.xl,
      ),
      Entry_Kind.notice => (_NoticeView(entry.notice), Space.m),
      Entry_Kind.notSet => (const SizedBox.shrink(), 0.0),
    };
    return Padding(
      padding: EdgeInsets.only(top: gap),
      child: child,
    );
  }
}

/// The agents' colors, for the transcript below: the agent's own, which
/// fills the person's bubbles, and the others', which name who sent an
/// event. [byId] holds every agent the app knows; a message from one of them
/// opens its conversation through [open].
class AgentTints extends InheritedWidget {
  const AgentTints({
    super.key,
    required this.own,
    required this.byId,
    this.agents = const {},
    this.open,
    required super.child,
  });

  final Color own;
  final Map<String, Color> byId;

  /// Who each agent is, for its character beside its messages: only what
  /// identity needs ([identityOf]), so a change in an agent's mood or
  /// work doesn't rebuild every message it ever sent.
  final Map<String, Agent> agents;

  /// Opens the conversation of the agent with this id.
  final ValueChanged<String>? open;

  static AgentTints? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<AgentTints>();

  @override
  bool updateShouldNotify(AgentTints old) =>
      old.own != own ||
      !mapEquals(old.byId, byId) ||
      !mapEquals(old.agents, agents);
}

/// The person's message: a bubble on the right in the agent's color.
class _UserText extends StatelessWidget {
  const _UserText(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    final color = AgentTints.of(context)?.own ?? Palette.of(context).signal;
    return Bubble(
      color: color,
      outgoing: true,
      child: Text(
        text,
        style: TextStyles.body.copyWith(color: const Color(0xFFFFFFFF)),
      ),
    );
  }
}

/// The agent's proposal to Rivet, for the person to decide: a card in
/// Rivet's color with the proposal, a comment box (Enter approves), Dismiss
/// and Approve. Decided, it says how and when, with the person's comment.
/// The backend sends the entry back decided; the card keeps nothing.
class _ProposalCard extends StatefulWidget {
  const _ProposalCard(this.proposal, {super.key});

  final Proposal proposal;

  @override
  State<_ProposalCard> createState() => _ProposalCardState();
}

class _ProposalCardState extends State<_ProposalCard> {
  final _comment = TextEditingController();
  var _deciding = false;

  /// Why the last decision was refused.
  String? _error;

  @override
  void dispose() {
    _comment.dispose();
    super.dispose();
  }

  Future<void> _decide({required bool approve}) async {
    if (_deciding) return;
    final scope = AppScope.of(context);
    setState(() {
      _deciding = true;
      _error = null;
    });
    try {
      await scope.decideProposal(
        scope.workspace.workspace.selectedAgentId,
        widget.proposal.id,
        approve: approve,
        comment: approve ? _comment.text.trim() : '',
      );
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _deciding = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) => _card(context, rivetIn(store.workspace)),
    );
  }

  Widget _card(BuildContext context, Agent? rivet) {
    final palette = Palette.of(context);
    final proposal = widget.proposal;
    final color = agentColor(rivet?.color ?? 'blue');
    final name = rivet?.name ?? 'Rivet';
    final pending =
        proposal.state != ProposalState.PROPOSAL_STATE_APPROVED &&
        proposal.state != ProposalState.PROPOSAL_STATE_DISMISSED;
    final dismissed = proposal.state == ProposalState.PROPOSAL_STATE_DISMISSED;
    final decided = proposal.decidedAtMs.toInt();
    return Container(
      padding: const EdgeInsets.all(Space.l),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.06),
        border: Border.all(
          color: color.withValues(alpha: pending ? 0.55 : 0.25),
        ),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              if (rivet != null) ...[
                AgentCharacter(agent: rivet, size: 20),
                const SizedBox(width: Space.xs + 2),
              ],
              Flexible(
                child: Text(
                  'Proposal to $name',
                  style: TextStyles.caption.copyWith(
                    color: color,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: Space.s),
          Markdown(proposal.text, color: dismissed ? palette.textMuted : null),
          const SizedBox(height: Space.m),
          if (pending) ...[
            TextField(
              controller: _comment,
              placeholder: 'A comment for $name, if you like',
              onSubmitted: (_) => _decide(approve: true),
            ),
            const SizedBox(height: Space.s),
            Row(
              children: [
                Expanded(
                  child: Text(
                    _error ?? 'Enter approves; $name gets it as your message.',
                    style: TextStyles.caption.copyWith(
                      color: _error == null
                          ? palette.textFaint
                          : palette.danger,
                    ),
                  ),
                ),
                const SizedBox(width: Space.s),
                Button(
                  label: 'Dismiss',
                  onPressed: _deciding ? null : () => _decide(approve: false),
                ),
                const SizedBox(width: Space.s),
                Button(
                  label: 'Approve',
                  primary: true,
                  onPressed: _deciding ? null : () => _decide(approve: true),
                ),
              ],
            ),
          ] else ...[
            Text.rich(
              TextSpan(
                children: [
                  TextSpan(
                    text: dismissed ? 'Dismissed' : 'Approved, sent to $name',
                    style: TextStyle(
                      color: dismissed ? palette.textMuted : color,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                  if (decided > 0)
                    TextSpan(
                      text:
                          '  ${logTime(DateTime.fromMillisecondsSinceEpoch(decided))}',
                      style: TextStyle(color: palette.textFaint),
                    ),
                ],
              ),
              style: TextStyles.caption,
            ),
            if (proposal.comment.isNotEmpty) ...[
              const SizedBox(height: Space.xs),
              Text(
                proposal.comment,
                style: TextStyles.body.copyWith(color: palette.textMuted),
              ),
            ],
          ],
        ],
      ),
    );
  }
}

/// Another agent's message: a bubble on the left in that agent's color,
/// under its name. Clicking the name or the bubble opens the sender's
/// conversation, when the app knows the sender; a click on the bubble waits
/// to be sure it isn't a double click, and one that drags or clears a
/// selection only selects.
class _AgentMessage extends StatefulWidget {
  const _AgentMessage(this.event);

  final Event event;

  @override
  State<_AgentMessage> createState() => _AgentMessageState();
}

class _AgentMessageState extends State<_AgentMessage> {
  var _hovered = false;

  /// Where a click on the bubble went down, while it may still be one that
  /// opens the sender.
  Offset? _down;

  /// A click on the bubble, waiting out the double-click time.
  Timer? _pending;

  @override
  void dispose() {
    _pending?.cancel();
    super.dispose();
  }

  String get _sender => widget.event.fromId;

  void _open() {
    final tints = context.getInheritedWidgetOfExactType<AgentTints>();
    tints?.open?.call(_sender);
  }

  void _pointerDown(PointerDownEvent event) {
    final pending = _pending;
    if (pending != null && pending.isActive) {
      // A double click, which selects a word.
      pending.cancel();
      _pending = null;
      _down = null;
      return;
    }
    _down =
        event.buttons == kPrimaryButton && !_TranscriptSelection.any(context)
        ? event.position
        : null;
  }

  void _pointerMove(PointerMoveEvent event) {
    final down = _down;
    if (down != null &&
        (event.position - down).distance > computeHitSlop(event.kind, null)) {
      _down = null;
    }
  }

  void _pointerUp(PointerUpEvent event) {
    if (_down == null) return;
    _down = null;
    _pending = Timer(kDoubleTapTimeout, () {
      _pending = null;
      if (mounted && !_TranscriptSelection.any(context)) _open();
    });
  }

  void _hover(bool hovered) {
    if (hovered != _hovered) setState(() => _hovered = hovered);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final tints = AgentTints.of(context);
    final color = tints?.byId[_sender];
    final known = color != null && tints?.open != null;
    final nameColor = color ?? palette.textMuted;
    // Its role now, faint after the name, when the app knows the sender.
    Widget name = NameThenRole(
      name: widget.event.from,
      role: tints?.agents[_sender]?.role ?? '',
      style: TextStyles.caption.copyWith(
        color: nameColor,
        fontWeight: FontWeight.w600,
        decoration: known && _hovered ? TextDecoration.underline : null,
        decorationColor: nameColor,
      ),
      roleColor: palette.textFaint,
    );
    // The pointer shows over the text too, which would otherwise show the
    // text cursor: a region laid over it that lets the pointer through.
    Widget pointing(Widget child) => Stack(
      children: [
        child,
        Positioned.fill(
          child: MouseRegion(
            opaque: false,
            cursor: SystemMouseCursors.click,
            onEnter: (_) => _hover(true),
            onExit: (_) => _hover(false),
          ),
        ),
      ],
    );
    if (known) {
      name = GestureDetector(onTap: _open, child: pointing(name));
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.only(left: Space.m, bottom: 2),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              // Who it's from, at rest: a reference, not their state now.
              if (tints?.agents[_sender] case final sender?) ...[
                AgentCharacter(agent: sender, size: 22),
                const SizedBox(width: Space.xs),
              ],
              Flexible(child: name),
            ],
          ),
        ),
        Bubble(
          color: color ?? palette.selected,
          outgoing: false,
          around: known
              ? (bubble) => Listener(
                  onPointerDown: _pointerDown,
                  onPointerMove: _pointerMove,
                  onPointerUp: _pointerUp,
                  onPointerCancel: (_) => _down = null,
                  child: pointing(bubble),
                )
              : null,
          child: Markdown(
            widget.event.text,
            color: color == null ? null : const Color(0xFFFFFFFF),
          ),
        ),
      ],
    );
  }
}

/// Whether the person has text selected in the transcript, for clicks that
/// should only clear it.
class _TranscriptSelection extends InheritedWidget {
  const _TranscriptSelection({required this.selected, required super.child});

  final bool Function() selected;

  static bool any(BuildContext context) =>
      context
          .getInheritedWidgetOfExactType<_TranscriptSelection>()
          ?.selected() ??
      false;

  @override
  bool updateShouldNotify(_TranscriptSelection old) => false;
}

/// An event from Factory or a background task: one centered line, which
/// opens to the whole message.
class _EventLine extends StatelessWidget {
  const _EventLine({
    super.key,
    required this.event,
    required this.expanded,
    required this.onToggle,
  });

  final Event event;
  final bool expanded;
  final ValueChanged<bool> onToggle;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final line = event.summary.isEmpty ? event.text : event.summary;
    final more = event.summary.isNotEmpty && event.text != event.summary;
    final task = event.source == EventSource.EVENT_SOURCE_TASK;
    return _Opening(
      expanded: expanded,
      onToggle: more ? onToggle : null,
      header: (context, open, hovered) {
        final color = hovered && more ? palette.textMuted : palette.textFaint;
        return Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Flexible(
              child: Text(
                line,
                maxLines: open ? null : 1,
                overflow: open ? null : TextOverflow.ellipsis,
                textAlign: TextAlign.center,
                style: TextStyles.caption.copyWith(color: color),
              ),
            ),
            if (more) ...[
              const SizedBox(width: Space.xs + 2),
              Chevron(color: color, down: open),
            ],
          ],
        );
      },
      body: (context) => Container(
        width: double.infinity,
        padding: const EdgeInsets.all(Space.m),
        decoration: BoxDecoration(
          color: palette.sidebar,
          borderRadius: BorderRadius.circular(Radii.panel),
          border: Border.all(color: palette.hairline),
        ),
        child: task
            ? Text(
                event.text,
                style: TextStyles.code.copyWith(color: palette.textMuted),
              )
            : Text(
                event.text,
                style: TextStyles.body.copyWith(color: palette.textMuted),
              ),
      ),
    );
  }
}

/// A header that opens to show more below it; without `onToggle` it's only
/// the header.
class _Opening extends StatefulWidget {
  const _Opening({
    required this.expanded,
    required this.onToggle,
    required this.header,
    required this.body,
  });

  final bool expanded;
  final ValueChanged<bool>? onToggle;
  final Widget Function(BuildContext context, bool open, bool hovered) header;
  final WidgetBuilder body;

  @override
  State<_Opening> createState() => _OpeningState();
}

class _OpeningState extends State<_Opening> {
  late var _open = widget.expanded;

  @override
  Widget build(BuildContext context) {
    final onToggle = widget.onToggle;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Pressable(
          onTap: onToggle == null
              ? null
              : () {
                  setState(() => _open = !_open);
                  onToggle(_open);
                },
          builder: (context, hovered) => widget.header(context, _open, hovered),
        ),
        if (_open && onToggle != null)
          Padding(
            padding: const EdgeInsets.only(top: Space.s),
            child: widget.body(context),
          ),
      ],
    );
  }
}

/// A message bubble with a tail, like iMessage's: on the right for what the
/// person sends, on the left for what they receive.
class Bubble extends StatelessWidget {
  const Bubble({
    super.key,
    required this.color,
    required this.outgoing,
    required this.child,
    this.around,
  });

  final Color color;
  final bool outgoing;
  final Widget child;

  /// Wraps the bubble itself, without its tail's margin or the room beside
  /// it, such as to make it clickable.
  final Widget Function(Widget bubble)? around;

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) => Align(
        alignment: outgoing ? Alignment.centerRight : Alignment.centerLeft,
        child: Padding(
          // Room for the tail.
          padding: EdgeInsets.only(
            left: outgoing ? 0 : _BubblePainter.tail,
            right: outgoing ? _BubblePainter.tail : 0,
          ),
          child: ConstrainedBox(
            constraints: BoxConstraints(
              maxWidth: constraints.maxWidth * 0.78,
              minHeight: 34,
            ),
            child: (around ?? (bubble) => bubble)(
              CustomPaint(
                painter: _BubblePainter(color: color, outgoing: outgoing),
                child: Padding(
                  padding: const EdgeInsets.fromLTRB(
                    Space.m,
                    Space.s - 1,
                    Space.m,
                    Space.s,
                  ),
                  child: child,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _BubblePainter extends CustomPainter {
  const _BubblePainter({required this.color, required this.outgoing});

  /// How far the tail reaches past the bubble.
  static const tail = 5.0;
  static const radius = 17.0;

  final Color color;
  final bool outgoing;

  @override
  void paint(Canvas canvas, Size size) {
    final w = size.width;
    final h = size.height;
    final body = Path()
      ..addRRect(RRect.fromLTRBR(0, 0, w, h, const Radius.circular(radius)));
    // Drawn for the right side: from the bubble's edge down to a point past
    // its corner, then curving back in along the bottom.
    final horn = Path()
      ..moveTo(w, h - 16)
      ..quadraticBezierTo(w, h - 2, w + tail, h)
      ..quadraticBezierTo(w - 3, h + 0.5, w - 11, h - 4)
      ..close();
    var shape = Path.combine(PathOperation.union, body, horn);
    if (!outgoing) {
      // Mirrored onto the left.
      final mirror = Matrix4.identity()
        ..translateByDouble(w, 0, 0, 1)
        ..scaleByDouble(-1, 1, 1, 1);
      shape = shape.transform(mirror.storage);
    }
    canvas.drawPath(shape, Paint()..color = color);
  }

  @override
  bool shouldRepaint(_BubblePainter old) =>
      old.color != color || old.outgoing != outgoing;
}

/// A one-line header that expands to show more below it.
class _Disclosure extends StatefulWidget {
  const _Disclosure({
    required this.expanded,
    required this.onToggle,
    required this.header,
    required this.body,
  });

  final bool expanded;
  final ValueChanged<bool> onToggle;
  final Widget Function(BuildContext context, bool open, bool hovered) header;
  final WidgetBuilder body;

  @override
  State<_Disclosure> createState() => _DisclosureState();
}

class _DisclosureState extends State<_Disclosure> {
  late var _open = widget.expanded;

  void _toggle() {
    setState(() => _open = !_open);
    widget.onToggle(_open);
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Pressable(
          onTap: _toggle,
          builder: (context, hovered) => widget.header(context, _open, hovered),
        ),
        if (_open)
          Padding(
            padding: const EdgeInsets.only(
              left: Space.l + 2,
              top: Space.s,
              bottom: Space.s,
            ),
            child: widget.body(context),
          ),
      ],
    );
  }
}

class _ThinkingView extends StatelessWidget {
  const _ThinkingView({
    super.key,
    required this.text,
    required this.live,
    required this.expanded,
    required this.onToggle,
  });

  final String text;
  final bool live;
  final bool expanded;
  final ValueChanged<bool> onToggle;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return _Disclosure(
      expanded: expanded,
      onToggle: onToggle,
      header: (context, open, hovered) => SizedBox(
        height: 24,
        child: Row(
          children: [
            SizedBox(
              width: Space.l + 2,
              child: live
                  ? const Align(
                      alignment: Alignment.centerLeft,
                      child: LiveDot(),
                    )
                  : Chevron(color: palette.textFaint, down: open),
            ),
            Text(
              live ? 'Thinking…' : 'Thought',
              style: TextStyles.body.copyWith(
                color: hovered || live ? palette.textMuted : palette.textFaint,
              ),
            ),
          ],
        ),
      ),
      body: (context) =>
          Text(text, style: TextStyles.body.copyWith(color: palette.textMuted)),
    );
  }
}

/// Where a tool call sits in the block of calls one model reply made.
@immutable
class ToolBlock {
  const ToolBlock({required this.first, required this.last});

  /// Its reply's first call here; the block's top.
  final bool first;

  /// Its reply's last call here; the block's bottom.
  final bool last;

  @override
  bool operator ==(Object other) =>
      other is ToolBlock && other.first == first && other.last == last;

  @override
  int get hashCode => Object.hash(first, last);
}

/// Where the entry at [index] sits among its reply's tool calls: the calls
/// next to each other with the same nonzero reply form one block.
ToolBlock toolBlockAt(TranscriptStore transcript, int index) {
  final entry = transcript[index].entry;
  bool sameReply(int other) {
    if (other < 0 || other >= transcript.length) return false;
    final neighbour = transcript[other].entry;
    return entry.reply != Int64.ZERO &&
        neighbour.hasToolCall() &&
        neighbour.reply == entry.reply;
  }

  return ToolBlock(first: !sameReply(index - 1), last: !sameReply(index + 1));
}

/// One call's part of its block: a hairline panel whose top and bottom
/// rounds belong to the block's first and last calls, with a hairline
/// between calls.
class _ToolBlockPiece extends StatelessWidget {
  const _ToolBlockPiece({required this.block, required this.child});

  final ToolBlock block;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    const radius = Radius.circular(Radii.panel);
    final side = BorderSide(color: palette.hairline);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: Space.s, vertical: 2),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.vertical(
          top: block.first ? radius : Radius.zero,
          bottom: block.last ? radius : Radius.zero,
        ),
        border: Border(
          left: side,
          right: side,
          top: block.first ? side : BorderSide.none,
          bottom: side,
        ),
      ),
      child: child,
    );
  }
}

class _ToolCallView extends StatelessWidget {
  const _ToolCallView({
    super.key,
    required this.call,
    required this.expanded,
    required this.onToggle,
  });

  final ToolCall call;
  final bool expanded;
  final ValueChanged<bool> onToggle;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final failed = call.state == ToolCallState.TOOL_CALL_STATE_FAILED;
    final running =
        call.state == ToolCallState.TOOL_CALL_STATE_PENDING ||
        call.state == ToolCallState.TOOL_CALL_STATE_RUNNING;
    final summary = toolSummary(call.name, call.input);
    return _Disclosure(
      expanded: expanded,
      onToggle: onToggle,
      header: (context, open, hovered) => SizedBox(
        height: 24,
        child: Row(
          children: [
            SizedBox(
              width: Space.l + 2,
              child: Align(
                alignment: Alignment.centerLeft,
                child: running
                    ? const LiveDot()
                    : Dot(color: failed ? palette.danger : palette.textFaint),
              ),
            ),
            Text(
              call.name,
              style: TextStyles.label.copyWith(
                color: failed ? palette.danger : palette.text,
              ),
            ),
            const SizedBox(width: Space.s),
            // What it does: the command, the path, whom it messages.
            Expanded(
              child: Text(
                summary.text,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: (summary.code ? TextStyles.code : TextStyles.body)
                    .copyWith(
                      color: hovered ? palette.text : palette.textMuted,
                    ),
              ),
            ),
            if (summary.more.isNotEmpty) ...[
              const SizedBox(width: Space.s),
              Text(
                summary.more,
                style: TextStyles.caption.copyWith(color: palette.textFaint),
              ),
            ],
            // How long it has run (ticking) and when bash will stop it, or
            // how long it took.
            if (call.started > 0 && (call.ended > 0 || running)) ...[
              const SizedBox(width: Space.s),
              Ticking(
                active: call.ended == 0,
                builder: (context, now) => Text(
                  toolClock(call, now),
                  style: TextStyles.caption.copyWith(
                    color: palette.textFaint,
                    fontFeatures: const [FontFeature.tabularFigures()],
                  ),
                ),
              ),
            ],
            if (failed) ...[
              const SizedBox(width: Space.s),
              Text(
                'Failed',
                style: TextStyles.caption.copyWith(color: palette.danger),
              ),
            ],
            const SizedBox(width: Space.s),
            Chevron(
              color: hovered ? palette.textMuted : palette.textFaint,
              down: open,
            ),
          ],
        ),
      ),
      body: (context) => _ToolCallBody(call: call, running: running),
    );
  }
}

/// A tool call's clock: while it runs, how long it has so far and, for
/// bash, when it will time out ("1m 23s · times out in 8m 37s", "· no limit"
/// in the background); once it ended, how long it took. Empty before it
/// starts, and for calls from before Factory kept the times.
String toolClock(ToolCall call, DateTime now) {
  if (call.started <= 0) return '';
  if (call.ended > 0) {
    return formatDuration(
      Duration(milliseconds: (call.ended - call.started).toInt()),
    );
  }
  // Finished without an end time: nothing true to say about how long.
  if (call.state != ToolCallState.TOOL_CALL_STATE_RUNNING &&
      call.state != ToolCallState.TOOL_CALL_STATE_PENDING) {
    return '';
  }
  final elapsed = Duration(
    milliseconds: now.millisecondsSinceEpoch - call.started.toInt(),
  );
  final ran = formatDuration(elapsed, whole: true);
  if (call.timeoutMs > 0) {
    final left = Duration(milliseconds: call.timeoutMs.toInt()) - elapsed;
    // Rounded up, so it reaches 0s only as it times out.
    final seconds = (left.inMilliseconds / 1000).ceil();
    return '$ran · times out in '
        '${formatDuration(Duration(seconds: seconds), whole: true)}';
  }
  return call.name == 'bash' ? '$ran · no limit' : ran;
}

class _ToolCallBody extends StatelessWidget {
  const _ToolCallBody({required this.call, required this.running});

  final ToolCall call;
  final bool running;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final failed = call.state == ToolCallState.TOOL_CALL_STATE_FAILED;
    Widget label(String text) => Padding(
      padding: const EdgeInsets.only(bottom: Space.xs),
      child: Text(
        text,
        style: TextStyles.caption.copyWith(color: palette.textFaint),
      ),
    );
    final Widget output;
    if (call.output.isEmpty) {
      output = Text(
        running ? 'Running…' : 'No output',
        style: TextStyles.caption.copyWith(color: palette.textFaint),
      );
    } else if (call.name == 'edit_file' && !failed) {
      output = DiffView(call.output);
    } else {
      output = CodeBlock(call.output, color: failed ? palette.danger : null);
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (call.input.isNotEmpty) ...[
          label('Input'),
          CodeBlock(call.input),
          const SizedBox(height: Space.m),
        ],
        label('Output'),
        output,
      ],
    );
  }
}

/// A unified diff. Added lines are bright on a tint, removed lines faint, so
/// the change reads without introducing new colors.
class DiffView extends StatelessWidget {
  const DiffView(this.diff, {super.key});

  final String diff;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final spans = <TextSpan>[];
    final lines = diff.split('\n');
    if (lines.isNotEmpty && lines.last.isEmpty) lines.removeLast();
    for (var i = 0; i < lines.length; i++) {
      final line = lines[i];
      final TextStyle style;
      if (line.startsWith('+++') ||
          line.startsWith('---') ||
          line.startsWith('Index: ') ||
          line.startsWith('====')) {
        style = TextStyle(color: palette.textFaint);
      } else if (line.startsWith('@@')) {
        style = TextStyle(color: palette.textMuted);
      } else if (line.startsWith('+')) {
        style = TextStyle(
          color: palette.text,
          backgroundColor: palette.selected,
        );
      } else if (line.startsWith('-')) {
        style = TextStyle(color: palette.textFaint);
      } else {
        style = TextStyle(color: palette.textMuted);
      }
      spans.add(
        TextSpan(text: i == lines.length - 1 ? line : '$line\n', style: style),
      );
    }
    return Container(
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
      ),
      child: SingleChildScrollView(
        scrollDirection: Axis.horizontal,
        padding: const EdgeInsets.symmetric(
          horizontal: Space.m,
          vertical: Space.s + 2,
        ),
        child: Text.rich(
          TextSpan(style: TextStyles.code, children: spans),
          softWrap: false,
        ),
      ),
    );
  }
}

class _CompactionView extends StatelessWidget {
  const _CompactionView({
    super.key,
    required this.compaction,
    required this.expanded,
    required this.onToggle,
  });

  final Compaction compaction;
  final bool expanded;
  final ValueChanged<bool> onToggle;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return _Disclosure(
      expanded: expanded,
      onToggle: onToggle,
      header: (context, open, hovered) {
        final color = hovered ? palette.textMuted : palette.textFaint;
        return SizedBox(
          height: 24,
          child: Row(
            children: [
              Expanded(child: Container(height: 1, color: palette.hairline)),
              const SizedBox(width: Space.m),
              Text(
                'Context compacted at '
                '${formatTokens(compaction.tokensBefore)} tokens',
                style: TextStyles.caption.copyWith(color: color),
              ),
              const SizedBox(width: Space.xs + 2),
              Chevron(color: color, down: open),
              const SizedBox(width: Space.m),
              Expanded(child: Container(height: 1, color: palette.hairline)),
            ],
          ),
        );
      },
      body: (context) => Markdown(compaction.summary, color: palette.textMuted),
    );
  }
}

class _NoticeView extends StatelessWidget {
  const _NoticeView(this.notice);

  final Notice notice;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final error = notice.level == NoticeLevel.NOTICE_LEVEL_ERROR;
    if (error) {
      final store = AppScope.of(context).workspace;
      final id = store.workspace.selectedAgentId;
      final name = store.agent(id)?.name ?? id;
      return ErrorLine(
        notice.text,
        about: "In $name's conversation (agent $id):",
      );
    }
    return Text(
      notice.text,
      style: TextStyles.caption.copyWith(color: palette.textMuted),
    );
  }
}

/// What a tool call does, for its collapsed row: its most telling argument
/// on one line, and how much more there is.
class ToolSummary {
  const ToolSummary(this.text, {this.more = '', this.code = true});

  static const none = ToolSummary('');

  /// One line: a command, a path, a recipient and message…
  final String text;

  /// What the line leaves out, such as "+3 lines"; empty when nothing.
  final String more;

  /// [text] is code (a command, a path), shown in mono.
  final bool code;

  @override
  bool operator ==(Object other) =>
      other is ToolSummary &&
      other.text == text &&
      other.more == more &&
      other.code == code;

  @override
  int get hashCode => Object.hash(text, more, code);

  @override
  String toString() => 'ToolSummary($text, $more, $code)';
}

/// What [name] does with [input], its arguments as JSON. [input] may be
/// cut off while the model is still writing it; what's there so far is
/// read as far as it goes.
ToolSummary toolSummary(String name, String input) {
  if (input.trim().isEmpty) return ToolSummary.none;
  Map<String, Object?>? arguments;
  try {
    final decoded = jsonDecode(input);
    if (decoded is Map<String, Object?>) arguments = decoded;
  } on FormatException {
    // Still being written.
  }
  String? text(String key) {
    if (arguments != null) {
      final value = arguments[key];
      return value is String ? value : value?.toString();
    }
    return _partialString(input, key);
  }

  ToolSummary lines(String? value, {bool code = true}) {
    if (value == null || value.trim().isEmpty) return ToolSummary.none;
    final all = value.trim().split('\n');
    final first = all.first.trimRight();
    final rest = all.length - 1;
    return ToolSummary(
      first,
      more: rest == 0 ? '' : '+$rest ${rest == 1 ? 'line' : 'lines'}',
      code: code,
    );
  }

  switch (name) {
    case 'bash':
      return lines(text('command'));
    case 'read_file' || 'write_file' || 'edit_file':
      return lines(text('path'));
    case 'agents_send':
      final to = text('to') ?? '';
      final message = lines(text('text'), code: false);
      if (to.isEmpty) return message;
      return ToolSummary(
        message.text.isEmpty ? '→ $to' : '→ $to: ${message.text}',
        more: message.more,
        code: false,
      );
    case 'checklist_update':
      final updates = arguments?['updates'];
      if (updates is List) {
        return ToolSummary(
          [
            for (final update in updates)
              if (update is Map)
                [
                  '#${update['id']}',
                  if (update['status'] is String)
                    (update['status'] as String).replaceAll('_', ' '),
                ].join(' '),
          ].join(', '),
          code: false,
        );
      }
      final ids = RegExp(r'"id"\s*:\s*(\d+)').allMatches(input);
      return ToolSummary(
        [for (final id in ids) '#${id.group(1)}'].join(', '),
        code: false,
      );
    case 'checklist_add':
      final items = arguments?['items'];
      if (items is List && items.isNotEmpty) {
        final first = lines('${items.first}', code: false);
        final others = items.length - 1;
        return ToolSummary(
          first.text,
          more: others == 0 ? '' : '+$others more',
          code: false,
        );
      }
      return lines(_partialString(input, 'items'), code: false);
    case 'checklist_item':
      final id =
          arguments?['id'] ??
          RegExp(r'"id"\s*:\s*(\d+)').firstMatch(input)?.group(1);
      return id == null ? ToolSummary.none : ToolSummary('#$id', code: false);
    case 'memory_update':
      final ops = arguments?['ops'];
      if (ops is List) {
        return ToolSummary(
          [
            for (final op in ops)
              if (op is Map)
                [
                  '${op['op']}'.replaceAll('_', ' '),
                  if (op['slug'] != null) op['slug'],
                  if (op['id'] != null) '#${op['id']}',
                ].join(' '),
          ].join(', '),
          code: false,
        );
      }
      return lines(text('op'), code: false);
    case 'highlight':
      return lines(text('text'), code: false);
  }
  // Others: the argument that usually says the most, or else the first
  // text there is.
  for (final key in const [
    'path',
    'command',
    'pattern',
    'query',
    'q',
    'url',
    'note',
    'task',
    'agent',
  ]) {
    final value = text(key);
    if (value != null && value.trim().isNotEmpty) return lines(value);
  }
  if (arguments != null) {
    for (final value in arguments.values) {
      if (value is String && value.trim().isNotEmpty) return lines(value);
    }
    return ToolSummary.none;
  }
  return lines(_partialString(input, null));
}

/// The string value of [key] in [json] as far as it's written, decoding its
/// escapes; with no [key], the first string value. Null when there's none
/// yet.
String? _partialString(String json, String? key) {
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

/// 412000 → "412k".
String formatTokens(Int64 tokens) {
  final n = tokens.toInt();
  if (n < 1000) return '$n';
  if (n < 1000000) return '${(n / 1000).round()}k';
  return '${(n / 1000000).toStringAsFixed(1)}M';
}
