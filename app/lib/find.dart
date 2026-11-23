// Find, across the app (Matt's ask: ⌘F works wherever you are): a pane
// that can be searched puts a [FindScope] above its body with its
// [FindTarget]; ⌘F (Ctrl+F off Apple's) opens a find bar for the pane that
// has focus, or the one last clicked in, or the largest on show, never
// nothing. ⌘G and ⇧⌘G (Enter and ⇧Enter in the field) go to the next and
// previous match, Esc closes. On a Mac and iPad the bar floats at the
// pane's top right; on a phone the phone's own bar (mobile/find_bar.dart)
// shows the same session at the page's foot.
import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'controls.dart';
import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;
import 'selection.dart' show Unselectable;

/// What a pane offers find: it searches, says how many and which, and goes
/// to a match (scrolling to it and marking it). Its owner writes only this.
abstract class FindTarget extends ChangeNotifier {
  /// The field's words when empty, and its name to VoiceOver: "Search this
  /// conversation".
  String get placeholder;

  /// Finds [query] (as typed, a moment after the last key; empty clears),
  /// and goes to the first match. Says what it found by notifying.
  void search(String query);

  /// Goes to the next match (1) or the one before (-1), wrapping.
  void step(int by);

  /// The bar closed: forgets the query and stops marking its matches.
  void clear();

  /// How many matches; [capped] when there are more than that.
  int get count => 0;
  bool get capped => false;

  /// Which match is shown, from 0; -1 for none.
  int get current => -1;

  /// Whether a search is on its way.
  bool get searching => false;

  /// Whether a search was made (and not cleared): an empty result says
  /// "No matches" only then.
  bool get searched => false;

  /// Why it couldn't search, in words to show; null when it could.
  String? get failed => null;

  /// Whether the next match is further up the screen (a conversation, read
  /// from its newest end); else down, as in a document.
  bool get nextIsUp => false;

  /// "3 of 12", "3 of 1000+", "No matches", or nothing before a search.
  String get countLabel {
    if (failed case final why?) return why;
    if (searching && count == 0) return 'Searching…';
    if (!searched) return '';
    if (count == 0) return 'No matches';
    return '${current + 1} of $count${capped ? '+' : ''}';
  }
}

/// One pane's find: its target, and the bar's state, shared by whichever
/// bar shows it (the floating one, a phone's).
class FindSession {
  FindSession._(this._scope);

  final _FindScopeState _scope;

  FindTarget get target => _scope.widget.target ?? _nothing;

  /// A pane with a search field of its own: ⌘F goes to it.
  bool get own => _scope.widget.field != null;

  /// What's typed.
  final query = TextEditingController();

  /// The field's focus.
  final field = FocusNode(debugLabel: 'Find');

  /// Whether its bar is open.
  ValueListenable<bool> get isOpen => _open;
  final _open = ValueNotifier(false);

  /// What had the keys when the bar opened: they go back to it when it
  /// closes.
  FocusNode? _before;

  Timer? _typing;
  String _searched = '';

  /// Opens its bar (or, open, goes back to its field), with what was found
  /// last selected, to type over.
  void open() {
    // A pane's own search field: ⌘F goes there, what's in it selected.
    if (_scope.widget.field case final field?) {
      final words = _scope.widget.words;
      if (words != null) {
        words.selection = TextSelection(
          baseOffset: 0,
          extentOffset: words.text.length,
        );
      }
      field.requestFocus();
      return;
    }
    final wasOpen = _open.value;
    final focused = FocusManager.instance.primaryFocus;
    if (!wasOpen && focused != field) _before = focused;
    _open.value = true;
    query.selection = TextSelection(
      baseOffset: 0,
      extentOffset: query.text.length,
    );
    // Reopened with words: they're found again.
    if (!wasOpen && query.text.trim().isNotEmpty) _search();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (_open.value && field.context != null) field.requestFocus();
    });
    WidgetsBinding.instance.ensureVisualUpdate();
  }

  /// Closes its bar: the pane stops marking matches; the words stay for
  /// next time.
  void close() {
    if (!_open.value) return;
    _open.value = false;
    _typing?.cancel();
    _searched = '';
    target.clear();
    // The keys go back to the pane.
    _scope.focusPane();
  }

  /// The next match (1) or the one before (-1).
  void step(int by) {
    if (own) return;
    if (!_open.value) return open();
    if (query.text != _searched) return _search();
    target.step(by);
  }

  void _typed() {
    if (!_open.value || query.text == _searched) return;
    _typing?.cancel();
    _typing = Timer(const Duration(milliseconds: 150), _search);
  }

  void _search() {
    _typing?.cancel();
    _searched = query.text;
    target.search(query.text.trim());
  }

  void _dispose() {
    _typing?.cancel();
    query.dispose();
    field.dispose();
    _open.dispose();
  }
}

