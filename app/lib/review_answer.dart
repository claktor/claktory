// Answering a review where the file is (Matt's ask): while an agent's
// review ask waits on the file a viewer pane shows, a bar at the pane's
// foot (expanded or not) says who asks and what, with the answer's field:
// Approve and Dismiss while it's empty; typed, one Submit, and Return is
// Submit (the words go as the person's reply), or the written answer when
// the ask wants one. Nothing else is added to the file meanwhile: on media
// the field is the only input; on text, line comments stay and go with the
// answer. Answered, the pane closes and the person is back where they
// pressed Review. The same model as the iPhone's (mobile/review_bar.dart)
// and the iPad's PDFs (ipad/document_review.dart).
// Nothing is kept: the ask is the agent's conversation's, fetched as it
// waits; where to go back to is navigation, in memory.

import 'dart:async';
import 'dart:io' show FileSystemException;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'ask_card.dart' show AskThread;
import 'backend.dart';
import 'card_answer.dart' show CardDrafts, cardKey;
import 'card_files.dart';
import 'characters/character.dart';
import 'controls.dart';
import 'decision.dart' show CommentTyped, SubmitButton;
import 'dictation.dart';
import 'identity.dart';
import 'layout.dart';
import 'markdown.dart';
import 'outbox.dart' show connectionLost, decisionWaiting;
import 'mobile/metrics.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'state.dart' show TranscriptStore;
import 'visible_codes.dart' show proseCodes;

/// Whether a review ask waits on the file below: viewers then offer
/// nothing that doesn't answer it (no Comment on media). Set by the bars
/// that answer it, on every device.
class ReviewAnswering extends InheritedWidget {
  const ReviewAnswering({
    super.key,
    required this.answering,
    required super.child,
  });

  final bool answering;

  static bool of(BuildContext context) =>
      context
          .dependOnInheritedWidgetOfExactType<ReviewAnswering>()
          ?.answering ??
      false;

  @override
  bool updateShouldNotify(ReviewAnswering old) => old.answering != answering;
}

/// An agent's review ask waiting on a file: its entry's seq and the ask.
typedef PendingReview = ({int seq, AskPerson ask});

/// Where the person was when they pressed Review: the page and the agent
/// shown, to go back to once it's answered.
typedef _Return = ({
  String agentId,
  String path,
  AppPage page,
  String selected,
});

_Return? _return;

/// Notes where the person is as they open [path] to review it for
/// [agentId] (the ask card's Review), so answering it brings them back.
void rememberReviewReturn(BuildContext context, String agentId, String path) {
  final scope = AppScope.scope(context);
  _return = (
    agentId: agentId,
    path: path,
    page: scope.page.value,
    selected: scope.backend.workspace.workspace.selectedAgentId,
  );
}

/// How much of each card's talk (its thread's messages) the card showed
/// when the person pressed its Review, by "agent/seq": sent with the answer
/// as talk_seen, so an approval from the file is refused only when
/// the agent replied while it was open. In memory only, as where to come
/// back to is.
final _talkSeen = <String, int>{};

/// Notes that [agentId]'s card [seq] showed [talkSeen] messages of its talk
/// as its Review opened the file: the bars show what came after. It's the
/// card the bars answer from then ([reviewChosen]).
void rememberReviewTalk(String agentId, int seq, int talkSeen) {
  _talkSeen['$agentId/$seq'] = talkSeen;
  _chosen[agentId] = seq;
  reviewChosenChanged.value++;
}

/// The card whose Review the person pressed last, by agent: with two of
/// its cards waiting on one file, the bar answers that one, never just the
/// oldest. In memory only, as the talk seen is.
final _chosen = <String, int>{};

/// The card whose Review was pressed last for [agentId] ([rememberReviewTalk]),
/// until it's answered: the bars fetch it even once it no longer waits, so
/// it stays in the bar, held, however the bar came.
int? reviewChosen(String agentId) => _chosen[agentId];

/// Forgets every card whose Review was pressed, and the talk each showed.
@visibleForTesting
void forgetReviewChoices() {
  _chosen.clear();
  _talkSeen.clear();
}

/// Ticks as a card's Review is pressed: the bars look again.
final reviewChosenChanged = ValueNotifier<int>(0);

