import 'dart:async' show scheduleMicrotask;
import 'dart:math' as math;

import 'package:flutter/gestures.dart' show HitTestResult, PointerDeviceKind;
import 'package:flutter/rendering.dart'
    show RenderBox, Selectable, SelectedContent;
import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';

import 'identity.dart';
import 'mobile/input.dart';

/// Selection with no handles or toolbar of its own: a mouse selects, and a
/// right click opens [CopyMenu]. (The handle mixin is what makes a region
/// build its context menu with [SelectableArea.menu].)
class _Controls extends EmptyTextSelectionControls
    with TextSelectionHandleControls {}

final selectionControls = _Controls();

/// Makes the text in [child] selectable with the mouse, copied with ⌘C or
/// a right click's Copy, like the conversation. Buttons, toggles and menus
/// inside keep out of it ([Unselectable]). On an iPhone it's iOS's own:
/// touch and hold selects, with handles, the magnifier and the system's
/// toolbar (Copy, Select All, Look Up…) plus [extra].
class SelectableArea extends StatefulWidget {
  const SelectableArea({
    super.key,
    required this.child,
    this.onSelectionChanged,
    this.extra,
    this.menuItems,
  });

  final Widget child;
  final ValueChanged<SelectedContent?>? onSelectionChanged;

  /// More for the iPhone's toolbar, such as Comment.
  final ExtraEditItems<SelectableRegionState>? extra;

  /// More for the right click's menu, after Copy, asked for as it opens
  /// (Reply, for the message under the pointer).
  final List<MenuItem> Function()? menuItems;

  /// A region's right-click menu: Copy, then [more].
  static Widget menu(
    BuildContext context,
    SelectableRegionState region, {
    List<MenuItem> more = const [],
  }) {
    VoidCallback? copy;
    for (final item in region.contextMenuButtonItems) {
      if (item.type == ContextMenuButtonType.copy) copy = item.onPressed;
    }
    return CopyMenu(
      anchor: region.contextMenuAnchors.primaryAnchor,
      onCopy: copy,
      onDismiss: region.hideToolbar,
      more: [
        for (final item in more)
          MenuItem(item.label, () {
            region.hideToolbar();
            item.onTap();
          }),
      ],
    );
  }

  @override
  State<SelectableArea> createState() => _SelectableAreaState();
}

class _SelectableAreaState extends State<SelectableArea> {
  final _focus = FocusNode(debugLabel: 'SelectableArea');

  /// Every area on show, for where ⌘A goes once the one it went to is gone.
  static final _shown = <_SelectableAreaState>{};

  /// The pane it's in, whose clicks outside it are its own.
  _SelectionPaneState? _pane;

  @override
  void initState() {
    super.initState();
    _shown.add(this);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final pane = context.dependOnInheritedWidgetOfExactType<_PaneScope>()?.pane;
    if (pane != _pane) {
      _pane?._leave(this);
      _pane = pane?.._area = this;
    }
  }

  @override
  void deactivate() {
    // Gone with ⌘A's focus (its pane closed, a tab changed): once the new
    // frame is up, ⌘A goes to what shows instead, unless something there
    // took focus.
    if (_focus.hasFocus) {
      SchedulerBinding.instance
        ..addPostFrameCallback((_) => _refocus())
        ..ensureVisualUpdate();
    }
    _shown.remove(this);
    super.deactivate();
  }

  @override
  void activate() {
    super.activate();
    _shown.add(this);
  }

  @override
  void dispose() {
    _shown.remove(this);
    _pane?._leave(this);
    _focus.dispose();
    super.dispose();
  }

  /// With nothing focused, the largest area on show takes ⌘A: the
  /// conversation, beside whatever closed. Only one in the scope that has
  /// focus, and in sight: never one under a dialog or sheet that's open
  /// (its scrim over it), whose own area takes it then.
  static void _refocus() {
    final primary = FocusManager.instance.primaryFocus;
    if (primary != null && primary is! FocusScopeNode) return;
    _SelectableAreaState? largest;
    var most = 0.0;
    for (final area in _shown) {
      if (!area.mounted) continue;
      if (primary != null && !area._focus.ancestors.contains(primary)) {
        continue;
      }
      final box = area.context.findRenderObject();
      if (box is! RenderBox || !box.attached || !box.hasSize) continue;
      if (!_inSight(area.context, box)) continue;
      final size = box.size.width * box.size.height;
      if (size > most) (largest, most) = (area, size);
    }
    largest?._focus.requestFocus();
  }

