// A step the model declined on safety grounds (Notice.refusal_category):
// not a failure, so not an error line. Its own quiet card says so, with the
// model's own explanation, and what happens next: the fallback model a
// declined step is retried on, or where to choose one.
import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'forms.dart' show AgentSettingsForm;
import 'identity.dart';
import 'mobile/metrics.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';

/// The safety area a refusal named, as the card says it: in plain words
/// where they're known, else as the provider gave it; empty when it named
/// none ("unspecified").
String refusalArea(String category) => switch (category.trim().toLowerCase()) {
  'unspecified' || '' => '',
  'cyber' => 'cybersecurity',
  'bio' => 'biology',
  final other => other,
};

/// The model a declined step of [agent]'s is retried on (its own fallback,
/// else the Claktory-wide default; null for a chat's, [agent] null), by
/// name; empty when there's none.
String fallbackModelName(Workspace workspace, Agent? agent) {
  final id = (agent?.fallbackModel ?? '').isNotEmpty
      ? agent!.fallbackModel
      : workspace.settings.defaultFallbackModel;
  if (id.isEmpty) return '';
  final model = workspace.models.where((model) => model.id == id).firstOrNull;
  return model == null || model.name.isEmpty ? id : model.name;
}

/// The card for a refusal [notice] in [agent]'s conversation (null in a
/// chat).
class RefusalCard extends StatelessWidget {
  const RefusalCard(this.notice, {super.key, this.agent});

  final Notice notice;
  final Agent? agent;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final workspace = AppScope.of(context).workspace.workspace;
    final area = refusalArea(notice.refusalCategory);
    final agent = this.agent;
    final fallback = fallbackModelName(workspace, agent);
    final explanation = notice.text.trim();
    return Container(
      padding: const EdgeInsets.all(Space.m),
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              ExtensionGlyph(
                icon: 'shield-alert',
                name: '',
                color: palette.textMuted,
                size: 14,
              ),
              const SizedBox(width: Space.s),
              Expanded(
                child: Text.rich(
                  TextSpan(
                    text: 'Declined for safety',
                    children: [
                      if (area.isNotEmpty)
                        TextSpan(
                          text: ' · $area',
                          style: type.caption.copyWith(
                            color: palette.textFaint,
                            fontWeight: FontWeight.w400,
                          ),
                        ),
                    ],
                  ),
                  style: type.caption.copyWith(
                    color: palette.textMuted,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          ),
          if (explanation.isNotEmpty) ...[
            const SizedBox(height: Space.xs),
            // The model's own words, shown as they are.
            Text(
              explanation,
              style: type.body.copyWith(color: palette.textMuted),
            ),
          ],
          const SizedBox(height: Space.s),
          if (fallback.isNotEmpty)
            Text(
              'Claktory tries declined steps once more on $fallback.',
              style: type.caption.copyWith(color: palette.textFaint),
            )
          else if (agent != null)
            Wrap(
              spacing: Space.s,
              crossAxisAlignment: WrapCrossAlignment.end,
              children: [
                Text(
                  'Claktory can try declined steps again on another model.',
                  style: type.caption.copyWith(color: palette.textFaint),
                ),
                _Link(
                  label: 'Choose a fallback model…',
                  onTap: () => showModal(
                    context,
                    (context, close) =>
                        AgentSettingsForm(agent: agent, close: close),
                  ),
                ),
              ],
            ),
        ],
      ),
    );
  }
}

/// A caption link, as an error line's.
class _Link extends StatelessWidget {
  const _Link({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = isPhoneLayout(context);
    return Semantics(
      button: true,
      label: label,
      onTap: onTap,
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => ConstrainedBox(
          // A finger's height on a phone.
          constraints: BoxConstraints(minHeight: phone ? Touch.target : 0),
          child: Align(
            widthFactor: 1,
            heightFactor: 1,
            alignment: Alignment.centerLeft,
            child: Text(
              label,
              style: TypeScale.of(context).caption.copyWith(
                color: hovered ? palette.text : palette.textMuted,
                fontWeight: FontWeight.w500,
                decoration: hovered ? TextDecoration.underline : null,
                decorationColor: palette.text,
              ),
            ),
          ),
        ),
      ),
    );
  }
}