/// The card [agentId]'s bar on [path] answers, as [entryOf] has its
/// entries: the one whose Review was pressed, else the one the bar showed
/// last ([shown]), while it's undecided; else among [waiting], its seqs
/// waiting on the person, oldest first, the first pending review of
/// [path]. So a card a reply (from any device) has wait on its agent stays
/// in the bar, held ([reviewWaits]) until its agent answers, rather than
/// the bar falling back to an older card, which an Approve there would
/// answer. Once it's decided, the bar moves on.
PendingReview? reviewPick({
  required String agentId,
  required String path,
  required List<int> waiting,
  required Entry? Function(int seq) entryOf,
  int? shown,
}) {
  PendingReview? on(int seq) {
    final entry = entryOf(seq);
    if (entry == null || entry.whichKind() != Entry_Kind.askPerson) {
      return null;
    }
    final ask = entry.askPerson;
    return ask.reviewPath == path ? (seq: seq, ask: ask) : null;
  }

  bool pending(PendingReview? review) =>
      review?.ask.state == AskPersonState.ASK_PERSON_STATE_PENDING;

  final chosen = _chosen[agentId];
  // Pressed, stopped waiting and not fetched yet: no bar for a moment
  // rather than an older card's.
  if (chosen != null && !waiting.contains(chosen) && entryOf(chosen) == null) {
    return null;
  }
  for (final pinned in {chosen, shown}) {
    if (pinned == null) continue;
    final held = on(pinned);
    final ask = held?.ask;
    // Waiting, replied, or as fetched before it stopped waiting (fetched
    // again).
    if (ask != null &&
        !ask.resolved &&
        (ask.state == AskPersonState.ASK_PERSON_STATE_PENDING ||
            (ask.state == AskPersonState.ASK_PERSON_STATE_ANSWERED &&
                ask.replied))) {
      return held;
    }
  }
  for (final seq in waiting) {
    final review = on(seq);
    if (pending(review)) return review;
  }
  return null;
}

/// Whether [entry], as fetched, may be out of date: it was pending and no
/// longer waits on the person ([waits]), or the other way round; or it's
/// the card a bar holds ([shown], not waiting, replied), which its agent
/// may answer, resolve or take back at any time. Fetched again as the
/// workspace changes, so a held bar shows the card as it is; one decided
/// for good isn't fetched again.
bool reviewStale(Entry entry, {required bool waits, bool shown = false}) {
  if (!entry.hasAskPerson()) return false;
  final ask = entry.askPerson;
  final pending = ask.state == AskPersonState.ASK_PERSON_STATE_PENDING;
  if (pending != waits) return true;
  return shown &&
      !waits &&
      ask.state == AskPersonState.ASK_PERSON_STATE_ANSWERED &&
      ask.replied &&
      !ask.resolved;
}

/// The seqs a review bar's watch fetches now: what waits on the person not
/// fetched yet; the card whose Review was pressed ([reviewChosen]) and the
/// one shown, held while they wait on their agent, even when the bar came
/// after they stopped waiting (a pane built again); and those fetched
/// before that are [reviewStale].
Iterable<int> reviewToFetch({
  required String agentId,
  required List<int> waiting,
  required Map<int, Entry> entries,
  int? shown,
}) sync* {
  final chosen = _chosen[agentId];
  final pinned = {?chosen, ?shown};
  for (final seq in {...waiting, ...entries.keys, ...pinned}) {
    final cached = entries[seq];
    final waits = waiting.contains(seq);
    if (cached == null
        ? waits || pinned.contains(seq)
        : reviewStale(cached, waits: waits, shown: pinned.contains(seq))) {
      yield seq;
    }
  }
}

/// Whether [agentId]'s card [seq] waits on the person (Agent.waiting_seqs).
/// One a reply took out waits on its agent instead: its bar holds, its
/// buttons off, until the agent answers ([reviewPick]).
bool reviewWaits(Backend backend, String agentId, int seq) =>
    backend.workspace
        .agent(agentId)
        ?.waitingSeqs
        .any((each) => each.toInt() == seq) ??
    false;

/// The cards a review bar showed, to tell when it moved on by itself (the
/// card it showed closed) to another of its agent's cards on the file,
/// rather than to one whose Review the person pressed: the bar then says
/// so, plainly ([ReviewMovedOn]), and its field starts empty, so what's
/// typed next answers the card it means to.
class ReviewMoves {
  int? _last;
  int? _moved;

