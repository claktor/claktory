import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import '../pipe_reports.dart' show pipesFeeding, readsReportsText, showReports;
import '../pipes.dart' show PortalGlyph, pipeLimitText, pipesLimiting;
import '../protocol/factory/v1/factory.pb.dart';
import '../quiet_copies.dart'
    show QuietCopiesChip, QuietWaitingChip, showsNewCopies;
import '../scope.dart';
import 'header.dart' show phoneTitleTextScale;
import 'metrics.dart' show TypeScale;
import 'platform_policy.dart';

/// How far the marks sit in from the title's start: past the agent's 28pt
/// character and the gap after it, so they line up under its name.
const _underName = 28.0 + Space.s + 2;

/// What sets an agent apart where pipes are concerned, under its name in
/// the phone's header ([PhoneHeader.marks]), as the Mac's header shows
/// beside it: "Limited to the pipe with …" (a safety signal, so it's
/// always in view) and "Reads reports from N pipes", which opens its
/// Reports. The same words as the Mac's PipeLimitMark and ReadsReportsMark,
/// but whole at any text size: a mark wraps onto more lines, and the two
/// onto two rows, rather than cutting off. Beside them, its new quiet
/// copies and the quiet messages waiting for it (QuietCopiesChip,
/// QuietWaitingChip), which open Messaging: here rather than at the bar's
/// end, which keeps two places, for Needs you and "…". Nothing for an agent
/// with none of these.
class PhoneAgentMarks extends StatelessWidget {
  const PhoneAgentMarks({super.key, required this.agent});

  final Agent agent;

  /// Whether [agent] has any mark to show.
  static bool shows(Agent agent) =>
      agent.quarantined ||
      agent.readsReports ||
      showsNewCopies(agent) ||
      agent.quietWaiting > 0;

  @override
  Widget build(BuildContext context) {
    if (!shows(agent)) return const SizedBox.shrink();
    final workspace = AppScope.of(context).workspace.workspace;
    final limiting = agent.quarantined
        ? pipesLimiting(workspace, agent.id)
        : const <Pipe>[];
    // The bar's words grow as far as its title does (iOS's bars do the
    // same), and wrap past that.
    return MediaQuery.withClampedTextScaling(
      maxScaleFactor: phoneTitleTextScale,
      child: Padding(
        padding: const EdgeInsetsDirectional.only(start: _underName),
        child: Wrap(
          spacing: Space.s,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            if (agent.quarantined)
              _Mark(
                key: const ValueKey('pipe-limit'),
                text: limiting.isEmpty
                    ? 'Limited to a pipe'
                    : pipeLimitText(limiting),
              ),
            if (agent.readsReports)
              _Mark(
                key: const ValueKey('reads-reports'),
                text: readsReportsText(
                  pipesFeeding(workspace, agent.id).length,
                ),
                onTap: () => showReports(context, reader: agent),
              ),
            if (showsNewCopies(agent))
              QuietCopiesChip(agent: agent, phone: true),
            if (agent.quietWaiting > 0)
              QuietWaitingChip(agent: agent, phone: true),
          ],
        ),
      ),
    );
  }
}

/// One mark: the pipe's portal and its words in a hairline capsule. With
/// [onTap], a button a finger's size tall, the capsule centred in it.
class _Mark extends StatelessWidget {
  const _Mark({super.key, required this.text, this.onTap});

  final String text;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    Widget capsule(bool pressed) => Container(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.s,
        vertical: Space.xs - 1,
      ),
      decoration: BoxDecoration(
        color: pressed ? palette.hover : null,
        border: Border.all(color: palette.hairline),
        borderRadius: BorderRadius.circular(Radii.control),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          const PortalGlyph(size: 12),
          const SizedBox(width: Space.xs + 2),
          Flexible(
            child: Text(
              text,
              style: type.caption.copyWith(color: palette.textMuted),
            ),
          ),
        ],
      ),
    );
    final tap = onTap;
    if (tap == null) {
      return Semantics(
        label: text,
        excludeSemantics: true,
        child: capsule(false),
      );
    }
    return Semantics(
      container: true,
      button: true,
      label: text,
      onTap: tap,
      excludeSemantics: true,
      child: Pressable(
        onTap: tap,
        builder: (context, pressed) => ConstrainedBox(
          constraints: BoxConstraints(
            minHeight: PhonePlatformPolicy.of(context).touchTarget,
          ),
          child: Align(
            alignment: AlignmentDirectional.centerStart,
            widthFactor: 1,
            heightFactor: 1,
            child: capsule(pressed),
          ),
        ),
      ),
    );
  }
}
