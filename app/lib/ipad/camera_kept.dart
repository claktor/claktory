// The camera kept from the app (an iPad sharing its screen in Split View,
// Slide Over or Stage Manager, or another app using it): the scanner's
// square has nothing to show, so it shrinks to a band that holds what keeps
// the camera away, and the way on (Paste code instead) stays in sight
// under it at every text size (Lucy's permissions.md P7, her 48961).
import 'dart:math' as math;

import 'package:flutter/widgets.dart';

import '../identity.dart';
import '../mobile/metrics.dart' show TypeScale;
import '../mobile/scroll_fade.dart' show ScrollsWithFade;

/// A black band of at least [minHeight], as tall as [words] need, over the
/// camera's [preview], which stays alive under it so the camera can say
/// when it's back. Give [preview] a GlobalKey, so moving between the square
/// and the band keeps the same native view.
class CameraKeptBand extends StatelessWidget {
  const CameraKeptBand({super.key, required this.words, required this.preview});

  final String words;
  final Widget preview;

  static const minHeight = 120.0;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Container(
      clipBehavior: Clip.antiAlias,
      decoration: BoxDecoration(
        color: const Color(0xFF000000),
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
      ),
      // No taller than 40% of the window: past that its words scroll, so
      // what's under it (the way on) stays in sight.
      child: ConstrainedBox(
        constraints: BoxConstraints(
          minHeight: minHeight,
          maxHeight: math.max(
            minHeight,
            MediaQuery.sizeOf(context).height * 0.4,
          ),
        ),
        child: Stack(
          alignment: Alignment.center,
          children: [
            Positioned.fill(child: preview),
            // Over the picture's own words, which a band this short would
            // cut; those stay the ones VoiceOver reads (a live region).
            const Positioned.fill(child: ColoredBox(color: Color(0xFF000000))),
            ScrollsWithFade(
              child: SizedBox(
                width: double.infinity,
                child: Padding(
                  padding: const EdgeInsets.all(Space.l),
                  child: ExcludeSemantics(
                    child: Text(
                      words,
                      key: const ValueKey('camera-kept-words'),
                      textAlign: TextAlign.center,
                      style: TypeScale.of(context).body
                          .copyWith(color: const Color(0xFFFFFFFF)),
                    ),
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
