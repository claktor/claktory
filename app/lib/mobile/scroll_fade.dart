// Words that scroll in a box shorter than they are (a permission sheet at
// the largest text, an iPad review bar's question): while there's more
// below, the box's foot fades out, so words cut off read as more to
// scroll, not as the end.
import 'dart:math' as math;

import 'package:flutter/widgets.dart';

/// [child] in a vertical scroll whose foot fades out over [fade] while
/// there's more below it, and stops fading at the end.
class ScrollsWithFade extends StatefulWidget {
  const ScrollsWithFade({super.key, required this.child, this.fade = 32});

  final Widget child;

  /// How tall the fade is.
  final double fade;

  @override
  State<ScrollsWithFade> createState() => _ScrollsWithFadeState();
}

class _ScrollsWithFadeState extends State<ScrollsWithFade> {
  var _more = false;

  bool _measured(ScrollMetricsNotification notification) {
    _follow(notification.metrics);
    return false;
  }

  bool _scrolled(ScrollUpdateNotification notification) {
    _follow(notification.metrics);
    return false;
  }

  void _follow(ScrollMetrics metrics) {
    final more = metrics.extentAfter > 1;
    if (more != _more) setState(() => _more = more);
  }

  static const _opaque = Color(0xFFFFFFFF);

  @override
  Widget build(BuildContext context) {
    // Always the mask, only its gradient changing: the scroll under it
    // keeps its place.
    return ShaderMask(
      blendMode: BlendMode.dstIn,
      shaderCallback: (bounds) => LinearGradient(
        begin: Alignment.topCenter,
        end: Alignment.bottomCenter,
        colors: [_opaque, _opaque, _more ? const Color(0x00FFFFFF) : _opaque],
        stops: [
          0,
          math.max(0, 1 - widget.fade / math.max(bounds.height, 1)),
          1,
        ],
      ).createShader(bounds),
      child: NotificationListener<ScrollUpdateNotification>(
        onNotification: _scrolled,
        child: NotificationListener<ScrollMetricsNotification>(
          onNotification: _measured,
          child: SingleChildScrollView(child: widget.child),
        ),
      ),
    );
  }
}