  /// Whether [seq], the card the bar shows now, is one it moved on to by
  /// itself; until it shows another.
  bool movedTo(String agentId, int seq) {
    if (seq != _last) {
      _moved = _last != null && reviewChosen(agentId) != seq ? seq : null;
      _last = seq;
    }
    return _moved == seq;
  }
}

/// Above the question of a card a review bar moved on to by itself
/// ([ReviewMoves]): the card it showed was closed, this is another.
class ReviewMovedOn extends StatelessWidget {
  const ReviewMovedOn({super.key, required this.name});

  final String name;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.xs),
      child: Semantics(
        liveRegion: true,
        child: Text(
          reviewMovedOn(name),
          style: TypeScale.of(context).caption
              .copyWith(color: palette.signal, fontWeight: FontWeight.w600),
        ),
      ),
    );
  }
}

/// What [ReviewMovedOn] says.
String reviewMovedOn(String name) =>
    'That request was closed. This is another one from $name.';

/// What a held bar says ([reviewWaits]): the person's reply is with its
/// agent.
String reviewHeld(AskPerson ask, String name) => ask.replied
    ? 'You replied. Waiting for $name to answer.'
    : 'Waiting for $name.';

/// How much of [agentId]'s card [seq]'s talk the person had seen as the
/// file opened: what the card showed as its Review was pressed, or 0 when
/// the file was opened some other way, so none of it was seen.
int reviewTalkSeen(String agentId, int seq) => _talkSeen['$agentId/$seq'] ?? 0;

/// What a review bar shows of its card's talk: what was said on it since
/// the person last saw it ([reviewTalkSeen]), its agent's replies above
/// all, so "Look at it again" has somewhere to point.
/// While a reply waits on its agent (the bar held), that reply last, as
/// the person's: the thread takes it in only with the agent's answer
/// .
List<AskThreadMessage> reviewTalkSince(String agentId, PendingReview review) {
  final seen = reviewTalkSeen(agentId, review.seq);
  final ask = review.ask;
  final thread = ask.thread;
  final since = seen >= thread.length
      ? const <AskThreadMessage>[]
      : thread.sublist(seen);
  if (ask.state != AskPersonState.ASK_PERSON_STATE_ANSWERED ||
      !ask.replied ||
      ask.resolved ||
      ask.answer.isEmpty) {
    return since;
  }
  return [...since, AskThreadMessage(text: ask.answer, atMs: ask.decidedMs)];
}

/// The talk since the person last saw the card ([reviewTalkSince]), as the
/// card shows it ([AskThread]), under the question in a little room of its
/// own that opens on the newest message; nothing when there's none.
class ReviewTalk extends StatelessWidget {
  const ReviewTalk({
    super.key,
    required this.agentId,
    required this.review,
    required this.agent,
    required this.name,
    this.maxHeight = 120,
  });

  final String agentId;
  final PendingReview review;
  final Agent? agent;
  final String name;
  final double maxHeight;

  @override
  Widget build(BuildContext context) {
    final talk = reviewTalkSince(agentId, review);
    final follows = reviewFollowsTalk(review.ask, name);
    if (talk.isEmpty && follows == null) return const SizedBox.shrink();
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.only(top: Space.s),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          if (talk.isNotEmpty)
            ConstrainedBox(
              constraints: BoxConstraints(maxHeight: maxHeight),
              child: SingleChildScrollView(
                reverse: true,
                child: AskThread(thread: talk, agent: agent, name: name),
              ),
            ),
          // What Approve means here, as the card says it.
          if (follows != null) ...[
            if (talk.isNotEmpty) const SizedBox(height: Space.xs),
            Text(
              follows,
              style: TypeScale.of(context).caption.copyWith(
                color: palette.textMuted,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
        ],
      ),
    );
  }
}

/// What answering means after a talk on the card, as the card says
/// it above its buttons: agreeing with where the talk ended, when its
/// agent's reply came after the card's words last changed
/// (AskPerson.approve_follows_talk); null otherwise.
String? reviewFollowsTalk(AskPerson ask, String name) {
  if (!ask.approveFollowsTalk) return null;
  return ask.wantsText
      ? "Your answer follows $name's last reply."
      : "Approving agrees with $name's last reply.";
}

