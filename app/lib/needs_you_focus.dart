// Needs you's focus mode (#514, Formaggio's spec): the whole window becomes
// what waits for the person, one card at a time, answered with the fewest
// clicks and keys, on to the next by itself once Claktory has accepted the
// answer.
//
// FocusSession holds the sitting's rules, apart from any layout, so the
// Mac's three columns (here), the iPad's tiers (ipad/focus.dart) and the
// iPhone's one card per screen drive the same one. The app keeps nothing
// of it: what waits is the backend's, and whether this device is in focus
// mode is the backend's too (Settings.focus_mode, §9.1).
import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart'
    show ValueListenable, TargetPlatform, defaultTargetPlatform;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'characters/character.dart';
import 'chats.dart' show ChatMessageTarget, ChatScope;
import 'claktory_identity.dart' show claktoryName;
import 'controls.dart';
import 'dictation.dart' show Dictation, DictationFor, DictationHold;
import 'identity.dart';
import 'inbox.dart';
import 'name_role_line.dart';
import 'layout.dart' show viewerKindFor;
import 'message_box.dart' show AgentMessageTarget, MessageBox, MessageTarget;
import 'card_answer.dart';
import 'needs_you_entries.dart';
import 'mobile/metrics.dart';
import 'pipes.dart' show PipeMessageView;
import 'protocol/factory/v1/factory.pb.dart';
import 'review_answer.dart' show ReviewAnswering, rememberReviewTalk;
import 'rail.dart' show ClaktoriesScope;
import 'scope.dart';
import 'state.dart' show PipeChat;
import 'transcript.dart' show TimelineItem, TranscriptView;
import 'usage.dart' show formatAge;
import 'viewers/registry.dart' show viewerBody;
import 'usage_wait.dart' show atWork;
import 'voice_session.dart' show activeVoice;
import 'waiting.dart' show NeedsYouMark;

/// The no's quick reasons, in this order everywhere (Trish's words, §5.4):
/// the Mac's numbered chips, a phone's action sheet.
const focusDismissReasons = [
  'Do it yourself',
  'Not now',
  'Too long, say it shorter',
  'Not at a computer',
  'Unclear',
];

/// The no without a reason, after the reasons.
const focusDismissWithoutReason = 'Dismiss without a reason';

/// How long a card changed by its agent takes no yes or no (§8).
const focusChangeGuard = Duration(seconds: 1);

/// How long the first ⌘↩ on a card that runs something waits for the
/// second (§5.6, Execute).
const focusPressAgainWithin = Duration(seconds: 3);

/// How long the no's reasons wait for a choice before the no is taken
/// back (§5.1).
const focusReasonsFor = Duration(seconds: 3);

/// How long the receipt of an accepted answer shows (§5.3).
const focusReceiptFor = Duration(seconds: 2);

/// What stops agents now, first in the queue with a flag (§4.1): a device
/// waiting to be allowed, the secrets locked while agents wait, a usage
/// limit or a sign-in stopping them, a pipe its loop breaker paused.
bool focusUrgent(NeedsYouItem item) => switch (item.kind) {
  NeedsYouKind.device ||
  NeedsYouKind.vault ||
  NeedsYouKind.providerWait ||
  NeedsYouKind.signIn ||
  NeedsYouKind.pipeLoop => true,
  _ => false,
};

/// Where an answered card stands in the sitting.
enum FocusOutcomeState {
  /// Sent from here, not yet accepted.
  sending,

  /// Accepted: how it ended, in [FocusOutcome.words].
  ended,
}

/// How an item of the sitting ended, or that its answer is on its way.
@immutable
class FocusOutcome {
  const FocusOutcome(this.state, this.words, {this.here = false});

  final FocusOutcomeState state;

  /// "Sending…", "Approved", "Dismissed", "Withdrawn by June",
  /// "Answered on Matt's iPhone".
  final String words;

  /// Answered on this device: the sitting moves on from it.
  final bool here;
}

/// One sitting in focus mode: the queue's order, the card showing, how
/// each answered card ended, and the guards that keep a key or a tap from
/// answering what the person hasn't seen. Layout-free: every device lays
/// it out its own way.
class FocusSession extends ChangeNotifier {
  FocusSession(
    this.backend, {
    NeedsYouEntries? entries,
    DateTime Function()? now,
  }) : entries = entries ?? NeedsYouEntries.of(backend),
       _now = now ?? DateTime.now {
    _startedAt = _now();
    HardwareKeyboard.instance.addHandler(_keyEvent);
    backend.needsYouCard.addListener(_sync);
    backend.workspace.addListener(_sync);
    this.entries.addListener(_sync);
    _sync();
  }

  final Backend backend;
  final NeedsYouEntries entries;
  final DateTime Function() _now;

  late final DateTime _startedAt;

  /// The sitting's items in their order, waiting or answered meanwhile.
  final _order = <String>[];
  final _items = <String, NeedsYouItem>{};
  var _waiting = <String>{};

  /// Until the person first moves or answers, the order follows what's
  /// fetched (times come in as the cards do); from then it holds still.
  var _settled = false;

  /// The card showing was asked for ([Backend.needsYouCard]): it stays
  /// while the order still settles around it.
  var _pinned = false;

  final _outcomes = <String, FocusOutcome>{};

  /// When the person last answered each card here (its yes, its no, a
  /// reply): a card that stops waiting soon after was answered here.
  final _acted = <String, DateTime>{};

  /// How a card read the last time it was seen, to tell a change by its
  /// agent (§8).
  final _shapes = <String, String>{};

  String? _current;
  DateTime? _changedAt;
  Timer? _guardTimer;
  Timer? _receiptTimer;
  var _answered = 0;
  var _disposed = false;

  /// The receipt of the last accepted answer ("Approved · June"), for
  /// [focusReceiptFor].
  String? get receipt => _receipt;
  String? _receipt;

  /// Context, beside the stage or over it.
  bool get contextOpen => _contextOpen;
  var _contextOpen = true;
  set contextOpen(bool value) {
    if (value == _contextOpen) return;
    _contextOpen = value;
    notifyListeners();
  }

  /// The no's reasons, showing (§5.4): for [focusReasonsFor], then the
  /// no is taken back.
  bool get reasonsShowing => _reasonsShowing;
  var _reasonsShowing = false;
  set reasonsShowing(bool value) {
    _reasonsTimer?.cancel();
    if (value) {
      _reasonsTimer = Timer(focusReasonsFor, () {
        if (!_disposed) reasonsShowing = false;
      });
    }
    if (value == _reasonsShowing) return;
    _reasonsShowing = value;
    notifyListeners();
  }

  Timer? _reasonsTimer;

  /// The card on the stage's answer, as it publishes it under the stage's
  /// [AnswerDock]: what ⌘↩, ⌘⌫ and the answer bar do.
  final answer = ValueNotifier<CardAnswer?>(null);

  /// What holding ⌘↩ or ⌘⌫ records into: the stage's answer field, while
  /// a card that takes words is showing.
  DictationHold? say;

  /// R: into the answer field.
  VoidCallback? intoField;

  /// Keys still held from before the card came up: ⌘↩ and ⌘⌫ take nothing
  /// until every key has been let go, so a held key never answers two
  /// cards (§5.1).
  var _armed = true;

  bool _keyEvent(KeyEvent event) {
    if (event is KeyUpEvent &&
        HardwareKeyboard.instance.logicalKeysPressed.isEmpty) {
      _armed = true;
    }
    return false;
  }

  /// ⌘↩ and ⌘⌫ (and the iPad's menu) would take something now: a card that
  /// waits, unchanged for a moment, with no key held over from before.
  bool get answerable {
    final key = _current;
    return key != null && _waiting.contains(key) && _armed && !guarded;
  }

  /// ⌘↩: the card's main action, or its Submit with words typed.
  ///
  /// [byKey]: from ⌘↩ (or a menu), not a click. Running a command on this
  /// computer is never one key away: on a card that asks for it
  /// ([CardAnswer.pressTwice], Execute) the first ⌘↩ only arms the second
  /// for [focusPressAgainWithin] ([pressAgain]); a click runs it at once.
  Future<void> yes({bool byKey = false}) async {
    final key = _current;
    final answer = this.answer.value;
    if (key == null || answer == null || !answerable) return;
    final run = answer.typed ? answer.onSubmit : answer.onYes;
    if (run == null) return;
    if (byKey && answer.pressTwice && !pressAgain) {
      _pressAgainTimer?.cancel();
      _pressAgainTimer = Timer(focusPressAgainWithin, () {
        _pressAgainTimer = null;
        if (!_disposed) notifyListeners();
      });
      notifyListeners();
      return;
    }
    _pressAgainTimer?.cancel();
    _pressAgainTimer = null;
    _saidNo.remove(key);
    await _send(key, run);
  }

  /// The first ⌘↩ on a card that runs something armed the second: "Press
  /// ⌘↩ again to run".
  bool get pressAgain => _pressAgainTimer != null;
  Timer? _pressAgainTimer;

  /// ⌘⌫: the card's no. With nothing typed on a card that takes words, the
  /// quick reasons first; then a reason, or ⌘⌫ again for none (§5.4).
  /// [askWhy] false sends it at once, with [reason] or none ("Dismiss
  /// without a reason" from a phone's sheet).
  Future<void> no({String? reason, bool askWhy = true}) async {
    final key = _current;
    final answer = this.answer.value;
    if (key == null || answer == null || !answerable) return;
    final run = answer.onNo;
    if (run == null) return;
    if (askWhy &&
        reason == null &&
        answer.docked &&
        !answer.typed &&
        !_reasonsShowing) {
      reasonsShowing = true;
      return;
    }
    reasonsShowing = false;
    _saidNo.add(key);
    await _send(key, () => run(reason));
  }

