import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import '../protocol/factory/v1/factory.pb.dart';
import '../scope.dart';
import 'browser_session.dart';
import 'errand_panel.dart' show errandStateLabel, showErrand;

// An agent's errand in Claktory's browser, in its header (#435 P5): the
// person sees how it goes, opens its log, or stops it at any moment, from
// the agent itself. They never drive the browser (Matt, card 222).

/// The errand (its panel: what it's for, how it goes, its log) and Stop,
/// while [agent] runs an errand in Claktory's browser; nothing otherwise.
class ErrandHeaderControls extends StatelessWidget {
  const ErrandHeaderControls({super.key, required this.agent});

  final Agent agent;

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    final workspace = backend.workspace.workspace;
    if (!agentBrowserAvailable(workspace)) return const SizedBox.shrink();
    final errand = runningErrandOf(workspace, agent.id);
    if (errand == null) return const SizedBox.shrink();
    final session = AgentBrowserSession.of(backend);
    final purpose = errand.purpose.isEmpty ? '' : ': ${errand.purpose}';
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        const SizedBox(width: Space.s),
        HoverCard(
          end: true,
          card: (context) => Text(
            "${agent.name}'s errand in Claktory's browser$purpose · "
            '${errandStateLabel(errand)}',
          ),
          child: GlyphButton(
            label: 'Errand',
            active: false,
            onTap: () => showErrand(context, errand),
            glyph: (color) =>
                ExtensionGlyph(icon: 'monitor', name: 'Errand', color: color),
          ),
        ),
        const SizedBox(width: Space.s),
        Button(label: 'Stop', onPressed: session.stop),
      ],
    );
  }
}
