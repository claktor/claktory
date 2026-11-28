import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/gestures.dart' show PointerDeviceKind, kTouchSlop;
import 'package:flutter/rendering.dart' show RenderAbstractViewport;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import '../mobile/back.dart' show BackHandle, CallbackBackToken, PhoneBack;
import '../mobile/haptics.dart';
import '../mobile/input.dart' show InputKind;
import '../mobile/metrics.dart';
import '../mobile/popover.dart';
import '../mobile/presentation.dart' show presentSheet;
import 'catalog.dart' show AppViewIcon;

/// A Button's menu (catalog v1's `menu`): its options, opened over the
/// page, never pushing it down: a sheet from the bottom on a phone, a
/// popover pointing at the button on an iPad, a dropdown under it
/// elsewhere. Past [menuSearchFrom] options it has a search field.
/// Choosing one sends its action and closes it; a tap outside, Esc, Back
/// or dragging the sheet down closes it without one.

/// One of a menu's options: its words, whether it's what's chosen (a check
/// beside it, a tint and a bar at its left), its action's event, an icon
/// after its words (a star, a house) and the heading it's under.
typedef AppMenuOption = ({
  String label,
  bool selected,
  Map<String, Object?> event,
  String icon,
  String section,
});

/// The chosen row's bar at its left, as Grafana marks it.
const _bar = 3.0;

/// A menu with more options than this has a search field.
const menuSearchFrom = 9;

/// A dropdown's width at least and at most, and its height at most.
const _dropdownMinWidth = 240.0;
const _dropdownMaxWidth = 360.0;
const _dropdownMaxHeight = 400.0;

/// What opens a menu: the button it's on calls [open].
class AppMenuOpener {
  VoidCallback? _open;

  void open() => _open?.call();
}

/// [child], the button, with its menu: [opener] opens it.
class AppViewMenu extends StatefulWidget {
  const AppViewMenu({
    super.key,
    required this.opener,
    required this.title,
    required this.options,
    required this.onChoose,
    required this.child,
  });

  final AppMenuOpener opener;

  /// Over the phone's sheet; nothing when empty.
  final String title;
  final List<AppMenuOption> options;
  final void Function(Map<String, Object?> event) onChoose;
  final Widget child;

  @override
  State<AppViewMenu> createState() => _AppViewMenuState();
}

class _AppViewMenuState extends State<AppViewMenu> {
  final _portal = OverlayPortalController();
  final _link = LayerLink();
  final _region = Object();

  /// Its hold on an Android window's Back while open, so Back closes it
  /// and nothing under it.
  BackHandle? _back;

  /// Where each finger outside it touched down.
  final _touches = <int, Offset>{};

  /// The options as they were when it opened: a choice sends the action
  /// the person saw, whatever the app has sent since.
  List<AppMenuOption> _shown = const [];

  void _open() {
    if (_portal.isShowing) {
      _close();
      return;
    }
    final options = widget.options;
    if (options.isEmpty) return;
    _shown = options;
    if (isPhoneLayout(context)) {
      unawaited(
        presentSheet<int>(
          context,
          title: widget.title.isEmpty ? null : widget.title,
          // About 60% of the screen; a long one keeps that height as its
          // search narrows it, so nothing jumps.
          builder: (context, close) {
            final most = MediaQuery.sizeOf(context).height * 0.6;
            final list = AppMenuList(
              options: options,
              touch: true,
              choose: close,
            );
            return options.length >= menuSearchFrom
                ? SizedBox(height: most, child: list)
                : ConstrainedBox(
                    constraints: BoxConstraints(maxHeight: most),
                    child: list,
                  );
          },
        ).then((index) => _chosen(options, index)),
      );
      return;
    }
    if (popoverHere(context)) {
      unawaited(
        presentPopover<int>(
          context,
          builder: (context, close) => SizedBox(
            width: popoverWidth,
            child: ConstrainedBox(
              constraints: BoxConstraints(
                maxHeight: math.min(
                  480,
                  MediaQuery.sizeOf(context).height * 0.6,
                ),
              ),
              child: AppMenuList(options: options, touch: true, choose: close),
            ),
          ),
        ).then((index) => _chosen(options, index)),
      );
      return;
    }
    setState(_portal.show);
    _back ??= PhoneBack.maybeOf(context)
        ?.register(CallbackBackToken(onBack: _close));
  }

