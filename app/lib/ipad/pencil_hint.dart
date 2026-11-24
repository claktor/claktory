import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/semantics.dart';
import 'package:flutter/widgets.dart';

import '../identity.dart';
import '../mobile/device.dart' show Idiom, deviceCapabilities;
import '../mobile/metrics.dart' show TypeScale;

/// Over a pane Apple Pencil doesn't draw in yet (an image, #362 I2): when
/// the Pencil touches it on an iPad, a line at the pane's foot says why
/// nothing drew and what works there instead, for a few seconds. Nothing
/// changes for a finger, a mouse or a trackpad, or anywhere but an iPad.
class PencilHint extends StatefulWidget {
  const PencilHint({super.key, required this.text, required this.child});

  /// What the line says (Trish's words).
  final String text;
  final Widget child;

  /// How long the line stays after the Pencil's last touch.
  @visibleForTesting
  static var shownFor = const Duration(seconds: 5);

  /// Whether this device draws with a Pencil: an iPad. Tests set it.
  @visibleForTesting
  static bool Function() onIpad = () =>
      defaultTargetPlatform == TargetPlatform.iOS &&
      deviceCapabilities.idiom == Idiom.pad;

  @override
  State<PencilHint> createState() => _PencilHintState();
}

class _PencilHintState extends State<PencilHint> {
  var _shown = false;
  Timer? _hide;

  void _down(PointerDownEvent event) {
    if (event.kind != PointerDeviceKind.stylus) return;
    _hide?.cancel();
    _hide = Timer(PencilHint.shownFor, () {
      if (mounted) setState(() => _shown = false);
    });
    if (_shown) return;
    setState(() => _shown = true);
    // VoiceOver says it too: a sighted person may not look down.
    final view = View.maybeOf(context);
    if (view != null) {
      unawaited(
        SemanticsService.sendAnnouncement(
          view,
          widget.text,
          Directionality.of(context),
        ),
      );
    }
  }

  @override
  void dispose() {
    _hide?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (!PencilHint.onIpad()) return widget.child;
    final palette = Palette.of(context);
    return Stack(
      children: [
        Positioned.fill(
          child: Listener(
            behavior: HitTestBehavior.translucent,
            onPointerDown: _down,
            child: widget.child,
          ),
        ),
        Positioned(
          left: Space.l,
          right: Space.l,
          bottom: Space.l,
          child: IgnorePointer(
            child: AnimatedOpacity(
              opacity: _shown ? 1 : 0,
              duration: Motion.fast,
              child: Center(
                child: Container(
                  padding: const EdgeInsets.symmetric(
                    horizontal: Space.m,
                    vertical: Space.s,
                  ),
                  decoration: BoxDecoration(
                    color: palette.sidebar,
                    border: Border.all(color: palette.hairline),
                    borderRadius: BorderRadius.circular(Radii.panel),
                  ),
                  child: Text(
                    widget.text,
                    textAlign: TextAlign.center,
                    style: TypeScale.of(context).body
                        .copyWith(color: palette.text),
                  ),
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}
