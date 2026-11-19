import 'package:flutter/foundation.dart';

import 'state.dart';

/// Where the person came from as they follow agents' messages from one
/// conversation to the next, so they can go back (and forward again), as in
/// a browser. It's transient: this window's, in memory, never kept or sent
/// to the backend, like hover. Picking an agent, project or page in the
/// sidebar starts over.
class AgentHistory extends ChangeNotifier {
  AgentHistory({required this.workspace, required this.select});

  final WorkspaceStore workspace;

  /// Shows an agent (SelectAgent).
  final void Function(String agentId) select;

  final _back = <String>[];
  final _forward = <String>[];

  /// The agent Back returns to, skipping any deleted meanwhile; null when
  /// there's none.
  String? get back => _live(_back);

  /// The agent Forward goes to again, if any.
  String? get forward => _live(_forward);

  String? _live(List<String> stack) {
    for (final id in stack.reversed) {
      if (workspace.agent(id) != null) return id;
    }
    return null;
  }

  /// Follows a link from [from]'s conversation to [to]'s.
  void follow(String from, String to) {
    if (from == to) return;
    _back.add(from);
    _forward.clear();
    select(to);
    notifyListeners();
  }

  /// Back from [current] to where it came from.
  void goBack(String current) => _step(_back, _forward, current);

  /// Forward from [current] again.
  void goForward(String current) => _step(_forward, _back, current);

  void _step(List<String> from, List<String> to, String current) {
    while (from.isNotEmpty) {
      final id = from.removeLast();
      if (workspace.agent(id) == null) continue;
      to.add(current);
      select(id);
      notifyListeners();
      return;
    }
    notifyListeners();
  }

  /// Back to plain viewing: the person picked something themselves.
  void clear() {
    if (_back.isEmpty && _forward.isEmpty) return;
    _back.clear();
    _forward.clear();
    notifyListeners();
  }
}
