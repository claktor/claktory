import 'dart:async';
import 'dart:convert';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';

import '../protocol/factory/v1/factory.pb.dart' show DmDeliveryState;
import '../tunnel/api/dm.dart' as dm;
import '../user_apps/session.dart';
import 'contacts.dart';
import 'e2ee.dart';
import 'review.dart';

// D3c, end to end since E5: a two-person chat at the account, straight
// from the service as this device, so it works with every Claktory off.
// The service relays what it can't read: the history is this device's
// own, decrypted into its sealed store by [E2ee] as each group's feed
// comes, and an open chat reads it from there as it moves. The person's
// draft, how far they've read and what they deleted go to their other
// devices through their self group, never to the service. A message
// shows Sent only once it came back in the feed, with its place in the
// conversation; until then Sending…, and when the answer never came,
// that it may or may not have been sent, with Try again, which sends the
// same bytes (so never twice). No presence, and nothing ever says the
// other person read it.

/// The most a message may hold, in bytes of UTF-8, as the service checks.
const messageByteLimit = 16384;

/// A draft as the service keeps it, and its revision (0: none yet).
class DmDraft {
  const DmDraft({this.text = '', this.revision = 0});

  factory DmDraft.fromJson(Object? json) => switch (json) {
    final Map<String, Object?> draft => DmDraft(
      text: _text(draft, 'text'),
      revision: _int(draft, 'revision'),
    ),
    _ => const DmDraft(),
  };

  final String text;
  final int revision;
}

/// What the person keeps of a conversation, the same on all their
/// devices and never seen by the other person: how far they've read and
/// deleted, and their draft.
class DmView {
  const DmView({
    this.readSeq = 0,
    this.deletedSeq = 0,
    this.draft = const DmDraft(),
  });

  factory DmView.fromJson(Object? json) => switch (json) {
    final Map<String, Object?> view => DmView(
      readSeq: _int(view, 'read_seq'),
      deletedSeq: _int(view, 'deleted_seq'),
      draft: DmDraft.fromJson(view['draft']),
    ),
    _ => const DmView(),
  };

  final int readSeq;
  final int deletedSeq;
  final DmDraft draft;

  DmView copyWith({int? readSeq, int? deletedSeq, DmDraft? draft}) => DmView(
    readSeq: readSeq ?? this.readSeq,
    deletedSeq: deletedSeq ?? this.deletedSeq,
    draft: draft ?? this.draft,
  );
}

/// A conversation with a friend, now or before.
class DmConversation {
  const DmConversation({
    required this.id,
    required this.contactId,
    required this.generation,
    required this.person,
    required this.active,
    required this.sinceMs,
    required this.endedMs,
    required this.lastSeq,
    this.view = const DmView(),
  });

  factory DmConversation.fromJson(Map<String, Object?> json) => DmConversation(
    id: _text(json, 'id'),
    contactId: _text(json, 'contact_id'),
    generation: _int(json, 'contact_generation'),
    person: PeoplePerson.fromJson(switch (json['with']) {
      final Map<String, Object?> person => person,
      _ => const {},
    }),
    active: json['active'] == true,
    sinceMs: _int(json, 'since_ms'),
    endedMs: _int(json, 'ended_ms'),
    lastSeq: _int(json, 'last_seq'),
    view: DmView.fromJson(json['view']),
  );

  final String id;
  final String contactId;

  /// The friendship's generation, which a message names.
  final int generation;

  /// Whom it's with: as they are now, or as they were when it ended.
  final PeoplePerson person;

  /// Still friends: messages can be sent. Ended, it can still be read.
  final bool active;
  final int sinceMs;
  final int endedMs;
  final int lastSeq;
  final DmView view;

  DmConversation withView(DmView view) => DmConversation(
    id: id,
    contactId: contactId,
    generation: generation,
    person: person,
    active: active,
    sinceMs: sinceMs,
    endedMs: endedMs,
    lastSeq: lastSeq,
    view: view,
  );
}

/// A line in a chat: a message, or a change in who can read it.
class DmMessage {
  const DmMessage({
    required this.seq,
    required this.mine,
    required this.acceptedMs,
    this.text = '',
    this.agentName = '',
    this.byAgent = false,
    this.clientMessageId = '',
    this.members,
    this.fromLeaf = '',
    this.resends = '',
    this.sentMs = 0,
    this.departed = false,
  });

