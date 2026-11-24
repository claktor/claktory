// A big file on its way (#375): how much of it has come, with a bar, so a
// slow link shows it's moving; and while the connection is away, that it
// carries on from there once it's back.
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import '../identity.dart';
import '../mobile/metrics.dart' show TypeScale;
import '../resources.dart' show formatBytes;

/// "Loading video… 2.1 MB of 9.4 MB" over a bar; [label] alone before the
/// size is known.
class LoadingProgress extends StatelessWidget {
  const LoadingProgress({
    super.key,
    required this.label,
    required this.received,
    required this.size,
    required this.connected,
    this.note,
  });

  /// A line under it: "Full size: this Claktory can't make a smaller
  /// copy." (#379).
  final String? note;

  /// What's loading: "Loading video…".
  final String label;
  final int received;

  /// The whole file's size; 0 before it's known.
  final int size;
  final ValueListenable<bool> connected;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final style = TypeScale.of(context).caption.copyWith(
      color: palette.textMuted,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    return Center(
      child: SingleChildScrollView(
        padding: const EdgeInsets.all(Space.xl),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              size == 0
                  ? label
                  : '$label ${formatBytes(Int64(received))} of '
                        '${formatBytes(Int64(size))}',
              textAlign: TextAlign.center,
              style: style,
            ),
            const SizedBox(height: Space.s),
            Semantics(
              value: size == 0
                  ? null
                  : '${(received * 100 / size).clamp(0, 100).round()}%',
              child: SizedBox(
                width: 160,
                height: 4,
                child: ClipRRect(
                  borderRadius: BorderRadius.circular(2),
                  child: Stack(
                    children: [
                      Positioned.fill(
                        child: ColoredBox(color: palette.selected),
                      ),
                      FractionallySizedBox(
                        widthFactor: size == 0
                            ? 0
                            : (received / size).clamp(0.0, 1.0),
                        child: ColoredBox(color: palette.signal),
                      ),
                    ],
                  ),
                ),
              ),
            ),
            if (note case final note?)
              Padding(
                padding: const EdgeInsets.only(top: Space.s),
                child: Text(note, textAlign: TextAlign.center, style: style),
              ),
            ValueListenableBuilder<bool>(
              valueListenable: connected,
              builder: (context, connected, _) => connected
                  ? const SizedBox.shrink()
                  : Padding(
                      padding: const EdgeInsets.only(top: Space.s),
                      child: Text(
                        "Waiting for the connection. It carries on from "
                        "here when it's back.",
                        textAlign: TextAlign.center,
                        style: style,
                      ),
                    ),
            ),
          ],
        ),
      ),
    );
  }
}
