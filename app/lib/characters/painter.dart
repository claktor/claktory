import 'dart:math' as math;

import 'package:flutter/animation.dart';
import 'package:flutter/rendering.dart';

import 'identity.dart';
import 'pose.dart';

/// Draws a character: its body, a flat fill of its color, and two white
/// capsule eyes posed by [pose]. Lids, a failed droop and a pleased
/// crescent are drawn over the eyes in the body's own color, so nothing
/// needs a mask or a layer. Repaints when [repaint] fires, without
/// building or laying out anything.
class CharacterPainter extends CustomPainter {
  CharacterPainter({
    required this.spec,
    required this.pose,
    this.eyeColor = const Color(0xFFFFFFFF),
    this.onPaint,
    this.morph,
    super.repaint,
  });

  final CharacterSpec spec;

  /// The pose to draw now.
  final Pose Function() pose;
  final Color eyeColor;

  /// Called each time it's painted: how the window's clock knows it's on
  /// screen.
  final VoidCallback? onPaint;

  /// While the shape is being changed: the shape it's leaving and how far
  /// it's gone (0 to 1), asked each paint.
  final (CharacterShape, double)? Function()? morph;

  // Made once per painter, reused every frame.
  late final _body = Paint()
    ..color = spec.color
    ..isAntiAlias = true;
  late final _eye = Paint()
    ..color = eyeColor
    ..isAntiAlias = true;
  late final _arc = Paint()
    ..style = PaintingStyle.stroke
    ..strokeCap = StrokeCap.round
    ..isAntiAlias = true;
  final _lidPath = Path();

  @override
  void paint(Canvas canvas, Size size) {
    onPaint?.call();
    final side = math.min(size.width, size.height);
    final pose = this.pose();
    canvas.save();
    canvas.translate((size.width - side) / 2, (size.height - side) / 2);
    // Breath: the body grows and shrinks a hair, feet on the ground.
    if (pose.breath != 1) {
      final ground = side * (1 - characterInset);
      canvas.translate(0, ground);
      canvas.scale(1, pose.breath);
      canvas.translate(0, -ground);
    }
    final morph = this.morph?.call();
    canvas.drawPath(
      morph == null || morph.$1 == spec.shape
          ? bodyPath(spec.shape, side)
          : morphPath(
              morph.$1,
              spec.shape,
              Curves.easeInOutCubic.transform(morph.$2),
              side,
            ),
      _body,
    );
    // The eyes move to their place in the new shape while they're shut,
    // halfway through the blink.
    final eyes = morph != null && morph.$2 * shapeMorph < blinkDuration / 2
        ? morph.$1
        : spec.shape;
    var layout = eyeLayout(eyes);
    if (side <= 18) layout = layout.scaled(smallEyeScale);
    _drawEye(
      canvas,
      side,
      pose,
      layout.left,
      layout.height,
      pose.scaleLeft,
      layout.width,
      left: true,
    );
    _drawEye(
      canvas,
      side,
      pose,
      layout.right,
      layout.height,
      pose.scaleRight,
      layout.width,
      left: false,
    );
    canvas.restore();
  }

  void _drawEye(
    Canvas canvas,
    double side,
    Pose pose,
    Offset at,
    double restHeight,
    double scale,
    double restWidth, {
    required bool left,
  }) {
    final gazeX = pose.gazeX.clamp(-gazeLimit, gazeLimit);
    final gazeY = pose.gazeY.clamp(-gazeLimit, gazeLimit);
    final cx = (at.dx + gazeX) * side;
    final cy = (at.dy + gazeY) * side;
    final width = restWidth * side * scale;
    final full = restHeight * side * scale;
    // A blink scales the eye's height down to a sliver and back.
    final open = (pose.open * (1 - pose.sleep)).clamp(0.0, 1.0);
    final sliver = width * 0.32;
    final height = sliver + (full - sliver) * open;
    final top = cy - height / 2, bottom = cy + height / 2;
    if (pose.sleep < 1) {
      canvas.drawRRect(
        RRect.fromLTRBR(
          cx - width / 2,
          top,
          cx + width / 2,
          bottom,
          Radius.circular(math.min(width, height) / 2),
        ),
        _eye,
      );
      // The upper lid: across the top, sloping toward the nose when sad.
      if (pose.lid > 0 || pose.droop > 0) {
        final margin = side * 0.02;
        final outer = top + height * pose.lid;
        final inner = outer + height * 0.35 * pose.droop;
        final x0 = cx - width / 2 - margin, x1 = cx + width / 2 + margin;
        _lidPath
          ..reset()
          ..moveTo(x0, top - margin)
          ..lineTo(x1, top - margin)
          ..lineTo(x1, left ? inner : outer)
          ..lineTo(x0, left ? outer : inner)
          ..close();
        canvas.drawPath(_lidPath, _body);
      }
      // A pleased crescent: the lower edge curves up.
      if (pose.crescent > 0) {
        final reach = height * (0.45 - 0.3 * pose.crescent);
        canvas.drawOval(
          Rect.fromCenter(
            center: Offset(cx, bottom + reach),
            width: width * 1.7,
            height: height * 0.9,
          ),
          _body,
        );
      }
    }
    // Asleep: closed lids, drawn as soft arcs.
    if (pose.sleep > 0) {
      _arc
        ..color = eyeColor.withValues(alpha: eyeColor.a * pose.sleep)
        ..strokeWidth = math.max(width * 0.5, 0.8);
      canvas.drawArc(
        Rect.fromCenter(
          center: Offset(cx, cy - full * 0.12),
          width: width * 1.5,
          height: full * 0.5,
        ),
        math.pi * 0.15,
        math.pi * 0.7,
        false,
        _arc,
      );
    }
  }

  @override
  bool shouldRepaint(CharacterPainter old) =>
      old.spec != spec ||
      old.eyeColor != eyeColor ||
      old.morph != morph ||
      old.pose != pose;
}