  /// From this device's history: [me] is the person's account.
  factory DmMessage.from(dm.DmMessage kept, String me) => DmMessage(
    seq: kept.seq,
    mine: kept.members == null ? kept.fromAccount == me : kept.mine,
    acceptedMs: kept.arrivedMs,
    text: kept.content.text ?? '',
    byAgent: kept.content.byAgent != null,
    agentName: kept.content.byAgent?.name ?? '',
    clientMessageId: kept.content.clientMessageId,
    members: kept.members,
    fromLeaf: kept.fromLeaf,
    resends: switch (_extra(kept.content.extraJson)) {
      {'resends': final String id} => id,
      _ => '',
    },
    sentMs: switch (_extra(kept.content.extraJson)) {
      {'sent_ms': final int ms} when ms > 0 => ms,
      _ => 0,
    },
    departed: kept.departed,
  );

  static Object? _extra(String extra) {
    if (extra.isEmpty) return null;
    try {
      return jsonDecode(extra);
    } on FormatException {
      return null;
    }
  }

  final int seq;

  /// From this person (any of their devices); else from the friend.
  final bool mine;
  final int acceptedMs;
  final String text;

  /// Sent by an agent with a grant, approved by whoever it's from, and
  /// what that agent is called.
  final bool byAgent;
  final String agentName;

  /// The sending device's id for it.
  final String clientMessageId;

  /// Who came and went with a commit, instead of something said.
  final dm.DmMembers? members;

  /// Which device (or Claktory) of its account said it.
  final String fromLeaf;

  /// The person's earlier message this sends again, by its id, after the
  /// friend's Claktory couldn't read that one (#451); empty for others.
  final String resends;

  /// When its sender says they sent it, by their own clock (row 556); 0
  /// when it doesn't say. Only shown, never its place: that's
  /// [acceptedMs], when the person's own home took it, which a friend
  /// can't set.
  final int sentMs;

  /// From a device its person has since taken off their list, read only
  /// because one of their devices that was there all along took it out
  /// again (row 561): said under it.
  final bool departed;

  /// The time it shows: when it was sent, unless that's unknown or later
  /// than it arrived (a clock ahead), then when it arrived.
  int get shownMs => sentMs > 0 && sentMs <= acceptedMs ? sentMs : acceptedMs;

  /// When it arrived, said too once that's more than [arrivedLateAfter]
  /// past when it was sent; null otherwise.
  int? get arrivedLate =>
      sentMs > 0 &&
          sentMs <= acceptedMs &&
          acceptedMs - sentMs > arrivedLateAfter.inMilliseconds
      ? acceptedMs
      : null;
}

/// How much later than it was sent a message must arrive for its arrival
/// time to show too (Bruno, row 556).
const arrivedLateAfter = Duration(minutes: 5);

/// How the person's own message is getting on past their home Claktory
/// (#451), as the home says.
enum DmDelivered {
  /// Taken by their home; nothing more known yet.
  sent,

  /// The friend's Claktory took it.
  delivered,

  /// The friend's Claktory isn't reachable; it goes once it is.
  notYet,

  /// The friend's Claktory turns the person's away (row 567: it isn't
  /// their home any more, or doesn't know the person's yet); it goes once
  /// it takes it.
  notTaking,

  /// The friend's Claktory couldn't read it (their conversation moved on
  /// since); the person is asked whether to send it again.
  askResend,

  /// Sent again, as a new message, after it bounced.
  resent,

  /// Bounced, and being sent again by itself from this device, which sent
  /// it.
  resending,

  /// Bounced, sent from another of the person's devices, which decides.
  bounced,
}

/// A page of what changed since a cursor.
typedef DmChanges = ({
  List<Map<String, Object?>> events,
  String next,
  bool hasMore,
});

/// The messages routes, through [session].
class ChatApi {
  const ChatApi(this.session);

  final UserAppsSession session;

  static const _dms = '/v1/apps/dms';

  Future<List<DmConversation>> conversations() async => [
    for (final item in _list(
      await session.get('$_dms/conversations'),
      'conversations',
    ))
      DmConversation.fromJson(item),
  ];

  /// Everything as it is now, and where its changes start.
  Future<
    ({
      List<DmConversation> conversations,
      List<DmProposal> proposals,
      List<DmAgentDraft> agentDrafts,
      String cursor,
    })
  >
  snapshot() async {
    final answer = await session.get('$_dms/sync');
    return (
      conversations: [
        for (final item in _list(answer, 'conversations'))
          DmConversation.fromJson(item),
      ],
      proposals: [
        for (final item in _list(answer, 'proposals'))
          DmProposal.fromJson(item),
      ],
      agentDrafts: [
        for (final item in _list(answer, 'agent_drafts'))
          DmAgentDraft.fromJson(item),
      ],
      cursor: _text(answer, 'cursor'),
    );
  }

