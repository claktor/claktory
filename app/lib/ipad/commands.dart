import 'dart:convert';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../layout.dart';
import '../protocol/factory/v1/factory.pb.dart';
import 'columns.dart';
import 'document_host.dart' show DocumentPanel, documentKind;
import 'workbench.dart';

/// The workbench's keyboard commands: the sidebar, moving between panes and
/// Panes. Each has a title as the iPadOS menu and the ⌘-hold list show it,
/// keys, and what it does; one with nothing to do is disabled rather than
/// left out, so the list doesn't shift.
///
/// The keys are bound here, in Flutter. Showing them in the system's menu
/// (UIKeyCommand, the menu bar and the ⌘-hold list) is the native side's,
/// through the `factory/commands` channel (phase 1, with Tomás): once it
/// takes them, the native menu runs them and these bindings stand down, so
/// no key is handled twice.
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
      title: 'Find…',
      keys: const SingleActivator(LogicalKeyboardKey.keyF, meta: true),
      run: agent == null || document == null
          ? null
          : () => focus.documents
                .of(agent.id, document)
                .openPanel(DocumentPanel.search),
    ),
  ];
}

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
/// when it shows them as UIKeyCommands and runs them itself (calling back
/// `run` with the id), and from then on [native] is true and the Flutter
/// bindings stand down, so no key is handled twice. Where nothing answers
/// (the Mac, an iPhone, an older build), [native] stays false and Flutter
/// binds the keys.
class CommandMenu extends ChangeNotifier {
  CommandMenu({this.channel = const MethodChannel(channelName)}) {
    channel.setMethodCallHandler(_handle);
  }

  static const channelName = 'factory/commands';

  final MethodChannel channel;

  bool get native => _native;
  var _native = false;

  var _runs = <String, VoidCallback>{};
  String? _sent;
  var _disposed = false;

  /// Sends [commands] unless they're what was sent last.
  Future<void> publish(List<WorkbenchCommand> commands) async {
    _runs = {for (final command in commands) command.id: ?command.run};
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
    run();
    return true;
  }

  @override
  void dispose() {
    _disposed = true;
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
        'input': command.keys.trigger == LogicalKeyboardKey.tab
            ? '\t'
            : command.keys.trigger.keyLabel.toLowerCase(),
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

/// Binds [commands]' keys over [child], unless the system's menu runs them
/// ([native]); disabled ones are left unbound, so their keys reach what's
/// focused. Esc ([escape]) is never a menu item: it's always bound here.
/// [child] is a focus scope of its own, so a hardware keyboard's keys
/// reach these bindings even when nothing in it is focused.
class WorkbenchShortcuts extends StatefulWidget {
  const WorkbenchShortcuts({
    super.key,
    required this.commands,
    required this.child,
    this.escape,
    this.native = false,
  });

  final List<WorkbenchCommand> commands;
  final Widget child;

  /// What Esc does here, if anything: close what's open over the window.
  final VoidCallback? escape;

  /// Whether the system's menu handles [commands]' keys.
  final bool native;

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
      if (!widget.native)
        for (final command in widget.commands) command.keys: ?command.run,
      const SingleActivator(LogicalKeyboardKey.escape): ?widget.escape,
    },
    child: FocusScope(node: _scope, autofocus: true, child: widget.child),
  );
}
