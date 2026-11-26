import 'package:flutter/widgets.dart';

import 'characters/character.dart';
import 'controls.dart';
import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'usage_wait.dart' show atWork;

/// The agents at work now, and how many there are, across every project
/// (an archived project's left out, as the sidebar leaves them out). One
/// held up by usage limits sleeps rather than works, so it isn't counted.
({List<Agent> working, int total}) teamActivity(Workspace workspace) {
  final archived = {
    for (final project in workspace.projects)
      if (project.archived) project.id,
  };
  final agents = [
    for (final agent in workspace.agents)
      if (!archived.contains(agent.projectId)) agent,
  ];
  return (working: agents.where(atWork).toList(), total: agents.length);
}

/// How many of the person's agents are working, always in sight in the
/// sidebar's footer before the usage circles: "4/23" after a dot, the live
/// dot while any works. Hovering (on iPhone, touching and holding) it shows
/// who; a click (a tap) opens the Map, which shows what each is doing.
class TeamActivity extends StatelessWidget {
  const TeamActivity({super.key, required this.workspace});

  final Workspace workspace;

  /// The most working agents the card names; the rest are counted.
  static const named = 8;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final (:working, :total) = teamActivity(workspace);
    final count = working.length;
    return Semantics(
      button: true,
      label: '$count of $total agents working. Open the Map',
      excludeSemantics: true,
      child: HoverCard(
        holdToShow: true,
        card: (context) => _TeamCard(working: working, total: total),
        child: Pressable(
          onTap: () => isPhoneLayout(context)
              ? AppScope.openPage(context, AppPage.map)
              : AppScope.togglePage(context, AppPage.map),
          builder: (context, hovered) => ConstrainedBox(
            // The usage circles' cell; taller with larger text, so the
            // count is never cut (an iPad at the largest text).
            constraints: const BoxConstraints(minHeight: 24),
            // As wide as the badge, in the footer's wrapping row too.
            child: Center(
              widthFactor: 1,
              child: AnimatedContainer(
                duration: Motion.fast,
                constraints: const BoxConstraints(minHeight: 20),
                padding: const EdgeInsets.symmetric(
                  horizontal: Space.s - 2,
                  vertical: 2,
                ),
                decoration: BoxDecoration(
                  color: hovered ? palette.hover : null,
                  border: Border.all(color: palette.hairline),
                  // Round ends at any height.
                  borderRadius: BorderRadius.circular(999),
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    if (count > 0)
                      const LiveDot()
                    else
                      Dot(color: palette.textFaint),
                    const SizedBox(width: Space.xs),
                    Text(
                      '$count/$total',
                      style: TextStyles.caption.copyWith(
                        fontSize: 11,
                        height: 1,
                        fontWeight: FontWeight.w500,
                        color: count > 0 ? palette.text : palette.textMuted,
                        fontFeatures: const [FontFeature.tabularFigures()],
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
  }
}

/// Who's working: the count, then each working agent (its character and
/// name), the first [TeamActivity.named] of them, and what a click does.
class _TeamCard extends StatelessWidget {
  const _TeamCard({required this.working, required this.total});

  final List<Agent> working;
  final int total;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final caption = TextStyles.caption.copyWith(color: palette.textMuted);
    final count = working.length;
    final more = count - TeamActivity.named;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          count == 0
              ? 'No agents working'
              : count == 1
              ? '1 of $total agents working'
              : '$count of $total agents working',
          style: TextStyles.label.copyWith(
            color: palette.text,
            fontWeight: FontWeight.w600,
          ),
        ),
        for (final agent in working.take(TeamActivity.named))
          Padding(
            padding: const EdgeInsets.only(top: Space.xs),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                AgentCharacter(agent: agent, size: 16, live: true),
                const SizedBox(width: Space.s),
                Text(
                  agent.name,
                  style: TextStyles.caption.copyWith(color: palette.text),
                ),
              ],
            ),
          ),
        if (more > 0)
          Padding(
            padding: const EdgeInsets.only(top: Space.xs),
            child: Text('and $more more', style: caption),
          ),
        Padding(
          padding: const EdgeInsets.only(top: Space.s),
          child: Text(
            isPhoneLayout(context)
                ? 'Tap to open the Map.'
                : 'Click to open the Map.',
            style: TextStyles.caption.copyWith(color: palette.textFaint),
          ),
        ),
      ],
    );
  }
}
