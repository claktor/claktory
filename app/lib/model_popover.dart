// The model popover (agent-view.md §3): under the model control in an
// agent's header, the model and its reasoning effort, applied at once as
// Settings → Defaults applies them, and a way to the rest of its settings.
import 'dart:async';

import 'package:flutter/widgets.dart';

import 'agent_menu.dart' show showAgentSettings, showFloating;
import 'backend.dart';
import 'controls.dart';
import 'forms.dart' show effortLabel;
import 'identity.dart';
import 'ipad/effort_segments.dart';
import 'mobile/metrics.dart';
import 'mobile/popover.dart' show popoverHere, presentPopover;
import 'model_select.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';

/// The popover's width on a Mac.
const modelPopoverWidth = 280.0;

/// The effort [agent] keeps on [model]: its own when the model takes it,
/// else the Claktory's default effort when the model takes that, else the
/// model's strongest; none for a model without efforts.
String effortOn(Workspace workspace, String model, String effort) {
  for (final candidate in workspace.models) {
    if (candidate.id != model) continue;
    if (candidate.efforts.isEmpty) return '';
    if (candidate.efforts.contains(effort)) return effort;
    final fallback = workspace.settings.defaultEffort;
    return candidate.efforts.contains(fallback)
        ? fallback
        : candidate.efforts.last;
  }
  return effort;
}

/// Opens the model popover for [agent] under [context]'s widget, the model
/// control. Each change applies at once; [changed] hears what the model and
/// effort were before it, for Undo. It stays open so the effort can follow
/// the model; Esc or a click outside closes it.
Future<void> showModelPopover(
  BuildContext context,
  Agent agent, {
  required void Function(String model, String effort) changed,
}) async {
  Widget body(BuildContext inner, VoidCallback close) => ModelPopover(
    agentId: agent.id,
    changed: changed,
    close: close,
    moreSettings: () {
      close();
      showAgentSettings(context, agent);
    },
  );
  if (popoverHere(context)) {
    await presentPopover<void>(
      context,
      builder: (inner, close) => body(inner, () => close()),
    );
    return;
  }
  await showFloating<void>(
    context,
    width: modelPopoverWidth,
    builder: (inner, close) => body(inner, () => close()),
  );
}

/// What the popover holds: on a Mac the model field (its list opens over
/// it) and the effort; on touch the list itself, a finger's rows, and the
/// effort under it. Then "More settings…".
class ModelPopover extends StatelessWidget {
  const ModelPopover({
    super.key,
    required this.agentId,
    required this.changed,
    required this.close,
    required this.moreSettings,
  });

  final String agentId;
  final void Function(String model, String effort) changed;
  final VoidCallback close;
  final VoidCallback moreSettings;

  void _apply(Backend backend, Agent agent, {String? model, String? effort}) {
    final workspace = backend.workspace.workspace;
    final update = UpdateAgent(agentId: agent.id);
    if (model != null && model.isNotEmpty && model != agent.model) {
      update.model = model;
      // A model without the effort snaps it quietly to one it takes.
      final kept = effortOn(workspace, model, agent.effort);
      if (kept != agent.effort) update.effort = kept;
    }
    if (effort != null && effort != agent.effort) update.effort = effort;
    if (!update.hasModel() && !update.hasEffort()) return;
    changed(agent.model, agent.effort);
    fireAndLog(backend.updateAgent(update));
  }

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final store = backend.workspace;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        final agent = store.agent(agentId);
        if (agent == null) return const SizedBox.shrink();
        return _body(context, backend, store.workspace, agent);
      },
    );
  }

  Widget _body(
    BuildContext context,
    Backend backend,
    Workspace workspace,
    Agent agent,
  ) {
    final touch = touchFirst(context);
    final model = workspace.models
        .where((model) => model.id == agent.model)
        .firstOrNull;
    final efforts = model?.efforts ?? const <String>[];
    final Widget? effort;
    if (efforts.isEmpty) {
      effort = null;
    } else if (touch) {
      // A finger's segments, not a list over the list (Sorbet).
      effort = Field(
        label: 'Reasoning effort',
        child: EffortSegments(
          efforts: efforts,
          value: agent.effort,
          onChanged: (effort) => _apply(backend, agent, effort: effort),
        ),
      );
    } else {
      effort = Field(
        label: 'Reasoning effort',
        child: Select(
          title: 'Reasoning effort',
          value: agent.effort,
          options: [
            for (final effort in efforts)
              SelectOption(effort, effortLabel(effort)),
            if (!efforts.contains(agent.effort))
              SelectOption(
                agent.effort,
                agent.effort.isEmpty ? 'Not set' : effortLabel(agent.effort),
              ),
          ],
          onChanged: (effort) => _apply(backend, agent, effort: effort),
        ),
      );
    }
    final more = Align(
      alignment: AlignmentDirectional.centerStart,
      child: _QuietLink(label: 'More settings…', onTap: moreSettings),
    );
    if (touch) {
      // No list over a list: the models themselves, a finger's rows, then
      // the effort's segments.
      return Padding(
        padding: const EdgeInsets.all(Space.l),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Flexible(
              child: ModelChooser(
                workspace: workspace,
                value: agent.model,
                embedded: true,
                onChoose: (id) => _apply(backend, agent, model: id),
                onCancel: close,
              ),
            ),
            const SizedBox(height: Space.m),
            ?effort,
            more,
          ],
        ),
      );
    }
    return Padding(
      padding: const EdgeInsets.all(Space.m),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Field(
            label: 'Model',
            child: ModelSelect(
              workspace: workspace,
              value: agent.model,
              onChanged: (id) => _apply(backend, agent, model: id),
            ),
          ),
          ?effort,
          more,
        ],
      ),
    );
  }
}

/// A quiet link: muted words, underlined under the pointer.
class _QuietLink extends StatelessWidget {
  const _QuietLink({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final touch = touchFirst(context);
    return Semantics(
      button: true,
      label: label,
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => Container(
          constraints: BoxConstraints(
            minHeight: touch ? Metrics.of(context).target : 24,
          ),
          alignment: AlignmentDirectional.centerStart,
          child: Text(
            label,
            style: TypeScale.of(context).body.copyWith(
              color: hovered ? palette.text : palette.textMuted,
              decoration: TextDecoration.underline,
              decorationColor: hovered ? palette.text : palette.textFaint,
            ),
          ),
        ),
      ),
    );
  }
}