/// The find sessions on show, for what isn't inside a pane but shows its
/// find (a phone page's header), and for ⌘F with focus in no pane.
class FindSessions {
  FindSessions._();

  static final instance = FindSessions._();

  /// Every pane on show that can be searched, in the order they came.
  ValueListenable<List<FindSession>> get sessions => _sessions;
  final _sessions = ValueNotifier<List<FindSession>>(const []);

  /// The pane last clicked or tapped in.
  FindSession? _pressed;

  /// The focus manager [_key] is a late handler of (a test's binding can
  /// start a new one).
  FocusManager? _handling;

  /// The session of the pane [context] is in (or above), if any.
  FindSession? forPane(BuildContext context) {
    final mine = FindScope.of(context);
    if (mine != null) return mine;
    for (final session in _sessions.value.reversed) {
      final scope = session._scope;
      if (!scope.mounted) continue;
      var inside = false;
      scope.context.visitAncestorElements((element) {
        if (element == context) inside = true;
        return !inside;
      });
      if (inside) return session;
    }
    return null;
  }

  /// The session of a pane inside [context] (a view whose body can be
  /// searched, for a button of its own outside the pane), if any; with
  /// [only], the one whose target it picks (the conversation's, not a pane
  /// beside it such as Needs you).
  FindSession? within(
    BuildContext context, {
    bool Function(FindTarget target)? only,
  }) {
    for (final session in _sessions.value.reversed) {
      final scope = session._scope;
      if (!scope.mounted) continue;
      if (only != null && !only(session.target)) continue;
      var inside = false;
      scope.context.visitAncestorElements((element) {
        if (element == context) inside = true;
        return !inside;
      });
      if (inside) return session;
    }
    return null;
  }

  void _add(FindSession session) {
    _sessions.value = [..._sessions.value, session];
    if (_handling != FocusManager.instance) {
      _handling = FocusManager.instance;
      FocusManager.instance.addLateKeyEventHandler(_key);
    }
  }

  void _remove(FindSession session) {
    if (_pressed == session) _pressed = null;
    _sessions.value = [
      for (final other in _sessions.value)
        if (other != session) other,
    ];
  }

