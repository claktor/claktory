import 'package:flutter/rendering.dart' show SelectedContent;
import 'package:flutter/widgets.dart';

import 'identity.dart';

/// Selection with no handles or toolbar of its own: a mouse selects, and a
/// right click opens [CopyMenu]. (The handle mixin is what makes a region
/// build its context menu with [SelectableArea.menu].)
class _Controls extends EmptyTextSelectionControls
    with TextSelectionHandleControls {}

final selectionControls = _Controls();

/// Makes the text in [child] selectable with the mouse, copied with ⌘C or
/// a right click's Copy, like the conversation. Buttons, toggles and menus
/// inside keep out of it ([Unselectable]).
class SelectableArea extends StatelessWidget {
  const SelectableArea({
    super.key,
    required this.child,
    this.onSelectionChanged,
  });

  final Widget child;
  final ValueChanged<SelectedContent?>? onSelectionChanged;

  /// A region's right-click menu: just Copy, for now.
  static Widget menu(BuildContext context, SelectableRegionState region) {
    VoidCallback? copy;
    for (final item in region.contextMenuButtonItems) {
      if (item.type == ContextMenuButtonType.copy) copy = item.onPressed;
    }
    return CopyMenu(
      anchor: region.contextMenuAnchors.primaryAnchor,
      onCopy: copy,
      onDismiss: region.hideToolbar,
    );
  }

  @override
  Widget build(BuildContext context) => SelectableRegion(
    selectionControls: selectionControls,
    contextMenuBuilder: menu,
    onSelectionChanged: onSelectionChanged,
    child: child,
  );
}

/// What a selection shouldn't take: a control's label.
class Unselectable extends StatelessWidget {
  const Unselectable({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) =>
      SelectionContainer.disabled(child: child);
}

/// The menu a right click on selectable text opens at [anchor] (global):
/// a floating layer like a select's menu, with Copy (faint when nothing is
/// selected) and its shortcut. A click elsewhere closes it.
class CopyMenu extends StatefulWidget {
  const CopyMenu({
    super.key,
    required this.anchor,
    required this.onCopy,
    required this.onDismiss,
  });

  final Offset anchor;
  final VoidCallback? onCopy;
  final VoidCallback onDismiss;

  @override
  State<CopyMenu> createState() => _CopyMenuState();
}

class _CopyMenuState extends State<CopyMenu> {
  var _hovered = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final enabled = widget.onCopy != null;
    return CustomSingleChildLayout(
      delegate: _MenuLayout(widget.anchor),
      child: TapRegion(
        onTapOutside: (_) => widget.onDismiss(),
        // As wide as its row, at least 160: the row's Expanded label would
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
            child: Semantics(
              button: true,
              enabled: enabled,
              label: 'Copy',
              excludeSemantics: true,
              child: MouseRegion(
                cursor: enabled
                    ? SystemMouseCursors.click
                    : SystemMouseCursors.basic,
                onEnter: (_) => setState(() => _hovered = true),
                onExit: (_) => setState(() => _hovered = false),
                child: GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTap: widget.onCopy,
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
                            'Copy',
                            style: TextStyles.body.copyWith(
                              color: enabled ? palette.text : palette.textFaint,
                            ),
                          ),
                        ),
                        const SizedBox(width: Space.l),
                        Text(
                          '⌘C',
                          style: TextStyles.caption.copyWith(
                            color: palette.textFaint,
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ),
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
