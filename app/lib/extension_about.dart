import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'forms.dart' show AgentSettingsForm;
import 'identity.dart';
import 'mobile/metrics.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';

/// The extensions whose panel the backend sends a view for (an
/// [ExtensionView] body); the others get [ExtensionAbout]. An instance
/// ("memory:factory") counts as its extension.
const extensionsWithViews = {
  'checklist',
  'memory',
  'agents',
  'tasks',
  'monitors',
  'database',
  'resources',
};

/// Whether [extension]'s panel is a view the backend sends.
bool hasExtensionView(Extension extension) => extensionsWithViews.contains(
  extension.base.isEmpty ? extension.id : extension.base,
);

/// The panel of an extension with nothing of its own to show (Web, Viewer,
/// Own checkout…): what it does, its options as set for [agentId], and a
/// way to its settings, so its icon never opens a blank panel.
class ExtensionAbout extends StatelessWidget {
  const ExtensionAbout({
    super.key,
    required this.extension,
    required this.agentId,
  });

  final Extension extension;
  final String agentId;

  @override
  Widget build(BuildContext context) {
    final store = AppScope.of(context).workspace;
    return ListenableBuilder(
      listenable: store,
      builder: (context, _) {
        final palette = Palette.of(context);
        final agent = store.agent(agentId);
        AgentExtension? settings;
        for (final candidate in agent?.extensions ?? const <AgentExtension>[]) {
          if (candidate.id == extension.id) settings = candidate;
        }
        return ListView(
          padding: const EdgeInsets.fromLTRB(
            Space.l,
            Space.m,
            Space.l,
            Space.l,
          ),
          children: [
            if (extension.description.isNotEmpty)
              Text(
                extension.description,
                style: TypeScale.of(context).body
                    .copyWith(color: palette.textMuted),
              ),
            for (final option in extension.options)
              Padding(
                padding: const EdgeInsets.only(top: Space.m),
                child: _OptionLine(
                  option: option,
                  value: settings?.options[option.id],
                  agentName: (id) => store.agent(id)?.name ?? id,
                ),
              ),
            if (agent != null) ...[
              const SizedBox(height: Space.l),
              Align(
                alignment: Alignment.centerLeft,
                child: Button(
                  label: 'Open agent settings',
                  onPressed: () => showModal(
                    context,
                    (context, close) =>
                        AgentSettingsForm(agent: agent, close: close),
                  ),
                ),
              ),
            ],
          ],
        );
      },
    );
  }
}

/// One option as the agent has it: its label, and On or Off, the agents it
/// lists, or the one of a few chosen.
class _OptionLine extends StatelessWidget {
  const _OptionLine({
    required this.option,
    required this.value,
    required this.agentName,
  });

  final ExtensionOption option;
  final OptionValue? value;
  final String Function(String id) agentName;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final value = this.value;
    final shown = switch (option.kind) {
      ExtensionOptionKind.EXTENSION_OPTION_KIND_SWITCH =>
        (value?.on ?? option.defaultValue) ? 'On' : 'Off',
      ExtensionOptionKind.EXTENSION_OPTION_KIND_AGENTS => [
        if (option.includesProject) 'Its project',
        for (final id in value?.agentIds ?? const <String>[]) agentName(id),
      ].join(', '),
      // The chosen one's label, the default's while none is set.
      ExtensionOptionKind.EXTENSION_OPTION_KIND_CHOICE => () {
        final chosen = (value?.choice ?? '').isEmpty
            ? option.defaultChoice
            : value!.choice;
        return option.choices
                .where((choice) => choice.value == chosen)
                .firstOrNull
                ?.label ??
            chosen;
      }(),
      _ => 'Needs a newer Claktory',
    };
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          option.label,
          style: TypeScale.of(context).caption
              .copyWith(color: palette.textFaint, fontWeight: FontWeight.w500),
        ),
        Text(
          shown.isEmpty ? 'None' : shown,
          style: TypeScale.of(context).body.copyWith(color: palette.text),
        ),
      ],
    );
  }
}
