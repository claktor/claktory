import 'dart:math' as math;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart' show fireAndLog;
import 'canvas.dart';
import 'controls.dart';
import 'identity.dart';
import 'map_layout.dart';
import 'pane_chrome.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart';

/// The color of a [MapLayer]'s lines. Not agent colors, which say whose
/// something is, and not signal, which marks selection: four hues of their
/// own, a little lighter in the dark.
Color mapLayerColor(MapLayer layer, Palette palette) {
  final dark = palette.canvas.computeLuminance() < 0.5;
  return switch (layer) {
    MapLayer.messages =>
      dark ? const Color(0xFF5AA6E8) : const Color(0xFF1D6FC2),
    MapLayer.secrets =>
      dark ? const Color(0xFFE2A93E) : const Color(0xFFA66A00),
    MapLayer.hierarchy => palette.textFaint,
    MapLayer.data => dark ? const Color(0xFF52B985) : const Color(0xFF16804B),
  };
}

/// What each layer is called in the key.
const mapLayerNames = {
  MapLayer.messages: 'Messages',
  MapLayer.secrets: 'Secrets',
  MapLayer.hierarchy: 'Under other agents',
  MapLayer.data: "Claktory's records",
};

/// The Map: how this Claktory is arranged, drawn like a circuit board.
/// Rivet and the other admins along the top, then the workspace and each
/// project as a board with its folders and agents, and the vault along the
/// bottom; lines show who can message whom, who has which secret and who
/// reads Claktory's records. Hover an agent to light up its lines; click it
/// to open its conversation.
class MapView extends StatefulWidget {
  const MapView({super.key});

  @override
  State<MapView> createState() => _MapViewState();
}

class _MapViewState extends State<MapView> {
  final _canvas = CanvasController();
  final _hidden = <MapLayer>{};
  String? _hover;
  String? _pinned;

  ClaktoryMap? _laidOut;
  Map<String, int>? _order;
  var _layout = MapLayout.empty;

  @override
  void dispose() {
    _canvas.dispose();
    super.dispose();
  }

  /// Lays the map out again only when it changed: the workspace comes
  /// again with every agent's every step.
  MapLayout _layoutFor(Workspace workspace) {
    final order = sidebarOrder(workspace);
    if (_laidOut != workspace.map || !_sameOrder(order)) {
      _laidOut = workspace.map;
      _order = order;
      _layout = layoutMap(workspace.map, order: order);
    }
    return _layout;
  }

  bool _sameOrder(Map<String, int> order) {
    final old = _order;
    if (old == null || old.length != order.length) return false;
    for (final MapEntry(:key, :value) in order.entries) {
      if (old[key] != value) return false;
    }
    return true;
  }

