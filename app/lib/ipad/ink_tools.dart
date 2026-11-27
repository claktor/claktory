// The iPad's drawing tools, Claktory's own (docs/ipad-workbench.md §6):
// PencilKit's tool picker takes the keyboard's focus from the message box
// when it shows (docs/ipad-ink-spike.md), and its compact form can lose
// Undo and Redo, so a document pane draws its tools in Flutter and sends
// the choice to its ink surface.
import 'package:flutter/widgets.dart';

import '../controls.dart' show Pressable, svgPath;
import '../identity.dart';
import '../mobile/metrics.dart';
import '../mobile/popover.dart' show presentPopover;
import 'ink_bridge.dart' show InkTool, InkToolKind;
import 'workbench.dart' show ToolButton;

/// What a stroke does.
enum DrawingTool {
  pen('Pen', 3),
  pencil('Pencil', 2),
  marker('Highlighter', 12),
  eraser('Eraser', 12),
  lasso('Lasso', 0);

  const DrawingTool(this.label, this.width);

  /// Its name on screen and to VoiceOver.
  final String label;

  /// Its width in points until the person changes it.
  final double width;

  /// Whether it draws in a color.
  bool get inks => this == pen || this == pencil || this == marker;

  /// The bridge's kind for it.
  InkToolKind get kind => InkToolKind.values.byName(name);

  /// Its name on the wire (`setTool`'s `tool`).
  String get wire => name;

  static DrawingTool of(InkToolKind kind) => values.byName(kind.name);
}

/// An ink color: fixed, never following light or dark, so a mark on a
/// white page reads the same on every device (ipad.md §2).
class InkColor {
  const InkColor(this.label, this.argb);

  final String label;
  final int argb;

  Color get color => Color(argb);
}

/// The pens' colors.
const penColors = [
  InkColor('Black', 0xFF1A1A1A),
  InkColor('Blue', 0xFF1F5FD6),
  InkColor('Red', 0xFFD63A2F),
  InkColor('Green', 0xFF1E8A4C),
];

/// The highlighter's colors, which PencilKit lays down translucent.
const markerColors = [
  InkColor('Yellow', 0xFFFFD60A),
  InkColor('Green', 0xFF7CE38B),
  InkColor('Pink', 0xFFFF8FB8),
  InkColor('Blue', 0xFF7CC4FF),
];

/// The colors [tool] offers; empty for one that doesn't ink.
List<InkColor> colorsOf(DrawingTool tool) => switch (tool) {
  DrawingTool.pen || DrawingTool.pencil => penColors,
  DrawingTool.marker => markerColors,
  DrawingTool.eraser || DrawingTool.lasso => const [],
};

/// One document pane's tools: the one in hand, each tool's last color and
/// width, whether Undo and Redo can act, and whether a finger draws. The
/// window's, like its focus: a choice of tool isn't Claktory's state.
class InkToolsController extends ChangeNotifier {
  DrawingTool _tool = DrawingTool.pen;
  DrawingTool _lastInk = DrawingTool.pen;
  final _colors = <DrawingTool, int>{
    for (final tool in DrawingTool.values)
      if (tool.inks) tool: colorsOf(tool).first.argb,
  };
  final _widths = <DrawingTool, double>{
    for (final tool in DrawingTool.values) tool: tool.width,
  };
  var _canUndo = false;
  var _canRedo = false;
  var _fingerDraws = false;

  DrawingTool get tool => _tool;

  /// The inking tool last in hand (pen, pencil or highlighter): what the
  /// one ink button of a narrow bar stands for.
  DrawingTool get lastInk => _lastInk;

  /// [tool]'s color, for those that ink.
  int? colorOf(DrawingTool tool) => _colors[tool];
  bool get canUndo => _canUndo;
  bool get canRedo => _canRedo;

  /// Whether a finger draws, rather than scrolling (Draw with finger).
  bool get fingerDraws => _fingerDraws;

  InkTool get choice =>
      InkTool(kind: _tool.kind, color: _colors[_tool], width: _widths[_tool]!);

  /// The person takes [tool], with its last color and width.
  InkTool choose(DrawingTool tool) {
    if (tool != _tool) {
      _tool = tool;
      if (tool.inks) _lastInk = tool;
      notifyListeners();
    }
    return choice;
  }

  /// The person gives the tool in hand [argb].
  InkTool chooseColor(int argb) {
    if (_tool.inks && _colors[_tool] != argb) {
      _colors[_tool] = argb;
      notifyListeners();
    }
    return choice;
  }

