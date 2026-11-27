// On a phone, a conversation's message box steps aside while a field in the
// conversation itself has the keyboard: a Needs you card's answer, say.
// Otherwise a second text field sits idle between the card's field and the
// keyboard, which costs most on a small iPhone at large text sizes, for no
// gain. The box comes back exactly as it was as soon as that field lets go,
// or the keyboard goes while it keeps the focus (an iPad's keyboard shrunk
// to its shortcut bar, a hardware keyboard: no keyboard, no reason to make
// room), with its draft, attachments and mic, since it's only hidden, never
// rebuilt.
import 'package:flutter/widgets.dart';

/// How tall, in points, the bottom inset must be to count as a keyboard.
const keyboardAtLeast = 100.0;

/// Around a conversation and its message box. [enabled] on a phone only.
/// The conversation goes in [TypingAboveArea], the box in [TypingAboveBox].
class TypingAbove extends StatefulWidget {
  const TypingAbove({super.key, required this.enabled, required this.child});

  final bool enabled;
  final Widget child;

  @override
  State<TypingAbove> createState() => _TypingAboveState();
}

class _TypingAboveState extends State<TypingAbove> with WidgetsBindingObserver {
  /// Holds the conversation: it has focus while anything in it does.
  final _area = FocusNode(
    debugLabel: 'Conversation',
    canRequestFocus: false,
    skipTraversal: true,
  );
  final _typing = ValueNotifier(false);

  @override
  void initState() {
    super.initState();
    // Any move of the focus, also from one field in it to another.
    FocusManager.instance.addListener(_check);
    // The keyboard coming and going.
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void didChangeMetrics() => _check();

  @override
  void didUpdateWidget(TypingAbove old) {
    super.didUpdateWidget(old);
    if (old.enabled != widget.enabled) _check();
  }

  /// Only a text field counts: selecting a message's words also takes the
  /// focus, and the box shouldn't jump away for that.
  void _check() {
    if (!mounted) return;
    final focus = FocusManager.instance.primaryFocus;
    _typing.value =
        widget.enabled &&
        _area.hasFocus &&
        focus != null &&
        focus != _area &&
        _inField(focus) &&
        _keyboardShown();
  }

  /// A keyboard on screen: taller than a hardware keyboard's suggestions
  /// or an iPad's shortcut bar (about 50 to 70 points), which leave the
  /// room as it was.
  bool _keyboardShown() {
    final view = View.maybeOf(context);
    if (view == null) return false;
    return view.viewInsets.bottom / view.devicePixelRatio > keyboardAtLeast;
  }

  static bool _inField(FocusNode focus) {
    final context = focus.context;
    if (context == null) return false;
    if (context.widget is EditableText) return true;
    return context.findAncestorStateOfType<EditableTextState>() != null;
  }

  @override
  void dispose() {
    FocusManager.instance.removeListener(_check);
    WidgetsBinding.instance.removeObserver(this);
    _area.dispose();
    _typing.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) =>
      _TypingAboveScope(state: this, child: widget.child);
}

class _TypingAboveScope extends InheritedWidget {
  const _TypingAboveScope({required this.state, required super.child});

  final _TypingAboveState state;

  @override
  bool updateShouldNotify(_TypingAboveScope old) => old.state != state;
}

/// The conversation, whose own fields send the box aside.
class TypingAboveArea extends StatelessWidget {
  const TypingAboveArea({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final scope = context
        .dependOnInheritedWidgetOfExactType<_TypingAboveScope>();
    if (scope == null) return child;
    return Focus(
      focusNode: scope.state._area,
      canRequestFocus: false,
      skipTraversal: true,
      includeSemantics: false,
      child: child,
    );
  }
}

/// The message box, hidden (but kept, state and all) while a field in the
/// conversation above has the keyboard.
class TypingAboveBox extends StatelessWidget {
  const TypingAboveBox({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final scope = context
        .dependOnInheritedWidgetOfExactType<_TypingAboveScope>();
    if (scope == null) return child;
    return ValueListenableBuilder<bool>(
      valueListenable: scope.state._typing,
      builder: (context, typing, child) => Visibility(
        visible: !typing,
        maintainState: true,
        maintainAnimation: true,
        child: child!,
      ),
      child: child,
    );
  }
}
