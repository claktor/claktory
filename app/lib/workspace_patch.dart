import 'package:protobuf/protobuf.dart';

import 'protocol/factory/v1/factory.pb.dart';

/// The Workspace [patch] stands for, after [prev], the last one this
/// connection had (whole or patched): exactly what the backend would have
/// sent whole. The patch holds what changed: its `kept` fields are as they
/// were in [prev]; an agent in `kept_agents` (only its id in the patch) is
/// [prev]'s; one in `kept_agent_fields` takes those of its fields from
/// [prev]'s. Null when the patch doesn't fit [prev] (an agent kept that
/// [prev] doesn't have), so the connection starts again from a whole one.
///
/// What's taken from [prev] is shared with it, not copied: neither is ever
/// changed once received.
Workspace? patchWorkspace(Workspace prev, WorkspacePatch patch) {
  final next = patch.workspace;
  _keep(next, prev, patch.kept, except: _agentsField);
  if (patch.keptAgents.isEmpty && patch.keptAgentFields.isEmpty) return next;
  final before = {for (final agent in prev.agents) agent.id: agent};
  final whole = patch.keptAgents.toSet();
  final partly = {
    for (final fields in patch.keptAgentFields) fields.id: fields.kept,
  };
  for (var i = 0; i < next.agents.length; i++) {
    final agent = next.agents[i];
    if (whole.contains(agent.id)) {
      final was = before[agent.id];
      if (was == null) return null;
      next.agents[i] = was;
    } else if (partly[agent.id] case final kept?) {
      final was = before[agent.id];
      if (was == null) return null;
      _keep(agent, was, kept, except: _agentIdField);
    }
  }
  return next;
}

/// Copies the fields numbered in [kept] from [from] into [into], as they
/// are there (an unset one cleared), but never [except].
void _keep(
  GeneratedMessage into,
  GeneratedMessage from,
  List<int> kept, {
  required int except,
}) {
  final fields = into.info_.fieldInfo;
  for (final number in kept) {
    if (number == except) continue;
    // A field this app doesn't know (a newer backend's) it can't show.
    if (!fields.containsKey(number)) continue;
    final was = from.getField(number);
    if (was is List) {
      (into.getField(number) as List)
        ..clear()
        ..addAll(was);
    } else if (was is Map) {
      (into.getField(number) as Map)
        ..clear()
        ..addAll(was);
    } else if (from.hasField(number)) {
      into.setField(number, was);
    } else {
      into.clearField(number);
    }
  }
}

/// Workspace.agents' field number: never kept, its agents come one by one.
final _agentsField = Workspace().info_.byName['agents']!.tagNumber;

/// Agent.id's field number: never kept, it says which agent it is.
final _agentIdField = Agent().info_.byName['id']!.tagNumber;