  /// The surface changed the tool itself (a Pencil's double tap or
  /// squeeze): follow it, sending nothing back.
  void follow(InkTool changed) {
    final tool = DrawingTool.of(changed.kind);
    final color = changed.color;
    final width = changed.width;
    _tool = tool;
    if (tool.inks) _lastInk = tool;
    if (color != null && tool.inks) _colors[tool] = color;
    _widths[tool] = width;
    notifyListeners();
  }

  /// What the surface says Undo and Redo can do now.
  void undoState({required bool canUndo, required bool canRedo}) {
    if (canUndo == _canUndo && canRedo == _canRedo) return;
    _canUndo = canUndo;
    _canRedo = canRedo;
    notifyListeners();
  }

  set fingerDraws(bool value) {
    if (value == _fingerDraws) return;
    _fingerDraws = value;
    notifyListeners();
  }
}

/// The least a document pane's tool bar takes with every tool and the
/// color showing, each a whole finger's target a gap apart; narrower, the
/// inking tools share one button.
const inkToolsFullWidth =
    (6 + 3) * Touch.target + 7 * Space.xs + 1 + 2 * Space.xs;

/// A document pane's tool bar: the tools, the color of the one in hand,
/// then Undo, Redo and Draw with finger kept at its end. Narrower than
/// [inkToolsFullWidth], pen, pencil and highlighter share one button whose
/// menu holds them and the colors, so nothing is cut.
class InkTools extends StatelessWidget {
  const InkTools({
    super.key,
    required this.controller,
    required this.onChoice,
    required this.onUndo,
    required this.onRedo,
    required this.onFingerDraws,
  });

  final InkToolsController controller;

  /// The person changed the tool or its color: send it to the surface.
  final ValueChanged<InkTool> onChoice;
  final VoidCallback onUndo;
  final VoidCallback onRedo;

