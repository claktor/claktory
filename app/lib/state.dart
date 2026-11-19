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

/// The latest file tree the backend sent for the focused project. Each one
/// replaces the last; views use it only while its project is the focused one.
class ExplorerStore extends ChangeNotifier {
  Explorer? _explorer;

  void apply(Explorer explorer) {
    _explorer = explorer;
    notifyListeners();
  }

  /// The tree for [projectId], or null until one arrives for it.
  Explorer? of(String projectId) {
    final explorer = _explorer;
    if (explorer == null || explorer.projectId != projectId) return null;
    return explorer;
  }
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

  void load(Transcript transcript) {
    for (final entry in _entries) {
      entry.dispose();
    }
    _agentId = transcript.agentId;
    _loads++;
    _entries.clear();
    _bySeq.clear();
    for (final entry in transcript.entries) {
      _insert(EntryModel(entry));
    }
    _changed(listChanged: true);
  }

  void update(EntryUpdate update) {
    if (update.agentId != _agentId) return;
    final existing = _bySeq[update.entry.seq.toInt()];
    if (existing != null) {
      existing.replace(update.entry);
      _changed(listChanged: false);
      return;
    }
    _insert(EntryModel(update.entry));
    _changed(listChanged: true);
  }

  void delta(EntryDelta delta) {
    if (delta.agentId != _agentId) return;
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