/// [agentId]'s entry [seq] as the open conversation holds it, kept up to
/// date as it changes; null when another agent's conversation is open, or
/// it isn't among what's held.
Entry? heldEntry(TranscriptStore transcript, String agentId, int seq) {
  if (transcript.agentId != agentId) return null;
  for (var i = transcript.length - 1; i >= 0; i--) {
    final model = transcript[i];
    if (model.seq == seq) return model.entry;
    if (model.seq < seq) break;
  }
  return null;
}

/// The newer of an ask's two copies: the open conversation's ([held]) and
/// the one fetched ([fetched]), which is newer only when the broadcast of
/// its change hasn't come yet.
Entry? newerEntry(Entry? held, Entry? fetched) {
  if (held == null || fetched == null) return held ?? fetched;
  if (!held.hasAskPerson() || !fetched.hasAskPerson()) return held;
  final a = held.askPerson;
  final b = fetched.askPerson;
  return b.revision > a.revision ||
          b.thread.length > a.thread.length ||
          b.updatedMs > a.updatedMs
      ? fetched
      : held;
}

/// Sends the answer to [review] as its bar shows it: the card's revision
/// and all of its talk, which the bar shows past what its card
/// showed ([ReviewTalk]). Dismiss and a reply decide nothing, so a
/// card its agent changed meanwhile (a reply on it gives it a new
/// revision) doesn't stop them: refused, the card is
/// fetched again ([refresh]) and they go once more with its latest
/// revision. An approval or an answer refused so stays refused, for the
/// person to look at the card again, which [refresh] puts in the bar.
///
/// Cut off by the connection, it waits ([onWaiting]: the bar says "Not sent
/// yet") and goes again by itself once the connection is back, as a card
/// does: the first may have got there
/// meanwhile, and then the Claktory's "already answered" means it went.
Future<void> sendReview(
  Backend backend,
  String agentId,
  PendingReview review, {
  required Future<PendingReview?> Function() refresh,
  String text = '',
  bool dismiss = false,
  bool reply = false,
  List<Attachment> attachments = const [],
  VoidCallback? onWaiting,
}) async {
  var again = false;
  while (true) {
    try {
      await _sendReview(
        backend,
        agentId,
        review,
        refresh: refresh,
        text: text,
        dismiss: dismiss,
        reply: reply,
        attachments: attachments,
      );
      return;
    } on RequestFailure catch (failure) {
      // The one sent before the connection dropped got there.
      if (again && failure.message.startsWith(alreadyDecided)) return;
      if (!connectionLost(failure)) rethrow;
      onWaiting?.call();
      await _connectedAgain(backend, now: !again);
      again = true;
    }
  }
}

/// How the Claktory refuses an answer to a card already decided.
const alreadyDecided = 'This question was already';

/// Once [backend] is connected again: at once when it already is and
/// [now] (it dropped and came back meanwhile), else at its next connect.
Future<void> _connectedAgain(Backend backend, {required bool now}) {
  final connected = backend.connected;
  if (now && connected.value) return Future.value();
  final back = Completer<void>();
  var lost = !connected.value;
  void changed() {
    if (!connected.value) {
      lost = true;
    } else if (lost && !back.isCompleted) {
      connected.removeListener(changed);
      back.complete();
    }
  }

  connected.addListener(changed);
  return back.future;
}

Future<void> _sendReview(
  Backend backend,
  String agentId,
  PendingReview review, {
  required Future<PendingReview?> Function() refresh,
  required String text,
  required bool dismiss,
  required bool reply,
  required List<Attachment> attachments,
}) async {
  // The files the person added, the card's own: sent up first, each
  // once, after [attachments] (a PDF's drawings).
  final card = cardFilesKey(agentId, review.ask.id);
  final files = [
    ...attachments,
    ...await CardUploads.of(backend)
        .send(backend, agentId, card, backend.attachments.of(card)),
  ];
  Future<void> send(AskPerson ask) => backend.answerAsk(
    agentId,
    review.seq,
    text: text,
    dismiss: dismiss,
    reply: reply,
    attachments: files,
    revision: ask.revision,
    talkSeen: ask.thread.length,
  );
  void sent() {
    backend.attachments.take(card);
    CardUploads.of(backend).sent(card);
  }

  try {
    await send(review.ask);
    sent();
  } on RequestFailure catch (failure) {
    if (connectionLost(failure)) rethrow;
    // Fetched again, the bar shows the card as it is now; when that fails
    // too, what the backend said stands.
    PendingReview? fresh;
    try {
      fresh = await refresh();
    } on RequestFailure {
      fresh = null;
    }
    if ((dismiss || reply) &&
        fresh != null &&
        fresh.seq == review.seq &&
        fresh.ask.revision != review.ask.revision) {
      await send(fresh.ask);
      sent();
      return;
    }
    rethrow;
  }
}