  /// The cards whose no went from here: their receipt says Dismissed, since
  /// the card held reads as it was before the answer (Zucchero row 650).
  final _saidNo = <String>{};

  /// [run], marking [key] as sent from here: it moves on once the card
  /// stops waiting; refused, it stays.
  Future<void> _send(String key, Future<void> Function() run) async {
    sending(key);
    try {
      await run();
    } on AnswerNotSent {
      // Nothing went (a confirmation cancelled, a first press arming the
      // second): the card stays, with no error, uncounted.
      notSent(key);
      return;
    } catch (error) {
      notSent(key);
      if (error is RequestFailure) return;
      rethrow;
    }
    // Accepted and still waiting (a step answered, its next one in place):
    // no longer on its way.
    if (!_disposed &&
        _waiting.contains(key) &&
        _outcomes[key]?.state == FocusOutcomeState.sending) {
      _outcomes.remove(key);
      notifyListeners();
    }
  }

  /// Every item of the sitting, in its order: waiting, then answered here
  /// meanwhile where they were.
  List<NeedsYouItem> get items => [for (final key in _order) _items[key]!];

  /// The items still waiting, in order.
  List<NeedsYouItem> get waiting => [
    for (final key in _order)
      if (_waiting.contains(key)) _items[key]!,
  ];

  NeedsYouItem? itemOf(String key) => _items[key];

  bool isWaiting(String key) => _waiting.contains(key);

  /// How [key] ended, or that its answer is on its way; null while it
  /// waits untouched.
  FocusOutcome? outcomeOf(String key) {
    final outcome = _outcomes[key];
    final item = _items[key];
    if (outcome == null ||
        item == null ||
        outcome.state != FocusOutcomeState.ended) {
      return outcome;
    }
    // Which device answered it, once the card says (§9.2): its words
    // follow, read now rather than when it stopped waiting.
    final entry = entries.entryOf(item);
    final on = entry != null && entry.hasAnsweredOn() ? entry.answeredOn : null;
    if (on == null) return outcome;
    final here = on.deviceId == backend.focusDeviceId;
    return FocusOutcome(
      FocusOutcomeState.ended,
      focusEnded(
        entry,
        here: here,
        who: focusWho(backend.workspace.workspace, item),
        device: on.deviceName,
      ),
      here: outcome.here,
    );
  }

  /// The card on the stage; null with nothing left.
  NeedsYouItem? get current => _current == null ? null : _items[_current];

  /// Its place among those waiting, from 1, and how many wait: "3 of 7".
  (int, int) get position {
    final waiting = this.waiting;
    final at = waiting.indexWhere((item) => item.key == _current);
    return (at + 1, waiting.length);
  }

  /// How many the person answered this sitting, and since when: "You
  /// answered 14 in 23 min." Counted here and never kept.
  int get answered => _answered;
  DateTime get startedAt => _startedAt;

  /// The current card changed in the last [focusChangeGuard]: its yes and
  /// no take nothing (§8), and the yes reads "Updated".
  /// Ended (focus mode left): nothing may be set on it any more.
  bool get disposed => _disposed;

  bool get guarded {
    final at = _changedAt;
    return at != null &&
        _guardTimer != null &&
        _now().difference(at) < focusChangeGuard;
  }

  /// Makes [key] the card on the stage.
  void select(String key) {
    if (!_items.containsKey(key) || key == _current) return;
    _settled = true;
    _setCurrent(key);
    notifyListeners();
  }

  /// The next card still waiting, after the current one; past the last,
  /// the first. Answered cards are skipped.
  void next() => _step(1);

  /// The waiting card before the current one.
  void previous() => _step(-1);

  void _step(int by) {
    final waiting = this.waiting;
    if (waiting.isEmpty) return;
    _settled = true;
    final keys = [for (final item in waiting) item.key];
    var at = keys.indexOf(_current ?? '');
    if (at < 0) {
      // On an answered card: the next waiting one after its place.
      final place = _order.indexOf(_current ?? '');
      final after = [
        for (final key in _order.skip(place + 1))
          if (_waiting.contains(key)) key,
      ];
      _setCurrent(by > 0 ? (after.firstOrNull ?? keys.first) : keys.last);
    } else {
      _setCurrent(keys[(at + by) % keys.length]);
    }
    notifyListeners();
  }

  /// The person answered [key] here (its yes, its no, a reply): if it
  /// stops waiting, it was this device's answer, and the sitting moves on.
  /// Until then its row says "Sending…".
  void sending(String key) {
    if (!_items.containsKey(key)) return;
    _settled = true;
    _acted[key] = _now();
    if (_waiting.contains(key)) {
      _outcomes[key] = const FocusOutcome(
        FocusOutcomeState.sending,
        'Sending…',
        here: true,
      );
    }
    notifyListeners();
  }

  /// The person pressed something in [key]'s card here (one of its own
  /// buttons): if it stops waiting soon after, it was their answer here.
  void touched(String key) {
    if (_items.containsKey(key)) _acted[key] = _now();
  }

  /// The answer to [key] didn't go (refused, or the connection dropped):
  /// the card stays, with the card's own error, and nothing moves on.
  void notSent(String key) {
    _acted.remove(key);
    _saidNo.remove(key);
    if (_outcomes[key]?.state == FocusOutcomeState.sending) {
      _outcomes.remove(key);
      notifyListeners();
    }
  }

  void _setCurrent(String? key) {
    if (key == _current) return;
    _current = key;
    _pressAgainTimer?.cancel();
    _pressAgainTimer = null;
    // The card before's answer goes with it (one that publishes nothing
    // keeps none of the last's).
    if (answer.value case final answer? when answer.key != key) {
      this.answer.value = null;
    }
    _changedAt = null;
    _reasonsShowing = false;
    _reasonsTimer?.cancel();
    _armed = HardwareKeyboard.instance.logicalKeysPressed.isEmpty;
    if (key != null) _shapes[key] = _shape(key);
  }

  /// What waits changed, or a card came in: new ones join the order, ones
  /// gone end, the card showing stays put.
  void _sync() {
    if (_disposed) return;
    final workspace = backend.workspace.workspace;
    final now = needsYouItems(workspace);
    final keys = {for (final item in now) item.key};
    for (final item in now) {
      if (_items.containsKey(item.key)) {
        _items[item.key] = item;
        continue;
      }
      _items[item.key] = item;
      entries.hold(item);
      _place(item.key, focusUrgent(item));
    }
    if (!_settled) _sort(workspace);

    // Gone from what waits: answered (here or elsewhere), or taken back.
    for (final key in _waiting.difference(keys)) {
      final item = _items[key]!;
      final acted = _acted[key];
      final here =
          acted != null &&
          _now().difference(acted) < const Duration(minutes: 2);
      final ended = focusEnded(
        entries.entryOf(item),
        here: here,
        who: focusWho(workspace, item),
      );
      final saidNo = _saidNo.remove(key);
      _outcomes[key] = FocusOutcome(
        FocusOutcomeState.ended,
        // Taken back still says so; a no from here, Dismissed.
        here && saidNo && !ended.startsWith('Withdrawn') ? 'Dismissed' : ended,
        here: here,
      );
      if (here) {
        _answered++;
        _showReceipt(key);
      }
      // A card with something to show once answered (a pipe invite's
      // code) stays until the person moves on.
      final keep = answer.value?.keep ?? false;
      if (key == _current && here && !keep) {
        // On to the next waiting card, after its place.
        _waiting = keys;
        final place = _order.indexOf(key);
        final after = [
          for (final next in _order.skip(place + 1))
            if (keys.contains(next)) next,
          for (final next in _order.take(place))
            if (keys.contains(next)) next,
        ];
        _setCurrent(after.firstOrNull);
      }
    }
    // Waiting again (its agent replied on it, #370): its row waits where it
    // is, with what changed marked by the card.
    for (final key in keys.difference(_waiting)) {
      if (_outcomes.containsKey(key) &&
          _outcomes[key]!.state == FocusOutcomeState.ended) {
        _outcomes.remove(key);
        _acted.remove(key);
      }
    }
    _waiting = keys;
    // A card dropped from the order once it's no longer anything's (its
    // agent deleted): nothing to show of it.
    _order.removeWhere((key) {
      final gone = !keys.contains(key) && !_outcomes.containsKey(key);
      if (gone) {
        _items.remove(key);
        entries.release(key);
      }
      return gone;
    });

    // Nothing showing yet, nothing left before, or the order still
    // settling as the cards come in: the first that waits.
    if (_current == null ||
        !_items.containsKey(_current) ||
        (!_settled && !_pinned && _waiting.contains(_current))) {
      _setCurrent(waiting.firstOrNull?.key);
    }

    // A card asked for (a notification tapped): there once it's come.
    final asked = backend.needsYouCard.value;
    if (asked != null && _items.containsKey(asked)) {
      // Shown, without settling the order: the person hasn't moved, and
      // the rest still sorts as their times come in.
      _pinned = true;
      _setCurrent(asked);
      backend.needsYouCard.value = null;
    }

    // The card showing changed by its agent: it takes no yes or no for a
    // moment (§8).
    final current = _current;
    if (current != null) {
      final shape = _shape(current);
      final before = _shapes[current];
      // A card only now fetched ('' before) didn't change.
      if (before != null &&
          before.isNotEmpty &&
          before != shape &&
          _waiting.contains(current)) {
        _changedAt = _now();
        _guardTimer?.cancel();
        _guardTimer = Timer(focusChangeGuard, () {
          _guardTimer = null;
          if (!_disposed) notifyListeners();
        });
      }
      _shapes[current] = shape;
    }
    notifyListeners();
  }

