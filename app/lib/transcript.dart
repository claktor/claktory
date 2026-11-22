import 'dart:async';
import 'dart:convert';
import 'dart:math' as math;

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';

import 'agent_name.dart';
import 'app_grant_card.dart';
import 'attachments.dart';
import 'backend.dart' show RequestFailure;
import 'characters/character.dart';
import 'avatar.dart';
import 'chats.dart' show ChatScope, chatTitle;
import 'connectors.dart' show ConnectProposalCard;
import 'controls.dart';
import 'decision.dart';
import 'conversation_scrollbar.dart';
import 'explorer.dart' show openFile;
import 'extensions.dart' show logTime;
import 'forms.dart' show rivetIn;
import 'identity.dart';
import 'mobile/haptics.dart';
import 'mobile/message_actions.dart';
import 'mobile/metrics.dart';
import 'mobile/platform_policy.dart';
import 'mobile/navigation.dart';
import 'message_time.dart';
import 'markdown.dart';
import 'outbox.dart';
import 'ask_card.dart';
import 'pairing_card.dart';
import 'pipe_start.dart' show PipeStartCard;
import 'pipes.dart' show SecretApprovalCard;
import 'refusal_card.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'quiet_copies.dart' show MessageProvenance, quietQueuedLabel;
import 'scope.dart';
import 'secrets.dart';
import 'state.dart';
import 'tell_rivet.dart';
import 'selection.dart';
import 'tool_views.dart';
import 'voice.dart';
import 'waiting.dart' show EntryReveal, EntryTarget;

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
/// An item of a conversation's timeline that isn't one of its entries
/// ([TranscriptView.extras]): [build] makes it, in the conversation's
/// column; [key] tells it apart, and names it to go to
/// ([EntryReveal.reachExtra]).
class TimelineItem {
  const TimelineItem({
    required this.key,
    required this.timestampMs,
    required this.build,
  });

  final Object key;
  final int timestampMs;
  final WidgetBuilder build;
}

/// Where [TranscriptView.extras] go: [before] the first entry held, and
/// [after] each entry by its index.
class _Placed {
  const _Placed(this.before, this.after);

  static const none = _Placed([], {});

  final List<TimelineItem> before;
  final Map<int, List<TimelineItem>> after;

  /// The index of the entry [key] follows: -1 before the first; null when
  /// it isn't placed (older than what's held, or not among the extras).
  int? indexOf(Object key) {
    if (before.any((item) => item.key == key)) return -1;
    for (final MapEntry(key: index, value: items) in after.entries) {
      if (items.any((item) => item.key == key)) return index;
    }
    return null;
  }
}

class TranscriptView extends StatefulWidget {
  const TranscriptView({
    super.key,
    required this.transcript,
    required this.working,
    required this.follow,
    this.queued,
    this.outbox,
    this.loadEarlier,
    this.agentId = '',
    this.reveal,
    this.loadFrom,
    this.extras,
    this.hidden,
  });

  /// More of the timeline than the transcript's entries, by time (a pipe's
  /// messages among its liaison's conversation), sorted by
  /// [TimelineItem.timestampMs]: each shows after the last entry no newer
  /// than it. One older than every entry held shows only once there's
  /// nothing earlier to load, so paging back never leaves a gap.
  final ValueListenable<List<TimelineItem>>? extras;

  /// Entries not to show (ones an extra already shows). Tool calls on
  /// either side of a hidden one still join up in one block.
  final bool Function(Entry entry)? hidden;

  /// Where to open instead of at the end: an entry to put at the top of
  /// the view (the oldest card waiting for the person), once, when it's
  /// this transcript's agent's. The view clears it once it's used.
  final EntryReveal? reveal;

  /// Loads every entry from a seq up to the first one held, to go to an
  /// older entry [reveal] names. Throws [RequestFailure] when it can't.
  final Future<void> Function(int seq)? loadFrom;

  /// Loads the entries before the first one held, when the transcript has
  /// any ([TranscriptStore.hasEarlier]); called as the person scrolls near
  /// the top. Throws [RequestFailure] when it can't.
  final Future<void> Function()? loadEarlier;

  final TranscriptStore transcript;

  /// The person's messages on their way, shown at once after what's
  /// queued, until the backend's own take their place.
  final Outbox? outbox;

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

/// How faded a queued message is: on a phone less, so its words keep at
/// least 3:1 (IDENTITY.md, Phone).
double _queuedOpacity(BuildContext context) =>
    isPhoneLayout(context) ? 0.75 : 0.5;

/// How long a message that left the queue stays, waiting for its entry,
/// before it fades out (as it does when /clear drops the queue).
const _leaveGrace = Duration(milliseconds: 1500);

/// How long a message the person sent still says "Sending…" once it's
/// queued: an idle agent takes it at once, so "Queued" would only flash.
const _sendingFor = Duration(seconds: 1);

/// A queued message on show, or one of the person's on its way.
class _Waiting {
  _Waiting(this.message, this.shown, {this.outgoing});

  /// The backend's queued message; for one still on its way, a stand-in
  /// with its text.
  QueuedMessage message;

  /// While it's the person's message on its way: the backend's queued
  /// message or entry takes its place, in place, when it comes.
  Outgoing? outgoing;

  /// The person's message just sent, queued a moment ago: it still says
  /// "Sending…".
  var justSent = false;

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
  late final _controller = _FollowController(_liveExtent);

  /// Every item's height once laid out, for the scrollbar
  /// ([ConversationScrollbar]).
  final _heights = ItemHeights();

  /// An item's id among [_heights]: an entry's seq, a queued message's
  /// place after them, or the earlier entries' row.
  Object _idOf(int index) {
    final transcript = widget.transcript;
    return index < transcript.length
        ? transcript[index].seq
        : ('queued', index - transcript.length);
  }

  /// The items' ids, top to bottom.
  List<Object> _order() {
    final transcript = widget.transcript;
    return [
      if (transcript.hasEarlier && widget.loadEarlier != null) 'earlier',
      if (transcript.length == 0 && _placed.before.isNotEmpty) 'extras',
      for (var i = 0; i < transcript.length + _waiting.length; i++) _idOf(i),
    ];
  }

  /// About how many entries come before the first held: seqs count the
  /// conversation's entries from 0.
  int _unseen() {
    final transcript = widget.transcript;
    if (!transcript.hasEarlier || transcript.length == 0) return 0;
    return transcript[0].seq;
  }

  /// The extras' keys seen so far, to tell new ones as they come.
  final _knownExtras = <Object>{};

  /// When extras that came after the view opened arrived: they ease in if
  /// built right after, as entries do.
  final _extraArrivals = <Object, DateTime>{};

  /// Each extra's place in the tree, to bring it into view.
  final _extraKeys = <Object, GlobalKey>{};

  GlobalKey _extraKey(Object key) => _extraKeys[key] ??= GlobalKey();