/// Why a review's answer didn't go, under its bar: the backend's own
/// words when it refused it (the card changed, or its agent replied on it
/// since), "Couldn't send" only when it didn't get there.
String reviewFailure(RequestFailure failure) => connectionLost(failure)
    ? "Couldn't send your answer. Try again."
    : failure.message;

/// [agentId]'s review is answered: what its card showed is spent, and
/// which card was chosen. All an answer in focus mode does, where
/// the agent's panes stay as they are; [finishReview] does it too.
void forgetReview(String agentId) {
  _talkSeen.removeWhere((key, _) => key.startsWith('$agentId/'));
  _chosen.remove(agentId);
}

/// After [agentId]'s review of [path] is answered: its pane closes (and
/// with it the expanded view), and the person is back where they pressed
/// Review, when they did.
Future<void> finishReview(
  BuildContext context,
  String agentId,
  String path,
) async {
  final scope = AppScope.scope(context);
  final backend = scope.backend;
  final back = _return;
  _return = null;
  forgetReview(agentId);
  final agent = backend.workspace.agent(agentId);
  if (agent != null) {
    final root = layoutOf(agent);
    var tree = root;
    for (final leaf in leaves(root)) {
      final kind = leaf.leaf.kind;
      if (leaf.leaf.path == path &&
          kind != chatKind &&
          kind != filesKind &&
          kind != 'app') {
        tree = close(tree, leaf.id) ?? tree;
      }
    }
    if (tree != root || agent.layoutFocus.isNotEmpty) {
      await backend.setLayout(
        agentId,
        isChatOnly(tree) ? null : tree,
        focus: '',
      );
    }
  }
  if (back == null || back.agentId != agentId || back.path != path) return;
  if (back.selected.isNotEmpty &&
      back.selected != backend.workspace.workspace.selectedAgentId &&
      backend.workspace.agent(back.selected) != null) {
    await backend.selectAgent(back.selected);
  }
  if (back.page != AppPage.agent) scope.page.value = back.page;
}

/// The oldest of [agentId]'s asks waiting on [path], as the agent's
/// waiting entries are fetched, or as its open conversation holds it,
/// kept up to date: null while none is.
class ReviewWatch extends ChangeNotifier {
  ReviewWatch(this.backend, this.agentId, this.path) {
    backend.workspace.addListener(_changed);
    backend.transcript.revision.addListener(_held);
    reviewChosenChanged.addListener(_held);
    _fetch();
    final now = pending;
    _heard = now?.ask;
    _shown = now?.seq;
  }

  final Backend backend;
  final String agentId;
  final String path;

  final _entries = <int, Entry>{};
  final _fetching = <int>{};
  var _disposed = false;

  /// The ask listeners last heard of: the open conversation changes often
  /// (its streaming), the ask seldom.
  AskPerson? _heard;

  List<int> get _waiting => [
    for (final seq in backend.workspace.agent(agentId)?.waitingSeqs ?? [])
      seq.toInt(),
  ];

  /// The card the bar showed last: it stays, held, while a reply has it
  /// wait on its agent ([reviewPick]).
  int? _shown;

  PendingReview? get pending => reviewPick(
    agentId: agentId,
    path: path,
    waiting: _waiting,
    entryOf: (seq) =>
        newerEntry(heldEntry(backend.transcript, agentId, seq), _entries[seq]),
    shown: _shown,
  );

  /// Fetches [seq] again, as it is now (its agent changed it, say): what
  /// waits on the file once it's come.
  Future<PendingReview?> refresh(int seq) async {
    final entry = await backend.fetchEntry(agentId, seq);
    if (_disposed) return null;
    if (entry != null) _entries[seq] = entry;
    _notify();
    return pending;
  }