  /// A new item's place: an urgent one after the last urgent, the rest at
  /// the end (§8: the order holds still for the sitting).
  void _place(String key, bool urgent) {
    if (!urgent) {
      _order.add(key);
      return;
    }
    var at = 0;
    while (at < _order.length &&
        focusUrgent(_items[_order[at]] ?? const NeedsYouItem.vault())) {
      at++;
    }
    _order.insert(at, key);
  }

  /// Urgent first, then the oldest; those without a time yet after the
  /// rest, by their key, never first.
  void _sort(Workspace workspace) {
    int rank(String key) => focusUrgent(_items[key]!) ? 0 : 1;
    _order.sort((a, b) {
      final ra = rank(a), rb = rank(b);
      if (ra != rb) return ra.compareTo(rb);
      final ta = entries.timeOf(_items[a]!, workspace);
      final tb = entries.timeOf(_items[b]!, workspace);
      if ((ta == 0) != (tb == 0)) return ta == 0 ? 1 : -1;
      if (ta != tb) return ta.compareTo(tb);
      return a.compareTo(b);
    });
  }

  /// How the card at [key] reads, to tell when its agent changed it.
  String _shape(String key) {
    final item = _items[key];
    if (item == null) return '';
    final entry = entries.entryOf(item);
    if (entry == null) return '';
    if (entry.hasAskPerson()) {
      final ask = entry.askPerson;
      return '${ask.revision}/${ask.thread.length}/${ask.steps.length}/'
          '${ask.state.value}';
    }
    if (entry.hasProposal()) return '${entry.proposal.revision}';
    return '';
  }

  void _showReceipt(String key) {
    final item = _items[key];
    final words = _outcomes[key]?.words ?? '';
    final who = item == null ? '' : focusWho(backend.workspace.workspace, item);
    _receipt = who.isEmpty ? words : '$words · $who';
    _receiptTimer?.cancel();
    _receiptTimer = Timer(focusReceiptFor, () {
      if (_disposed) return;
      _receipt = null;
      notifyListeners();
    });
  }

  @override
  void dispose() {
    _disposed = true;
    HardwareKeyboard.instance.removeHandler(_keyEvent);
    backend.needsYouCard.removeListener(_sync);
    _reasonsTimer?.cancel();
    _pressAgainTimer?.cancel();
    answer.dispose();
    _guardTimer?.cancel();
    _receiptTimer?.cancel();
    backend.workspace.removeListener(_sync);
    entries.removeListener(_sync);
    for (final key in _order) {
      entries.release(key);
    }
    super.dispose();
  }
}

/// Who asks: the agent's name ("June"), the chat's, the pipe's, or what a
/// system item is about.
String focusWho(Workspace workspace, NeedsYouItem item) {
  for (final agent in workspace.agents) {
    if (agent.id == item.agentId) return agent.name;
  }
  for (final chat in workspace.chats) {
    if (chat.id == item.chatId) return chat.title;
  }
  for (final pipe in workspace.pipes) {
    if (pipe.id == item.pipeId) return pipe.name;
  }
  return '';
}

/// How a card that stopped waiting ended, in a word or two (§4.1):
/// "Approved", "Answered", "Dismissed", "Withdrawn by June", "Answered on
/// another device"; [here] when it was this device's answer, [who] whose
/// card it was.
String focusEnded(
  Entry? entry, {
  required bool here,
  String who = '',
  String device = '',
}) {
  final ask = entry?.hasAskPerson() == true ? entry!.askPerson : null;
  final proposal = entry?.hasProposal() == true ? entry!.proposal : null;
  if (ask?.withdrawn == true || proposal?.withdrawn == true) {
    return who.isEmpty ? 'Withdrawn' : 'Withdrawn by $who';
  }
  if (!here) {
    return device.isEmpty
        ? 'Answered on another device'
        : 'Answered on $device';
  }
  if (ask != null) {
    if (ask.resolved) return 'Resolved';
    if (ask.state == AskPersonState.ASK_PERSON_STATE_DISMISSED) {
      return 'Dismissed';
    }
    if (ask.wantsText) return 'Answered';
    return 'Approved';
  }
  if (proposal != null) {
    return proposal.state == ProposalState.PROPOSAL_STATE_DISMISSED
        ? 'Dismissed'
        : 'Approved';
  }
  return 'Answered';
}

/// Keys the shell binds for focus mode, the app over: ⌘⇧N turns it on or
/// off.
const focusModeKey = SingleActivator(
  LogicalKeyboardKey.keyN,
  meta: true,
  shift: true,
);

/// Below this width Context folds away, and opens over the stage (§4.3).
const focusContextFoldsBelow = 1180.0;

/// The stage's column, at most this wide (§4.2).
const focusStageMaxWidth = 720.0;

/// Context, at least this wide (§4.3).
const focusContextMinWidth = 360.0;

/// The rows of the queue (§4.1).
const focusRowHeight = 52.0;

/// Focus mode on a Mac (§4): the queue at the left, the card on the stage,
/// its conversation in Context at the right. The rail and the sidebar are
/// gone; the window keeps its size.
class NeedsYouFocus extends StatelessWidget {
  const NeedsYouFocus({super.key, required this.session, required this.leave});

  final FocusSession session;

  /// Leaves focus mode, back to exactly where the person was.
  final VoidCallback leave;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ColoredBox(
      color: palette.canvas,
      child: FocusKeys(
        session: session,
        leave: leave,
        child: LayoutBuilder(
          builder: (context, constraints) {
            final wide = constraints.maxWidth >= focusContextFoldsBelow;
            final contextWidth = math.max(
              focusContextMinWidth,
              constraints.maxWidth -
                  sidebarWidth -
                  focusStageMaxWidth -
                  2 * Space.xxl,
            );
            return ListenableBuilder(
              listenable: session,
              builder: (context, _) => Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  FocusQueue(session: session),
                  Expanded(
                    child: Stack(
                      children: [
                        Positioned.fill(
                          child: FocusStage(
                            session: session,
                            leave: leave,
                            showContextButton: !wide,
                          ),
                        ),
                        // Folded: over the stage's right side, as the
                        // chip's panel floats.
                        if (!wide && session.contextOpen)
                          Positioned(
                            top: titleBarHeight + Space.s,
                            right: Space.s,
                            bottom: Space.s,
                            width: math.min(
                              focusContextMinWidth + 40,
                              constraints.maxWidth - sidebarWidth - Space.xl,
                            ),
                            child: Container(
                              clipBehavior: Clip.antiAlias,
                              decoration: BoxDecoration(
                                color: palette.canvas,
                                borderRadius: BorderRadius.circular(
                                  Radii.floating,
                                ),
                                border: Border.all(color: palette.hairline),
                                boxShadow: floatingShadow,
                              ),
                              child: FocusContext(session: session),
                            ),
                          ),
                      ],
                    ),
                  ),
                  if (wide && session.contextOpen)
                    Container(
                      width: contextWidth,
                      decoration: BoxDecoration(
                        border: Border(
                          left: BorderSide(color: palette.hairline),
                        ),
                      ),
                      child: FocusContext(session: session),
                    ),
                ],
              ),
            );
          },
        ),
      ),
    );
  }
}

/// The queue (§4.1): what waits, urgent first, then the oldest, and what
/// was answered this sitting dimmed where it was. The sidebar's width,
/// colour and hairline, under the window's traffic lights.
class FocusQueue extends StatelessWidget {
  const FocusQueue({
    super.key,
    required this.session,
    this.width = sidebarWidth,
    this.onPicked,
  });

  final FocusSession session;
  final double width;