  /// The person turned Draw with finger on or off.
  final ValueChanged<bool> onFingerDraws;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: controller,
    builder: (context, _) {
      final palette = Palette.of(context);
      final tool = controller.tool;
      final colors = colorsOf(tool);
      final color = controller.choice.color;
      return Semantics(
        container: true,
        explicitChildNodes: true,
        label: 'Drawing tools',
        child: SizedBox(
          height: Touch.target,
          child: LayoutBuilder(
            builder: (context, box) {
              final narrow = box.maxWidth < inkToolsFullWidth;
              final lead = narrow
                  ? _narrowTools(controller.tool)
                  : _tools(controller.tool, colors, color);
              return Row(
                children: [
                  Expanded(
                    // Only below the narrowest pane does it scroll.
                    child: SingleChildScrollView(
                      scrollDirection: Axis.horizontal,
                      child: Row(children: _spaced(lead)),
                    ),
                  ),
                  Container(
                    width: 1,
                    height: 24,
                    margin: const EdgeInsets.symmetric(horizontal: Space.xs),
                    color: palette.hairline,
                  ),
                  ..._spaced([
                    ToolButton(
                      key: const ValueKey('ink-undo'),
                      label: 'Undo',
                      active: false,
                      onTap: controller.canUndo ? onUndo : null,
                      glyph: (color) => _Glyph(_undoGlyph, color),
                    ),
                    ToolButton(
                      key: const ValueKey('ink-redo'),
                      label: 'Redo',
                      active: false,
                      onTap: controller.canRedo ? onRedo : null,
                      glyph: (color) => _Glyph(_redoGlyph, color),
                    ),
                    // Draw with finger in plain sight, lit while it's on:
                    // a finger tap then draws rather than following a
                    // link, so the person sees why and turns it off (as
                    // in Notes).
                    ToolButton(
                      key: const ValueKey('ink-finger'),
                      label: 'Draw with finger',
                      active: controller.fingerDraws,
                      onTap: () {
                        final on = !controller.fingerDraws;
                        controller.fingerDraws = on;
                        onFingerDraws(on);
                      },
                      glyph: (color) => _Glyph(_fingerGlyph, color),
                    ),
                  ]),
                ],
              );
            },
          ),
        ),
      );
    },
  );

  /// [controls] a finger's gap apart.
  List<Widget> _spaced(List<Widget> controls) => [
    for (final (i, control) in controls.indexed) ...[
      if (i > 0) const SizedBox(width: Space.xs),
      control,
    ],
  ];

  /// Every tool, then the color of the one in hand.
  List<Widget> _tools(DrawingTool tool, List<InkColor> colors, int? color) => [
    for (final each in DrawingTool.values)
      ToolButton(
        key: ValueKey('ink-tool-${each.wire}'),
        label: each.label,
        active: each == tool,
        onTap: () => onChoice(controller.choose(each)),
        glyph: (color) => _Glyph(_toolGlyph[each]!, color),
      ),
    if (colors.isNotEmpty && color != null)
      Builder(
        builder: (anchor) => _Swatch(
          key: const ValueKey('ink-color'),
          color: Color(color),
          label: 'Color, ${_colorLabel(colors, color)}',
          onTap: () => _chooseColor(anchor, colors, color),
        ),
      ),
  ];

  /// A narrow pane's: one button for the inking tools, showing the last
  /// in hand and its color (a tap takes it; another, while it's in hand,
  /// offers the others and the colors), then the eraser and the lasso.
  List<Widget> _narrowTools(DrawingTool tool) {
    final ink = controller.lastInk;
    final argb = controller.colorOf(ink);
    final colors = colorsOf(ink);
    return [
      Builder(
        builder: (anchor) => ToolButton(
          key: const ValueKey('ink-tool-ink'),
          label: argb == null
              ? ink.label
              : '${ink.label}, ${_colorLabel(colors, argb)}',
          active: tool.inks,
          onTap: tool.inks
              ? () => _chooseInk(anchor)
              : () => onChoice(controller.choose(ink)),
          glyph: (color) => Stack(
            clipBehavior: Clip.none,
            children: [
              _Glyph(_toolGlyph[ink]!, color),
              if (argb != null)
                Positioned(
                  right: -3,
                  bottom: -3,
                  child: _Dot(color: Color(argb), size: 9),
                ),
            ],
          ),
        ),
      ),
      for (final each in [DrawingTool.eraser, DrawingTool.lasso])
        ToolButton(
          key: ValueKey('ink-tool-${each.wire}'),
          label: each.label,
          active: each == tool,
          onTap: () => onChoice(controller.choose(each)),
          glyph: (color) => _Glyph(_toolGlyph[each]!, color),
        ),
    ];
  }

  /// The ink button's menu: the inking tools, then the colors of the one
  /// in hand.
  Future<void> _chooseInk(BuildContext anchor) async {
    final tool = controller.tool;
    final current = controller.colorOf(tool);
    final colors = colorsOf(tool);
    final palette = Palette.of(anchor);
    final chosen = await presentPopover<(DrawingTool?, int?)>(
      anchor,
      builder: (context, close) => Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final each in DrawingTool.values.where((t) => t.inks))
            _Row(
              key: ValueKey('ink-menu-${each.wire}'),
              label: each.label,
              checked: each == tool,
              leading: _Glyph(_toolGlyph[each]!, palette.text),
              onTap: () => close((each, null)),
            ),
          if (colors.isNotEmpty) ...[
            Container(height: 1, color: palette.hairline),
            for (final color in colors)
              _Row(
                key: ValueKey('ink-color-${color.label}'),
                label: color.label,
                checked: color.argb == current,
                leading: _Dot(color: color.color),
                onTap: () => close((null, color.argb)),
              ),
          ],
        ],
      ),
    );
    switch (chosen) {
      case (final DrawingTool tool, _):
        onChoice(controller.choose(tool));
      case (_, final int argb):
        onChoice(controller.chooseColor(argb));
      case null:
        break;
    }
  }

  static String _colorLabel(List<InkColor> colors, int argb) => colors
      .firstWhere((c) => c.argb == argb, orElse: () => colors.first)
      .label;

  Future<void> _chooseColor(
    BuildContext anchor,
    List<InkColor> colors,
    int current,
  ) async {
    final chosen = await presentPopover<int>(
      anchor,
      builder: (context, close) => Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final color in colors)
            _Row(
              key: ValueKey('ink-color-${color.label}'),
              label: color.label,
              checked: color.argb == current,
              leading: _Dot(color: color.color),
              onTap: () => close(color.argb),
            ),
        ],
      ),
    );
    if (chosen != null) onChoice(controller.chooseColor(chosen));
  }
}

/// A popover's row: a finger's height, a mark when [checked].
class _Row extends StatelessWidget {
  const _Row({
    super.key,
    required this.label,
    required this.checked,
    required this.onTap,
    this.leading,
  });

  final String label;
  final bool checked;
  final VoidCallback onTap;
  final Widget? leading;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    return Semantics(
      button: true,
      selected: checked,
      label: label,
      onTap: onTap,
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => Container(
          constraints: const BoxConstraints(minHeight: Touch.target),
          padding: const EdgeInsets.symmetric(horizontal: Space.l),
          color: hovered ? palette.hover : const Color(0x00000000),
          child: Row(
            children: [
              if (leading != null) ...[
                leading!,
                const SizedBox(width: Space.m),
              ],
              Expanded(
                child: Padding(
                  padding: const EdgeInsets.symmetric(vertical: Space.s),
                  child: Text(
                    label,
                    style: type.label.copyWith(color: palette.text),
                  ),
                ),
              ),
              if (checked) _Glyph(_checkGlyph, palette.text),
            ],
          ),
        ),
      ),
    );
  }
}

