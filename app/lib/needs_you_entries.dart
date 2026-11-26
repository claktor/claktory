// What Needs you's items are about, as fetched: each waiting card's entry
// (an agent's or a chat's) and each held or unanswered pipe message, fetched
// as it is now, one store per Claktory, so the Needs you page and focus mode
// (#514) read the same copies and never fetch twice. The app keeps nothing
// of it beyond this cache: what waits is the backend's (needsYouItems), and
// the open conversation's own live copy is preferred to what was fetched.
import 'dart:async';

import 'package:flutter/foundation.dart';

import 'backend.dart';
import 'inbox.dart'
    show
        NeedsYouItem,
        NeedsYouKind,
        browserSessionOf,
        dmSendsOf,
        needsYouItems,
        pendingDeviceOf;
import 'ask_card.dart' show waitingSince;
import 'protocol/factory/v1/factory.pb.dart';
import 'state.dart' show TranscriptStore;

/// [backend]'s Needs you items' cards, as fetched ([of]): it fetches every
/// waiting item it doesn't hold yet as the workspace changes, while anyone
/// listens, and tells its listeners when what it holds changes, or the open
/// conversation's copies do.
class NeedsYouEntries extends ChangeNotifier {
  NeedsYouEntries._(this.backend);

  static final _stores = Expando<NeedsYouEntries>('NeedsYouEntries');

  /// [backend]'s store: one per Claktory, shared by whoever shows its items.
  static NeedsYouEntries of(Backend backend) =>
      _stores[backend] ??= NeedsYouEntries._(backend);

  final Backend backend;

  /// An alert whose notification was tapped (Notify.alert_id): Needs you
  /// shows it, in view and read, then clears this.
  final revealAlert = ValueNotifier<String?>(null);

  final _entries = <String, Entry>{};
  final _messages = <String, PipeMessage>{};

  /// Fetches under way, and those that failed (why).
  final _fetching = <String>{};
  final _failed = <String, String>{};

  /// Asked for again while their fetch was under way.
  final _again = <String, NeedsYouItem>{};

  /// Items shown though they may no longer wait ([hold]), how many times
  /// each: fetched once more as they stop waiting, to show how they ended.
  final _held = <String, ({NeedsYouItem item, int count})>{};

  /// What waited at the last workspace change.
  var _waiting = <String>{};

  var _listening = false;

  @override
  void addListener(VoidCallback listener) {
    super.addListener(listener);
    if (!_listening) {
      _listening = true;
      backend.workspace.addListener(_changed);
      backend.transcript.addListener(notifyListeners);
      backend.chatTranscript.addListener(notifyListeners);
      backend.pipeChat.addListener(notifyListeners);
      _changed();
    }
  }

  @override
  void removeListener(VoidCallback listener) {
    super.removeListener(listener);
    // No one shows its items: nothing to keep up to date meanwhile. What's
    // fetched stays, and the next listener brings it up to date.
    if (_listening && !hasListeners) {
      _listening = false;
      backend.workspace.removeListener(_changed);
      backend.transcript.removeListener(notifyListeners);
      backend.chatTranscript.removeListener(notifyListeners);
      backend.pipeChat.removeListener(notifyListeners);
    }
  }

  /// Keeps [item] up to date though it stops waiting, until [release]: it
  /// shows how it ended (answered here, on another device, withdrawn).
  void hold(NeedsYouItem item) {
    final held = _held[item.key];
    _held[item.key] = (item: item, count: (held?.count ?? 0) + 1);
  }

  /// Stops holding [key] ([hold]), once for each hold.
  void release(String key) {
    final held = _held[key];
    if (held == null) return;
    if (held.count <= 1) {
      _held.remove(key);
    } else {
      _held[key] = (item: held.item, count: held.count - 1);
    }
  }

  /// What waits changed, perhaps: fetch what's new, a procedure a step
  /// behind, and once more what's held that stopped waiting.
  void _changed() {
    final workspace = backend.workspace.workspace;
    final items = needsYouItems(workspace);
    final now = {for (final item in items) item.key};
    final agents = {for (final agent in workspace.agents) agent.id: agent};
    final chats = {for (final chat in workspace.chats) chat.id: chat};
    for (final item in items) {
      if (!_has(item) || _stepping(item, agents, chats)) fetch(item);
    }
    for (final (:item, count: _) in _held.values) {
      if (_waiting.contains(item.key) && !now.contains(item.key)) fetch(item);
    }
    _waiting = now;
    notifyListeners();
  }

  bool _has(NeedsYouItem item) => switch (item.kind) {
    NeedsYouKind.entry || NeedsYouKind.chat => _entries.containsKey(item.key),
    NeedsYouKind.pipeHeld ||
    NeedsYouKind.pipeUnanswered => _messages.containsKey(item.key),
    _ => true,
  };