  void _notify() {
    final now = pending;
    _heard = now?.ask;
    _shown = now?.seq;
    notifyListeners();
  }

  void _held() {
    if (_disposed || identical(pending?.ask, _heard)) return;
    _notify();
  }

  void _changed() {
    if (_disposed) return;
    _notify();
    _fetch();
  }

  void _fetch() {
    // What waits, what the bar holds, and what changed since it came.
    for (final seq in reviewToFetch(
      agentId: agentId,
      waiting: _waiting,
      entries: _entries,
      shown: _shown,
    ).toList()) {
      if (!_fetching.add(seq)) continue;
      unawaited(() async {
        try {
          final entry = await backend.fetchEntry(agentId, seq);
          if (entry != null && !_disposed) {
            _entries[seq] = entry;
            _notify();
          }
        } on RequestFailure {
          // Asked again when the workspace next changes.
        } finally {
          _fetching.remove(seq);
        }
      }());
    }
  }

  @override
  void dispose() {
    _disposed = true;
    backend.workspace.removeListener(_changed);
    backend.transcript.revision.removeListener(_held);
    reviewChosenChanged.removeListener(_held);
    super.dispose();
  }
}

/// A viewer pane's file, for [agentId]: as it is, or while the agent's
/// review ask waits on [path], with the bar that answers it at its foot
/// and [ReviewAnswering] over it.
class ReviewFrame extends StatefulWidget {
  const ReviewFrame({
    super.key,
    required this.agentId,
    required this.path,
    required this.child,
  });

  final String agentId;
  final String path;
  final Widget child;

  @override
  State<ReviewFrame> createState() => _ReviewFrameState();
}

class _ReviewFrameState extends State<ReviewFrame> {
  ReviewWatch? _watch;

  /// The child keeps its state as the bar comes and goes.
  final _childKey = GlobalKey();

  // What's typed in each card's bar is the card's draft ([CardDrafts]): it
  // stays while the bar goes and comes back (a reply from another device),
  // and while the app runs, wherever the card's reopened.

  /// Whether the bar moved on to its card by itself.
  final _moves = ReviewMoves();

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _rewatch();
  }

  @override
  void didUpdateWidget(ReviewFrame old) {
    super.didUpdateWidget(old);
    if (old.agentId != widget.agentId || old.path != widget.path) {
      _rewatch(force: true);
    }
  }

  void _rewatch({bool force = false}) {
    final backend = AppScope.of(context);
    if (!force && _watch?.backend == backend) return;
    _watch?.dispose();
    _watch = ReviewWatch(backend, widget.agentId, widget.path);
  }

  @override
  void dispose() {
    _watch?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: _watch!,
    builder: (context, _) {
      final pending = _watch!.pending;
      final child = KeyedSubtree(key: _childKey, child: widget.child);
      if (pending == null) return child;
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Expanded(child: ReviewAnswering(answering: true, child: child)),
          CardDropArea(
            child: ReviewBar(
              key: ValueKey(pending.seq),
              agentId: widget.agentId,
              path: widget.path,
              review: pending,
              movedOn: _moves.movedTo(widget.agentId, pending.seq),
              refresh: () => _watch!.refresh(pending.seq),
              answer: CardDrafts.of(_watch!.backend)
                  .field(cardKey(widget.agentId, pending.ask.id)),
            ),
          ),
        ],
      );
    },
  );
}

/// Whether [path] is a text file's, whose line comments go with the
/// answer (rather than media, where the field is all there is).
bool reviewHasLineComments(String path) {
  final dot = path.lastIndexOf('.');
  final ext = dot < 0 ? '' : path.substring(dot + 1).toLowerCase();
  return !const {
    'png', 'jpg', 'jpeg', 'gif', 'webp', 'heic', 'bmp', 'tif', 'tiff', //
    'svg', 'mp4', 'mov', 'm4v', 'webm', 'pdf',
  }.contains(ext);
}

/// At a viewer's foot while [review] waits: the agent and its question,
/// the answer's field (Return sends it; Shift-Return a new line), Dismiss
/// and Approve, or Send once something's typed (Send only, when the ask
/// wants words). Answered, [finishReview].
class ReviewBar extends StatefulWidget {
  const ReviewBar({
    super.key,
    required this.agentId,
    required this.path,
    required this.review,
    required this.refresh,
    required this.answer,
    this.movedOn = false,
  });

