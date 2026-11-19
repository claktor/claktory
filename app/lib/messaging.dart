import 'package:flutter/widgets.dart';

import 'agent_name.dart';
import 'characters/character.dart';
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'state.dart';

/// Who [agentId] can message and who can message it, each agent with its
/// dot, name, project when it's another one's, and whether it's working
/// now, from the live workspace.
class MessagingView extends StatelessWidget {
  const MessagingView({
    super.key,
    required this.messaging,
    required this.agentId,
  });

  final Messaging messaging;
  final String agentId;

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        final agent = store.agent(agentId);
        final reachable =
            agent == null || _option(agent, 'agents', 'reachable') != false;
        return ListView(
          padding: const EdgeInsets.fromLTRB(
            Space.s,
            Space.s,
            Space.s,
            Space.l,
          ),
          children: [
            const _Heading('Can message'),
            if (messaging.reaches.isEmpty)
              const _Empty("It can't message any agent.")
            else
              for (final id in messaging.reaches)
                _AgentLine(id: id, store: store, home: agent?.projectId),
            const _Heading('Can be messaged by'),
            if (messaging.reachedBy.isEmpty)
              _Empty(
                reachable
                    ? 'No agent can message it.'
                    : 'No agent can message it: '
                          '"Other agents can message it" is off in its '
                          'settings.',
              )
            else
              for (final id in messaging.reachedBy)
                _AgentLine(id: id, store: store, home: agent?.projectId),
          ],
        );
      },
    );
  }
}

/// A switch option's value for [agent], or null when it has none.
bool? _option(Agent agent, String extension, String option) {
  for (final settings in agent.extensions) {
    if (settings.id == extension) return settings.options[option]?.on;
  }
  return null;
}

class _Heading extends StatelessWidget {
  const _Heading(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.fromLTRB(Space.s, Space.m, Space.s, Space.xs),
      child: Text(
        text,
        style: TextStyles.caption.copyWith(
          color: palette.textFaint,
          fontWeight: FontWeight.w500,
        ),
      ),
    );
  }
}

class _Empty extends StatelessWidget {
  const _Empty(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Padding(
    padding: const EdgeInsets.symmetric(
      horizontal: Space.s,
      vertical: Space.xs,
    ),
    child: Text(
      text,
      style: TextStyles.body.copyWith(color: Palette.of(context).textFaint),
    ),
  );
}

/// Wide enough for the longest status, "no model".
const _statusWidth = 60.0;

/// One agent: its dot (pulsing while it works), name, project when it isn't
/// [home], and what it's doing.
class _AgentLine extends StatelessWidget {
  const _AgentLine({required this.id, required this.store, required this.home});

  final String id;
  final WorkspaceStore store;

  /// The project of the agent whose panel this is.
  final String? home;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final agent = store.agent(id);
    final working = agent?.status == AgentStatus.AGENT_STATUS_WORKING;
    final String place;
    if (agent == null || agent.projectId == home) {
      place = '';
    } else if (agent.projectId.isEmpty) {
      place = outsideProjectsLabel(agent);
    } else {
      place = store.project(agent.projectId)?.name ?? agent.projectId;
    }
    final status = agent == null
        ? ''
        : !agent.modelAvailable
        ? 'no model'
        : working
        ? 'working'
        : 'idle';
    return Container(
      // Taller with a role, for its line under the name, as in the sidebar.
      height: (agent?.role ?? '').isEmpty ? 32 : 40,
      padding: const EdgeInsets.symmetric(horizontal: Space.s),
      child: Row(
        children: [
          Padding(
            padding: const EdgeInsets.only(right: Space.s - 2),
            child: agent == null
                ? const UnknownCharacter(size: 24)
                : AgentCharacter(agent: agent, size: 24, live: true),
          ),
          Expanded(
            child: NameOverRole(
              name: agent?.name ?? id,
              role: agent?.role ?? '',
              style: TextStyles.body.copyWith(
                color: agent == null ? palette.textFaint : palette.text,
              ),
              roleColor: palette.textFaint,
              trailing: place.isEmpty
                  ? null
                  : Flexible(
                      child: Padding(
                        padding: const EdgeInsets.only(left: Space.s),
                        child: Text(
                          place,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyles.caption.copyWith(
                            color: palette.textFaint,
                          ),
                        ),
                      ),
                    ),
            ),
          ),
          const SizedBox(width: Space.s),
          // A column of its own at the edge, so every row's status lines up.
          SizedBox(
            width: _statusWidth,
            child: Text(
              status,
              maxLines: 1,
              textAlign: TextAlign.right,
              style: TextStyles.caption.copyWith(
                color: working ? palette.textMuted : palette.textFaint,
              ),
            ),
          ),
        ],
      ),
    );
  }
}
