import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'identity.dart';
import 'selection.dart';

/// Moves the window: dragging anywhere in [child] drags the window, and
/// double-clicking a spot where nothing in [child] takes the click does what
/// double-clicking a title bar does (zoom, by default). Header bars at the
/// top of the window are wrapped in it, since there's no native title bar
/// above them. Clicks on controls inside still reach them.
class WindowDrag extends StatefulWidget {
  const WindowDrag({super.key, required this.child});

  final Widget child;

  static const _channel = MethodChannel('factory/window');

  /// Asks the window for [method]; does nothing where there's no native
  /// window, such as in tests.
  static void _call(String method) {
    _channel.invokeMethod<void>(method).catchError((Object _) {});
  }

  @override
  State<WindowDrag> createState() => _WindowDragState();
}

class _WindowDragState extends State<WindowDrag> {
  /// When and where the last click on an empty spot was, to spot a double
  /// click: macOS's default double-click interval is half a second.
  DateTime? _lastClick;
  Offset? _lastPosition;

  void _clicked(TapUpDetails details) {
    final now = DateTime.now();
    final last = _lastClick;
    final double =
        last != null &&
        now.difference(last) < const Duration(milliseconds: 500) &&
        (details.globalPosition - _lastPosition!).distance < 6;
    _lastClick = double ? null : now;
    _lastPosition = details.globalPosition;
    if (double) WindowDrag._call('zoom');
  }

  @override
  Widget build(BuildContext context) => GestureDetector(
    behavior: HitTestBehavior.opaque,
    // Controls inside join the gesture arena first, so they win their own
    // clicks and this only sees clicks on empty spots.
    onTapUp: _clicked,
    onPanStart: (_) => WindowDrag._call('drag'),
    child: widget.child,
  );
}

/// Asks the person for a folder in the system's sheet, starting in [from];
/// its absolute path, or null when cancelled or where there's no native
/// window.
Future<String?> pickFolder({String from = ''}) async {
  try {
    return await WindowDrag._channel.invokeMethod<String>('pickFolder', from);
  } catch (_) {
    return null;
  }
}

/// The empty band at the top of a view with no header bar: where the traffic
/// lights sit over the sidebar, and what the window is dragged by.
class TitleSpace extends StatelessWidget {
  const TitleSpace({super.key});

  @override
  Widget build(BuildContext context) =>
      const WindowDrag(child: SizedBox(height: titleBarHeight));
}

/// A clickable region that tells its builder whether the pointer is over it.
class Pressable extends StatefulWidget {
  const Pressable({
    super.key,
    required this.onTap,
    required this.builder,
    this.onSecondaryTap,
  });

  final VoidCallback? onTap;

  /// A right click (or two-finger click).
  final VoidCallback? onSecondaryTap;
  final Widget Function(BuildContext context, bool hovered) builder;

  @override
  State<Pressable> createState() => _PressableState();
}

class _PressableState extends State<Pressable> {
  var _hovered = false;

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      cursor: widget.onTap == null
          ? SystemMouseCursors.basic
          : SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: widget.onTap,
        onSecondaryTap: widget.onSecondaryTap,
        child: widget.builder(context, _hovered),
      ),
    );
  }
}

/// A small text button. `primary` is the one main action of a form; it's
/// filled with the text color, since signal is never a large fill.
class Button extends StatefulWidget {
  const Button({
    super.key,
    required this.label,
    required this.onPressed,
    this.primary = false,
  });

  final String label;
  final VoidCallback? onPressed;
  final bool primary;

  @override
  State<Button> createState() => _ButtonState();
}

class _ButtonState extends State<Button> {
  var _hovered = false;
  var _focused = false;

  @override
  Widget build(BuildContext context) =>
      // A control's label is no part of a selection.
      Unselectable(child: _control(context));

  Widget _control(BuildContext context) {
    final palette = Palette.of(context);
    final enabled = widget.onPressed != null;
    final Color background;
    final Color foreground;
    if (widget.primary) {
      background = enabled
          ? palette.text.withValues(alpha: _hovered ? 0.88 : 1)
          : palette.selected;
      foreground = enabled ? palette.canvas : palette.textFaint;
    } else {
      background = _hovered && enabled
          ? palette.hover
          : const Color(0x00000000);
      foreground = !enabled
          ? palette.textFaint
          : _hovered
          ? palette.text
          : palette.textMuted;
    }
    return FocusableActionDetector(
      enabled: enabled,
      mouseCursor: enabled
          ? SystemMouseCursors.click
          : SystemMouseCursors.basic,
      onShowHoverHighlight: (value) => setState(() => _hovered = value),
      onShowFocusHighlight: (value) => setState(() => _focused = value),
      actions: {
        ActivateIntent: CallbackAction<ActivateIntent>(
          onInvoke: (_) {
            widget.onPressed?.call();
            return null;
          },
        ),
      },
      child: GestureDetector(
        onTap: widget.onPressed,
        child: Container(
          height: 28,
          padding: const EdgeInsets.symmetric(horizontal: Space.m),
          decoration: BoxDecoration(
            color: background,
            borderRadius: BorderRadius.circular(Radii.control),
            border: Border.all(
              color: _focused ? palette.signal : const Color(0x00000000),
            ),
          ),
          // As wide as its label, wherever it's placed.
          child: Center(
            widthFactor: 1,
            child: Text(
              widget.label,
              maxLines: 1,
              style: TextStyles.label.copyWith(color: foreground),
            ),
          ),
        ),
      ),
    );
  }
}

/// A text field on the widgets layer: an [EditableText] with mouse selection,
/// a placeholder, and a hairline border that turns signal when focused.
class TextField extends StatefulWidget {
  const TextField({
    super.key,
    required this.controller,
    this.focusNode,
    this.placeholder = '',
    this.maxLines = 1,
    this.minLines,
    this.autofocus = false,
    this.onSubmitted,
    this.bordered = true,
    this.obscureText = false,
  });

  final TextEditingController controller;
  final FocusNode? focusNode;
  final String placeholder;

  /// Null grows without limit.
  final int? maxLines;
  final int? minLines;
  final bool autofocus;
  final ValueChanged<String>? onSubmitted;
  final bool bordered;

  /// Hides what's typed, for secrets.
  final bool obscureText;

  @override
  State<TextField> createState() => _TextFieldState();
}

