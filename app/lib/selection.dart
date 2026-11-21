import 'package:flutter/rendering.dart' show Selectable, SelectedContent;
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
class SelectableArea extends StatelessWidget {
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
  Widget build(BuildContext context) {
    final native = nativeTextSelection(context);
    final items = menuItems;
    return SelectableRegion(
      selectionControls: native.controls ?? selectionControls,
      contextMenuBuilder:
          nativeSelectableMenu(extra: extra) ??
          (items == null
              ? menu
              : (context, region) => menu(context, region, more: items())),
      magnifierConfiguration: native.magnifier,
      onSelectionChanged: onSelectionChanged,
      child: child,
    );
  }
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
/// by line breaks.
class CopyShape extends StatefulWidget {
  const CopyShape({
    super.key,
    this.separator = '',
    this.lead = '',
    this.cells = false,
    required this.child,
  });

  final String separator;
  final String lead;
  final bool cells;
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
    final parts = <({String text, double top})>[];
    _wider = wider || selected.length > 1;
    try {
      for (final selectable in selected) {
        final content = selectable.getSelectedContent();
        if (content == null || content.plainText.isEmpty) continue;
        final top = shape.cells
            ? selectable.getTransformTo(null).getTranslation().y
            : 0.0;
        parts.add((text: content.plainText, top: top));
      }
    } finally {
      _wider = wider;
    }
    if (parts.isEmpty) return null;
    final buffer = StringBuffer();
    for (var i = 0; i < parts.length; i++) {
      if (i > 0) {
        buffer.write(
          !shape.cells
              ? shape.separator
              : (parts[i].top - parts[i - 1].top).abs() < 1
              ? '\t'
              : '\n',
        );
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
    return SelectedContent(plainText: text);
  }
}
