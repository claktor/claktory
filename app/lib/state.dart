import 'package:flutter/foundation.dart';

import 'protocol/factory/v1/factory.pb.dart';

/// The latest workspace snapshot from the backend. The backend sends a full
/// snapshot on every change, so this only ever replaces it.
class WorkspaceStore extends ChangeNotifier {
  Workspace _workspace = Workspace();

  Workspace get workspace => _workspace;

  /// The backend has sent the workspace at least once; until then it's empty
  /// because nothing is known yet.
  bool get received => _received;
  var _received = false;

  void apply(Workspace workspace) {
    _workspace = workspace;
    _received = true;
    notifyListeners();
  }

  /// Forgets the workspace, as for another Claktory: empty again, and not
  /// [received] until that one's arrives, so nothing takes "not known yet"
  /// for "has nothing".
  void clear() {
    _workspace = Workspace();
    _received = false;
    notifyListeners();
  }

  Agent? get selectedAgent => agent(_workspace.selectedAgentId);

  Agent? agent(String id) {
    for (final agent in _workspace.agents) {
      if (agent.id == id) return agent;
    }
    return null;
  }

  Project? project(String id) {
    for (final project in _workspace.projects) {
      if (project.id == id) return project;
    }
    return null;
  }

  Model? model(String id) {
    for (final model in _workspace.models) {
      if (model.id == id) return model;
    }
    return null;
  }

  List<Agent> agentsIn(String projectId) {
    final agents = <Agent>[];
    for (final agent in _workspace.agents) {
      if (agent.projectId == projectId) agents.add(agent);
    }
    return agents;
  }
}

/// The latest file tree the backend sent for each project whose Files are
/// open (a Files pane, the phone's Files page, or the focused project).
/// Each replaces the last for its project.
class ExplorerStore extends ChangeNotifier {
  final _explorers = <String, Explorer>{};

  void apply(Explorer explorer) {
    _explorers[explorer.projectId] = explorer;
    notifyListeners();
  }

  /// The tree for [projectId], or null until one arrives for it.
  Explorer? of(String projectId) => _explorers[projectId];
}

/// The latest content the backend sent for the open extension panel. Each
/// one replaces the last; views use it only while it's for the panel and
/// agent they show.
class ExtensionViewStore extends ChangeNotifier {
  ExtensionView? _view;

  /// The view the backend sent last, whichever it is.
  ExtensionView? get view => _view;

  void apply(ExtensionView view) {
    _view = view;
    notifyListeners();
  }

  /// The view of [extensionId] for [agentId], or null until one arrives.
  ExtensionView? of(String extensionId, String agentId) {
    final view = _view;
    if (view == null ||
        view.extensionId != extensionId ||
        view.agentId != agentId) {
      return null;
    }
    return view;
  }
}

/// The messages waiting for the selected agent, oldest first, as the
/// backend last sent them. Each list replaces the last.
class QueuedStore extends ChangeNotifier {
  QueuedMessages _queued = QueuedMessages();

  void apply(QueuedMessages queued) {
    _queued = queued;
    notifyListeners();
  }

  /// What's waiting for [agentId]; empty when nothing is, or the backend
  /// hasn't said for this agent yet.
  List<QueuedMessage> of(String agentId) =>
      _queued.agentId == agentId ? _queued.messages : const [];
}

/// One transcript entry. It notifies only its own widget, so streamed text
/// rebuilds one entry rather than the whole transcript.
class EntryModel extends ChangeNotifier {
  EntryModel(this._entry);

  Entry _entry;

  /// Streamed text not yet folded into [_entry]. Deltas arrive many times per
  /// frame; folding once per read keeps appends linear.
  StringBuffer? _pending;

  Entry get entry {
    _fold();
    return _entry;
  }

  int get seq => _entry.seq.toInt();

  void replace(Entry entry) {
    _entry = entry;
    _pending = null;
    notifyListeners();
  }

  /// Appends streamed text to an assistant or thinking entry. Returns false
  /// for entries that don't stream text.
  bool append(String text) {
    if (!_entry.hasAssistant() && !_entry.hasThinking()) return false;
    (_pending ??= StringBuffer()).write(text);
    notifyListeners();
    return true;
  }

  void _fold() {
    final pending = _pending;
    if (pending == null) return;
    _pending = null;
    if (_entry.hasAssistant()) {
      _entry.assistant.text += pending.toString();
    } else {
      _entry.thinking.text += pending.toString();
    }
  }
}

