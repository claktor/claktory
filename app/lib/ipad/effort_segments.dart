// An agent's reasoning effort as a segmented control (Lucy's agent view
// §3, the header's model popover): one press chooses, in place. A model
// has up to seven efforts (off to max), so where their words won't fit
// across the popover they take as many rows as they need, each segment a
// finger's 44pt on touch, 28px on a Mac.
import 'dart:math' as math;

import 'package:flutter/widgets.dart';

import '../controls.dart' show Pressable;
import '../forms.dart' show effortLabel;
import '../identity.dart';
import '../mobile/metrics.dart' show Touch, TypeScale;

class EffortSegments extends StatelessWidget {
  const EffortSegments({
    super.key,
    required this.efforts,
    required this.value,
    required this.onChanged,
  });

  /// The model's efforts, in its order ("low", "medium", "high"…).
  final List<String> efforts;

  /// The one chosen; none shows chosen when it isn't one of [efforts].
  final String value;

  /// Null while it can't change (the change being sent).
  final ValueChanged<String>? onChanged;

  /// The track's inset around the segments.
  static const _inset = 2.0;

  @override
  Widget build(BuildContext context) {
    if (efforts.isEmpty) return const SizedBox.shrink();
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final touch = touchFirst(context);
    final rest = type.secondary.copyWith(fontWeight: FontWeight.w500);
    final chosenStyle = type.secondary.copyWith(fontWeight: FontWeight.w600);
    final scaler = MediaQuery.textScalerOf(context);
    final height = touch ? Touch.target : 28.0;
    return LayoutBuilder(
      builder: (context, constraints) {
        // The widest word, with room either side, decides how many go in
        // a row.
        var widest = 0.0;
        for (final effort in efforts) {
          final painter = TextPainter(
            text: TextSpan(text: effortLabel(effort), style: chosenStyle),
            textDirection: TextDirection.ltr,
            textScaler: scaler,
            maxLines: 1,
          )..layout();
          widest = math.max(widest, painter.width);
          painter.dispose();
        }
        final each = widest + Space.s * 2 + _inset * 2;
        final across = constraints.maxWidth.isFinite
            ? (constraints.maxWidth / each).floor().clamp(1, efforts.length)
            : efforts.length;
        final rows = (efforts.length / across).ceil();
        // As even as rows can be: 7 in rows of 4 and 3, not 6 and 1, and in
        // three rows 3, 2 and 2, not 3, 3 and 1; the columns are the
        // fullest row's, so segments line up.
        final perRow = (efforts.length / rows).ceil();
        final shorter = efforts.length ~/ rows;
        final longer = efforts.length % rows;
        final inRows = <List<String>>[];
        var at = 0;
        for (var row = 0; row < rows; row++) {
          final count = row < longer ? shorter + 1 : shorter;
          inRows.add(efforts.sublist(at, at + count));
          at += count;
        }

        Widget segment(String effort) {
          final chosen = effort == value;
          final label = effortLabel(effort);
          final onChanged = this.onChanged;
          void tap() {
            if (!chosen) onChanged?.call(effort);
          }

          return Expanded(
            child: Semantics(
              button: true,
              selected: chosen,
              enabled: onChanged != null,
              inMutuallyExclusiveGroup: true,
              label: label,
              onTap: onChanged == null ? null : tap,
              excludeSemantics: true,
              child: Pressable(
                key: ValueKey('effort-$effort'),
                onTap: onChanged == null ? null : tap,
                // The inset is inside each segment's target, so every one
                // is the whole height.
                builder: (context, hovered) => Padding(
                  padding: const EdgeInsets.all(_inset),
                  child: AnimatedContainer(
                    duration: Motion.fast,
                    curve: Curves.easeOut,
                    constraints: BoxConstraints(minHeight: height - _inset * 2),
                    padding: const EdgeInsets.symmetric(
                      horizontal: Space.xs,
                      vertical: 2,
                    ),
                    decoration: BoxDecoration(
                      color: chosen ? palette.canvas : const Color(0x00000000),
                      borderRadius: BorderRadius.circular(
                        Radii.control - _inset,
                      ),
                      boxShadow: chosen ? floatingShadow : null,
                    ),
                    alignment: Alignment.center,
                    child: Text(
                      label,
                      textAlign: TextAlign.center,
                      style: (chosen ? chosenStyle : rest).copyWith(
                        color: chosen || hovered
                            ? palette.text
                            : palette.textMuted,
                      ),
                    ),
                  ),
                ),
              ),
            ),
          );
        }

        return Semantics(
          container: true,
          label: 'Reasoning effort',
          explicitChildNodes: true,
          child: DecoratedBox(
            decoration: BoxDecoration(
              color: palette.hover,
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                for (final row in inRows)
                  Row(
                    children: [
                      for (final effort in row) segment(effort),
                      // A shorter row keeps the room, so segments line up.
                      for (var gap = row.length; gap < perRow; gap++)
                        const Expanded(child: SizedBox()),
                    ],
                  ),
              ],
            ),
          ),
        );
      },
    );
  }
}
