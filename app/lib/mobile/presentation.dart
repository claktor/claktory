import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart' show ValueListenable;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../controls.dart';
import '../identity.dart';
import '../selection.dart' show SelectableArea;
import 'back.dart';
import 'haptics.dart';
import 'header.dart';
import 'metrics.dart';
import 'navigation.dart' show PhoneRoutesScope;
import 'popover.dart';

/// How the app presents something over what shows, the one place that
/// decides it (docs/IDENTITY.md, "Phone"): on a phone a sheet that rises
/// from the bottom ([PhoneSheet]); on a Mac a dialog, as [showModal] shows.
///
/// - [showActions]: a few things to do with something (a menu).
/// - [showChoice]: one of a list (a dropdown).
/// - [showEditor]: a form to fill in, nearly the screen's height.
/// - [showPanel]: something to read or work through, with Close.
/// - [showModal] itself shows any dialog as a sheet on a phone.

/// One of [showActions]' choices.
class SheetAction<T> {
  const SheetAction(
    this.label,
    this.value, {
    this.glyph,
    this.destructive = false,
  });

  final String label;
  final T value;

  /// A 16pt glyph before the label, in the color given.
  final Widget Function(Color color)? glyph;

  /// It deletes or removes something: in `danger`.
  final bool destructive;
}

/// Asks what to do: on a phone an action sheet ([title] and [message] in
/// small print over a 44pt row for each action, Cancel apart under them);
/// on an iPad's wide window a popover pointing at [context]'s widget (the
/// row or button it's about), without Cancel: a tap outside it cancels;
/// elsewhere a dialog listing them. Completes with the action's value, or
/// null when cancelled. [beside]: the popover to the right of the widget
/// rather than under it, where there's room (a sidebar row's menu).
Future<T?> showActions<T>(
  BuildContext context, {
  String? title,
  String? message,
  required List<SheetAction<T>> actions,
  String cancelLabel = 'Cancel',
  bool beside = false,
}) {
  if (popoverHere(context)) {
    return presentPopover<T>(
      context,
      beside: beside,
      wrap: (child) => _CountedSheet(child: child),
      builder: (context, close) => _ActionList<T>(
        title: title,
        message: message,
        actions: actions,
        cancelLabel: null,
        close: close,
      ),
    );
  }
  final phone = isPhoneLayout(context);
  if (phone) unawaited(Haptics.selection());
  return _present<T>(
    context,
    phone: phone,
    builder: (context, close) => _ActionList<T>(
      title: title,
      message: message,
      actions: actions,
      cancelLabel: cancelLabel,
      close: close,
    ),
  );
}

/// Asks for one of [options]: on a phone a sheet of rows with a check on
/// [selected], under [title] (what's chosen) when given; on an iPad's wide
/// window a popover pointing at [context]'s widget; elsewhere a dialog.
/// Completes with the value chosen, or null when closed without one.
Future<T?> showChoice<T>(
  BuildContext context, {
  String? title,
  required List<SelectOption<T>> options,
  T? selected,
}) => popoverHere(context)
    ? presentPopover<T>(
        context,
        wrap: (child) => _CountedSheet(child: child),
        builder: (context, close) =>
            _ChoiceList<T>(options: options, selected: selected, choose: close),
      )
    : _present<T>(
        context,
        phone: isPhoneLayout(context),
        title: title,
        builder: (context, close) =>
            _ChoiceList<T>(options: options, selected: selected, choose: close),
      );

/// A form: on a phone a sheet nearly the screen's height, under [title]
/// and Close, sitting above the keyboard; elsewhere a dialog. [builder]
/// owns its scrolling: it gets a bounded height, so it can keep a part in
/// place (a quoted passage, its buttons) while the rest scrolls.
Future<void> showEditor(
  BuildContext context, {
  required String title,
  required Widget Function(BuildContext context, VoidCallback close) builder,
  Future<bool> Function()? shouldClose,
}) => _present<void>(
  context,
  phone: isPhoneLayout(context),
  title: title,
  tall: true,
  shouldClose: shouldClose,
  builder: (context, close) => builder(context, close),
);

