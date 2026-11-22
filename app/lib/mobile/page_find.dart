// Find on a phone (Matt's ask: find wherever you are): a page whose pane can
// be searched (a FindScope in it, find.dart) shows the phone's find bar at
// its foot while its find is open, above the keyboard, in place of what's
// usually there (a conversation's message box steps aside). Search, in the
// page's menu, or ⌘F from a keyboard opens it; Done closes it.

import 'package:flutter/widgets.dart';

import '../find.dart';
import 'find_bar.dart';

/// Around a phone page: the find of the pane in it, if it can be searched,
/// for its menu ([PhonePageFind]) and its foot (the find bar).
class PhoneFindHost extends StatefulWidget {
  const PhoneFindHost({super.key, required this.child});

  final Widget child;

  @override
  State<PhoneFindHost> createState() => PhoneFindHostState();
}

class PhoneFindHostState extends State<PhoneFindHost> {
  FindSession? _session;

  /// The page's find, when its pane can be searched.
  FindSession? get session => _session;

  @override
  void initState() {
    super.initState();
    FindSessions.instance.sessions.addListener(_later);
    _later();
  }

  @override
  void dispose() {
    FindSessions.instance.sessions.removeListener(_later);
    _session?.isOpen.removeListener(_changed);
    super.dispose();
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  /// Panes come and go while the tree builds: looked at once it's built.
  void _later() => WidgetsBinding.instance
    ..addPostFrameCallback((_) => _sessionsChanged())
    ..ensureVisualUpdate();

  /// The session whose pane is on this page, as panes come and go.
  void _sessionsChanged() {
    if (!mounted) return;
    final found = FindSessions.instance.forPane(context);
    if (found == _session) return;
    _session?.isOpen.removeListener(_changed);
    found?.isOpen.addListener(_changed);
    setState(() => _session = found);
  }

  @override
  Widget build(BuildContext context) {
    final session = _session;
    final open = session != null && session.isOpen.value;
    return PhonePageFind(
      session: session,
      open: open,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Expanded(child: widget.child),
          if (open) PhoneSessionFindBar(session: session),
        ],
      ),
    );
  }
}

/// The phone's find bar for [session]: its words, which match of how many,
/// the matches up and down the screen, and Done.
class PhoneSessionFindBar extends StatelessWidget {
  const PhoneSessionFindBar({super.key, required this.session});

  final FindSession session;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: session.target,
    builder: (context, _) {
      final target = session.target;
      return PhoneFindBar(
        controller: session.query,
        focusNode: session.field,
        placeholder: target.placeholder,
        count: target.countLabel,
        nextIsUp: target.nextIsUp,
        onStep: session.step,
        canStep: target.count > 0 && target.failed == null,
        failed: target.failed != null,
        onDone: () {
          session.close();
          // On a phone, Done puts the keyboard away: no field of the page
          // (a setting's, a card's comment) takes the keys instead.
          FocusManager.instance.primaryFocus?.unfocus();
        },
      );
    },
  );
}

/// What a phone page can search, for its menu's Search, and whether its find
/// is open (what usually sits at the page's foot then steps aside).
class PhonePageFind extends InheritedWidget {
  const PhonePageFind({
    super.key,
    required this.session,
    required this.open,
    required super.child,
  });

  final FindSession? session;
  final bool open;

  static PhonePageFind? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<PhonePageFind>();

  /// Whether the page's find bar is open: a message box hides meanwhile.
  static bool finding(BuildContext context) => of(context)?.open ?? false;

  @override
  bool updateShouldNotify(PhonePageFind old) =>
      old.session != session || old.open != open;
}

/// What Search on [context]'s page opens: the page's find, when it can be
/// searched with the phone's bar (not a pane with a search field of its
/// own) and it isn't open already; null otherwise. For a Search in a
/// page's ⋯ sheet, or the magnifier a header with no other action shows.
VoidCallback? phonePageSearch(BuildContext context) {
  final page = PhonePageFind.of(context);
  final session = page?.session;
  if (page == null || session == null || session.own || page.open) {
    return null;
  }
  return session.open;
}
