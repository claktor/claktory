import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../conversation_find.dart' show ConversationFind;
import '../find.dart' show FindSessions;
import '../layout.dart';
import '../panes.dart' show pageKinds;
import '../scope.dart' show AppPage;
import '../protocol/factory/v1/factory.pb.dart';
import 'columns.dart';
import 'document_host.dart' show DocumentPanel, documentKind;
import 'settings_keys.dart' show FocusSettingsSearchIntent;
import 'workbench.dart';

/// The workbench's keyboard commands: the sidebar, moving between panes and
/// Panes. Each has a title as the iPadOS menu and the ⌘-hold list show it,
/// keys, and what it does; one with nothing to do is disabled rather than
/// left out, so the list doesn't shift.
///
/// The keys are bound here, in Flutter, always. Showing them in the
/// system's menu (UIKeyCommand, the menu bar and the ⌘-hold list) is the
/// native side's, through the `factory/commands` channel (phase 1, with
/// Tomás). A key UIKit runs as a menu command never reaches Flutter, and
/// one that reaches Flutter isn't run again by the menu ([CommandMenu]),
/// so no key is handled twice, whether or not the menu's keys fire (in the
/// Simulator they don't).
class WorkbenchCommand {
  const WorkbenchCommand({
    required this.id,
    required this.title,
    required this.keys,
    this.run,
  });

  /// Stable, for the native menu and tests: `sidebar`, `next-pane`…
  final String id;

  /// As the iPadOS menu bar and the ⌘-hold list show it: in title case,
  /// as the system's menus are (the Mac's menu bar is the same exception to
  /// the app's sentence case).
  final String title;

  final SingleActivator keys;

  /// Null when there's nothing to do (one pane, no agent).
  final VoidCallback? run;

  bool get enabled => run != null;
}

/// The workbench's commands for a window at [tier], with [agent] selected.
List<WorkbenchCommand> workbenchCommands({
  required WorkbenchTier tier,
  required WorkbenchFocus focus,
  required Agent? agent,
  VoidCallback? showPanes,
  List<Agent> pins = const [],
  void Function(Agent agent)? openPin,
  VoidCallback? openSettings,
  MoveUndo moveUndo = MoveUndo.none,
  VoidCallback? undoMove,
}) {
  final pinned = tier == WorkbenchTier.full && !focus.sidebarHidden;
  final sidebarShown = pinned || focus.overlayOpen;
  final cycle = agent == null ? const <String>[] : _cycle(tier, focus, agent);
  VoidCallback? step(int by) {
    if (agent == null || cycle.length < 2) return null;
    return () {
      final root = layoutOf(agent);
      final at = cycle.indexOf(focus.focusedIn(agent.id, root));
      final next = cycle[(at + by + cycle.length) % cycle.length];
      focus.focus(agent.id, next);
    };
  }

  // The pane being read, when it's a PDF: Find… searches it.
  final focused = agent == null
      ? null
      : leaves(layoutOf(agent))
            .where(
              (leaf) => leaf.id == focus.focusedIn(agent.id, layoutOf(agent)),
            )
            .firstOrNull;
  final document = focused != null && focused.leaf.kind == documentKind
      ? focused
      : null;
  // The conversation, when it's the pane being read: ⌘F finds in it, its find
  // bar the one ⌘F opens on a Mac.
  final conversation = agent != null && focused?.leaf.kind == chatKind;
  // Settings, when it's the page shown or the pane being read: ⌘F searches
  // it (settings_keys.dart).
  final settings = focus.settings;
  final searchSettings =
      settings != null &&
      settings.mounted &&
      (agent == null || focused?.leaf.kind == pageKinds[AppPage.settings]);

  return [
    WorkbenchCommand(
      id: 'sidebar',
      title: sidebarShown ? 'Hide Sidebar' : 'Show Sidebar',
      keys: const SingleActivator(
        LogicalKeyboardKey.keyS,
        meta: true,
        control: true,
      ),
      run: () => focus.toggleSidebar(tier),
    ),
    WorkbenchCommand(
      id: 'next-pane',
      title: 'Next Pane',
      keys: const SingleActivator(LogicalKeyboardKey.tab, control: true),
      run: step(1),
    ),
    WorkbenchCommand(
      id: 'previous-pane',
      title: 'Previous Pane',
      keys: const SingleActivator(
        LogicalKeyboardKey.tab,
        control: true,
        shift: true,
      ),
      run: step(-1),
    ),
    WorkbenchCommand(
      id: 'panes',
      title: 'Panes…',
      keys: const SingleActivator(
        LogicalKeyboardKey.keyP,
        meta: true,
        alt: true,
      ),
      run: agent == null || leaves(layoutOf(agent)).length < 2
          ? null
          : showPanes,
    ),
    WorkbenchCommand(
      id: 'find',
      // One word, whatever's read: the list names keys.
      title: searchSettings ? 'Search Settings' : 'Find',
      keys: const SingleActivator(LogicalKeyboardKey.keyF, meta: true),
      run: searchSettings
          ? () =>
                Actions.maybeInvoke(settings, const FocusSettingsSearchIntent())
          : conversation
          ? _findInConversation
          : agent == null || document == null
          ? null
          : () => focus.documents
                .of(agent.id, document)
                .openPanel(DocumentPanel.search),
    ),
    // ⌘, opens Settings at its last section, the platform's own key:
    // native puts it in the app menu's Settings
    // place, over iPadOS's own (which opens the system's Settings app).
    WorkbenchCommand(
      id: 'settings',
      title: 'Settings',
      keys: const SingleActivator(LogicalKeyboardKey.comma, meta: true),
      run: openSettings,
    ),
    // The person's pinned agents: ⌘1 to ⌘9 open the
    // first nine, named, as clicking a pin does. Only the pins there are:
    // they end the list, so nothing after them shifts.
    for (final (i, pin) in pins.take(9).indexed)
      WorkbenchCommand(
        id: 'open-pin-${i + 1}',
        title: 'Open ${pin.name}',
        keys: SingleActivator(_digits[i], meta: true),
        run: openPin == null ? null : () => openPin(pin),
      ),
    // Only while a move in the sidebar can be put back, last so nothing
    // shifts; disabled while a text field has the keys, whose own undo
    // ⌘Z is then.
    if (moveUndo != MoveUndo.none)
      WorkbenchCommand(
        id: 'undo-move',
        title: 'Undo Move',
        keys: const SingleActivator(LogicalKeyboardKey.keyZ, meta: true),
        run: moveUndo == MoveUndo.offered ? undoMove : null,
      ),
  ];
}