  /// The messages agents proposed, for the person to decide.
  Future<List<DmProposal>> proposals() async => [
    for (final item in _list(await session.get('$_dms/proposals'), 'proposals'))
      DmProposal.fromJson(item),
  ];

  /// Approves [proposal], with the revision and the digest of exactly
  /// what this device opened; the device sends it after (#451, Mista
  /// 32507). `409 changed` ([UserAppProblem.stale]) when it changed since;
  /// `410 unsendable` ([UserAppProblem.gone]) when it can't be sent any
  /// more.
  Future<void> approve(DmProposal proposal) => session.post(
    '$_dms/proposals/${proposal.id}/approve',
    {'revision': proposal.revision, 'proposal_digest': proposal.digest},
  );

  /// Declines [proposal]: it's never sent.
  Future<void> decline(DmProposal proposal) =>
      session.post('$_dms/proposals/${proposal.id}/decline', {});

  /// The agents' open drafts in [conversation], newest first.
  Future<List<DmAgentDraft>> agentDrafts(String conversation) async => [
    for (final item in _list(
      await session.get('$_dms/conversations/$conversation/agent-drafts'),
      'drafts',
    ))
      DmAgentDraft.fromJson(item),
  ];

  /// Closes an agent's draft, used or dismissed.
  Future<void> closeAgentDraft(String conversation, String draft) =>
      session.delete('$_dms/conversations/$conversation/agent-drafts/$draft');

  /// What changed after [since], held up to [wait] seconds for something
  /// to; `410 resync_required` ([UserAppProblem.gone]) when [since] is
  /// too old.
  Future<DmChanges> changes(String since, {int wait = 0}) async {
    final answer = await session.get(
      '$_dms/sync?since=${Uri.encodeQueryComponent(since)}'
      '${wait > 0 ? '&wait=$wait' : ''}',
    );
    return (
      events: _list(answer, 'events'),
      next: _text(answer, 'next_cursor'),
      hasMore: answer['has_more'] == true,
    );
  }

  /// Hides the person's copy of [conversation] through [seq] at the
  /// service (its mailbox copies, for this account only).
  Future<void> hide(String conversation, int seq) => session.post(
    '$_dms/conversations/$conversation/delete',
    {'through_seq': seq},
  );
}

/// Where a message the person wrote here stands.
enum OutgoingState {
  /// On its way; it hasn't come back in the feed.
  sending,

  /// No answer came: it may or may not have been sent. Try again can't
  /// send it twice.
  unknown,

  /// The service refused it, and why ([OutgoingMessage.problem]); for an
  /// ended conversation, nothing more can be sent.
  refused,

  /// Kept on this device until the person's home Claktory takes it, and
  /// why ([OutgoingMessage.problem]: `unreachable` or `notHome`, the home
  /// isn't there; `homeFull`; `rateLimited`; `behind`): it goes by itself
  /// (#451).
  waiting,
}

/// A message written here that hasn't come back in the feed yet: kept
/// encrypted on this device until it does.
class OutgoingMessage {
  OutgoingMessage({
    required this.clientId,
    required this.text,
    this.state = OutgoingState.sending,
    this.problem = '',
    this.agentName = '',
  });

  final String clientId;
  final String text;

  /// An agent's message the person approved (#451): that agent's name.
  final String agentName;
  OutgoingState state;
  String problem;
}

/// One chat as it shows: this device's history of it, newest last, what
/// the person sent from here that hasn't come back yet, and their view of
/// it (read, draft, deleted), kept up as [e2ee] takes in the feeds.
class ChatThread extends ChangeNotifier {
  ChatThread(
    this.api,
    this.e2ee,
    this.conversation, {
    String Function()? newId,
    this.retryAfter = const Duration(seconds: 5),
    this.readEvery = const Duration(seconds: 3),
  }) : _newId = newId ?? newActionId;

  final ChatApi api;
  final E2ee e2ee;
  DmConversation conversation;
  final String Function() _newId;

  /// How long to wait after the service couldn't be reached.
  final Duration retryAfter;

  /// At most one read position goes to the person's devices in this time.
  final Duration readEvery;

  /// The conversation's group: empty until there is one.
  String group = '';

  /// The friend's account.
  String friend = '';

