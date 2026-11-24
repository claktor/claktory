import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'identity.dart';
import 'mobile/back.dart';
import 'mobile/input.dart';
import 'mobile/metrics.dart';
import 'mobile/presentation.dart';
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
  Widget build(BuildContext context) => isPhoneLayout(context)
      // A phone has no window to drag, and its status bar is room enough.
      ? const SizedBox(height: Space.s)
      : const WindowDrag(child: SizedBox(height: titleBarHeight));
}

/// What a scroll view holds, for selecting in it under the app's zoom.
/// Flutter's scroll view, inside a selectable area, adds its offset to a
/// drag's position in its own units and takes it off in the window's, so
/// at any zoom but 100% a selection lands (zoom − 1) × the offset away
/// from the pointer: lines away down a long file (Matt). This takes that
/// back off before what it holds sees the drag. Put it right inside each
/// scroll view (its child, or each of a list's items) whose text can be
/// selected; it does nothing where nothing is selectable or nothing
/// zooms. [trail] follows what it copies when the selection runs past its
/// end: a line's break, in a list of lines.
class ScrolledSelection extends StatefulWidget {
  const ScrolledSelection({super.key, this.trail = '', required this.child});

  final String trail;
  final Widget child;

  @override
  State<ScrolledSelection> createState() => _ScrolledSelectionState();
}

class _ScrolledSelectionState extends State<ScrolledSelection> {
  late final _delegate = _ScrolledSelectionDelegate(
    () => mounted ? Scrollable.maybeOf(context) : null,
  )..trail = widget.trail;

  @override
  void didUpdateWidget(ScrolledSelection old) {
    super.didUpdateWidget(old);
    _delegate.trail = widget.trail;
  }

  @override
  void dispose() {
    _delegate.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final scrollable = Scrollable.maybeOf(context);
    // A scroll view outside any selectable area takes no part in it.
    if (scrollable == null ||
        SelectionContainer.maybeOf(scrollable.context) == null) {
      return widget.child;
    }
    return SelectionContainer(delegate: _delegate, child: widget.child);
  }
}

class _ScrolledSelectionDelegate extends StaticSelectionContainerDelegate {
  _ScrolledSelectionDelegate(this.scrollable);

  final ScrollableState? Function() scrollable;
  String trail = '';

  @override
  SelectedContent? getSelectedContent() {
    final content = super.getSelectedContent();
    if (trail.isEmpty || content == null) return content;
    final range = getSelection();
    if (range == null ||
        math.max(range.startOffset, range.endOffset) < contentLength) {
      return content;
    }
    return SelectedContent(plainText: content.plainText + trail);
  }

  /// What the scroll view added to a position the window gave it: its
  /// offset, scaled by the zoom less one.
  Offset _skew() {
    final state = scrollable();
    final box = state?.context.findRenderObject();
    if (state == null || box is! RenderBox || !box.attached) {
      return Offset.zero;
    }
    // How big a unit along it is in the window.
    final vertical = axisDirectionToAxis(state.axisDirection) == Axis.vertical;
    final unit = vertical ? const Offset(0, 1) : const Offset(1, 0);
    final zoom =
        (box.localToGlobal(unit) - box.localToGlobal(Offset.zero)).distance;
    final extra = (zoom - 1) * state.position.pixels;
    if (extra.abs() < 0.01) return Offset.zero;
    return switch (state.axisDirection) {
      AxisDirection.down => Offset(0, extra),
      AxisDirection.up => Offset(0, -extra),
      AxisDirection.right => Offset(extra, 0),
      AxisDirection.left => Offset(-extra, 0),
    };
  }

  @override
  SelectionResult handleSelectionEdgeUpdate(SelectionEdgeUpdateEvent event) {
    final skew = _skew();
    final at = event.globalPosition;
    if (skew == Offset.zero || !at.isFinite) {
      return super.handleSelectionEdgeUpdate(event);
    }
    return super.handleSelectionEdgeUpdate(
      event.type == SelectionEventType.endEdgeUpdate
          ? SelectionEdgeUpdateEvent.forEnd(
              globalPosition: at - skew,
              granularity: event.granularity,
            )
          : SelectionEdgeUpdateEvent.forStart(
              globalPosition: at - skew,
              granularity: event.granularity,
            ),
    );
  }
}

/// [parent], the app's way of scrolling, with one change while the app is
/// zoomed ([zoomed]): a drag that selects never scrolls the view by
/// itself. Flutter's view scrolls while a drag's edge is past its end, but
/// under the zoom it takes the pointer to be (zoom − 1) × its offset away,
/// so a drag held still well inside it ran off scrolling, screens at a
/// time (Abbacchio, at 125%). A wheel or trackpad still scrolls it
/// meanwhile, and [ScrolledSelection] keeps the selection under the
/// pointer. [AppZoom] puts it round the whole app.
class ZoomedSelectionScrollBehavior extends ScrollBehavior {
  const ZoomedSelectionScrollBehavior(this.parent, {required this.zoomed});

  final ScrollBehavior parent;
  final bool zoomed;

  @override
  ScrollPhysics getScrollPhysics(BuildContext context) {
    final physics = parent.getScrollPhysics(context);
    return zoomed ? _NoSelectionEdgeScroll(parent: physics) : physics;
  }

  @override
  Widget buildOverscrollIndicator(
    BuildContext context,
    Widget child,
    ScrollableDetails details,
  ) => parent.buildOverscrollIndicator(context, child, details);

  @override
  Widget buildScrollbar(
    BuildContext context,
    Widget child,
    ScrollableDetails details,
  ) => parent.buildScrollbar(context, child, details);

  @override
  TargetPlatform getPlatform(BuildContext context) =>
      parent.getPlatform(context);

  @override
  Set<PointerDeviceKind> get dragDevices => parent.dragDevices;

  @override
  MultitouchDragStrategy getMultitouchDragStrategy(BuildContext context) =>
      parent.getMultitouchDragStrategy(context);

  @override
  Set<LogicalKeyboardKey> get pointerAxisModifiers =>
      parent.pointerAxisModifiers;

  @override
  GestureVelocityTrackerBuilder velocityTrackerBuilder(BuildContext context) =>
      parent.velocityTrackerBuilder(context);

  @override
  ScrollViewKeyboardDismissBehavior getKeyboardDismissBehavior(
    BuildContext context,
  ) => parent.getKeyboardDismissBehavior(context);

  @override
  bool shouldNotify(ZoomedSelectionScrollBehavior oldDelegate) =>
      zoomed != oldDelegate.zoomed ||
      parent.runtimeType != oldDelegate.parent.runtimeType ||
      parent.shouldNotify(oldDelegate.parent);
}

/// [parent]'s scrolling, without a selecting drag's own.
class _NoSelectionEdgeScroll extends ScrollPhysics {
  const _NoSelectionEdgeScroll({super.parent});

  @override
  _NoSelectionEdgeScroll applyTo(ScrollPhysics? ancestor) =>
      _NoSelectionEdgeScroll(parent: buildParent(ancestor));

  @override
  bool get allowSelectionEdgeScrolling => false;
}

/// Whether a text field has the keyboard now (a comment, a review's
/// answer, a find bar, a message box): its keys are its own.
bool typingInField() =>
    FocusManager.instance.primaryFocus?.context
        ?.findAncestorStateOfType<EditableTextState>() !=
    null;