  /// ⌘F where focus is in no pane that can be searched (nothing focused,
  /// the sidebar, a message box): the pane last clicked in, else the
  /// largest on show. ⌘G, ⇧⌘G and Esc go to the one open.
  KeyEventResult _key(KeyEvent event) {
    if (event is! KeyDownEvent) return KeyEventResult.ignored;
    final key = event.logicalKey;
    final keyboard = HardwareKeyboard.instance;
    final command = _apple ? keyboard.isMetaPressed : keyboard.isControlPressed;
    if (command && key == LogicalKeyboardKey.keyF) {
      final session = _resolve();
      if (session == null) return KeyEventResult.ignored;
      session.open();
      return KeyEventResult.handled;
    }
    final open = _sessions.value.where((s) => s._open.value).lastOrNull;
    if (open == null) return KeyEventResult.ignored;
    if (command && key == LogicalKeyboardKey.keyG) {
      open.step(keyboard.isShiftPressed ? -1 : 1);
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  FindSession? _resolve() {
    final shown = [
      for (final session in _sessions.value)
        if (session._scope.mounted && session._scope._inSight()) session,
    ];
    final pressed = _pressed;
    if (pressed != null && shown.contains(pressed)) return pressed;
    FindSession? largest;
    var most = 0.0;
    for (final session in shown) {
      final size = session._scope._area();
      if (size > most) (largest, most) = (session, size);
    }
    return largest;
  }
}

/// [child], floating over a pane in a [Stack] (Needs you's chip), hidden
/// while a find bar is open in a pane of that stack, so it never covers
/// the bar's arrows and Done (an iPad by touch).
class ClearOfFindBar extends StatefulWidget {
  const ClearOfFindBar({super.key, required this.child});

  final Widget child;

  @override
  State<ClearOfFindBar> createState() => _ClearOfFindBarState();
}

class _ClearOfFindBarState extends State<ClearOfFindBar> {
  var _watched = <FindSession>[];

  @override
  void initState() {
    super.initState();
    FindSessions.instance.sessions.addListener(_sessionsChanged);
    _sessionsChanged();
  }

  @override
  void dispose() {
    FindSessions.instance.sessions.removeListener(_sessionsChanged);
    for (final session in _watched) {
      session.isOpen.removeListener(_changed);
    }
    super.dispose();
  }

  void _sessionsChanged() {
    for (final session in _watched) {
      session.isOpen.removeListener(_changed);
    }
    _watched = [...FindSessions.instance.sessions.value];
    for (final session in _watched) {
      session.isOpen.addListener(_changed);
    }
    // Panes come and go while the tree builds: looked at once it's built.
    WidgetsBinding.instance.addPostFrameCallback((_) => _changed());
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  /// Whether [session]'s pane is in the stack this floats in.
  bool _beside(FindSession session) {
    final stack = context.findAncestorRenderObjectOfType<RenderStack>();
    if (stack == null || !session._scope.mounted) return false;
    RenderObject? object = session._scope.context.findRenderObject();
    while (object != null) {
      if (object == stack) return true;
      object = object.parent;
    }
    return false;
  }

  @override
  Widget build(BuildContext context) {
    final hidden = _watched.any(
      (session) => session.isOpen.value && _beside(session),
    );
    return Visibility(
      visible: !hidden,
      maintainState: true,
      child: widget.child,
    );
  }
}

/// How find's shortcut reads here: "⌘F" on Apple's platforms, else
/// "Ctrl+F".
String get findShortcut => _apple ? '⌘F' : 'Ctrl+F';

bool get _apple =>
    defaultTargetPlatform == TargetPlatform.macOS ||
    defaultTargetPlatform == TargetPlatform.iOS;

/// Makes [child] (a pane's body) searchable with ⌘F, by [target]. On a Mac
/// or iPad its bar floats at the top right; on a phone the page shows it.
class FindScope extends StatefulWidget {
  const FindScope({
    super.key,
    required FindTarget this.target,
    required this.child,
  }) : field = null,
       words = null;

  /// A pane that searches with a field of its own (History's): ⌘F goes
  /// to [field], [words] selected to type over; no bar.
  const FindScope.field({
    super.key,
    required FocusNode this.field,
    this.words,
    required this.child,
  }) : target = null;

  final FindTarget? target;
  final FocusNode? field;
  final TextEditingController? words;
  final Widget child;

  /// The session of the pane [context] is in, if it can be searched.
  static FindSession? of(BuildContext context) =>
      context.getInheritedWidgetOfExactType<_FindSessionScope>()?.session;

  @override
  State<FindScope> createState() => _FindScopeState();
}

class _FindScopeState extends State<FindScope> {
  late final session = FindSession._(this);

  /// The pane's own focus, which the keys go back to when the bar closes.
  final _pane = FocusNode(debugLabel: 'FindScope', skipTraversal: true);

  /// The floating bar, to measure.
  final _bar = GlobalKey();

  /// How far the pane's body moves down for the bar: in a pane too narrow
  /// for the bar to float beside what it covers (a panel), its height.
  var _inset = 0.0;

  /// A pane narrower than this has its body under the bar, not behind it.
  static const _narrow = 480.0;

  /// Measures the bar once laid out, and moves the body for it.
  void _placeBar() {
    if (!mounted) return;
    final pane = context.findRenderObject();
    final bar = _bar.currentContext?.findRenderObject();
    var inset = 0.0;
    if (session._open.value &&
        pane is RenderBox &&
        pane.hasSize &&
        pane.size.width < _narrow &&
        bar is RenderBox &&
        bar.hasSize) {
      inset = Space.s + bar.size.height + Space.s;
    }
    if (inset != _inset) setState(() => _inset = inset);
  }

  @override
  void initState() {
    super.initState();
    session.query.addListener(session._typed);
    widget.target?.addListener(_changed);
    FindSessions.instance._add(session);
  }

  @override
  void didUpdateWidget(FindScope old) {
    super.didUpdateWidget(old);
    if (old.target != widget.target) {
      old.target?.removeListener(_changed);
      widget.target?.addListener(_changed);
      if (session._open.value && session.query.text.trim().isNotEmpty) {
        session._search();
      }
    }
  }

  @override
  void dispose() {
    widget.target?.removeListener(_changed);
    FindSessions.instance._remove(session);
    session._dispose();
    _pane.dispose();
    super.dispose();
  }

  void _changed() {
    if (mounted) setState(() {});
  }

  /// The keys back to the pane: to what takes them inside it (its
  /// selectable text, so ⌘A and ⌘C work on), else the pane itself.
  /// Back where the keys were before the bar opened (the message box, a
  /// selection), else to the pane itself: never to a field there that
  /// wasn't being typed in (Settings' Claktory name).
  void focusPane() {
    final before = session._before;
    session._before = null;
    if (!mounted || !session.field.hasFocus) return;
    if (before != null &&
        before != session.field &&
        before.context != null &&
        before.canRequestFocus) {
      before.requestFocus();
      return;
    }
    _pane.requestFocus();
  }

  bool _inSight() {
    final box = context.findRenderObject();
    if (box is! RenderBox || !box.attached || !box.hasSize) return false;
    final result = HitTestResult();
    WidgetsBinding.instance.hitTestInView(
      result,
      box.localToGlobal(box.size.center(Offset.zero)),
      View.of(context).viewId,
    );
    return result.path.any((entry) => entry.target == box);
  }

  double _area() {
    final box = context.findRenderObject();
    if (box is! RenderBox || !box.hasSize) return 0;
    return box.size.width * box.size.height;
  }

  @override
  Widget build(BuildContext context) {
    final command = _apple;
    final bindings = <ShortcutActivator, VoidCallback>{
      SingleActivator(
        LogicalKeyboardKey.keyF,
        meta: command,
        control: !command,
      ): session.open,
      SingleActivator(
        LogicalKeyboardKey.keyG,
        meta: command,
        control: !command,
      ): () =>
          session.step(1),
      SingleActivator(
        LogicalKeyboardKey.keyG,
        meta: command,
        control: !command,
        shift: true,
      ): () =>
          session.step(-1),
    };
    return _FindSessionScope(
      session: session,
      child: CallbackShortcuts(
        bindings: bindings,
        child: Focus(
          focusNode: _pane,
          child: Listener(
            behavior: HitTestBehavior.translucent,
            onPointerDown: (_) => FindSessions.instance._pressed = session,
            child: ValueListenableBuilder(
              valueListenable: session.isOpen,
              builder: (context, open, child) {
                final floating = open && !isPhoneLayout(context);
                WidgetsBinding.instance.addPostFrameCallback(
                  (_) => _placeBar(),
                );
                return Stack(
                  // The pane's body as it would be laid out without the bar.
                  fit: StackFit.passthrough,
                  children: [
                    Padding(
                      padding: EdgeInsets.only(top: floating ? _inset : 0),
                      child: child,
                    ),
                    // A phone's page shows its own bar, at its foot.
                    if (floating)
                      Positioned(
                        top: Space.s,
                        left: Space.m,
                        right: Space.m,
                        // At the right, as narrow as the pane needs.
                        child: Align(
                          alignment: Alignment.topRight,
                          child: FindBar(key: _bar, session: session),
                        ),
                      ),
                  ],
                );
              },
              child: widget.child,
            ),
          ),
        ),
      ),
    );
  }
}

class _FindSessionScope extends InheritedWidget {
  const _FindSessionScope({required this.session, required super.child});

  final FindSession session;

  @override
  bool updateShouldNotify(_FindSessionScope old) => old.session != session;
}

/// The find bar on a Mac or iPad: the field, which match of how many, the
/// previous and next, and Done.
class FindBar extends StatelessWidget {
  const FindBar({super.key, required this.session});

  final FindSession session;

  /// Narrower than this (a panel), the field takes a line of its own over
  /// the count, the arrows and Done.
  static const _twoLines = 400.0;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final target = session.target;
    void step(int by) => session.step(by);
    // Up the screen first, then down: where the matches go.
    final arrows = target.nextIsUp
        ? const [(1, 'Next match', true), (-1, 'Previous match', false)]
        : const [(-1, 'Previous match', true), (1, 'Next match', false)];
    final field = CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.enter): () => step(1),
        const SingleActivator(LogicalKeyboardKey.enter, shift: true): () =>
            step(-1),
        const SingleActivator(LogicalKeyboardKey.escape): session.close,
      },
      child: TextField(
        controller: session.query,
        focusNode: session.field,
        placeholder: target.placeholder,
        semanticsLabel: target.placeholder,
        // Where Return comes as the keyboard's action rather than a key the
        // bindings above see (an iPad's, on screen or not), it goes to the
        // next match too, the field kept (Zucchero's row 214). A key they
        // take never reaches the field: no step twice.
        textInputAction: TextInputAction.search,
        onSubmitted: (_) => step(1),
        onEditingComplete: () {},
      ),
    );
    final count = ListenableBuilder(
      listenable: target,
      builder: (context, _) => Text(
        target.countLabel,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: TypeScale.of(context).caption.copyWith(
          color: target.failed == null ? palette.textMuted : palette.danger,
          fontFeatures: const [FontFeature.tabularFigures()],
        ),
      ),
    );
    final controls = [
      for (final (by, label, up) in arrows)
        GlyphButton(
          label: label,
          active: false,
          onTap: () => step(by),
          glyph: (color) => RotatedBox(
            quarterTurns: up ? 1 : 3,
            child: ExtensionGlyph(
              icon: 'chevron-left',
              name: label,
              color: color,
            ),
          ),
        ),
      _Done(onTap: session.close),
    ];
    return Unselectable(
      child: TextFieldTapRegion(
        child: Container(
          padding: const EdgeInsets.fromLTRB(
            Space.s,
            Space.xs,
            Space.xs,
            Space.xs,
          ),
          decoration: BoxDecoration(
            color: palette.canvas,
            borderRadius: BorderRadius.circular(Radii.floating),
            border: Border.all(color: palette.hairline),
            boxShadow: floatingShadow,
          ),
          child: LayoutBuilder(
            builder: (context, constraints) => constraints.maxWidth < _twoLines
                ? Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    spacing: Space.xs,
                    children: [
                      field,
                      Row(
                        spacing: Space.xs,
                        children: [
                          Expanded(child: count),
                          ...controls,
                        ],
                      ),
                    ],
                  )
                : Row(
                    mainAxisSize: MainAxisSize.min,
                    spacing: Space.xs,
                    children: [
                      // 220 wide, less where it's only just wide enough.
                      Flexible(child: SizedBox(width: 220, child: field)),
                      count,
                      ...controls,
                    ],
                  ),
          ),
        ),
      ),
    );
  }
}

