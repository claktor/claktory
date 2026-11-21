import 'package:flutter/widgets.dart';

/// How a phone moves between its screens (docs/IDENTITY.md, "Phone"):
/// the conversation fills the screen; a drawer from the leading edge holds
/// the Claktories and their projects and agents; everything else is a page
/// pushed over the conversation, with iOS's interactive Back (a swipe from
/// the leading edge, or the Back button), or a sheet (mobile/presentation.dart).
///
/// Which screen shows is this window's own, like [AppPage]: nothing of it
/// goes to the backend. Going Back pops a page on this phone only; it never
/// changes the agent's layout ([Backend.setLayout]), which other devices
/// share.
///
/// **Insets.** One thing owns the keyboard and the safe areas: the page
/// scaffold that shows a screen. It keeps the screen's content under the
/// status bar and above max(home indicator, keyboard), and removes the
/// padding and the view insets from the [MediaQuery] it passes down, so
/// nothing inside (the composer, a field) adds them a second time. A sheet
/// owns its own the same way. Widgets inside read neither
/// [MediaQueryData.viewInsets] nor [MediaQueryData.padding].
abstract class PhoneRoutes {
  /// The phone's navigation, or null where there's none (a Mac's window,
  /// a test of one view).
  static PhoneRoutes? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<PhoneRoutesScope>()?.routes;

  /// Whether the drawer is out.
  bool get drawerOpen;

  void openDrawer();
  void closeDrawer();

  /// Pushes [route] over what shows, sliding in from the trailing edge;
  /// completes with what it's popped with.
  Future<T?> push<T>(PhoneRoute<T> route);

  /// Pops the page on top, with [result].
  void pop<T>([T? result]);

  /// Whether there's a page to go back from (rather than the conversation
  /// itself, whose leading button opens the drawer).
  bool get canPop;

  /// Shows [agentId]'s conversation: the drawer closes and pushed pages
  /// pop. With [seq], it opens at that entry (a message linked to, the
  /// oldest card waiting for the person), else where it was.
  void openConversation(String agentId, {int? seq});

  /// Keeps the drawer's edge swipe from starting while something else owns
  /// the finger: a recording's hold and slide, a code block or a picture
  /// panned sideways, a selection's handles. Release the hold to give it
  /// back; holds stack. [NoDrawerGesture] does it for one pointer.
  DrawerHold holdGestures();
}

/// A hold on the drawer's edge swipe ([PhoneRoutes.holdGestures]).
abstract class DrawerHold {
  void release();
}

/// A page pushed on a phone: [id] tells it apart (pushing the same id
/// again brings that page back rather than stacking another).
class PhoneRoute<T> {
  const PhoneRoute({
    required this.id,
    required this.builder,
    this.fullscreenDialog = false,
  });

  final String id;
  final WidgetBuilder builder;

  /// Rises from the bottom and closes with Cancel or Done rather than Back
  /// (a long form), as iOS presents a task of its own.
  final bool fullscreenDialog;
}

/// Gives the widgets below a phone's [PhoneRoutes].
class PhoneRoutesScope extends InheritedWidget {
  const PhoneRoutesScope({
    super.key,
    required this.routes,
    required this.version,
    required super.child,
  });

  final PhoneRoutes routes;

  /// Changes whenever what [routes] reports does (the drawer opening, a
  /// page pushed), so readers rebuild.
  final int version;

  @override
  bool updateShouldNotify(PhoneRoutesScope old) =>
      old.routes != routes || old.version != version;
}

/// Keeps the drawer's edge swipe off while a finger is down on [child]:
/// for things with their own sideways drag (a code block's scroll, a
/// picture's pan, a recording's slide to cancel).
class NoDrawerGesture extends StatefulWidget {
  const NoDrawerGesture({super.key, required this.child});

  final Widget child;

  @override
  State<NoDrawerGesture> createState() => _NoDrawerGestureState();
}

class _NoDrawerGestureState extends State<NoDrawerGesture> {
  final _holds = <int, DrawerHold>{};

  void _release(int pointer) => _holds.remove(pointer)?.release();

  @override
  void dispose() {
    for (final hold in _holds.values) {
      hold.release();
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Listener(
    onPointerDown: (event) {
      final routes = PhoneRoutes.of(context);
      if (routes != null) _holds[event.pointer] = routes.holdGestures();
    },
    onPointerUp: (event) => _release(event.pointer),
    onPointerCancel: (event) => _release(event.pointer),
    child: widget.child,
  );
}