  /// After a row's tap selects its card (the iPad's popover closes).
  final VoidCallback? onPicked;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final backend = session.backend;
    final count = session.waiting.length;
    return Container(
      width: width,
      decoration: BoxDecoration(
        color: palette.sidebar,
        border: Border(right: BorderSide(color: palette.hairline)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // The traffic lights sit over its top row, as over the sidebar.
          const WindowDrag(child: SizedBox(height: titleBarHeight)),
          Padding(
            padding: const EdgeInsets.fromLTRB(Space.l, 0, Space.l, Space.s),
            child: Row(
              children: [
                Flexible(
                  child: Text(
                    'Needs you',
                    // Whole at large text sizes, on two lines if need be.
                    maxLines: 2,
                    style: type.label.copyWith(
                      color: palette.text,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
                if (count > 0) ...[
                  const SizedBox(width: Space.s),
                  NeedsYouMark(count: count, label: needsYouLabel(count)),
                ],
                const Spacer(),
                ListenableBuilder(
                  listenable: backend.connected,
                  builder: (context, _) => backend.connected.value
                      ? const SizedBox.shrink()
                      : Text(
                          'Reconnecting…',
                          style: type.caption.copyWith(
                            color: palette.textFaint,
                          ),
                        ),
                ),
              ],
            ),
          ),
          const _OtherClaktories(),
          Expanded(
            child: ListenableBuilder(
              listenable: session.entries,
              builder: (context, _) => ListView(
                padding: const EdgeInsets.only(bottom: Space.l),
                children: [
                  for (final item in session.items)
                    _QueueRow(
                      key: ValueKey(item.key),
                      session: session,
                      item: item,
                      onPicked: onPicked,
                    ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// What waits on the person's other Claktories, under the queue's title
/// ("Laptop 2"), since focus mode hides the rail; a tap carries on in
/// focus mode there (Q10). Counted as the rail counts them.
class _OtherClaktories extends StatelessWidget {
  const _OtherClaktories();

  @override
  Widget build(BuildContext context) {
    final claktories = ClaktoriesScope.of(context);
    if (claktories == null) return const SizedBox.shrink();
    final others = [
      for (final claktory in claktories.all)
        if (claktory != claktories.selected) claktory,
    ];
    if (others.isEmpty) return const SizedBox.shrink();
    return ListenableBuilder(
      listenable: Listenable.merge([
        for (final claktory in others) claktory.backend.workspace,
      ]),
      builder: (context, _) {
        final waiting = [
          for (final claktory in others)
            if (needsYouCount(claktory.backend.workspace.workspace)
                case final count when count > 0)
              (claktory, count),
        ];
        if (waiting.isEmpty) return const SizedBox.shrink();
        return Padding(
          padding: const EdgeInsets.fromLTRB(
            Space.l - Space.m,
            0,
            Space.l,
            Space.s,
          ),
          child: Wrap(
            spacing: Space.xs,
            runSpacing: Space.xs,
            children: [
              for (final (claktory, count) in waiting)
                Button(
                  label: '${claktoryName(claktory.backend)} $count',
                  onPressed: () {
                    setFocusMode(claktory.backend, true);
                    claktories.select(claktory);
                  },
                ),
            ],
          ),
        );
      },
    );
  }
}

class _QueueRow extends StatelessWidget {
  const _QueueRow({
    super.key,
    required this.session,
    required this.item,
    this.onPicked,
  });

  final FocusSession session;
  final NeedsYouItem item;
  final VoidCallback? onPicked;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final backend = session.backend;
    final workspace = backend.workspace.workspace;
    final entries = session.entries;
    final entry = item.isEntry ? entries.entryOf(item) : null;
    final (:who, :what) = needsYouWords(
      backend,
      workspace,
      item,
      entry: entry,
      message: entries.messageOf(item),
    );
    final current = session.current?.key == item.key;
    final waiting = session.isWaiting(item.key);
    final outcome = session.outcomeOf(item.key);
    final time = entries.timeOf(item, workspace);
    final agent = workspace.agents
        .where((agent) => agent.id == item.agentId)
        .firstOrNull;
    return Semantics(
      button: true,
      selected: current,
      label: '$who, $what',
      excludeSemantics: true,
      child: Pressable(
        onTap: () {
          session.select(item.key);
          onPicked?.call();
        },
        builder: (context, hovered) => Container(
          constraints: const BoxConstraints(minHeight: focusRowHeight),
          decoration: BoxDecoration(
            color: current
                ? palette.selected
                : hovered
                ? palette.hover
                : null,
            border: Border(
              left: BorderSide(
                color: current ? palette.signal : const Color(0x00000000),
                width: 2,
              ),
            ),
          ),
          padding: const EdgeInsets.fromLTRB(
            Space.m - 2,
            Space.xs + 2,
            Space.m,
            Space.xs + 2,
          ),
          child: Row(
            children: [
              SizedBox.square(
                dimension: 24,
                child: Center(
                  child: agent != null
                      ? AgentCharacter(agent: agent, size: 24)
                      : ExtensionGlyph(
                          icon: _glyph(item),
                          name: '',
                          color: waiting
                              ? palette.textMuted
                              : palette.textFaint,
                        ),
                ),
              ),
              const SizedBox(width: Space.s + 2),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Row(
                      children: [
                        if (waiting && focusUrgent(item)) ...[
                          ExtensionGlyph(
                            icon: 'flag',
                            name: '',
                            color: palette.signal,
                            size: 12,
                          ),
                          const SizedBox(width: Space.xs),
                        ],
                        // Its agent's role beside its name, cut first
                        // (§3.10, Lucy 46817).
                        Expanded(
                          child: Builder(
                            builder: (context) {
                              final named = needsYouWho(workspace, item, who);
                              return NameRoleLine(
                                name: named.name,
                                role: named.role,
                                after: [
                                  if (named.after.isNotEmpty)
                                    TextSpan(text: named.after),
                                ],
                                style: type.label.copyWith(
                                  color: waiting
                                      ? palette.text
                                      : palette.textMuted,
                                  fontWeight: FontWeight.w600,
                                ),
                                roleStyle: type.label.copyWith(
                                  color: palette.textMuted,
                                  fontWeight: FontWeight.w400,
                                ),
                              );
                            },
                          ),
                        ),
                        const SizedBox(width: Space.s),
                        // Once answered, how it ended takes the second
                        // line, whatever its length ("Answered on Matt's
                        // iPad Pro").
                        if (outcome == null && time > 0)
                          Ticking(
                            active: true,
                            builder: (context, now) => Text(
                              formatAge(time, now),
                              style: type.caption.copyWith(
                                color: waitedADay(time, now)
                                    ? palette.signal
                                    : palette.textFaint,
                              ),
                            ),
                          ),
                      ],
                    ),
                    Text(
                      outcome?.words ?? what,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: outcome != null
                          ? type.caption.copyWith(color: palette.textFaint)
                          : type.body.copyWith(
                              color: waiting
                                  ? palette.textMuted
                                  : palette.textFaint,
                            ),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  static String _glyph(NeedsYouItem item) => switch (item.kind) {
    NeedsYouKind.vault || NeedsYouKind.recoveryKey => 'key-round',
    NeedsYouKind.device => 'smartphone',
    NeedsYouKind.providerWait => 'chart-column',
    NeedsYouKind.keepAwake ||
    NeedsYouKind.backendService ||
    NeedsYouKind.lidStuck => 'laptop-minimal',
    NeedsYouKind.chat => 'message-circle',
    NeedsYouKind.signIn => 'user',
    _ => 'arrow-left-right',
  };
}

/// The stage (§4.2): the card on it, as large as it reads well, with whose
/// it is above and how long it waited; nothing left, what was done.
class FocusStage extends StatelessWidget {
  const FocusStage({
    super.key,
    required this.session,
    required this.leave,
    this.showContextButton = false,
    this.trailing = const [],
    this.reviewBody,
  });

  final FocusSession session;
  final VoidCallback leave;

  /// Context is folded: its button in the header opens it.
  final bool showContextButton;

  /// More in the header's right, before Done (the iPad's "3 of 7" ‹ ›).
  final List<Widget> trailing;

  /// A review the platform shows itself (the iPad's PDFs and pictures,
  /// with Pencil); null, or returning null, for the stage's own.
  final FocusReviewBody? Function(
    BuildContext context,
    NeedsYouItem item,
    Entry card,
  )?
  reviewBody;

  @override
  Widget build(BuildContext context) {
    final item = session.current;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _StageHeader(
          session: session,
          leave: leave,
          showContextButton: showContextButton && item != null,
          trailing: trailing,
        ),
        // The next card fades up as the last goes (§5.3); with Reduce
        // Motion it simply swaps. Only one card is ever built, so only
        // one answers the keys.
        Expanded(
          child: item == null
              ? _NothingLeft(session: session)
              : TweenAnimationBuilder<double>(
                  key: ValueKey(item.key),
                  tween: Tween(begin: 0, end: 1),
                  duration: MediaQuery.disableAnimationsOf(context)
                      ? Duration.zero
                      : Motion.base,
                  curve: Curves.easeOut,
                  builder: (context, shown, child) =>
                      Opacity(opacity: shown, child: child),
                  child: _StageCard(
                    session: session,
                    item: item,
                    reviewBody: reviewBody,
                  ),
                ),
        ),
        _Receipt(session: session),
      ],
    );
  }
}

class _StageHeader extends StatelessWidget {
  const _StageHeader({
    required this.session,
    required this.leave,
    required this.showContextButton,
    required this.trailing,
  });

  final FocusSession session;
  final VoidCallback leave;
  final bool showContextButton;
  final List<Widget> trailing;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final workspace = session.backend.workspace.workspace;
    final item = session.current;
    final agent = item == null
        ? null
        : workspace.agents
              .where((agent) => agent.id == item.agentId)
              .firstOrNull;
    final project = agent == null
        ? null
        : workspace.projects
              .where((project) => project.id == agent.projectId)
              .firstOrNull;
    final pipe = agent == null || agent.pipeId.isEmpty
        ? null
        : workspace.pipes.where((pipe) => pipe.id == agent.pipeId).firstOrNull;
    final time = item == null ? 0 : session.entries.timeOf(item, workspace);
    final who = item == null
        ? ''
        : needsYouWords(
            session.backend,
            workspace,
            item,
            entry: item.isEntry ? session.entries.entryOf(item) : null,
          ).who;
    return WindowDrag(
      child: Container(
        constraints: const BoxConstraints(minHeight: titleBarHeight + Space.l),
        padding: const EdgeInsets.fromLTRB(Space.xl, Space.s, Space.m, Space.s),
        decoration: BoxDecoration(
          // Whose card it is reads from across the room.
          color: agent == null
              ? null
              : agentColor(agent.color).withValues(alpha: 0.06),
          border: Border(bottom: BorderSide(color: palette.hairline)),
        ),
        child: Row(
          children: [
            if (agent != null) ...[
              AgentCharacter(agent: agent, size: 32),
              const SizedBox(width: Space.m),
            ],
            // Who, and the actions beside it as words while they fit;
            // narrower (an iPad's focused tier, a Mac at a large text
            // size), the actions as glyphs, their words to VoiceOver.
            Expanded(
              child: LayoutBuilder(
                builder: (context, box) {
                  final go = item != null && item.kind != NeedsYouKind.signIn;
                  void goThere() {
                    leave();
                    openNeedsYouItem(context, item!, closePage: false);
                  }

                  void toggleContext() =>
                      session.contextOpen = !session.contextOpen;
                  final labels = [
                    if (go) 'Go to conversation',
                    if (showContextButton) 'Context',
                  ];
                  final words =
                      box.maxWidth - _buttonsWidth(context, labels) >=
                      _whoAtLeast;
                  return Row(
                    children: [
                      if (item != null)
                        Expanded(
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              // Its agent's role quietly beside its name,
                              // cut first (§3.10).
                              Builder(
                                builder: (context) {
                                  final named = needsYouWho(
                                    workspace,
                                    item,
                                    who,
                                  );
                                  return NameRoleLine(
                                    name: named.name,
                                    role: named.role,
                                    after: [
                                      if (named.after.isNotEmpty)
                                        TextSpan(text: named.after),
                                    ],
                                    style: type.emphasis.copyWith(
                                      color: palette.text,
                                    ),
                                    roleStyle: type.emphasis.copyWith(
                                      color: palette.textMuted,
                                      fontWeight: FontWeight.w400,
                                    ),
                                  );
                                },
                              ),
                              if (project != null || pipe != null || time > 0)
                                Ticking(
                                  active: time > 0,
                                  builder: (context, now) => Text(
                                    [
                                      if (project != null) project.name,
                                      if (pipe != null) pipe.name,
                                      if (time > 0)
                                        'Waited ${formatAge(time, now).replaceFirst(' ago', '')}',
                                    ].join(' · '),
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                    style: type.caption.copyWith(
                                      color: palette.textFaint,
                                    ),
                                  ),
                                ),
                            ],
                          ),
                        )
                      else
                        const Spacer(),
                      const SizedBox(width: Space.s),
                      if (go)
                        words
                            ? Button(
                                label: 'Go to conversation',
                                onPressed: goThere,
                              )
                            : GlyphButton(
                                label: 'Go to conversation',
                                active: false,
                                onTap: goThere,
                                glyph: (color) => ExtensionGlyph(
                                  icon: 'message-square-text',
                                  name: '',
                                  color: color,
                                ),
                              ),
                      if (showContextButton)
                        words
                            ? Button(label: 'Context', onPressed: toggleContext)
                            : GlyphButton(
                                label: 'Context',
                                active: session.contextOpen,
                                onTap: toggleContext,
                                glyph: (color) => ExtensionGlyph(
                                  icon: 'scroll-text',
                                  name: '',
                                  color: color,
                                ),
                              ),
                    ],
                  );
                },
              ),
            ),
            ...trailing,
            Button(label: 'Done', onPressed: leave),
          ],
        ),
      ),
    );
  }
}

/// What a platform shows for a review on the stage ([FocusStage.reviewBody]):
/// its [body] (the iPad's document host), and whether that [answers] the
/// card itself. Answering, it takes everything under the header, the card
/// and the stage's bar aren't built, and it puts the card's [CardAnswer]
/// in the stage's [AnswerDock] itself (docked false: the stage draws no
/// bar, only the keys go to it).
@immutable
class FocusReviewBody {
  const FocusReviewBody(this.body, {this.answers = false});

  final Widget body;
  final bool answers;
}

/// The file a card asks to have reviewed takes this much of the stage's
/// height above the card, and never less than [focusReviewLeast].
const focusReviewShare = 0.6;
const focusReviewLeast = 320.0;

/// The file a card asks to have reviewed, on the stage above it: the
/// viewer as a pane shows it, with what answers the review left to the
/// card (no Comment on media meanwhile); the iPad's own ([body]) for its
/// PDFs and pictures. Opening it notes the talk the card showed, as the
/// card's Review does (#492).
class _StageReview extends StatefulWidget {
  const _StageReview({
    super.key,
    required this.item,
    required this.card,
    this.own,
  });

  final NeedsYouItem item;
  final Entry card;

  /// The platform's own body for it ([FocusStage.reviewBody]).
  final FocusReviewBody? own;

  @override
  State<_StageReview> createState() => _StageReviewState();
}

class _StageReviewState extends State<_StageReview> {
  @override
  void initState() {
    super.initState();
    final ask = widget.card.askPerson;
    rememberReviewTalk(widget.item.agentId, widget.item.seq, ask.thread.length);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final path = widget.card.askPerson.reviewPath;
    final own = widget.own;
    // It answers the card itself: all of the stage, unframed.
    if (own != null && own.answers) return own.body;
    return Container(
      clipBehavior: Clip.antiAlias,
      decoration: BoxDecoration(
        color: palette.canvas,
        border: Border.all(color: palette.hairline),
        borderRadius: BorderRadius.circular(Radii.panel),
      ),
      child:
          own?.body ??
          ReviewAnswering(
            answering: true,
            child: viewerBody(
              context,
              ViewLeaf(
                kind: viewerKindFor(path),
                path: path,
                title:
                    path
                        .split('/')
                        .where((part) => part.isNotEmpty)
                        .lastOrNull ??
                    path,
              ),
            ),
          ),
    );
  }
}

/// Who asks keeps at least this much of the stage's header beside its
/// actions' words; with less, the actions are glyphs.
const _whoAtLeast = 160.0;

/// How wide [labels] are as [Button]s side by side.
double _buttonsWidth(BuildContext context, List<String> labels) {
  final touch = touchFirst(context);
  final style = touch ? PhoneType.label : TextStyles.label;
  final scaler = MediaQuery.textScalerOf(context)
      .clamp(maxScaleFactor: touch ? 2 : double.infinity);
  var width = 0.0;
  for (final label in labels) {
    final painter = TextPainter(
      text: TextSpan(text: label, style: style),
      textDirection: TextDirection.ltr,
      textScaler: scaler,
      maxLines: 1,
    )..layout();
    width += painter.width + 2 * (touch ? Touch.gutter : Space.m);
    painter.dispose();
  }
  return width;
}

/// The card on the stage: its real widget, with its real controls, in a
/// column at most [focusStageMaxWidth] wide. A press in it counts as the
/// person acting on it, so the card stopping waiting soon after was their
/// answer here.
class _StageCard extends StatelessWidget {
  const _StageCard({
    required this.session,
    required this.item,
    this.reviewBody,
  });

  final FocusSession session;
  final NeedsYouItem item;
  final FocusReviewBody? Function(
    BuildContext context,
    NeedsYouItem item,
    Entry card,
  )?
  reviewBody;

  @override
  Widget build(BuildContext context) {
    final entries = session.entries;
    final outcome = session.outcomeOf(item.key);
    return ListenableBuilder(
      listenable: entries,
      builder: (context, _) => LayoutBuilder(
        builder: (context, box) => _scroll(context, box, outcome),
      ),
    );
  }

  Widget _scroll(
    BuildContext context,
    BoxConstraints box,
    FocusOutcome? outcome,
  ) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final entries = session.entries;
    final card = item.isEntry ? entries.entryOf(item) : null;
    // A file it asks to have reviewed is the card's body, read here (§10).
    final review =
        card != null &&
            card.hasAskPerson() &&
            card.askPerson.reviewPath.isNotEmpty &&
            // Answered elsewhere or taken back while it's read (drawn on),
            // it stays, with how it ended and Next (§6.6).
            (session.isWaiting(item.key) ||
                outcome?.state == FocusOutcomeState.ended)
        ? card
        : null;
    final own = review == null ? null : reviewBody?.call(context, item, review);
    if (review != null && own != null && own.answers) {
      // The platform's review answers the card itself (the iPad's bar,
      // with the drawings): it publishes the card's answer in the dock,
      // and takes everything under the header; no card, no second bar.
      return Listener(
        onPointerDown: (_) => session.touched(item.key),
        child: AnswerDock(
          answer: session.answer,
          draft: null,
          itemKey: item.key,
          child: _StageReview(
            key: ValueKey(review.askPerson.reviewPath),
            item: item,
            card: review,
            own: own,
          ),
        ),
      );
    }
    return Listener(
      onPointerDown: (_) => session.touched(item.key),
      child: SingleChildScrollView(
        padding: const EdgeInsets.fromLTRB(
          Space.xl,
          Space.xxl,
          Space.xl,
          Space.xxl,
        ),
        child: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: focusStageMaxWidth),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (review != null) ...[
                  SizedBox(
                    height: math.max(
                      focusReviewLeast,
                      box.maxHeight * focusReviewShare,
                    ),
                    child: _StageReview(
                      key: ValueKey(review.askPerson.reviewPath),
                      item: item,
                      card: review,
                      own: own,
                    ),
                  ),
                  const SizedBox(height: Space.l),
                ],
                // How it ended, while it's still on the stage: taken
                // back, or answered on another device. It never moves on
                // by itself (§8).
                if (outcome != null &&
                    outcome.state == FocusOutcomeState.ended) ...[
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          outcome.words,
                          style: type.label.copyWith(color: palette.textMuted),
                        ),
                      ),
                      Button(
                        label: 'Next',
                        primary: true,
                        onPressed: session.waiting.isEmpty
                            ? null
                            : session.next,
                      ),
                    ],
                  ),
                  const SizedBox(height: Space.l),
                ],
                AnswerDock(
                  answer: session.answer,
                  draft: null,
                  itemKey: item.key,
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      // Changed by its agent a moment ago: its buttons
                      // take nothing until it's been seen (§8).
                      AbsorbPointer(
                        absorbing: session.guarded,
                        child: NeedsYouCard(
                          item: item,
                          entry: item.isEntry ? entries.entryOf(item) : null,
                          message: entries.messageOf(item),
                          failed: entries.failedOf(item),
                          workspace: session.backend.workspace.workspace,
                        ),
                      ),
                      _AnswerBar(session: session, item: item),
                    ],
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

/// The answer bar under a card that leaves its field and its yes and no
/// to the stage ([CardAnswer.docked]: questions and reviews): the field,
/// with the mic and hold-to-say, then the no and the yes with their keys;
/// after ⌘⌫ with nothing typed, the no's numbered reasons (§5.4).
class _AnswerBar extends StatefulWidget {
  const _AnswerBar({required this.session, required this.item});

  final FocusSession session;
  final NeedsYouItem item;

  @override
  State<_AnswerBar> createState() => _AnswerBarState();
}

class _AnswerBarState extends State<_AnswerBar> {
  final _focus = FocusNode(debugLabel: 'Focus mode answer');
  final _dictation = GlobalKey();

  FocusSession get _session => widget.session;

  @override
  void initState() {
    super.initState();
    _session.answer.addListener(_changed);
    _session.intoField = _focus.requestFocus;
  }

  @override
  void didUpdateWidget(_AnswerBar old) {
    super.didUpdateWidget(old);
    if (old.session != widget.session) {
      old.session.answer.removeListener(_changed);
      widget.session.answer.addListener(_changed);
    }
  }

  @override
  void dispose() {
    _session.answer.removeListener(_changed);
    if (_session.intoField == _focus.requestFocus) _session.intoField = null;
    if (identical(_session.say, _dictation.currentState)) _session.say = null;
    _focus.dispose();
    super.dispose();
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  /// Return in the field sends what's typed; on an empty field it does
  /// nothing, so a stray Return never approves (Q12).
  void _return() {
    if (_session.answer.value?.typed ?? false) {
      unawaited(_session.yes(byKey: true));
    }
  }

  @override
  Widget build(BuildContext context) {
    final answer = _session.answer.value;
    final field = answer?.field;
    if (answer == null || !answer.docked || field == null) {
      _session.say = null;
      return const SizedBox.shrink();
    }
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      final state = _dictation.currentState;
      _session.say = state is DictationHold ? state as DictationHold : null;
    });
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final session = _session;
    final workspace = session.backend.workspace.workspace;
    final who = focusWho(workspace, widget.item);
    final mac = defaultTargetPlatform == TargetPlatform.macOS;
    final placeholder = answer.placeholder.isNotEmpty
        ? answer.placeholder
        : who.isEmpty
        ? 'Your answer'
        : 'Your answer to $who';
    Widget keyed(Widget button, String keys) => !mac
        ? button
        : Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              button,
              const SizedBox(width: Space.xs),
              Text(
                keys,
                style: type.caption.copyWith(color: palette.textFaint),
              ),
            ],
          );
    final answerable = session.answerable;
    Widget buttons(BuildContext context) {
      final typed = answer.typed;
      final yes = typed ? answer.onSubmit : answer.onYes;
      return session.reasonsShowing
          ? Wrap(
              spacing: Space.s,
              runSpacing: Space.xs,
              alignment: WrapAlignment.end,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                for (final (index, reason) in focusDismissReasons.indexed)
                  keyed(
                    Button(
                      label: reason,
                      onPressed: () => unawaited(session.no(reason: reason)),
                    ),
                    '${index + 1}',
                  ),
                keyed(
                  Button(
                    label: focusDismissWithoutReason,
                    onPressed: () => unawaited(session.no(askWhy: false)),
                  ),
                  '⌘⌫',
                ),
              ],
            )
          : Wrap(
              spacing: Space.m,
              runSpacing: Space.xs,
              alignment: WrapAlignment.end,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                if (!typed)
                  keyed(
                    Button(
                      label: answer.no,
                      onPressed: answerable && answer.onNo != null
                          ? () => unawaited(session.no())
                          : null,
                    ),
                    '⌘⌫',
                  ),
                keyed(
                  Button(
                    // Changed a moment ago: it says so, and waits (§8).
                    label: session.guarded
                        ? 'Updated'
                        : typed
                        ? 'Submit'
                        : answer.yes,
                    primary: true,
                    onPressed: answerable && yes != null
                        ? () => unawaited(session.yes())
                        : null,
                  ),
                  '⌘↩',
                ),
              ],
            );
    }

    return Padding(
      padding: const EdgeInsets.only(top: Space.l),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Dictation(
            key: _dictation,
            controller: field,
            focusNode: _focus,
            keep: DictationFor(
              'focus/${widget.item.key}',
              who.isEmpty ? 'Your answer' : 'Your answer to $who',
            ),
            child: CallbackShortcuts(
              bindings: {
                const SingleActivator(LogicalKeyboardKey.enter): _return,
                const SingleActivator(LogicalKeyboardKey.numpadEnter): _return,
              },
              child: TextField(
                controller: field,
                focusNode: _focus,
                placeholder: placeholder,
                minLines: 1,
                maxLines: 6,
                semanticsLabel: placeholder,
              ),
            ),
          ),
          // The card's own word on its answer ("Not sent yet…").
          if (answer.status case final status? when status.isNotEmpty) ...[
            const SizedBox(height: Space.xs),
            Text(
              status,
              style: TypeScale.of(context).caption
                  .copyWith(color: Palette.of(context).textMuted),
            ),
          ],
          if (session.pressAgain) ...[
            const SizedBox(height: Space.xs),
            Text(
              'Press ⌘↩ again to run',
              style: TypeScale.of(context).caption
                  .copyWith(color: Palette.of(context).signal),
            ),
          ],
          const SizedBox(height: Space.s),
          // Typed or not, as the words change: Submit, or the no and yes.
          Align(
            alignment: Alignment.centerRight,
            child: ListenableBuilder(
              listenable: field,
              builder: (context, _) => buttons(context),
            ),
          ),
        ],
      ),
    );
  }
}

/// The receipt of an accepted answer at the stage's foot, for two seconds:
/// "Approved · June".
class _Receipt extends StatelessWidget {
  const _Receipt({required this.session});

  final FocusSession session;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final receipt = session.receipt;
    return AnimatedSwitcher(
      duration: Motion.fast,
      child: receipt == null
          ? const SizedBox(key: ValueKey('none'), height: Space.xl)
          : SizedBox(
              key: ValueKey(receipt),
              height: Space.xl,
              child: Center(
                child: Text(
                  receipt,
                  style: TypeScale.of(context).caption
                      .copyWith(color: palette.textMuted),
                ),
              ),
            ),
    );
  }
}

/// Nothing left (§4.2): "Nothing needs you.", what the person did this
/// sitting, and the agents at work now.
class _NothingLeft extends StatelessWidget {
  const _NothingLeft({required this.session});

  final FocusSession session;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final workspace = session.backend.workspace.workspace;
    final working = [
      for (final agent in workspace.agents)
        if (agent.status == AgentStatus.AGENT_STATUS_WORKING) agent,
    ];
    final answered = session.answered;
    final minutes = DateTime.now().difference(session.startedAt).inMinutes;
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(Space.xl),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              'Nothing needs you.',
              textAlign: TextAlign.center,
              style: type.title.copyWith(color: palette.textMuted),
            ),
            if (answered > 0) ...[
              const SizedBox(height: Space.s),
              Text(
                'You answered $answered in '
                '${minutes < 1 ? 'under a minute' : '$minutes min'}.',
                textAlign: TextAlign.center,
                style: type.caption.copyWith(color: palette.textFaint),
              ),
            ],
            if (working.isNotEmpty) ...[
              const SizedBox(height: Space.l),
              Wrap(
                spacing: Space.s,
                runSpacing: Space.s,
                alignment: WrapAlignment.center,
                children: [
                  for (final agent in working)
                    Semantics(
                      label: agent.name,
                      child: AgentCharacter(agent: agent, size: 24, live: true),
                    ),
                ],
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// Context (§4.3): the card's conversation, an agent's, a chat's or a
/// pipe's, live, ending at the card, which shows as a line since the stage
/// shows it whole; with the agent's own message box at its foot, for what
/// isn't an answer to the card. It's watched without selecting it
/// (WatchTranscript, §9.8), so moving through the queue changes nothing
/// on the person's other devices. A system item says what waits on it.
class FocusContext extends StatefulWidget {
  const FocusContext({super.key, required this.session});

  final FocusSession session;

  @override
  State<FocusContext> createState() => _FocusContextState();
}

/// What [item]'s Context watches; null for a system item.
WatchTranscript? focusWatchOf(NeedsYouItem item) => switch (item.kind) {
  NeedsYouKind.entry => WatchTranscript(agentId: item.agentId),
  NeedsYouKind.chat => WatchTranscript(chatId: item.chatId),
  NeedsYouKind.pipeHeld ||
  NeedsYouKind.pipeUnanswered => WatchTranscript(pipeId: item.pipeId),
  _ => null,
};

class _FocusContextState extends State<FocusContext> {
  WatchTranscript? _watching;
  final _follow = ValueNotifier(0);
  final _marker = ValueNotifier<List<TimelineItem>>(const []);

  FocusSession get _session => widget.session;

  @override
  void initState() {
    super.initState();
    _session.addListener(_changed);
    _changed();
  }

  @override
  void didUpdateWidget(FocusContext old) {
    super.didUpdateWidget(old);
    if (old.session != widget.session) {
      old.session.removeListener(_changed);
      widget.session.addListener(_changed);
      _changed();
    }
  }

  @override
  void dispose() {
    _session.removeListener(_changed);
    if (_watching != null) fireAndLog(_session.backend.watchTranscript(null));
    _follow.dispose();
    _marker.dispose();
    super.dispose();
  }

  /// The card changed: watch its conversation, if that's another.
  void _changed() {
    final item = _session.current;
    final watch = item == null ? null : focusWatchOf(item);
    if (watch == _watching) return;
    _watching = watch;
    if (watch != null) fireAndLog(_session.backend.watchTranscript(watch));
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final session = _session;
    final backend = session.backend;
    return ListenableBuilder(
      listenable: Listenable.merge([
        session,
        backend.watchedTranscript,
        backend.watchedPipe,
      ]),
      builder: (context, _) {
        final item = session.current;
        if (item == null) return const SizedBox.shrink();
        final workspace = backend.workspace.workspace;
        Widget line(String text) => Padding(
          padding: const EdgeInsets.all(Space.xl),
          child: Text(
            text,
            style: type.label.copyWith(color: palette.textMuted),
          ),
        );
        switch (item.kind) {
          case NeedsYouKind.entry || NeedsYouKind.chat:
            final id = item.kind == NeedsYouKind.chat
                ? item.chatId
                : item.agentId;
            final transcript = backend.watchedTranscript;
            if (transcript.agentId != id) {
              return line('Loading the conversation…');
            }
            return _conversation(context, item, workspace);
          case NeedsYouKind.pipeHeld || NeedsYouKind.pipeUnanswered:
            final pipe = workspace.pipes
                .where((pipe) => pipe.id == item.pipeId)
                .firstOrNull;
            final chat = backend.watchedPipe;
            if (pipe == null || chat.pipeId != item.pipeId) {
              return line('Loading the conversation…');
            }
            return _pipe(context, item, pipe, chat);
          default:
            // No conversation: what waits on it.
            return line(needsYouWords(backend, workspace, item).what);
        }
      },
    );
  }

  /// "▸ This card", where the card is in its conversation.
  Widget _thisCard(BuildContext context) {
    final type = TypeScale.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.xl,
        vertical: Space.s,
      ),
      child: Text(
        '▸ This card',
        style: type.caption.copyWith(color: Palette.of(context).signal),
      ),
    );
  }

  Widget _conversation(
    BuildContext context,
    NeedsYouItem item,
    Workspace workspace,
  ) {
    final backend = _session.backend;
    final transcript = backend.watchedTranscript;
    final chat = item.kind == NeedsYouKind.chat
        ? workspace.chats.where((chat) => chat.id == item.chatId).firstOrNull
        : null;
    final agent = item.kind == NeedsYouKind.entry
        ? workspace.agents
              .where((agent) => agent.id == item.agentId)
              .firstOrNull
        : null;
    final card = _session.entries.entryOf(item);
    final at = card?.timestampMs.toInt() ?? 0;
    final marker = [
      if (at > 0)
        TimelineItem(
          key: 'focus-card/${item.key}',
          timestampMs: at,
          build: _thisCard,
        ),
    ];
    if (_marker.value.length != marker.length ||
        (marker.isNotEmpty && _marker.value.first.key != marker.first.key)) {
      _marker.value = marker;
    }
    final working = agent != null
        ? atWork(agent)
        : chat?.status == AgentStatus.AGENT_STATUS_WORKING;
    Widget view = TranscriptView(
      key: ValueKey('focus-context/${transcript.agentId}/${transcript.loads}'),
      transcript: transcript,
      working: working,
      follow: _follow,
      queued: backend.watchedQueued,
      agentId: transcript.agentId,
      extras: _marker,
      hidden: (entry) => entry.seq.toInt() == item.seq,
    );
    if (chat != null) view = ChatScope(chatId: chat.id, child: view);
    final MessageTarget? target = agent != null
        ? AgentMessageTarget(agent, backend)
        : chat != null
        ? ChatMessageTarget(chat, backend)
        : null;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(child: view),
        if (target != null)
          MessageBox(
            key: ValueKey('focus-box/${target.key}'),
            target: target,
            onSent: () => _follow.value++,
            autofocus: false,
          ),
      ],
    );
  }

  Widget _pipe(
    BuildContext context,
    NeedsYouItem item,
    Pipe pipe,
    PipeChat chat,
  ) {
    final messages = chat.messages;
    return ListView(
      reverse: true,
      padding: const EdgeInsets.all(Space.l),
      children: [
        for (final message in messages.reversed)
          if (message.seq.toInt() == item.seq)
            _thisCard(context)
          else
            Padding(
              padding: const EdgeInsets.only(bottom: Space.m),
              child: PipeMessageView(
                key: ValueKey('focus-pipe/${message.seq}'),
                pipe: pipe,
                message: message,
              ),
            ),
      ],
    );
  }
}

/// Whether a text field has the keys, or a recording is on: the bare
/// letters are off, and so are the commands whose keys mean something in
/// a field (⌘⌫, ⌥⌘↑/↓, ⌘⇧C; §6.3 rule 3).
bool get focusFieldHasKeys {
  if (activeVoice.value != null) return true;
  final focused = FocusManager.instance.primaryFocus?.context;
  if (focused == null) return false;
  return focused.widget is EditableText ||
      focused.findAncestorWidgetOfExactType<EditableText>() != null;
}

/// One of focus mode's commands, for a native menu (the iPad's
/// CommandMenu): its id, its title as the menu shows it, its keys, and
/// what it does; [run] null while it can't (§6.3).
@immutable
class FocusCommand {
  const FocusCommand({
    required this.id,
    required this.title,
    required this.activator,
    required this.run,
  });