  /// The history read so far, oldest first, one per seq.
  final messages = <DmMessage>[];
  final outgoing = <OutgoingMessage>[];

  /// There's more history before the first message here.
  var hasEarlier = false;
  var loaded = false;

  /// Why the history couldn't be read, in the person's words.
  UserAppError? problem;

  /// The conversation ended while the person wrote to it.
  var ended = false;

  /// The draft as the person's devices last had it.
  String draft = '';
  var _view = const dm.DmView(draft: '', draftMs: 0, readSeq: 0, deletedSeq: 0);

  /// What agents proposed sending here, waiting for the person, oldest
  /// first; and the agents' suggested drafts, newest first.
  final proposals = <DmProposal>[];
  final agentDrafts = <DmAgentDraft>[];

  /// What came of the last decision on one, when it didn't go: `changed`
  /// (it changed since it was shown), `unsendable`, or the problem's name.
  String? reviewProblem;

  /// The agent whose proposal [reviewProblem] is about.
  String reviewAgent = '';

  /// Proposals and drafts being decided, by id.
  final deciding = <String>{};

  /// Agents' proposals and drafts the service lists here whose message
  /// hasn't come from their Claktory through the home yet (#451): never
  /// shown as text until it has, only that something is on its way.
  var arriving = 0;
  var _proposalsArriving = 0;
  var _draftsArriving = 0;
  List<DmProposal> _listed = const [];

  /// Puts text the person chose (Edit) into their box.
  void Function(String text)? onUse;

  /// Another device changed the draft (or it was read on opening): the
  /// box shows it unless the person typed since.
  void Function(String text)? onDraft;

  /// Still friends: it can be written to (once its group is here).
  bool get writable => conversation.active && !ended;

  /// It can be sent to now: writable, its group and this device's keys
  /// are here, and the group isn't waiting for a new security key, nor
  /// held at a change this device can't take yet (a send then would only
  /// fail: Fugo, row 654).
  bool get canSend =>
      writable &&
      group.isNotEmpty &&
      e2ee.setup == DmSetup.ready &&
      waitingFor == null &&
      joining == null &&
      !held;

  /// The friend isn't in this conversation yet (row 676): nothing is sent
  /// until a device of theirs is, and the chat says so. Their account.
  String? get joining => group.isEmpty ? null : e2ee.joining[group];

  /// Whose new security key the group waits for (row 630, Fugo): someone
  /// started over on a new key, and their devices under the old one are
  /// still in it until their new device takes them out. Nothing is sent
  /// until it has (it would reach those devices); the chat says so.
  String? get waitingFor => group.isEmpty ? null : e2ee.waitingFor[group];

  /// What it waits for is a reinstalled device (row 202): its name.
  String? get reinstalling => group.isEmpty ? null : e2ee.reinstalling[group];

  /// The friend's security key changed and the person hasn't seen it:
  /// nothing is sent to them until they have (Fugo 2b).
  bool get keyChanged => e2ee.pin(friend)?.changedUnseen ?? false;

  /// The friend's key now is one the person said didn't match before
  /// (Verify's The numbers don't match, E11-3): its note says so.
  bool get keyMismatched => e2ee.pin(friend)?.mismatched ?? false;

  /// The key the service gives for the friend now is one this device
  /// refused (E11-3): the chat says their key changed.
  bool get keyRefused => e2ee.keyRefused.contains(friend);

  /// The feed waits at a commit under a security key this device doesn't
  /// have yet: nothing after it is read until it does.
  bool get held => group.isNotEmpty && e2ee.held.containsKey(group);

  /// The feed waits at a message from past this device's place in the
  /// chat, until what it missed comes (row 551): nothing is passed over
  /// unread meanwhile.
  bool get behind => group.isNotEmpty && e2ee.behind.containsKey(group);

  /// Held a day or more: Join again is offered (Fugo).
  bool get heldLong => held && e2ee.heldLong(group);

  /// Joins the conversation again, past what it waits at.
  Future<void> rejoin() => e2ee.rejoinHeld(group);

  /// The person compared safety numbers with the friend, for this key.
  bool get verified => e2ee.pin(friend)?.verified ?? false;

  var _closed = false;
  StreamSubscription<String>? _moves;
  StreamSubscription<Map<String, Object?>>? _events;
  Timer? _reading;
  DateTime? _lastRead;