class _TextFieldState extends State<TextField>
    with WidgetsBindingObserver
    implements TextSelectionGestureDetectorBuilderDelegate {
  @override
  final editableTextKey = GlobalKey<EditableTextState>();

  @override
  bool get forcePressEnabled => false;

  @override
  bool get selectionEnabled => true;

  late final _gestures = TextSelectionGestureDetectorBuilder(delegate: this);
  FocusNode? _ownFocusNode;

  /// The pointer is over it: the placeholder holds still to be read.
  var _hovered = false;

  FocusNode get _focusNode =>
      widget.focusNode ?? (_ownFocusNode ??= FocusNode());

  /// The window is the active one. Elsewhere the caret hides, as in any
  /// Mac app, and stops asking for two frames a second to blink.
  var _active = _isActive(WidgetsBinding.instance.lifecycleState);

  static bool _isActive(AppLifecycleState? state) =>
      state == null || state == AppLifecycleState.resumed;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    final active = _isActive(state);
    if (active != _active) setState(() => _active = active);
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _ownFocusNode?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final style = TextStyles.body.copyWith(color: palette.text);
    final singleLine = widget.maxLines == 1;
    final editable = EditableText(
      key: editableTextKey,
      controller: widget.controller,
      focusNode: _focusNode,
      style: style,
      cursorColor: palette.signal,
      backgroundCursorColor: palette.textFaint,
      selectionColor: palette.signal.withValues(alpha: 0.28),
      cursorWidth: 1.5,
      showCursor: _active,
      maxLines: widget.maxLines,
      minLines: widget.minLines,
      autofocus: widget.autofocus,
      obscureText: widget.obscureText,
      autocorrect: !widget.obscureText,
      enableSuggestions: !widget.obscureText,
      keyboardType: singleLine ? TextInputType.text : TextInputType.multiline,
      textInputAction: singleLine
          ? TextInputAction.done
          : TextInputAction.newline,
      onSubmitted: widget.onSubmitted,
      rendererIgnoresPointer: true,
    );
    // Several lines of placeholder take turns, one at a time.
    final hints = placeholderLines(widget.placeholder);
    final hintStyle = style.copyWith(color: palette.textFaint);
    final field = Stack(
      children: [
        ListenableBuilder(
          listenable: widget.controller,
          builder: (context, _) => widget.controller.text.isNotEmpty
              ? const SizedBox.shrink()
              : hints.length > 1
              ? RotatingText(lines: hints, style: hintStyle, paused: _hovered)
              : Text(
                  hints.firstOrNull ?? '',
                  style: hintStyle,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
        ),
        _gestures.buildGestureDetector(
          behavior: HitTestBehavior.translucent,
          child: editable,
        ),
      ],
    );
    final input = MouseRegion(
      cursor: SystemMouseCursors.text,
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: Listener(
        onPointerDown: (_) => _focusNode.requestFocus(),
        child: field,
      ),
    );
    if (!widget.bordered) return input;
    return ListenableBuilder(
      listenable: _focusNode,
      builder: (context, child) => Container(
        padding: const EdgeInsets.symmetric(
          horizontal: Space.s + 2,
          vertical: Space.s - 2,
        ),
        decoration: BoxDecoration(
          color: palette.canvas,
          borderRadius: BorderRadius.circular(Radii.control),
          border: Border.all(
            color: _focusNode.hasFocus ? palette.signal : palette.hairline,
          ),
        ),
        child: child,
      ),
      child: input,
    );
  }
}

/// A placeholder's lines, trimmed, without empty ones: one is a hint, several
/// are examples that take turns.
List<String> placeholderLines(String placeholder) => [
  for (final line in placeholder.split('\n'))
    if (line.trim().isNotEmpty) line.trim(),
];

/// How long each of a [RotatingText]'s lines shows, and how long the next
/// takes to slide in.
const rotateEvery = Duration(milliseconds: 3500);
const rotateSlide = Duration(milliseconds: 450);

/// One line at a time from [lines], each sliding up and out as the next
/// slides up into its place, a calm vertical carousel. It holds still while
/// [paused]; with reduced motion the lines simply swap.
class RotatingText extends StatefulWidget {
  const RotatingText({
    super.key,
    required this.lines,
    required this.style,
    this.paused = false,
  });

  final List<String> lines;
  final TextStyle style;
  final bool paused;

  @override
  State<RotatingText> createState() => _RotatingTextState();
}

class _RotatingTextState extends State<RotatingText>
    with SingleTickerProviderStateMixin {
  var _index = 0;
  Timer? _timer;
  late final _slide = AnimationController(vsync: this, duration: rotateSlide);

  @override
  void initState() {
    super.initState();
    _schedule();
  }

  @override
  void didUpdateWidget(RotatingText old) {
    super.didUpdateWidget(old);
    if (_index >= widget.lines.length) _index = 0;
    if (old.paused != widget.paused) _schedule();
  }

  @override
  void dispose() {
    _timer?.cancel();
    _slide.dispose();
    super.dispose();
  }

  void _schedule() {
    _timer?.cancel();
    _timer = null;
    // A slide under way finishes and schedules the next itself.
    if (widget.paused || widget.lines.length < 2 || _slide.isAnimating) {
      return;
    }
    _timer = Timer(rotateEvery, _next);
  }

  void _next() {
    final lines = widget.lines.length;
    if (MediaQuery.maybeDisableAnimationsOf(context) ?? false) {
      setState(() => _index = (_index + 1) % lines);
      return _schedule();
    }
    _slide.forward(from: 0).whenCompleteOrCancel(() {
      if (!mounted) return;
      setState(() {
        _index = (_index + 1) % widget.lines.length;
        _slide.value = 0;
      });
      _schedule();
    });
  }

  @override
  Widget build(BuildContext context) {
    Widget line(int index) => Text(
      widget.lines[index % widget.lines.length],
      style: widget.style,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
    );
    return ClipRect(
      child: AnimatedBuilder(
        animation: _slide,
        builder: (context, _) {
          final t = Curves.easeInOutCubic.transform(_slide.value);
          if (t == 0) return line(_index);
          return Stack(
            children: [
              FractionalTranslation(
                translation: Offset(0, -t),
                child: Opacity(opacity: 1 - t, child: line(_index)),
              ),
              Positioned.fill(
                child: FractionalTranslation(
                  translation: Offset(0, 1 - t),
                  child: Opacity(opacity: t, child: line(_index + 1)),
                ),
              ),
            ],
          );
        },
      ),
    );
  }
}

class SelectOption<T> {
  const SelectOption(this.value, this.label);

  final T value;
  final String label;
}

/// A dropdown. Opens with a click, Enter, Space or Down; arrow keys move,
/// Enter picks, Esc or a click outside closes.
class Select<T> extends StatefulWidget {
  const Select({
    super.key,
    required this.value,
    required this.options,
    required this.onChanged,
  });