class _Done extends StatelessWidget {
  const _Done({required this.onTap});

  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      label: 'Done',
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => Container(
          height: 28,
          padding: const EdgeInsets.symmetric(horizontal: Space.s),
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: hovered ? palette.hover : const Color(0x00000000),
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          child: Text(
            'Done',
            style: TypeScale.of(context).label.copyWith(color: palette.text),
          ),
        ),
      ),
    );
  }
}

/// The wash of a match ([FindHighlight]): the yellow a find marks with on
/// Apple's platforms, as the PDF viewer's.
const _findYellow = Color(0xFFFFD60A);

/// One occurrence as laid out: in the paragraph whose words are [text]
/// (as [RenderParagraph.text] reads, without semantics labels), the one
/// starting at [start].
typedef FindExact = ({String text, int start});

/// What a find marks in the text under it: [query]'s occurrences, washed
/// yellow, and in the match gone to ([current]), yellow with dark text, so
/// it stands out from the rest. Text that shows a find's matches
/// ([Markdown], a message's words) asks it with [mark].
class FindHighlight extends InheritedWidget {
  const FindHighlight({
    super.key,
    required this.query,
    this.current = false,
    this.nth,
    this.exact,
    required super.child,
  });

  /// As found: matched case-insensitively. Empty marks nothing.
  final String query;
  final bool current;

