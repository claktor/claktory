import 'package:flutter/widgets.dart';

import '../protocol/factory/v1/factory.pb.dart' show ViewLeaf;

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

  /// Focus mode on a screen of its own, over what shows (an iPhone's);
  /// false where the window lays it out itself (an iPad's, a Mac's) or
  /// there's none.
  bool openFocus();

  /// Whether there's a page to go back from (rather than the conversation
  /// itself, whose leading button opens the drawer).
  bool get canPop;

  /// Shows [agentId]'s conversation: the drawer closes and pushed pages
  /// pop. With [seq], it opens at that entry (a message linked to, the
  /// oldest card waiting for the person), else where it was.
  void openConversation(String agentId, {int? seq});

  /// Shows [agentId]'s conversation as a page over what shows (Back returns
  /// to it), with [panel]'s sheet (an extension id: 'checklist', 'tasks')
  /// up over it when given. True when it's done here; false where these
  /// routes can't (no such agent), for the caller to open it its own way.
  bool pushConversation(String agentId, {String panel = ''});

  /// Shows [path] in a viewer over [agentId]'s conversation, as a page:
  /// a review the agent asked for, say. The agent is chosen and the
  /// file opened in its layout (its `show`), if it isn't already, then the
  /// viewer pushed once it's there. True when it's done here; false where
  /// these routes can't show it, for the caller to open it its own way.
  bool showFile(String agentId, String path);

  /// Shows [agentId]'s pane [paneId] (a file, an app's view) in a viewer
  /// over its conversation, as a page: the agent is chosen, and the viewer
  /// pushed once the pane is in its layout (the caller puts it there, as
  /// an app's dock does). True when it's done here; false where these
  /// routes can't show it, for the caller to show it its own way.
  bool showPane(String agentId, String paneId);

  /// Shows an app's view ([leaf], an app's leaf) as a page of its own
  /// over what shows: never in an agent's layout, which other devices
  /// share. Back pops it.
  void pushApp(ViewLeaf leaf);

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
    this.edgeToEdge = false,
  });

  final String id;
  final WidgetBuilder builder;

  /// A file's viewer: it has the whole screen, its safe areas its own to
  /// lay out (mobile/viewer_chrome.dart).
  final bool edgeToEdge;

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