/// Something to read or work through: on a phone a sheet with Close (as
/// tall as its content, or nearly the screen with [tall]); elsewhere a
/// dialog.
Future<void> showPanel(
  BuildContext context, {
  String? title,
  bool tall = false,
  required Widget Function(BuildContext context, VoidCallback close) builder,
}) => _present<void>(
  context,
  phone: isPhoneLayout(context),
  title: title,
  closeButton: true,
  tall: tall,
  builder: (context, close) => builder(context, close),
);

/// Shows [builder] as a [PhoneSheet] on a phone, else in [showModal]'s
/// dialog.
Future<T?> _present<T>(
  BuildContext context, {
  required bool phone,
  required Widget Function(BuildContext context, void Function([T?]) close)
  builder,
  String? title,
  bool tall = false,
  bool closeButton = false,
  Future<bool> Function()? shouldClose,
}) async {
  if (phone) {
    return presentSheet<T>(
      context,
      title: title,
      tall: tall,
      shouldClose: shouldClose,
      closeButton: closeButton || title != null,
      builder: builder,
    );
  }
  T? result;
  await showModal(context, (context, close) {
    void done([T? value]) {
      result = value;
      close();
    }

    final body = builder(context, done);
    if (title == null) return body;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          title,
          style: TextStyles.emphasis.copyWith(color: Palette.of(context).text),
        ),
        const SizedBox(height: Space.l),
        Flexible(child: body),
      ],
    );
  });
  return result;
}

/// Counts the sheets presented, so what floats over the screen (a card
/// held open, [HoverCard]) steps aside for one: an [OverlayPortal]'s card
/// would stay painted above it.
final sheetsPresented = ValueNotifier(0);

/// Changes [sheetsPresented] after the frame: what listens to it may
/// rebuild, which it can't while a sheet builds or goes.
void _countSheets(int by) {
  WidgetsBinding.instance
    ..addPostFrameCallback((_) => sheetsPresented.value += by)
    ..ensureVisualUpdate();
}

/// Counts [child] in [sheetsPresented] for as long as it's in the tree:
/// however it goes, closed or torn down with its overlay, it's counted out.
class _CountedSheet extends StatefulWidget {
  const _CountedSheet({required this.child, this.gone});

  final Widget child;

  /// Called as it leaves the tree, however it goes: closed, or torn down
  /// with the overlay it's in (a window frozen for a move).
  final VoidCallback? gone;

  @override
  State<_CountedSheet> createState() => _CountedSheetState();
}

class _CountedSheetState extends State<_CountedSheet> {
  @override
  void initState() {
    super.initState();
    _countSheets(1);
  }

  @override
  void dispose() {
    _countSheets(-1);
    widget.gone?.call();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}

/// Where sheets go: the overlay over a phone's pages, rather than the one
/// inside their navigator, which keeps only its pages in order (a sheet
/// there could end up under the next page pushed). Elsewhere, the nearest
/// [Overlay].
class SheetOverlay extends InheritedWidget {
  const SheetOverlay({super.key, required this.overlay, required super.child});

  final OverlayState overlay;

  static OverlayState of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<SheetOverlay>()?.overlay ??
      Overlay.of(context);