  /// Whether a click at [box]'s middle would reach it, nothing over it.
  static bool _inSight(BuildContext context, RenderBox box) {
    final result = HitTestResult();
    WidgetsBinding.instance.hitTestInView(
      result,
      box.localToGlobal(box.size.center(Offset.zero)),
      View.of(context).viewId,
    );
    return result.path.any((entry) => entry.target == box);
  }

  /// A click anywhere in it, a card's button included, makes it the one
  /// ⌘A selects, as a click on its text does, rather than the selection
  /// clicked in before (a click outside a text field already takes its
  /// focus). Focus already inside it, in a field of its own, stays; and
  /// only a mouse or trackpad's counts: a touch that scrolls mustn't put
  /// the keyboard away.
  void _pressed(PointerDownEvent event) {
    if (event.kind != PointerDeviceKind.mouse &&
        event.kind != PointerDeviceKind.trackpad) {
      return;
    }
    final primary = FocusManager.instance.primaryFocus;
    if (primary == _focus || (primary?.ancestors.contains(_focus) ?? false)) {
      return;
    }
    // Once the press is handled: a text field outside lets go of focus
    // as it sees it, which would undo it.
    scheduleMicrotask(() {
      if (mounted) _focus.requestFocus();
    });
  }

  @override
  Widget build(BuildContext context) {
    final native = nativeTextSelection(context);
    final items = widget.menuItems;
    final extra = widget.extra;
    final child = widget.child;
    return Listener(
      onPointerDown: _pressed,
      child: SelectableRegion(
        focusNode: _focus,
        selectionControls: native.controls ?? selectionControls,
        contextMenuBuilder:
            nativeSelectableMenu(extra: extra) ??
            (items == null
                ? SelectableArea.menu
                : (context, region) =>
                      SelectableArea.menu(context, region, more: items())),
        magnifierConfiguration: native.magnifier,
        onSelectionChanged: widget.onSelectionChanged,
        // What's copied ends where the selection does, not at the line
        // break an item's [CopyShape.trail] leaves after it.
        child: CopyShape(trimEnd: true, child: child),
      ),
    );
  }
}

/// A pane with a selectable area and more around it (a header, tabs): a
/// click anywhere in the pane makes its area the one ⌘A selects, as a
/// click in the area does.
class SelectionPane extends StatefulWidget {
  const SelectionPane({super.key, required this.child});

  final Widget child;

  @override
  State<SelectionPane> createState() => _SelectionPaneState();
}

class _SelectionPaneState extends State<SelectionPane> {
  /// Its area on show, the latest to join.
  _SelectableAreaState? _area;

  void _leave(_SelectableAreaState area) {
    if (_area == area) _area = null;
  }

  @override
  Widget build(BuildContext context) => Listener(
    onPointerDown: (event) {
      final area = _area;
      if (area != null && area.mounted) area._pressed(event);
    },
    child: _PaneScope(pane: this, child: widget.child),
  );
}

class _PaneScope extends InheritedWidget {
  const _PaneScope({required this.pane, required super.child});

  final _SelectionPaneState pane;

  @override
  bool updateShouldNotify(_PaneScope old) => pane != old.pane;
}

/// What a selection shouldn't take: a control's label.
class Unselectable extends StatelessWidget {
  const Unselectable({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) =>
      SelectionContainer.disabled(child: child);
}

/// A row of a right click's menu after Copy.
class MenuItem {
  const MenuItem(this.label, this.onTap);

  final String label;
  final VoidCallback onTap;
}

/// The menu a right click on selectable text opens at [anchor] (global):
/// a floating layer like a select's menu, with Copy (faint when nothing is
/// selected) and its shortcut, then [more]. A click elsewhere closes it.
class CopyMenu extends StatelessWidget {
  const CopyMenu({
    super.key,
    required this.anchor,
    required this.onCopy,
    required this.onDismiss,
    this.more = const [],
  });