/// Whether the sidebar's last move can be put back from the keyboard.
enum MoveUndo {
  /// No move's Undo is offered.
  none,

  /// One is, and ⌘Z puts it back.
  offered,

  /// One is, but a text field has the keys: ⌘Z is its own undo.
  inField,
}

const _digits = [
  LogicalKeyboardKey.digit1,
  LogicalKeyboardKey.digit2,
  LogicalKeyboardKey.digit3,
  LogicalKeyboardKey.digit4,
  LogicalKeyboardKey.digit5,
  LogicalKeyboardKey.digit6,
  LogicalKeyboardKey.digit7,
  LogicalKeyboardKey.digit8,
  LogicalKeyboardKey.digit9,
];

/// Opens the find bar of the conversation on show.
void _findInConversation() => FindSessions.instance.sessions.value
    .where((session) => session.target is ConversationFind)
    .lastOrNull
    ?.open();

/// The panes ⌃Tab moves through, in the layout's order: in the focused
/// tier all of them (each shows in turn), wider only those shown.
List<String> _cycle(WorkbenchTier tier, WorkbenchFocus focus, Agent agent) {
  final all = [for (final leaf in leaves(layoutOf(agent))) leaf.id];
  if (tier == WorkbenchTier.focused) return all;
  final hidden = focus.projectionOf(agent.id)?.hidden.toSet() ?? const {};
  return [
    for (final id in all)
      if (!hidden.contains(id)) id,
  ];
}

/// The system's menu for the workbench's commands: the iPadOS menu bar and
/// the list shown while ⌘ is held, through `factory/commands`
/// (app/test/ipad/fixtures/commands/ has the messages).
///
/// [publish] sends the commands whenever they change; native answers true
/// when it shows them as UIKeyCommands (calling back `run` with the id when
/// one is chosen or its keys pressed), and [native] is then true. Where
/// nothing answers (the Mac, an iPhone, an older build) it stays false.
/// Flutter binds the keys either way ([WorkbenchShortcuts], focus mode's
/// own): a `run` for a command whose keys Flutter has just had is the same
/// press, already handled there, and isn't run again.
class CommandMenu extends ChangeNotifier {
  CommandMenu({this.channel = const MethodChannel(channelName)}) {
    channel.setMethodCallHandler(_handle);
    HardwareKeyboard.instance.addHandler(_saw);
  }

  /// How long after Flutter had a command's keys a `run` for it counts as
  /// the same press.
  static const samePress = Duration(milliseconds: 500);

  static const channelName = 'factory/commands';

  final MethodChannel channel;

  bool get native => _native;
  var _native = false;

  var _runs = <String, VoidCallback>{};
  var _keys = <String, SingleActivator>{};

  /// When Flutter last had each command's keys, by id.
  final _had = <String, DateTime>{};
  String? _sent;
  var _disposed = false;

  /// Notes which commands' keys a key going down is; never handles it.
  bool _saw(KeyEvent event) {
    if (event is! KeyDownEvent) return false;
    final now = DateTime.now();
    for (final MapEntry(:key, :value) in _keys.entries) {
      if (value.accepts(event, HardwareKeyboard.instance)) _had[key] = now;
    }
    return false;
  }

