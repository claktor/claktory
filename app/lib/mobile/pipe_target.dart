import 'dart:async';

import 'package:flutter/widgets.dart';

import '../characters/character.dart';
import '../controls.dart' show Chevron, Pressable, SelectOption;
import '../identity.dart';
import '../pipe_timeline.dart' show PipeTarget;
import '../pipes.dart';
import '../protocol/factory/v1/factory.pb.dart';
import 'haptics.dart';
import 'metrics.dart';
import 'platform_policy.dart';
import 'presentation.dart';

/// On a phone, above the box of a pipe with a liaison: whom the box writes
/// to, one chip at its leading edge ("To Kai ▾"), a finger's size, that
/// opens a choice of the two. While it's the other side, the chip takes
/// the pipe's color and its portal, so a message that leaves this
/// Claktory never looks like one that stays. The same props as the Mac's
/// pills (PipeTargetChooser); the box goes back to the liaison after every
/// message ([PipeTargets.reset]).
class PhonePipeTarget extends StatelessWidget {
  const PhonePipeTarget({
    super.key,
    required this.pipe,
    required this.liaison,
    required this.target,
    required this.onChanged,
  });

  final Pipe pipe;
  final Agent liaison;
  final PipeTarget target;
  final ValueChanged<PipeTarget> onChanged;

  String get _liaisonLabel => 'To ${liaison.name}';
  String get _pipeLabel => 'To ${pipePeerClaktory(pipe)}';

  Future<void> _choose(BuildContext context) async {
    final chosen = await showChoice<PipeTarget>(
      context,
      title: 'Write to',
      selected: target,
      options: [
        SelectOption(PipeTarget.liaison, _liaisonLabel),
        SelectOption(PipeTarget.pipe, _pipeLabel),
      ],
    );
    if (chosen == null || chosen == target) return;
    unawaited(Haptics.selection());
    onChanged(chosen);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final through = target == PipeTarget.pipe;
    final color = through ? pipeColor : palette.textMuted;
    final label = through ? _pipeLabel : _liaisonLabel;
    final height = PhonePlatformPolicy.of(context).touchTarget;
    return Padding(
      padding: const EdgeInsets.fromLTRB(Space.s, Space.xs, Space.s, 0),
      child: Align(
        alignment: AlignmentDirectional.centerStart,
        child: Semantics(
          button: true,
          label: label,
          hint: 'Choose whom to write to',
          onTap: () => _choose(context),
          excludeSemantics: true,
          child: Pressable(
            onTap: () => _choose(context),
            builder: (context, pressed) => ConstrainedBox(
              constraints: BoxConstraints(minHeight: height),
              child: Center(
                widthFactor: 1,
                child: AnimatedContainer(
                  duration: Motion.fast,
                  curve: Curves.easeOut,
                  padding: const EdgeInsets.symmetric(
                    horizontal: Space.m,
                    vertical: Space.xs,
                  ),
                  decoration: BoxDecoration(
                    color: through
                        ? pipeColor.withValues(alpha: pressed ? 0.24 : 0.14)
                        : (pressed ? palette.hover : palette.selected),
                    borderRadius: BorderRadius.circular(999),
                    border: Border.all(
                      color: through
                          ? pipeColor.withValues(alpha: 0.55)
                          : const Color(0x00000000),
                    ),
                  ),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      if (through)
                        const PortalGlyph(size: 16)
                      else
                        AgentCharacter(agent: identityOf(liaison), size: 16),
                      const SizedBox(width: Space.xs + 2),
                      Flexible(
                        child: Text(
                          label,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: type.caption.copyWith(
                            color: through ? pipeColor : palette.text,
                            fontWeight: FontWeight.w500,
                          ),
                        ),
                      ),
                      const SizedBox(width: Space.xs),
                      Chevron(color: color, down: true),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
