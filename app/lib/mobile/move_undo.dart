import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import '../scope.dart';
import '../sidebar.dart' show SidebarMove, sidebarMove, undoSidebarMove;
import 'drawer.dart' show largeText;
import 'haptics.dart';
import 'metrics.dart';
import 'navigation.dart';

/// A move made in the drawer offers Undo on the moved row for a few
/// seconds ([sidebarMove]). If the drawer closes before they're up, the
/// Undo comes to the foot of the screen for the rest of them, rather than
/// going with the drawer; the drawer out again, it's back on the row only.
/// A swipe down puts the bar away early, the move still undoable on its
/// row.
class PhoneMoveUndo extends StatefulWidget {
  const PhoneMoveUndo({super.key});

  @override
  State<PhoneMoveUndo> createState() => _PhoneMoveUndoState();
}

class _PhoneMoveUndoState extends State<PhoneMoveUndo> {
  Timer? _ends;

  /// The last move shown, kept while it slides away.
  SidebarMove? _last;

  /// The move whose bar was swiped away: it stays undoable on its row.
  String? _dismissed;

  /// How far a finger has pulled the bar down.
  var _pulled = 0.0;

  @override
  void initState() {
    super.initState();
    sidebarMove.addListener(_changed);
    _changed();
  }

  void _changed() {
    _ends?.cancel();
    _ends = null;
    final move = sidebarMove.value;
    if (move != null && move.live) {
      _ends = Timer(move.until.difference(DateTime.now()), () {
        _ends = null;
        if (mounted) setState(() {});
      });
    }
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    sidebarMove.removeListener(_changed);
    _ends?.cancel();
    super.dispose();
  }

  void _dismiss() => setState(() {
    _dismissed = sidebarMove.value?.token;
    _pulled = 0;
  });

  void _pullEnd(DragEndDetails details) {
    if (_pulled > _dismissAt || (details.primaryVelocity ?? 0) > 300) {
      _dismiss();
    } else {
      setState(() => _pulled = 0);
    }
  }

  static const _dismissAt = 24.0;

  void _undo() {
    unawaited(Haptics.selection());
    undoSidebarMove(context);
  }

  @override
  Widget build(BuildContext context) {
    final move = sidebarMove.value;
    final routes = PhoneRoutes.of(context);
    final shown =
        move != null &&
        _ends != null &&
        move.token != _dismissed &&
        move.backend == AppScope.of(context) &&
        routes != null &&
        !routes.drawerOpen;
    if (shown) _last = move;
    final last = _last;
    if (last == null) return const SizedBox.shrink();
    final media = MediaQuery.of(context);
    final still = media.disableAnimations;
    final bottom =
        (media.viewInsets.bottom > media.viewPadding.bottom
            ? media.viewInsets.bottom
            : media.viewPadding.bottom) +
        Space.s;
    return Padding(
      padding: EdgeInsets.only(
        left: media.viewPadding.left + Space.s,
        right: media.viewPadding.right + Space.s,
        bottom: bottom,
      ),
      child: Align(
        alignment: Alignment.bottomCenter,
        child: IgnorePointer(
          ignoring: !shown,
          child: ExcludeSemantics(
            excluding: !shown,
            child: AnimatedSlide(
              offset: shown ? Offset.zero : const Offset(0, 0.5),
              duration: still ? Duration.zero : Motion.base,
              curve: Curves.easeOutCubic,
              child: AnimatedOpacity(
                opacity: shown ? 1 : 0,
                duration: still ? Duration.zero : Motion.base,
                curve: Curves.easeOut,
                onEnd: () {
                  if (!shown && mounted) setState(() => _last = null);
                },
                child: GestureDetector(
                  onVerticalDragUpdate: (details) => setState(
                    () => _pulled = math.max(
                      0,
                      _pulled + (details.primaryDelta ?? 0),
                    ),
                  ),
                  onVerticalDragEnd: _pullEnd,
                  onVerticalDragCancel: () => setState(() => _pulled = 0),
                  child: Transform.translate(
                    offset: Offset(0, _pulled),
                    child: Semantics(
                      onDismiss: _dismiss,
                      child: _Bar(move: last, onUndo: _undo),
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

/// The moved agent's name over what changed ("Now reports to Bruno"), as
/// its row said it, and Undo at the end, a finger's width. At the largest
/// text sizes Undo takes a line of its own under the words, which wrap
/// whole rather than being cut beside it.
class _Bar extends StatelessWidget {
  const _Bar({required this.move, required this.onUndo});

  final SidebarMove move;
  final VoidCallback onUndo;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final target = Metrics.of(context).target;
    final stacked = largeText(context);
    final words = Padding(
      padding: EdgeInsets.only(top: Space.s, bottom: stacked ? 0 : Space.s),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            move.name,
            maxLines: stacked ? null : 1,
            overflow: stacked ? null : TextOverflow.ellipsis,
            style: PhoneType.label.copyWith(color: palette.text),
          ),
          Text(
            move.says,
            maxLines: stacked ? null : 2,
            overflow: stacked ? null : TextOverflow.ellipsis,
            style: PhoneType.caption.copyWith(color: palette.textMuted),
          ),
        ],
      ),
    );
    final undo = Semantics(
      button: true,
      label: 'Undo',
      excludeSemantics: true,
      onTap: onUndo,
      child: Pressable(
        onTap: onUndo,
        builder: (context, pressed) => ConstrainedBox(
          constraints: BoxConstraints(minWidth: target, minHeight: target),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: Space.m),
            child: Center(
              widthFactor: 1,
              heightFactor: 1,
              child: Text(
                'Undo',
                style: PhoneType.label.copyWith(
                  color: pressed ? palette.textMuted : palette.text,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          ),
        ),
      ),
    );
    return Semantics(
      container: true,
      liveRegion: true,
      child: Container(
        constraints: BoxConstraints(minHeight: target + Space.s),
        padding: const EdgeInsets.only(left: Space.m),
        decoration: BoxDecoration(
          color: palette.sidebar,
          borderRadius: BorderRadius.circular(Radii.floating),
          border: Border.all(color: palette.hairline),
          boxShadow: floatingShadow,
        ),
        child: stacked
            ? Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Padding(
                    padding: const EdgeInsets.only(right: Space.m),
                    child: words,
                  ),
                  Align(alignment: AlignmentDirectional.centerEnd, child: undo),
                ],
              )
            : Row(
                children: [
                  Expanded(child: words),
                  undo,
                ],
              ),
      ),
    );
  }
}