  final Offset anchor;
  final VoidCallback? onCopy;
  final VoidCallback onDismiss;
  final List<MenuItem> more;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return CustomSingleChildLayout(
      delegate: _MenuLayout(anchor),
      child: TapRegion(
        onTapOutside: (_) => onDismiss(),
        // As wide as its rows, at least 160: a row's Expanded label would
        // otherwise take the whole window.
        child: IntrinsicWidth(
          child: Container(
            constraints: const BoxConstraints(minWidth: 160),
            padding: const EdgeInsets.all(Space.xs),
            decoration: BoxDecoration(
              color: palette.sidebar,
              borderRadius: BorderRadius.circular(Radii.panel),
              border: Border.all(color: palette.hairline),
              boxShadow: floatingShadow,
            ),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                _MenuRow(label: 'Copy', shortcut: '⌘C', onTap: onCopy),
                for (final item in more)
                  _MenuRow(label: item.label, onTap: item.onTap),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// One 28px row of [CopyMenu]: its label, faint while it can't be done,
/// and its shortcut, if any.
class _MenuRow extends StatefulWidget {
  const _MenuRow({required this.label, required this.onTap, this.shortcut});

  final String label;
  final VoidCallback? onTap;
  final String? shortcut;

  @override
  State<_MenuRow> createState() => _MenuRowState();
}

class _MenuRowState extends State<_MenuRow> {
  var _hovered = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final enabled = widget.onTap != null;
    return Semantics(
      button: true,
      enabled: enabled,
      label: widget.label,
      excludeSemantics: true,
      child: MouseRegion(
        cursor: enabled ? SystemMouseCursors.click : SystemMouseCursors.basic,
        onEnter: (_) => setState(() => _hovered = true),
        onExit: (_) => setState(() => _hovered = false),
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: widget.onTap,
          child: Container(
            height: 28,
            padding: const EdgeInsets.symmetric(horizontal: Space.s),
            decoration: BoxDecoration(
              color: _hovered && enabled
                  ? palette.selected
                  : const Color(0x00000000),
              borderRadius: BorderRadius.circular(Radii.control),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Expanded(
                  child: Text(
                    widget.label,
                    style: TextStyles.body.copyWith(
                      color: enabled ? palette.text : palette.textFaint,
                    ),
                  ),
                ),
                if (widget.shortcut case final shortcut?) ...[
                  const SizedBox(width: Space.l),
                  Text(
                    shortcut,
                    style: TextStyles.caption.copyWith(
                      color: palette.textFaint,
                    ),
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Puts the menu's top left at the anchor, kept inside the window.
class _MenuLayout extends SingleChildLayoutDelegate {
  _MenuLayout(this.anchor);

  final Offset anchor;

  @override
  BoxConstraints getConstraintsForChild(BoxConstraints constraints) =>
      constraints.loosen();

  @override
  Offset getPositionForChild(Size size, Size child) => Offset(
    anchor.dx.clamp(0, (size.width - child.width).clamp(0, double.infinity)),
    anchor.dy.clamp(0, (size.height - child.height).clamp(0, double.infinity)),
  );

  @override
  bool shouldRelayout(_MenuLayout old) => old.anchor != anchor;
}

/// What a selection copies of [child], shaped as it reads: its parts (the
/// blocks, lines or cells inside, in reading order) joined by [separator],
/// led by [lead] (a list item's "• " or "2. ") with the lines after the
/// first indented under the text, as a pasted email or list needs. With
/// [cells], parts side by side are a table's row, joined by tabs, and rows
/// by line breaks; with [lines], parts side by side are joined by a space
/// and rows by line breaks, as a card's lines read. [trail] follows it when
/// the selection runs to its end (a list's items, which a scrolling list
/// would otherwise run together), and [trimEnd] drops line breaks at the
/// very end (a selection area's root).
class CopyShape extends StatefulWidget {
  const CopyShape({
    super.key,
    this.separator = '',
    this.lead = '',
    this.cells = false,
    this.lines = false,
    this.trail = '',
    this.trimEnd = false,
    required this.child,
  });

  final String separator;
  final String lead;
  final bool cells;
  final bool lines;
  final String trail;
  final bool trimEnd;
  final Widget child;

  @override
  State<CopyShape> createState() => _CopyShapeState();
}

class _CopyShapeState extends State<CopyShape> {
  late final _delegate = _ShapeDelegate(widget);

  @override
  void didUpdateWidget(CopyShape old) {
    super.didUpdateWidget(old);
    _delegate.shape = widget;
  }

  @override
  void dispose() {
    _delegate.dispose();
    super.dispose();
  }

  // Outside a selection there's nothing to shape: the text stays as plain
  // as it was, with nothing registering for a selection.
  @override
  Widget build(BuildContext context) =>
      SelectionContainer.maybeOf(context) == null
      ? widget.child
      : SelectionContainer(delegate: _delegate, child: widget.child);
}

/// While a [_ShapeDelegate] gathers its parts: whether an enclosing one has
/// more than one part selected, so the selection reaches past the part
/// being gathered (a list item copied with others gets its marker; a word
/// copied out of it doesn't).
bool _wider = false;

class _ShapeDelegate extends StaticSelectionContainerDelegate {
  _ShapeDelegate(this.shape);

  CopyShape shape;

  // Flutter takes in a container's new selectables after a frame, and one
  // that changes after a frame's end (nested containers register with
  // their parents from a microtask) waits for the next frame, which may not
  // come for a while: until something redraws, the text can't be selected.
  // So each change asks for a frame, until the containers above have it.
  void _frame() => SchedulerBinding.instance.ensureVisualUpdate();

  @override
  void add(Selectable selectable) {
    super.add(selectable);
    _frame();
  }

  @override
  void didChangeSelectables() {
    super.didChangeSelectables();
    _frame();
  }

  @override
  SelectedContent? getSelectedContent() {
    final wider = _wider;
    final selected = [
      for (final selectable in selectables)
        if (selectable.value.hasSelection) selectable,
    ];
    final parts = <({String text, double top, double bottom})>[];
    _wider = wider || selected.length > 1;
    try {
      for (final selectable in selected) {
        final content = selectable.getSelectedContent();
        if (content == null || content.plainText.isEmpty) continue;
        final rowed = shape.cells || shape.lines;
        final top = rowed
            ? selectable.getTransformTo(null).getTranslation().y
            : 0.0;
        parts.add((
          text: content.plainText,
          top: top,
          bottom: rowed ? top + selectable.size.height : 0.0,
        ));
      }
    } finally {
      _wider = wider;
    }
    if (parts.isEmpty) return null;
    final buffer = StringBuffer();
    for (var i = 0; i < parts.length; i++) {
      if (i > 0) {
        final (a, b) = (parts[i - 1], parts[i]);
        // Side by side: they share most of a row (a row's parts may be
        // centred at different heights).
        final overlap = math.min(a.bottom, b.bottom) - math.max(a.top, b.top);
        final least = math.min(a.bottom - a.top, b.bottom - b.top);
        final sameRow = overlap > least / 2;
        buffer.write(switch (shape) {
          CopyShape(cells: true) => sameRow ? '\t' : '\n',
          CopyShape(lines: true) => sameRow ? ' ' : '\n',
          _ => shape.separator,
        });
      }
      buffer.write(parts[i].text);
    }
    var text = buffer.toString();
    final lead = shape.lead;
    final range = getSelection();
    final fromStart =
        range != null && (range.startOffset == 0 || range.endOffset == 0);
    if (lead.isNotEmpty && wider && fromStart) {
      final pad = ' ' * lead.length;
      final lines = text.split('\n');
      text = [
        '$lead${lines.first}',
        for (final line in lines.skip(1)) line.isEmpty ? '' : '$pad$line',
      ].join('\n');
    }
    // Run to its end, the selection goes on past it: what follows starts
    // apart.
    if (shape.trail.isNotEmpty &&
        range != null &&
        math.max(range.startOffset, range.endOffset) >= contentLength) {
      text += shape.trail;
    }
    if (shape.trimEnd) text = text.replaceFirst(RegExp(r'\n+$'), '');
    return SelectedContent(plainText: text);
  }
}