  @override
  bool updateShouldNotify(SheetOverlay old) => old.overlay != overlay;
}

/// Presents [builder] in a [PhoneSheet] over everything, and completes with
/// what it's closed with: by `close`, Close, a tap on the dimmed screen
/// around it, dragging it down, or Esc on a keyboard (null). Those four,
/// the person's, ask [shouldClose] first when it's given (or what's in it
/// set with [guardSheet]), and the sheet stays up unless it answers true;
/// `close` from inside never asks.
Future<T?> presentSheet<T>(
  BuildContext context, {
  required Widget Function(BuildContext context, void Function([T?]) close)
  builder,
  String? title,
  bool tall = false,
  bool closeButton = true,
  bool padded = true,
  Future<bool> Function()? shouldClose,
  ValueListenable<bool>? held,
}) {
  final overlay = SheetOverlay.of(context);
  // The overlay may be above the phone's Back: it's found from here.
  final back = PhoneBack.maybeOf(context);
  // And above its routes: what's in the sheet opens pages as anything
  // under it would (an app chosen from All apps, a file).
  final routes = context.getInheritedWidgetOfExactType<PhoneRoutesScope>();
  final done = Completer<T?>();
  late final OverlayEntry entry;
  final host = GlobalKey<_SheetHostState>();
  var removed = false;
  void remove() {
    if (removed) return;
    removed = true;
    entry.remove();
    entry.dispose();
  }

  void close([T? value]) {
    if (done.isCompleted) return;
    done.complete(value);
    final state = host.currentState;
    if (state == null) {
      remove();
    } else {
      state.dismiss().whenComplete(remove);
    }
  }

  // Torn down without closing, with the overlay it was in (a window frozen
  // for a move takes everything in it): it ends as if dismissed, its Back
  // token released as its host goes, and the entry let go once the frame
  // that took it is over.
  void gone() {
    if (!done.isCompleted) done.complete(null);
    if (!removed) scheduleMicrotask(remove);
  }

  final guard = _SheetGuardSlot()
    ..mayClose = shouldClose
    ..changed = () => host.currentState?.backChanged();
  // Dragged down, the dimmed screen tapped or Esc: what's in it may ask
  // first (a form with changes), and stays up if told to.
  Future<void> dismiss() async {
    final mayClose = guard.mayClose;
    if (mayClose != null && !await mayClose()) {
      host.currentState?.springBack();
      return;
    }
    close();
  }

  entry = OverlayEntry(
    builder: (context) => _CountedSheet(
      gone: gone,
      child: _SheetHost(
        key: host,
        onDismiss: dismiss,
        back: back,
        held: held,
        canClose: () => guard.mayClose == null,
        child: PhoneSheet(
          title: title,
          tall: tall,
          padded: padded,
          onClose: closeButton ? dismiss : null,
          child: _SheetGuard(
            slot: guard,
            child: Builder(
              builder: (context) => routes == null
                  ? builder(context, close)
                  : PhoneRoutesScope(
                      routes: routes.routes,
                      version: routes.version,
                      child: Builder(
                        builder: (context) => builder(context, close),
                      ),
                    ),
            ),
          ),
        ),
      ),
    ),
  );
  overlay.insert(entry);
  return done.future;
}

/// A sheet driven by state rather than presented: [child] shows in a tall
/// [PhoneSheet] (no title or Close: it draws its own) rising over what's
/// under this widget, for as long as it isn't null, and goes down when it
/// turns null. Dragging it down, tapping the dimmed screen or Esc calls
/// [onDismiss], which should make [child] null (an extension's panel,
/// closed on the backend). Place it in a [Stack] over the screen.
class PhoneSheetHost extends StatefulWidget {
  const PhoneSheetHost({super.key, required this.onDismiss, this.child});

  final VoidCallback onDismiss;
  final Widget? child;

  @override
  State<PhoneSheetHost> createState() => _PhoneSheetHostState();
}

class _PhoneSheetHostState extends State<PhoneSheetHost> {
  var _host = GlobalKey<_SheetHostState>();

  /// What shows: [PhoneSheetHost.child], kept while it goes down.
  Widget? _shown;

  /// Going down: one that rises meanwhile starts afresh.
  var _closing = false;

  @override
  void initState() {
    super.initState();
    _show(widget.child);
  }