  void _close() {
    _back?.release();
    _back = null;
    if (_portal.isShowing) setState(_portal.hide);
  }

  @override
  void dispose() {
    _back?.release();
    super.dispose();
  }

  void _chosen(List<AppMenuOption> options, int? index) {
    if (index == null || index < 0 || index >= options.length) return;
    widget.onChoose(options[index].event);
  }

  @override
  Widget build(BuildContext context) {
    widget.opener._open = _open;
    // A finger closes it on lifting from a tap outside, never on touching
    // down: an edge's Back swipe starts with a touch, and the menu must
    // still hold Back when the swipe's Back comes.
    return TapRegion(
      groupId: _region,
      onTapOutside: (event) {
        if (event.kind == PointerDeviceKind.touch) {
          _touches[event.pointer] = event.position;
        } else {
          _close();
        }
      },
      onTapUpOutside: (event) {
        final down = _touches.remove(event.pointer);
        if (down != null && (event.position - down).distance <= kTouchSlop) {
          _close();
        }
      },
      child: CompositedTransformTarget(
        link: _link,
        child: OverlayPortal(
          controller: _portal,
          overlayChildBuilder: _dropdown,
          child: widget.child,
        ),
      ),
    );
  }

  /// Under the button (over it where there's more room above), its left
  /// edge at the button's, kept inside the window.
  Widget _dropdown(BuildContext context) {
    final box = this.context.findRenderObject();
    final window = MediaQuery.sizeOf(context);
    var anchor = Rect.zero;
    if (box is RenderBox && box.hasSize) {
      anchor = box.localToGlobal(Offset.zero) & box.size;
    }
    final width = math.max(
      _dropdownMinWidth,
      math.min(_dropdownMaxWidth, window.width - 2 * Space.s),
    );
    final below = window.height - anchor.bottom - Space.s;
    final above = anchor.top - Space.s;
    final down = below >= math.min(_dropdownMaxHeight, 200) || below >= above;
    final room = math.max(120.0, (down ? below : above) - Space.xs);
    final left = math.max(
      Space.s - anchor.left,
      math.min(0.0, window.width - Space.s - anchor.left - width),
    );
    final options = _shown;
    return CompositedTransformFollower(
      link: _link,
      targetAnchor: down ? Alignment.bottomLeft : Alignment.topLeft,
      followerAnchor: down ? Alignment.topLeft : Alignment.bottomLeft,
      offset: Offset(left, down ? Space.xs : -Space.xs),
      child: Align(
        alignment: down ? Alignment.topLeft : Alignment.bottomLeft,
        child: TapRegion(
          groupId: _region,
          child: Container(
            width: width,
            constraints: BoxConstraints(
              maxHeight: math.min(_dropdownMaxHeight, room),
            ),
            decoration: BoxDecoration(
              color: Palette.of(context).sidebar,
              borderRadius: BorderRadius.circular(Radii.panel),
              border: Border.all(color: Palette.of(context).hairline),
              boxShadow: floatingShadow,
            ),
            clipBehavior: Clip.antiAlias,
            child: AppMenuList(
              options: options,
              touch: false,
              choose: ([index]) {
                _close();
                _chosen(options, index);
              },
            ),
          ),
        ),
      ),
    );
  }
}