  /// 'focus.toggle', 'focus.yes', 'focus.no', 'focus.next',
  /// 'focus.previous', 'focus.context', 'focus.go'.
  final String id;

  /// "Approve", "Dismiss", "Next Card"…: the card's own words for its yes
  /// and no.
  final String title;
  final SingleActivator activator;
  final VoidCallback? run;
}

/// Focus mode's commands with modifiers (§6.3 rule 1), for a native menu,
/// as they stand now: call again when [session] or the field's keys
/// ([focusFieldHasKeys]) change. The bare letters are never here; they're
/// [FocusKeys]'.
List<FocusCommand> focusCommands(
  BuildContext context,
  FocusSession session, {
  required VoidCallback leave,
}) {
  final answer = session.answer.value;
  final field = focusFieldHasKeys;
  final item = session.current;
  final answerable = session.answerable && answer != null;
  final waiting = session.waiting.isNotEmpty;
  return [
    FocusCommand(
      id: 'focus.toggle',
      title: 'Leave Focus Mode',
      activator: focusModeKey,
      run: leave,
    ),
    FocusCommand(
      id: 'focus.yes',
      title: answer == null || answer.yes.isEmpty
          ? 'Approve'
          : answer.typed
          ? 'Submit'
          : answer.yes,
      activator: const SingleActivator(LogicalKeyboardKey.enter, meta: true),
      run: answerable && (answer.typed ? answer.onSubmit : answer.onYes) != null
          ? () => unawaited(session.yes(byKey: true))
          : null,
    ),
    FocusCommand(
      id: 'focus.no',
      title: answer == null || answer.no.isEmpty ? 'Dismiss' : answer.no,
      activator: const SingleActivator(
        LogicalKeyboardKey.backspace,
        meta: true,
      ),
      run: answerable && answer.onNo != null && !field
          ? () => unawaited(session.no())
          : null,
    ),
    FocusCommand(
      id: 'focus.next',
      title: 'Next Card',
      activator: const SingleActivator(
        LogicalKeyboardKey.arrowDown,
        meta: true,
        alt: true,
      ),
      run: waiting && !field ? session.next : null,
    ),
    FocusCommand(
      id: 'focus.previous',
      title: 'Previous Card',
      activator: const SingleActivator(
        LogicalKeyboardKey.arrowUp,
        meta: true,
        alt: true,
      ),
      run: waiting && !field ? session.previous : null,
    ),
    FocusCommand(
      id: 'focus.context',
      title: session.contextOpen ? 'Hide Context' : 'Show Context',
      activator: const SingleActivator(
        LogicalKeyboardKey.keyC,
        meta: true,
        shift: true,
      ),
      run: field ? null : () => session.contextOpen = !session.contextOpen,
    ),
    FocusCommand(
      id: 'focus.go',
      title: 'Go to Conversation',
      activator: const SingleActivator(
        LogicalKeyboardKey.keyG,
        meta: true,
        shift: true,
      ),
      run: item == null || item.kind == NeedsYouKind.signIn
          ? null
          : () {
              leave();
              openNeedsYouItem(context, item, closePage: false);
            },
    ),
  ];
}

/// How long ⌘↩ or ⌘⌫ is held before it records instead (§5.2).
const focusHoldToSay = Duration(milliseconds: 300);

/// Focus mode's keys (§5.1) over [child]: the letters only while no field
/// has the keys, Esc the innermost thing first. ⌘↩ and ⌘⌫ answer the card
/// on the stage; held, they record into its field instead.
class FocusKeys extends StatefulWidget {
  const FocusKeys({
    super.key,
    required this.session,
    required this.leave,
    required this.child,
    this.nativeIds = const {},
  });

