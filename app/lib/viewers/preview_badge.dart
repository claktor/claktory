// A file shown as a preview: a phone or tablet gets an image made
// for its screen, and a slow link a smaller video, so it comes many times
// sooner. Never without saying so: this badge sits over the file while
// it's a preview, with Show original to load the file itself in place.
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart' show Pressable;
import '../identity.dart';
import '../mobile/metrics.dart' show Touch, TypeScale;
import '../resources.dart' show formatBytes;
import '../selection.dart' show Unselectable;

/// Where loading the original has got.
sealed class OriginalLoad {
  const OriginalLoad();
}

/// Not asked for yet: Show original.
class OriginalNotLoaded extends OriginalLoad {
  const OriginalNotLoaded();
}

/// On its way: how much has come of [size].
class OriginalLoading extends OriginalLoad {
  const OriginalLoading(this.received, this.size);

  final int received;
  final int size;
}

/// It couldn't be loaded: Try again.
class OriginalFailed extends OriginalLoad {
  const OriginalFailed();
}

/// How far [PreviewBadge]'s text grows with the person's text size.
const maxPreviewBadgeTextScale = 1.5;

/// "Preview · Show original · 3.3 MB", over the file's bottom-left corner.
class PreviewBadge extends StatelessWidget {
  const PreviewBadge({
    super.key,
    required this.originalSize,
    required this.load,
    required this.onShowOriginal,
  });

  /// The original's size in bytes.
  final int originalSize;
  final OriginalLoad load;
  final VoidCallback onShowOriginal;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final touch =
        defaultTargetPlatform == TargetPlatform.iOS ||
        defaultTargetPlatform == TargetPlatform.android;
    final caption = TypeScale.of(context).caption;
    final muted = caption.copyWith(
      color: palette.textMuted,
      fontWeight: FontWeight.w500,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    final size = formatBytes(Int64(originalSize));
    Widget action(String label, String spoken) => Semantics(
      button: true,
      label: spoken,
      // Its own tap: excludeSemantics drops Pressable's.
      onTap: onShowOriginal,
      excludeSemantics: true,
      child: Pressable(
        onTap: onShowOriginal,
        builder: (context, hovered) => ConstrainedBox(
          // A finger's reach on a phone or tablet; a pointer's on a Mac.
          constraints: BoxConstraints(minHeight: touch ? Touch.target : 28),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: Space.s),
            child: Align(
              alignment: Alignment.centerLeft,
              widthFactor: 1,
              heightFactor: 1,
              child: Text(
                label,
                style: caption.copyWith(
                  color: hovered ? palette.text : palette.signal,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          ),
        ),
      ),
    );
    final Widget detail = switch (load) {
      OriginalNotLoaded() => action(
        'Show original · $size',
        'Show original, $size',
      ),
      OriginalLoading(:final received, :final size) => Padding(
        padding: const EdgeInsets.symmetric(horizontal: Space.s),
        child: ConstrainedBox(
          constraints: BoxConstraints(minHeight: touch ? Touch.target : 28),
          child: Align(
            alignment: Alignment.centerLeft,
            widthFactor: 1,
            heightFactor: 1,
            child: Semantics(
              liveRegion: true,
              child: Text(
                size == 0
                    ? 'Loading original…'
                    : 'Loading original… ${formatBytes(Int64(received))} '
                          'of ${formatBytes(Int64(size))}',
                style: muted,
              ),
            ),
          ),
        ),
      ),
      OriginalFailed() => Wrap(
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          Padding(
            padding: const EdgeInsets.only(left: Space.s),
            child: Text("Couldn't load the original.", style: muted),
          ),
          action('Try again', 'Try again'),
        ],
      ),
    };
    // Over the picture it labels, it grows with the text only so far, as
    // iOS's own badges over photos and video do: at the largest sizes it
    // would hide most of what it labels (at AX5 on an
    // iPhone SE). VoiceOver reads it whole all the same.
    return MediaQuery.withClampedTextScaling(
      maxScaleFactor: maxPreviewBadgeTextScale,
      child: Unselectable(
        child: Container(
          key: const ValueKey('preview-badge'),
          decoration: BoxDecoration(
            color: palette.sidebar,
            border: Border.all(color: palette.hairline),
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          // At the largest text, what it says goes under "Preview".
          child: Wrap(
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              Padding(
                padding: const EdgeInsets.only(left: Space.s),
                child: Text('Preview', style: muted),
              ),
              ExcludeSemantics(child: Text(' ·', style: muted)),
              detail,
            ],
          ),
        ),
      ),
    );
  }
}