/// A menu's options, the chosen one checked and scrolled into view as it
/// opens, under a search field past [menuSearchFrom] of them. [touch]:
/// a finger's rows (a sheet, a popover); else a pointer's, with keys: the
/// arrows move, Return chooses, Esc closes.
class AppMenuList extends StatefulWidget {
  const AppMenuList({
    super.key,
    required this.options,
    required this.touch,
    required this.choose,
  });

  final List<AppMenuOption> options;
  final bool touch;

  /// With the option's index; with nothing, closes it.
  final void Function([int?]) choose;

  @override
  State<AppMenuList> createState() => _AppMenuListState();
}

class _AppMenuListState extends State<AppMenuList> {
  final _query = TextEditingController();
  final _scroll = ScrollController();
  final _keys = <int, GlobalKey>{};
  late final _focus = FocusNode(debugLabel: 'Menu', onKeyEvent: _onKey);
  final _field = FocusNode(debugLabel: 'Menu search');

  /// The row the keys are on, in [_matching]'s order.
  var _highlighted = 0;

  bool get _searchable => widget.options.length >= menuSearchFrom;

  List<int> get _matching {
    final query = _fold(_query.text.trim());
    return [
      for (var i = 0; i < widget.options.length; i++)
        if (query.isEmpty || _fold(widget.options[i].label).contains(query)) i,
    ];
  }