/// Key shortcuts for what's under them, as [CallbackShortcuts] binds them,
/// that never take a key while a text field has focus: Space types a space
/// in a video's comment instead of playing it (Matt), Escape stays the
/// field's. For a viewer's own keys.
class ViewerKeys extends StatelessWidget {
  const ViewerKeys({super.key, required this.bindings, required this.child});

  final Map<ShortcutActivator, VoidCallback> bindings;
  final Widget child;

  KeyEventResult _key(FocusNode node, KeyEvent event) {
    if (typingInField()) return KeyEventResult.ignored;
    for (final MapEntry(key: activator, value: callback) in bindings.entries) {
      if (activator.accepts(event, HardwareKeyboard.instance)) {
        callback();
        return KeyEventResult.handled;
      }
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) => Focus(
    canRequestFocus: false,
    skipTraversal: true,
    onKeyEvent: _key,
    child: child,
  );
}

/// A clickable region that tells its builder whether the pointer is over
/// it, or a finger is down on it (a phone has no pointer to hover).
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

  /// A finger is down on it.
  var _pressed = false;

  void _press(bool pressed) {
    if (mounted && pressed != _pressed) setState(() => _pressed = pressed);
  }

  @override
  Widget build(BuildContext context) {
    final tappable = widget.onTap != null;
    return MouseRegion(
      cursor: tappable ? SystemMouseCursors.click : SystemMouseCursors.basic,
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: widget.onTap,
        onTapDown: tappable
            ? (details) => _press(details.kind == PointerDeviceKind.touch)
            : null,
        onTapUp: tappable ? (_) => _press(false) : null,
        onTapCancel: tappable ? () => _press(false) : null,
        onSecondaryTap: widget.onSecondaryTap,
        child: widget.builder(context, _hovered || _pressed),
      ),
    );
  }
}