  final FocusSession session;
  final VoidCallback leave;
  final Widget child;

  /// Commands a native menu binds itself ([focusCommands]' ids): left
  /// unbound here, so a key never runs twice.
  final Set<String> nativeIds;

  @override
  State<FocusKeys> createState() => _FocusKeysState();
}

class _FocusKeysState extends State<FocusKeys> {
  /// ⌘↩ or ⌘⌫ down, not yet let go: which, and the timer that makes it a
  /// hold.
  LogicalKeyboardKey? _down;
  Timer? _holdTimer;

  /// Held past [focusHoldToSay]: recording into this field.
  DictationHold? _saying;

  FocusSession get _session => widget.session;

  @override
  void dispose() {
    _holdTimer?.cancel();
    _focus.dispose();
    super.dispose();
  }

  static bool _isYes(LogicalKeyboardKey key) =>
      key == LogicalKeyboardKey.enter || key == LogicalKeyboardKey.numpadEnter;

  KeyEventResult _key(FocusNode node, KeyEvent event) {
    final key = event.logicalKey;
    final yes = _isYes(key);
    final no = key == LogicalKeyboardKey.backspace;
    if (!yes && !no) return KeyEventResult.ignored;
    if (event is KeyUpEvent) {
      if (key != _down) return KeyEventResult.ignored;
      _down = null;
      _holdTimer?.cancel();
      if (_saying case final say?) {
        _saying = null;
        say.holdEnd();
      } else {
        unawaited(yes ? _session.yes(byKey: true) : _session.no());
      }
      return KeyEventResult.handled;
    }
    if (event is KeyRepeatEvent) {
      return key == _down ? KeyEventResult.handled : KeyEventResult.ignored;
    }
    final keyboard = HardwareKeyboard.instance;
    if (!keyboard.isMetaPressed ||
        keyboard.isShiftPressed ||
        keyboard.isAltPressed ||
        keyboard.isControlPressed) {
      return KeyEventResult.ignored;
    }
    if (widget.nativeIds.contains(yes ? 'focus.yes' : 'focus.no')) {
      return KeyEventResult.ignored;
    }
    // In a field, ⌘⌫ deletes to the line's start, as everywhere on a Mac.
    if (no && focusFieldHasKeys) return KeyEventResult.ignored;
    if (!_session.answerable) return KeyEventResult.handled;
    _down = key;
    _holdTimer?.cancel();
    final say = _session.say;
    if (say != null) {
      _holdTimer = Timer(focusHoldToSay, () {
        if (!mounted || _down != key) return;
        _saying = say;
        unawaited(say.holdStart());
      });
    }
    return KeyEventResult.handled;
  }