  @override
  void initState() {
    super.initState();
    final chosen = widget.options.indexWhere((option) => option.selected);
    _highlighted = math.max(0, chosen);
    _query.addListener(() => setState(() => _highlighted = 0));
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      // A pointer's menu takes the keys at once, even from the button
      // clicked or a field elsewhere that had them (autofocus waits for
      // nothing else to be focused): typing filters straight away.
      if (!widget.touch) (_searchable ? _field : _focus).requestFocus();
      // The chosen one in the middle of the list where it can be, the
      // list starting with a whole row.
      if (chosen >= 0) _center(chosen);
    });
  }

  @override
  void dispose() {
    _query.dispose();
    _scroll.dispose();
    _focus.dispose();
    _field.dispose();
    super.dispose();
  }

  GlobalKey _key(int index) => _keys.putIfAbsent(index, GlobalKey.new);

  /// Scrolls [index]'s row to the middle of the list, then up to the top
  /// of the row the list would start in, so none starts cut under the
  /// search field.
  void _center(int index) {
    if (!_scroll.hasClients) return;
    final position = _scroll.position;
    double? top(int i) {
      final box = _keys[i]?.currentContext?.findRenderObject();
      final content = box is RenderBox
          ? RenderAbstractViewport.maybeOf(box)
          : null;
      if (box is! RenderBox || content == null) return null;
      return content.getOffsetToReveal(box, 0).offset;
    }

    final row = top(index);
    final box = _keys[index]?.currentContext?.findRenderObject();
    if (row == null || box is! RenderBox) return;
    final middle = row - (position.viewportDimension - box.size.height) / 2;
    var target = middle.clamp(
      position.minScrollExtent,
      position.maxScrollExtent,
    );
    // The last row starting at or above where the list would start.
    double? snapped;
    for (final i in _keys.keys) {
      final at = top(i);
      if (at != null &&
          at <= target + 0.5 &&
          (snapped == null || at > snapped)) {
        snapped = at;
      }
    }
    if (snapped != null && target > position.minScrollExtent) {
      target = snapped.clamp(
        position.minScrollExtent,
        position.maxScrollExtent,
      );
    }
    position.jumpTo(target);
  }

  void _reveal(int index, double alignment) {
    final row = _keys[index]?.currentContext;
    if (row == null) return;
    unawaited(
      Scrollable.ensureVisible(
        row,
        alignment: alignment,
        alignmentPolicy: alignment == 0.5
            ? ScrollPositionAlignmentPolicy.explicit
            : alignment == 0
            ? ScrollPositionAlignmentPolicy.keepVisibleAtStart
            : ScrollPositionAlignmentPolicy.keepVisibleAtEnd,
      ),
    );
  }

  /// Chosen or closed already: a second key or tap does nothing.
  var _done = false;

  void _choose(int index) {
    if (_done) return;
    _done = true;
    if (widget.touch) unawaited(Haptics.selection());
    widget.choose(index);
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    final matching = _matching;
    final key = event.logicalKey;
    if (key == LogicalKeyboardKey.escape) {
      if (!_done) widget.choose();
      _done = true;
      return KeyEventResult.handled;
    }
    if (matching.isEmpty) return KeyEventResult.ignored;
    if (key == LogicalKeyboardKey.arrowDown ||
        key == LogicalKeyboardKey.arrowUp) {
      final down = key == LogicalKeyboardKey.arrowDown;
      setState(() {
        _highlighted =
            (_highlighted + (down ? 1 : -1) + matching.length) %
            matching.length;
      });
      _reveal(matching[_highlighted], down ? 1 : 0);
      return KeyEventResult.handled;
    }
    if (key == LogicalKeyboardKey.enter ||
        key == LogicalKeyboardKey.numpadEnter) {
      _choose(matching[_highlighted.clamp(0, matching.length - 1)]);
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final matching = _matching;
    final list = SingleChildScrollView(
      controller: _scroll,
      padding: EdgeInsets.all(widget.touch ? 0 : Space.xs),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final (at, index) in matching.indexed) ...[
            if (widget.options[index].section case final section
                when section.isNotEmpty &&
                    (at == 0 ||
                        widget.options[matching[at - 1]].section != section))
              _heading(context, section, first: at == 0),
            _row(
              context,
              index,
              highlighted: !widget.touch && at == _highlighted,
            ),
          ],
          if (matching.isEmpty)
            Padding(
              padding: const EdgeInsets.all(Space.m),
              child: Text(
                'No matches',
                style: type.body.copyWith(color: palette.textMuted),
              ),
            ),
        ],
      ),
    );
    return Focus(
      focusNode: _focus,
      // Keys reach it from its search field too.
      autofocus: !widget.touch && !_searchable,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (_searchable)
            Padding(
              padding: widget.touch
                  ? const EdgeInsets.only(bottom: Space.s)
                  : const EdgeInsets.fromLTRB(Space.xs, Space.xs, Space.xs, 0),
              child: TextField(
                controller: _query,
                focusNode: _field,
                placeholder: 'Search',
                semanticsLabel: 'Search',
                kind: InputKind.text,
                textInputAction: TextInputAction.search,
                autocorrect: false,
                // A pointer's menu takes the keys at once; a finger's
                // brings the keyboard only when the field is tapped.
                autofocus: !widget.touch,
                onSubmitted: (_) {
                  final now = _matching;
                  if (now.isNotEmpty) {
                    _choose(now[_highlighted.clamp(0, now.length - 1)]);
                  }
                },
              ),
            ),
          Flexible(child: list),
        ],
      ),
    );
  }

  /// The heading over a run of options of one section.
  Widget _heading(BuildContext context, String text, {required bool first}) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final inset = widget.touch ? Space.m : Space.s;
    return Semantics(
      header: true,
      child: Padding(
        padding: EdgeInsets.fromLTRB(
          inset,
          first ? Space.xs : Space.m,
          inset,
          Space.xs,
        ),
        child: Text(
          text,
          style: type.caption.copyWith(
            color: palette.textMuted,
            fontWeight: FontWeight.w500,
          ),
        ),
      ),
    );
  }

  Widget _row(BuildContext context, int index, {required bool highlighted}) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final metrics = Metrics.of(context);
    final option = widget.options[index];
    final touch = widget.touch;
    return Semantics(
      key: _key(index),
      button: true,
      selected: option.selected,
      label: option.label,
      onTap: () => _choose(index),
      excludeSemantics: true,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: touch
            ? null
            : (_) => setState(() => _highlighted = _matching.indexOf(index)),
        child: Pressable(
          onTap: () => _choose(index),
          builder: (context, pressed) => Container(
            constraints: BoxConstraints(
              minHeight: touch ? metrics.row : metrics.target,
            ),
            // The bar takes its width from the left inset, so the chosen
            // row's words line up with the others'.
            padding: EdgeInsets.fromLTRB(
              (touch ? Space.m : Space.s) - (option.selected ? _bar : 0),
              touch ? Space.s : Space.xs,
              touch ? Space.m : Space.s,
              touch ? Space.s : Space.xs,
            ),
            decoration: BoxDecoration(
              color: option.selected
                  ? palette.selected
                  : pressed || highlighted
                  ? palette.hover
                  : null,
              border: option.selected
                  ? Border(
                      left: BorderSide(color: palette.signal, width: _bar),
                    )
                  : null,
            ),
            child: Row(
              spacing: Space.s,
              children: [
                Expanded(
                  child: Text(
                    option.label,
                    style: (touch ? type.body : TextStyles.body).copyWith(
                      color: palette.text,
                    ),
                  ),
                ),
                // A star marks a starred one: filled, in the accent, as
                // Grafana's starred star is (an outline reads as "not
                // starred"). One this app can't draw leaves no mark.
                if (option.icon == 'star')
                  ExcludeSemantics(
                    child: CustomPaint(
                      size: Size.square(touch ? 18 : 16),
                      painter: _FilledStar(palette.signal),
                    ),
                  )
                else if (hasLucideIcon(option.icon))
                  AppViewIcon(name: option.icon, size: touch ? 18 : 16),
                if (option.selected)
                  ExtensionGlyph(
                    icon: 'check',
                    name: 'Chosen',
                    color: palette.signal,
                    size: touch ? 20 : 16,
                  ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// A five-pointed star, filled, its points rounded as the icons' strokes
/// are, in a 24-unit square scaled to the size.
class _FilledStar extends CustomPainter {
  const _FilledStar(this.color);

  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.scale(size.width / 24, size.height / 24);
    const center = Offset(12, 12.6);
    final path = Path();
    for (var i = 0; i < 10; i++) {
      final radius = i.isEven ? 9.6 : 4.1;
      final angle = -math.pi / 2 + i * math.pi / 5;
      final point =
          center + Offset(math.cos(angle) * radius, math.sin(angle) * radius);
      i == 0
          ? path.moveTo(point.dx, point.dy)
          : path.lineTo(point.dx, point.dy);
    }
    path.close();
    canvas.drawPath(
      path,
      Paint()
        ..color = color
        ..style = PaintingStyle.fill,
    );
    canvas.drawPath(
      path,
      Paint()
        ..color = color
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2
        ..strokeJoin = StrokeJoin.round,
    );
  }

  @override
  bool shouldRepaint(_FilledStar old) => old.color != color;
}

/// [text] lowercased without its accents, for search ("Zoë" finds "zoe").
String _fold(String text) {
  final lower = text.toLowerCase();
  final out = StringBuffer();
  for (final rune in lower.runes) {
    out.write(_plain[rune] ?? String.fromCharCode(rune));
  }
  return out.toString();
}

const _plain = <int, String>{
  0xe0: 'a', 0xe1: 'a', 0xe2: 'a', 0xe3: 'a', 0xe4: 'a', 0xe5: 'a', //
  0xe7: 'c', 0xe8: 'e', 0xe9: 'e', 0xea: 'e', 0xeb: 'e', 0xec: 'i', //
  0xed: 'i', 0xee: 'i', 0xef: 'i', 0xf1: 'n', 0xf2: 'o', 0xf3: 'o', //
  0xf4: 'o', 0xf5: 'o', 0xf6: 'o', 0xf8: 'o', 0xf9: 'u', 0xfa: 'u', //
  0xfb: 'u', 0xfc: 'u', 0xfd: 'y', 0xff: 'y', 0xdf: 'ss', //
};