  /// With [current], which occurrence is the one gone to, from 0 in
  /// reading order, when the text under it is one span; null for all.
  final int? nth;

  /// With [current], the one occurrence gone to, where it's laid out (a
  /// long message's, among many): only it in full, the rest washed. Null
  /// for [nth]'s.
  final FindExact? exact;

  /// [span] with the occurrences of the query of the [FindHighlight] above
  /// [context] marked; as it is when there's none. A match across two
  /// styles ("a **b**") isn't marked.
  static TextSpan mark(BuildContext context, TextSpan span) {
    final find = context.dependOnInheritedWidgetOfExactType<FindHighlight>();
    if (find == null || find.query.isEmpty) return span;
    final exact = find.exact;
    if (find.current && exact != null) {
      final words = span.toPlainText(includeSemanticsLabels: false);
      return markFound(
        span,
        find.query,
        current: true,
        at: words == exact.text ? exact.start : -1,
      );
    }
    return markFound(span, find.query, current: find.current, nth: find.nth);
  }

  /// [text] in [style], marked as [mark] does.
  static TextSpan markText(
    BuildContext context,
    String text,
    TextStyle? style,
  ) => mark(context, TextSpan(text: text, style: style));

  @override
  bool updateShouldNotify(FindHighlight old) =>
      old.query != query ||
      old.current != current ||
      old.nth != nth ||
      old.exact != exact;
}

/// [span] with [query]'s occurrences in each piece of its text washed
/// (see [FindHighlight]): with [current], the one gone to (the one at [at]
/// in its words, none for -1; else the [nth] in reading order, or all when
/// null) in full.
TextSpan markFound(
  TextSpan span,
  String query, {
  bool current = false,
  int? nth,
  int? at,
}) {
  final needle = query.toLowerCase();
  if (needle.isEmpty) return span;
  const strong = TextStyle(
    backgroundColor: _findYellow,
    color: Color(0xFF1C1C1E),
  );
  final wash = TextStyle(backgroundColor: _findYellow.withValues(alpha: 0.35));
  var marked = false;
  var seen = 0;
  // Where in the words the walk is, as toPlainText reads them.
  var offset = 0;
  TextStyle washOf() {
    final gone =
        current && (at != null ? offset == at : nth == null || nth == seen);
    seen++;
    return gone ? strong : wash;
  }

  InlineSpan walk(InlineSpan span) {
    if (span is! TextSpan) {
      // A placeholder's one character.
      if (span is PlaceholderSpan) offset++;
      return span;
    }
    final text = span.text;
    final pieces = text == null || span.semanticsLabel != null
        ? null
        : _split(text, needle);
    // Its own words come before its children's, in reading order.
    final ownPieces = pieces == null
        ? null
        : [
            for (final (text, match) in pieces)
              (text, match ? washOf() : null, offset += text.length),
          ];
    if (pieces == null) offset += text?.length ?? 0;
    final children = span.children;
    final walked = children == null
        ? null
        : [for (final c in children) walk(c)];
    if (ownPieces == null) {
      if (walked == null) return span;
      return TextSpan(
        text: text,
        style: span.style,
        children: walked,
        recognizer: span.recognizer,
        mouseCursor: span.mouseCursor,
        onEnter: span.onEnter,
        onExit: span.onExit,
        semanticsLabel: span.semanticsLabel,
        locale: span.locale,
        spellOut: span.spellOut,
      );
    }
    marked = true;
    TextSpan piece(String text, TextStyle? style) => TextSpan(
      text: text,
      style: style,
      recognizer: span.recognizer,
      mouseCursor: span.mouseCursor,
      onEnter: span.onEnter,
      onExit: span.onExit,
    );
    return TextSpan(
      style: span.style,
      locale: span.locale,
      children: [
        for (final (text, style, _) in ownPieces) piece(text, style),
        ...?walked,
      ],
    );
  }

  final result = walk(span) as TextSpan;
  return marked ? result : span;
}