/// The selected agent's transcript.
///
/// Listeners hear when the list of entries changes; each [EntryModel]
/// notifies its own content changes. [revision] ticks on any change at all,
/// for keeping the view pinned to the newest entry.
class TranscriptStore extends ChangeNotifier {
  String _agentId = '';
  var _loads = 0;
  final List<EntryModel> _entries = [];
  final Map<int, EntryModel> _bySeq = {};
  final revision = ValueNotifier<int>(0);

  /// The agent this transcript belongs to; empty before the first one loads.
  String get agentId => _agentId;

  /// How many times a whole transcript has loaded. Views key on it to reset.
  int get loads => _loads;

  int get length => _entries.length;

  EntryModel operator [](int index) => _entries[index];

  /// Whether the conversation has entries before the first one held: the
  /// backend sends only the latest, and earlier ones load as the person
  /// scrolls up.
  bool get hasEarlier => _hasEarlier;
  var _hasEarlier = false;

  /// How many entries have been put before the first one held since the
  /// load, so views can tell them from new ones and keep their place.
  int get earlier => _earlier;
  var _earlier = 0;

  /// The revision of this conversation last applied
  /// (ServerMessage.transcript_revision), sent back in Hello so a
  /// reconnect brings only what changed since; empty when none is
  /// held (an older Claktory, or nothing loaded).
  String get heldRevision => _heldRevision;
  var _heldRevision = '';

  /// Takes [transcript], whole or, [Transcript.resumed], only what changed
  /// since [heldRevision]; [revision] is where it brings the conversation.
  void load(Transcript transcript, {String revision = ''}) {
    if (transcript.resumed && transcript.agentId == _agentId) {
      // Only what was added or changed: in place, the rest kept as it is,
      // has-earlier too.
      _heldRevision = revision;
      _merge(transcript);
      return;
    }
    // Resumed for another conversation than the one held can't be put
    // anywhere: what it brings shows, and the next reconnect asks whole.
    _heldRevision = transcript.resumed ? '' : revision;
    if (_sameConversation(transcript)) {
      _merge(transcript);
      return;
    }
    for (final entry in _entries) {
      entry.dispose();
    }
    _agentId = transcript.agentId;
    _loads++;
    _hasEarlier = transcript.hasEarlier;
    _earlier = 0;
    _entries.clear();
    _bySeq.clear();
    for (final entry in transcript.entries) {
      _insert(EntryModel(entry));
    }
    _changed(listChanged: true);
  }

  /// Whether [transcript] is the conversation already held, sent again
  /// (the agent selected again, its project focused, a reconnect): its
  /// first entry is one held, unchanged in time and kind, and it ends no
  /// sooner than what's held. A cleared conversation starts over at other
  /// entries, so it loads afresh.
  bool _sameConversation(Transcript transcript) {
    if (transcript.agentId != _agentId ||
        _entries.isEmpty ||
        transcript.entries.isEmpty) {
      return false;
    }
    final first = transcript.entries.first;
    final held = _bySeq[first.seq.toInt()];
    return held != null &&
        held.entry.timestampMs == first.timestampMs &&
        held.entry.whichKind() == first.whichKind() &&
        transcript.entries.last.seq.toInt() >= _entries.last.seq;
  }

  /// Takes [transcript], the conversation held sent again, in place: what
  /// changed meanwhile is replaced, what's new added, and the entries paged
  /// in before it are kept, so views keep their place.
  void _merge(Transcript transcript) {
    var added = false;
    for (final entry in transcript.entries) {
      final existing = _bySeq[entry.seq.toInt()];
      if (existing == null) {
        _insert(EntryModel(entry));
        added = true;
      } else if (existing.entry != entry) {
        existing.replace(entry);
      }
    }
    _changed(listChanged: added);
  }

  /// Puts [entries], earlier ones the backend sent for [agentId], before
  /// the first one held; [hasEarlier] says whether more remain.
  void prepend(
    String agentId,
    List<Entry> entries, {
    required bool hasEarlier,
  }) {
    if (agentId != _agentId) return;
    final first = _entries.isEmpty ? null : _entries.first.seq;
    final added = [
      for (final entry in entries)
        if ((first == null || entry.seq.toInt() < first) &&
            !_bySeq.containsKey(entry.seq.toInt()))
          EntryModel(entry),
    ]..sort((a, b) => a.seq.compareTo(b.seq));
    for (final entry in added) {
      _bySeq[entry.seq] = entry;
    }
    _entries.insertAll(0, added);
    _earlier += added.length;
    _hasEarlier = hasEarlier;
    _changed(listChanged: true);
  }