  /// The service's snapshot for this conversation, its group, and the
  /// newest page of this device's history; the draft into the box.
  Future<void> load() async {
    try {
      final snapshot = await api.snapshot();
      final here = snapshot.conversations
          .where((each) => each.id == conversation.id)
          .firstOrNull;
      if (here != null) conversation = here;
      await _findGroup();
      // Opened from their Claktories' channels, which the engine knows by
      // now.
      unawaited(_proposals(snapshot.proposals));
      unawaited(_readAgentDrafts());
      _fresh = true;
      await _refresh();
      loaded = true;
      problem = null;
    } on UserAppError catch (error) {
      problem = error;
    }
    _changed();
  }

  /// The conversation's group: as the worker lists it, or made now by this
  /// device when there's none and it can still be written to.
  Future<void> _findGroup() async {
    if (e2ee.setup != DmSetup.ready) return;
    final known = e2ee.groups.values
        .where(
          (each) =>
              each.kind == 'conversation' &&
              each.conversationId == conversation.id &&
              each.generation == conversation.generation,
        )
        .firstOrNull;
    friend =
        known?.accounts.where((each) => each != e2ee.accountId).firstOrNull ??
        conversation.person.accountId;
    if (known != null) {
      group = known.groupId;
      return;
    }
    if (!conversation.active || friend.isEmpty) return;
    final made = await e2ee.conversationGroup(
      conversation.id,
      conversation.generation,
      friend,
    );
    group = made?.groupId ?? '';
  }

  /// This device's history and view of the group, and what waits to go.
  Future<void> _read() async {
    if (group.isEmpty) return;
    final fresh = _fresh;
    _fresh = false;
    final me = e2ee.accountId;
    final count = math.max(_page, messages.length);
    final page = await e2ee.history(group, limit: count + 1);
    hasEarlier = page.length > count;
    messages
      ..clear()
      ..addAll([
        for (final kept in page.skip(hasEarlier ? 1 : 0))
          DmMessage.from(kept, me),
      ]);
    final view = await e2ee.view(group);
    final draftMoved =
        fresh || view.draftMs != _view.draftMs || view.draft != _view.draft;
    _view = view;
    draft = view.draft;
    if (draftMoved) onDraft?.call(view.draft);
    await _readOutgoing();
    _markRead();
  }

  /// What the person wrote here that hasn't come back: kept encrypted on
  /// this device, with where it stands; and what was refused.
  Future<void> _readOutgoing() async {
    final waiting = await e2ee.unsent(group);
    outgoing
      ..clear()
      ..addAll([
        for (final unsent in waiting)
          if (unsent.content.kind == 'text')
            OutgoingMessage(
              clientId: unsent.clientMessageId,
              text: unsent.content.text ?? '',
              agentName: unsent.content.byAgent?.name ?? '',
              state: switch (e2ee.sending[unsent.clientMessageId]?.state) {
                DmSendState.unknown => OutgoingState.unknown,
                DmSendState.waiting => OutgoingState.waiting,
                _ => OutgoingState.sending,
              },
              problem: e2ee.sending[unsent.clientMessageId]?.problem ?? '',
            ),
        for (final MapEntry(:key, :value) in e2ee.sending.entries)
          if (value.group == group && value.state == DmSendState.refused)
            OutgoingMessage(
              clientId: key,
              text: value.text,
              state: OutgoingState.refused,
              problem: value.problem,
            ),
      ]);
    if (outgoing.any((each) => each.problem == 'closed')) ended = true;
  }

  /// Keeps up until [close]: with the feeds [e2ee] takes in, and the
  /// service's changes to this conversation, its proposals and drafts.
  Future<void> follow() async {
    _moves ??= e2ee.groupChanges.listen((moved) {
      if (moved == group) {
        unawaited(_refresh());
      } else if (arriving > 0 && e2ee.groups[moved]?.kind == 'channel') {
        // A Claktory's channel moved: what was on its way may be here.
        unawaited(_arrived());
      }
    });
    _events ??= e2ee.events.listen((event) => unawaited(_apply(event)));
    e2ee.addListener(_engineChanged);
    await load();
  }

  void _engineChanged() {
    // Set up, or the groups came: this chat finds its own.
    if (group.isEmpty && e2ee.setup == DmSetup.ready && !_closed) {
      unawaited(load());
    } else {
      _changed();
    }
  }

  Future<void>? _refreshing;

  /// The next read is the first since loading: the draft goes in the box.
  var _fresh = false;
  var _refreshAgain = false;

  /// Reads the history again, one read at a time: a change meanwhile reads
  /// once more after it.
  Future<void> _refresh() {
    if (_refreshing case final running?) {
      _refreshAgain = true;
      return running;
    }
    return _refreshing = _refreshLoop().whenComplete(() => _refreshing = null);
  }