/// [text] in pieces, each saying whether it's an occurrence of [needle]
/// (lower case); null when it has none, or when lower case changes its
/// length (so places in one aren't places in the other).
List<(String, bool)>? _split(String text, String needle) {
  final lower = text.toLowerCase();
  if (lower.length != text.length) return null;
  var at = lower.indexOf(needle);
  if (at < 0) return null;
  final pieces = <(String, bool)>[];
  var from = 0;
  while (at >= 0) {
    if (at > from) pieces.add((text.substring(from, at), false));
    pieces.add((text.substring(at, at + needle.length), true));
    from = at + needle.length;
    at = lower.indexOf(needle, from);
  }
  if (from < text.length) pieces.add((text.substring(from), false));
  return pieces;
}

/// Where a pane's find is: the query, and the occurrence gone to, as the
/// item it's in ([LocalFind.items]' key) and which of the item's it is.
typedef LocalMarks = ({String query, Object? at, int nth});

/// A find in what a pane holds itself (a file's text, a list of items):
/// searched here, in [items]' order, each item's occurrences in turn.
/// [go] brings occurrence [nth] of the item [key] into sight; the pane
/// marks the matches from [marks] (with [FindHighlight]).
class LocalFind extends FindTarget {
  LocalFind({required this.placeholder, required this.items, required this.go});

  @override
  final String placeholder;

  /// What can be found, in order: each item's key and its text.
  final Iterable<(Object, String)> Function() items;

  final void Function(Object key, int nth) go;

  ValueListenable<LocalMarks> get marks => _marks;
  final _marks = ValueNotifier<LocalMarks>((query: '', at: null, nth: 0));

  var _query = '';
  var _found = <(Object, int)>[];
  var _current = -1;
  var _searched = false;

  @override
  int get count => _found.length;

  @override
  int get current => _current;

  @override
  bool get searched => _searched;

  @override
  void search(String query) {
    _query = query;
    _searched = query.isNotEmpty;
    _find();
    _current = _found.isEmpty ? -1 : 0;
    _go();
    notifyListeners();
  }

  /// What the pane holds changed (a file read again): found again, staying
  /// on the same occurrence where it can, without moving; or, shown
  /// another way ([reveal]: a file's Formatted and Plain), brought into
  /// sight there.
  void refresh({bool reveal = false}) {
    if (_query.isEmpty) return;
    final before = _found;
    final was = _current;
    _find();
    _current = _found.isEmpty ? -1 : was.clamp(0, _found.length - 1);
    if (reveal) {
      _go();
    } else {
      _mark();
    }
    // Told only when what's found changed: a pane refreshes after every
    // build, which mustn't build it again.
    if (_current != was || !listEquals(before, _found)) notifyListeners();
  }

  void _find() {
    final needle = _query.toLowerCase();
    _found = [];
    if (needle.isEmpty) return;
    for (final (key, text) in items()) {
      final lower = text.toLowerCase();
      var nth = 0;
      for (var at = lower.indexOf(needle); at >= 0;) {
        _found.add((key, nth++));
        at = lower.indexOf(needle, at + needle.length);
      }
    }
  }

  @override
  void step(int by) {
    if (_found.isEmpty) return;
    _current = (_current + by) % _found.length;
    _go();
    notifyListeners();
  }

  @override
  void clear() {
    _query = '';
    _found = [];
    _current = -1;
    _searched = false;
    _mark();
    notifyListeners();
  }

  @override
  void dispose() {
    _marks.dispose();
    super.dispose();
  }