  void _escape() {
    if (_session.reasonsShowing) {
      _session.reasonsShowing = false;
    } else if (focusFieldHasKeys) {
      // Out of the field; what's typed stays.
      FocusManager.instance.primaryFocus?.unfocus();
      _focus.requestFocus();
    } else {
      widget.leave();
    }
  }

  final _focus = FocusNode(debugLabel: 'Focus mode');

  /// 1–9: while the no's reasons show, a reason, sent with the no; on a
  /// card with choices (which secret), that choice, sending nothing (§5.1).
  void _digit(int index) {
    if (_session.reasonsShowing) {
      if (index < focusDismissReasons.length) {
        unawaited(_session.no(reason: focusDismissReasons[index]));
      }
      return;
    }
    final choices = _session.answer.value?.choices ?? const [];
    if (index < choices.length) choices[index].choose();
  }

  @override
  Widget build(BuildContext context) {
    final session = _session;
    void go() {
      final item = session.current;
      if (item == null) return;
      widget.leave();
      openNeedsYouItem(context, item, closePage: false);
    }

    const digits = [
      LogicalKeyboardKey.digit1,
      LogicalKeyboardKey.digit2,
      LogicalKeyboardKey.digit3,
      LogicalKeyboardKey.digit4,
      LogicalKeyboardKey.digit5,
      LogicalKeyboardKey.digit6,
      LogicalKeyboardKey.digit7,
      LogicalKeyboardKey.digit8,
      LogicalKeyboardKey.digit9,
    ];
    return Focus(
      onKeyEvent: _key,
      child: CallbackShortcuts(
        bindings: {
          // ⌘⇧N is FocusModeLayer's, wherever the keys are.
          const SingleActivator(LogicalKeyboardKey.escape): _escape,
        },
        child: ViewerKeys(
          bindings: {
            const SingleActivator(LogicalKeyboardKey.keyJ): session.next,
            const SingleActivator(LogicalKeyboardKey.arrowDown): session.next,
            const SingleActivator(LogicalKeyboardKey.keyK): session.previous,
            const SingleActivator(LogicalKeyboardKey.arrowUp): session.previous,
            const SingleActivator(LogicalKeyboardKey.keyC): () =>
                session.contextOpen = !session.contextOpen,
            const SingleActivator(LogicalKeyboardKey.keyG): go,
            const SingleActivator(LogicalKeyboardKey.keyR): () =>
                session.intoField?.call(),
            for (final (index, digit) in digits.indexed)
              SingleActivator(digit): () => _digit(index),
          },
          child: Focus(focusNode: _focus, autofocus: true, child: widget.child),
        ),
      ),
    );
  }
}

/// Whether this device is in focus mode on a Claktory: the backend's
/// (Settings.focus_mode, §9.1), so it's the same after a relaunch, and
/// each device has its own. A change shows at once, before the backend
/// says so; a Claktory too old to keep it keeps it for this run.
class _FocusMode extends ChangeNotifier implements ValueListenable<bool> {
  _FocusMode(this._backend) {
    _backend.workspace.addListener(_changed);
    _kept = _keptOn;
  }