/// The tool in hand's color, as a button that offers the others.
class _Swatch extends StatelessWidget {
  const _Swatch({
    super.key,
    required this.color,
    required this.label,
    required this.onTap,
  });

  final Color color;
  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => ToolButton(
    label: label,
    active: false,
    onTap: onTap,
    glyph: (_) => _Dot(color: color),
  );
}

/// A color's dot, ringed so a light one shows on a light pane.
class _Dot extends StatelessWidget {
  const _Dot({required this.color, this.size = 20});

  final Color color;
  final double size;

  @override
  Widget build(BuildContext context) => Container(
    width: size,
    height: size,
    decoration: BoxDecoration(
      color: color,
      shape: BoxShape.circle,
      border: Border.all(color: Palette.of(context).hairline),
    ),
  );
}

/// A Lucide glyph from its 24-unit paths, 20pt.
class _Glyph extends StatelessWidget {
  const _Glyph(this.paths, this.color);

  final List<Path> paths;
  final Color color;

  @override
  Widget build(BuildContext context) => CustomPaint(
    size: const Size.square(20),
    painter: _GlyphPainter(paths, color),
  );
}

class _GlyphPainter extends CustomPainter {
  const _GlyphPainter(this.paths, this.color);

  final List<Path> paths;
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final scale = size.width / 24;
    canvas.scale(scale);
    final paint = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.5 / scale
      ..strokeJoin = StrokeJoin.round
      ..strokeCap = StrokeCap.round;
    for (final path in paths) {
      canvas.drawPath(path, paint);
    }
  }

  @override
  bool shouldRepaint(_GlyphPainter old) =>
      old.color != color || old.paths != paths;
}

List<Path> _lucide(List<String> data) => [for (final d in data) svgPath(d)];

// Lucide's pen, pencil, highlighter, eraser, lasso, undo-2, redo-2,
// ellipsis and check (ISC licence, as the rest of the app's glyphs).
final _toolGlyph = <DrawingTool, List<Path>>{
  DrawingTool.pen: _lucide(const [
    'M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83'
        'l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z',
  ]),
  DrawingTool.pencil: _lucide(const [
    'M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83'
        'l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z',
    'm15 5 4 4',
  ]),
  DrawingTool.marker: _lucide(const [
    'm9 11-6 6v3h9l3-3',
    'm22 12-4.6 4.6a2 2 0 0 1-2.8 0l-5.2-5.2a2 2 0 0 1 0-2.8L14 4',
  ]),
  DrawingTool.eraser: _lucide(const [
    'M21 21H8a2 2 0 0 1-1.42-.587l-3.994-3.999a2 2 0 0 1 0-2.828l10-10'
        'a2 2 0 0 1 2.829 0l5.999 6a2 2 0 0 1 0 2.828L12.834 21',
    'm5.082 11.09 8.828 8.828',
  ]),
  DrawingTool.lasso: _lucide(const [
    'M7 22a5 5 0 0 1-2-4',
    'M3.3 14A6.8 6.8 0 0 1 2 10c0-4.4 4.5-8 10-8s10 3.6 10 8-4.5 8-10 8'
        'a12 12 0 0 1-5-1',
    'M3 16a2 2 0 1 0 4 0a2 2 0 1 0-4 0',
  ]),
};

final _undoGlyph = _lucide(const [
  'M9 14 4 9l5-5',
  'M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5a5.5 5.5 0 0 1-5.5 5.5H11',
]);

final _redoGlyph = _lucide(const [
  'm15 14 5-5-5-5',
  'M20 9H9.5A5.5 5.5 0 0 0 4 14.5A5.5 5.5 0 0 0 9.5 20H13',
]);

final _fingerGlyph = _lucide(const [
  'M22 14a8 8 0 0 1-8 8',
  'M18 11v-1a2 2 0 0 0-2-2a2 2 0 0 0-2 2',
  'M14 10V9a2 2 0 0 0-2-2a2 2 0 0 0-2 2v1',
  'M10 9.5V4a2 2 0 0 0-2-2a2 2 0 0 0-2 2v10',
  'M18 11a2 2 0 1 1 4 0v3a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34'
      'l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15',
]);

final _checkGlyph = _lucide(const ['M20 6 9 17l-5-5']);