  void _mark() {
    final at = _current < 0 ? null : _found[_current];
    _marks.value = (query: _query, at: at?.$1, nth: at?.$2 ?? 0);
  }

  void _go() {
    _mark();
    if (_current >= 0) go(_found[_current].$1, _found[_current].$2);
  }
}

/// Marks a [LocalFind]'s matches in [child], the item [itemKey]: all of
/// its occurrences in full when it's the one gone to, or with [exact]
/// (its text is one span) only the one.
class LocalFindMarks extends StatelessWidget {
  const LocalFindMarks({
    super.key,
    required this.find,
    required this.itemKey,
    this.exact = false,
    required this.child,
  });

  final LocalFind? find;
  final Object itemKey;
  final bool exact;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final find = this.find;
    if (find == null) return child;
    return ValueListenableBuilder(
      valueListenable: find.marks,
      child: child,
      builder: (context, marks, child) {
        final current = marks.at == itemKey;
        return FindHighlight(
          query: marks.query,
          current: current,
          nth: current && exact ? marks.nth : null,
          child: child!,
        );
      },
    );
  }
}

/// The target of a pane that searches with a field of its own: finds
/// nothing.
final _nothing = _Nothing();

class _Nothing extends FindTarget {
  @override
  String get placeholder => '';

  @override
  void search(String query) {}

  @override
  void step(int by) {}

  @override
  void clear() {}
}

/// Brings what [key] holds into sight in [scroll]'s list, a fifth of the
/// way down: once laid out; one not built yet (far down a long lazy list)
/// is near [fraction] of the way, where the list goes first to build it.
void bringIntoSight(
  ScrollController scroll,
  GlobalKey key,
  double fraction, {
  int tries = 4,
}) {
  WidgetsBinding.instance.addPostFrameCallback((_) {
    if (!scroll.hasClients) return;
    final place = key.currentContext;
    if (place != null) {
      Scrollable.ensureVisible(place, alignment: 0.2);
      return;
    }
    if (tries == 0) return;
    final position = scroll.position;
    position.jumpTo(position.maxScrollExtent * fraction.clamp(0.0, 1.0));
    bringIntoSight(scroll, key, fraction, tries: tries - 1);
  });
  WidgetsBinding.instance.ensureVisualUpdate();
}

/// A find in the text a pane shows, whatever builds it (Settings, made of
/// many controls): every paragraph laid out under [root] is searched, in
/// the order they're laid out, and the matches are washed over the text
/// ([RenderedFindMarks]). Only what's laid out is found: a lazy list
/// lays all of its children out ([ListView.cacheExtent]) for it.
class RenderedFind extends FindTarget {
  RenderedFind({required this.placeholder, required this.root});

  @override
  final String placeholder;

  /// Where what's searched is: the pane's scroll view.
  final GlobalKey root;

  var _query = '';
  var _found = <(RenderParagraph, int, int)>[];
  var _current = -1;
  var _searched = false;

  @override
  int get count => _found.length;

  @override
  int get current => _current;

  @override
  bool get searched => _searched;

  /// The matches now, each a paragraph and where in its text, and which is
  /// the one gone to.
  List<(RenderParagraph, int, int)> get found => _found;

  @override
  void search(String query) {
    _query = query;
    _searched = query.isNotEmpty;
    _collect();
    _current = _found.isEmpty ? -1 : 0;
    _go();
    notifyListeners();
  }

  /// The pane was built again: found again where it's laid out now,
  /// staying on the same match where it can, without moving.
  void refresh() {
    if (_query.isEmpty) return;
    final before = _found;
    final was = _current;
    _collect();
    _current = _found.isEmpty ? -1 : was.clamp(0, _found.length - 1);
    if (_current != was || !listEquals(before, _found)) notifyListeners();
  }

  void _collect() {
    final object = root.currentContext?.findRenderObject();
    _found = object == null ? [] : shownMatches(object, _query);
  }

  @override
  void step(int by) {
    if (_found.isEmpty) return;
    _current = (_current + by) % _found.length;
    _go();
    notifyListeners();
  }

  @override
  void clear() {
    _query = '';
    _found = [];
    _current = -1;
    _searched = false;
    notifyListeners();
  }

  void _go() {
    if (_current < 0) return;
    final (paragraph, start, end) = _found[_current];
    final rect = foundRect(paragraph, start, end);
    if (rect == null) return;
    // Clear of the find bar above it, with a line's room below.
    paragraph.showOnScreen(
      rect: rect.inflate(Space.xxl),
      duration: Motion.fast,
      curve: Curves.easeOut,
    );
  }
}