  void _extrasChanged() {
    final items = widget.extras?.value ?? const <TimelineItem>[];
    final now = DateTime.now();
    var arrived = false;
    for (final item in items) {
      if (_knownExtras.add(item.key)) {
        _extraArrivals[item.key] = now;
        arrived = true;
      }
    }
    final keys = {for (final item in items) item.key};
    _extraKeys.removeWhere((key, _) => !keys.contains(key));
    if (!mounted) return;
    setState(() {});
    // One that came in at the end is followed there, like an entry.
    if (arrived && _controller.pinned) _startGlide();
    if (widget.reveal?.extra != null) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _revealExtra());
    }
  }

  /// Where the extras go among the entries held now.
  _Placed _place() {
    final items = widget.extras?.value ?? const <TimelineItem>[];
    if (items.isEmpty) return _Placed.none;
    final transcript = widget.transcript;
    final earlier = transcript.hasEarlier && widget.loadEarlier != null;
    final before = <TimelineItem>[];
    final after = <int, List<TimelineItem>>{};
    for (final item in items) {
      // The last entry no newer than it.
      var low = 0;
      var high = transcript.length - 1;
      var found = -1;
      while (low <= high) {
        final middle = (low + high) >> 1;
        if (transcript[middle].entry.timestampMs.toInt() <= item.timestampMs) {
          found = middle;
          low = middle + 1;
        } else {
          high = middle - 1;
        }
      }
      if (found >= 0) {
        (after[found] ??= []).add(item);
      } else if (!earlier) {
        before.add(item);
      }
    }
    return _Placed(before, after);
  }

  /// Where the extras are at the last build.
  var _placed = _Placed.none;

  /// Goes to the extra [EntryReveal.extra] names, once it's this view's:
  /// loads back page by page while it's older than what's held, then puts
  /// it at the top.
  void _revealExtra() {
    final reveal = widget.reveal;
    final target = reveal?.extra;
    final transcript = widget.transcript;
    if (reveal == null ||
        target == null ||
        !mounted ||
        target.agentId != transcript.agentId) {
      return;
    }
    final items = widget.extras?.value ?? const <TimelineItem>[];
    // Not among the extras yet: looked for again when they change.
    if (!items.any((item) => item.key == target.key)) return;
    final index = _place().indexOf(target.key);
    if (index == null) {
      final load = widget.loadEarlier;
      if (load == null || !transcript.hasEarlier || _earlierFailed != null) {
        reveal.usedExtra(target);
      } else if (!_loadingEarlier) {
        // The page joins on above; this is looked at again then.
        unawaited(_loadEarlier(load));
      }
      return;
    }
    reveal.usedExtra(target);
    if (index >= 0) _goTo(index);
    // The item itself at the top, once laid out there.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final context = _extraKeys[target.key]?.currentContext;
      if (!mounted || context == null) return;
      _controller.pinned = false;
      Scrollable.ensureVisible(context);
    });
  }

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

  /// Runs of thoughts the user has expanded, by their first thought's seq.
  final _expandedRuns = <int>{};

  /// Whether words are selected: a hold on a message is then the
  /// selection's, not the message's actions ([MessageSelection]).
  final _selecting = ValueNotifier(false);

  /// The message the pointer is over, for the right click's Reply.
  final _pointed = ValueNotifier<_Pointed?>(null);

  /// Quotes [message] into this conversation's message box: an agent's,
  /// or a chat's.
  void _reply(_Pointed message) {
    final chatId = ChatScope.chatIdOf(context);
    final key = chatId != null ? 'chat:$chatId' : widget.transcript.agentId;
    if (key.isEmpty) return;
    AppScope.scope(context).drafts.reply(key, message.text, from: message.from);
  }

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
    _earlier = widget.transcript.earlier;
    _controller.addListener(_scrolled);
    widget.follow.addListener(_followAgain);
    widget.transcript.addListener(_entriesChanged);
    widget.queued?.addListener(_queueChanged);
    widget.outbox?.addListener(_outboxChanged);
    widget.reveal?.addListener(_revealChanged);
    widget.extras?.addListener(_extrasChanged);
    _knownExtras.addAll([
      for (final item in widget.extras?.value ?? const <TimelineItem>[])
        item.key,
    ]);
    _loads = widget.transcript.loads;
    _queueChanged(initial: true);
    _outboxChanged(initial: true);
    // Opened at what waits for the person: there from the first frame.
    final target = widget.reveal?.value;
    final index = target == null ? null : _indexFor(target);
    if (target != null && index != null) {
      widget.reveal!.used(target);
      if (_worthGoing(index)) {
        _split = index;
        _controller.pinned = false;
        _atTop();
      }
    } else {
      WidgetsBinding.instance.addPostFrameCallback((_) => _revealChanged());
    }
  }

  /// Loading back to the entry [TranscriptView.reveal] names.
  var _revealing = false;

  /// The load of the transcript this view shows ([TranscriptStore.loads]).
  late final int _loads;

  /// Where [target] is in the transcript, when it's this agent's and held.
  int? _indexFor(EntryTarget target) {
    final transcript = widget.transcript;
    if (transcript.agentId != target.agentId) return null;
    for (var i = transcript.length - 1; i >= 0; i--) {
      final seq = transcript[i].seq;
      if (seq == target.seq) return i;
      if (seq < target.seq) return null;
    }
    return null;
  }

  /// Goes to the entry [TranscriptView.reveal] names, once it's this
  /// agent's: loads back to it if it's older than what's held, then puts
  /// it at the top. One that isn't there leaves the view where it is.
  void _revealChanged() {
    _revealExtra();
    final reveal = widget.reveal;
    final target = reveal?.value;
    final transcript = widget.transcript;
    if (reveal == null ||
        target == null ||
        _revealing ||
        !mounted ||
        // A new load is on its way to a view of its own, which goes there.
        transcript.loads != _loads ||
        transcript.agentId != target.agentId ||
        transcript.length == 0) {
      return;
    }
    final index = _indexFor(target);
    final load = widget.loadFrom;
    if (index != null ||
        load == null ||
        !reveal.reachBack ||
        !transcript.hasEarlier ||
        target.seq >= transcript[0].seq) {
      reveal.used(target);
      if (index != null && _worthGoing(index)) _goTo(index);
      return;
    }
    _revealing = true;
    unawaited(() async {
      var failed = false;
      try {
        await load(target.seq);
      } on RequestFailure {
        // It stays where it is; scrolling up loads the rest as ever.
        failed = true;
      }
      _revealing = false;
      // A page scrolling up brought in meanwhile can make this one not
      // join on: then once more.
      if (failed || ++_revealTries > 1 || _indexFor(target) != null) {
        reveal.used(target);
      }
      if (!mounted) return;
      final index = _indexFor(target);
      if (index != null) {
        _goTo(index);
      } else {
        _revealChanged();
      }
    }());
  }

  /// Loads tried for the entry [TranscriptView.reveal] names.
  var _revealTries = 0;

  /// Whether to go to the entry at [index] that [TranscriptView.reveal]
  /// names: when the person asked to reach it, or when it's news, nothing
  /// of theirs after it. A card they've written past is old news, and the
  /// conversation stays at its end.
  bool _worthGoing(int index) {
    if (widget.reveal?.reachBack ?? false) return true;
    final transcript = widget.transcript;
    for (var i = index + 1; i < transcript.length; i++) {
      if (transcript[i].entry.hasUser()) return false;
    }
    return true;
  }

  /// Puts the entry at [index] at the top of the view: it starts the part
  /// below the center line, which sits at offset 0. Following the end
  /// starts again once the person scrolls down to it.
  void _goTo(int index) {
    _glide.stop();
    _controller
      ..gliding = false
      ..pinned = false;
    setState(() => _split = index);
    // The center line at the top from the next frame on, not one after:
    // the frame between would show the old place with the new split.
    if (_controller.hasClients) {
      _jumping = true;
      _controller.jumpTo(0);
      _jumping = false;
    }
    _atTop();
  }

  /// After the next layout, the view at the center line: the entry after
  /// it at the top, or as near as the content below it allows.
  void _atTop() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !_controller.hasClients) return;
      final position = _controller.position;
      _controller.jumpTo(
        0.0.clamp(position.minScrollExtent, position.maxScrollExtent),
      );
    });
  }

  /// Says "Queued" once a message just sent has been queued a moment.
  Timer? _relabel;

  /// How many earlier entries the view has seen paged in.
  late int _earlier;

  /// Earlier entries are on their way.
  var _loadingEarlier = false;

  /// Why the earlier entries couldn't load; they load again on Retry.
  String? _earlierFailed;

  /// How near the top (in pixels) earlier entries start loading, so they're
  /// usually there before the person reaches them.
  static const _loadWithin = 1200.0;

  /// Loads earlier entries if the view is near the top and there are some.
  void _maybeLoadEarlier() {
    final load = widget.loadEarlier;
    if (load == null ||
        !mounted ||
        _loadingEarlier ||
        _earlierFailed != null ||
        !widget.transcript.hasEarlier ||
        !_controller.hasClients) {
      return;
    }
    final position = _controller.position;
    if (!position.hasContentDimensions ||
        position.pixels - position.minScrollExtent > _loadWithin) {
      return;
    }
    unawaited(_loadEarlier(load));
  }

  Future<void> _loadEarlier(Future<void> Function() load) async {
    setState(() {
      _loadingEarlier = true;
      _earlierFailed = null;
    });
    String? failed;
    try {
      await load();
    } on RequestFailure catch (failure) {
      failed = failure.message;
    }
    if (!mounted) return;
    setState(() {
      _loadingEarlier = false;
      _earlierFailed = failed;
    });
    // Still near the top (a short page): the next one.
    if (failed == null) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _maybeLoadEarlier());
    }
  }

  /// The person's message on its way that [clientId] says this is (the
  /// backend carries it from the send), or for one sent without it, the
  /// oldest with [text].
  _Waiting? _outgoing(String clientId, String text) {
    for (final waiting in _waiting) {
      final outgoing = waiting.outgoing;
      if (outgoing == null) continue;
      if (clientId.isEmpty
          ? waiting.message.text == text
          : outgoing.clientId == clientId) {
        return waiting;
      }
    }
    return null;
  }

  /// [waiting], the person's message on its way, is now the backend's: it
  /// leaves the outbox and stays where it is.
  void _arrived(_Waiting waiting) {
    final outgoing = waiting.outgoing;
    if (outgoing == null) return;
    waiting.outgoing = null;
    widget.outbox?.remove(outgoing);
  }

  /// The outbox changed: show what was just sent, or failed; drop what
  /// left it otherwise (taken back to edit, or the backend had it and its
  /// own never showed here).
  void _outboxChanged({bool initial = false}) {
    final outbox = widget.outbox;
    if (outbox == null) return;
    final current = outbox.of(widget.agentId);
    var added = false;
    for (final outgoing in current) {
      if (_waiting.any((waiting) => waiting.outgoing == outgoing)) continue;
      _waiting.add(
        _Waiting(
          QueuedMessage(source: 'person', text: outgoing.text),
          initial ? DateTime(0) : DateTime.now(),
          outgoing: outgoing,
        ),
      );
      added = true;
    }
    _waiting.removeWhere(
      (waiting) =>
          waiting.outgoing != null && !current.contains(waiting.outgoing),
    );
    if (initial) return;
    setState(() {});
    if (added && _controller.pinned) _startGlide();
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
      final shown = _waiting.where(
        (w) => w.outgoing == null && w.message.id == message.id,
      );
      final sent = message.source == 'person' && shown.isEmpty
          ? _outgoing(message.clientId, message.text)
          : null;
      if (sent != null) {
        // The person's message just sent: it's queued, where it shows.
        sent
          ..message = message
          ..justSent = true;
        _arrived(sent);
        _relabel?.cancel();
        _relabel = Timer(_sendingFor, () {
          if (!mounted) return;
          setState(() {
            for (final waiting in _waiting) {
              waiting.justSent = false;
            }
          });
        });
      } else if (shown.isEmpty) {
        // Before the person's messages still on their way.
        final at = _waiting.indexWhere((w) => w.outgoing != null);
        _waiting.insert(
          at < 0 ? _waiting.length : at,
          _Waiting(message, initial ? DateTime(0) : now),
        );
        added = true;
      } else {
        // Still queued, or back in the queue: it stays, as it is now (a
        // voice message's transcription moves on).
        shown.first
          ..message = message
          ..leaving?.cancel()
          ..leaving = null
          ..gone = false;
      }
    }
    for (final waiting in _waiting) {
      if (waiting.outgoing == null && !ids.contains(waiting.message.id)) {
        _leave(waiting);
      }
    }
    if (initial) return;
    setState(() {});
    if (added && _controller.pinned) _startGlide();
  }

  /// [waiting] left the queue. Its entry normally arrives at once and takes
  /// its place; if it doesn't (/clear dropped the queue), it fades out, then
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
    if (old.outbox != widget.outbox) {
      old.outbox?.removeListener(_outboxChanged);
      widget.outbox?.addListener(_outboxChanged);
    }
    if (old.reveal != widget.reveal) {
      old.reveal?.removeListener(_revealChanged);
      widget.reveal?.addListener(_revealChanged);
    }
    if (old.extras != widget.extras) {
      old.extras?.removeListener(_extrasChanged);
      widget.extras?.addListener(_extrasChanged);
      _extrasChanged();
    }
  }

  @override
  void dispose() {
    _selecting.dispose();
    _pointed.dispose();
    widget.follow.removeListener(_followAgain);
    widget.transcript.removeListener(_entriesChanged);
    widget.queued?.removeListener(_queueChanged);
    widget.outbox?.removeListener(_outboxChanged);
    widget.reveal?.removeListener(_revealChanged);
    widget.extras?.removeListener(_extrasChanged);
    _relabel?.cancel();
    for (final waiting in _waiting) {
      waiting.leaving?.cancel();
    }
    _glide.dispose();
    _controller.dispose();
    _heights.dispose();
    super.dispose();
  }

  bool get _reducedMotion => MediaQuery.disableAnimationsOf(context);

  /// New entries arrived: note when, and glide to them if following.
  void _entriesChanged() {
    if (widget.reveal?.extra != null) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _revealExtra());
    }
    final transcript = widget.transcript;
    final length = transcript.length;
    // Earlier entries paged in go above the center line, so what's on
    // screen stays where it is.
    final earlier = transcript.earlier - _earlier;
    if (earlier > 0) {
      _earlier = transcript.earlier;
      _split += earlier;
      _known += earlier;
    }
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
    var clientId = '';
    if (entry.hasUser()) {
      (text, person) = (entry.user.text, true);
      clientId = entry.user.clientId;
    } else if (entry.hasEvent()) {
      (text, person) = (entry.event.text, false);
    } else {
      return;
    }
    bool same(_Waiting waiting) {
      final message = waiting.message;
      if ((message.source == 'person') != person) return false;
      if (clientId.isEmpty) return message.text == text;
      return (waiting.outgoing?.clientId ?? message.clientId) == clientId;
    }

    for (final waiting in _waiting) {
      final message = waiting.message;
      if (!waiting.gone && same(waiting)) {
        waiting.leaving?.cancel();
        _waiting.remove(waiting);
        if (waiting.outgoing != null) {
          _arrived(waiting);
        } else {
          _delivered.add(message.id);
        }
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
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) _anchorAtEnd();
      });
    }
  }

  double? _liveExtent() {
    final sliver = _liveKey.currentContext?.findRenderObject();
    return sliver is RenderSliver ? sliver.geometry?.scrollExtent : null;
  }

  /// The view is being put at a place of its own choosing: not the person
  /// scrolling, so following stays as it was set.
  var _jumping = false;

  void _scrolled() {
    if (!_controller.hasClients || _jumping) return;
    final position = _controller.position;
    if (_controller.gliding) {
      // The glide's own step; anything else is the user taking over.
      if ((position.pixels - (_glidePixels ?? -1)).abs() < 0.5) return;
      _controller.gliding = false;
      _glide.stop();
    }
    _controller.pinned = _atEnd();
    _maybeLoadEarlier();
    // The end may come into the list only once laid out at this place.
    if (!_settling) {
      _settling = true;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        _settling = false;
        if (!mounted || !_controller.hasClients || _controller.gliding) return;
        _controller.pinned = _atEnd();
        _anchorAtEnd();
      });
    }
  }

  /// A check of [_atEnd] is due after the next layout.
  var _settling = false;

  /// Whether the view is at the true bottom: within a few pixels of the
  /// end, and the list below the center line laid out to its last item.
  /// Before that, its end is only an estimate (after going far back to a
  /// card, say), and following it would leap when the estimate's put right.
  bool _atEnd() {
    final position = _controller.position;
    if (position.pixels < position.maxScrollExtent - 8) return false;
    final padding = _liveKey.currentContext?.findRenderObject();
    final list = padding is RenderSliverPadding ? padding.child : null;
    if (list is! RenderSliverMultiBoxAdaptor) return true;
    final count =
        widget.transcript.length -
        math.min(_split, widget.transcript.length) +
        _waiting.length;
    if (count == 0) return true;
    final last = list.lastChild;
    return last != null && list.indexOf(last) == count - 1;
  }

  /// How many items may be below the center line before following the end
  /// moves the line down to it.
  static const _anchorAfter = 30;

  /// Following the end with many entries below the center line (after
  /// going back to a card and down again, or a long session), moves the
  /// line to the newest entry: all else is laid out upward from it, so
  /// what changes above can't move the bottom, and the end is exact. Left
  /// for later while the person is typing in the conversation (in a card).
  void _anchorAtEnd() {
    final length = widget.transcript.length;
    if (!_controller.pinned ||
        _controller.gliding ||
        length - _split <= _anchorAfter) {
      return;
    }
    final focus = FocusManager.instance.primaryFocus?.context;
    if (focus != null &&
        focus.findAncestorStateOfType<_TranscriptViewState>() == this) {
      return;
    }
    setState(() => _split = length - 1);
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

  Widget _measured(int index) =>
      MeasuredItem(id: _idOf(index), heights: _heights, child: _item(index));

  Widget _item(int index) {
    final transcript = widget.transcript;
    if (index >= transcript.length) {
      return _queuedItem(_waiting[index - transcript.length]);
    }
    return _entryItem(index, transcript[index]);
  }

  Widget _entryItem(int index, EntryModel model) {
    final before = index == 0 ? _placed.before : const <TimelineItem>[];
    final after = _placed.after[index] ?? const <TimelineItem>[];
    final shown = !(widget.hidden?.call(model.entry) ?? false);
    final entry = shown ? _entryOnly(index, model) : null;
    final day = shown ? _dayChange(index) : null;
    if (before.isEmpty && after.isEmpty && day == null) {
      return entry ?? const SizedBox.shrink();
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (final item in before) _extraItem(item),
        if (day != null) DayLine(ms: day),
        ?entry,
        for (final item in after) _extraItem(item),
      ],
    );
  }

  /// An extra ([TranscriptView.extras]) in the conversation's column, a
  /// message's gap above it, easing in when it just came.
  Widget _extraItem(TimelineItem item) => Align(
    alignment: Alignment.topCenter,
    child: ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: contentMaxWidth),
      child: Padding(
        padding: EdgeInsets.only(
          top: Space.l,
          left: Metrics.of(context).gutter,
          right: Metrics.of(context).gutter,
        ),
        child: Arrival(
          animate:
              !_reducedMotion &&
              DateTime.now().difference(
                    _extraArrivals[item.key] ?? DateTime(0),
                  ) <
                  _glideDuration,
          child: KeyedSubtree(
            key: _extraKey(item.key),
            child: Builder(builder: item.build),
          ),
        ),
      ),
    ),
  );

  /// The time of the entry at [index] when a new day starts there (it's
  /// the first entry of the conversation, or the last one shown before it
  /// was on another day), for a day line above it; null otherwise.
  int? _dayChange(int index) {
    final transcript = widget.transcript;
    final ms = transcript[index].entry.timestampMs.toInt();
    if (ms <= 0) return null;
    // Inside a closed run of thoughts nothing shows to mark.
    final run = thinkingRunAt(transcript, index, hidden: widget.hidden);
    if (run != null && !run.first) return null;
    for (var i = index - 1; i >= 0; i--) {
      final entry = transcript[i].entry;
      if (widget.hidden?.call(entry) ?? false) continue;
      final before = entry.timestampMs.toInt();
      if (before <= 0) continue;
      return sameDay(before, ms) ? null : ms;
    }
    // The first held: the conversation's first, unless earlier ones are
    // still to load.
    return transcript.hasEarlier ? null : ms;
  }

  Widget _entryOnly(int index, EntryModel model) {
    final transcript = widget.transcript;
    final fromQueue = _fromQueue.contains(model.seq);
    return Align(
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: contentMaxWidth),
        child: Padding(
          padding: EdgeInsets.symmetric(horizontal: Metrics.of(context).gutter),
          child: Arrival(
            // A delivered message brightens where its queued bubble was.
            from: fromQueue ? _queuedOpacity(context) : 0,
            rise: !fromQueue,
            // Only an entry built as it arrives eases in; one built later,
            // as the user scrolls to it, is just there.
            animate:
                !_reducedMotion &&
                DateTime.now().difference(_arrivals[model.seq] ?? DateTime(0)) <
                    _glideDuration,
            child: Builder(
              builder: (context) {
                final run = thinkingRunAt(
                  transcript,
                  index,
                  hidden: widget.hidden,
                  working: widget.working,
                  // An extra after a thought shows there: the run ends.
                  ends: (index) => _placed.after[index]?.isNotEmpty ?? false,
                );
                return EntryView(
                  model: model,
                  block: model.entry.hasToolCall()
                      ? toolBlockAt(transcript, index, hidden: widget.hidden)
                      : const ToolBlock(first: true, last: true),
                  live: widget.working && index == transcript.length - 1,
                  expanded: _expanded.contains(model.seq),
                  onToggle: (open) => _toggled(model.seq, open),
                  run: run,
                  runExpanded:
                      run != null && _expandedRuns.contains(run.firstSeq),
                  onRunToggle: run == null
                      ? null
                      : (open) => setState(() {
                          if (open) {
                            _expandedRuns.add(run.firstSeq);
                          } else {
                            _expandedRuns.remove(run.firstSeq);
                          }
                        }),
                );
              },
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
    final files = waiting.outgoing?.files ?? const <OutgoingFile>[];
    // A voice message: on its way, or being transcribed (or failed). Once
    // its words are there, it waits as they are, with ▶.
    final heard =
        message.hasTranscription() &&
        message.transcription.state ==
            TranscriptionState.TRANSCRIPTION_STATE_DONE;
    final voice =
        waiting.outgoing?.voice ??
        (message.hasTranscription() && !heard
            ? Duration(milliseconds: message.transcription.durationMs.toInt())
            : null);
    final voiceFailed =
        waiting.outgoing == null &&
        message.transcription.state ==
            TranscriptionState.TRANSCRIPTION_STATE_FAILED;
    final (Widget bubble, CrossAxisAlignment side) = switch (message.source) {
      'person' when voice != null => (
        _UserText('', waitingVoice: voice),
        CrossAxisAlignment.end,
      ),
      'person' when files.isNotEmpty => (
        Column(
          crossAxisAlignment: CrossAxisAlignment.end,
          children: [
            if (message.text.isNotEmpty) ...[
              _UserText(message.text),
              const SizedBox(height: Space.xs),
            ],
            // Each file as it uploads.
            Wrap(
              alignment: WrapAlignment.end,
              spacing: Space.xs,
              runSpacing: Space.xs,
              children: [
                for (final file in files)
                  AttachmentChip(
                    name: file.file.name,
                    size: file.file.size,
                    progress: file.done == null ? file.progress : null,
                  ),
              ],
            ),
          ],
        ),
        CrossAxisAlignment.end,
      ),
      'person' => (
        _UserText(
          message.text,
          files: message.attachments,
          voice: heard ? message.transcription : null,
        ),
        CrossAxisAlignment.end,
      ),
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
          style: TypeScale.of(context).caption
              .copyWith(color: palette.textFaint),
        ),
        CrossAxisAlignment.center,
      ),
    };
    final outgoing = waiting.outgoing;
    final failure = outgoing?.failure;
    final sending = outgoing != null || waiting.justSent;
    // Under the bubble: where it was sent from, and Queued or Sending…
    // (or how a voice message's transcription goes).
    final Widget? status = failure == null && !voiceFailed
        ? Padding(
            padding: EdgeInsets.only(
              top: 2,
              left: Space.m,
              // Under the bubble, clear of the avatar.
              right: message.source == 'person'
                  ? Space.m + userAvatarSize + Space.xs
                  : Space.m,
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (message.source == 'person' && message.hasSentFrom()) ...[
                  DeviceMark(message.sentFrom),
                  const SizedBox(width: Space.xs),
                ],
                // A voice message says how its
                // transcription goes instead.
                if (voice != null && !sending)
                  const TranscribingLine()
                else
                  // Wraps: a quiet one's words name the agent.
                  Flexible(
                    child: Text(
                      sending
                          ? 'Sending…'
                          // A quiet one waits for the agent to start working.
                          : message.quiet
                          ? quietQueuedLabel(
                              AppScope.of(context).workspace
                                      .agent(widget.agentId)
                                      ?.name ??
                                  'the agent',
                            )
                          : 'Queued',
                      style: TypeScale.of(context).caption
                          .copyWith(color: palette.textFaint),
                    ),
                  ),
              ],
            ),
          )
        : null;
    final statusClear = isPhoneLayout(context);
    return Align(
      key: ObjectKey(waiting),
      alignment: Alignment.topCenter,
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: contentMaxWidth),
        child: Padding(
          padding: EdgeInsets.fromLTRB(
            Metrics.of(context).gutter,
            Space.l,
            Metrics.of(context).gutter,
            0,
          ),
          child: AnimatedOpacity(
            opacity: waiting.gone ? 0 : 1,
            duration: Motion.fast,
            curve: Curves.easeOut,
            child: Arrival(
              animate:
                  !_reducedMotion &&
                  DateTime.now().difference(waiting.shown) < _glideDuration,
              child: Column(
                crossAxisAlignment: side,
                children: [
                  Opacity(
                    opacity: _queuedOpacity(context),
                    child: Column(
                      crossAxisAlignment: side,
                      children: [bubble, if (!statusClear) ?status],
                    ),
                  ),
                  // On a phone, what it waits for is read at full strength.
                  if (statusClear) ?status,
                  if (voiceFailed)
                    Padding(
                      padding: const EdgeInsets.only(
                        top: 2,
                        left: Space.m,
                        right: Space.m + userAvatarSize + Space.xs,
                      ),
                      child: VoiceFailed(
                        agentId: widget.agentId,
                        message: message,
                      ),
                    ),
                  if (outgoing != null && failure != null)
                    _NotSent(
                      why: failure,
                      onRetry: () => widget.outbox?.retry(outgoing),
                      onEdit: () => _edit(outgoing),
                    ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }

  /// Takes a message that wasn't sent back to the message box, to change
  /// it; after anything already typed there.
  void _edit(Outgoing outgoing) {
    final scope = AppScope.scope(context);
    final drafts = scope.drafts;
    final draft = drafts.of(outgoing.agentId);
    if (outgoing.text.isNotEmpty) {
      drafts.set(
        outgoing.agentId,
        draft.trim().isEmpty ? outgoing.text : '$draft\n\n${outgoing.text}',
      );
    }
    unawaited(
      scope.backend.attachments.add(outgoing.agentId, [
        for (final file in outgoing.files) file.file.path,
      ]),
    );
    widget.outbox?.takeBack(outgoing);
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: widget.transcript,
      builder: (context, _) {
        final length = widget.transcript.length;
        final split = math.min(_split, length);
        final hasEarlier =
            widget.transcript.hasEarlier && widget.loadEarlier != null;
        _placed = _place();
        if (hasEarlier) {
          // Shorter than the view, or opened at the top: load at once.
          WidgetsBinding.instance.addPostFrameCallback(
            (_) => _maybeLoadEarlier(),
          );
        }
        return ToolViewScope(
          transcript: widget.transcript,
          agentId: widget.transcript.agentId,
          child: _WithLatest(
            away: _controller.away,
            transcript: widget.transcript,
            onTap: _followAgain,
            child: SelectableArea(
              onSelectionChanged: (content) => _selecting.value =
                  content != null && content.plainText.isNotEmpty,
              // Reply, for the message the right click is on.
              menuItems: () => [
                if (_pointed.value case final pointed?)
                  MenuItem('Reply', () => _reply(pointed)),
              ],
              child: MessageSelection(
                selecting: _selecting,
                child: _ReplyScope(
                  pointed: _pointed,
                  reply: _reply,
                  child: ConversationScrollbar(
                    controller: _controller,
                    heights: _heights,
                    order: _order,
                    unseen: _unseen,
                    after: Space.xl,
                    child: CustomScrollView(
                      controller: _controller,
                      center: _liveKey,
                      // On a phone, scrolling the conversation tucks the
                      // keyboard away, as Messages does.
                      keyboardDismissBehavior: isPhoneLayout(context)
                          ? ScrollViewKeyboardDismissBehavior.onDrag
                          : ScrollViewKeyboardDismissBehavior.manual,
                      slivers: [
                        SliverList(
                          delegate: SliverChildBuilderDelegate(
                            (context, index) => index == split
                                ? MeasuredItem(
                                    id: 'earlier',
                                    heights: _heights,
                                    child: _EarlierRow(
                                      failed: _earlierFailed,
                                      onRetry: () {
                                        setState(() => _earlierFailed = null);
                                        _maybeLoadEarlier();
                                      },
                                    ),
                                  )
                                : _measured(split - 1 - index),
                            childCount: split + (hasEarlier ? 1 : 0),
                          ),
                        ),
                        // With no entries yet, the extras on their own,
                        // just above the center line.
                        if (length == 0 && _placed.before.isNotEmpty)
                          SliverToBoxAdapter(
                            child: MeasuredItem(
                              id: 'extras',
                              heights: _heights,
                              child: Column(
                                crossAxisAlignment: CrossAxisAlignment.stretch,
                                children: [
                                  for (final item in _placed.before)
                                    _extraItem(item),
                                ],
                              ),
                            ),
                          ),
                        SliverPadding(
                          key: _liveKey,
                          padding: const EdgeInsets.only(bottom: Space.xl),
                          sliver: SliverList(
                            delegate: SliverChildBuilderDelegate(
                              (context, index) => _measured(split + index),
                              childCount: length - split + _waiting.length,
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

/// While the conversation is scrolled up away from its end: a button to go
/// back down and follow it again, saying how many replies came in
/// meanwhile. On a phone a pill at its foot (research mobile.md §4); on a
/// Mac a small round button at its bottom right, "3 new" beside its arrow
/// when replies came in.
class _WithLatest extends StatefulWidget {
  const _WithLatest({
    required this.away,
    required this.transcript,
    required this.onTap,
    required this.child,
  });

  final ValueListenable<bool> away;
  final TranscriptStore transcript;
  final VoidCallback onTap;
  final Widget child;

  @override
  State<_WithLatest> createState() => _WithLatestState();
}

class _WithLatestState extends State<_WithLatest> {
  /// The transcript's length when the view left its end.
  int? _from;

  @override
  void initState() {
    super.initState();
    widget.away.addListener(_changed);
    widget.transcript.addListener(_changed);
    if (widget.away.value) _from = widget.transcript.length;
  }

  @override
  void didUpdateWidget(_WithLatest old) {
    super.didUpdateWidget(old);
    if (old.away != widget.away) {
      old.away.removeListener(_changed);
      widget.away.addListener(_changed);
    }
    if (old.transcript != widget.transcript) {
      old.transcript.removeListener(_changed);
      widget.transcript.addListener(_changed);
    }
  }

  @override
  void dispose() {
    widget.away.removeListener(_changed);
    widget.transcript.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    if (!mounted) return;
    // Following can change while the list lays out: show it after.
    if (SchedulerBinding.instance.schedulerPhase ==
        SchedulerPhase.persistentCallbacks) {
      SchedulerBinding.instance.addPostFrameCallback((_) => _changed());
      return;
    }
    final away = widget.away.value;
    if (away == (_from != null) && !away) return;
    setState(() {
      if (!away) {
        _from = null;
      } else {
        _from ??= widget.transcript.length;
      }
    });
  }

  /// Replies and messages from agents since the view left the end.
  int get _new {
    final from = _from;
    if (from == null) return 0;
    var count = 0;
    for (var i = from; i < widget.transcript.length; i++) {
      final entry = widget.transcript[i].entry;
      if ((entry.hasAssistant() && entry.assistant.text.isNotEmpty) ||
          (entry.hasEvent() &&
              entry.event.source == EventSource.EVENT_SOURCE_AGENT)) {
        count++;
      }
    }
    return count;
  }

  /// What it says to VoiceOver.
  static String _label(int count) => count == 0
      ? 'Go to the latest message'
      : '$count new ${count == 1 ? 'message' : 'messages'}, go to the latest';

  /// A Mac's: at the bottom right, over the conversation's end.
  Widget _mac(BuildContext context, bool shown, int count) {
    final palette = Palette.of(context);
    return Stack(
      children: [
        Positioned.fill(child: widget.child),
        Positioned(
          right: Space.l,
          bottom: Space.m,
          child: IgnorePointer(
            ignoring: !shown,
            child: AnimatedOpacity(
              opacity: shown ? 1 : 0,
              duration: Motion.fast,
              child: ExcludeSemantics(
                excluding: !shown,
                child: Semantics(
                  container: true,
                  button: true,
                  label: _label(count),
                  onTap: widget.onTap,
                  excludeSemantics: true,
                  child: Pressable(
                    key: const ValueKey('latest'),
                    onTap: widget.onTap,
                    builder: (context, hovered) => AnimatedContainer(
                      duration: Motion.fast,
                      curve: Curves.easeOut,
                      // A pointer's 28 on a Mac; a finger's on a wide iPad
                      // or Android window.
                      height: Metrics.of(context).target,
                      constraints: BoxConstraints(
                        minWidth: Metrics.of(context).target,
                      ),
                      padding: EdgeInsets.only(
                        left: count == 0 ? 0 : Space.s + 2,
                        right: count == 0 ? 0 : Space.s,
                      ),
                      decoration: BoxDecoration(
                        color: hovered ? palette.selected : palette.sidebar,
                        borderRadius: BorderRadius.circular(Radii.floating),
                        border: Border.all(color: palette.hairline),
                        boxShadow: floatingShadow,
                      ),
                      child: Row(
                        mainAxisSize: MainAxisSize.min,
                        mainAxisAlignment: MainAxisAlignment.center,
                        children: [
                          if (count > 0) ...[
                            Text(
                              '$count new',
                              style: TextStyles.label.copyWith(
                                color: palette.text,
                                fontWeight: FontWeight.w500,
                                fontFeatures: const [
                                  FontFeature.tabularFigures(),
                                ],
                              ),
                            ),
                            const SizedBox(width: Space.xs),
                          ],
                          ExtensionGlyph(
                            icon: 'arrow-down',
                            name: 'Latest',
                            color: hovered ? palette.text : palette.textMuted,
                            size: 14,
                          ),
                        ],
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }

  @override
  Widget build(BuildContext context) {
    final shown = _from != null;
    final count = _new;
    if (!isPhoneLayout(context)) return _mac(context, shown, count);
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Stack(
      children: [
        Positioned.fill(child: widget.child),
        Positioned(
          left: 0,
          right: 0,
          bottom: Space.s,
          child: Center(
            child: IgnorePointer(
              ignoring: !shown,
              child: AnimatedOpacity(
                opacity: shown ? 1 : 0,
                duration: Motion.fast,
                child: ExcludeSemantics(
                  excluding: !shown,
                  child: Semantics(
                    container: true,
                    button: true,
                    label: _label(count),
                    onTap: widget.onTap,
                    excludeSemantics: true,
                    child: Pressable(
                      onTap: () {
                        unawaited(Haptics.selection());
                        widget.onTap();
                      },
                      // A floating control: its words grow only so far,
                      // so it never covers the conversation it leads to.
                      builder: (context, pressed) =>
                          MediaQuery.withClampedTextScaling(
                            maxScaleFactor: 1.5,
                            child: Container(
                              constraints: BoxConstraints(
                                minHeight: PhonePlatformPolicy.of(context)
                                    .touchTarget,
                              ),
                              padding: const EdgeInsets.symmetric(
                                horizontal: Space.l,
                              ),
                              decoration: BoxDecoration(
                                color: pressed
                                    ? palette.selected
                                    : palette.sidebar,
                                borderRadius: BorderRadius.circular(
                                  PhonePlatformPolicy.of(context).touchTarget /
                                      2,
                                ),
                                border: Border.all(color: palette.hairline),
                                boxShadow: const [
                                  BoxShadow(
                                    color: Color(0x40000000),
                                    blurRadius: 12,
                                    offset: Offset(0, 4),
                                  ),
                                ],
                              ),
                              child: Row(
                                mainAxisSize: MainAxisSize.min,
                                children: [
                                  ExtensionGlyph(
                                    icon: 'arrow-down',
                                    name: 'Latest',
                                    color: palette.text,
                                    size: 16,
                                  ),
                                  const SizedBox(width: Space.xs),
                                  Text(
                                    count == 0 ? 'Latest' : '$count new',
                                    style: type.label.copyWith(
                                      color: palette.text,
                                    ),
                                  ),
                                ],
                              ),
                            ),
                          ),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

/// At the top of a conversation with earlier entries still to load: a
/// quiet line while they load, or why they couldn't, with Retry.
class _EarlierRow extends StatelessWidget {
  const _EarlierRow({required this.failed, required this.onRetry});

  final String? failed;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final failed = this.failed;
    return SizedBox(
      height: 48,
      child: Center(
        child: failed == null
            ? Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  LiveDot(color: palette.textFaint),
                  const SizedBox(width: Space.s),
                  Text(
                    'Loading earlier messages…',
                    style: TypeScale.of(context).caption
                        .copyWith(color: palette.textFaint),
                  ),
                ],
              )
            : Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Flexible(
                    child: Text(
                      "Couldn't load earlier messages. $failed",
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TypeScale.of(context).caption
                          .copyWith(color: palette.danger),
                    ),
                  ),
                  const SizedBox(width: Space.s),
                  Pressable(
                    onTap: onRetry,
                    builder: (context, hovered) => Text(
                      'Retry',
                      style: TypeScale.of(context).caption.copyWith(
                        color: hovered ? palette.text : palette.textMuted,
                        fontWeight: FontWeight.w500,
                      ),
                    ),
                  ),
                ],
              ),
      ),
    );
  }
}

/// A scroll controller that keeps its position at the bottom while [pinned].
class _FollowController extends ScrollController {
  _FollowController(this.liveExtent);

  /// The extent of the content below the center line, read during layout.
  final double? Function() liveExtent;

  /// Following the end; when not, a phone offers Latest to go back to it.
  bool get pinned => !away.value;
  set pinned(bool value) => away.value = !value;

  /// Scrolled up away from the end: [pinned]'s opposite, to listen to.
  final away = ValueNotifier(false);

  @override
  void dispose() {
    away.dispose();
    super.dispose();
  }

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
    this.run,
    this.runExpanded = false,
    this.onRunToggle,
  });

  final EntryModel model;

  /// For a thought, or nothing shown between thoughts, the run of thoughts
  /// it's in ([thinkingRunAt]); null when it stands alone.
  final ThinkingRun? run;

  /// The run is open: each thought shows as it would alone.
  final bool runExpanded;
  final ValueChanged<bool>? onRunToggle;

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
    } else if (old.live != widget.live ||
        old.block != widget.block ||
        old.run != widget.run ||
        old.runExpanded != widget.runExpanded) {
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

  /// An entry in a run of thoughts: the first shows the run's row, and
  /// under it, while the run is open, each thought as it would alone;
  /// closed, the rest show nothing. Anything else in a run (a reply with
  /// no words) shows nothing either.
  Widget _inRun(Entry entry, int seq, ThinkingRun run) {
    final open = widget.runExpanded;
    final thought = entry.hasThinking() && open
        ? Padding(
            padding: EdgeInsets.only(
              left: Space.l + 2,
              top: run.first ? Space.xs : Space.s,
            ),
            child: _ThinkingView(
              key: ValueKey(seq),
              text: entry.thinking.text,
              live: widget.live,
              expanded: widget.expanded,
              onToggle: widget.onToggle,
            ),
          )
        : null;
    if (!run.first) return thought ?? const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.only(top: Space.m),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _ThinkingRunRow(
            run: run,
            open: open,
            onToggle: () => widget.onRunToggle?.call(!open),
          ),
          ?thought,
        ],
      ),
    );
  }

  /// On a phone, the hold's Reply: quotes the message into the box.
  VoidCallback? _replyTo(String text, String? from) {
    final scope = context.getInheritedWidgetOfExactType<_ReplyScope>();
    if (scope == null) return null;
    return () => scope.reply((text: text, from: from));
  }

  Widget _build() {
    final entry = widget.model.entry;
    final seq = widget.model.seq;
    final run = widget.run;
    if (run != null) return _inRun(entry, seq, run);
    final (Widget child, double gap) = switch (entry.whichKind()) {
      Entry_Kind.user => (
        _UserText(
          entry.user.text,
          sentFrom: entry.user.hasSentFrom() ? entry.user.sentFrom : null,
          files: entry.user.attachments,
          voice: entry.user.hasVoice() ? entry.user.voice : null,
          sentMs: entry.timestampMs.toInt(),
        ),
        Space.l,
      ),
      Entry_Kind.proposal => (
        ProposalCard(key: ValueKey(entry.proposal.id), entry.proposal),
        Space.l,
      ),
      Entry_Kind.event =>
        entry.event.source == EventSource.EVENT_SOURCE_AGENT
            ? (
                _AgentMessage(entry.event, sentMs: entry.timestampMs.toInt()),
                Space.l,
              )
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
            : _OwnReply(
                entry.assistant.text,
                answers: repliedTo(entry),
                seq: seq,
                sentMs: entry.timestampMs.toInt(),
              ),
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
            seq: seq,
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
      Entry_Kind.secretRequest => (
        SecretRequestCard(
          key: ValueKey(entry.secretRequest.id),
          entry.secretRequest,
          // An admin agent's conversation shows the secret it collects.
          shownIn: AppScope.of(context).transcript.agentId,
        ),
        Space.l,
      ),
      Entry_Kind.connectProposal => (
        ConnectProposalCard(
          key: ValueKey(entry.connectProposal.id),
          entry.connectProposal,
        ),
        Space.l,
      ),
      Entry_Kind.devicePairing => (
        DevicePairingCard(
          key: ValueKey(entry.devicePairing.id),
          entry.devicePairing,
        ),
        Space.l,
      ),
      Entry_Kind.secretApproval => (
        SecretApprovalCard(key: ValueKey(seq), entry.secretApproval, seq: seq),
        Space.l,
      ),
      Entry_Kind.pipeStart => (
        PipeStartCard(key: ValueKey(entry.pipeStart.id), entry.pipeStart),
        Space.l,
      ),
      // Priya's ask card (#208) replaces this.
      Entry_Kind.askPerson => (
        AskPersonCard(
          key: ValueKey(seq),
          entry.askPerson,
          agentId: AppScope.of(context).transcript.agentId,
          seq: seq,
        ),
        Space.l,
      ),
      Entry_Kind.appGrant => (
        AppGrantCard(
          key: ValueKey(entry.appGrant.id),
          entry.appGrant,
          agentId: AppScope.of(context).transcript.agentId,
          seq: seq,
        ),
        Space.l,
      ),
      Entry_Kind.notSet => (const SizedBox.shrink(), 0.0),
    };
    // A message's words, for its Copy and Select text on a phone and its
    // Reply; and who sent it, when it's another agent.
    final words = switch (entry.whichKind()) {
      Entry_Kind.user => entry.user.text,
      Entry_Kind.assistant => entry.assistant.text,
      Entry_Kind.event
          when entry.event.source == EventSource.EVENT_SOURCE_AGENT =>
        splitAlsoSentTo(entry.event.text).$1,
      _ => null,
    };
    final from = entry.whichKind() == Entry_Kind.event
        ? senderName(
            entry.event,
            context.getInheritedWidgetOfExactType<AgentTints>()?.agents,
          )
        : null;
    return Padding(
      padding: EdgeInsets.only(top: gap),
      child: words == null
          ? child
          : _Repliable(
              text: words,
              from: from,
              child: MessageActions(
                text: words,
                markdown: entry.whichKind() != Entry_Kind.user,
                onReply: _replyTo(words, from),
                child: child,
              ),
            ),
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

/// The person's avatar beside their messages.
const userAvatarSize = 22.0;

/// The person's message: a bubble on the right in the agent's color, their
/// avatar at its side, and a mark under it for the device it was sent from.
class _UserText extends StatelessWidget {
  const _UserText(
    this.text, {
    this.sentFrom,
    this.files = const [],
    this.voice,
    this.waitingVoice,
    this.sentMs = 0,
  });

  final String text;

  /// When it was sent, at the bubble's foot; 0 for none (still waiting).
  final int sentMs;

  /// Said rather than typed: [text] is what was heard, files[0] the
  /// recording, played from ▶ under it.
  final Transcription? voice;

  /// A voice message not transcribed yet, this long: its length shows
  /// instead of words.
  final Duration? waitingVoice;

  /// Where it was sent from; null when the backend doesn't know (sent
  /// before it kept that), and then nothing shows.
  final SentFrom? sentFrom;

  /// The files sent with it, under the bubble (or on their own).
  final List<Attachment> files;

  @override
  Widget build(BuildContext context) {
    final color = AgentTints.of(context)?.own ?? Palette.of(context).signal;
    final words = Text(
      text,
      style: TypeScale.of(context).body
          .copyWith(color: const Color(0xFFFFFFFF)),
    );
    final voice = this.voice;
    final waiting = waitingVoice;
    final bubble = Bubble(
      color: color,
      outgoing: true,
      sentMs: sentMs,
      timeColor: const Color(0xB3FFFFFF),
      child: waiting != null
          ? VoiceWaiting(duration: waiting)
          : voice != null && this.files.isNotEmpty
          ? Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                words,
                const SizedBox(height: Space.xs),
                VoicePlay(
                  path: this.files.first.path,
                  duration: Duration(milliseconds: voice.durationMs.toInt()),
                ),
              ],
            )
          : words,
    );
    // A voice message's recording plays from its bubble.
    final files = voice != null || waiting != null
        ? const <Attachment>[]
        : this.files;
    final from = sentFrom;
    // The person's avatar beside it, at its foot, as an agent's character
    // heads its messages.
    final message = Row(
      crossAxisAlignment: CrossAxisAlignment.end,
      children: [
        Expanded(
          child: files.isEmpty
              ? bubble
              : Column(
                  crossAxisAlignment: CrossAxisAlignment.end,
                  children: [
                    if (text.isNotEmpty) bubble,
                    Padding(
                      padding: EdgeInsets.only(
                        top: text.isEmpty ? 0 : Space.xs,
                      ),
                      child: SentFiles(files),
                    ),
                  ],
                ),
        ),
        const SizedBox(width: Space.xs),
        const PersonAvatar(size: userAvatarSize),
      ],
    );
    if (from == null) return message;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.end,
      children: [
        message,
        Padding(
          padding: const EdgeInsets.only(
            right: Space.m + userAvatarSize + Space.xs,
            top: 2,
          ),
          child: DeviceMark(from),
        ),
      ],
    );
  }
}

/// The files sent with one of the person's messages, at its side: each
/// picture as a thumbnail, any other file as a chip, both opening in a
/// viewer beside the conversation.
class SentFiles extends StatelessWidget {
  const SentFiles(this.files, {super.key});

  final List<Attachment> files;

  @override
  Widget build(BuildContext context) => Wrap(
    alignment: WrapAlignment.end,
    spacing: Space.xs,
    runSpacing: Space.xs,
    children: [
      for (final file in files)
        isImageName(file.name)
            ? _Thumbnail(file)
            : AttachmentChip(
                name: file.name,
                size: file.size.toInt(),
                onOpen: () => openFile(context, file.path),
              ),
    ],
  );
}

/// A picture sent with a message, fitted in 240×180 at most, 10px radius
/// and a hairline; it opens in the image viewer. One too big to fetch here,
/// or that can't be shown, is a chip instead.
class _Thumbnail extends StatefulWidget {
  const _Thumbnail(this.file);

  final Attachment file;

  /// The biggest picture fetched for a thumbnail.
  static const maxBytes = 8 * 1024 * 1024;

  /// Pictures fetched, by path, the latest last: a conversation scrolled
  /// back and forth doesn't fetch them again.
  static final _cache = <String, Uint8List>{};
  static const _kept = 40;

  @override
  State<_Thumbnail> createState() => _ThumbnailState();
}

class _ThumbnailState extends State<_Thumbnail> {
  Uint8List? _bytes;
  var _failed = false;

  @override
  void initState() {
    super.initState();
    final file = widget.file;
    final cached = _Thumbnail._cache.remove(file.path);
    if (cached != null) {
      _Thumbnail._cache[file.path] = cached;
      _bytes = cached;
    } else if (file.size.toInt() > _Thumbnail.maxBytes) {
      _failed = true;
    } else {
      unawaited(_fetch());
    }
  }

  Future<void> _fetch() async {
    try {
      final content = await AppScope.of(context)
          .readFile(widget.file.path, maxBytes: _Thumbnail.maxBytes);
      if (content.error.isNotEmpty || content.truncated) {
        throw const RequestFailure('');
      }
      final bytes = Uint8List.fromList(content.data);
      _Thumbnail._cache[widget.file.path] = bytes;
      while (_Thumbnail._cache.length > _Thumbnail._kept) {
        _Thumbnail._cache.remove(_Thumbnail._cache.keys.first);
      }
      if (mounted) setState(() => _bytes = bytes);
    } on RequestFailure {
      if (mounted) setState(() => _failed = true);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final file = widget.file;
    void open() => openFile(context, file.path);
    final chip = AttachmentChip(
      name: file.name,
      size: file.size.toInt(),
      onOpen: open,
    );
    if (_failed) return chip;
    final bytes = _bytes;
    final ratio = MediaQuery.devicePixelRatioOf(context);
    return Semantics(
      image: true,
      label: file.name,
      child: Pressable(
        onTap: open,
        builder: (context, hovered) => Container(
          constraints: const BoxConstraints(maxWidth: 240, maxHeight: 180),
          decoration: BoxDecoration(
            color: palette.sidebar,
            borderRadius: BorderRadius.circular(Radii.panel),
            border: Border.all(color: palette.hairline),
          ),
          clipBehavior: Clip.antiAlias,
          child: bytes == null
              // On its way: the room it will most likely take.
              ? const SizedBox(width: 160, height: 120)
              : Image.memory(
                  bytes,
                  cacheWidth: (240 * ratio).round(),
                  fit: BoxFit.contain,
                  gaplessPlayback: true,
                  errorBuilder: (context, error, stack) => chip,
                ),
        ),
      ),
    );
  }
}

/// Under a message the backend never got: "Not sent" in `danger` (why on
/// hover, or when held), then Retry and Edit, which takes it back to the
/// message box.
class _NotSent extends StatelessWidget {
  const _NotSent({
    required this.why,
    required this.onRetry,
    required this.onEdit,
  });

  final String why;
  final VoidCallback onRetry;
  final VoidCallback onEdit;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    Widget action(String label, VoidCallback onTap) => Pressable(
      onTap: onTap,
      builder: (context, hovered) => Text(
        label,
        style: TypeScale.of(context).caption.copyWith(
          color: hovered ? palette.text : palette.textMuted,
          fontWeight: FontWeight.w500,
        ),
      ),
    );
    return Padding(
      padding: const EdgeInsets.only(top: 2, left: Space.m, right: Space.m),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          HoverCard(
            holdToShow: true,
            end: true,
            card: (context) => Text(why),
            child: Text(
              'Not sent',
              style: TypeScale.of(context).caption
                  .copyWith(color: palette.danger),
            ),
          ),
          const SizedBox(width: Space.s),
          action('Retry', onRetry),
          const SizedBox(width: Space.s),
          action('Edit', onEdit),
        ],
      ),
    );
  }
}

/// The Lucide glyph for a kind of device; a generic one when the app that
/// sent it didn't say.
String deviceGlyph(DeviceKind kind) => switch (kind) {
  DeviceKind.DEVICE_KIND_DESKTOP => 'monitor',
  DeviceKind.DEVICE_KIND_LAPTOP => 'laptop-minimal',
  DeviceKind.DEVICE_KIND_PHONE => 'smartphone',
  DeviceKind.DEVICE_KIND_TABLET => 'tablet',
  _ => 'monitor-smartphone',
};

/// What the mark under a message says on hover: the device's name, or
/// its kind when it has none.
String sentFromText(SentFrom from) {
  final name = from.deviceName.trim();
  if (name.isNotEmpty) return 'Sent from $name';
  return switch (from.kind) {
    DeviceKind.DEVICE_KIND_DESKTOP => 'Sent from a desktop computer',
    DeviceKind.DEVICE_KIND_LAPTOP => 'Sent from a laptop',
    DeviceKind.DEVICE_KIND_PHONE => 'Sent from a phone',
    DeviceKind.DEVICE_KIND_TABLET => 'Sent from a tablet',
    _ => 'Sent from another device',
  };
}

/// A faint 12px glyph for the device a message was sent from, naming it on
/// hover, or when touched and held on a phone.
class DeviceMark extends StatelessWidget {
  const DeviceMark(this.from, {super.key});

  final SentFrom from;

  @override
  Widget build(BuildContext context) {
    final text = sentFromText(from);
    return Semantics(
      label: text,
      child: HoverCard(
        holdToShow: true,
        end: true,
        card: (context) => Text(text),
        child: ExcludeSemantics(
          child: ExtensionGlyph(
            icon: deviceGlyph(from.kind),
            name: text,
            color: Palette.of(context).textFaint,
            size: 12,
          ),
        ),
      ),
    );
  }
}

/// The agent's proposal to Rivet, for the person to decide: a card in
/// Rivet's color with the proposal, Dismiss and Approve, and a comment
/// for either (Enter approves). Decided, it says how and when, with the person's comment.
/// The backend sends the entry back decided; the card keeps nothing.
class ProposalCard extends StatefulWidget {
  const ProposalCard(this.proposal, {super.key, this.agentId, this.chatId});

  final Proposal proposal;

  /// The agent that proposed it; the selected agent when null (its
  /// conversation is the one showing).
  final String? agentId;

  /// The chat that proposed it instead; when null, the chat whose
  /// conversation it's in ([ChatScope]), if any.
  final String? chatId;

  @override
  State<ProposalCard> createState() => _ProposalCardState();
}

class _ProposalCardState extends State<ProposalCard> {
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
      final chatId = widget.chatId ?? ChatScope.chatIdOf(context) ?? '';
      await scope.decideProposal(
        chatId.isEmpty
            ? (widget.agentId ?? scope.workspace.workspace.selectedAgentId)
            : '',
        widget.proposal.id,
        approve: approve,
        comment: _comment.text.trim(),
        chatId: chatId,
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
                  style: TypeScale.of(context).caption
                      .copyWith(color: color, fontWeight: FontWeight.w600),
                ),
              ),
            ],
          ),
          const SizedBox(height: Space.s),
          Markdown(proposal.text, color: dismissed ? palette.textMuted : null),
          const SizedBox(height: Space.m),
          if (pending) ...[
            Row(
              children: [
                Expanded(
                  child: Text(
                    _error ?? 'If you approve, $name gets it as your message.',
                    style: TypeScale.of(context).caption.copyWith(
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
            DecisionComment(
              controller: _comment,
              agent: name,
              enabled: !_deciding,
              onSubmitted: () => _decide(approve: true),
            ),
          ] else ...[
            // Who decided: the person.
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                if (true) ...[
                  const PersonAvatar(size: 16),
                  const SizedBox(width: Space.xs + 2),
                ],
                Flexible(
                  child: Text.rich(
                    TextSpan(
                      children: [
                        TextSpan(
                          text: dismissed
                              ? 'Dismissed'
                              : 'Approved, sent to $name',
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
                    style: TypeScale.of(context).caption,
                  ),
                ),
              ],
            ),
            if (proposal.comment.isNotEmpty) ...[
              const SizedBox(height: Space.xs),
              Text(
                proposal.comment,
                style: TypeScale.of(context).body
                    .copyWith(color: palette.textMuted),
              ),
            ],
          ],
        ],
      ),
    );
  }
}

/// Another agent's message: a bubble on the left in that agent's color,
/// under its character and name. Clicking the name (or the character)
/// opens the sender's conversation, when the app knows the sender; the
/// bubble is like any other, for selecting text, links and Show more.
class _AgentMessage extends StatefulWidget {
  const _AgentMessage(this.event, {this.sentMs = 0});

  final Event event;

  /// When it came, at the bubble's foot.
  final int sentMs;

  @override
  State<_AgentMessage> createState() => _AgentMessageState();
}

class _AgentMessageState extends State<_AgentMessage> {
  var _hovered = false;

  String get _sender => widget.event.fromId;

  void _open() {
    final tints = context.getInheritedWidgetOfExactType<AgentTints>();
    tints?.open?.call(_sender);
  }

  void _hover(bool hovered) {
    if (hovered != _hovered) setState(() => _hovered = hovered);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final tints = AgentTints.of(context);
    final (message, also) = splitAlsoSentTo(widget.event.text);
    final color = tints?.byId[_sender];
    final known = color != null && tints?.open != null;
    final nameColor = color ?? palette.textMuted;
    Widget from = Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        // Who it's from, at rest: a reference, not their state now.
        if (tints?.agents[_sender] case final sender?) ...[
          AgentCharacter(agent: sender, size: 22),
          const SizedBox(width: Space.xs),
        ],
        // Its role now, faint after the name, when the app knows the
        // sender.
        Flexible(
          child: NameThenRole(
            name: senderName(widget.event, tints?.agents),
            role: tints?.agents[_sender]?.role ?? '',
            style: TypeScale.of(context).caption.copyWith(
              color: nameColor,
              fontWeight: FontWeight.w600,
              decoration: known && _hovered ? TextDecoration.underline : null,
              decorationColor: nameColor,
            ),
            roleColor: palette.textFaint,
          ),
        ),
      ],
    );
    if (known) {
      // The pointer shows over the name too, which would otherwise show
      // the text cursor: a region laid over it that lets the pointer
      // through.
      from = Semantics(
        button: true,
        label: 'Open ${senderName(widget.event, tints?.agents)}',
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: _open,
          child: Stack(
            children: [
              from,
              Positioned.fill(
                child: MouseRegion(
                  opaque: false,
                  cursor: SystemMouseCursors.click,
                  onEnter: (_) => _hover(true),
                  onExit: (_) => _hover(false),
                ),
              ),
            ],
          ),
        ),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.only(left: Space.m, bottom: 2),
          child: from,
        ),
        Bubble(
          color: color ?? palette.selected,
          outgoing: false,
          sentMs: widget.sentMs,
          timeColor: color == null ? null : const Color(0xB3FFFFFF),
          child: Markdown(
            message,
            color: color == null ? null : const Color(0xFFFFFFFF),
          ),
        ),
        // Who else got it, quietly under the bubble.
        if (also != null)
          Padding(
            padding: const EdgeInsets.only(left: Space.m, top: 2),
            child: Text(
              'Also sent to $also',
              style: TypeScale.of(context).caption
                  .copyWith(color: palette.textFaint),
            ),
          ),
        // Why it was sent and what it replies to, when it says.
        MessageProvenance(event: widget.event),
      ],
    );
  }
}

/// A message in the conversation, its words and who sent it when it's
/// another agent (null for the agent's own and the person's).
typedef _Pointed = ({String text, String? from});

/// Where a message tells the conversation that the pointer is over it, and
/// how to reply to one ([_Repliable]).
class _ReplyScope extends InheritedWidget {
  const _ReplyScope({
    required this.pointed,
    required this.reply,
    required super.child,
  });

  final ValueNotifier<_Pointed?> pointed;
  final void Function(_Pointed message) reply;

  @override
  bool updateShouldNotify(_ReplyScope old) =>
      old.pointed != pointed || old.reply != reply;
}

/// A message that can be replied to: on a Mac it says when the pointer is
/// over it, so the right click's menu offers Reply for it.
class _Repliable extends StatelessWidget {
  const _Repliable({
    required this.text,
    required this.from,
    required this.child,
  });

  final String text;
  final String? from;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final scope = context.getInheritedWidgetOfExactType<_ReplyScope>();
    if (scope == null) return child;
    final me = (text: text, from: from);
    return MouseRegion(
      opaque: false,
      hitTestBehavior: HitTestBehavior.translucent,
      onEnter: (_) => scope.pointed.value = me,
      onHover: (_) => scope.pointed.value = me,
      onExit: (_) {
        if (scope.pointed.value == me) scope.pointed.value = null;
      },
      child: child,
    );
  }
}

/// The agent's own reply in its conversation: a bubble on the left like
/// another agent's message, but washed with the agent's own color rather
/// than filled with it, edged in it, its words in `text`, and with no name
/// over it: it's whose conversation this is. Others' messages, filled with
/// their color under their name, stand apart from it at a glance.
class _OwnReply extends StatelessWidget {
  const _OwnReply(this.text, {this.answers, this.seq = 0, this.sentMs = 0});

  final String text;

  /// When it was written, at the bubble's foot.
  final int sentMs;

  /// The seq of the message this reply answers ([repliedTo]), if any, for
  /// its quote over the bubble.
  final int? answers;

  /// This reply's own seq.
  final int seq;

  @override
  Widget build(BuildContext context) {
    final own = AgentTints.of(context)?.own ?? Palette.of(context).textFaint;
    final bubble = Bubble(
      color: own.withValues(alpha: 0.14),
      edge: own.withValues(alpha: 0.5),
      outgoing: false,
      wide: true,
      sentMs: sentMs,
      child: Markdown(text),
    );
    final answers = this.answers;
    if (answers == null) return bubble;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ReplyQuote(answers: answers, seq: seq),
        bubble,
      ],
    );
  }
}

/// The seq of the person's message [entry] answers (`Entry.replied_to`: the
/// first text of a run's reply carries it), or null. Seq 0 is a message
/// too, the conversation's first, so it's whether it's set, not whether
/// it's 0.
int? repliedTo(Entry entry) =>
    entry.hasRepliedTo() ? entry.repliedTo.toInt() : null;

/// Over an agent's reply, the message it answers, as chat apps quote what's
/// replied to: a bar in its sender's color, who sent it ("You") and its
/// first line, faint. A click or tap goes to that message, loading back to
/// it if it's older than what's held. Where nothing came between the two
/// (the reply right under its question) it's left out: there it's plain
/// which message it answers. It follows the transcript, so a message paged
/// in later, or one that comes between, shows it.
class ReplyQuote extends StatefulWidget {
  const ReplyQuote({super.key, required this.answers, required this.seq});

  /// The seq of the message answered.
  final int answers;

  /// The reply's own seq.
  final int seq;

  @override
  State<ReplyQuote> createState() => _ReplyQuoteState();
}

/// What a [ReplyQuote] shows: the message answered when it's held, and
/// whether anything came between it and the reply.
typedef _Quoted = ({Entry? message, bool apart});

class _ReplyQuoteState extends State<ReplyQuote> {
  TranscriptStore? _store;
  _Quoted _quoted = (message: null, apart: true);
  var _hovered = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final store = ToolViewScope.of(context)?.transcript;
    if (store != _store) {
      _store?.removeListener(_changed);
      _store = store?..addListener(_changed);
      _quoted = _find();
    }
  }

  @override
  void didUpdateWidget(ReplyQuote old) {
    super.didUpdateWidget(old);
    if (old.answers != widget.answers || old.seq != widget.seq) {
      _quoted = _find();
    }
  }

  @override
  void dispose() {
    _store?.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    final quoted = _find();
    if (quoted != _quoted) setState(() => _quoted = quoted);
  }

  /// Where an entry is in [store], by its seq; null when it isn't held.
  static int? _indexOf(TranscriptStore store, int seq) {
    var low = 0;
    var high = store.length - 1;
    while (low <= high) {
      final middle = (low + high) >> 1;
      final at = store[middle].seq;
      if (at == seq) return middle;
      if (at < seq) {
        low = middle + 1;
      } else {
        high = middle - 1;
      }
    }
    return null;
  }

  _Quoted _find() {
    final store = _store;
    if (store == null) return (message: null, apart: true);
    final from = _indexOf(store, widget.answers);
    if (from == null) return (message: null, apart: true);
    final to = _indexOf(store, widget.seq) ?? store.length;
    var apart = false;
    for (var i = from + 1; i < to && !apart; i++) {
      final entry = store[i].entry;
      apart = switch (entry.whichKind()) {
        Entry_Kind.user => true,
        Entry_Kind.assistant => entry.assistant.text.isNotEmpty,
        Entry_Kind.event =>
          entry.event.source == EventSource.EVENT_SOURCE_AGENT,
        _ => false,
      };
    }
    return (message: store[from].entry, apart: apart);
  }

  void _go() {
    final agentId = ToolViewScope.of(context)?.agentId ?? '';
    if (agentId.isEmpty) return;
    AppScope.of(context).reveal.reachSeq(agentId, widget.answers);
  }

  void _hover(bool hovered) {
    if (hovered != _hovered) setState(() => _hovered = hovered);
  }

  @override
  Widget build(BuildContext context) {
    final (:message, :apart) = _quoted;
    if (!apart) return const SizedBox.shrink();
    final palette = Palette.of(context);
    final tints = AgentTints.of(context);
    final type = TypeScale.of(context);
    // Who sent it: the person (in the agent's color, as their bubbles are),
    // or another agent; one not held yet is "Earlier message".
    final (String? name, Color color) = switch (message?.whichKind()) {
      Entry_Kind.user => ('You', tints?.own ?? palette.textMuted),
      Entry_Kind.event => (
        senderName(message!.event, tints?.agents),
        tints?.byId[message.event.fromId] ?? palette.textMuted,
      ),
      _ => (null, palette.textFaint),
    };
    final words = message == null ? '' : _excerpt(message);
    final style = type.caption;
    final line = Text.rich(
      TextSpan(
        children: [
          TextSpan(
            text: name ?? 'Earlier message',
            style: TextStyle(
              color: name == null ? palette.textMuted : color,
              fontWeight: FontWeight.w600,
              decoration: _hovered ? TextDecoration.underline : null,
              decorationColor: name == null ? palette.textMuted : color,
            ),
          ),
          if (words.isNotEmpty)
            TextSpan(
              text: '  $words',
              style: TextStyle(color: palette.textMuted),
            ),
        ],
      ),
      style: style,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
    );
    final quote = Container(
      padding: const EdgeInsets.only(left: Space.s, top: 1, bottom: 1),
      decoration: BoxDecoration(
        border: Border(left: BorderSide(color: color, width: 2)),
      ),
      child: line,
    );
    return Padding(
      // Over the bubble's text, a little apart from it.
      padding: const EdgeInsets.only(left: Space.m, bottom: Space.xs),
      child: Align(
        alignment: Alignment.centerLeft,
        child: FractionallySizedBox(
          widthFactor: isPhoneLayout(context) ? 0.94 : 0.9,
          alignment: Alignment.centerLeft,
          child: Unselectable(
            child: Semantics(
              button: true,
              label:
                  'In reply to ${name ?? 'an earlier message'}'
                  '${words.isEmpty ? '' : ': $words'}. Go to that message',
              excludeSemantics: true,
              child: GestureDetector(
                behavior: HitTestBehavior.opaque,
                onTap: _go,
                child: MouseRegion(
                  cursor: SystemMouseCursors.click,
                  onEnter: (_) => _hover(true),
                  onExit: (_) => _hover(false),
                  child: Align(alignment: Alignment.centerLeft, child: quote),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  /// A message's words on one line: its text, or for one without any, its
  /// files' names or that it's a voice message.
  static String _excerpt(Entry message) {
    final text = switch (message.whichKind()) {
      Entry_Kind.user => message.user.text,
      Entry_Kind.event => splitAlsoSentTo(message.event.text).$1,
      _ => '',
    };
    final words = text.replaceAll(RegExp(r'\s+'), ' ').trim();
    if (words.isNotEmpty || !message.hasUser()) return words;
    final user = message.user;
    if (user.attachments.isNotEmpty) {
      return [for (final file in user.attachments) file.name].join(', ');
    }
    return user.hasVoice() ? 'Voice message' : '';
  }
}

/// Who an agent's message is from, as the app shows it. The backend gives
/// the sender's signature, as the receiving agent read it: "Sam (Backend)",
/// "Ana (Frontend, Web Site)" from another project, "Rivet (admin)". When
/// the app knows the sender ([agents] has its id), its character and role
/// show beside the name, so the name alone, as it was when sent; an unknown
/// one (deleted since) keeps the whole signature, which still says who it
/// was.
String senderName(Event event, Map<String, Agent>? agents) {
  final from = event.from;
  final name = agents?[event.fromId]?.name;
  if (name == null || from == name) return from;
  // Its name now, then the role and project: signed under that name. A
  // name that itself ends in parentheses ("Ana (old)") is signed bare, so
  // the name is matched first rather than any last parentheses taken off.
  if (from.startsWith('$name (')) return name;
  // Renamed since: the signature's last parentheses are its role and
  // project.
  return from.replaceFirst(_signatureTail, '');
}

final _signatureTail = RegExp(r' \([^()]*\)$');

/// A message sent to several agents ends with a paragraph naming the
/// others ("Also sent to: Lena (Backend), Omar (Backend)."): the message
/// without it, and the names. Null names for a message to one agent.
(String, String?) splitAlsoSentTo(String text) {
  final match = RegExp(r'\n\nAlso sent to: ([^\n]+?)\.?\s*$').firstMatch(text);
  if (match == null) return (text, null);
  return (text.substring(0, match.start), match.group(1));
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
                style: TypeScale.of(context).caption.copyWith(color: color),
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
                style: TypeScale.of(context).code
                    .copyWith(color: palette.textMuted),
              )
            : Text(
                event.text,
                style: TypeScale.of(context).body
                    .copyWith(color: palette.textMuted),
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
    this.edge,
    this.wide = false,
    this.sentMs = 0,
    this.timeColor,
  });

  final Color color;
  final bool outgoing;
  final Widget child;

  /// When it was sent, shown small at its foot ([TimeFoot]); 0 for none.
  final int sentMs;

  /// The time's color: faint on a light bubble, its words' color, dimmed,
  /// on a filled one.
  final Color? timeColor;

  /// A 1px line around it, tail and all; none when null.
  final Color? edge;

  /// Room for an agent's own reply, its code and tables: most of the
  /// column rather than a message's share of it.
  final bool wide;

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
              // A phone's narrow column keeps more of its width for words.
              maxWidth:
                  constraints.maxWidth *
                  (wide
                      ? (isPhoneLayout(context) ? 0.94 : 0.9)
                      : (isPhoneLayout(context) ? 0.86 : 0.78)),
              minHeight: 34,
            ),
            child: CustomPaint(
              painter: _BubblePainter(
                color: color,
                outgoing: outgoing,
                edge: edge,
              ),
              child: Padding(
                padding: const EdgeInsets.fromLTRB(
                  Space.m,
                  Space.s - 1,
                  Space.m,
                  Space.s,
                ),
                child: sentMs <= 0
                    ? child
                    : TimeFoot(
                        ms: sentMs,
                        color: timeColor ?? Palette.of(context).textFaint,
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
  const _BubblePainter({
    required this.color,
    required this.outgoing,
    this.edge,
  });

  final Color? edge;

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
    if (edge case final edge?) {
      canvas.drawPath(
        shape,
        Paint()
          ..color = edge
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1,
      );
    }
  }

  @override
  bool shouldRepaint(_BubblePainter old) =>
      old.color != color || old.outgoing != outgoing || old.edge != edge;
}

/// A one-line header that expands to show more below it.
class _Disclosure extends StatefulWidget {
  const _Disclosure({
    required this.expanded,
    required this.onToggle,
    required this.header,
    required this.body,
    this.under,
  });

  final bool expanded;
  final ValueChanged<bool> onToggle;
  final Widget Function(BuildContext context, bool open, bool hovered) header;
  final WidgetBuilder body;

  /// Shown under the header open or not, apart from its click; null for
  /// nothing.
  final Widget? Function(BuildContext context)? under;

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
        Semantics(
          // On a phone, one thing to open or close, saying which it is.
          container: true,
          button: true,
          expanded: _open,
          child: Pressable(
            onTap: _toggle,
            builder: (context, hovered) =>
                widget.header(context, _open, hovered),
          ),
        ),
        if (widget.under?.call(context) case final under?)
          Padding(
            padding: const EdgeInsets.only(left: Space.l + 2, bottom: Space.xs),
            child: under,
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

/// A disclosure's header line: 24pt on a Mac; on a phone a finger's
/// height at least, and taller as the text grows.
class _HeaderLine extends StatelessWidget {
  const _HeaderLine({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) => isPhoneLayout(context)
      ? ConstrainedBox(
          constraints: const BoxConstraints(minHeight: Touch.target),
          child: Align(
            alignment: AlignmentDirectional.centerStart,
            child: child,
          ),
        )
      : SizedBox(height: 24, child: child);
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
      header: (context, open, hovered) => _HeaderLine(
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
              style: TypeScale.of(context).body.copyWith(
                color: hovered || live ? palette.textMuted : palette.textFaint,
              ),
            ),
          ],
        ),
      ),
      body: (context) => Text(
        text,
        style: TypeScale.of(context).body.copyWith(color: palette.textMuted),
      ),
    );
  }
}

/// A run of thoughts in a row as one line: "Thought 5 times", or while
/// the last is under way "Thinking… · 3 so far" with the live dot. A click
/// opens or closes it.
class _ThinkingRunRow extends StatelessWidget {
  const _ThinkingRunRow({
    required this.run,
    required this.open,
    required this.onToggle,
  });

  final ThinkingRun run;
  final bool open;
  final VoidCallback onToggle;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final live = run.live;
    final label = live
        ? 'Thinking… · ${run.count} so far'
        : 'Thought ${run.count} times';
    return Semantics(
      container: true,
      button: true,
      expanded: open,
      // What a tap does, after what it is.
      label: '$label. ${open ? 'Hide' : 'Show'} thoughts.',
      excludeSemantics: true,
      child: Pressable(
        onTap: onToggle,
        builder: (context, hovered) => _HeaderLine(
          child: Row(
            children: [
              SizedBox(
                width: Space.l + 2,
                child: live && !open
                    ? const Align(
                        alignment: Alignment.centerLeft,
                        child: LiveDot(),
                      )
                    : Chevron(color: palette.textFaint, down: open),
              ),
              Text(
                label,
                style: TypeScale.of(context).body.copyWith(
                  color: hovered || live
                      ? palette.textMuted
                      : palette.textFaint,
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Where an entry sits in a run of the agent's thoughts in a row, which
/// shows as one line ("Thought 5 times").
@immutable
class ThinkingRun {
  const ThinkingRun({
    required this.first,
    required this.count,
    required this.live,
    required this.firstSeq,
  });

  /// The run's first thought, which shows the run's line.
  final bool first;

  /// How many thoughts.
  final int count;

  /// The agent is still thinking its last one.
  final bool live;

  /// The first thought's seq: which run it is.
  final int firstSeq;

  @override
  bool operator ==(Object other) =>
      other is ThinkingRun &&
      other.first == first &&
      other.count == count &&
      other.live == live &&
      other.firstSeq == firstSeq;

  @override
  int get hashCode => Object.hash(first, count, live, firstSeq);
}

/// The run of thoughts the entry at [index] is in: two or more thoughts
/// with nothing shown between them (a hidden entry or a reply with no
/// words doesn't count; anything else ends the run, as does [ends], an
/// entry something else shows after). Null for an entry in no such run.
/// [working]: the agent is at work, so a run nothing shown follows is
/// still being thought.
ThinkingRun? thinkingRunAt(
  TranscriptStore transcript,
  int index, {
  bool Function(Entry entry)? hidden,
  bool Function(int index)? ends,
  bool working = false,
}) {
  bool isHidden(int i) => hidden?.call(transcript[i].entry) ?? false;
  bool thought(int i) => transcript[i].entry.hasThinking() && !isHidden(i);
  // Shows nothing: a run goes on past it.
  bool nothing(int i) {
    final entry = transcript[i].entry;
    return isHidden(i) ||
        (entry.hasAssistant() && entry.assistant.text.isEmpty) ||
        entry.whichKind() == Entry_Kind.notSet;
  }

  bool within(int i) => thought(i) || nothing(i);
  bool stops(int i) => ends?.call(i) ?? false;
  if (!within(index)) return null;
  var start = index;
  while (start > 0 && within(start - 1) && !stops(start - 1)) {
    start--;
  }
  var end = index;
  while (end < transcript.length - 1 && !stops(end) && within(end + 1)) {
    end++;
  }
  // From the first thought to the last.
  while (start < end && !thought(start)) {
    start++;
  }
  while (end > start && !thought(end)) {
    end--;
  }
  if (index < start || index > end) return null;
  var count = 0;
  for (var i = start; i <= end; i++) {
    if (thought(i)) count++;
  }
  if (count < 2) return null;
  var after = end + 1;
  while (after < transcript.length && nothing(after)) {
    after++;
  }
  return ThinkingRun(
    first: index == start,
    count: count,
    live: working && after == transcript.length,
    firstSeq: transcript[start].seq,
  );
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
ToolBlock toolBlockAt(
  TranscriptStore transcript,
  int index, {
  bool Function(Entry entry)? hidden,
}) {
  final entry = transcript[index].entry;
  // The next entry shown that way, past hidden ones.
  bool sameReply(int step) {
    var other = index + step;
    while (other >= 0 &&
        other < transcript.length &&
        (hidden?.call(transcript[other].entry) ?? false)) {
      other += step;
    }
    if (other < 0 || other >= transcript.length) return false;
    final neighbour = transcript[other].entry;
    return entry.reply != Int64.ZERO &&
        neighbour.hasToolCall() &&
        neighbour.reply == entry.reply;
  }

  return ToolBlock(first: !sameReply(-1), last: !sameReply(1));
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
    this.seq = 0,
    required this.call,
    required this.expanded,
    required this.onToggle,
  });

  final int seq;
  final ToolCall call;
  final bool expanded;
  final ValueChanged<bool> onToggle;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final info = ToolCallInfo(call, seq: seq);
    final failed = info.failed;
    final running = info.running;
    final view = toolViewFor(info);
    // On a phone how long it ran and whether it failed go under the line,
    // which needs the width; only the chevron stays at its end.
    final phone = isPhoneLayout(context);
    final timed = call.started > 0 && (call.ended > 0 || running);
    Widget? meta() => !phone || (!timed && !failed)
        ? null
        : Wrap(
            spacing: Space.s,
            children: [
              if (timed)
                Ticking(
                  active: call.ended == 0,
                  builder: (context, now) => Text(
                    toolClock(call, now),
                    style: TypeScale.of(context).caption.copyWith(
                      color: palette.textFaint,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    ),
                  ),
                ),
              if (failed)
                Text(
                  'Failed',
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.danger),
                ),
            ],
          );
    // After the line: how long it ran, whether it failed, and the chevron
    // that opens its input and output.
    List<Widget> ending(bool open, bool hovered) => [
      // How long it has run (ticking) and when bash will stop it, or how
      // long it took.
      if (timed && !phone) ...[
        const SizedBox(width: Space.s),
        Ticking(
          active: call.ended == 0,
          builder: (context, now) => Text(
            toolClock(call, now),
            style: TypeScale.of(context).caption.copyWith(
              color: palette.textFaint,
              fontFeatures: const [FontFeature.tabularFigures()],
            ),
          ),
        ),
      ],
      if (failed && !phone) ...[
        const SizedBox(width: Space.s),
        Text(
          'Failed',
          style: TypeScale.of(context).caption.copyWith(color: palette.danger),
        ),
      ],
      const SizedBox(width: Space.s),
      Chevron(
        color: hovered ? palette.textMuted : palette.textFaint,
        down: open,
      ),
    ];
    if (view != null) {
      return _Disclosure(
        expanded: expanded,
        onToggle: onToggle,
        header: (context, open, hovered) => _HeaderLine(
          child: Row(
            children: [
              SizedBox(
                width: Space.l + 2,
                child: Align(
                  alignment: Alignment.centerLeft,
                  child: running
                      ? const LiveDot()
                      : ExtensionGlyph(
                          icon: view.icon,
                          name: call.name,
                          size: 14,
                          color: failed
                              ? palette.danger
                              : hovered
                              ? palette.textMuted
                              : palette.textFaint,
                        ),
                ),
              ),
              Expanded(
                child: switch (meta()) {
                  final meta? => Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [view.line(context, info), meta],
                  ),
                  null => view.line(context, info),
                },
              ),
              ...ending(open, hovered),
            ],
          ),
        ),
        // What it did, always in sight: the message, the items, the results.
        under: (context) {
          final body = view.body?.call(context, info);
          final error = failed && call.output.trim().isNotEmpty;
          if (body == null && !error) return null;
          return Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              ?body,
              if (error)
                Text(
                  call.output.trim(),
                  maxLines: 3,
                  overflow: TextOverflow.ellipsis,
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.danger),
                ),
            ],
          );
        },
        body: (context) => _ToolCallBody(call: call, running: running),
      );
    }
    final summary = toolSummary(call.name, call.input);
    if (phone) {
      // Its name over what it does, and how long it ran under that: on one
      // line they'd leave each other no room.
      return _Disclosure(
        expanded: expanded,
        onToggle: onToggle,
        header: (context, open, hovered) => _HeaderLine(
          child: Padding(
            padding: const EdgeInsets.symmetric(vertical: Space.xs),
            child: Row(
              children: [
                SizedBox(
                  width: Space.l + 2,
                  child: Align(
                    alignment: Alignment.centerLeft,
                    child: running
                        ? const LiveDot()
                        : Dot(
                            color: failed ? palette.danger : palette.textFaint,
                          ),
                  ),
                ),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        call.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TypeScale.of(context).label.copyWith(
                          color: failed ? palette.danger : palette.text,
                        ),
                      ),
                      if (summary.text.isNotEmpty)
                        Text(
                          [
                            summary.text,
                            if (summary.more.isNotEmpty) summary.more,
                          ].join(' '),
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style:
                              (summary.code
                                      ? TypeScale.of(context).code
                                      : TypeScale.of(context).secondary)
                                  .copyWith(color: palette.textMuted),
                        ),
                      ?meta(),
                    ],
                  ),
                ),
                ...ending(open, hovered),
              ],
            ),
          ),
        ),
        body: (context) => _ToolCallBody(call: call, running: running),
      );
    }
    return _Disclosure(
      expanded: expanded,
      onToggle: onToggle,
      header: (context, open, hovered) => _HeaderLine(
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
              style: TypeScale.of(context).label
                  .copyWith(color: failed ? palette.danger : palette.text),
            ),
            const SizedBox(width: Space.s),
            // What it does: the command, the path, whom it messages.
            Expanded(
              child: Text(
                summary.text,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style:
                    (summary.code
                            ? TypeScale.of(context).code
                            : TypeScale.of(context).body)
                        .copyWith(
                          color: hovered ? palette.text : palette.textMuted,
                        ),
              ),
            ),
            if (summary.more.isNotEmpty) ...[
              const SizedBox(width: Space.s),
              Text(
                summary.more,
                style: TypeScale.of(context).caption
                    .copyWith(color: palette.textFaint),
              ),
            ],
            ...ending(open, hovered),
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
        style: TypeScale.of(context).caption.copyWith(color: palette.textFaint),
      ),
    );
    final Widget output;
    if (call.output.isEmpty) {
      output = Text(
        running ? 'Running…' : 'No output',
        style: TypeScale.of(context).caption.copyWith(color: palette.textFaint),
      );
    } else if (call.name == 'edit_file' && !failed) {
      output = DiffView(call.output);
    } else {
      output = CodeBlock(
        call.output,
        color: failed ? palette.danger : null,
        // A CLI's sign-in page, say, opened through the tunnel's forwarding.
        links: call.name == 'bash',
      );
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
      child: NoDrawerGesture(
        child: SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          padding: const EdgeInsets.symmetric(
            horizontal: Space.m,
            vertical: Space.s + 2,
          ),
          child: Text.rich(
            TextSpan(style: TypeScale.of(context).code, children: spans),
            softWrap: false,
          ),
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
        return _HeaderLine(
          child: Row(
            children: [
              Expanded(child: Container(height: 1, color: palette.hairline)),
              const SizedBox(width: Space.m),
              Text(
                compaction.idle
                    ? 'Earlier messages summarized while idle'
                    : 'Earlier messages summarized',
                style: TypeScale.of(context).caption.copyWith(color: color),
              ),
              const SizedBox(width: Space.xs + 2),
              Chevron(color: color, down: open),
              const SizedBox(width: Space.m),
              Expanded(child: Container(height: 1, color: palette.hairline)),
            ],
          ),
        );
      },
      body: (context) => Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // How big the conversation was, for whoever opens it to see.
          if (compaction.tokensBefore > 0)
            Padding(
              padding: const EdgeInsets.only(bottom: Space.s),
              child: Text(
                'Summarized at ${formatTokens(compaction.tokensBefore)} tokens.',
                style: TypeScale.of(context).caption
                    .copyWith(color: palette.textFaint),
              ),
            ),
          Markdown(compaction.summary, color: palette.textMuted),
        ],
      ),
    );
  }
}

class _NoticeView extends StatelessWidget {
  const _NoticeView(this.notice);

  final Notice notice;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    // A step the model declined on safety grounds: its own card, not a
    // failure.
    if (notice.refusalCategory.isNotEmpty) {
      final store = AppScope.of(context).workspace;
      return RefusalCard(
        notice,
        agent: ChatScope.chatIdOf(context) == null
            ? store.agent(store.workspace.selectedAgentId)
            : null,
      );
    }
    final error = notice.level == NoticeLevel.NOTICE_LEVEL_ERROR;
    if (error) {
      final store = AppScope.of(context).workspace;
      // In a chat, the chat's error, not the selected agent's.
      final chatId = ChatScope.chatIdOf(context);
      if (chatId != null) {
        final chat = store.workspace.chats
            .where((chat) => chat.id == chatId)
            .firstOrNull;
        final title = chat == null ? 'New chat' : chatTitle(chat);
        if (notice.hasRefused()) {
          return RefusedLine(notice.refused, agent: 'this chat');
        }
        return ErrorLine(
          notice.text,
          about: 'In the chat "$title" (chat $chatId):',
        );
      }
      final id = store.workspace.selectedAgentId;
      final name = store.agent(id)?.name ?? id;
      if (notice.hasRefused()) return RefusedLine(notice.refused, agent: name);
      return ErrorLine(
        notice.text,
        about: "In $name's conversation (agent $id):",
      );
    }
    return Text(
      notice.text,
      style: TypeScale.of(context).caption.copyWith(color: palette.textMuted),
    );
  }
}

/// A run that ended because the provider refused the agent's key or
/// sign-in: says so plainly, with the way to fix it (a new key, signing in
/// again) rather than the provider's raw answer, and nothing to ask Rivet,
/// whose model may be the one refused.
class RefusedLine extends StatefulWidget {
  const RefusedLine(this.refused, {super.key, required this.agent});

  final CredentialsRefused refused;

  /// The agent's name.
  final String agent;

  @override
  State<RefusedLine> createState() => _RefusedLineState();
}

class _RefusedLineState extends State<RefusedLine> {
  String? _failure;

  /// Starts [provider]'s sign-in with [method], in place of the refused
  /// account when it's the first (a key from the environment can't be
  /// replaced: a stored one goes before it), and opens Settings →
  /// Providers, where it shows.
  Future<void> _fix(Provider? provider, String method) async {
    final scope = AppScope.scope(context);
    setState(() => _failure = null);
    AppScope.openProviders(context);
    if (provider == null || !provider.methods.any((m) => m.id == method)) {
      return;
    }
    final account = widget.refused.accountId;
    final env = account == 'env';
    final first =
        provider.accounts.isEmpty || provider.accounts.first.id == account;
    // Another account than the first: Settings, to choose what to do.
    if (!env && !first) return;
    try {
      await scope.login.start(
        provider.id,
        method,
        addAccount: env && provider.accounts.isNotEmpty,
      );
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _failure = failure.message);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final refused = widget.refused;
    final workspace = AppScope.of(context).workspace.workspace;
    final provider = workspace.providers
        .where((p) => p.id == refused.providerId)
        .firstOrNull;
    final name = provider?.name ?? refused.providerId;
    final env = refused.accountId == 'env';
    final variable = env
        ? provider?.accounts
                  .where((a) => a.id == 'env')
                  .map((a) => a.label)
                  .firstOrNull ??
              ''
        : '';
    final (
      String text,
      String action,
      String method,
    ) = switch (refused.methodId) {
      'api_key' when env => (
        variable.isEmpty
            ? '$name refused the key from the environment.'
            : '$name refused the key in $variable.',
        'Add a key…',
        'api_key',
      ),
      'api_key' => (
        "$name refused ${widget.agent}'s key.",
        'Change key…',
        'api_key',
      ),
      'oauth' => (
        "$name refused ${widget.agent}'s sign-in.",
        'Sign in again…',
        'oauth',
      ),
      _ => ("$name refused ${widget.agent}'s sign-in.", 'Open Providers…', ''),
    };
    final caption = TypeScale.of(context).caption;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Wrap(
          spacing: Space.s,
          crossAxisAlignment: WrapCrossAlignment.end,
          children: [
            Text(text, style: caption.copyWith(color: palette.danger)),
            Semantics(
              button: true,
              label: action,
              excludeSemantics: true,
              child: Pressable(
                onTap: () => _fix(provider, method),
                builder: (context, hovered) => Text(
                  action,
                  style: caption.copyWith(
                    color: hovered ? palette.text : palette.textMuted,
                    fontWeight: FontWeight.w500,
                    decoration: hovered ? TextDecoration.underline : null,
                    decorationColor: palette.text,
                  ),
                ),
              ),
            ),
          ],
        ),
        if (_failure case final failure?)
          Padding(
            padding: const EdgeInsets.only(top: Space.xs),
            child: Text(
              failure,
              style: caption.copyWith(color: palette.danger),
            ),
          ),
      ],
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
    return partialJsonString(input, key);
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
      return lines(partialJsonString(input, 'items'), code: false);
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
  return lines(partialJsonString(input, null));
}

/// 412000 → "412k".
String formatTokens(Int64 tokens) {
  final n = tokens.toInt();
  if (n < 1000) return '$n';
  if (n < 1000000) return '${(n / 1000).round()}k';
  return '${(n / 1000000).toStringAsFixed(1)}M';
}