  final T value;
  final List<SelectOption<T>> options;
  final ValueChanged<T> onChanged;

  @override
  State<Select<T>> createState() => _SelectState<T>();
}

class _SelectState<T> extends State<Select<T>> {
  final _menu = OverlayPortalController();
  final _link = LayerLink();
  final _tapRegion = Object();
  late final _focusNode = FocusNode(onKeyEvent: _onKey);
  var _highlighted = 0;
  var _hovered = false;

  @override
  void dispose() {
    _focusNode.dispose();
    super.dispose();
  }

  int get _selectedIndex {
    for (var i = 0; i < widget.options.length; i++) {
      if (widget.options[i].value == widget.value) return i;
    }
    return 0;
  }

  void _open() {
    if (widget.options.isEmpty) return;
    _focusNode.requestFocus();
    setState(() => _highlighted = _selectedIndex);
    _menu.show();
  }

  void _close() {
    if (_menu.isShowing) _menu.hide();
  }

  void _pick(int index) {
    _close();
    widget.onChanged(widget.options[index].value);
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    final key = event.logicalKey;
    if (!_menu.isShowing) {
      if (key == LogicalKeyboardKey.enter ||
          key == LogicalKeyboardKey.space ||
          key == LogicalKeyboardKey.arrowDown) {
        _open();
        return KeyEventResult.handled;
      }
      return KeyEventResult.ignored;
    }
    if (key == LogicalKeyboardKey.escape) {
      _close();
    } else if (key == LogicalKeyboardKey.arrowDown) {
      setState(() => _highlighted = (_highlighted + 1) % widget.options.length);
    } else if (key == LogicalKeyboardKey.arrowUp) {
      setState(
        () => _highlighted =
            (_highlighted - 1 + widget.options.length) % widget.options.length,
      );
    } else if (key == LogicalKeyboardKey.enter ||
        key == LogicalKeyboardKey.space) {
      _pick(_highlighted);
    } else {
      return KeyEventResult.ignored;
    }
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) =>
      // A control's label is no part of a selection.
      Unselectable(child: _control(context));

