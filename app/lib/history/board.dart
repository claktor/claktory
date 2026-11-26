// History's board (#458): the Map's boxes, lines and chips as they were at
// the playhead, with what moves on top: messages as pulses along the
// lines, a glow on agents while they work, a tick for a task, monitor or
// Claktory event. Reconstructed things are dotted.
//
// What changes only when the picture does (boxes, lines, chips) is built
// from a [SceneFrame]; what moves every frame is painted from the
// playhead alone.
import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/widgets.dart';

import '../avatar.dart';
import '../identity.dart';
import '../map.dart' show MapChipView, mapLayerColor;
import '../map_layout.dart';
import '../pipes.dart' show PortalGlyph, pipeColor;
import '../protocol/factory/v1/factory.pb.dart';
import 'controller.dart';
import 'pulses.dart';
import 'scene.dart';
import 'timeline.dart' show spanAt;

/// How long a chip, box or pad takes to fade in or out.
const _fade = Duration(milliseconds: 300);

/// A moved agent's hop to its new place.
const hopDuration = Duration(milliseconds: 600);

/// A video's frame (export.dart): where each thing is and how faded,
/// worked out a step at a time rather than animated, and the words a share
/// shows in place of the person's own.
class BoardStill {
  const BoardStill({
    required this.chips,
    required this.boxRects,
    required this.boxOpacity,
    required this.padOpacity,
    required this.words,
  });

  /// Each node's chip (an index into the scene's chips) where it is now,
  /// and how faded.
  final List<({int chip, Rect rect, double opacity})> chips;
  final List<Rect> boxRects;
  final List<double> boxOpacity;
  final List<double> padOpacity;
  final BoardWords words;
}

/// What a share calls things.
abstract class BoardWords {
  /// Chip [chip]'s name and role, given what they were then.
  (String, String) chip(int chip, String label, String detail);

  /// Box [box]'s title and path.
  (String, String) box(int box);

  /// Pad [pad]'s name.
  String pad(int pad);

  /// How [person] is drawn as You.
  Person person(Person person);
}

class HistoryBoard extends StatefulWidget {
  const HistoryBoard({
    super.key,
    required this.controller,
    required this.frame,
    required this.person,
    this.focus,
    this.onHover,
    this.onTap,
    this.budget = pulseBudget,
    this.still,
  });

  final HistoryController controller;
  final SceneFrame frame;
  final Person person;

  /// The chip pointed at or pinned (an index into the scene's chips): its
  /// lines lit, the rest dimmed.
  final int? focus;
  final ValueChanged<int?>? onHover;
  final ValueChanged<int>? onTap;
  final int budget;

  /// A video's frame: drawn as it says, with nothing animated or pointed
  /// at.
  final BoardStill? still;

  @override
  State<HistoryBoard> createState() => _HistoryBoardState();
}

class _HistoryBoardState extends State<HistoryBoard> {
  /// Where each agent's chip was last shown, so it fades out in place.
  final _lastChip = <String, int>{};

  /// Where each box was last drawn, and which were shown.
  final _lastBox = <String, Rect>{};
  final _shownBoxes = <String>{};