  Future<void> _refreshLoop() async {
    do {
      _refreshAgain = false;
      if (_closed) return;
      try {
        await _read();
      } on Object catch (error) {
        // The store couldn't be read: the next change reads it again.
        debugPrint('people: $error');
      }
      _changed();
    } while (_refreshAgain);
  }

  /// Stops following; nothing after this changes it.
  void close() {
    _closed = true;
    _reading?.cancel();
    unawaited(_moves?.cancel());
    unawaited(_events?.cancel());
    e2ee.removeListener(_engineChanged);
  }

  Future<void> _apply(Map<String, Object?> event) async {
    switch (event['kind']) {
      case 'resync' || 'conversation':
        try {
          final now = (await api.conversations())
              .where((each) => each.id == conversation.id)
              .firstOrNull;
          if (now != null) conversation = now;
          if (group.isEmpty) await _findGroup();
        } on UserAppError {
          // The next change reads it again.
        }
        if (event['kind'] == 'resync') {
          await _readProposals();
          await _readAgentDrafts();
        }
      case 'proposal':
        await _readProposals();
      case 'agent_draft' when event['conversation_id'] == conversation.id:
        await _readAgentDrafts();
    }
    _changed();
  }

  /// The pending ones of [all] that are this conversation's, each as
  /// this device opened it; one that doesn't open (or names another
  /// conversation inside) isn't shown.
  Future<void> _proposals(List<DmProposal> all) async {
    _listed = all;
    final opened = <DmProposal>[];
    var waiting = 0;
    for (final proposal in all) {
      if (!proposal.pending || proposal.conversationId != conversation.id) {
        continue;
      }
      if (await e2ee.openProposal(proposal) case final open?) {
        opened.add(open);
      } else if (await _onItsWay(
        proposal.envelope,
        proposal.claktoryId,
        proposal.proposalDigest,
      )) {
        waiting++;
      }
    }
    proposals
      ..clear()
      ..addAll(opened)
      ..sort((a, b) => a.createdMs.compareTo(b.createdMs));
    _proposalsArriving = waiting;
    arriving = _proposalsArriving + _draftsArriving;
    _changed();
  }

  /// Whether a record with no message here yet names one its Claktory is
  /// still sending through the home (#451); one that came and didn't open
  /// isn't shown at all.
  Future<bool> _onItsWay(
    String envelope,
    String claktory,
    String digest,
  ) async =>
      envelope.isEmpty &&
      digest.isNotEmpty &&
      !await e2ee.sideArrived(claktory, digest);

  /// A channel moved while something was on its way: what's listed is
  /// opened again.
  Future<void> _arrived() async {
    if (_proposalsArriving > 0) await _proposals(_listed);
    if (_draftsArriving > 0) await _readAgentDrafts();
  }

  Future<void> _readProposals() async {
    try {
      await _proposals(await api.proposals());
    } on UserAppError {
      // The next change reads them again.
    }
  }

  Future<void> _readAgentDrafts() async {
    try {
      final drafts = await api.agentDrafts(conversation.id);
      final opened = <DmAgentDraft>[];
      var waiting = 0;
      for (final draft in drafts) {
        if (await e2ee.openAgentDraft(draft, conversation.id)
            case final open?) {
          opened.add(open);
        } else if (await _onItsWay(
          draft.envelope,
          draft.claktoryId,
          draft.draftDigest,
        )) {
          waiting++;
        }
      }
      agentDrafts
        ..clear()
        ..addAll(opened);
      _draftsArriving = waiting;
      arriving = _proposalsArriving + _draftsArriving;
      _changed();
    } on UserAppError {
      // The next change reads them again.
    }
  }

  /// Sends [proposal] as its agent's message, exactly as shown: approved
  /// from this device with its own hash of it. One that changed since is
  /// shown again to be read again; one that can't go any more goes away.
  Future<void> approve(DmProposal proposal) async {
    if (!canSend || !deciding.add(proposal.id)) return;
    reviewProblem = null;
    reviewAgent = proposal.agentName;
    _changed();
    final group = this.group;
    try {
      // Approved first; then sealed here and sent through the home, kept
      // until it took it (#451).
      await api.approve(proposal);
      proposals.removeWhere((each) => each.id == proposal.id);
      _changed();
      await e2ee.sendApproved(group, proposal);
      await _refresh();
    } on UserAppError catch (error) {
      reviewProblem = switch (error.problem) {
        UserAppProblem.stale => 'changed',
        UserAppProblem.gone => 'unsendable',
        _ => error.problem.name,
      };
      if (error.problem == UserAppProblem.gone) {
        proposals.removeWhere((each) => each.id == proposal.id);
      }
      await _readProposals();
    } catch (error) {
      // It can't be sealed (the friend's key changed and isn't seen yet).
      reviewProblem = 'unencryptable';
    } finally {
      deciding.remove(proposal.id);
    }
    _changed();
  }