  void _open(MapNode node) {
    if (node.kind != MapNodeKind.MAP_NODE_KIND_AGENT) {
      // Anything else lights up its lines, and off again.
      setState(() => _pinned = _pinned == node.id ? null : node.id);
      return;
    }
    final backend = AppScope.of(context);
    AppScope.closePage(context);
    fireAndLog(backend.selectAgent(node.refId));
    // On a phone, its conversation is pushed over the list.
    PhoneNavigation.of(context)?.value = true;
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final inPane = InPane.of(context);
    final phone = isPhoneLayout(context);
    void close() => AppScope.closePage(context);
    return CallbackShortcuts(
      bindings: {const SingleActivator(LogicalKeyboardKey.escape): close},
      child: FocusScope(
        autofocus: !inPane,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (!inPane)
              WindowDrag(
                child: Container(
                  height: titleBarHeight,
                  padding: const EdgeInsets.only(
                    left: Space.xl,
                    right: Space.m,
                  ),
                  decoration: BoxDecoration(
                    border: Border(bottom: BorderSide(color: palette.hairline)),
                  ),
                  child: Row(
                    children: [
                      Text(
                        'Map',
                        style: TextStyles.label.copyWith(
                          color: palette.text,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                      const Spacer(),
                      Button(label: 'Done', onPressed: close),
                    ],
                  ),
                ),
              ),
            Expanded(
              child: ColoredBox(
                color: palette.canvas,
                child: ListenableBuilder(
                  listenable: backend.workspace,
                  builder: (context, _) {
                    final workspace = backend.workspace.workspace;
                    if (!workspace.hasMap() || workspace.map.nodes.isEmpty) {
                      return Center(
                        child: Padding(
                          padding: const EdgeInsets.all(Space.xl),
                          child: Text(
                            'Update Claktory to see its map.',
                            textAlign: TextAlign.center,
                            style: TextStyles.body.copyWith(
                              color: palette.textMuted,
                            ),
                          ),
                        ),
                      );
                    }
                    final layout = _layoutFor(workspace);
                    final focus = _hover ?? _pinned;
                    return Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        _Key(
                          layout: layout,
                          hidden: _hidden,
                          phone: phone,
                          canvas: _canvas,
                          onToggle: (layer) => setState(() {
                            if (!_hidden.remove(layer)) _hidden.add(layer);
                          }),
                        ),
                        Expanded(
                          child: ZoomCanvas(
                            controller: _canvas,
                            size: layout.size,
                            background: (transform) =>
                                _GridPainter(transform, palette.hairline),
                            child: _Board(
                              layout: layout,
                              hidden: _hidden,
                              focus: focus,
                              onHover: (id) => setState(() => _hover = id),
                              onTap: _open,
                            ),
                          ),
                        ),
                      ],
                    );
                  },
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// The board at its own size: boxes and lines painted, chips as widgets
/// to hover and click.
class _Board extends StatelessWidget {
  const _Board({
    required this.layout,
    required this.hidden,
    required this.focus,
    required this.onHover,
    required this.onTap,
  });

  final MapLayout layout;
  final Set<MapLayer> hidden;
  final String? focus;
  final ValueChanged<String?> onHover;
  final ValueChanged<MapNode> onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final focus = this.focus;
    final lit = focus == null ? null : {focus, ...?layout.related[focus]};
    return Stack(
      children: [
        Positioned.fill(
          child: RepaintBoundary(
            child: CustomPaint(
              painter: _BoardPainter(
                layout: layout,
                palette: palette,
                hidden: hidden,
                focus: focus,
              ),
            ),
          ),
        ),
        for (final chip in layout.chips)
          Positioned.fromRect(
            rect: chip.rect,
            child: AnimatedOpacity(
              duration: Motion.fast,
              opacity: lit == null || lit.contains(chip.id) ? 1 : 0.3,
              child: MouseRegion(
                cursor: SystemMouseCursors.click,
                onEnter: (_) => onHover(chip.id),
                onExit: (_) {
                  if (focus == chip.id) onHover(null);
                },
                child: GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTap: () => onTap(chip.node),
                  child: _Chip(chip: chip, focused: focus == chip.id),
                ),
              ),
            ),
          ),
      ],
    );
  }
}

/// A node on the board: an agent (its color down the left, its name, role
/// and badges), a secret, or Claktory's records.
class _Chip extends StatelessWidget {
  const _Chip({required this.chip, required this.focused});

  final MapChip chip;
  final bool focused;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final node = chip.node;
    final agent = node.kind == MapNodeKind.MAP_NODE_KIND_AGENT;
    final border = BoxDecoration(
      color: palette.canvas,
      borderRadius: BorderRadius.circular(Radii.control),
      border: Border.all(color: focused ? palette.textMuted : palette.hairline),
    );
    if (!agent) {
      final secret = node.kind == MapNodeKind.MAP_NODE_KIND_SECRET;
      final color = mapLayerColor(
        secret ? MapLayer.secrets : MapLayer.data,
        palette,
      );
      return Container(
        decoration: border,
        padding: const EdgeInsets.symmetric(horizontal: Space.s),
        child: Row(
          children: [
            ExtensionGlyph(
              icon: secret ? 'key-round' : 'database',
              name: '',
              color: color,
              size: 14,
            ),
            const SizedBox(width: Space.s - 2),
            Expanded(
              child: Text(
                node.label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyles.caption.copyWith(
                  color: palette.text,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ),
          ],
        ),
      );
    }
    final color = agentColor(node.color);
    return Container(
      decoration: border,
      clipBehavior: Clip.antiAlias,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Container(width: 3, color: color),
          Expanded(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(
                Space.s,
                Space.xs + 1,
                Space.s,
                Space.xs,
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  Text(
                    node.label,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyles.label.copyWith(
                      color: palette.text,
                      height: 1.3,
                    ),
                  ),
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          node.detail,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyles.caption.copyWith(
                            color: palette.textMuted,
                            height: 1.3,
                          ),
                        ),
                      ),
                      for (final badge in node.badges) ...[
                        const SizedBox(width: Space.xs),
                        ExtensionGlyph(
                          icon: badge.icon,
                          name: badge.label,
                          color: _quiet.contains(badge.key)
                              ? palette.textFaint
                              : palette.textMuted,
                          size: 12,
                        ),
                      ],
                    ],
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// Badges most agents have (both are on by default): fainter than the rest.
const _quiet = {'web', 'tree'};

/// Boxes, labels, lines, dots and chevrons.
class _BoardPainter extends CustomPainter {
  _BoardPainter({
    required this.layout,
    required this.palette,
    required this.hidden,
    required this.focus,
  }) : hiddenKey = Set.of(hidden);

  final MapLayout layout;
  final Palette palette;
  final Set<MapLayer> hidden;
  final Set<MapLayer> hiddenKey;
  final String? focus;

  @override
  void paint(Canvas canvas, Size size) {
    final focus = this.focus;
    final reads = focus == null ? null : layout.reads[focus];
    final writes = focus == null ? null : layout.writes[focus];
    final data = mapLayerColor(MapLayer.data, palette);

    for (final box in layout.boxes) {
      final rect = box.rect;
      switch (box.kind) {
        case MapBoxKind.workspace || MapBoxKind.project:
          final shape = RRect.fromRectAndRadius(
            rect,
            const Radius.circular(Radii.panel),
          );
          canvas.drawRRect(shape, Paint()..color = palette.sidebar);
          canvas.drawRRect(shape, _stroke(palette.hairline, 1));
          _text(
            canvas,
            box.label,
            TextStyles.label.copyWith(
              color: palette.text,
              fontWeight: FontWeight.w600,
            ),
            Offset(rect.left + _titleInset(box), rect.top + mapCell + 1),
            rect.width - _titleInset(box) - mapCell,
          );
          _text(
            canvas,
            box.detail,
            TextStyles.caption.copyWith(color: palette.textFaint),
            Offset(rect.left + _titleInset(box), rect.top + mapCell * 3 + 2),
            rect.width - _titleInset(box) - mapCell,
          );
        case MapBoxKind.folder || MapBoxKind.vault || MapBoxKind.admins:
          final writing = writes?.contains(box.id) ?? false;
          final reading = reads?.contains(box.id) ?? false;
          if (!box.outlined) {
            if (writing) {
              // A project's own folder with no other: the project's box.
              final whole = layout.boxes.firstWhere(
                (other) =>
                    other.id == box.id && other.kind == MapBoxKind.project,
                orElse: () => box,
              );
              canvas.drawRRect(
                RRect.fromRectAndRadius(
                  whole.rect.deflate(1),
                  const Radius.circular(Radii.panel - 1),
                ),
                _stroke(data, 1.5),
              );
            }
            continue;
          }
          final shape = RRect.fromRectAndRadius(
            rect,
            const Radius.circular(Radii.control),
          );
          canvas.drawRRect(
            shape,
            Paint()
              ..color = box.kind == MapBoxKind.vault
                  ? palette.sidebar
                  : palette.canvas.withValues(alpha: 0.6),
          );
          canvas.drawRRect(
            shape,
            writing
                ? _stroke(data, 1.5)
                : reading
                ? _stroke(data.withValues(alpha: 0.55), 1)
                : _stroke(palette.hairline, 1),
          );
          _text(
            canvas,
            box.label,
            TextStyles.caption.copyWith(
              color: writing || reading ? data : palette.textMuted,
              fontWeight: FontWeight.w500,
            ),
            Offset(rect.left + mapCell, rect.top + mapCell - 2),
            rect.width - mapCell * 2,
          );
      }
    }

    // Lines: the dimmed first, so the lit ones run over them.
    bool shown(MapLayer layer) => !hidden.contains(layer);
    bool lit(Set<String> ends) => focus == null || ends.contains(focus);
    for (final pass in [false, true]) {
      for (final wire in layout.wires) {
        if (!shown(wire.layer) || lit(wire.ends) != pass) continue;
        final color = mapLayerColor(wire.layer, palette);
        final paint = _stroke(
          pass ? color : color.withValues(alpha: 0.22),
          wire.bus
              ? 2.5
              : focus != null && pass
              ? 1.75
              : 1.25,
        )..strokeCap = StrokeCap.square;
        final path = Path()..moveTo(wire.points.first.dx, wire.points.first.dy);
        for (final point in wire.points.skip(1)) {
          path.lineTo(point.dx, point.dy);
        }
        canvas.drawPath(path, paint);
        for (final arrow in wire.arrows) {
          _chevron(canvas, arrow, paint.color);
        }
      }
      for (final dot in layout.dots) {
        if (!shown(dot.layer) || lit(dot.ends) != pass) continue;
        final color = mapLayerColor(dot.layer, palette);
        canvas.drawCircle(
          dot.at,
          2.75,
          Paint()..color = pass ? color : color.withValues(alpha: 0.22),
        );
      }
    }
  }

  static double _titleInset(MapBox box) {
    // Titles line up with the chips, right of the bus.
    return box.kind == MapBoxKind.workspace || box.kind == MapBoxKind.project
        ? mapCell * 4
        : mapCell;
  }

  static Paint _stroke(Color color, double width) => Paint()
    ..color = color
    ..style = PaintingStyle.stroke
    ..strokeWidth = width
    ..strokeJoin = StrokeJoin.miter;

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

  static void _text(
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

  @override
  bool shouldRepaint(_BoardPainter old) =>
      old.layout != layout ||
      old.palette != palette ||
      old.focus != focus ||
      !old.hiddenKey.containsAll(hidden) ||
      old.hiddenKey.length != hidden.length;
}

/// A faint dot every two cells, like a board's grid, at every zoom.
class _GridPainter extends CustomPainter {
  _GridPainter(this.transform, this.color);

  final Matrix4 transform;
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final scale = transform.getMaxScaleOnAxis();
    var step = mapCell * 2 * scale;
    // Never denser than every 12 pixels on screen.
    while (step < 12) {
      step *= 2;
    }
    final origin = Offset(transform.storage[12], transform.storage[13]);
    final x0 = origin.dx % step;
    final y0 = origin.dy % step;
    final paint = Paint()..color = color;
    final radius = math.max(0.6, math.min(1.0, scale));
    for (var y = y0; y < size.height; y += step) {
      for (var x = x0; x < size.width; x += step) {
        canvas.drawCircle(Offset(x, y), radius, paint);
      }
    }
  }

  @override
  bool shouldRepaint(_GridPainter old) =>
      old.transform != transform || old.color != color;
}

/// The key, along the top: a switch for each kind of line, with its color,
/// then the zoom; under them, what the badges on agents mean.
class _Key extends StatelessWidget {
  const _Key({
    required this.layout,
    required this.hidden,
    required this.phone,
    required this.canvas,
    required this.onToggle,
  });

  final MapLayout layout;
  final Set<MapLayer> hidden;
  final bool phone;
  final CanvasController canvas;
  final ValueChanged<MapLayer> onToggle;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final layers = {for (final wire in layout.wires) wire.layer};
    final badges = <String, MapBadge>{};
    for (final chip in layout.chips) {
      for (final badge in chip.node.badges) {
        badges.putIfAbsent(badge.key, () => badge);
      }
    }
    final switches = [
      for (final layer in MapLayer.values)
        if (layers.contains(layer))
          Padding(
            padding: const EdgeInsets.only(right: Space.xs),
            child: _LayerSwitch(
              layer: layer,
              shown: !hidden.contains(layer),
              onToggle: () => onToggle(layer),
            ),
          ),
    ];
    final caption = TextStyles.caption.copyWith(color: palette.textFaint);
    return Unselectable(
      child: Container(
        padding: const EdgeInsets.fromLTRB(Space.m, Space.s, Space.m, Space.s),
        decoration: BoxDecoration(
          border: Border(bottom: BorderSide(color: palette.hairline)),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Expanded(
                  child: SingleChildScrollView(
                    scrollDirection: Axis.horizontal,
                    child: Row(children: switches),
                  ),
                ),
                if (!phone) _ZoomControls(controller: canvas),
              ],
            ),
            // On a phone, room for the board: the switches only.
            if (!phone) ...[
              const SizedBox(height: Space.xs),
              Wrap(
                spacing: Space.m,
                runSpacing: Space.xs,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  for (final badge in badges.values)
                    Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        ExtensionGlyph(
                          icon: badge.icon,
                          name: badge.label,
                          color: palette.textFaint,
                          size: 12,
                        ),
                        const SizedBox(width: Space.xs),
                        Text(badge.label, style: caption),
                      ],
                    ),
                  Text(
                    'Agents can read their whole project but change files '
                    'only in their own folder. Hover an agent to see which.',
                    style: caption,
                  ),
                ],
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// A kind of line, its color, and whether it shows: a quiet pill that's
/// tinted while on.
class _LayerSwitch extends StatelessWidget {
  const _LayerSwitch({
    required this.layer,
    required this.shown,
    required this.onToggle,
  });

  final MapLayer layer;
  final bool shown;
  final VoidCallback onToggle;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final color = mapLayerColor(layer, palette);
    return Semantics(
      toggled: shown,
      label: mapLayerNames[layer],
      excludeSemantics: true,
      onTap: onToggle,
      child: Pressable(
        onTap: onToggle,
        builder: (context, hovered) => AnimatedContainer(
          duration: Motion.fast,
          height: 28,
          padding: const EdgeInsets.symmetric(horizontal: Space.s),
          decoration: BoxDecoration(
            color: shown
                ? palette.selected
                : hovered
                ? palette.hover
                : null,
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              _Swatch(
                color: shown ? color : color.withValues(alpha: 0.3),
                bus: layer == MapLayer.messages,
              ),
              const SizedBox(width: Space.s - 2),
              Text(
                mapLayerNames[layer]!,
                style: TextStyles.caption.copyWith(
                  color: shown
                      ? palette.text
                      : hovered
                      ? palette.textMuted
                      : palette.textFaint,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A short stretch of a layer's line: a bus with a tap, for messages.
class _Swatch extends StatelessWidget {
  const _Swatch({required this.color, this.bus = false});

  final Color color;
  final bool bus;

  @override
  Widget build(BuildContext context) => CustomPaint(
    size: const Size(20, 12),
    painter: _SwatchPainter(color, bus),
  );
}

class _SwatchPainter extends CustomPainter {
  const _SwatchPainter(this.color, this.bus);

  final Color color;
  final bool bus;

  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.25;
    final y = size.height / 2;
    if (bus) {
      canvas.drawLine(
        Offset(0, y - 3),
        Offset(size.width, y - 3),
        paint..strokeWidth = 2.5,
      );
      canvas.drawLine(
        Offset(size.width / 2, y - 3),
        Offset(size.width / 2, size.height),
        paint..strokeWidth = 1.25,
      );
      canvas.drawCircle(
        Offset(size.width / 2, y - 3),
        2.75,
        Paint()..color = color,
      );
      return;
    }
    canvas.drawLine(Offset(0, y), Offset(size.width, y), paint);
  }

  @override
  bool shouldRepaint(_SwatchPainter old) =>
      old.color != color || old.bus != bus;
}

/// Zoom out, fit and zoom in, over the board's bottom right.
class _ZoomControls extends StatelessWidget {
  const _ZoomControls({required this.controller});

  final CanvasController controller;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Unselectable(
      child: SizedBox(
        child: ListenableBuilder(
          listenable: controller,
          builder: (context, _) => Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              GlyphButton(
                label: 'Zoom out',
                active: false,
                onTap: controller.zoomOut,
                glyph: (color) =>
                    ExtensionGlyph(icon: 'minus', name: '', color: color),
              ),
              Pressable(
                onTap: controller.fit,
                builder: (context, hovered) => Container(
                  height: 28,
                  constraints: const BoxConstraints(minWidth: 48),
                  alignment: Alignment.center,
                  padding: const EdgeInsets.symmetric(horizontal: Space.xs),
                  decoration: BoxDecoration(
                    color: hovered ? palette.hover : null,
                    borderRadius: BorderRadius.circular(Radii.control),
                  ),
                  child: Text(
                    controller.fitted
                        ? 'Fit'
                        : '${(controller.scale * 100).round()}%',
                    style: TextStyles.caption.copyWith(
                      color: hovered ? palette.text : palette.textMuted,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    ),
                  ),
                ),
              ),
              GlyphButton(
                label: 'Zoom in',
                active: false,
                onTap: controller.zoomIn,
                glyph: (color) =>
                    ExtensionGlyph(icon: 'plus', name: '', color: color),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
