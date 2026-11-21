import 'package:flutter/widgets.dart';

import 'identity.dart';
import 'pipes.dart' show PortalGlyph, pipeColor, pipePeerClaktory;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';

/// Where what an agent asks or proposes may come from: for each pipe it has
/// read (a card's `taint`), one line, "from Liaison, which reads the pipe
/// with Ana's Claktory", after a small portal. Nothing when [taint] is
/// empty. [agent] is the name of the agent whose card it is, for a pipe it
/// read itself.
class TaintLabel extends StatelessWidget {
  const TaintLabel({super.key, required this.taint, required this.agent});

  final List<PipeTaint> taint;
  final String agent;

  @override
  Widget build(BuildContext context) {
    if (taint.isEmpty) return const SizedBox.shrink();
    final palette = Palette.of(context);
    final pipes = AppScope.of(context).workspace.workspace.pipes;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final line in taintLines(taint, agent: agent, pipes: pipes))
          Padding(
            padding: const EdgeInsets.only(top: 2),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Padding(
                  padding: EdgeInsets.only(top: 2),
                  child: ExcludeSemantics(
                    child: PortalGlyph(color: pipeColor, size: 12),
                  ),
                ),
                const SizedBox(width: Space.xs + 2),
                Expanded(
                  child: Text(
                    line,
                    style: TextStyles.caption.copyWith(
                      color: palette.textMuted,
                    ),
                  ),
                ),
              ],
            ),
          ),
      ],
    );
  }
}

/// [taint] in words, a line per pipe: "from Liaison, which reads the pipe
/// with Ana's Claktory"; through others, "from Lea, which heard from the
/// pipe with Ana's Claktory via Liaison". The other side is named from
/// [pipes] by the pipe's id; a pipe that's gone, by its name.
List<String> taintLines(
  List<PipeTaint> taint, {
  required String agent,
  required List<Pipe> pipes,
}) => [
  for (final read in taint)
    () {
      final pipe = pipes.where((pipe) => pipe.id == read.pipeId).firstOrNull;
      final where = pipe == null
          ? 'the pipe “${read.pipeName}”'
          : 'the pipe with ${pipePeerClaktory(pipe)}';
      final via = read.viaNames;
      if (via.length < 2) {
        final reader = via.isEmpty ? agent : via.first;
        return 'from $reader, which reads $where';
      }
      return 'from ${via.last}, which heard from $where via '
          '${via.sublist(0, via.length - 1).join(' → ')}';
    }(),
];