  /// Declines [proposal]: it's never sent.
  Future<void> decline(DmProposal proposal) async {
    if (!deciding.add(proposal.id)) return;
    reviewProblem = null;
    _changed();
    try {
      await api.decline(proposal);
      proposals.removeWhere((each) => each.id == proposal.id);
    } on UserAppError catch (error) {
      if (error.problem == UserAppProblem.gone) {
        proposals.removeWhere((each) => each.id == proposal.id);
      } else {
        reviewProblem = error.problem.name;
      }
      await _readProposals();
    } finally {
      deciding.remove(proposal.id);
    }
    _changed();
  }

  /// Puts [proposal]'s text in the person's box instead, to send as their
  /// own if they choose; the proposal is declined, so it can't go as well.
  Future<void> useProposal(DmProposal proposal) async {
    if (deciding.contains(proposal.id)) return;
    onUse?.call(proposal.text);
    await decline(proposal);
  }

  /// Puts [draft]'s text in the person's box, and closes it.
  Future<void> useAgentDraft(DmAgentDraft draft) async {
    onUse?.call(draft.text);
    await dismissAgentDraft(draft);
  }

  /// Closes [draft] unused.
  Future<void> dismissAgentDraft(DmAgentDraft draft) async {
    if (!deciding.add(draft.id)) return;
    _changed();
    try {
      await api.closeAgentDraft(conversation.id, draft.id);
      agentDrafts.removeWhere((each) => each.id == draft.id);
    } on UserAppError catch (error) {
      if (error.problem == UserAppProblem.notFound ||
          error.problem == UserAppProblem.gone) {
        agentDrafts.removeWhere((each) => each.id == draft.id);
      } else {
        reviewProblem = error.problem.name;
      }
    } finally {
      deciding.remove(draft.id);
    }
    _changed();
  }

  /// The page before what's here.
  Future<void> loadEarlier() async {
    if (messages.isEmpty || group.isEmpty) return load();
    final page = await e2ee.history(
      group,
      before: messages.first.seq,
      limit: _page + 1,
    );
    hasEarlier = page.length > _page;
    messages.insertAll(0, [
      for (final kept in page.skip(hasEarlier ? 1 : 0))
        DmMessage.from(kept, e2ee.accountId),
    ]);
    _changed();
  }

  /// Read through the newest here, on all the person's devices, at most
  /// once every [readEvery]. Only theirs: the other person never sees it.
  void _markRead() {
    if (messages.isEmpty || _reading != null) return;
    final wait = _lastRead == null
        ? Duration.zero
        : readEvery - DateTime.now().difference(_lastRead!);
    _reading = Timer(wait.isNegative ? Duration.zero : wait, () {
      _reading = null;
      if (_closed || messages.isEmpty) return;
      final last = messages.last.seq;
      if (last <= _view.readSeq) return;
      _lastRead = DateTime.now();
      unawaited(e2ee.tellMyDevices('read', group, seq: last));
    });
  }

  /// Keeps [text] as the draft, for the person's other devices.
  Future<void> saveDraft(String text) async {
    if (!canSend || text == draft) return;
    draft = text;
    await e2ee.tellMyDevices('draft', group, text: text);
    _changed();
  }

  /// Deletes the person's copy of everything here, on all their devices,
  /// for good; the other person keeps theirs.
  Future<void> deleteHistory() async {
    if (group.isEmpty || messages.isEmpty) return;
    final through = messages.last.seq;
    try {
      await e2ee.tellMyDevices('delete', group, seq: through);
      await api.hide(conversation.id, through);
      problem = null;
    } on UserAppError catch (error) {
      problem = error;
    }
    await _refresh();
  }

  /// Sends [text] (trimmed): Sending… until it comes back in the feed.
  Future<void> send(String text) async {
    final trimmed = text.trim();
    if (trimmed.isEmpty || !canSend) return;
    final id = _newId();
    outgoing.add(OutgoingMessage(clientId: id, text: trimmed));
    _changed();
    await e2ee.say(
      group,
      dm.DmContent(
        kind: 'text',
        clientMessageId: id,
        text: trimmed,
        extraJson: '',
      ),
    );
    await _refresh();
  }

