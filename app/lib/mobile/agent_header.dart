// An agent's header on a phone (agent-view.md §5): Back or Menu, the
// character and the name over "role · model", Needs you and ⋯. A tap on
// the name opens Agent settings, whose first field is the model.
import 'package:flutter/widgets.dart';

import '../characters/character.dart';
import '../controls.dart';
import '../forms.dart' show AgentSettingsForm;
import '../identity.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../scope.dart';
import '../settings_view.dart' show modelLabel;
import '../usage_wait.dart' show atWork;
import 'agent_marks.dart';
import 'header.dart';
import 'metrics.dart' show TypeScale;
import 'needs_you_button.dart';

/// The header over an agent's conversation on a phone.
class PhoneAgentHeader extends StatelessWidget {
  const PhoneAgentHeader({
    super.key,
    required this.agent,
    this.onBack,
    this.onMenu,
    this.onMore,
  });

  final Agent agent;
  final VoidCallback? onBack;
  final VoidCallback? onMenu;
  final VoidCallback? onMore;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final color = agentColor(agent.color);
    final working = atWork(agent);
    final model = phoneModelName(context, agent);
    return PhoneHeader(
      background: Color.alphaBlend(
        color.withValues(alpha: 0.14),
        palette.canvas,
      ),
      hairline: false,
      leading: onBack != null
          ? HeaderButton.back(onTap: onBack!)
          : HeaderButton.menu(onTap: onMenu ?? () {}),
      title: Row(
        children: [
          ExcludeSemantics(
            child: AgentCharacter(agent: agent, size: 28, live: true),
          ),
          const SizedBox(width: Space.s + 2),
          Flexible(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  agent.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: type.emphasis.copyWith(color: palette.text),
                ),
                RoleAndModel(role: agent.role, model: model),
              ],
            ),
          ),
        ],
      ),
      titleLabel: [
        agent.name,
        if (agent.role.isNotEmpty) agent.role,
        model ?? 'No model available',
      ].join(', '),
      titleHint: 'Opens agent settings',
      onTitleTap: () => showModal(
        context,
        (context, close) => AgentSettingsForm(agent: agent, close: close),
      ),
      // Limited to a pipe, Reads reports, quiet copies and waiting: under
      // its name.
      marks: PhoneAgentMarks.shows(agent)
          ? PhoneAgentMarks(agent: agent)
          : null,
      // How many things need the person, and "…". The quiet copies and
      // waiting messages show under the name, with the pipe marks.
      trailing: [
        // Needs you as the page it is on the phone, so a document a card
        // opens (Review) comes over it, and Back returns to it.
        PhoneNeedsYouButton(
          onOpen: () => AppScope.openPage(context, AppPage.inbox),
        ),
        if (onMore != null)
          HeaderButton.more(label: 'Agent menu', onTap: onMore!),
      ],
      // Its colored line, with a brighter stretch running along it while
      // it works, in step with its sidebar row's wave.
      bottom: SizedBox(
        height: 2,
        child: Stack(
          fit: StackFit.expand,
          children: [
            ColoredBox(color: color.withValues(alpha: 0.7)),
            if (working) const WorkingWave(color: Color(0xFFFFFFFF), peak: 0.6),
          ],
        ),
      ),
    );
  }
}

/// The model [agent] runs, as a phone's header names it ("Opus 4.8": the
/// maker's name left out, the header being short); null when it can't run.
String? phoneModelName(BuildContext context, Agent agent) {
  if (!agent.modelAvailable) return null;
  final model = AppScope.of(context).workspace.model(agent.model);
  final name = model == null ? agent.model : modelLabel(model);
  return name.startsWith('Claude ') ? name.substring(7) : name;
}

/// "Coordinator · Opus 4.8" on one line, never wrapping: when it doesn't
/// fit, the role is cut first, then the model at its end. With no role,
/// the model; with no model, "No model available" ("No model" where that
/// doesn't fit), muted as the Mac's header has it (Lucy 44844).
class RoleAndModel extends StatelessWidget {
  const RoleAndModel({super.key, required this.role, required this.model});

  final String role;

  /// Null: the agent's model can't run.
  final String? model;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final style = TypeScale.of(context).caption.copyWith(
      color: palette.textMuted,
    );
    final missing = style;
    return LayoutBuilder(
      builder: (context, constraints) {
        final scaler = MediaQuery.textScalerOf(context);
        final direction = Directionality.of(context);
        double width(String text, TextStyle style) {
          final painter = TextPainter(
            text: TextSpan(text: text, style: style),
            textDirection: direction,
            textScaler: scaler,
            maxLines: 1,
          )..layout();
          final width = painter.width;
          painter.dispose();
          return width;
        }

        final room = constraints.maxWidth;
        final modelText =
            model ??
            (width('No model available', missing) <= room
                ? 'No model available'
                : 'No model');
        final modelStyle = model == null ? missing : style;
        final modelWidth = width(modelText, modelStyle);
        final separator = width(' · ', style);
        Widget line(String text, TextStyle style) => Text(
          text,
          maxLines: 1,
          softWrap: false,
          overflow: TextOverflow.ellipsis,
          // As wide as what's drawn, so a cut role's "…" sits against the
          // " · " with no gap.
          textWidthBasis: TextWidthBasis.longestLine,
          style: style,
        );
        // The role gets what the model leaves, if that's enough to read a
        // few letters of it; else the model alone.
        final left = room - modelWidth - separator;
        if (role.isEmpty || left < width('Abc…', style)) {
          return line(modelText, modelStyle);
        }
        return Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            ConstrainedBox(
              constraints: BoxConstraints(maxWidth: left),
              child: line(role, style),
            ),
            Text(' · ', maxLines: 1, style: style),
            line(modelText, modelStyle),
          ],
        );
      },
    );
  }
}