  Widget _control(BuildContext context) {
    final palette = Palette.of(context);
    final label = widget.options.isEmpty
        ? ''
        : widget.options[_selectedIndex].label;
    return TapRegion(
      groupId: _tapRegion,
      onTapOutside: (_) => _close(),
      child: CompositedTransformTarget(
        link: _link,
        child: OverlayPortal(
          controller: _menu,
          overlayChildBuilder: (context) => _buildMenu(context, palette),
          child: Focus(
            focusNode: _focusNode,
            child: ListenableBuilder(
              listenable: _focusNode,
              builder: (context, _) => MouseRegion(
                cursor: SystemMouseCursors.click,
                onEnter: (_) => setState(() => _hovered = true),
                onExit: (_) => setState(() => _hovered = false),
                child: GestureDetector(
                  onTap: () => _menu.isShowing ? _close() : _open(),
                  child: Container(
                    height: 30,
                    padding: const EdgeInsets.symmetric(
                      horizontal: Space.s + 2,
                    ),
                    decoration: BoxDecoration(
                      color: _hovered ? palette.hover : palette.canvas,
                      borderRadius: BorderRadius.circular(Radii.control),
                      border: Border.all(
                        color: _focusNode.hasFocus
                            ? palette.signal
                            : palette.hairline,
                      ),
                    ),
                    child: Row(
                      children: [
                        Expanded(
                          child: Text(
                            label,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyles.body.copyWith(
                              color: palette.text,
                            ),
                          ),
                        ),
                        const SizedBox(width: Space.s),
                        Chevron(color: palette.textMuted, down: true),
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

  Widget _buildMenu(BuildContext context, Palette palette) {
    // As wide as the field itself; this state's context is the field.
    final field = this.context.findRenderObject();
    final width = field is RenderBox && field.hasSize ? field.size.width : null;
    return CompositedTransformFollower(
      link: _link,
      targetAnchor: Alignment.bottomLeft,
      offset: const Offset(0, Space.xs),
      child: Align(
        alignment: Alignment.topLeft,
        child: TapRegion(
          groupId: _tapRegion,
          child: Container(
            width: width,
            constraints: const BoxConstraints(maxHeight: 280, minWidth: 200),
            padding: const EdgeInsets.all(Space.xs),
            decoration: BoxDecoration(
              color: palette.sidebar,
              borderRadius: BorderRadius.circular(Radii.panel),
              border: Border.all(color: palette.hairline),
              boxShadow: floatingShadow,
            ),
            child: ListView.builder(
              shrinkWrap: true,
              itemCount: widget.options.length,
              itemBuilder: (context, index) {
                final option = widget.options[index];
                final selected = option.value == widget.value;
                return MouseRegion(
                  cursor: SystemMouseCursors.click,
                  onEnter: (_) => setState(() => _highlighted = index),
                  child: GestureDetector(
                    onTap: () => _pick(index),
                    child: Container(
                      height: 28,
                      padding: const EdgeInsets.symmetric(horizontal: Space.s),
                      decoration: BoxDecoration(
                        color: index == _highlighted
                            ? palette.selected
                            : const Color(0x00000000),
                        borderRadius: BorderRadius.circular(Radii.control),
                      ),
                      child: Row(
                        children: [
                          Expanded(
                            child: Text(
                              option.label,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyles.body.copyWith(
                                color: palette.text,
                              ),
                            ),
                          ),
                          if (selected) Dot(color: palette.signal),
                        ],
                      ),
                    ),
                  ),
                );
              },
            ),
          ),
        ),
      ),
    );
  }
}

/// Shows a dialog over the whole window. The builder gets a `close` callback;
/// Esc and a click on the scrim close it too.
Future<void> showModal(
  BuildContext context,
  Widget Function(BuildContext context, VoidCallback close) builder,
) {
  final overlay = Overlay.of(context);
  final done = Completer<void>();
  late final OverlayEntry entry;
  void close() {
    if (done.isCompleted) return;
    entry.remove();
    entry.dispose();
    done.complete();
  }

  entry = OverlayEntry(
    builder: (context) => _Modal(close: close, child: builder(context, close)),
  );
  overlay.insert(entry);
  return done.future;
}

class _Modal extends StatelessWidget {
  const _Modal({required this.close, required this.child});

  final VoidCallback close;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return TweenAnimationBuilder<double>(
      tween: Tween(begin: 0, end: 1),
      duration: Motion.fast,
      curve: Curves.easeOut,
      builder: (context, t, child) => Opacity(opacity: t, child: child),
      child: Stack(
        children: [
          Positioned.fill(
            child: GestureDetector(
              onTap: close,
              child: const ColoredBox(color: Color(0x66000000)),
            ),
          ),
          Center(
            child: FocusScope(
              autofocus: true,
              child: CallbackShortcuts(
                bindings: {
                  const SingleActivator(LogicalKeyboardKey.escape): close,
                },
                child: Container(
                  width: 480,
                  margin: const EdgeInsets.all(Space.xl),
                  padding: const EdgeInsets.all(Space.xl),
                  decoration: BoxDecoration(
                    color: palette.canvas,
                    borderRadius: BorderRadius.circular(Radii.floating),
                    border: Border.all(color: palette.hairline),
                    boxShadow: floatingShadow,
                  ),
                  child: SelectableArea(child: child),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// A form field's label with its control below.
class Field extends StatelessWidget {
  const Field({super.key, required this.label, required this.child, this.note});

  final String label;
  final Widget child;

  /// Extra guidance under the control.
  final String? note;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.l),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            label,
            style: TextStyles.caption.copyWith(
              color: palette.textMuted,
              fontWeight: FontWeight.w500,
            ),
          ),
          const SizedBox(height: Space.xs + 2),
          child,
          if (note != null) ...[
            const SizedBox(height: Space.xs + 2),
            Text(
              note!,
              style: TextStyles.caption.copyWith(color: palette.textFaint),
            ),
          ],
        ],
      ),
    );
  }
}

/// Floats [card] above [child] while the pointer rests on [child], like a
/// tooltip: a floating layer in the sidebar's color, left-aligned with
/// [child], or with [beside] to its right, centered on it. It never takes
/// the pointer itself.
class HoverCard extends StatefulWidget {
  const HoverCard({
    super.key,
    required this.card,
    required this.child,
    this.beside = false,
  });

  final WidgetBuilder card;
  final Widget child;
  final bool beside;

  @override
  State<HoverCard> createState() => _HoverCardState();
}

class _HoverCardState extends State<HoverCard> {
  final _portal = OverlayPortalController();
  final _link = LayerLink();

  /// Waits a moment, so the card doesn't flash while the pointer passes by.
  Timer? _wait;

  @override
  void dispose() {
    _wait?.cancel();
    super.dispose();
  }

  void _enter(PointerEnterEvent _) {
    _wait?.cancel();
    _wait = Timer(const Duration(milliseconds: 300), _portal.show);
  }

  void _exit(PointerExitEvent _) {
    _wait?.cancel();
    if (_portal.isShowing) _portal.hide();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return CompositedTransformTarget(
      link: _link,
      child: OverlayPortal(
        controller: _portal,
        overlayChildBuilder: (context) => CompositedTransformFollower(
          link: _link,
          targetAnchor: widget.beside
              ? Alignment.centerRight
              : Alignment.topLeft,
          followerAnchor: widget.beside
              ? Alignment.centerLeft
              : Alignment.bottomLeft,
          offset: widget.beside
              ? const Offset(Space.xs, 0)
              : const Offset(0, -Space.xs),
          child: Align(
            alignment: widget.beside
                ? Alignment.centerLeft
                : Alignment.bottomLeft,
            child: IgnorePointer(
              child: TweenAnimationBuilder<double>(
                tween: Tween(begin: 0, end: 1),
                duration: Motion.fast,
                curve: Curves.easeOut,
                builder: (context, t, child) =>
                    Opacity(opacity: t, child: child),
                child: Container(
                  constraints: const BoxConstraints(maxWidth: 320),
                  padding: const EdgeInsets.all(Space.m),
                  decoration: BoxDecoration(
                    color: palette.sidebar,
                    borderRadius: BorderRadius.circular(Radii.panel),
                    border: Border.all(color: palette.hairline),
                    boxShadow: floatingShadow,
                  ),
                  child: DefaultTextStyle(
                    style: TextStyles.body.copyWith(color: palette.text),
                    child: widget.card(context),
                  ),
                ),
              ),
            ),
          ),
        ),
        child: MouseRegion(onEnter: _enter, onExit: _exit, child: widget.child),
      ),
    );
  }
}

class Dot extends StatelessWidget {
  const Dot({super.key, required this.color, this.size = 6});

  final Color color;
  final double size;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: size,
      height: size,
      decoration: BoxDecoration(color: color, shape: BoxShape.circle),
    );
  }
}

/// A 10px chevron, pointing right or down.
class Chevron extends StatelessWidget {
  const Chevron({super.key, required this.color, required this.down});

  final Color color;
  final bool down;

  @override
  Widget build(BuildContext context) {
    return CustomPaint(
      size: const Size.square(10),
      painter: _ChevronPainter(color: color, down: down),
    );
  }
}

class _ChevronPainter extends CustomPainter {
  const _ChevronPainter({required this.color, required this.down});

  final Color color;
  final bool down;

  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.5
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round;
    final w = size.width;
    final h = size.height;
    final path = Path();
    if (down) {
      path
        ..moveTo(w * 0.2, h * 0.35)
        ..lineTo(w * 0.5, h * 0.65)
        ..lineTo(w * 0.8, h * 0.35);
    } else {
      path
        ..moveTo(w * 0.35, h * 0.2)
        ..lineTo(w * 0.65, h * 0.5)
        ..lineTo(w * 0.35, h * 0.8);
    }
    canvas.drawPath(path, paint);
  }

  @override
  bool shouldRepaint(_ChevronPainter old) =>
      old.color != color || old.down != down;
}

/// A 14px folder outline (Lucide's "folder"), marking a row as a project.
class FolderGlyph extends StatelessWidget {
  const FolderGlyph({super.key, required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) {
    return CustomPaint(
      size: const Size.square(14),
      painter: _FolderPainter(color),
    );
  }
}

class _FolderPainter extends CustomPainter {
  const _FolderPainter(this.color);

  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    // Drawn on Lucide's 24-unit grid, scaled to the glyph size.
    final scale = size.width / 24;
    canvas.scale(scale);
    final paint = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.5 / scale
      ..strokeJoin = StrokeJoin.round;
    const r = Radius.circular(2);
    final path = Path()
      ..moveTo(20, 20)
      ..arcToPoint(const Offset(22, 18), radius: r, clockwise: false)
      ..lineTo(22, 8)
      ..arcToPoint(const Offset(20, 6), radius: r, clockwise: false)
      ..lineTo(12.1, 6)
      ..lineTo(9.6, 3.9)
      ..lineTo(7.9, 3)
      ..lineTo(4, 3)
      ..arcToPoint(const Offset(2, 5), radius: r, clockwise: false)
      ..lineTo(2, 18)
      ..arcToPoint(const Offset(4, 20), radius: r, clockwise: false)
      ..close();
    canvas.drawPath(path, paint);
  }

  @override
  bool shouldRepaint(_FolderPainter old) => old.color != color;
}

/// A 14px file outline (Lucide's "file").
class FileGlyph extends StatelessWidget {
  const FileGlyph({super.key, required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) {
    return CustomPaint(
      size: const Size.square(14),
      painter: _FilePainter(color),
    );
  }
}

class _FilePainter extends CustomPainter {
  const _FilePainter(this.color);

  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    // Lucide's 24-unit grid, scaled to the glyph size.
    final scale = size.width / 24;
    canvas.scale(scale);
    final paint = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.5 / scale
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round;
    const r = Radius.circular(2);
    final outline = Path()
      ..moveTo(15, 2)
      ..lineTo(6, 2)
      ..arcToPoint(const Offset(4, 4), radius: r, clockwise: false)
      ..lineTo(4, 20)
      ..arcToPoint(const Offset(6, 22), radius: r, clockwise: false)
      ..lineTo(18, 22)
      ..arcToPoint(const Offset(20, 20), radius: r, clockwise: false)
      ..lineTo(20, 7)
      ..close();
    final fold = Path()
      ..moveTo(14, 2)
      ..lineTo(14, 6)
      ..arcToPoint(const Offset(16, 8), radius: r, clockwise: false)
      ..lineTo(20, 8);
    canvas
      ..drawPath(outline, paint)
      ..drawPath(fold, paint);
  }

  @override
  bool shouldRepaint(_FilePainter old) => old.color != color;
}

/// Three dots in a 14px square (Lucide's "ellipsis"): more about this.
class MoreGlyph extends StatelessWidget {
  const MoreGlyph({super.key, required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) {
    return SizedBox.square(
      dimension: 14,
      child: Row(
        mainAxisAlignment: MainAxisAlignment.spaceEvenly,
        children: [for (var i = 0; i < 3; i++) Dot(color: color, size: 2.5)],
      ),
    );
  }
}

/// A signal dot that pulses while something is live.
class LiveDot extends StatefulWidget {
  const LiveDot({super.key, this.color});

  /// Signal orange unless given.
  final Color? color;

  @override
  State<LiveDot> createState() => _LiveDotState();
}

class _LiveDotState extends State<LiveDot> with SingleTickerProviderStateMixin {
  late final _pulse = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 900),
  )..repeat(reverse: true);

  @override
  void dispose() {
    _pulse.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return RepaintBoundary(
      child: FadeTransition(
        opacity: Tween<double>(
          begin: 0.35,
          end: 1,
        ).animate(CurvedAnimation(parent: _pulse, curve: Curves.easeInOut)),
        child: Dot(color: widget.color ?? Palette.of(context).signal),
      ),
    );
  }
}

/// Rebuilds [builder] with the time every second while [active], for clocks
/// that count up (or down) between the backend's updates.
class Ticking extends StatefulWidget {
  const Ticking({super.key, required this.active, required this.builder});

  final bool active;
  final Widget Function(BuildContext context, DateTime now) builder;

  @override
  State<Ticking> createState() => _TickingState();
}

class _TickingState extends State<Ticking> {
  Timer? _timer;

  @override
  void initState() {
    super.initState();
    _schedule();
  }

  @override
  void didUpdateWidget(Ticking old) {
    super.didUpdateWidget(old);
    if (old.active != widget.active) _schedule();
  }

  void _schedule() {
    _timer?.cancel();
    _timer = widget.active
        ? Timer.periodic(const Duration(seconds: 1), (_) => setState(() {}))
        : null;
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.builder(context, DateTime.now());
}

/// How long something took or has run: "0.4s" and "3.2s" under ten
/// seconds (whole seconds when [whole], for a clock that ticks), then "42s",
/// "1m 23s", "1h 02m".
String formatDuration(Duration duration, {bool whole = false}) {
  final ms = duration.inMilliseconds < 0 ? 0 : duration.inMilliseconds;
  if (ms < 10000 && !whole) return '${(ms / 1000).toStringAsFixed(1)}s';
  final seconds = ms ~/ 1000;
  if (seconds < 60) return '${seconds}s';
  if (seconds < 3600) return '${seconds ~/ 60}m ${seconds % 60}s';
  final minutes = (seconds % 3600) ~/ 60;
  return '${seconds ~/ 3600}h ${minutes.toString().padLeft(2, '0')}m';
}

/// While something works: a soft band of [color] gliding left to right
/// across the area and back again, without pause, turning at each end while
/// still half in view, so it's always visible. Every wave keeps the same
/// clock, so several working at once move together and read as one calm
/// rhythm. With reduced motion it's a
/// still tint instead. It never takes the pointer.
class WorkingWave extends StatefulWidget {
  const WorkingWave({
    super.key,
    required this.color,
    this.peak = 0.16,
    this.radius = 0,
  });

  final Color color;

  /// The band's opacity at its middle.
  final double peak;

  /// The corners it's clipped to.
  final double radius;

  /// There and back: one way takes half of it.
  static const period = Duration(milliseconds: 3600);

  @override
  State<WorkingWave> createState() => _WorkingWaveState();
}

class _WorkingWaveState extends State<WorkingWave>
    with SingleTickerProviderStateMixin {
  final _phase = ValueNotifier(0.0);
  late final _ticker = createTicker((_) => _phase.value = _now());

  /// Where the shared clock is in the period, 0 to 1.
  static double _now() {
    final period = WorkingWave.period.inMilliseconds;
    return (DateTime.now().millisecondsSinceEpoch % period) / period;
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final still = MediaQuery.disableAnimationsOf(context);
    if (still && _ticker.isActive) _ticker.stop();
    if (!still && !_ticker.isActive) _ticker.start();
  }

  @override
  void dispose() {
    _ticker.dispose();
    _phase.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final still = MediaQuery.disableAnimationsOf(context);
    return IgnorePointer(
      child: RepaintBoundary(
        child: ClipRRect(
          borderRadius: BorderRadius.circular(widget.radius),
          child: still
              ? ColoredBox(
                  color: widget.color.withValues(alpha: widget.peak * 0.6),
                )
              : CustomPaint(
                  painter: _WavePainter(
                    phase: _phase,
                    color: widget.color,
                    peak: widget.peak,
                  ),
                  child: const SizedBox.expand(),
                ),
        ),
      ),
    );
  }
}

/// How wide the wave's band is across an area [width] wide.
double waveBand(double width) => math.max(80.0, width * 0.55);

/// Where the middle of the wave's band is at [phase] (0 to 1) of
/// [WorkingWave.period]: from the left edge to the right one in the first
/// half and back in the second, easing into each turn. At the turns its
/// middle is on the edge, so half of it is still in view.
double waveCenter(double phase, double width) {
  final there = phase < 0.5 ? phase * 2 : 2 - phase * 2;
  return Curves.easeInOut.transform(there.clamp(0.0, 1.0)) * width;
}

class _WavePainter extends CustomPainter {
  _WavePainter({required this.phase, required this.color, required this.peak})
    : super(repaint: phase);

  final ValueNotifier<double> phase;
  final Color color;
  final double peak;

  @override
  void paint(Canvas canvas, Size size) {
    final band = waveBand(size.width);
    final left = waveCenter(phase.value, size.width) - band / 2;
    final clear = color.withValues(alpha: 0);
    canvas.drawRect(
      Rect.fromLTWH(left, 0, band, size.height),
      Paint()
        ..shader = LinearGradient(
          colors: [
            clear,
            color.withValues(alpha: peak),
            clear,
          ],
        ).createShader(Rect.fromLTWH(left, 0, band, size.height)),
    );
  }

  @override
  bool shouldRepaint(_WavePainter old) =>
      old.color != color || old.peak != peak || old.phase != phase;
}

/// A 28px square button holding a 16px glyph, tinted on hover and while
/// what it opens is showing.
class GlyphButton extends StatelessWidget {
  const GlyphButton({
    super.key,
    required this.label,
    required this.active,
    required this.onTap,
    required this.glyph,
  });

  /// What it opens, for assistive technology and tests.
  final String label;
  final bool active;
  final VoidCallback onTap;
  final Widget Function(Color color) glyph;

  @override
  Widget build(BuildContext context) =>
      // A control's label is no part of a selection.
      Unselectable(child: _control(context));

  Widget _control(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      label: label,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => AnimatedContainer(
          duration: Motion.fast,
          width: 28,
          height: 28,
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: active
                ? palette.selected
                : hovered
                ? palette.hover
                : const Color(0x00000000),
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: glyph(active || hovered ? palette.text : palette.textMuted),
        ),
      ),
    );
  }
}

/// A 16px cog, for Settings.
class CogGlyph extends StatelessWidget {
  const CogGlyph({super.key, required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) =>
      CustomPaint(size: const Size.square(16), painter: _CogPainter(color));
}

class _CogPainter extends CustomPainter {
  const _CogPainter(this.color);

  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final scale = size.width / 24;
    canvas.scale(scale);
    final paint = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.5 / scale
      ..strokeJoin = StrokeJoin.round;
    // Eight teeth around a ring, each a little narrower at its tip.
    const center = Offset(12, 12);
    const teeth = 8;
    const outer = 9.5;
    const inner = 7.2;
    const step = 2 * math.pi / teeth;
    final path = Path();
    for (var i = 0; i < teeth; i++) {
      final mid = i * step - math.pi / 2;
      final points = [
        (mid - step * 0.5, inner),
        (mid - step * 0.2, inner),
        (mid - step * 0.13, outer),
        (mid + step * 0.13, outer),
        (mid + step * 0.2, inner),
      ];
      for (final (angle, radius) in points) {
        final point =
            center + Offset(math.cos(angle), math.sin(angle)) * radius;
        if (i == 0 && angle == points.first.$1) {
          path.moveTo(point.dx, point.dy);
        } else {
          path.lineTo(point.dx, point.dy);
        }
      }
    }
    path.close();
    canvas.drawPath(path, paint);
    canvas.drawCircle(center, 3, paint);
  }

  @override
  bool shouldRepaint(_CogPainter old) => old.color != color;
}

/// A 16px shopping bag (Lucide's "shopping-bag"), for the Marketplace.
class BagGlyph extends StatelessWidget {
  const BagGlyph({super.key, required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) =>
      CustomPaint(size: const Size.square(16), painter: _BagPainter(color));
}

class _BagPainter extends CustomPainter {
  const _BagPainter(this.color);

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
    const r = Radius.circular(2);
    final bag = Path()
      ..moveTo(6, 2)
      ..lineTo(3, 6)
      ..lineTo(3, 20)
      ..arcToPoint(const Offset(5, 22), radius: r, clockwise: false)
      ..lineTo(19, 22)
      ..arcToPoint(const Offset(21, 20), radius: r, clockwise: false)
      ..lineTo(21, 6)
      ..lineTo(18, 2)
      ..close();
    canvas.drawPath(bag, paint);
    canvas.drawLine(const Offset(3, 6), const Offset(21, 6), paint);
    final handle = Path()
      ..moveTo(16, 10)
      ..arcToPoint(
        const Offset(8, 10),
        radius: const Radius.circular(4),
        clockwise: true,
      );
    canvas.drawPath(handle, paint);
  }

  @override
  bool shouldRepaint(_BagPainter old) => old.color != color;
}

/// Draws a Lucide icon's strokes, given in its 24-unit grid, at 16px with a
/// 1.5px stroke.
class _LucidePainter extends CustomPainter {
  const _LucidePainter(this.color, this.draw);

  final Color color;
  final void Function(Canvas canvas, Paint paint) draw;

  @override
  void paint(Canvas canvas, Size size) {
    final scale = size.width / 24;
    canvas.scale(scale);
    draw(
      canvas,
      Paint()
        ..color = color
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1.5 / scale
        ..strokeJoin = StrokeJoin.round
        ..strokeCap = StrokeCap.round,
    );
  }

  @override
  bool shouldRepaint(_LucidePainter old) =>
      old.color != color || old.draw != draw;
}

void _drawUsers(Canvas canvas, Paint paint) {
  const r4 = Radius.circular(4);
  canvas.drawCircle(const Offset(9, 7), 4, paint);
  canvas.drawPath(
    Path()
      ..moveTo(16, 21)
      ..lineTo(16, 19)
      ..arcToPoint(const Offset(12, 15), radius: r4, clockwise: false)
      ..lineTo(6, 15)
      ..arcToPoint(const Offset(2, 19), radius: r4, clockwise: false)
      ..lineTo(2, 21),
    paint,
  );
  canvas.drawPath(
    Path()
      ..moveTo(22, 21)
      ..lineTo(22, 19)
      ..arcToPoint(const Offset(19, 15.13), radius: r4, clockwise: false),
    paint,
  );
  canvas.drawPath(
    Path()
      ..moveTo(16, 3.13)
      ..arcToPoint(const Offset(16, 10.88), radius: r4, clockwise: true),
    paint,
  );
}

void _drawListChecks(Canvas canvas, Paint paint) {
  for (final y in [7.0, 17.0]) {
    canvas.drawPath(
      Path()
        ..moveTo(3, y)
        ..lineTo(5, y + 2)
        ..lineTo(9, y - 2),
      paint,
    );
  }
  for (final y in [6.0, 12.0, 18.0]) {
    canvas.drawLine(Offset(13, y), Offset(21, y), paint);
  }
}

void _drawX(Canvas canvas, Paint paint) {
  canvas.drawLine(const Offset(18, 6), const Offset(6, 18), paint);
  canvas.drawLine(const Offset(6, 6), const Offset(18, 18), paint);
}

/// A path from SVG path data, as Lucide icons and brand marks are given:
/// moves, lines, arcs and curves (cubic, smooth, quadratic), absolute or
/// relative.
Path svgPath(String data) {
  final tokens = RegExp(r'[A-Za-z]|-?(?:\d+\.?\d*|\.\d+)(?:e-?\d+)?')
      .allMatches(data)
      .map((match) => match.group(0)!)
      .toList();
  final path = Path();
  var i = 0;
  var command = '';
  var x = 0.0, y = 0.0, startX = 0.0, startY = 0.0;
  // The last curve's second control point, for smooth curves.
  Offset? control;
  double number() => double.parse(tokens[i++]);
  while (i < tokens.length) {
    if (RegExp('[A-Za-z]').hasMatch(tokens[i])) command = tokens[i++];
    final relative = command == command.toLowerCase();
    final dx = relative ? x : 0.0, dy = relative ? y : 0.0;
    final upper = command.toUpperCase();
    final previous = control;
    control = null;
    switch (upper) {
      case 'M':
        x = number() + dx;
        y = number() + dy;
        path.moveTo(x, y);
        startX = x;
        startY = y;
        // Pairs after a move are lines.
        command = relative ? 'l' : 'L';
      case 'L':
        x = number() + dx;
        y = number() + dy;
        path.lineTo(x, y);
      case 'H':
        x = number() + dx;
        path.lineTo(x, y);
      case 'V':
        y = number() + dy;
        path.lineTo(x, y);
      case 'A':
        final rx = number(), ry = number(), rotation = number();
        final large = number() != 0, sweep = number() != 0;
        x = number() + dx;
        y = number() + dy;
        path.arcToPoint(
          Offset(x, y),
          radius: Radius.elliptical(rx, ry),
          rotation: rotation,
          largeArc: large,
          clockwise: sweep,
        );
      case 'C' || 'S':
        // A smooth curve's first control point mirrors the last one's.
        final Offset first;
        if (upper == 'C') {
          first = Offset(number() + dx, number() + dy);
        } else {
          first = previous == null
              ? Offset(x, y)
              : Offset(2 * x - previous.dx, 2 * y - previous.dy);
        }
        final second = Offset(number() + dx, number() + dy);
        x = number() + dx;
        y = number() + dy;
        path.cubicTo(first.dx, first.dy, second.dx, second.dy, x, y);
        control = second;
      case 'Q':
        final cx = number() + dx, cy = number() + dy;
        x = number() + dx;
        y = number() + dy;
        path.quadraticBezierTo(cx, cy, x, y);
      case 'Z':
        path.close();
        x = startX;
        y = startY;
      default:
        // A command this parser doesn't know: stop rather than misdraw.
        return path;
    }
  }
  return path;
}

/// Draws Lucide icon paths.
void Function(Canvas, Paint) _paths(List<String> data) {
  final paths = [for (final d in data) svgPath(d)];
  return (canvas, paint) {
    for (final path in paths) {
      canvas.drawPath(path, paint);
    }
  };
}

final _drawBrain = _paths(const [
  'M12 5a3 3 0 1 0-5.997.125 4 4 0 0 0-2.526 5.77 4 4 0 0 0 .556 6.588'
      'A4 4 0 1 0 12 18Z',
  'M12 5a3 3 0 1 1 5.997.125 4 4 0 0 1 2.526 5.77 4 4 0 0 1-.556 6.588'
      'A4 4 0 1 1 12 18Z',
  'M15 13a4.5 4.5 0 0 1-3-4 4.5 4.5 0 0 1-3 4',
  'M17.599 6.5a3 3 0 0 0 .399-1.375',
  'M6.003 5.125A3 3 0 0 0 6.401 6.5',
  'M3.477 10.896a4 4 0 0 1 .585-.396',
  'M19.938 10.5a4 4 0 0 1 .585.396',
  'M6 18a4 4 0 0 1-1.967-.516',
  'M19.967 17.484A4 4 0 0 1 18 18',
]);

/// The Lucide icons this app can draw, by name.
final _lucide = <String, void Function(Canvas, Paint)>{
  'users': _drawUsers,
  'list-checks': _drawListChecks,
  'x': _drawX,
  'brain': _drawBrain,
  'arrow-down': _drawArrowDown,
  'arrow-left': _drawArrowLeft,
  'plus': _drawPlus,
  'terminal': _drawTerminal,
  'sparkles': _drawSparkles,
  'git-branch': _drawGitBranch,
  'scroll-text': _drawScrollText,
  'alarm-clock': _drawAlarmClock,
  'book-open': _drawBookOpen,
  'database': _drawDatabase,
  'globe': _drawGlobe,
  'gauge': _drawGauge,
};

final _drawGauge = _paths(const ['m12 14 4-4', 'M3.34 19a10 10 0 1 1 17.32 0']);

final _drawGlobe = _paths(const [
  'M2 12a10 10 0 1 0 20 0a10 10 0 1 0-20 0',
  'M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20',
  'M2 12h20',
]);

final _drawBookOpen = _paths(const [
  'M12 7v14',
  'M3 18a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h5a4 4 0 0 1 4 4 4 4 0 0 1 4-4h5'
      'a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1h-6a3 3 0 0 0-3 3 3 3 0 0 0-3-3z',
]);

final _drawDatabase = _paths(const [
  'M3 5a9 3 0 1 0 18 0a9 3 0 1 0-18 0',
  'M3 5V19A9 3 0 0 0 21 19V5',
  'M3 12A9 3 0 0 0 21 12',
]);

final _drawAlarmClock = _paths(const [
  'M4 13a8 8 0 1 0 16 0a8 8 0 1 0-16 0',
  'M12 9v4l2 2',
  'M5 3 2 6',
  'm22 6-3-3',
  'M6.38 18.7 4 21',
  'M17.64 18.67 20 21',
]);

final _drawScrollText = _paths(const [
  'M15 12h-5',
  'M15 8h-5',
  'M19 17V5a2 2 0 0 0-2-2H4',
  'M8 21h12a2 2 0 0 0 2-2v-1a1 1 0 0 0-1-1H11a1 1 0 0 0-1 1v1a2 2 0 1 1-4 0'
      'V5a2 2 0 1 0-4 0v2a1 1 0 0 0 1 1h3',
]);

final _drawGitBranch = _paths(const [
  'M6 3v12',
  'M15 6a3 3 0 1 0 6 0a3 3 0 1 0-6 0',
  'M3 18a3 3 0 1 0 6 0a3 3 0 1 0-6 0',
  'M18 9a9 9 0 0 1-9 9',
]);

final _drawSparkles = _paths(const [
  'M9.937 15.5A2 2 0 0 0 8.5 14.063l-6.135-1.582a.5.5 0 0 1 0-.962'
      'L8.5 9.936A2 2 0 0 0 9.937 8.5l1.582-6.135a.5.5 0 0 1 .963 0'
      'L14.063 8.5A2 2 0 0 0 15.5 9.937l6.135 1.581a.5.5 0 0 1 0 .964'
      'L15.5 14.063a2 2 0 0 0-1.437 1.437l-1.582 6.135a.5.5 0 0 1-.963 0z',
  'M20 3v4',
  'M22 5h-4',
  'M4 17v2',
  'M5 18H3',
]);

final _drawArrowLeft = _paths(const ['m12 19-7-7 7-7', 'M19 12H5']);

final _drawTerminal = _paths(const ['m4 17 6-6-6-6', 'M12 19h8']);

final _drawPlus = _paths(const ['M5 12h14', 'M12 5v14']);

final _drawArrowDown = _paths(const ['M12 5v14', 'm19 12-7 7-7-7']);

/// An extension's 16px icon: the Lucide icon it names, or for one this app
/// can't draw, the first letter of [name].
class ExtensionGlyph extends StatelessWidget {
  const ExtensionGlyph({
    super.key,
    required this.icon,
    required this.name,
    required this.color,
    this.size = 16,
  });

  final String icon;
  final String name;
  final Color color;
  final double size;

  @override
  Widget build(BuildContext context) {
    final draw = _lucide[icon];
    if (draw != null) {
      return CustomPaint(
        size: Size.square(size),
        painter: _LucidePainter(color, draw),
      );
    }
    return SizedBox.square(
      dimension: size,
      child: Center(
        child: Text(
          name.isEmpty ? '?' : name.characters.first.toUpperCase(),
          style: TextStyles.label.copyWith(color: color, height: 1),
        ),
      ),
    );
  }
}

/// A 16px ×, for closing a panel.
class CloseGlyph extends StatelessWidget {
  const CloseGlyph({super.key, required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) => CustomPaint(
    size: const Size.square(16),
    painter: _LucidePainter(color, _drawX),
  );
}

/// An on/off switch: a 28×16 pill, signal when on, with a knob that slides.
class Toggle extends StatelessWidget {
  const Toggle({
    super.key,
    required this.value,
    required this.onChanged,
    required this.label,
  });

  final bool value;
  final ValueChanged<bool>? onChanged;

  /// What it turns on, for assistive technology and tests.
  final String label;

  @override
  Widget build(BuildContext context) =>
      // A control's label is no part of a selection.
      Unselectable(child: _control(context));

  Widget _control(BuildContext context) {
    final palette = Palette.of(context);
    final enabled = onChanged != null;
    return Semantics(
      toggled: value,
      enabled: enabled,
      label: label,
      child: Pressable(
        onTap: enabled ? () => onChanged!(!value) : null,
        builder: (context, hovered) => AnimatedContainer(
          duration: Motion.fast,
          curve: Curves.easeOut,
          width: 28,
          height: 16,
          padding: const EdgeInsets.all(2),
          alignment: value ? Alignment.centerRight : Alignment.centerLeft,
          decoration: BoxDecoration(
            color: value
                ? palette.signal.withValues(alpha: enabled ? 1 : 0.5)
                : hovered
                ? palette.text.withValues(alpha: 0.2)
                : palette.text.withValues(alpha: 0.14),
            borderRadius: BorderRadius.circular(8),
          ),
          child: Container(
            width: 12,
            height: 12,
            decoration: BoxDecoration(
              color: value ? const Color(0xFF000000) : palette.canvas,
              shape: BoxShape.circle,
            ),
          ),
        ),
      ),
    );
  }
}

/// A small checkbox mark: a 10px square, filled with signal when on. [dim]
/// greys it, for a choice that's fixed.
class CheckMark extends StatelessWidget {
  const CheckMark({super.key, required this.on, this.dim = false});

  final bool on;
  final bool dim;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final fill = dim ? palette.textFaint : palette.signal;
    return AnimatedContainer(
      duration: Motion.fast,
      width: 10,
      height: 10,
      decoration: BoxDecoration(
        color: on ? fill : const Color(0x00000000),
        borderRadius: BorderRadius.circular(2),
        border: Border.all(color: on ? fill : palette.textFaint),
      ),
    );
  }
}

/// Eases something new in, such as a conversation entry or a checklist
/// item that moved: it fades in and rises the last few pixels, in
/// [Motion.base]. With [animate] false it's simply there.
class Arrival extends StatelessWidget {
  const Arrival({
    super.key,
    required this.animate,
    required this.child,
    this.from = 0,
    this.rise = true,
  });

  final bool animate;
  final Widget child;

  /// The opacity it starts at: 0 for something new, more for something
  /// that was already there faded (a queued message being delivered).
  final double from;

  /// Whether it rises into place; false when it stays where it was.
  final bool rise;

  @override
  Widget build(BuildContext context) => TweenAnimationBuilder<double>(
    tween: Tween(begin: animate ? 0 : 1, end: 1),
    duration: Motion.base,
    curve: Curves.easeOut,
    // The same widgets throughout, so the entry keeps its state when done.
    builder: (context, t, child) => Opacity(
      opacity: from + (1 - from) * t,
      child: Transform.translate(
        offset: Offset(0, rise ? (1 - t) * Space.s : 0),
        child: child,
      ),
    ),
    child: child,
  );
}