  final Backend _backend;
  late bool _kept;

  /// Set here and not yet said back.
  bool? _asked;

  bool get _keptOn => _backend.workspace.workspace.settings.focusMode.contains(
    _backend.focusDeviceId,
  );

  @override
  bool get value => _asked ?? _kept;

  void _changed() {
    final was = value;
    final kept = _keptOn;
    if (kept != _kept) {
      _kept = kept;
      // Said back, or changed since (on another device's say).
      _asked = null;
    } else if (_asked == kept) {
      _asked = null;
    }
    if (value != was) notifyListeners();
  }

  void set(bool on) {
    if (on == value) return;
    _asked = on;
    notifyListeners();
    _backend.setFocusMode(on).catchError((Object error) {
      // An older Claktory: this run keeps it.
      if (error is! RequestFailure) throw error;
    });
  }
}

final _focusModes = Expando<_FocusMode>();

_FocusMode _focusModeOf(Backend backend) =>
    _focusModes[backend] ??= _FocusMode(backend);

/// Whether this device is in focus mode on [backend]'s Claktory.
ValueListenable<bool> focusModeOf(Backend backend) => _focusModeOf(backend);

/// Focus mode on or off on this device.
void setFocusMode(Backend backend, bool on) => _focusModeOf(backend).set(on);

/// Into focus mode, from the chip's panel, the page's Focus or ⌘⇧N.
void openFocusMode(BuildContext context) =>
    setFocusMode(AppScope.of(context), true);

/// The window, or focus mode over it (§3): the window stays as it was
/// underneath, out of sight and still, so leaving comes back to exactly
/// where the person was. ⌘⇧N turns it on and off anywhere in the window.
class FocusModeLayer extends StatefulWidget {
  const FocusModeLayer({super.key, required this.child, this.focus});

  final Widget child;

  /// Focus mode as this device lays it out (the iPad's tiers); null for
  /// the Mac's [NeedsYouFocus].
  final Widget Function(FocusSession session, VoidCallback leave)? focus;

  @override
  State<FocusModeLayer> createState() => _FocusModeLayerState();
}

class _FocusModeLayerState extends State<FocusModeLayer> {
  Backend? _backend;
  FocusSession? _session;

  @override
  void initState() {
    super.initState();
    // ⌘⇧N wherever the keys are, a field's or nothing's: the keyboard's
    // own handler, so nothing focused (or a field) can keep it.
    HardwareKeyboard.instance.addHandler(_key);
  }

  bool _key(KeyEvent event) {
    if (!focusModeKey.accepts(event, HardwareKeyboard.instance)) return false;
    // Not under a dialog or a page opened over the window.
    if (!mounted || ModalRoute.of(context)?.isCurrent == false) return false;
    final backend = _backend;
    if (backend == null) return false;
    setFocusMode(backend, _session == null);
    return true;
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final backend = AppScope.of(context);
    if (identical(backend, _backend)) return;
    final old = _backend;
    if (old != null) focusModeOf(old).removeListener(_changed);
    _backend = backend;
    focusModeOf(backend).addListener(_changed);
    _changed();
  }

  void _changed() {
    final backend = _backend!;
    final on = focusModeOf(backend).value;
    final old = _session;
    // Off, or another Claktory now: the sitting ends; on there, a new one.
    if (old != null && (!on || !identical(old.backend, backend))) {
      _session = null;
      // After the frame that no longer shows it.
      WidgetsBinding.instance.addPostFrameCallback((_) => old.dispose());
    }
    if (on && _session == null) _session = FocusSession(backend);
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    HardwareKeyboard.instance.removeHandler(_key);
    final backend = _backend;
    if (backend != null) focusModeOf(backend).removeListener(_changed);
    _session?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final backend = _backend!;
    final session = _session;
    return Stack(
      fit: StackFit.expand,
      children: [
        Offstage(
          offstage: session != null,
          child: TickerMode(
            enabled: session == null,
            child: ExcludeFocus(
              excluding: session != null,
              child: widget.child,
            ),
          ),
        ),
        if (session != null)
          widget.focus?.call(session, () => setFocusMode(backend, false)) ??
              NeedsYouFocus(
                session: session,
                leave: () => setFocusMode(backend, false),
              ),
      ],
    );
  }
}