  /// Sends [commands] unless they're what was sent last.
  Future<void> publish(List<WorkbenchCommand> commands) async {
    _runs = {for (final command in commands) command.id: ?command.run};
    _keys = {for (final command in commands) command.id: command.keys};
    final payload = encodeCommands(commands);
    final text = jsonEncode(payload);
    if (text == _sent) return;
    _sent = text;
    bool taken;
    try {
      taken = await channel.invokeMethod<bool>('setCommands', payload) ?? false;
    } on MissingPluginException {
      taken = false;
    } on PlatformException {
      taken = false;
    }
    if (_disposed || taken == _native) return;
    _native = taken;
    notifyListeners();
  }

  Future<Object?> _handle(MethodCall call) async {
    if (call.method != 'run') return null;
    final id = (call.arguments as Map?)?['id'];
    final run = _runs[id];
    if (run == null) return false;
    final had = _had.remove(id);
    // The press Flutter's binding already ran.
    if (had != null && DateTime.now().difference(had) < samePress) return true;
    run();
    return true;
  }

  @override
  void dispose() {
    _disposed = true;
    HardwareKeyboard.instance.removeHandler(_saw);
    channel.setMethodCallHandler(null);
    super.dispose();
  }
}

/// [commands] as `setCommands` sends them: each one's id, title, key
/// (UIKeyCommand's input: a lowercase letter, or a tab character for Tab)
/// and modifiers (UIKeyModifierFlags' names), and whether it can run now.
Map<String, Object?> encodeCommands(List<WorkbenchCommand> commands) => {
  'schema_version': 1,
  'commands': [
    for (final command in commands)
      {
        'id': command.id,
        'title': command.title,
        'input': keyCommandInput(command.keys.trigger),
        'modifiers': [
          if (command.keys.meta) 'command',
          if (command.keys.control) 'control',
          if (command.keys.alt) 'alternate',
          if (command.keys.shift) 'shift',
        ],
        'enabled': command.enabled,
      },
  ],
};

/// [key] as UIKeyCommand's input: a lowercase letter or digit, a control
/// character for Tab, Return and Delete (backspace, U+0008: that's
/// `UIKeyCommand.inputDelete`'s value, not a name), or UIKit's names for the
/// arrows (`UIKeyCommand.inputDownArrow`'s, and so on). RunnerTests checks
/// them against UIKit's constants.
@visibleForTesting
String keyCommandInput(LogicalKeyboardKey key) => switch (key) {
  LogicalKeyboardKey.tab => '\t',
  LogicalKeyboardKey.enter => '\r',
  LogicalKeyboardKey.backspace => '\b',
  LogicalKeyboardKey.arrowUp => 'UIKeyInputUpArrow',
  LogicalKeyboardKey.arrowDown => 'UIKeyInputDownArrow',
  LogicalKeyboardKey.arrowLeft => 'UIKeyInputLeftArrow',
  LogicalKeyboardKey.arrowRight => 'UIKeyInputRightArrow',
  _ => key.keyLabel.toLowerCase(),
};

/// Binds [commands]' keys over [child], whether or not the system's menu
/// shows them too ([CommandMenu]); disabled ones are left unbound, so their
/// keys reach what's focused. Esc ([escape]) is never a menu item: it's always bound here.
/// [child] is a focus scope of its own, so a hardware keyboard's keys
/// reach these bindings even when nothing in it is focused.
class WorkbenchShortcuts extends StatefulWidget {
  const WorkbenchShortcuts({
    super.key,
    required this.commands,
    required this.child,
    this.escape,
  });

  final List<WorkbenchCommand> commands;
  final Widget child;

  /// What Esc does here, if anything: close what's open over the window.
  final VoidCallback? escape;

  @override
  State<WorkbenchShortcuts> createState() => _WorkbenchShortcutsState();
}

class _WorkbenchShortcutsState extends State<WorkbenchShortcuts> {
  final _scope = FocusScopeNode(debugLabel: 'workbench');

  @override
  void initState() {
    super.initState();
    FocusManager.instance.addListener(_keepKeys);
  }

  @override
  void dispose() {
    FocusManager.instance.removeListener(_keepKeys);
    _scope.dispose();
    super.dispose();
  }

  /// Where the keyboard goes when what had it lets go (the message box
  /// unfocused, a popover closed, a tap that dismissed the keyboard): this
  /// window, inside its bindings, never the view's root, which nothing
  /// here hears.
  void _keepKeys() {
    if (!_aboveUs(FocusManager.instance.primaryFocus)) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !_aboveUs(FocusManager.instance.primaryFocus)) return;
      if (_scope.canRequestFocus) _scope.requestFocus();
    });
  }

  /// Whether the keyboard is with nothing (no focus, or a scope this
  /// window is in: the view's, the app's).
  bool _aboveUs(FocusNode? primary) =>
      primary == null || _scope.ancestors.contains(primary);

  @override
  Widget build(BuildContext context) => CallbackShortcuts(
    bindings: {
      for (final command in widget.commands) command.keys: ?command.run,
      const SingleActivator(LogicalKeyboardKey.escape): ?widget.escape,
    },
    child: FocusScope(node: _scope, autofocus: true, child: widget.child),
  );
}