/// A small text button. `primary` is the one main action of a form; it's
/// filled with the text color, since signal is never a large fill. On a
/// phone it's a finger's size: 44pt tall, its label in 17pt.
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

  /// A finger is down on it.
  var _pressed = false;

  @override
  Widget build(BuildContext context) =>
      // A control's label is no part of a selection.
      Unselectable(child: _control(context));

  Widget _control(BuildContext context) {
    final palette = Palette.of(context);
    final enabled = widget.onPressed != null;
    final phone = touchFirst(context);
    final hovered = _hovered || _pressed;
    final Color background;
    final Color foreground;
    if (widget.primary) {
      background = enabled
          ? palette.text.withValues(alpha: hovered ? 0.88 : 1)
          : palette.selected;
      foreground = enabled ? palette.canvas : palette.textFaint;
    } else if (phone) {
      // With no pointer to hover, a quiet button shows it's one at rest:
      // a faint fill, as iOS's gray buttons have.
      background = !enabled
          ? palette.hover.withValues(alpha: palette.hover.a / 2)
          : hovered
          ? palette.selected
          : palette.hover;
      foreground = enabled ? palette.text : palette.textFaint;
    } else {
      background = hovered && enabled ? palette.hover : const Color(0x00000000);
      foreground = !enabled
          ? palette.textFaint
          : hovered
          ? palette.text
          : palette.textMuted;
    }
    // A button to a screen reader (TalkBack, VoiceOver), dimmed while it
    // can't be pressed.
    return Semantics(
      button: true,
      enabled: enabled,
      child: FocusableActionDetector(
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
          onTapDown: enabled
              ? (details) => setState(
                  () => _pressed = details.kind == PointerDeviceKind.touch,
                )
              : null,
          onTapUp: enabled ? (_) => setState(() => _pressed = false) : null,
          onTapCancel: enabled ? () => setState(() => _pressed = false) : null,
          child: Container(
            // At least a finger's height on a phone, taller with larger text.
            height: phone ? null : 28,
            constraints: phone
                ? BoxConstraints(minHeight: Metrics.of(context).target)
                : null,
            padding: EdgeInsets.symmetric(
              horizontal: phone ? Touch.gutter : Space.m,
              vertical: phone ? Space.s : 0,
            ),
            decoration: BoxDecoration(
              color: background,
              borderRadius: BorderRadius.circular(Radii.control),
              border: Border.all(
                color: _focused ? palette.signal : const Color(0x00000000),
              ),
            ),
            // As wide as its label, wherever it's placed. On a phone its
            // words grow only to 2×, so a long one ("notification") still
            // fits a 320pt screen whole rather than breaking mid-word.
            child: Center(
              widthFactor: 1,
              child: MediaQuery.withClampedTextScaling(
                maxScaleFactor: phone ? 2 : double.infinity,
                child: Text(
                  widget.label,
                  maxLines: phone ? null : 1,
                  textAlign: TextAlign.center,
                  style: (phone ? PhoneType.label : TextStyles.label).copyWith(
                    color: foreground,
                    fontWeight: phone && widget.primary
                        ? FontWeight.w600
                        : null,
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

/// The box a [TextField] without a border sits in, drawn by its owner (the
/// message box, a search field): makes the whole box the field, to a finger
/// and to a screen reader, as a bordered field's own box is. A tap anywhere
/// in it, or a screen reader's double tap, focuses the field; it's one
/// element as big as the box, with the field's label, text and editing
/// actions. Give the field the same [focusNode].
class TextFieldBox extends StatelessWidget {
  const TextFieldBox({super.key, required this.focusNode, required this.child});

  final FocusNode focusNode;
  final Widget child;

  @override
  Widget build(BuildContext context) => MergeSemantics(
    child: Semantics(
      onTap: focusNode.requestFocus,
      child: GestureDetector(
        behavior: HitTestBehavior.translucent,
        onTap: focusNode.requestFocus,
        excludeFromSemantics: true,
        child: child,
      ),
    ),
  );
}

/// A text field on the widgets layer: an [EditableText] with mouse selection,
/// a placeholder, and a hairline border that turns signal when focused.
///
/// On an iPhone it selects as iOS does (handles, the magnifier, the
/// system's edit menu: mobile/input.dart) and brings the keyboard [kind]
/// asks for; on a phone its text is 17pt in a field at least 44pt tall.
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
    this.onEditingComplete,
    this.bordered = true,
    this.obscureText = false,
    this.kind,
    this.keyboardType,
    this.textInputAction,
    this.autofillHints,
    this.textCapitalization,
    this.inputFormatters,
    this.autocorrect,
    this.scrollPadding = const EdgeInsets.all(20),
    this.editItems,
    this.semanticsLabel,
  });

  final TextEditingController controller;
  final FocusNode? focusNode;
  final String placeholder;

  /// Null grows without limit.
  final int? maxLines;
  final int? minLines;
  final bool autofocus;
  final ValueChanged<String>? onSubmitted;

  /// Return/Search/Go without leaving the field, when set.
  final VoidCallback? onEditingComplete;
  final bool bordered;

  /// Hides what's typed, for secrets.
  final bool obscureText;

  /// What it's for: the keyboard, autofill and corrections to offer. The
  /// fields after it override what it sets.
  final InputKind? kind;
  final TextInputType? keyboardType;
  final TextInputAction? textInputAction;
  final Iterable<String>? autofillHints;
  final TextCapitalization? textCapitalization;
  final List<TextInputFormatter>? inputFormatters;
  final bool? autocorrect;

  /// How much room to keep around it when it's scrolled into view.
  final EdgeInsets scrollPadding;

  /// More items for the iPhone's edit menu, after Copy and Paste.
  final ExtraEditItems<EditableTextState>? editItems;

  /// What the field is, to a screen reader (TalkBack, VoiceOver), typed in
  /// or not: "Message to June". Without it the placeholder speaks for it
  /// while it's empty, and nothing once it isn't.
  final String? semanticsLabel;

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

  /// The focus node listened to, to build the field again when it gains or
  /// loses focus: the editable's semantics (focused, and its editing
  /// actions for a screen reader) follow its build, which a gained focus
  /// alone doesn't bring when the selection is already valid.
  FocusNode? _listened;

  void _listen() {
    final node = _focusNode;
    if (node == _listened) return;
    _listened?.removeListener(_focusChanged);
    _listened = node..addListener(_focusChanged);
  }

  void _focusChanged() {
    if (mounted) setState(() {});
  }

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _listen();
  }

  @override
  void didUpdateWidget(TextField old) {
    super.didUpdateWidget(old);
    _listen();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    final active = _isActive(state);
    if (active != _active) setState(() => _active = active);
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    _listened?.removeListener(_focusChanged);
    _ownFocusNode?.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final phone = touchFirst(context);
    final style = TypeScale.of(context).body.copyWith(color: palette.text);
    final singleLine = widget.maxLines == 1;
    final setup = widget.kind?.setup;
    final obscure = widget.obscureText || (setup?.obscureText ?? false);
    final native = nativeTextSelection(context, extra: widget.editItems);
    final editable = EditableText(
      key: editableTextKey,
      controller: widget.controller,
      focusNode: _focusNode,
      style: style,
      cursorColor: palette.signal,
      backgroundCursorColor: palette.textFaint,
      selectionColor: palette.signal.withValues(alpha: 0.28),
      cursorWidth: 1.5,
      // The caret blinks on and off, two frames a second, as macOS's own
      // fields do; a fade would draw at the display's rate while a field
      // is focused, which the message box is most of the day (#384).
      cursorOpacityAnimates: false,
      showCursor: _active,
      maxLines: widget.maxLines,
      minLines: widget.minLines,
      autofocus: widget.autofocus,
      obscureText: obscure,
      autocorrect: widget.autocorrect ?? setup?.autocorrect ?? !obscure,
      enableSuggestions: !obscure && (setup?.enableSuggestions ?? true),
      keyboardType:
          widget.keyboardType ??
          setup?.keyboardType ??
          (singleLine ? TextInputType.text : TextInputType.multiline),
      textInputAction:
          widget.textInputAction ??
          setup?.textInputAction ??
          (singleLine ? TextInputAction.done : TextInputAction.newline),
      autofillHints: widget.autofillHints ?? setup?.autofillHints,
      textCapitalization:
          widget.textCapitalization ??
          setup?.textCapitalization ??
          TextCapitalization.none,
      inputFormatters: widget.inputFormatters ?? setup?.inputFormatters,
      scrollPadding: widget.scrollPadding,
      selectionControls: native.controls,
      contextMenuBuilder: native.contextMenuBuilder,
      magnifierConfiguration: native.magnifier,
      onSubmitted: widget.onSubmitted,
      onEditingComplete: widget.onEditingComplete,
      rendererIgnoresPointer: true,
    );
    // Several lines of placeholder take turns, one at a time.
    final hints = placeholderLines(widget.placeholder);
    final hintStyle = style.copyWith(color: palette.textFaint);
    final label = widget.semanticsLabel;
    final placeholder = ListenableBuilder(
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
    );
    final field = Stack(
      children: [
        // With a label, it speaks for the field instead, once.
        label == null ? placeholder : ExcludeSemantics(child: placeholder),
        _gestures.buildGestureDetector(
          behavior: HitTestBehavior.translucent,
          child: label == null
              ? editable
              : Semantics(label: label, child: editable),
        ),
      ],
    );
    final input = MouseRegion(
      cursor: SystemMouseCursors.text,
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      // On an iPhone a tap focuses it once the finger lifts, as iOS does,
      // so a scroll that starts on it doesn't bring the keyboard up.
      child: nativeSelectionHere
          ? field
          : Listener(
              onPointerDown: (_) => _focusNode.requestFocus(),
              child: field,
            ),
    );
    if (!widget.bordered) return input;
    // The whole box is the field, to a finger and to a screen reader: a tap
    // on its padding focuses it too, and it's one element as big as the box
    // (its label, text and editing actions merged into it).
    // A screen reader's double tap focuses it, as a finger's does (the
    // gestures inside say nothing to one). A field without a border is in
    // a box of its owner's, which does the same for it (TextFieldBox).
    void focus() => editableTextKey.currentState?.requestKeyboard();
    return MergeSemantics(
      child: Semantics(
        onTap: focus,
        child: GestureDetector(
          behavior: HitTestBehavior.translucent,
          onTap: focus,
          excludeFromSemantics: true,
          child: ListenableBuilder(
            listenable: _focusNode,
            builder: (context, child) => Container(
              // A finger's height on a phone, growing with its text.
              constraints: phone
                  ? BoxConstraints(minHeight: Metrics.of(context).target)
                  : null,
              alignment: phone && singleLine ? Alignment.centerLeft : null,
              padding: phone
                  ? const EdgeInsets.symmetric(
                      horizontal: Space.m,
                      vertical: 10,
                    )
                  : const EdgeInsets.symmetric(
                      horizontal: Space.s + 2,
                      vertical: Space.s - 2,
                    ),
              decoration: BoxDecoration(
                color: palette.canvas,
                borderRadius: BorderRadius.circular(Radii.control),
                border: Border.all(
                  color: _focusNode.hasFocus
                      ? palette.signal
                      : palette.hairline,
                ),
              ),
              child: child,
            ),
            child: input,
          ),
        ),
      ),
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
    with SingleTickerProviderStateMixin, WidgetsBindingObserver {
  var _index = 0;
  Timer? _timer;
  late final _slide = AnimationController(vsync: this, duration: rotateSlide);

  /// The window is in front: a window in the background doesn't rotate.
  var _focused = true;

  /// The next line is sliding in.
  var _sliding = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    final state = WidgetsBinding.instance.lifecycleState;
    _focused = state == null || state == AppLifecycleState.resumed;
    _schedule();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    final focused = state == AppLifecycleState.resumed;
    if (focused == _focused) return;
    _focused = focused;
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
    WidgetsBinding.instance.removeObserver(this);
    _timer?.cancel();
    _slide.dispose();
    super.dispose();
  }

  void _schedule() {
    _timer?.cancel();
    _timer = null;
    // A slide under way finishes and schedules the next itself.
    if (widget.paused ||
        !_focused ||
        widget.lines.length < 2 ||
        _slide.isAnimating) {
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
    setState(() => _sliding = true);
    _slide.forward(from: 0).whenCompleteOrCancel(() {
      if (!mounted) return;
      setState(() {
        _index = (_index + 1) % widget.lines.length;
        _sliding = false;
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
    // The slide moves the two lines in paint only (a Flow), never
    // rebuilding or laying out: a rebuild under a LayoutBuilder (the
    // message box's) would lay out and repaint the window every frame. It
    // rebuilds only as a slide starts and ends.
    return RepaintBoundary(
      child: ClipRect(
        child: Stack(
          children: [
            // As tall as a line, as wide as there's room for.
            SizedBox(
              width: double.infinity,
              child: Text(' ', style: widget.style, maxLines: 1),
            ),
            Positioned.fill(
              child: Flow(
                delegate: _SlideFlow(_slide),
                children: [
                  line(_index),
                  // The next only while it slides in.
                  if (_sliding) line(_index + 1),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// Paints the line showing sliding up and out, and the next one in after
/// it, as [slide] runs.
class _SlideFlow extends FlowDelegate {
  _SlideFlow(this.slide) : super(repaint: slide);

  final Animation<double> slide;

  @override
  void paintChildren(FlowPaintingContext context) {
    final t = Curves.easeInOutCubic.transform(slide.value);
    final height = context.size.height;
    if (t == 0 || context.childCount < 2) {
      context.paintChild(0);
      return;
    }
    context.paintChild(
      0,
      transform: Matrix4.translationValues(0, -t * height, 0),
      opacity: 1 - t,
    );
    context.paintChild(
      1,
      transform: Matrix4.translationValues(0, (1 - t) * height, 0),
      opacity: t,
    );
  }

  @override
  BoxConstraints getConstraintsForChild(int i, BoxConstraints constraints) =>
      constraints.loosen();

  @override
  bool shouldRepaint(_SlideFlow old) => old.slide != slide;
}

class SelectOption<T> {
  const SelectOption(this.value, this.label);

  final T value;
  final String label;
}

/// A dropdown. Opens with a click, Enter, Space or Down; arrow keys move,
/// Enter picks, Esc or a click outside closes. On a phone it's 44pt tall
/// and a tap opens the choices in a sheet ([showChoice]) titled [title].
class Select<T> extends StatefulWidget {
  const Select({
    super.key,
    required this.value,
    required this.options,
    required this.onChanged,
    this.title,
  });

  final T value;
  final List<SelectOption<T>> options;
  final ValueChanged<T> onChanged;

  /// What's chosen, over the phone's sheet of choices ("Model").
  final String? title;

  @override
  State<Select<T>> createState() => _SelectState<T>();
}

/// A Select menu's height at most, and each of its rows'.
const _menuHeight = 280.0;
const _rowHeight = 28.0;

class _SelectState<T> extends State<Select<T>> {
  final _menu = OverlayPortalController();
  final _link = LayerLink();
  final _tapRegion = Object();
  late final _focusNode = FocusNode(onKeyEvent: _onKey);
  var _highlighted = 0;
  var _hovered = false;
  ScrollController? _scroll;

  @override
  void dispose() {
    _focusNode.dispose();
    _scroll?.dispose();
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
    if (isPhoneLayout(context)) {
      unawaited(_choose());
      return;
    }
    _focusNode.requestFocus();
    // Opens at what's chosen (21:30 in a day of half hours), in the
    // middle of the menu where it can be.
    final shown = _menuHeight - 2 * Space.xs - 2;
    final most = math.max(0.0, widget.options.length * _rowHeight - shown);
    final at = (_selectedIndex + 0.5) * _rowHeight - shown / 2;
    _scroll?.dispose();
    _scroll = ScrollController(initialScrollOffset: at.clamp(0.0, most));
    setState(() => _highlighted = _selectedIndex);
    _menu.show();
  }

  void _close() {
    if (_menu.isShowing) _menu.hide();
  }

  /// A phone's sheet of choices.
  Future<void> _choose() async {
    // Nothing keeps focus while the sheet is up, so closing it gives none
    // back: a text field focused before (its keyboard hidden) would raise
    // the keyboard again and scroll itself into view.
    FocusManager.instance.primaryFocus?.unfocus();
    final chosen = await showChoice<T>(
      context,
      title: widget.title,
      options: widget.options,
      selected: widget.value,
    );
    if (chosen != null && chosen != widget.value) widget.onChanged(chosen);
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
    final phone = touchFirst(context);
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
                    height: phone ? null : 30,
                    constraints: phone
                        ? BoxConstraints(minHeight: Metrics.of(context).target)
                        : null,
                    padding: EdgeInsets.symmetric(
                      horizontal: phone ? Space.m : Space.s + 2,
                      vertical: phone ? Space.s : 0,
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
                            maxLines: phone ? null : 1,
                            overflow: phone ? null : TextOverflow.ellipsis,
                            style: TypeScale.of(context).body
                                .copyWith(color: palette.text),
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
            constraints: const BoxConstraints(
              maxHeight: _menuHeight,
              minWidth: 200,
            ),
            padding: const EdgeInsets.all(Space.xs),
            decoration: BoxDecoration(
              color: palette.sidebar,
              borderRadius: BorderRadius.circular(Radii.panel),
              border: Border.all(color: palette.hairline),
              boxShadow: floatingShadow,
            ),
            child: ListView.builder(
              controller: _scroll,
              shrinkWrap: true,
              itemExtent: _rowHeight,
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
                      height: _rowHeight,
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
/// Esc and a click on the scrim close it too. On a phone it's a sheet from
/// the bottom instead ([PhoneSheet]), which dragging down closes too.
Future<void> showModal(
  BuildContext context,
  Widget Function(BuildContext context, VoidCallback close) builder,
) {
  if (isPhoneLayout(context)) return showModalSheet(context, builder);
  final overlay = Overlay.of(context);
  final done = Completer<void>();
  late final OverlayEntry entry;
  BackHandle? back;
  void close() {
    if (done.isCompleted) return;
    back?.release();
    back = null;
    entry.remove();
    entry.dispose();
    done.complete();
  }

  entry = OverlayEntry(
    builder: (context) => _Modal(close: close, child: builder(context, close)),
  );
  overlay.insert(entry);
  // Android's Back closes it as Esc does, before anything under it; a
  // Claktory replaced under it takes it away without closing anything.
  back = PhoneBack.maybeOf(context)
      ?.register(CallbackBackToken(onBack: close, onRetired: close));
  return done.future;
}

class _Modal extends StatefulWidget {
  const _Modal({required this.close, required this.child});

  final VoidCallback close;
  final Widget child;

  @override
  State<_Modal> createState() => _ModalState();
}

class _ModalState extends State<_Modal> {
  final _scope = FocusScopeNode(debugLabel: 'modal');

  @override
  void initState() {
    super.initState();
    // It takes the keys as it opens, even from what had them under it (the
    // iPad's paired and full tiers): a scope's autofocus waits for nothing
    // else to be focused, and Esc must reach its binding (Sorbet). A field
    // inside that took them on its own keeps them.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && !_scope.hasFocus) _scope.requestFocus();
    });
  }

  @override
  void dispose() {
    _scope.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final close = widget.close;
    final child = widget.child;
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
          // Centred in what the on-screen keyboard leaves (an iPad with
          // none attached), so a field and its buttons stay in sight; a
          // form's fields scroll when even that is too short (Sorbet).
          Padding(
            padding: EdgeInsets.only(
              bottom: MediaQuery.viewInsetsOf(context).bottom,
            ),
            child: Center(
              // Esc above the scope: the scope itself has the keys when
              // nothing in it took them.
              child: CallbackShortcuts(
                bindings: {
                  const SingleActivator(LogicalKeyboardKey.escape): close,
                },
                child: FocusScope(
                  node: _scope,
                  autofocus: true,
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
/// [child] (or [end]-aligned), or with [beside] to its right, centered on it. It never takes
/// the pointer itself. With [holdToShow], touching and holding [child]
/// shows it too, until the finger lifts (on a phone, with no pointer).
class HoverCard extends StatefulWidget {
  const HoverCard({
    super.key,
    required this.card,
    required this.child,
    this.beside = false,
    this.end = false,
    this.holdToShow = false,
    this.interactive = false,
  });

  final WidgetBuilder card;
  final Widget child;
  final bool beside;

  /// Right-aligned with [child] instead, for one near a right edge.
  final bool end;
  final bool holdToShow;

  /// The card takes clicks (it has buttons): it stays open while the
  /// pointer is over [child] or the card, and closes a moment after it
  /// leaves both. With [holdToShow], a long press opens it until a tap
  /// elsewhere.
  final bool interactive;

  @override
  State<HoverCard> createState() => _HoverCardState();
}

class _HoverCardState extends State<HoverCard> {
  final _portal = OverlayPortalController();
  final _link = LayerLink();

  /// Waits a moment, so the card doesn't flash while the pointer passes by.
  Timer? _wait;

  /// An interactive card's moment to be reached after the pointer leaves
  /// [HoverCard.child].
  Timer? _closing;

  @override
  void initState() {
    super.initState();
    // A sheet rising over the screen hides the card, which would stay
    // painted over it.
    sheetsPresented.addListener(_hide);
  }

  @override
  void dispose() {
    sheetsPresented.removeListener(_hide);
    _wait?.cancel();
    _closing?.cancel();
    super.dispose();
  }

  void _enter(PointerEnterEvent _) {
    _closing?.cancel();
    if (_portal.isShowing) return;
    _wait?.cancel();
    _wait = Timer(const Duration(milliseconds: 300), _portal.show);
  }

  void _exit(PointerExitEvent _) {
    if (!widget.interactive) return _hide();
    _wait?.cancel();
    _closeSoon();
  }

  void _closeSoon() {
    _closing?.cancel();
    _closing = Timer(const Duration(milliseconds: 200), _hide);
  }

  void _hide() {
    _wait?.cancel();
    _closing?.cancel();
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
              : widget.end
              ? Alignment.topRight
              : Alignment.topLeft,
          followerAnchor: widget.beside
              ? Alignment.centerLeft
              : widget.end
              ? Alignment.bottomRight
              : Alignment.bottomLeft,
          offset: widget.beside
              ? const Offset(Space.xs, 0)
              : const Offset(0, -Space.xs),
          child: Align(
            alignment: widget.beside
                ? Alignment.centerLeft
                : widget.end
                ? Alignment.bottomRight
                : Alignment.bottomLeft,
            child: _cardInput(
              TweenAnimationBuilder<double>(
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
        child: _held(
          MouseRegion(
            onEnter: _enter,
            onExit: _exit,
            child: widget.holdToShow
                ? GestureDetector(
                    behavior: HitTestBehavior.opaque,
                    onLongPressStart: (_) => _portal.show(),
                    onLongPressEnd: widget.interactive ? null : (_) => _hide(),
                    onLongPressCancel: widget.interactive ? null : _hide,
                    child: widget.child,
                  )
                : widget.child,
          ),
        ),
      ),
    );
  }

  /// The card as it takes the pointer: not at all, or (interactive) staying
  /// open while it's over it.
  Widget _cardInput(Widget card) {
    if (!widget.interactive) return IgnorePointer(child: card);
    return _held(
      MouseRegion(
        onEnter: (_) => _closing?.cancel(),
        onExit: (_) => _closeSoon(),
        child: card,
      ),
    );
  }

  /// A held-open interactive card and its trigger: a tap outside both
  /// closes it.
  Widget _held(Widget child) => widget.interactive && widget.holdToShow
      ? TapRegion(groupId: this, onTapOutside: (_) => _hide(), child: child)
      : child;
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

/// A signal dot that pulses while something is live: from 35% to full and
/// back every 1.8s, eased, moved by [LiveClock] at about 12 fps.
class LiveDot extends StatefulWidget {
  const LiveDot({super.key, this.color});

  /// Signal orange unless given.
  final Color? color;

  /// One way of the pulse; it eases there and back.
  static const half = Duration(milliseconds: 900);

  /// The dot's opacity at [now].
  static double opacityAt(DateTime now) {
    final period = half.inMilliseconds * 2;
    final phase = (now.millisecondsSinceEpoch % period) / period;
    final there = phase < 0.5 ? phase * 2 : 2 - phase * 2;
    return 0.35 + 0.65 * Curves.easeInOut.transform(there);
  }

  @override
  State<LiveDot> createState() => _LiveDotState();
}

class _LiveDotState extends State<LiveDot> with LiveClockSubscriber {
  @override
  LiveBeat get beat => LiveBeat.dots;

  @override
  Widget build(BuildContext context) {
    final color = widget.color ?? Palette.of(context).signal;
    return RepaintBoundary(
      child: CustomPaint(
        size: const Size.square(6),
        painter: _LiveDotPainter(color: color, still: still),
      ),
    );
  }
}

class _LiveDotPainter extends CustomPainter {
  _LiveDotPainter({required this.color, required this.still})
    : super(repaint: still ? null : LiveClock.instance.dots);

  final Color color;
  final bool still;

  @override
  void paint(Canvas canvas, Size size) {
    final opacity = still ? 1.0 : LiveDot.opacityAt(LiveClock.instance.now);
    canvas.drawCircle(
      size.center(Offset.zero),
      size.shortestSide / 2,
      Paint()..color = color.withValues(alpha: color.a * opacity),
    );
  }

  @override
  bool shouldRepaint(_LiveDotPainter old) =>
      old.color != color || old.still != still;
}

/// Which of [LiveClock]'s beats something moves by.
enum LiveBeat { waves, dots }

/// The window's one clock for live indicators: working waves at
/// [waveFps] and live dots at half that, from one timer, so every wave and
/// dot on screen repaints in the same frame and nothing asks for a frame
/// at the display's full rate. It runs only while something subscribes
/// and the app is showing; [now] is the time of its last tick, which every
/// painter reads so they stay in step.
///
/// While it runs, the window's other clocks (the characters' faces, the
/// clocks that count seconds) move on its ticks ([beat]) rather than on
/// timers of their own: each frame on a Mac redraws the whole window, so
/// frames from clocks out of step with each other would add up.
class LiveClock with WidgetsBindingObserver {
  LiveClock._();

  static final instance = LiveClock._();

  /// How often waves move; dots move every other tick.
  static const waveFps = 25;

  /// Ticks for waves, and every other one for dots.
  final waves = _Beat();
  final dots = _Beat();

  /// Every tick, for other clocks to move on while it runs ([running]).
  final beat = _Beat();

  /// Whether it's ticking: other clocks move on [beat] while it is, and on
  /// their own timers while it isn't.
  final running = ValueNotifier(false);

  /// When the last tick was.
  DateTime get now => _now ?? DateTime.now();
  DateTime? _now;

  final _counts = {LiveBeat.waves: 0, LiveBeat.dots: 0};
  Timer? _timer;
  var _tick = 0;
  var _observing = false;
  var _hidden = false;

  /// How many ticks it has made: for tests and profiling.
  int get ticks => _tick;

  void _subscribe(LiveBeat beat) {
    _counts[beat] = _counts[beat]! + 1;
    if (!_observing) {
      WidgetsBinding.instance.addObserver(this);
      _observing = true;
    }
    _schedule();
  }

  void _unsubscribe(LiveBeat beat) {
    _counts[beat] = _counts[beat]! - 1;
    _schedule();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    // Losing focus (inactive) isn't hidden: the window still shows.
    _hidden =
        state == AppLifecycleState.hidden || state == AppLifecycleState.paused;
    _schedule();
  }

  void _schedule() {
    final wanted = !_hidden && _counts.values.any((count) => count > 0);
    if (!wanted) {
      _timer?.cancel();
      _timer = null;
      running.value = false;
      return;
    }
    _timer ??= Timer.periodic(
      const Duration(microseconds: 1000000 ~/ waveFps),
      (_) => _beat(),
    );
    running.value = true;
  }

  void _beat() {
    _tick++;
    _now = DateTime.now();
    if (_counts[LiveBeat.waves]! > 0) waves._notify();
    if (_tick.isEven && _counts[LiveBeat.dots]! > 0) dots._notify();
    beat._notify();
  }
}

class _Beat extends ChangeNotifier {
  void _notify() => notifyListeners();
}

/// Subscribes a live indicator to [LiveClock] while it's mounted, moving
/// (not muted by [TickerMode]) and motion isn't reduced, in which case
/// it's [still].
mixin LiveClockSubscriber<T extends StatefulWidget> on State<T> {
  LiveBeat get beat;

  /// Reduced motion: drawn at rest, off the clock.
  var still = false;
  var _subscribed = false;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    still = MediaQuery.maybeDisableAnimationsOf(context) ?? false;
    final moving = !still && TickerMode.valuesOf(context).enabled;
    if (moving && !_subscribed) LiveClock.instance._subscribe(beat);
    if (!moving && _subscribed) LiveClock.instance._unsubscribe(beat);
    _subscribed = moving;
  }

  @override
  void dispose() {
    if (_subscribed) LiveClock.instance._unsubscribe(beat);
    _subscribed = false;
    super.dispose();
  }
}

/// The window's one clock for what counts seconds ([Ticking]): every
/// second, all of them together, from one timer while nothing else ticks,
/// and on every [LiveClock.waveFps]th of [LiveClock]'s ticks while it
/// runs, so counting clocks never ask for frames of their own. It runs only
/// while something listens.
class Seconds extends ChangeNotifier {
  Seconds._();

  static final instance = Seconds._();

  Timer? _timer;
  var _riding = false;
  var _watching = false;
  var _beats = 0;

  @override
  void addListener(VoidCallback listener) {
    super.addListener(listener);
    _schedule();
  }

  @override
  void removeListener(VoidCallback listener) {
    super.removeListener(listener);
    _schedule();
  }

  void _schedule() {
    final live = LiveClock.instance;
    final wanted = hasListeners;
    if (wanted != _watching) {
      _watching = wanted;
      wanted
          ? live.running.addListener(_schedule)
          : live.running.removeListener(_schedule);
    }
    final ride = wanted && live.running.value;
    if (ride != _riding) {
      _riding = ride;
      _beats = 0;
      ride ? live.beat.addListener(_beat) : live.beat.removeListener(_beat);
    }
    if (wanted && !ride) {
      _timer ??= Timer.periodic(
        const Duration(seconds: 1),
        (_) => notifyListeners(),
      );
    } else {
      _timer?.cancel();
      _timer = null;
    }
  }

  void _beat() {
    if (++_beats < LiveClock.waveFps) return;
    _beats = 0;
    notifyListeners();
  }
}

/// Rebuilds [builder] with the time every second while [active], for clocks
/// that count up (or down) between the backend's updates. They all tick
/// together ([Seconds]).
class Ticking extends StatefulWidget {
  const Ticking({super.key, required this.active, required this.builder});

  final bool active;
  final Widget Function(BuildContext context, DateTime now) builder;

  @override
  State<Ticking> createState() => _TickingState();
}

class _TickingState extends State<Ticking> {
  var _listening = false;

  /// Not muted ([TickerMode]): a window no one can see counts nothing.
  var _moving = true;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _moving = TickerMode.valuesOf(context).enabled;
    _schedule();
  }

  @override
  void didUpdateWidget(Ticking old) {
    super.didUpdateWidget(old);
    if (old.active != widget.active) _schedule();
  }

  void _schedule() {
    final listen = widget.active && _moving;
    if (listen == _listening) return;
    _listening = listen;
    _listening
        ? Seconds.instance.addListener(_tick)
        : Seconds.instance.removeListener(_tick);
  }

  void _tick() => setState(() {});

  @override
  void dispose() {
    if (_listening) Seconds.instance.removeListener(_tick);
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

class _WorkingWaveState extends State<WorkingWave> with LiveClockSubscriber {
  @override
  LiveBeat get beat => LiveBeat.waves;

  @override
  Widget build(BuildContext context) {
    return IgnorePointer(
      child: RepaintBoundary(
        child: ClipRRect(
          borderRadius: BorderRadius.circular(widget.radius),
          child: still
              ? ColoredBox(
                  color: widget.color.withValues(alpha: widget.peak * 0.6),
                )
              : CustomPaint(
                  painter: _WavePainter(color: widget.color, peak: widget.peak),
                  child: const SizedBox.expand(),
                ),
        ),
      ),
    );
  }
}

/// Where every wave is in [WorkingWave.period] at [now], 0 to 1: one
/// shared clock, so all waves move together.
double wavePhase(DateTime now) {
  final period = WorkingWave.period.inMilliseconds;
  return (now.millisecondsSinceEpoch % period) / period;
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
  _WavePainter({required this.color, required this.peak})
    : super(repaint: LiveClock.instance.waves);

  final Color color;
  final double peak;

  @override
  void paint(Canvas canvas, Size size) {
    final band = waveBand(size.width);
    final phase = wavePhase(LiveClock.instance.now);
    final left = waveCenter(phase, size.width) - band / 2;
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
      old.color != color || old.peak != peak;
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
      // On a phone, its own element for VoiceOver: a row or field beside it
      // isn't read as part of it.
      container: touchFirst(context),
      button: true,
      label: label,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => _phoneTarget(
          context,
          AnimatedContainer(
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
      ),
    );
  }
}

/// On touch, [visual] in the middle of a finger's target (44pt on an
/// iPhone or iPad, 48dp on Android; less where its parent has less room);
/// with a pointer, [visual] itself.
Widget _phoneTarget(BuildContext context, Widget visual) => touchFirst(context)
    ? SizedBox.square(
        dimension: Metrics.of(context).target,
        child: Center(child: visual),
      )
    : visual;

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

void _drawUser(Canvas canvas, Paint paint) {
  canvas.drawCircle(const Offset(12, 7), 4, paint);
  _paths(const ['M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2'])(canvas, paint);
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
  'user': _drawUser,
  'list-checks': _drawListChecks,
  'x': _drawX,
  'brain': _drawBrain,
  'arrow-down': _drawArrowDown,
  'arrow-left': _drawArrowLeft,
  'chevron-left': _drawChevronLeft,
  'chevron-right': _drawChevronRight,
  'chevron-up': _drawChevronUp,
  'chevron-down': _drawChevronDown,
  'menu': _drawMenu,
  'search': _drawSearch,
  'list-filter': _drawListFilter,
  'mic': _drawMic,
  'check': _drawCheck,
  'image': _drawImage,
  'minus': _drawMinus,
  'rotate-cw': _drawRotateCw,
  'ellipsis': _drawEllipsis,
  'plus': _drawPlus,
  'terminal': _drawTerminal,
  'sparkles': _drawSparkles,
  'git-branch': _drawGitBranch,
  'history': _drawHistory,
  'scroll-text': _drawScrollText,
  'alarm-clock': _drawAlarmClock,
  'book-open': _drawBookOpen,
  'database': _drawDatabase,
  'globe': _drawGlobe,
  'gauge': _drawGauge,
  'send': _drawSend,
  'panels-top-left': _drawPanelsTopLeft,
  'key-round': _drawKeyRound,
  'monitor': _drawMonitor,
  'laptop-minimal': _drawLaptopMinimal,
  'smartphone': _drawSmartphone,
  'tablet': _drawTablet,
  'monitor-smartphone': _drawMonitorSmartphone,
  'network': _drawNetwork,
  'paperclip': _drawPaperclip,
  'message-square': _drawMessageSquare,
  'message-square-text': _drawMessageSquareText,
  'play': _drawPlay,
  'pause': _drawPause,
  'film': _drawFilm,
  'chart-column': _drawChartColumn,
  'upload': _drawUpload,
  'share': _drawShare,
  'inbox': _drawInbox,
  'bell': _drawBell,
  'message-circle-question': _drawMessageCircleQuestion,
  'shield-alert': _drawShieldAlert,
  'maximize-2': _drawMaximize2,
  'minimize-2': _drawMinimize2,
  'layout-grid': _drawLayoutGrid,
  'pin': _drawPin,
  'pin-off': _drawPinOff,
  'copy': _drawCopy,
  'star': _drawStar,
  'house': _drawHouse,
  'home': _drawHouse,
  'clock': _drawClock,
};

/// Whether the app draws the Lucide icon [name] (an app's menu leaves out
/// an icon it doesn't).
bool hasLucideIcon(String name) => _lucide.containsKey(name);

final _drawStar = _paths(const [
  'M11.525 2.295a.53.53 0 0 1 .95 0l2.31 4.679a2.123 2.123 0 0 0 1.595 1.16'
      'l5.166.756a.53.53 0 0 1 .294.904l-3.736 3.638a2.123 2.123 0 0 0-.611 '
      '1.878l.882 5.14a.53.53 0 0 1-.771.56l-4.618-2.428a2.122 2.122 0 0 0'
      '-1.973 0L6.396 21.01a.53.53 0 0 1-.77-.56l.881-5.139a2.122 2.122 0 0 0'
      '-.611-1.879L2.16 9.795a.53.53 0 0 1 .294-.906l5.165-.755a2.122 2.122 0 '
      '0 0 1.597-1.16z',
]);

final _drawHouse = _paths(const [
  'M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8',
  'M3 10a2 2 0 0 1 .709-1.528l7-6a2 2 0 0 1 2.582 0l7 6A2 2 0 0 1 21 10v9'
      'a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z',
]);

final _drawClock = _paths(const [
  'M2 12a10 10 0 1 0 20 0a10 10 0 1 0-20 0',
  'M12 6v6l4 2',
]);

final _drawCopy = _paths(const [
  'M10 8h10a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H10a2 2 0 0 1-2-2V10a2 2 0 0 1 2-2z',
  'M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2',
]);

final _drawLayoutGrid = _paths(const [
  'M4 3h5a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z',
  'M15 3h5a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1h-5a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z',
  'M15 14h5a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1h-5a1 1 0 0 1-1-1v-5a1 1 0 0 1 1-1z',
  'M4 14h5a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1v-5a1 1 0 0 1 1-1z',
]);

final _drawPin = _paths(const [
  'M12 17v5',
  'M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1'
      'h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76'
      'V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z',
]);

final _drawPinOff = _paths(const [
  'M12 17v5',
  'M15 9.34V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H7.89',
  'm2 2 20 20',
  'M9 9v1.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h11',
]);

final _drawMaximize2 = _paths(const [
  'M15 3h6v6',
  'm21 3-7 7',
  'm3 21 7-7',
  'M9 21H3v-6',
]);

final _drawMinimize2 = _paths(const [
  'm14 10 7-7',
  'M20 10h-6V4',
  'm3 21 7-7',
  'M4 14h6v6',
]);

final _drawShieldAlert = _paths(const [
  'M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 '
      '0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 '
      '5 19 5a1 1 0 0 1 1 1z',
  'M12 8v4',
  'M12 16h.01',
]);

final _drawEllipsis = _paths(const [
  'M11 12a1 1 0 1 0 2 0a1 1 0 1 0-2 0',
  'M18 12a1 1 0 1 0 2 0a1 1 0 1 0-2 0',
  'M4 12a1 1 0 1 0 2 0a1 1 0 1 0-2 0',
]);

final _drawChevronLeft = _paths(const ['m15 18-6-6 6-6']);
final _drawChevronRight = _paths(const ['m9 18 6-6-6-6']);
final _drawChevronUp = _paths(const ['m18 15-6-6-6 6']);
final _drawChevronDown = _paths(const ['m6 9 6 6 6-6']);

final _drawMessageCircleQuestion = _paths(const [
  'M7.9 20A9 9 0 1 0 4 16.1L2 22Z',
  'M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3',
  'M12 17h.01',
]);

final _drawInbox = _paths(const [
  'M22 12h-6l-2 3h-4l-2-3H2',
  'M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89'
      'A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z',
]);

final _drawBell = _paths(const [
  'M10.268 21a2 2 0 0 0 3.464 0',
  'M3.262 15.326A1 1 0 0 0 4 17h16a1 1 0 0 0 .74-1.673'
      'C19.41 13.956 18 12.499 18 8A6 6 0 0 0 6 8'
      'c0 4.499-1.411 5.956-2.738 7.326',
]);

final _drawMenu = _paths(const ['M4 12h16', 'M4 18h16', 'M4 6h16']);

final _drawSearch = _paths(const [
  'M3 11a8 8 0 1 0 16 0a8 8 0 1 0-16 0',
  'm21 21-4.34-4.34',
]);

final _drawListFilter = _paths(const ['M2 5h20', 'M6 12h12', 'M9 19h6']);

final _drawCheck = _paths(const ['M20 6 9 17l-5-5']);

final _drawMic = _paths(const [
  'M12 19v3',
  'M19 10v2a7 7 0 0 1-14 0v-2',
  'M12 2a3 3 0 0 1 3 3v7a3 3 0 0 1-6 0V5a3 3 0 0 1 3-3Z',
]);

final _drawImage = _paths(const [
  'M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z',
  'M7 9a2 2 0 1 0 4 0a2 2 0 1 0-4 0',
  'm21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21',
]);

final _drawMinus = _paths(const ['M5 12h14']);

final _drawRotateCw = _paths(const [
  'M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8',
  'M21 3v5h-5',
]);

final _drawSend = _paths(const [
  'M14.536 21.686a.5.5 0 0 0 .937-.024l6.5-19a.496.496 0 0 0-.635-.635'
      'l-19 6.5a.5.5 0 0 0-.024.937l7.93 3.18a2 2 0 0 1 1.112 1.11z',
  'm21.854 2.147-10.94 10.939',
]);

final _drawMonitor = _paths(const [
  'M4 3h16a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z',
  'M8 21h8',
  'M12 17v4',
]);

final _drawMessageSquare = _paths(const [
  'M22 17a2 2 0 0 1-2 2H6.828a2 2 0 0 0-1.414.586l-2.202 2.202'
      'A.71.71 0 0 1 2 21.286V5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2z',
]);

final _drawMessageSquareText = _paths(const [
  'M22 17a2 2 0 0 1-2 2H6.828a2 2 0 0 0-1.414.586l-2.202 2.202'
      'A.71.71 0 0 1 2 21.286V5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2z',
  'M7 11h10',
  'M7 15h6',
  'M7 7h8',
]);

final _drawChartColumn = _paths(const [
  'M3 3v16a2 2 0 0 0 2 2h16',
  'M18 17V9',
  'M13 17V5',
  'M8 17v-3',
]);

final _drawPlay = _paths(const [
  'M5 5a2 2 0 0 1 3.008-1.728l11.997 6.998a2 2 0 0 1 .003 3.458l-12 7'
      'A2 2 0 0 1 5 19z',
]);

final _drawPause = _paths(const [
  'M15 3h3a1 1 0 0 1 1 1v16a1 1 0 0 1-1 1h-3a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z',
  'M6 3h3a1 1 0 0 1 1 1v16a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z',
]);

final _drawFilm = _paths(const [
  'M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z',
  'M7 3v18',
  'M3 7.5h4',
  'M3 12h18',
  'M3 16.5h4',
  'M17 3v18',
  'M17 7.5h4',
  'M17 16.5h4',
]);

final _drawPaperclip = _paths(const [
  'm16 6-8.414 8.586a2 2 0 0 0 2.829 2.829l8.414-8.586a4 4 0 1 0-5.657-5.657'
      'l-8.379 8.551a6 6 0 1 0 8.485 8.485l8.379-8.551',
]);

final _drawNetwork = _paths(const [
  'M17 16h4a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1h-4a1 1 0 0 1-1-1v-4a1 1 0 0 1 1-1z',
  'M3 16h4a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1v-4a1 1 0 0 1 1-1z',
  'M10 2h4a1 1 0 0 1 1 1v4a1 1 0 0 1-1 1h-4a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1z',
  'M5 16v-3a1 1 0 0 1 1-1h12a1 1 0 0 1 1 1v3',
  'M12 12V8',
]);

final _drawMonitorSmartphone = _paths(const [
  'M18 8V6a2 2 0 0 0-2-2H4a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h8',
  'M10 15v4',
  'M7 19h5',
  'M18 12h2a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2h-2a2 2 0 0 1-2-2v-6a2 2 0 0 1 2-2z',
]);

final _drawLaptopMinimal = _paths(const [
  'M6 4h12a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z',
  'M2 20h20',
]);

final _drawSmartphone = _paths(const [
  'M7 2h10a2 2 0 0 1 2 2v16a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2z',
  'M12 18h.01',
]);

final _drawTablet = _paths(const [
  'M6 2h12a2 2 0 0 1 2 2v16a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2z',
  'M12 18h.01',
]);

final _drawKeyRound = _paths(const [
  'M2.586 17.414A2 2 0 0 0 2 18.828V21a1 1 0 0 0 1 1h3a1 1 0 0 0 1-1v-1'
      'a1 1 0 0 1 1-1h1a1 1 0 0 0 1-1v-1a1 1 0 0 1 1-1h.172'
      'a2 2 0 0 0 1.414-.586l.814-.814a6.5 6.5 0 1 0-4-4z',
  'M16 7.5a.5.5 0 1 0 1 0a.5.5 0 1 0-1 0',
]);

final _drawPanelsTopLeft = _paths(const [
  'M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z',
  'M3 9h18',
  'M9 21V9',
]);

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

final _drawHistory = _paths(const [
  'M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8',
  'M3 3v5h5',
  'M12 7v5l4 2',
]);

final _drawUpload = _paths(const [
  'M12 3v12',
  'm17 8-5-5-5 5',
  'M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4',
]);

final _drawShare = _paths(const [
  'M12 2v13',
  'm16 6-4-4-4 4',
  'M4 12v8a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-8',
]);

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
        builder: (context, hovered) => _phoneTarget(
          context,
          AnimatedContainer(
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
      ),
    );
  }
}

/// A switch's whole row, its label and note included, as one click
/// target for it: a click anywhere on [child] turns it over, as on the
/// switch itself (whose own click goes to it alone). Its words stay
/// selectable by dragging, as settings' descriptions are.
class ToggleTap extends StatelessWidget {
  const ToggleTap({
    super.key,
    required this.value,
    required this.onChanged,
    required this.child,
  });

  final bool value;

  /// Null when it can't be changed now.
  final ValueChanged<bool>? onChanged;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final onChanged = this.onChanged;
    if (onChanged == null) return child;
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: () => onChanged(!value),
      // The pointer over the words too, which would otherwise show the
      // text cursor: a region laid over them that lets the pointer through.
      child: Stack(
        children: [
          child,
          const Positioned.fill(
            child: MouseRegion(opaque: false, cursor: SystemMouseCursors.click),
          ),
        ],
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

/// An edge the person drags to resize what's beside it, laid over its
/// hairline: a thin hit area with the resize cursor that shows a subtle
/// line while hovered or dragged, a finger's width on touch
/// ([widthOf]). [onDrag] gets how far the pointer has moved since the drag
/// started; a double click (a double tap) resets it ([onReset]).
class ResizeHandle extends StatefulWidget {
  const ResizeHandle({
    super.key,
    required this.onDragStart,
    required this.onDrag,
    required this.onDragEnd,
    required this.onReset,
    this.axis = Axis.horizontal,
  });

  /// How wide the area that takes the drag is, centered on the edge, with
  /// a pointer.
  static const width = 8.0;

  /// How wide it is here: [width], or a finger's on touch. Callers place
  /// it centred on the edge with this.
  static double widthOf(BuildContext context) =>
      touchFirst(context) ? Metrics.of(context).target : width;

  final VoidCallback onDragStart;
  final ValueChanged<double> onDrag;
  final VoidCallback onDragEnd;
  final VoidCallback onReset;

  /// Which way it drags: sideways for an edge between columns, up and down
  /// for one between rows.
  final Axis axis;

  @override
  State<ResizeHandle> createState() => _ResizeHandleState();
}

class _ResizeHandleState extends State<ResizeHandle> {
  var _hovered = false;
  var _dragging = false;
  var _moved = 0.0;

  void _start() {
    setState(() {
      _dragging = true;
      _moved = 0;
    });
    widget.onDragStart();
  }

  void _update(double delta) {
    _moved += delta;
    widget.onDrag(_moved);
  }

  void _end() {
    if (!_dragging) return;
    setState(() => _dragging = false);
    widget.onDragEnd();
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final shown = _hovered || _dragging;
    final across = widget.axis == Axis.horizontal;
    final line = AnimatedContainer(
      duration: Motion.fast,
      curve: Curves.easeOut,
      width: across ? 2 : null,
      height: across ? null : 2,
      color: shown
          ? palette.textFaint.withValues(alpha: 0.5)
          : const Color(0x00000000),
    );
    return MouseRegion(
      cursor: across
          ? SystemMouseCursors.resizeColumn
          : SystemMouseCursors.resizeRow,
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        // Counted from where the pointer went down, so the edge stays under
        // it once the drag starts.
        dragStartBehavior: DragStartBehavior.down,
        onHorizontalDragStart: across ? (_) => _start() : null,
        onHorizontalDragUpdate: across
            ? (details) => _update(details.delta.dx)
            : null,
        onHorizontalDragEnd: across ? (_) => _end() : null,
        onHorizontalDragCancel: across ? _end : null,
        onVerticalDragStart: across ? null : (_) => _start(),
        onVerticalDragUpdate: across
            ? null
            : (details) => _update(details.delta.dy),
        onVerticalDragEnd: across ? null : (_) => _end(),
        onVerticalDragCancel: across ? null : _end,
        onDoubleTap: widget.onReset,
        child: SizedBox(
          width: across ? ResizeHandle.widthOf(context) : null,
          height: across ? null : ResizeHandle.widthOf(context),
          child: Center(child: line),
        ),
      ),
    );
  }
}