  final String agentId;
  final String path;
  final PendingReview review;

  /// The bar moved on to it by itself, from a card that closed: it says
  /// so ([ReviewMovedOn]).
  final bool movedOn;

  /// Fetches the card again, as it is now: after a refusal, the bar shows
  /// what changed ([sendReview]).
  final Future<PendingReview?> Function() refresh;

  /// What's typed: the frame's, so it outlasts the bar.
  final TextEditingController answer;

  @override
  State<ReviewBar> createState() => _ReviewBarState();
}

class _ReviewBarState extends State<ReviewBar> {
  TextEditingController get _text => widget.answer;
  final _focus = FocusNode(debugLabel: 'Review answer');
  var _busy = false;

  /// Why the last answer didn't go: it couldn't be sent, or its agent
  /// changed the card meanwhile.
  String? _failed;

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  bool get _wantsText => widget.review.ask.wantsText;

  /// [dismiss], or answer; with [reply], what's typed on an
  /// approval goes as the person's reply, neither a yes nor a no.
  Future<void> _answer({required bool dismiss, bool reply = false}) async {
    final text = _text.text.trim();
    if (_busy ||
        // Held: it waits on its agent (Return, say).
        !reviewWaits(AppScope.of(context), widget.agentId, widget.review.seq) ||
        (!dismiss && _wantsText && text.isEmpty) ||
        (reply && text.isEmpty)) {
      return;
    }
    setState(() {
      _busy = true;
      _failed = null;
    });
    try {
      // The card as the bar shows it, and its talk.
      await sendReview(
        AppScope.of(context),
        widget.agentId,
        widget.review,
        refresh: widget.refresh,
        text: text,
        dismiss: dismiss,
        reply: reply,
        // Cut off: "Not sent yet", and it goes once the connection's back.
        onWaiting: () {
          if (mounted) setState(() => _failed = decisionWaiting);
        },
      );
      // Sent: what was typed went with it.
      _text.clear();
      if (!mounted) return;
      // Sent, after waiting for the connection, maybe.
      setState(() => _failed = null);
      await finishReview(context, widget.agentId, widget.path);
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() => _failed = reviewFailure(failure));
      }
    } on FileSystemException catch (error) {
      // A file to go with it couldn't be read.
      if (mounted) setState(() => _failed = cardFilesFailure(error));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  /// Where the files added to the answer wait: the card's, so
  /// what was added on it comes here too.
  String get _files => cardFilesKey(widget.agentId, widget.review.ask.id);

  void _filesNote(String? note) {
    if (mounted) setState(() => _failed = note);
  }

  /// Return sends what's typed: a written answer, or on an approval the
  /// person's reply (Submit). An empty field never approves by itself.
  void _return() {
    if (_text.text.trim().isEmpty) return;
    unawaited(_answer(dismiss: false, reply: !_wantsText));
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final backend = AppScope.of(context);
    final agent = backend.workspace.agent(widget.agentId);
    final name = agent?.name ?? 'The agent';
    final color = agentColor(agent?.color ?? 'slate');
    final ask = widget.review.ask;
    final touch = touchFirst(context);
    // A reply has it wait on its agent: the bar holds, nothing to send
    // until the agent answers.
    final held = !reviewWaits(backend, widget.agentId, widget.review.seq);
    final off = _busy || held;
    final note = held && _failed == null ? reviewHeld(ask, name) : _failed;
    return Container(
      padding: EdgeInsets.fromLTRB(
        Space.l,
        Space.m,
        Space.l,
        Space.m + MediaQuery.paddingOf(context).bottom,
      ),
      decoration: BoxDecoration(
        color: palette.sidebar,
        border: Border(top: BorderSide(color: palette.hairline)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          Row(
            children: [
              if (agent != null) ...[
                AgentCharacter(agent: agent, size: 20),
                const SizedBox(width: Space.xs + 2),
              ],
              Flexible(
                child: Text(
                  '$name asks you to review this',
                  style: type.caption.copyWith(
                    color: color,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: Space.xs),
          if (widget.movedOn) ReviewMovedOn(name: name),
          // The question; a long one scrolls in its own little room so
          // the file keeps its.
          ConstrainedBox(
            constraints: const BoxConstraints(maxHeight: 120),
            child: SingleChildScrollView(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  // The agent's words: what reorders them shows by its code
                  // (proseCodes).
                  Text(
                    proseCodes(ask.title),
                    style: type.emphasis.copyWith(color: palette.text),
                  ),
                  if (ask.body.isNotEmpty) ...[
                    const SizedBox(height: 2),
                    Markdown(proseCodes(ask.body), color: palette.textMuted),
                  ],
                ],
              ),
            ),
          ),
          // What was said on its card since the person saw it.
          ReviewTalk(
            agentId: widget.agentId,
            review: widget.review,
            agent: agent,
            name: name,
          ),
          const SizedBox(height: Space.s),
          // Typed or said: the mic puts what's said in, to check before
          // sending; Dismiss and Send step aside while it records.
          CardFileDrop(
            card: _files,
            label: 'your answer to $name',
            enabled: !_busy,
            onNote: _filesNote,
            child: Dictation(
              controller: _text,
              focusNode: _focus,
              enabled: !_busy,
              // As the ask's card keeps it: it comes back to either.
              keep: DictationFor(
                cardKey(widget.agentId, ask.id),
                ask.title.isEmpty
                    ? 'Your answer to $name'
                    : 'Your answer to $name: ${ask.title}',
                select: (backend) => backend.selectAgent(widget.agentId),
              ),
              trailing: Row(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.end,
                children: [
                  // Files with the answer.
                  CardAttachButton(
                    card: _files,
                    enabled: !_busy,
                    onNote: _filesNote,
                  ),
                  const SizedBox(width: Space.xs),
                  // Typed on an approval: one Submit in place of both, the
                  // words a reply, neither a yes nor a no.
                  CommentTyped(
                    controller: _text,
                    builder: (context, typed) => Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        if (typed && !_wantsText)
                          _Sized(
                            touch: touch,
                            child: SubmitButton(
                              onPressed: off
                                  ? null
                                  : () => _answer(dismiss: false, reply: true),
                            ),
                          )
                        else ...[
                          _Sized(
                            touch: touch,
                            child: Button(
                              label: 'Dismiss',
                              onPressed: off
                                  ? null
                                  : () => _answer(dismiss: true),
                            ),
                          ),
                          const SizedBox(width: Space.xs),
                          _Sized(
                            touch: touch,
                            child: Button(
                              label: _wantsText ? 'Send' : 'Approve',
                              primary: true,
                              onPressed: off || (_wantsText && !typed)
                                  ? null
                                  : () => _answer(dismiss: false),
                            ),
                          ),
                        ],
                      ],
                    ),
                  ),
                ],
              ),
              child: CallbackShortcuts(
                bindings: {
                  const SingleActivator(LogicalKeyboardKey.enter): _return,
                  const SingleActivator(LogicalKeyboardKey.numpadEnter):
                      _return,
                },
                child: Semantics(
                  hint: 'Return sends it to $name.',
                  child: TextField(
                    controller: _text,
                    focusNode: _focus,
                    placeholder: 'Your answer to $name',
                    minLines: 1,
                    maxLines: 6,
                    textInputAction: TextInputAction.send,
                    onSubmitted: (_) => _return(),
                    semanticsLabel: 'Your answer to $name',
                  ),
                ),
              ),
            ),
          ),
          CardFileChips(card: _files, busy: _busy),
          if (note != null || reviewHasLineComments(widget.path)) ...[
            const SizedBox(height: Space.xs),
            Text(
              note ?? 'Your comments on lines go to $name with your answer.',
              style: type.caption.copyWith(
                color: note == decisionWaiting || (held && _failed == null)
                    ? palette.textMuted
                    : note != null
                    ? palette.danger
                    : palette.textFaint,
              ),
            ),
          ],
        ],
      ),
    );
  }
}

/// A button at touch size on a touch-first device (44 pt high).
class _Sized extends StatelessWidget {
  const _Sized({required this.touch, required this.child});

  final bool touch;
  final Widget child;

  @override
  Widget build(BuildContext context) => touch
      ? ConstrainedBox(
          constraints: const BoxConstraints(minHeight: Touch.target),
          child: Center(widthFactor: 1, child: child),
        )
      : child;
}