/// Where [query] shows in what's laid out under [root], in reading order:
/// each paragraph and where in its text, matched case-insensitively.
List<(RenderParagraph, int, int)> shownMatches(
  RenderObject root,
  String query,
) {
  final found = <(RenderParagraph, int, int)>[];
  final needle = query.toLowerCase();
  if (needle.isEmpty) return found;
  void visit(RenderObject object) {
    if (object is RenderParagraph) {
      final text = object.text.toPlainText(includeSemanticsLabels: false);
      final lower = text.toLowerCase();
      // Places in lower case are places in the text only when it keeps
      // its length.
      if (lower.length == text.length) {
        for (var at = lower.indexOf(needle); at >= 0;) {
          found.add((object, at, at + needle.length));
          at = lower.indexOf(needle, at + needle.length);
        }
      }
    }
    object.visitChildren(visit);
  }

  visit(root);
  return found;
}

/// Whether a find's match gone to shows, in full, in what [root] lays
/// out ([FindHighlight.current]).
bool showsCurrent(RenderObject root) {
  var shows = false;
  void visit(RenderObject object) {
    if (shows) return;
    if (object is RenderParagraph) {
      object.text.visitChildren((span) {
        if (span.style?.backgroundColor == _findYellow) shows = true;
        return !shows;
      });
    }
    object.visitChildren(visit);
  }

  visit(root);
  return shows;
}

/// Where [start] to [end] of [paragraph]'s text is laid out, in its own
/// coordinates: its first line's part; null when it isn't laid out.
Rect? foundRect(RenderParagraph paragraph, int start, int end) {
  if (!paragraph.attached || !paragraph.hasSize) return null;
  final boxes = paragraph.getBoxesForSelection(
    TextSelection(baseOffset: start, extentOffset: end),
  );
  return boxes.isEmpty ? null : boxes.first.toRect();
}

/// Makes [child] (a pane built from many widgets: Settings, a panel)
/// searchable with ⌘F by the words it lays out ([RenderedFind]), each
/// match washed yellow over the text, the one gone to stronger: painted
/// over it, since the text isn't the find's to restyle, and moving with
/// any scroll inside. A lazy list in it finds only what it has laid out:
/// give it a [ListView.scrollCacheExtent] of [everything] to find in all.
class RenderedFindScope extends StatefulWidget {
  const RenderedFindScope({
    super.key,
    required this.placeholder,
    required this.child,
  });

  /// How far beyond the view a lazy list lays out for find: all of it.
  static const everything = ScrollCacheExtent.pixels(1e6);

  final String placeholder;
  final Widget child;

  @override
  State<RenderedFindScope> createState() => _RenderedFindScopeState();
}

class _RenderedFindScopeState extends State<RenderedFindScope> {
  final _root = GlobalKey();
  final _layer = GlobalKey();
  late final _find = RenderedFind(placeholder: widget.placeholder, root: _root);

  /// Ticks as anything inside scrolls, to paint the marks where they are.
  final _scrolled = ValueNotifier(0);

  @override
  void dispose() {
    _find.dispose();
    _scrolled.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    // What's found follows the pane as it changes.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _find.refresh();
    });
    return FindScope(
      target: _find,
      child: NotificationListener<ScrollNotification>(
        onNotification: (_) {
          _scrolled.value++;
          return false;
        },
        child: Stack(
          fit: StackFit.passthrough,
          children: [
            KeyedSubtree(key: _root, child: widget.child),
            Positioned.fill(
              child: IgnorePointer(
                child: ClipRect(
                  child: CustomPaint(
                    key: _layer,
                    painter: _MarksPainter(
                      _find,
                      _layer,
                      Listenable.merge([_find, _scrolled]),
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

class _MarksPainter extends CustomPainter {
  _MarksPainter(this.find, this.layer, Listenable repaint)
    : super(repaint: repaint);

  final RenderedFind find;
  final GlobalKey layer;

  @override
  void paint(Canvas canvas, Size size) {
    final box = layer.currentContext?.findRenderObject();
    if (box is! RenderBox || !box.attached) return;
    final wash = Paint()..color = _findYellow.withValues(alpha: 0.35);
    final strong = Paint()..color = _findYellow.withValues(alpha: 0.7);
    final found = find.found;
    for (var i = 0; i < found.length; i++) {
      final (paragraph, start, end) = found[i];
      if (!paragraph.attached || !paragraph.hasSize) continue;
      final transform = paragraph.getTransformTo(box);
      for (final text in paragraph.getBoxesForSelection(
        TextSelection(baseOffset: start, extentOffset: end),
      )) {
        final rect = MatrixUtils.transformRect(transform, text.toRect());
        if (!rect.overlaps(Offset.zero & size)) continue;
        canvas.drawRRect(
          RRect.fromRectAndRadius(rect.inflate(1), const Radius.circular(2)),
          i == find.current ? strong : wash,
        );
      }
    }
  }

  @override
  bool shouldRepaint(_MarksPainter old) => old.find != find;
}