  @override
  void didUpdateWidget(PhoneSheetHost old) {
    super.didUpdateWidget(old);
    final child = widget.child;
    if (child != null) {
      _show(child);
    } else if (_shown != null && !_closing) {
      _closing = true;
      final host = _host;
      unawaited(
        (host.currentState?.dismiss() ?? Future<void>.value()).whenComplete(() {
          // Unless another rose meanwhile.
          if (mounted && _host == host && widget.child == null) {
            setState(_hide);
          }
        }),
      );
    }
  }

  void _show(Widget? child) {
    if (child == null) return;
    if (_shown == null) _countSheets(1);
    if (_closing) {
      _closing = false;
      _host = GlobalKey<_SheetHostState>();
    }
    _shown = child;
  }

  void _hide() {
    if (_shown == null) return;
    _shown = null;
    _closing = false;
    _countSheets(-1);
  }

  @override
  void dispose() {
    _hide();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final shown = _shown;
    if (shown == null) return const SizedBox.shrink();
    return _SheetHost(
      key: _host,
      onDismiss: widget.onDismiss,
      back: PhoneBack.maybeOf(context),
      child: PhoneSheet(tall: true, padded: false, child: shown),
    );
  }
}

/// What a sheet's content says about being dismissed (dragged down, the
/// dimmed screen tapped, Esc): null lets it go; else it's asked, and the
/// sheet stays up unless it answers true.
class _SheetGuardSlot {
  Future<bool> Function()? mayClose;

  /// [mayClose] was set: whether Back closes the sheet as it is changed.
  VoidCallback? changed;
}

/// Lets what's in a sheet hold off its dismissal: a form with changes asks
/// before they're dropped ([guardSheet]).
class _SheetGuard extends InheritedWidget {
  const _SheetGuard({required this.slot, required super.child});

  final _SheetGuardSlot slot;

  @override
  bool updateShouldNotify(_SheetGuard old) => old.slot != slot;
}

/// While [mayClose] is set, dismissing the sheet [context] is in asks it
/// first; null lets it go again. Does nothing outside a sheet.
void guardSheet(BuildContext context, Future<bool> Function()? mayClose) {
  final slot = context.getInheritedWidgetOfExactType<_SheetGuard>()?.slot;
  if (slot == null || slot.mayClose == mayClose) return;
  slot.mayClose = mayClose;
  slot.changed?.call();
}

/// Asks whether to drop a form's changes: an action sheet with Discard
/// changes (destructive) and Keep editing. True to drop them.
Future<bool> confirmDiscard(BuildContext context) async =>
    await showActions<bool>(
      context,
      actions: const [SheetAction('Discard changes', true, destructive: true)],
      cancelLabel: 'Keep editing',
    ) ??
    false;

/// What a sheet tells the [_SheetHost] above it: how far it's dragged down.
class _SheetDrag extends InheritedWidget {
  const _SheetDrag({required this.host, required super.child});

  final _SheetHostState host;

  static _SheetHostState? of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<_SheetDrag>()?.host;

  @override
  bool updateShouldNotify(_SheetDrag old) => old.host != host;
}

/// Animates a sheet in and out over the dimmed screen, follows it as it's
/// dragged down, and closes it when it's let go far or fast enough.
class _SheetHost extends StatefulWidget {
  const _SheetHost({
    super.key,
    required this.onDismiss,
    required this.child,
    this.back,
    this.canClose,
    this.held,
  });

  final VoidCallback onDismiss;
  final Widget child;

  /// While true, it can't be set aside, only answered (a screen before a
  /// system prompt): no grabber, no drag, and the dimmed screen isn't read
  /// out as Close.
  final ValueListenable<bool>? held;

  /// The phone's Back, which closes it ([onDismiss]) before what's under
  /// it while it's up.
  final PhoneBack? back;

  /// Whether it closes as it is ([BackToken.canClose]); always without.
  final bool Function()? canClose;