  void update(EntryUpdate update, {String revision = ''}) {
    if (update.agentId != _agentId) return;
    _heldRevision = revision;
    final seq = update.entry.seq.toInt();
    final existing = _bySeq[seq];
    if (existing != null) {
      existing.replace(update.entry);
      _changed(listChanged: false);
      return;
    }
    // Older than what's held: it comes as it is now when paged in.
    if (_hasEarlier && _entries.isNotEmpty && seq < _entries.first.seq) return;
    _insert(EntryModel(update.entry));
    _changed(listChanged: true);
  }

  void delta(EntryDelta delta, {String revision = ''}) {
    if (delta.agentId != _agentId) return;
    _heldRevision = revision;
    final entry = _bySeq[delta.seq.toInt()];
    if (entry != null && entry.append(delta.text)) {
      _changed(listChanged: false);
    }
  }

  /// Keeps entries ordered by `seq`. They normally arrive in order, so this
  /// is an append.
  void _insert(EntryModel entry) {
    _bySeq[entry.seq] = entry;
    var index = _entries.length;
    while (index > 0 && _entries[index - 1].seq > entry.seq) {
      index--;
    }
    _entries.insert(index, entry);
  }

  void _changed({required bool listChanged}) {
    revision.value++;
    if (listChanged) notifyListeners();
  }

  @override
  void dispose() {
    for (final entry in _entries) {
      entry.dispose();
    }
    revision.dispose();
    super.dispose();
  }
}

/// Whether [agent] is an admin agent (Messaging's admin option on), such as
/// Rivet: it sees and messages every agent.
bool isAdminAgent(Agent agent) => agent.extensions.any(
  (extension) =>
      extension.id == 'agents' &&
      extension.enabled &&
      extension.options['admin']?.on == true,
);

/// What to call the group of [agent], which is in no project: "Admin" for
/// an admin agent, "Workspace" for others.
String outsideProjectsLabel(Agent agent) =>
    isAdminAgent(agent) ? 'Admin' : 'Workspace';

/// The selected pipe's messages, oldest first: the backend sends the latest
/// when a pipe is selected (PipeTranscript), each new or changed one after
/// (PipeMessageUpdate), and earlier ones on request.
class PipeChat extends ChangeNotifier {
  String _pipeId = '';
  final List<PipeMessage> _messages = [];
  var _hasEarlier = false;
  var _loads = 0;

  /// The pipe these are of; empty before one is selected.
  String get pipeId => _pipeId;
  List<PipeMessage> get messages => List.unmodifiable(_messages);
  bool get hasEarlier => _hasEarlier;

  /// How many times a pipe's messages have loaded; views key on it.
  int get loads => _loads;

  void load(PipeTranscript transcript) {
    _pipeId = transcript.pipeId;
    _hasEarlier = transcript.hasEarlier;
    _loads++;
    _messages
      ..clear()
      ..addAll(transcript.messages)
      ..sort((a, b) => a.seq.compareTo(b.seq));
    notifyListeners();
  }

  void update(PipeMessageUpdate update) {
    if (update.pipeId != _pipeId) return;
    final message = update.message;
    final at = _messages.indexWhere((m) => m.seq == message.seq);
    if (at >= 0) {
      _messages[at] = message;
    } else {
      var index = _messages.length;
      while (index > 0 && _messages[index - 1].seq > message.seq) {
        index--;
      }
      _messages.insert(index, message);
    }
    notifyListeners();
  }

  /// Earlier messages, before the first one held.
  void prepend(PipeMessagePage page) {
    if (page.pipeId != _pipeId) return;
    final first = _messages.isEmpty ? null : _messages.first.seq;
    final earlier = [
      for (final message in page.messages)
        if (first == null || message.seq < first) message,
    ]..sort((a, b) => a.seq.compareTo(b.seq));
    _messages.insertAll(0, earlier);
    _hasEarlier = page.hasEarlier;
    notifyListeners();
  }
}