  /// Tries [message] again, as the same message.
  Future<void> retry(OutgoingMessage message) async {
    if (!outgoing.contains(message) || !canSend) return;
    message.state = OutgoingState.sending;
    _changed();
    await e2ee.retry(message.clientId);
    await _refresh();
  }

  /// Takes [message] out (it was refused, or the person gives up on it;
  /// one in an unknown state may still have been sent, and then shows
  /// with the history).
  Future<void> discard(OutgoingMessage message) async {
    outgoing.remove(message);
    _changed();
    await e2ee.discard(group, message.clientId);
  }

  /// Where the person's own [message] stands past their home (#451), and
  /// for [DmDelivered.notYet] since when the friend's Claktory has been
  /// out of reach (0 when it isn't known).
  ({DmDelivered state, int sinceMs}) delivered(DmMessage message) {
    final id = message.clientMessageId;
    if (id.isNotEmpty &&
        messages.any((each) => each.mine && each.resends == id)) {
      return (state: DmDelivered.resent, sinceMs: 0);
    }
    final delivery = id.isEmpty ? null : e2ee.delivery(id);
    switch (delivery?.state) {
      case DmDeliveryState.DM_DELIVERY_STATE_DELIVERED:
        return (state: DmDelivered.delivered, sinceMs: 0);
      case DmDeliveryState.DM_DELIVERY_STATE_BOUNCED:
        if (e2ee.askResend.contains(id)) {
          return (state: DmDelivered.askResend, sinceMs: 0);
        }
        // Only the device that sent it sends it again (Fugo).
        return (
          state: message.fromLeaf == e2ee.leafId
              ? DmDelivered.resending
              : DmDelivered.bounced,
          sinceMs: 0,
        );
      default:
        // Not delivered yet: the friend's Claktory out of reach, as the
        // home says of it (Mista 32614: not a delivery of its own). Only
        // the home's word: another Claktory of the person's doesn't send
        // (Zucchero's row 567).
        final home = e2ee.homeIsHere ? e2ee.homeState : null;
        // Sent before this Claktory became the home (row 595): an earlier
        // home sent it on, and only that one knew whether it arrived, so
        // it's said as sent, never as not delivered (Mista 41960).
        final since = home?.homeSinceMs.toInt() ?? 0;
        if (since > 0 && message.shownMs < since) {
          return (state: DmDelivered.sent, sinceMs: 0);
        }
        final peer = home?.peers
            .where((each) => each.accountId == friend)
            .firstOrNull;
        // Only what came after it was last reachable can't have reached
        // it: one before may have, before a restart lost its delivery.
        final last = peer?.lastReachableMs.toInt() ?? 0;
        if (peer != null &&
            !peer.reachable &&
            (last == 0 || message.acceptedMs > last)) {
          return (
            state: peer.refused == dmTurnedAway
                ? DmDelivered.notTaking
                : DmDelivered.notYet,
            sinceMs: peer.lastReachableMs.toInt(),
          );
        }
        return (state: DmDelivered.sent, sinceMs: 0);
    }
  }

  /// The person says yes to sending [message] again, after the friend's
  /// Claktory couldn't read it.
  Future<void> resend(DmMessage message) async {
    await e2ee.resend(group, message.clientMessageId);
    await _refresh();
  }

  /// The person saw that the friend's security key changed: sending goes
  /// on.
  Future<void> keySeen() async {
    await e2ee.seen(friend);
    _changed();
  }

  void _changed() {
    // Ended (unfriended, blocked): nothing waiting can be sent or used
    // here anymore, and the service ends it too, so its cards go.
    if (loaded && !writable) {
      proposals.clear();
      agentDrafts.clear();
      arriving = _proposalsArriving = _draftsArriving = 0;
    }
    if (!_closed) notifyListeners();
  }

  @override
  void dispose() {
    close();
    super.dispose();
  }
}

/// How many messages a page of history holds.
const _page = 50;

String _text(Map<String, Object?> json, String key) => switch (json[key]) {
  final String text => text,
  _ => '',
};

int _int(Map<String, Object?> json, String key) => switch (json[key]) {
  final int number => number,
  _ => 0,
};

List<Map<String, Object?>> _list(Map<String, Object?> json, String key) => [
  for (final item in json[key] as List? ?? const [])
    if (item is Map<String, Object?>) item,
];