  @override
  State<_SheetHost> createState() => _SheetHostState();
}

class _SheetHostState extends State<_SheetHost> with TickerProviderStateMixin {
  late final _shown = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 280),
    reverseDuration: Motion.base,
    value: 0,
  )..forward();

  /// How far it's dragged down, in points.
  final _dragged = ValueNotifier(0.0);
  final _sheet = GlobalKey();

  double get _height =>
      (_sheet.currentContext?.findRenderObject() as RenderBox?)?.size.height ??
      1;

  /// Its place in the phone's Back while it's up.
  BackHandle? _back;

  /// The sheet's focus, taken as it rises (autofocus wouldn't: a field
  /// behind it, the message box's say, already has focus): keys, and
  /// what's read out, are the sheet's then. The keyboard goes down unless a
  /// field in the sheet takes focus, as iOS's sheets do; once the sheet is
  /// gone, focus goes back where it was.
  final _focus = FocusScopeNode(debugLabel: 'Sheet');

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _focus.requestFocus();
    });
    _back = widget.back?.register(
      CallbackBackToken(
        onBack: () => widget.onDismiss(),
        canClose: () => widget.canClose?.call() ?? true,
      ),
    );
  }

  /// What [_SheetHost.canClose] says changed.
  void backChanged() => _back?.changed();

  /// Plays it out; completes when it's gone.
  Future<void> dismiss() async {
    // Going: Back reaches what's under it from now.
    _back?.release();
    _back = null;
    if (!mounted) return;
    await _shown.reverse().orCancel.catchError((_) {});
  }

  /// It can't be set aside now ([_SheetHost.held]).
  bool get held => widget.held?.value ?? false;

  /// What [held] listens to, if anything.
  ValueListenable<bool>? get heldListenable => widget.held;

  void drag(double delta) {
    if (held) return;
    _dragged.value = math.max(0, _dragged.value + delta);
  }

  /// The finger lifted: closes it past a third of its height or flung down,
  /// else it springs back.
  void release(double velocity) {
    if (held) return springBack();
    if (_dragged.value > _height / 3 || velocity > 700) {
      unawaited(Haptics.light());
      widget.onDismiss();
      return;
    }
    springBack();
  }

  /// Back to where it rests, after a drag that didn't close it.
  void springBack() {
    if (!mounted) return;
    final from = _dragged.value;
    if (from == 0) return;
    final back = AnimationController(vsync: this, duration: Motion.base);
    back.addListener(() {
      _dragged.value = from * (1 - Curves.easeOut.transform(back.value));
    });
    back.forward().whenComplete(back.dispose);
  }

  @override
  void dispose() {
    _back?.release();
    _focus.dispose();
    _shown.dispose();
    _dragged.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final reduced = MediaQuery.disableAnimationsOf(context);
    final keyboard = MediaQuery.viewInsetsOf(context).bottom;
    // A sheet over the phone's overlay, above its Back: what it presents
    // (a question, another sheet) goes over it in Back's order too.
    final back = widget.back;
    final content = back == null
        ? widget.child
        : PhoneBackScope(back: back, child: widget.child);
    final curve = CurvedAnimation(
      parent: _shown,
      curve: Curves.easeOutCubic,
      reverseCurve: Curves.easeInCubic,
    );
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): widget.onDismiss,
      },
      // Nothing behind it is read out or reached while it's up.
      child: BlockSemantics(
        child: Stack(
          fit: StackFit.expand,
          children: [
            ValueListenableBuilder<bool>(
              valueListenable: widget.held ?? const _NeverHeld(),
              // Held, it isn't read out as Close: it doesn't close.
              builder: (context, held, child) => Semantics(
                container: true,
                button: !held,
                label: held ? null : 'Close',
                onTap: held ? null : widget.onDismiss,
                // Its own tap, labelled, in place of the detector's.
                excludeSemantics: true,
                child: child,
              ),
              child: GestureDetector(
                onTap: widget.onDismiss,
                child: AnimatedBuilder(
                  animation: Listenable.merge([curve, _dragged]),
                  builder: (context, _) => ColoredBox(
                    color: const Color(0xFF000000).withValues(
                      alpha:
                          0.3 *
                          curve.value *
                          (1 - (_dragged.value / _height).clamp(0, 1)),
                    ),
                  ),
                ),
              ),
            ),
            Padding(
              // It sits on the keyboard.
              padding: EdgeInsets.only(bottom: keyboard),
              child: Align(
                alignment: Alignment.bottomCenter,
                child: AnimatedBuilder(
                  animation: Listenable.merge([curve, _dragged]),
                  builder: (context, child) {
                    // Taps land where it comes to rest, even while it's
                    // still sliding in.
                    final dragged = Transform.translate(
                      offset: Offset(0, _dragged.value),
                      transformHitTests: false,
                      child: child,
                    );
                    if (reduced) {
                      return Opacity(opacity: curve.value, child: dragged);
                    }
                    return FractionalTranslation(
                      translation: Offset(0, 1 - curve.value),
                      transformHitTests: false,
                      child: dragged,
                    );
                  },
                  child: FocusScope(
                    node: _focus,
                    child: _SheetDrag(
                      host: this,
                      child: KeyedSubtree(key: _sheet, child: content),
                    ),
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// A sheet on a phone (docs/IDENTITY.md, "Phone"): a floating layer in the
/// sidebar's color with 14pt top corners and a grabber, [title] and Close
/// under it when given, then [child]. As tall as its content, at most up
/// to the status bar's edge ([tall]: that tall always); above the keyboard
/// and clear of the home indicator. Dragged down by its grabber or title,
/// or by pulling its content down from the top, it closes.
///
/// It takes the safe areas and the keyboard out of the [MediaQuery] it
/// passes down: [child] adds neither.
class PhoneSheet extends StatelessWidget {
  const PhoneSheet({
    super.key,
    required this.child,
    this.title,
    this.onClose,
    this.tall = false,
    this.padded = true,
  });

  final Widget child;
  final String? title;

  /// Close, at the end of the title's row; none without one.
  final VoidCallback? onClose;
  final bool tall;

  /// [child] 16pt in from the sides and the bottom.
  final bool padded;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final media = MediaQuery.of(context);
    final keyboard = media.viewInsets.bottom;
    final room = media.size.height - media.padding.top - Space.s - keyboard;
    final host = _SheetDrag.of(context);
    // Dragging the grabber or the title moves the sheet.
    Widget draggable(Widget child) => host == null
        ? child
        : GestureDetector(
            behavior: HitTestBehavior.opaque,
            onVerticalDragUpdate: (details) => host.drag(details.delta.dy),
            onVerticalDragEnd: (details) =>
                host.release(details.velocity.pixelsPerSecond.dy),
            child: child,
          );
    final title = this.title;
    final onClose = this.onClose;
    final top = Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // The grabber, while it can be dragged away.
        SizedBox(
          height: Space.l + 4,
          child: ValueListenableBuilder<bool>(
            valueListenable: host?.heldListenable ?? const _NeverHeld(),
            builder: (context, held, _) => held
                ? const SizedBox.shrink()
                : Center(
                    child: Container(
                      width: 36,
                      height: 5,
                      decoration: BoxDecoration(
                        color: palette.textFaint.withValues(alpha: 0.5),
                        borderRadius: BorderRadius.circular(2.5),
                      ),
                    ),
                  ),
          ),
        ),
        if (title != null || onClose != null)
          Padding(
            padding: const EdgeInsetsDirectional.only(
              start: Touch.gutter,
              end: Space.xs,
            ),
            child: ConstrainedBox(
              constraints: const BoxConstraints(minHeight: Touch.target),
              child: Row(
                children: [
                  Expanded(
                    child: Semantics(
                      header: true,
                      child: Text(
                        title ?? '',
                        style: PhoneType.emphasis.copyWith(color: palette.text),
                      ),
                    ),
                  ),
                  if (onClose != null) HeaderButton.close(onTap: onClose),
                ],
              ),
            ),
          ),
      ],
    );
    Widget body = withoutInsets(
      context,
      child: Padding(
        padding: padded
            ? const EdgeInsets.fromLTRB(
                Touch.gutter,
                Space.s,
                Touch.gutter,
                Touch.gutter,
              )
            : EdgeInsets.zero,
        child: child,
      ),
    );
    if (host != null) {
      // Pulled down past its top, the content takes the sheet with it: so
      // it doesn't bounce there, and the pull reaches the sheet instead.
      body = NotificationListener<ScrollNotification>(
        onNotification: (notification) {
          if (notification.depth != 0 ||
              notification.metrics.axis != Axis.vertical) {
            return false;
          }
          if (notification is OverscrollNotification &&
              notification.dragDetails != null &&
              notification.overscroll < 0) {
            host.drag(-notification.overscroll);
          } else if (notification is ScrollUpdateNotification &&
              notification.dragDetails != null &&
              host._dragged.value > 0) {
            // Back up while dragged: the sheet rises before the content
            // scrolls.
            host.drag(-(notification.scrollDelta ?? 0));
          } else if (notification is ScrollEndNotification &&
              host._dragged.value > 0) {
            host.release(
              notification.dragDetails?.velocity.pixelsPerSecond.dy ?? 0,
            );
          }
          return false;
        },
        child: ScrollConfiguration(
          behavior: ScrollConfiguration.of(context)
              .copyWith(physics: const ClampingScrollPhysics()),
          child: body,
        ),
      );
    }
    return Semantics(
      scopesRoute: true,
      namesRoute: title != null,
      label: title,
      explicitChildNodes: true,
      child: Container(
        constraints: BoxConstraints(maxHeight: math.max(0, room)),
        height: tall ? math.max(0, room) : null,
        width: double.infinity,
        // Turned on its side, the screen's sides stay clear of the notch.
        margin: EdgeInsets.only(
          left: media.padding.left,
          right: media.padding.right,
        ),
        decoration: BoxDecoration(
          color: palette.sidebar,
          borderRadius: const BorderRadius.vertical(
            top: Radius.circular(Radii.floating),
          ),
          border: Border.all(color: palette.hairline),
          boxShadow: floatingShadow,
        ),
        clipBehavior: Clip.antiAlias,
        child: DefaultTextStyle(
          style: PhoneType.body.copyWith(color: palette.text),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              draggable(top),
              if (tall) Expanded(child: body) else Flexible(child: body),
              // Clear of the home indicator, unless the keyboard is up.
              SizedBox(height: keyboard > 0 ? 0 : media.padding.bottom),
            ],
          ),
        ),
      ),
    );
  }
}

/// [showModal]'s dialog as a sheet on a phone: the same [child], with its
/// words selectable as in the dialog.
Future<void> showModalSheet(
  BuildContext context,
  Widget Function(BuildContext context, VoidCallback close) builder,
) => presentSheet<void>(
  context,
  closeButton: false,
  builder: (context, close) => SelectableArea(child: builder(context, close)),
);

class _ActionList<T> extends StatelessWidget {
  const _ActionList({
    required this.title,
    required this.message,
    required this.actions,
    required this.cancelLabel,
    required this.close,
  });

  final String? title;
  final String? message;
  final List<SheetAction<T>> actions;

  /// Cancel's row, apart under the actions; none in a popover, which a tap
  /// outside cancels.
  final String? cancelLabel;
  final void Function([T?]) close;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final metrics = Metrics.of(context);
    Widget row({
      required String label,
      required VoidCallback onTap,
      Widget Function(Color color)? glyph,
      Color? color,
      bool bold = false,
    }) {
      final ink = color ?? palette.text;
      return Semantics(
        button: true,
        label: label,
        // Its own tap, labelled, in place of the detector's.
        onTap: onTap,
        excludeSemantics: true,
        child: Pressable(
          onTap: onTap,
          builder: (context, pressed) => Container(
            constraints: BoxConstraints(minHeight: metrics.target + 8),
            padding: const EdgeInsets.symmetric(
              horizontal: Space.m,
              vertical: Space.s,
            ),
            decoration: BoxDecoration(
              color: pressed ? palette.hover : null,
              borderRadius: BorderRadius.circular(Radii.panel),
            ),
            child: Row(
              children: [
                if (glyph != null) ...[
                  SizedBox(width: 20, child: Center(child: glyph(ink))),
                  const SizedBox(width: Space.m),
                ],
                Expanded(
                  child: Text(
                    label,
                    style: (bold ? type.emphasis : type.body).copyWith(
                      color: ink,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      );
    }

    final title = this.title;
    final message = this.message;
    return SingleChildScrollView(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (title != null || message != null)
            Padding(
              padding: const EdgeInsets.fromLTRB(Space.m, 0, Space.m, Space.s),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  if (title != null)
                    Text(
                      title,
                      style: type.secondary.copyWith(
                        color: palette.textMuted,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  if (message != null)
                    Text(
                      message,
                      style: type.secondary.copyWith(color: palette.textMuted),
                    ),
                ],
              ),
            ),
          for (final action in actions)
            row(
              label: action.label,
              glyph: action.glyph,
              color: action.destructive ? palette.danger : null,
              onTap: () => close(action.value),
            ),
          if (cancelLabel case final cancel?) ...[
            Padding(
              padding: const EdgeInsets.symmetric(vertical: Space.s),
              child: Container(height: 1, color: palette.hairline),
            ),
            row(label: cancel, bold: true, onTap: close),
          ],
        ],
      ),
    );
  }
}

class _ChoiceList<T> extends StatelessWidget {
  const _ChoiceList({
    required this.options,
    required this.selected,
    required this.choose,
  });

  final List<SelectOption<T>> options;
  final T? selected;
  final void Function([T?]) choose;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final metrics = Metrics.of(context);
    return ListView(
      shrinkWrap: true,
      padding: EdgeInsets.zero,
      children: [
        for (final option in options)
          Semantics(
            button: true,
            selected: option.value == selected,
            label: option.label,
            onTap: () {
              unawaited(Haptics.selection());
              choose(option.value);
            },
            excludeSemantics: true,
            child: Pressable(
              onTap: () {
                unawaited(Haptics.selection());
                choose(option.value);
              },
              builder: (context, pressed) => Container(
                constraints: BoxConstraints(minHeight: metrics.row),
                padding: const EdgeInsets.symmetric(
                  horizontal: Space.m,
                  vertical: Space.s,
                ),
                decoration: BoxDecoration(
                  color: pressed
                      ? palette.hover
                      : option.value == selected
                      ? palette.selected
                      : null,
                  borderRadius: BorderRadius.circular(Radii.panel),
                ),
                child: Row(
                  children: [
                    Expanded(
                      child: Text(
                        option.label,
                        style: type.body.copyWith(color: palette.text),
                      ),
                    ),
                    if (option.value == selected)
                      ExtensionGlyph(
                        icon: 'check',
                        name: 'Chosen',
                        color: palette.signal,
                        size: 20,
                      ),
                  ],
                ),
              ),
            ),
          ),
      ],
    );
  }
}

/// Never held: a sheet that can always be set aside.
class _NeverHeld implements ValueListenable<bool> {
  const _NeverHeld();

  @override
  bool get value => false;

  @override
  void addListener(VoidCallback listener) {}

  @override
  void removeListener(VoidCallback listener) {}
}