  @override
  Widget build(BuildContext context) {
    if (widget.still case final still?) return _still(context, still);
    final controller = widget.controller;
    final scene = controller.scene!;
    final frame = widget.frame;
    final palette = Palette.of(context);
    final reduced = MediaQuery.disableAnimationsOf(context);
    final fade = reduced ? Duration.zero : _fade;
    final focus = widget.focus;
    final lit = focus == null ? null : _litChips(scene, frame, focus);

    // One chip per node: a moved agent's shown one, or where it was last.
    final chipFor = <String, int>{};
    for (var i = 0; i < scene.chips.length; i++) {
      final origin = scene.originOf(scene.chips[i].id);
      if (frame.chips[i] != hidden) {
        chipFor[origin] = i;
      } else {
        chipFor.putIfAbsent(origin, () => _lastChip[origin] ?? i);
      }
    }
    _lastChip
      ..clear()
      ..addAll(chipFor);

    // A box's edges follow what's in it; one appearing takes its place
    // while still unseen, and one going fades where it was.
    final boxRects = <Rect>[];
    final boxMoves = <bool>[];
    for (var i = 0; i < scene.boxes.length; i++) {
      // By index: a project's own folder has the project's id.
      final key = '$i:${scene.boxes[i].box.id}';
      final shown = frame.boxes[i] != hidden;
      final rect = shown
          ? frame.boxRects[i]
          : _lastBox[key] ?? frame.boxRects[i];
      boxMoves.add(shown && _shownBoxes.contains(key));
      boxRects.add(rect);
      _lastBox[key] = rect;
    }
    _shownBoxes
      ..clear()
      ..addAll([
        for (var i = 0; i < scene.boxes.length; i++)
          if (frame.boxes[i] != hidden) '$i:${scene.boxes[i].box.id}',
      ]);

    return Stack(
      children: [
        for (var i = 0; i < scene.boxes.length; i++)
          AnimatedPositioned.fromRect(
            key: ValueKey('box:$i:${scene.boxes[i].box.id}'),
            duration: reduced || !boxMoves[i] ? Duration.zero : hopDuration,
            curve: Curves.easeInOutCubic,
            rect: boxRects[i],
            child: IgnorePointer(
              child: AnimatedOpacity(
                duration: fade,
                opacity: frame.boxes[i] == hidden ? 0 : 1,
                child: CustomPaint(
                  painter: _BoxPainter(
                    scene.boxes[i].box,
                    palette,
                    dotted: frame.boxes[i] == dotted,
                  ),
                ),
              ),
            ),
          ),
        Positioned.fill(
          child: IgnorePointer(
            child: RepaintBoundary(
              child: CustomPaint(
                painter: _LinesPainter(
                  scene: scene,
                  frame: frame,
                  palette: palette,
                  focus: focus,
                ),
              ),
            ),
          ),
        ),
        Positioned.fill(
          child: IgnorePointer(
            child: RepaintBoundary(
              child: CustomPaint(
                painter: _GlowPainter(controller, palette, reduced: reduced),
              ),
            ),
          ),
        ),
        if (scene.layout.you case final you?)
          Positioned.fromRect(
            rect: you,
            child: IgnorePointer(
              child: AnimatedOpacity(
                duration: fade,
                opacity: lit == null ? 1 : 0.3,
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    PersonAvatar(person: widget.person, size: 32),
                    const SizedBox(height: Space.xs),
                    Text(
                      'You',
                      style: TextStyles.label.copyWith(color: palette.text),
                    ),
                  ],
                ),
              ),
            ),
          ),
        for (var i = 0; i < scene.pads.length; i++)
          Positioned.fromRect(
            rect: scene.pads[i].rect,
            child: IgnorePointer(
              child: AnimatedOpacity(
                duration: fade,
                opacity: frame.pads[i] == hidden
                    ? 0
                    : lit == null
                    ? (frame.pads[i] == dotted ? 0.7 : 1)
                    : 0.3,
                child: _PadView(label: scene.pads[i].label),
              ),
            ),
          ),
        for (final MapEntry(key: origin, value: i) in chipFor.entries)
          AnimatedPositioned.fromRect(
            key: ValueKey(origin),
            duration: reduced ? Duration.zero : hopDuration,
            curve: Curves.easeInOutCubic,
            rect: scene.chips[i].rect,
            child: _ChipView(
              chip: scene.chips[i],
              label: frame.labels[i],
              detail: frame.details[i],
              state: frame.chips[i],
              opacity: frame.chips[i] == hidden
                  ? 0
                  : lit == null || lit.contains(i)
                  ? 1
                  : 0.3,
              focused: focus == i,
              fade: fade,
              onHover: (on) => widget.onHover?.call(on ? i : null),
              onTap: () => widget.onTap?.call(i),
            ),
          ),
        Positioned.fill(
          child: IgnorePointer(
            child: RepaintBoundary(
              child: CustomPaint(
                painter: _PulsePainter(
                  controller,
                  palette,
                  budget: widget.budget,
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

extension on _HistoryBoardState {
  /// [still], drawn as it says.
  Widget _still(BuildContext context, BoardStill still) {
    final controller = widget.controller;
    final scene = controller.scene!;
    final frame = widget.frame;
    final palette = Palette.of(context);
    final words = still.words;
    return Stack(
      children: [
        for (var i = 0; i < scene.boxes.length; i++)
          if (still.boxOpacity[i] > 0)
            Positioned.fromRect(
              rect: still.boxRects[i],
              child: Opacity(
                opacity: still.boxOpacity[i],
                child: CustomPaint(
                  painter: _BoxPainter(
                    scene.boxes[i].box,
                    palette,
                    dotted: frame.boxes[i] == dotted,
                    words: words.box(i),
                  ),
                ),
              ),
            ),
        Positioned.fill(
          child: CustomPaint(
            painter: _LinesPainter(
              scene: scene,
              frame: frame,
              palette: palette,
              focus: null,
            ),
          ),
        ),
        Positioned.fill(
          child: CustomPaint(
            painter: _GlowPainter(controller, palette, reduced: false),
          ),
        ),
        if (scene.layout.you case final you?)
          Positioned.fromRect(
            rect: you,
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                PersonAvatar(person: words.person(widget.person), size: 32),
                const SizedBox(height: Space.xs),
                Text(
                  'You',
                  style: TextStyles.label.copyWith(color: palette.text),
                ),
              ],
            ),
          ),
        for (var i = 0; i < scene.pads.length; i++)
          if (still.padOpacity[i] > 0)
            Positioned.fromRect(
              // Unnamed, only its portal, under its line (Lucy).
              rect: words.pad(i).isEmpty
                  ? Rect.fromLTWH(
                      pipeTop(scene.pads[i].rect).dx - padHeight / 2,
                      scene.pads[i].rect.top,
                      padHeight,
                      padHeight,
                    )
                  : scene.pads[i].rect,
              child: Opacity(
                opacity:
                    still.padOpacity[i] * (frame.pads[i] == dotted ? 0.7 : 1),
                child: _PadView(label: words.pad(i)),
              ),
            ),
        for (final chip in still.chips)
          if (chip.opacity > 0)
            Positioned.fromRect(
              rect: chip.rect,
              child: Opacity(
                opacity: chip.opacity,
                child: _StillChip(
                  chip: scene.chips[chip.chip],
                  text: words.chip(
                    chip.chip,
                    frame.labels[chip.chip],
                    frame.details[chip.chip],
                  ),
                  dotted: frame.chips[chip.chip] == dotted,
                ),
              ),
            ),
        Positioned.fill(
          child: CustomPaint(
            painter: _PulsePainter(controller, palette, budget: widget.budget),
          ),
        ),
      ],
    );
  }
}

/// A chip in a video's frame: as the Map draws it, with a share's words.
class _StillChip extends StatelessWidget {
  const _StillChip({
    required this.chip,
    required this.text,
    required this.dotted,
  });

  final SceneChip chip;
  final (String, String) text;
  final bool dotted;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final node = chip.chip.node.deepCopy()
      ..label = text.$1
      ..detail = text.$2;
    return Stack(
      fit: StackFit.expand,
      children: [
        MapChipView(
          chip: MapChip(node: node, rect: chip.rect),
          focused: false,
        ),
        if (dotted)
          CustomPaint(
            painter: _DashedOutline(
              palette.textFaint,
              Radii.control,
              fill: palette.canvas,
            ),
          ),
      ],
    );
  }
}

/// The chips [focus] shares a shown line with, and itself.
Set<int> _litChips(HistoryScene scene, SceneFrame frame, int focus) {
  final out = {focus};
  for (var i = 0; i < scene.wires.length; i++) {
    if (frame.wires[i] == hidden) continue;
    final wire = scene.wires[i];
    if (wire.a == focus && wire.b >= 0) out.add(wire.b);
    if (wire.b == focus && wire.a >= 0) out.add(wire.a);
  }
  return out;
}

/// A chip as the Map draws it, at the name and role it had then; dotted
/// round its edge when reconstructed.
class _ChipView extends StatelessWidget {
  const _ChipView({
    required this.chip,
    required this.label,
    required this.detail,
    required this.state,
    required this.opacity,
    required this.focused,
    required this.fade,
    required this.onHover,
    required this.onTap,
  });

  final SceneChip chip;
  final String label;
  final String detail;
  final int state;
  final double opacity;
  final bool focused;
  final Duration fade;
  final ValueChanged<bool> onHover;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final node = chip.chip.node;
    final shownNode = node.label == label && node.detail == detail
        ? node
        : (node.deepCopy()
            ..label = label
            ..detail = detail);
    return IgnorePointer(
      ignoring: state == hidden,
      child: AnimatedOpacity(
        duration: fade,
        opacity: opacity,
        child: MouseRegion(
          cursor: SystemMouseCursors.click,
          onEnter: (_) => onHover(true),
          onExit: (_) => onHover(false),
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTap: onTap,
            child: Stack(
              fit: StackFit.expand,
              children: [
                MapChipView(
                  chip: MapChip(node: shownNode, rect: chip.rect),
                  focused: focused,
                ),
                if (state == dotted)
                  CustomPaint(
                    painter: _DashedOutline(
                      palette.textFaint,
                      Radii.control,
                      fill: palette.canvas,
                    ),
                  ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// A pipe's pad: its portal and, with room, its name.
class _PadView extends StatelessWidget {
  const _PadView({required this.label});

  final String label;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Container(
      // Unnamed, only its portal, in a pill its own height.
      padding: label.isEmpty
          ? EdgeInsets.zero
          : const EdgeInsets.symmetric(horizontal: Space.xs),
      decoration: BoxDecoration(
        // The pipe's color washed in, so it reads as a way out of the board
        // even small (Bruno 44370), as the Map's pad does.
        color: Color.alphaBlend(
          pipeColor.withValues(alpha: 0.12),
          palette.canvas,
        ),
        borderRadius: BorderRadius.circular(padHeight / 2),
        border: Border.all(color: pipeColor, width: 1.5),
      ),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          const PortalGlyph(size: 12),
          if (label.isNotEmpty) ...[
            const SizedBox(width: Space.xs),
            Flexible(
              child: Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyles.caption.copyWith(
                  color: palette.text,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ),
          ],
        ],
      ),
    );
  }
}

/// A box as the Map paints it: the workspace or a project as a board with
/// its title and path, a folder, the vault or the apps as a region.
class _BoxPainter extends CustomPainter {
  _BoxPainter(this.box, this.palette, {required this.dotted, this.words});

  final MapBox box;
  final Palette palette;
  final bool dotted;

  /// A share's title and path in place of the box's own.
  final (String, String)? words;

  @override
  void paint(Canvas canvas, Size size) {
    final rect = Offset.zero & size;
    final board =
        box.kind == MapBoxKind.workspace || box.kind == MapBoxKind.project;
    if (!board && !box.outlined) return;
    final radius = board ? Radii.panel : Radii.control;
    final shape = RRect.fromRectAndRadius(rect, Radius.circular(radius));
    final fill =
        board || box.kind == MapBoxKind.vault || box.kind == MapBoxKind.apps
        ? palette.sidebar
        : palette.canvas.withValues(alpha: 0.6);
    canvas.drawRRect(shape, Paint()..color = fill);
    if (dotted) {
      _dashRRect(canvas, shape, _stroke(palette.textFaint, 1));
    } else {
      canvas.drawRRect(shape, _stroke(palette.hairline, 1));
    }
    if (board) {
      const inset = mapCell * 4;
      _text(
        canvas,
        words?.$1 ?? box.label,
        TextStyles.label.copyWith(
          color: palette.text,
          fontWeight: FontWeight.w600,
        ),
        const Offset(inset, mapCell + 1),
        rect.width - inset - mapCell,
      );
      _text(
        canvas,
        words?.$2 ?? box.detail,
        TextStyles.caption.copyWith(color: palette.textFaint),
        const Offset(inset, mapCell * 3 + 2),
        rect.width - inset - mapCell,
      );
      return;
    }
    _text(
      canvas,
      words?.$1 ?? box.label,
      TextStyles.caption.copyWith(
        color: palette.textMuted,
        fontWeight: FontWeight.w500,
      ),
      const Offset(mapCell, mapCell - 2),
      rect.width - mapCell * 2,
    );
  }

  @override
  bool shouldRepaint(_BoxPainter old) =>
      old.box != box ||
      old.palette != palette ||
      old.dotted != dotted ||
      old.words != words;
}

/// The lines shown, in their layers' colors; dashed where reconstructed.
/// With a chip pointed at, its lines in full and the rest dimmed.
class _LinesPainter extends CustomPainter {
  _LinesPainter({
    required this.scene,
    required this.frame,
    required this.palette,
    required this.focus,
  });

  final HistoryScene scene;
  final SceneFrame frame;
  final Palette palette;
  final int? focus;

  @override
  void paint(Canvas canvas, Size size) {
    final focus = this.focus;
    bool lit(SceneWire wire) =>
        focus == null ||
        wire.a == focus ||
        wire.b == focus ||
        (wire.a < 0 && wire.ends.contains(focus));

    // A pipe's line leaves its pad up out of the board, ending in a ring.
    for (var i = 0; i < scene.pads.length; i++) {
      if (frame.pads[i] == hidden) continue;
      final rect = scene.pads[i].rect;
      final color = focus == null
          ? pipeColor
          : pipeColor.withValues(alpha: 0.3);
      final top = pipeTop(rect);
      final end = top - const Offset(0, pipeReach);
      canvas
        ..drawLine(top, end, _stroke(color, 2))
        ..drawCircle(end, 3.5, Paint()..color = palette.canvas)
        ..drawCircle(end, 3.5, _stroke(color, 2));
    }

    for (final pass in [false, true]) {
      for (var i = 0; i < scene.wires.length; i++) {
        final state = frame.wires[i];
        if (state == hidden) continue;
        final wire = scene.wires[i];
        if (lit(wire) != pass) continue;
        final color = mapLayerColor(wire.wire.layer, palette);
        final paint = _stroke(
          pass ? color : color.withValues(alpha: 0.22),
          wire.wire.bus
              ? 2.5
              : focus != null && pass
              ? 1.75
              : 1.25,
        )..strokeCap = StrokeCap.square;
        final points = wire.wire.points;
        if (state == dotted) {
          paint.strokeCap = StrokeCap.butt;
          _dashPolyline(canvas, points, paint);
        } else {
          final path = Path()..moveTo(points.first.dx, points.first.dy);
          for (final point in points.skip(1)) {
            path.lineTo(point.dx, point.dy);
          }
          canvas.drawPath(path, paint);
        }
        for (final arrow in wire.wire.arrows) {
          _chevron(canvas, arrow, paint.color);
        }
      }
    }
    for (var i = 0; i < scene.dots.length; i++) {
      if (frame.dots[i] == hidden) continue;
      final dot = scene.dots[i].dot;
      final color = mapLayerColor(dot.layer, palette);
      canvas.drawCircle(
        dot.at,
        2.75,
        Paint()
          ..color = focus == null || dot.ends.contains(_idOf(focus))
              ? color
              : color.withValues(alpha: 0.22),
      );
    }
  }

  String _idOf(int chip) => scene.chips[chip].id;

  @override
  bool shouldRepaint(_LinesPainter old) =>
      old.scene != scene ||
      old.frame != frame ||
      old.palette != palette ||
      old.focus != focus;
}

/// Agents working, glowing in their color; an agent renamed lights up for
/// a moment.
class _GlowPainter extends CustomPainter {
  _GlowPainter(this.controller, this.palette, {required this.reduced})
    : super(repaint: controller.time);

  final HistoryController controller;
  final Palette palette;
  final bool reduced;

  @override
  void paint(Canvas canvas, Size size) {
    final scene = controller.scene;
    final timeline = controller.timeline;
    if (scene == null || timeline == null) return;
    final t = controller.time.value;
    final now = controller.playAt(t);
    final project = controller.project;
    bool out(int chip) =>
        project != null && !scene.chipInProject(chip, t, project);
    for (final node in controller.log.runStarts.keys) {
      if (!controller.log.running(node, t)) continue;
      final chip = scene.chipAt(node, t);
      if (chip < 0 || out(chip)) continue;
      // A node newer than the overview isn't drawn until it's read again.
      if (node > timeline.nodes.length ||
          spanAt(timeline.nodeSpans[node - 1], t) == null) {
        continue;
      }
      final color = agentColor(scene.chips[chip].chip.node.color);
      _glow(canvas, scene.chips[chip].rect, color, 1);
    }
    for (var node = 1; node <= timeline.nodes.length; node++) {
      final renamed = timeline.renamedAt(node, t);
      if (renamed == null) continue;
      final age = now - controller.playAt(renamed);
      if (age < 0 || age > 1.4) continue;
      final chip = scene.chipAt(node, t);
      if (chip < 0 || out(chip)) continue;
      // In over 0.3 s, held 0.3 s, out over 0.8 s, as a highlight is.
      final strength = reduced
          ? 0.7
          : age < 0.3
          ? Curves.easeOut.transform(age / 0.3)
          : age < 0.6
          ? 1.0
          : 1 - Curves.easeInOut.transform((age - 0.6) / 0.8);
      final color = agentColor(scene.chips[chip].chip.node.color);
      _glow(canvas, scene.chips[chip].rect, color, strength * 1.6);
    }
  }

  void _glow(Canvas canvas, Rect rect, Color color, double strength) {
    final shape = RRect.fromRectAndRadius(
      rect,
      const Radius.circular(Radii.control),
    );
    canvas
      ..drawRRect(
        shape.inflate(2),
        Paint()
          ..color = color.withValues(alpha: (0.25 * strength).clamp(0, 1))
          ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 8),
      )
      ..drawRRect(
        shape.inflate(1),
        _stroke(color.withValues(alpha: (0.6 * strength).clamp(0, 1)), 1.5),
      );
  }

  @override
  bool shouldRepaint(_GlowPainter old) =>
      old.controller != controller || old.palette != palette;
}

/// The pulses and ticks at the playhead.
class _PulsePainter extends CustomPainter {
  _PulsePainter(this.controller, this.palette, {required this.budget})
    : super(repaint: controller.time);

  final HistoryController controller;
  final Palette palette;
  final int budget;

  /// Counts' words, by count and color: the same few, frame after frame.
  static final _counts = <(int, int), TextPainter>{};

  @override
  void paint(Canvas canvas, Size size) {
    final scene = controller.scene;
    final routes = controller.routes;
    if (scene == null || routes == null) return;
    final (:pulses, :ticks) = pulsesAt(
      scene: scene,
      log: controller.log,
      routes: routes,
      playAt: controller.playAt,
      historyAt: controller.historyAt,
      t: controller.time.value,
      budget: budget,
      project: controller.project,
    );
    for (final tick in ticks) {
      final rect = scene.chips[tick.chip].rect;
      final at = rect.topRight + const Offset(-mapCell, mapCell);
      canvas.drawCircle(
        at,
        2 + 6 * tick.age,
        _stroke(palette.textMuted.withValues(alpha: 1 - tick.age), 1.25),
      );
    }
    for (final pulse in pulses) {
      // Through a pipe: never drawn over its pad, which brightens as the
      // message crosses it (Lucy).
      final padRect = pulse.pad >= 0 && pulse.pad < scene.pads.length
          ? scene.pads[pulse.pad].rect
          : null;
      if (padRect != null) {
        final seconds = pulseSeconds(pulse.route.length);
        final ring = pipeTop(padRect) - const Offset(0, pipeReach);
        final inward = (pulse.route.start - ring).distance < 1;
        final crossing = pulse.route.length <= 0
            ? 0.0
            : pipeReach / pulse.route.length;
        final at = inward ? crossing : 1 - crossing;
        final away = (pulse.at - at).abs() * seconds;
        if (away < 0.15) {
          // Its portal: the pad as a video without names draws it too.
          final top = pipeTop(padRect);
          canvas.drawRRect(
            RRect.fromRectAndRadius(
              Rect.fromLTWH(
                top.dx - padHeight / 2,
                padRect.top,
                padHeight,
                padHeight,
              ),
              const Radius.circular(padHeight / 2),
            ),
            Paint()
              ..color = pipeColor.withValues(alpha: 0.12 * (1 - away / 0.15)),
          );
        }
      }
      bool over(Offset point) =>
          padRect != null && padRect.deflate(0.5).contains(point);
      final color = switch (pulse.kind) {
        PulseKind.agent => mapLayerColor(MapLayer.messages, palette),
        PulseKind.person => palette.text,
        PulseKind.asks => palette.signal,
        PulseKind.pipe => pipeColor,
        PulseKind.app => mapLayerColor(MapLayer.apps, palette),
      };
      final pipe = pulse.kind == PulseKind.pipe;
      final strength = pulse.quiet && !pipe ? 0.55 : 1.0;
      final radius =
          (pipe
              ? 3.5
              : pulse.quiet
              ? 2.0
              : 3.0) +
          (pulse.count > 1 ? 1.2 * math.log(pulse.count) / math.ln2 : 0);
      // Through a pipe: a halo, so a message to or from another Claktory
      // is noticed (Bruno 44370).
      if (pipe && !over(pulse.point)) {
        canvas.drawCircle(
          pulse.point,
          math.min(radius, 8) * 2.4,
          Paint()..color = pipeColor.withValues(alpha: 0.22),
        );
      }
      // A short tail behind it.
      for (var k = 3; k >= 1; k--) {
        final behind = pulse.at - 0.025 * k;
        if (behind < 0 || over(pulse.route.at(behind))) continue;
        canvas.drawCircle(
          pulse.route.at(behind),
          radius * (1 - 0.18 * k),
          Paint()..color = color.withValues(alpha: strength * (0.5 - 0.12 * k)),
        );
      }
      final point = pulse.point;
      if (over(point)) continue;
      canvas.drawCircle(
        point,
        math.min(radius, 8),
        Paint()..color = color.withValues(alpha: strength),
      );
      if (pulse.count > 1) {
        final text = _counts.putIfAbsent(
          (pulse.count, color.toARGB32()),
          () => TextPainter(
            text: TextSpan(
              text: '${pulse.count}',
              style: TextStyles.caption.copyWith(
                color: color,
                fontSize: 10,
                fontWeight: FontWeight.w600,
                fontFeatures: const [ui.FontFeature.tabularFigures()],
              ),
            ),
            textDirection: TextDirection.ltr,
          )..layout(),
        );
        text.paint(canvas, point + Offset(radius + 2, -text.height / 2));
      }
    }
  }

  @override
  bool shouldRepaint(_PulsePainter old) =>
      old.controller != controller ||
      old.palette != palette ||
      old.budget != budget;
}

/// A dashed rounded outline, over a chip's own.
class _DashedOutline extends CustomPainter {
  _DashedOutline(this.color, this.radius, {required this.fill});

  final Color color;
  final double radius;
  final Color fill;

  @override
  void paint(Canvas canvas, Size size) {
    final shape = RRect.fromRectAndRadius(
      (Offset.zero & size).deflate(0.5),
      Radius.circular(radius),
    );
    // Covers the solid hairline, then dashes it.
    canvas.drawRRect(shape, _stroke(fill, 1.5));
    _dashRRect(canvas, shape, _stroke(color, 1));
  }

  @override
  bool shouldRepaint(_DashedOutline old) =>
      old.color != color || old.radius != radius || old.fill != fill;
}

Paint _stroke(Color color, double width) => Paint()
  ..color = color
  ..style = PaintingStyle.stroke
  ..strokeWidth = width
  ..strokeJoin = StrokeJoin.miter;

const _dash = 4.0;
const _gap = 3.0;

void _dashRRect(Canvas canvas, RRect shape, Paint paint) {
  final path = Path()..addRRect(shape);
  for (final metric in path.computeMetrics()) {
    var at = 0.0;
    while (at < metric.length) {
      canvas.drawPath(metric.extractPath(at, at + _dash), paint);
      at += _dash + _gap;
    }
  }
}

void _dashPolyline(Canvas canvas, List<Offset> points, Paint paint) {
  const period = _dash + _gap;
  // Where in the dash pattern each stretch starts, so dashes run on
  // round corners.
  var phase = 0.0;
  for (var i = 0; i + 1 < points.length; i++) {
    final a = points[i];
    final b = points[i + 1];
    final length = (b - a).distance;
    if (length == 0) continue;
    final step = (b - a) / length;
    var at = 0.0;
    while (at < length) {
      final dash = phase < _dash;
      final to = math.min(length, at + (dash ? _dash - phase : period - phase));
      if (dash) canvas.drawLine(a + step * at, a + step * to, paint);
      phase = (phase + to - at) % period;
      at = to;
    }
  }
}

void _chevron(Canvas canvas, MapArrow arrow, Color color) {
  final d = arrow.direction;
  final side = Offset(-d.dy, d.dx);
  final tip = arrow.at + d * 3;
  final path = Path()
    ..moveTo((tip - d * 5 + side * 4).dx, (tip - d * 5 + side * 4).dy)
    ..lineTo(tip.dx, tip.dy)
    ..lineTo((tip - d * 5 - side * 4).dx, (tip - d * 5 - side * 4).dy);
  canvas.drawPath(
    path,
    _stroke(color, 1.5)
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round,
  );
}

void _text(
  Canvas canvas,
  String text,
  TextStyle style,
  Offset at,
  double width,
) {
  if (text.isEmpty || width <= 0) return;
  final painter = TextPainter(
    text: TextSpan(text: text, style: style),
    textDirection: TextDirection.ltr,
    maxLines: 1,
    ellipsis: '…',
  )..layout(maxWidth: width);
  painter.paint(canvas, at);
  painter.dispose();
}