  /// A procedure (ask_person's steps) that waits moves on a step at a time
  /// with no change to what waits: once its agent (or chat) changes in the
  /// Workspace (told of the step, done here or on another device), what was
  /// fetched of it may be a step behind, so it's fetched again (friend bug
  /// #11). Only then: a busy Claktory sends the Workspace a few times a
  /// second, and fetching every waiting procedure each time was most of what
  /// the app asked of it (Polpo's needs-you-refetch.md). Not when the open
  /// conversation holds it: that copy is live.
  bool _stepping(
    NeedsYouItem item,
    Map<String, Agent> agents,
    Map<String, Chat> chats,
  ) {
    final entry = _entries[item.key];
    if (entry == null ||
        !entry.hasAskPerson() ||
        entry.askPerson.steps.isEmpty ||
        entry.askPerson.state != AskPersonState.ASK_PERSON_STATE_PENDING) {
      return false;
    }
    if (!identical(entryOf(item), entry)) return false;
    final was = _fetchedUnder[item.key];
    final now = item.kind == NeedsYouKind.chat
        ? chats[item.chatId]
        : agents[item.agentId];
    // A patch keeps an unchanged agent's own object; a whole Workspace
    // brings a new one, the same or not.
    return !(identical(was, now) || was == now);
  }

  /// The agent (or chat) each fetched card's fetch started under, as the
  /// Workspace had it: [_stepping] fetches it again once that changes.
  final _fetchedUnder = <String, Object?>{};

  /// [item]'s agent, or chat, in the Workspace now.
  Object? _ownerOf(NeedsYouItem item) {
    final workspace = backend.workspace.workspace;
    if (item.kind == NeedsYouKind.chat) {
      return workspace.chats
          .where((chat) => chat.id == item.chatId)
          .firstOrNull;
    }
    return workspace.agents
        .where((agent) => agent.id == item.agentId)
        .firstOrNull;
  }

  /// Fetches [item]'s card as it is now (Try again, or opened); once more
  /// after a fetch already under way, since what that brings may be older
  /// than what changed meanwhile. Items with nothing to fetch (their state
  /// is the Workspace's) are left alone.
  void fetch(NeedsYouItem item) {
    final key = item.key;
    if (item.kind != NeedsYouKind.entry &&
        item.kind != NeedsYouKind.chat &&
        item.kind != NeedsYouKind.pipeHeld &&
        item.kind != NeedsYouKind.pipeUnanswered) {
      return;
    }
    if (!_fetching.add(key)) {
      _again[key] = item;
      return;
    }
    unawaited(() async {
      // Before the request: a step answered meanwhile changes its agent
      // after this, and so fetches it again.
      if (item.kind == NeedsYouKind.entry || item.kind == NeedsYouKind.chat) {
        _fetchedUnder[key] = _ownerOf(item);
      }
      try {
        if (item.kind == NeedsYouKind.entry) {
          final entry = await backend.fetchEntry(item.agentId, item.seq);
          if (entry != null) _entries[key] = entry;
        } else if (item.kind == NeedsYouKind.chat) {
          final entry = await backend.fetchChatEntry(item.chatId, item.seq);
          if (entry != null) _entries[key] = entry;
        } else {
          final message = await backend.fetchPipeMessage(item.pipeId, item.seq);
          if (message != null) _messages[key] = message;
        }
        _failed.remove(key);
      } on RequestFailure catch (failure) {
        _failed[key] = failure.message;
      } finally {
        _fetching.remove(key);
        final again = _again.remove(key);
        notifyListeners();
        if (again != null) fetch(again);
      }
    }());
  }

  /// [item]'s card as it is now: the open conversation's own copy when it
  /// holds it (it's live), else what was fetched; null until it's come.
  Entry? entryOf(NeedsYouItem item) {
    Entry? live(TranscriptStore transcript) {
      for (var i = transcript.length - 1; i >= 0; i--) {
        final entry = transcript[i].entry;
        if (entry.seq.toInt() == item.seq) return entry;
        if (entry.seq.toInt() < item.seq) break;
      }
      return null;
    }

    final chat = backend.chatTranscript;
    final held = item.kind == NeedsYouKind.chat
        ? (chat.chatId == item.chatId ? live(chat) : null)
        : (backend.transcript.agentId == item.agentId
              ? live(backend.transcript)
              : null);
    return held ?? _entries[item.key];
  }

  /// [item]'s pipe message as it is now: the open pipe's copy, else what
  /// was fetched.
  PipeMessage? messageOf(NeedsYouItem item) {
    final chat = backend.pipeChat;
    if (chat.pipeId == item.pipeId) {
      for (final message in chat.messages) {
        if (message.seq.toInt() == item.seq) return message;
      }
    }
    return _messages[item.key];
  }

  /// Why [item]'s card couldn't be fetched, when it couldn't.
  String? failedOf(NeedsYouItem item) => _failed[item.key];

  /// Whether [item]'s card is being fetched.
  bool fetching(NeedsYouItem item) => _fetching.contains(item.key);

  /// When [item] began waiting, in ms since 1970; 0 while unknown (a pipe's
  /// approval, its pause, or a card not fetched yet).
  int timeOf(NeedsYouItem item, Workspace workspace) => switch (item.kind) {
    // A card its agent replied on rises by its thread's last message.
    NeedsYouKind.entry || NeedsYouKind.chat => waitingSince(entryOf(item)),
    NeedsYouKind.pipeHeld ||
    NeedsYouKind.pipeUnanswered => messageOf(item)?.timestampMs.toInt() ?? 0,
    NeedsYouKind.browser =>
      browserSessionOf(workspace, item)?.createdMs.toInt() ?? 0,
    NeedsYouKind.dmSend =>
      dmSendsOf(workspace, item).firstOrNull?.createdMs.toInt() ?? 0,
    NeedsYouKind.device =>
      pendingDeviceOf(workspace, item)?.requestedMs.toInt() ?? 0,
    _ => 0,
  };
}
