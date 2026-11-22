import 'dart:async';
import 'dart:convert';
import 'dart:developer' as developer;

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'claktory_identity.dart';
import 'connection.dart';
import 'connections.dart';
import 'controls.dart';
import 'devices.dart';
import 'drafts.dart';
import 'forms.dart';
import 'history.dart';
import 'identity.dart';
import 'login.dart';
import 'mobile/drawer.dart' show PhoneDrawerHost;
import 'mobile/haptics.dart';
import 'mobile/input.dart' show InputKind;
import 'mobile/metrics.dart'
    show DenseList, DenseType, PhoneType, Touch, TypeScale;
import 'mobile/platform_policy.dart';
import 'phone.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'selection.dart';
import 'usage_wait.dart' show atWork;
import 'inbox.dart' show needsYouCount;
import 'waiting.dart' show waitingLabel;

/// The rail of Claktories at the window's far left, as thin as the
/// extension bar.
const railWidth = 40.0;

/// One Claktory this app is connected to: this Mac's, or one on another
/// machine. Each has its own live connection and everything the window
/// shows of it, so switching between them is instant and nothing of one
/// ever shows as another's.
class Claktory {
  Claktory({required this.target, required this.backend, required this.drafts})
    : key = target?.code ?? '' {
    drafts.watch(backend.workspace);
  }

  /// Its pairing code; '' for this device's own.
  final String key;

  /// The Claktory on another machine; null for this Mac's (on a phone:
  /// none yet).
  final RemoteFactory? target;

  final Backend backend;

  /// The unsent drafts in its agents' message boxes.
  final Drafts drafts;

  late final login = LoginLauncher(
    workspace: backend.workspace,
    send: backend.startLogin,
    // On another machine, its sign-ins come back there through this one.
    forwards: backend.forwards,
  );
  late final history = AgentHistory(
    workspace: backend.workspace,
    select: (id) => fireAndLog(backend.selectAgent(id)),
  );

  /// What its window shows: an agent, Settings… kept while another shows.
  final page = ValueNotifier(AppPage.agent);

  Future<void> close() async {
    login.dispose();
    history.dispose();
    page.dispose();
    await backend.close();
  }
}

/// Every Claktory the app is connected to, all live at once, kept in step
/// with [connections]: a Claktory paired there is connected here, and one
/// forgotten there is closed. Which one the window shows is
/// [Connections.current].
class Claktories extends ChangeNotifier {
  Claktories({
    required this.connections,
    required bool localBackend,
    required this.open,
    required this.drafts,
    this.keepLocal,
  }) // Mutable, behind a getter: setLocalBackend changes it.
    // ignore: prefer_initializing_formals
    : _localBackend = localBackend {
    connections.addListener(_sync);
    _sync();
    // iOS suspends the app in the background, and its connections die
    // without a word: back in front, each reconnects at once.
    final suspends = switch (defaultTargetPlatform) {
      TargetPlatform.iOS || TargetPlatform.android => true,
      _ => false,
    };
    // A Mac's app keeps running behind other windows, but its connections
    // can die meanwhile (the network changed, the other machine slept):
    // back in front, each checks its connection is alive.
    _lifecycle = suspends
        ? AppLifecycleListener(
            onHide: () {
              for (final claktory in _open.values) {
                claktory.backend.hidden();
              }
            },
            onShow: () {
              for (final claktory in _open.values) {
                claktory.backend.resumed();
              }
            },
          )
        : AppLifecycleListener(
            onResume: () {
              for (final claktory in _open.values) {
                claktory.backend.check();
              }
            },
            // The screen unlocked (a Mac reports it as shown): checked
            // before the first frame after, so a connection that died
            // asleep doesn't show as it was.
            onShow: () {
              for (final claktory in _open.values) {
                claktory.backend.check();
              }
            },
          );
  }

  late final AppLifecycleListener _lifecycle;

  final Connections connections;

  /// Whether this device runs a Claktory of its own: a Mac does, unless
  /// it uses only Claktories on other computers ("app only").
  bool get localBackend => _localBackend;
  bool _localBackend;

  /// Keeps on this device whether it runs a Claktory of its own
  /// (Setup.swift's ClaktoryAppOnly); null where that can't change.
  final Future<void> Function(bool local)? keepLocal;

  /// Whether this Mac's own Claktory can be set up or given up here.
  bool get canChooseLocal => keepLocal != null;

  /// This Mac runs a Claktory of its own from now on, or doesn't: its
  /// Claktory starts again as one or the other, and the choice is kept.
  void setLocalBackend(bool local) {
    if (local == _localBackend) return;
    _localBackend = local;
    unawaited(keepLocal?.call(local));
    final old = _open.remove('');
    if (old != null) unawaited(old.close());
    if (local && connections.current != null) {
      // Its own shows: the one it chose.
      connections.use(null);
    }
    _sync();
  }

  /// Connects to a Claktory (null: this device's own), live from now on.
  final Backend Function(RemoteFactory? target) open;

  final DeviceDrafts drafts;

  final _open = <String, Claktory>{};

  /// The Claktories in the rail: this Mac's first, then the others in the
  /// order they were added.
  List<Claktory> get all => [
    if (localBackend) _open['']!,
    for (final target in connections.saved) _open[target.code]!,
  ];

  /// The one the window shows. On a phone with nothing paired, an empty
  /// one that waits to be paired.
  Claktory get selected => _open[connections.current?.code ?? ''] ?? _open['']!;

  /// Shows [claktory] in the window.
  void select(Claktory claktory) => connections.use(claktory.target);

  /// Whether [_sync] is running, and whether [connections] changed
  /// meanwhile.
  var _syncing = false;
  var _syncAgain = false;

  /// Opens a Claktory for each one in [connections] and closes the rest.
  /// Opening one can change [connections] (its first connection makes this
  /// device's key for it, and keeps it), which would run this again before
  /// the first one is in [_open]: it opens a second, live connection to the
  /// same Claktory as the same device, never closed, which then fights the
  /// first one for its relay. So a change while it runs is taken after it.
  void _sync() {
    if (_syncing) {
      _syncAgain = true;
      return;
    }
    _syncing = true;
    try {
      do {
        _syncAgain = false;
        _syncOnce();
      } while (_syncAgain);
    } finally {
      _syncing = false;
    }
  }

  void _syncOnce() {
    // On a phone, after the one showing was removed, another shows.
    final saved = connections.saved;
    if (!localBackend && connections.current == null && saved.isNotEmpty) {
      connections.use(saved.first);
      return;
    }
    final wanted = <String, RemoteFactory?>{
      '': null,
      for (final target in saved) target.code: target,
    };
    for (final key in _open.keys.toList()) {
      if (!wanted.containsKey(key)) unawaited(_open.remove(key)!.close());
    }
    for (final MapEntry(:key, value: target) in wanted.entries) {
      final open = _open[key];
      if (open == null) {
        _open[key] = Claktory(
          target: target,
          backend: this.open(target),
          drafts: drafts.of(key),
        );
      }
    }
    notifyListeners();
  }

  @override
  void dispose() {
    _lifecycle.dispose();
    connections.removeListener(_sync);
    for (final claktory in _open.values) {
      unawaited(claktory.close());
    }
    _open.clear();
    super.dispose();
  }
}

/// The drafts this device keeps, for every Claktory at once: this Mac's
/// agents' by agent id, as always, and another Claktory's under a prefix
/// of its own, since two Claktories can have agents with the same id.
class DeviceDrafts {
  DeviceDrafts(this._device, [Map<String, String> kept = const {}])
    : _all = {...kept};

  /// What the device kept; nothing if it can't be read.
  static Future<DeviceDrafts> load(DraftStorage device) async {
    try {
      return DeviceDrafts(device, await device.read());
    } catch (error) {
      developer.log('could not read drafts: $error', name: 'factory.drafts');
      return DeviceDrafts(device);
    }
  }

  final DraftStorage _device;
  final Map<String, String> _all;

  static const _marker = 'claktory:';

  /// The prefix of the drafts for the Claktory with pairing code [key]
  /// ('' for this device's own, whose drafts have none).
  static String _prefix(String key) => key.isEmpty
      ? ''
      : '$_marker${sha256.convert(utf8.encode(key)).toString().substring(0, 16)}:';

  static bool _belongs(String draftKey, String prefix) => prefix.isEmpty
      ? !draftKey.startsWith(_marker)
      : draftKey.startsWith(prefix);

  /// The drafts of the Claktory with pairing code [key].
  Drafts of(String key) {
    final prefix = _prefix(key);
    return Drafts(_ClaktoryDrafts(this, prefix), {
      for (final MapEntry(key: draftKey, :value) in _all.entries)
        if (_belongs(draftKey, prefix))
          draftKey.substring(prefix.length): value,
    });
  }

  Future<void> _write(String prefix, Map<String, String> drafts) {
    _all.removeWhere((draftKey, _) => _belongs(draftKey, prefix));
    for (final MapEntry(:key, :value) in drafts.entries) {
      _all['$prefix$key'] = value;
    }
    return _device.write(Map.of(_all));
  }
}

class _ClaktoryDrafts implements DraftStorage {
  _ClaktoryDrafts(this._drafts, this._prefix);

  final DeviceDrafts _drafts;
  final String _prefix;

  @override
  Future<Map<String, String>> read() async => {
    for (final MapEntry(:key, :value) in _drafts._all.entries)
      if (DeviceDrafts._belongs(key, _prefix))
        key.substring(_prefix.length): value,
  };

  @override
  Future<void> write(Map<String, String> drafts) =>
      _drafts._write(_prefix, drafts);
}

/// The Claktories, for the rail and the phone's list.
class ClaktoriesScope extends InheritedNotifier<Claktories> {
  const ClaktoriesScope({
    super.key,
    required Claktories claktories,
    required super.child,
  }) : super(notifier: claktories);

  /// Null in a window with no Claktories around it (tests of one view).
  static Claktories? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<ClaktoriesScope>()?.notifier;
}

/// How a Claktory is doing, at a glance: connected or not, whether one of
/// its agents works, and whether one has news.
class _Glance {
  _Glance(Claktory claktory) {
    final backend = claktory.backend;
    final workspace = backend.workspace.workspace;
    // Archived projects' agents don't run, and their news waits.
    final agents = [
      for (final agent in workspace.agents)
        if (archivedProjectOf(workspace, agent) == null) agent,
    ];
    connected = backend.connected.value;
    working = connected && agents.any(atWork);
    unseen = agents.any((agent) => agent.unseen);
    waiting = needsYouCount(workspace);
    connection = Connection(backend);
    problem = connection.problem ? connection.line : null;
  }

  late final bool connected;
  late final bool working;
  late final bool unseen;

  /// What waits for the person's answer there, everything Needs you counts:
  /// agents', chats' and pipes'.
  late final int waiting;

  /// How it's connected.
  late final Connection connection;

  /// Why it can't connect; null while it can (or is trying).
  late final String? problem;

  /// Opening, or coming back: its tile's mark pulses.
  bool get connecting => connection.link == Link.connecting;

  /// What a Claktory's glance depends on.
  static Listenable of(Claktory claktory) => Listenable.merge([
    claktory.backend.workspace,
    Connection.of(claktory.backend),
  ]);
}

/// Up to two letters for [name]: "mac-mini" is "MM", "nixos" "N".
String claktoryInitials(String name) {
  final words = name
      .split(RegExp(r'[\s\-_.]+'))
      .where((word) => word.isNotEmpty)
      .toList();
  if (words.isEmpty) return '?';
  return [for (final word in words.take(2)) word.characters.first.toUpperCase()]
      .join();
}

/// The Claktories at the window's far left, like a chat app's servers: a
/// tile for each, the one showing tinted with a signal mark at the edge,
/// and "+" at the bottom to add one. A click shows it; a right click
/// renames or removes it.
class ClaktoryRail extends StatelessWidget {
  const ClaktoryRail({super.key});

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final claktories = ClaktoriesScope.of(context);
    if (claktories == null) return const SizedBox.shrink();
    final selected = claktories.selected;
    return Unselectable(
      child: Container(
        width: railWidth,
        color: palette.sidebar,
        child: Column(
          children: [
            // Where the traffic lights sit, over the rail and the sidebar
            // as one band, so no line runs through them.
            const TitleSpace(),
            Expanded(
              child: DecoratedBox(
                decoration: BoxDecoration(
                  border: Border(right: BorderSide(color: palette.hairline)),
                ),
                child: Column(
                  children: [
                    Expanded(
                      child: SingleChildScrollView(
                        child: Column(
                          children: [
                            for (final claktory in claktories.all)
                              Padding(
                                padding: const EdgeInsets.only(
                                  bottom: Space.xs,
                                ),
                                child: _RailTile(
                                  key: ValueKey(claktory.key),
                                  claktory: claktory,
                                  selected: claktory == selected,
                                  onTap: () => claktories.select(claktory),
                                ),
                              ),
                          ],
                        ),
                      ),
                    ),
                    Padding(
                      padding: const EdgeInsets.only(bottom: Space.s),
                      child: HoverCard(
                        beside: true,
                        card: (context) => const Text('Add a Claktory'),
                        child: GlyphButton(
                          label: 'Add a Claktory',
                          active: false,
                          onTap: () => showModal(
                            context,
                            (context, close) => FactoryChooser(close: close),
                          ),
                          glyph: (color) => ExtensionGlyph(
                            icon: 'plus',
                            name: '+',
                            color: color,
                          ),
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// One Claktory in the rail: a 28px tile with this Mac's glyph or the
/// other's initials, the working wave while one of its agents works, and a
/// dot while one has news (in `danger` while it can't connect).
class _RailTile extends StatelessWidget {
  const _RailTile({
    super.key,
    required this.claktory,
    required this.selected,
    required this.onTap,
  });

  final Claktory claktory;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ListenableBuilder(
      listenable: _Glance.of(claktory),
      builder: (context, _) {
        final glance = _Glance(claktory);
        final name = claktoryName(claktory.backend);
        return SizedBox(
          width: railWidth,
          height: 28,
          child: Stack(
            clipBehavior: Clip.none,
            alignment: Alignment.center,
            children: [
              // The one showing: a signal mark at the window's edge.
              Positioned(
                left: 0,
                top: 6,
                bottom: 6,
                child: AnimatedOpacity(
                  duration: Motion.fast,
                  opacity: selected ? 1 : 0,
                  child: Container(
                    width: 2,
                    decoration: BoxDecoration(
                      color: palette.signal,
                      borderRadius: const BorderRadius.horizontal(
                        right: Radius.circular(1),
                      ),
                    ),
                  ),
                ),
              ),
              HoverCard(
                beside: true,
                card: (context) => Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      name,
                      style: TextStyles.label.copyWith(color: palette.text),
                    ),
                    Text(
                      glance.connection.line,
                      style: TextStyles.caption.copyWith(
                        color: glance.problem != null
                            ? palette.danger
                            : palette.textMuted,
                      ),
                    ),
                    Text(
                      glance.connection.details,
                      style: TextStyles.caption.copyWith(
                        color: palette.textFaint,
                      ),
                    ),
                  ],
                ),
                child: Semantics(
                  button: true,
                  selected: selected,
                  label: name,
                  child: Pressable(
                    onTap: selected ? null : onTap,
                    onSecondaryTap: () => showModal(
                      context,
                      (context, close) =>
                          ClaktoryForm(claktory: claktory, close: close),
                    ),
                    builder: (context, hovered) => _Tile(
                      claktory: claktory,
                      name: name,
                      glance: glance,
                      selected: selected,
                      hovered: hovered,
                    ),
                  ),
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

/// A Claktory's 28px tile: on its own in the rail, and leading its row in
/// the phone's list. On a wash of its color, its glyph or initials in it.
class _Tile extends StatelessWidget {
  const _Tile({
    required this.claktory,
    required this.name,
    required this.glance,
    required this.selected,
    required this.hovered,
    this.size = 28,
  });

  final Claktory claktory;
  final String name;
  final _Glance glance;
  final bool selected;
  final bool hovered;

  /// 28px in the rail and the list; larger in a phone's drawer.
  final double size;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final own = claktoryColor(claktory.backend);
    final color = !glance.connected
        ? palette.textFaint
        : own ?? (selected || hovered ? palette.text : palette.textMuted);
    return SizedBox.square(
      dimension: size,
      child: Stack(
        clipBehavior: Clip.none,
        children: [
          Positioned.fill(
            child: AnimatedContainer(
              duration: Motion.fast,
              decoration: BoxDecoration(
                color: own != null
                    ? own.withValues(
                        alpha: selected
                            ? 0.24
                            : hovered
                            ? 0.2
                            : 0.14,
                      )
                    : selected
                    ? palette.selected
                    : hovered
                    ? palette.hover
                    : null,
                borderRadius: BorderRadius.circular(Radii.control),
                border: Border.all(
                  color: selected || own != null
                      ? const Color(0x00000000)
                      : palette.hairline,
                ),
              ),
            ),
          ),
          if (glance.working)
            Positioned.fill(
              child: WorkingWave(
                color: palette.text,
                peak: 0.14,
                radius: Radii.control,
              ),
            ),
          Center(
            child: claktory.key.isEmpty
                ? CustomPaint(
                    size: Size.square(16 * size / 28),
                    painter: _LaptopPainter(color),
                  )
                : Text(
                    claktoryInitials(name),
                    style: TextStyles.caption.copyWith(
                      color: color,
                      fontWeight: FontWeight.w600,
                      height: 1,
                      fontSize: size == 28
                          ? null
                          : (TextStyles.caption.fontSize ?? 11) * size / 28,
                    ),
                  ),
          ),
          // Its connection first (can't, or connecting), then what waits
          // for the person, then its news.
          if (glance.problem != null ||
              glance.connecting ||
              glance.waiting > 0 ||
              glance.unseen)
            Positioned(
              right: -3,
              top: -3,
              child: Semantics(
                label:
                    glance.problem ??
                    (glance.connecting
                        ? glance.connection.line
                        : glance.waiting > 0
                        ? waitingLabel(name, glance.waiting)
                        : '$name has something new'),
                child: Container(
                  width: 11,
                  height: 11,
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: palette.sidebar,
                    shape: BoxShape.circle,
                  ),
                  child: glance.problem == null && glance.connecting
                      ? const LiveDot()
                      : Dot(
                          color: glance.problem != null
                              ? palette.danger
                              : glance.waiting > 0
                              ? palette.signal
                              : palette.text,
                          size: 7,
                        ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// Lucide's laptop-minimal at 16px: this Mac's own Claktory.
class _LaptopPainter extends CustomPainter {
  const _LaptopPainter(this.color);

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
    canvas.drawRRect(
      RRect.fromRectAndRadius(
        const Rect.fromLTWH(4, 4, 16, 12),
        const Radius.circular(2),
      ),
      paint,
    );
    canvas.drawLine(const Offset(2, 20), const Offset(22, 20), paint);
  }

  @override
  bool shouldRepaint(_LaptopPainter old) => old.color != color;
}

/// A Claktory's name and color, the same on every device, and removing
/// it from this one.
class ClaktoryForm extends StatefulWidget {
  const ClaktoryForm({super.key, required this.claktory, required this.close});

  final Claktory claktory;
  final VoidCallback close;

  @override
  State<ClaktoryForm> createState() => _ClaktoryFormState();
}

class _ClaktoryFormState extends State<ClaktoryForm> {
  late final Connections _connections = AppScope.scope(context).connections;
  Backend get _backend => widget.claktory.backend;
  late final _name = TextEditingController(
    text: _backend.workspace.workspace.settings.name,
  );
  late var _color = claktoryColorName(_backend);
  var _confirmingRemove = false;
  var _saving = false;
  String? _error;

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    final settings = _backend.workspace.workspace.settings;
    final name = _name.text.trim();
    final update = UpdateSettings(
      name: name == settings.name ? null : name,
      color: _color == settings.color ? null : _color,
    );
    if (!update.hasName() && !update.hasColor()) return widget.close();
    setState(() {
      _saving = true;
      _error = null;
    });
    try {
      await _backend.updateSettings(update);
      if (mounted) widget.close();
    } on RequestFailure catch (error) {
      if (mounted) {
        setState(() {
          _saving = false;
          _error = error.message;
        });
      }
    }
  }

  void _remove(RemoteFactory target) {
    // It forgets this device's key, so its pairing is of no more use:
    // leave its devices too, when it can be reached.
    final backend = widget.claktory.backend;
    if (backend.connected.value && backend.deviceId.isNotEmpty) {
      fireAndLog(backend.unpairDevice(backend.deviceId));
    }
    _connections.forget(target);
    widget.close();
  }

  @override
  Widget build(BuildContext context) {
    final target = widget.claktory.target;
    final name = claktoryName(_backend);
    if (_confirmingRemove && target != null) {
      return FormFrame(
        title: 'Remove $name?',
        body:
            'This $thisDeviceKind disconnects from it. Its agents and work '
            'stay on $name. To connect again, add this '
            '$thisDeviceKind on $name in Settings → Devices.',
        fields: const [],
        error: null,
        submitLabel: 'Remove',
        onSubmit: () => _remove(target),
        onCancel: () => setState(() => _confirmingRemove = false),
      );
    }
    final remove = target == null
        ? null
        : Button(
            label: 'Remove…',
            onPressed: () => setState(() => _confirmingRemove = true),
          );
    // Its name and color are its own: they change only while connected.
    if (!_backend.connected.value) {
      return FormFrame(
        title: name,
        body: 'Connect to $name to change its name or color.',
        fields: const [],
        error: null,
        submitLabel: 'Done',
        onSubmit: widget.close,
        onCancel: widget.close,
        secondary: remove,
      );
    }
    final machine = _backend.workspace.workspace.remote.machine;
    return FormFrame(
      title: name,
      fields: [
        Field(
          label: 'Name',
          note: 'Shown on every device connected to it.',
          child: TextField(
            controller: _name,
            placeholder: machine,
            kind: InputKind.text,
            // Not over a phone's form: the name is there already.
            autofocus: !touchFirst(context),
            onSubmitted: (_) => _save(),
          ),
        ),
        Field(
          label: 'Color',
          child: ColorPicker(
            value: _color,
            onChanged: (color) => setState(() => _color = color),
          ),
        ),
      ],
      error: _error,
      submitLabel: 'Save',
      onSubmit: _saving ? null : _save,
      onCancel: widget.close,
      // On a phone, FormFrame puts it on a line of its own.
      secondary: remove,
    );
  }
}

/// On a phone, the way back from a Claktory to the list of them: lets
/// [ClaktoriesBackRow] find it.
class _BackToList extends InheritedWidget {
  const _BackToList({super.key, required this.back, required super.child});

  final VoidCallback back;

  @override
  bool updateShouldNotify(_BackToList old) => old.back != back;
}

/// On a phone, [ClaktoriesBackRow] along the top of the screen, in the
/// sidebar's color, over a step that covers the rest (the name step, say),
/// so another Claktory is always in reach. Nothing where there's no list to
/// go back to.
class ClaktoriesBackStrip extends StatelessWidget {
  const ClaktoriesBackStrip({super.key});

  @override
  Widget build(BuildContext context) {
    if (context.dependOnInheritedWidgetOfExactType<_BackToList>() == null ||
        ClaktoriesScope.of(context) == null) {
      return const SizedBox.shrink();
    }
    final palette = Palette.of(context);
    return DecoratedBox(
      decoration: BoxDecoration(
        color: palette.sidebar,
        border: Border(bottom: BorderSide(color: palette.hairline)),
      ),
      child: const SafeArea(bottom: false, child: ClaktoriesBackRow()),
    );
  }
}

/// On a phone, a 44pt "‹ Claktories" row at the top of a Claktory's
/// sidebar screen, back to the list of them, with a dot while another one
/// has news. Nothing anywhere else.
class ClaktoriesBackRow extends StatelessWidget {
  const ClaktoriesBackRow({super.key});

  @override
  Widget build(BuildContext context) {
    final back = context.dependOnInheritedWidgetOfExactType<_BackToList>();
    final claktories = ClaktoriesScope.of(context);
    if (back == null || claktories == null) return const SizedBox.shrink();
    final others = [
      for (final claktory in claktories.all)
        if (claktory != claktories.selected) claktory,
    ];
    final shown = claktories.selected;
    return ListenableBuilder(
      listenable: Listenable.merge([
        for (final other in others) _Glance.of(other),
        Connection.of(shown.backend),
      ]),
      builder: (context, _) => _BackRow(
        label: 'Claktories',
        news: others.any((other) => _Glance(other).unseen),
        waiting: others.any((other) => _Glance(other).waiting > 0),
        onTap: back.back,
        // How the one showing is connected, at the top of its agents.
        trailing: PhoneConnectionWord(shown.backend),
      ),
    );
  }
}

class _BackRow extends StatelessWidget {
  const _BackRow({
    required this.label,
    required this.onTap,
    this.news = false,
    this.waiting = false,
    this.trailing,
  });

  final String label;
  final VoidCallback onTap;
  final bool news;

  /// Something waits for the person's answer there: a `signal` dot, in
  /// place of the news dot.
  final bool waiting;

  /// At its end, such as [PhoneConnectionWord].
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Unselectable(
      child: Semantics(
        button: true,
        label: 'Back to $label',
        child: Pressable(
          onTap: onTap,
          // The whole row takes the tap, not only its words.
          builder: (context, hovered) => Container(
            color: const Color(0x00000000),
            // A finger's height, taller as the text grows.
            constraints: const BoxConstraints(minHeight: 44),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: Space.s),
              child: Row(
                children: [
                  ExtensionGlyph(
                    icon: 'chevron-left',
                    name: '‹',
                    color: hovered ? palette.text : palette.textMuted,
                  ),
                  const SizedBox(width: Space.xs),
                  Flexible(
                    child: Text(
                      label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: PhoneType.label.copyWith(
                        color: hovered ? palette.text : palette.textMuted,
                      ),
                    ),
                  ),
                  if (waiting || news) ...[
                    const SizedBox(width: Space.s),
                    Dot(
                      color: waiting ? palette.signal : palette.text,
                      size: 7,
                    ),
                  ],
                  if (trailing case final trailing?)
                    Expanded(
                      child: Align(
                        alignment: AlignmentDirectional.centerEnd,
                        child: trailing,
                      ),
                    ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// On a phone, the connection of the Claktory showing, in a word with its
/// mark ("Connected", "Reconnecting…"); a tap opens [ConnectionDialog].
class PhoneConnectionWord extends StatelessWidget {
  const PhoneConnectionWord(this.backend, {super.key});

  final Backend backend;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final connection = Connection(backend);
    return Semantics(
      button: true,
      label: connection.line,
      child: Pressable(
        onTap: () => showModal(
          context,
          (context, close) => ConnectionDialog(backend: backend, close: close),
        ),
        builder: (context, hovered) => ExcludeSemantics(
          child: ConstrainedBox(
            // A finger's height, however short the word; a row's in the
            // drawer.
            constraints: BoxConstraints(
              minHeight: PhonePlatformPolicy.least(
                context,
                DenseList.of(context) ? Touch.denseLine : Touch.target,
              ),
            ),
            child: Align(
              alignment: AlignmentDirectional.centerStart,
              widthFactor: 1,
              child: Padding(
                padding: const EdgeInsets.symmetric(horizontal: Space.xs),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    ConnectionDot(link: connection.link),
                    const SizedBox(width: Space.xs + 2),
                    Flexible(
                      child: Text(
                        connection.short,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TypeScale.of(context).caption.copyWith(
                          color: connection.problem
                              ? palette.danger
                              : palette.textFaint,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// What a phone shows: the Claktory chosen, with a way back to the list of
/// them; the list, to choose another or add one; or pairing a new one.
class PhoneClaktories extends StatefulWidget {
  const PhoneClaktories({super.key, required this.shell});

  /// The chosen Claktory's window.
  final Widget shell;

  @override
  State<PhoneClaktories> createState() => _PhoneClaktoriesState();
}

enum _PhoneScreen { claktory, list, adding }

class _PhoneClaktoriesState extends State<PhoneClaktories> {
  var _screen = _PhoneScreen.claktory;
  Claktory? _shown;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // A new one paired: it shows. (One removed from the list leaves the
    // list showing, with the next one chosen.)
    final selected = ClaktoriesScope.of(context)?.selected;
    if (_shown != null &&
        selected != _shown &&
        _screen == _PhoneScreen.adding) {
      _screen = _PhoneScreen.claktory;
    }
    _shown = selected;
  }

  void _go(_PhoneScreen screen) => setState(() => _screen = screen);

  @override
  Widget build(BuildContext context) {
    final claktories = ClaktoriesScope.of(context);
    // Nothing paired yet: the window pairs the first.
    if (claktories == null || claktories.all.isEmpty) return widget.shell;
    final Widget child;
    switch (_screen) {
      case _PhoneScreen.claktory:
        // The drawer sits over the Claktory's window, so it stays open as
        // another takes its place.
        child = _BackToList(
          key: const ValueKey('claktory'),
          back: () => _go(_PhoneScreen.list),
          child: PhoneDrawerHost(
            rail: PhoneClaktoryRail(onAdd: () => _go(_PhoneScreen.adding)),
            onClaktories: () => _go(_PhoneScreen.list),
            child: widget.shell,
          ),
        );
      case _PhoneScreen.list:
        child = _PhoneScreenFrame(
          key: const ValueKey('list'),
          child: ClaktoryList(
            onChoose: (claktory) {
              claktories.select(claktory);
              _go(_PhoneScreen.claktory);
            },
            onAdd: () => _go(_PhoneScreen.adding),
          ),
        );
      case _PhoneScreen.adding:
        child = _PhoneScreenFrame(
          key: const ValueKey('adding'),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              _BackRow(
                label: 'Claktories',
                onTap: () => _go(_PhoneScreen.list),
              ),
              const Expanded(child: PairScreen()),
            ],
          ),
        );
    }
    return AnimatedSwitcher(
      duration: Motion.base,
      switchInCurve: Curves.easeOut,
      switchOutCurve: Curves.easeIn,
      child: child,
    );
  }
}

/// The drawer's rail: a row for each Claktory, its tile in the middle.
const _railRow = 46.0;
const _railTile = 36.0;

/// The Claktories at the drawer's leading edge on a phone: a 36pt tile for
/// each in a 46pt row, the one showing marked at the edge, then "+" to add
/// one. A tap shows another (the drawer stays open, to choose an agent in
/// it); a touch and hold renames or removes one.
class PhoneClaktoryRail extends StatelessWidget {
  const PhoneClaktoryRail({super.key, required this.onAdd});

  final VoidCallback onAdd;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final claktories = ClaktoriesScope.of(context);
    if (claktories == null) return const SizedBox.shrink();
    final selected = claktories.selected;
    return Unselectable(
      child: ListView(
        padding: const EdgeInsets.symmetric(vertical: Space.s),
        children: [
          for (final claktory in claktories.all)
            _PhoneRailTile(
              key: ValueKey(claktory.key),
              claktory: claktory,
              selected: claktory == selected,
              onTap: () {
                if (claktory == selected) return;
                unawaited(Haptics.selection());
                claktories.select(claktory);
              },
            ),
          Semantics(
            button: true,
            label: 'Add a Claktory',
            excludeSemantics: true,
            child: Pressable(
              onTap: onAdd,
              builder: (context, hovered) => SizedBox(
                height: _railRow,
                child: Center(
                  child: Container(
                    width: _railTile,
                    height: _railTile,
                    alignment: Alignment.center,
                    decoration: BoxDecoration(
                      color: hovered ? palette.hover : null,
                      borderRadius: BorderRadius.circular(Radii.control),
                      border: Border.all(color: palette.hairline),
                    ),
                    child: ExtensionGlyph(
                      icon: 'plus',
                      name: '+',
                      color: palette.textMuted,
                    ),
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

class _PhoneRailTile extends StatelessWidget {
  const _PhoneRailTile({
    super.key,
    required this.claktory,
    required this.selected,
    required this.onTap,
  });

  final Claktory claktory;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ListenableBuilder(
      listenable: _Glance.of(claktory),
      builder: (context, _) {
        final glance = _Glance(claktory);
        final name = claktoryName(claktory.backend);
        return Semantics(
          button: true,
          selected: selected,
          label: name,
          child: GestureDetector(
            onLongPress: () => showModal(
              context,
              (context, close) =>
                  ClaktoryForm(claktory: claktory, close: close),
            ),
            child: Pressable(
              onTap: onTap,
              builder: (context, hovered) => SizedBox(
                height: _railRow,
                child: Stack(
                  alignment: Alignment.center,
                  children: [
                    // The one showing: a signal mark at the edge.
                    PositionedDirectional(
                      start: 0,
                      top: 10,
                      bottom: 10,
                      child: AnimatedOpacity(
                        duration: Motion.fast,
                        opacity: selected ? 1 : 0,
                        child: Container(
                          width: 3,
                          decoration: BoxDecoration(
                            color: palette.signal,
                            borderRadius:
                                const BorderRadiusDirectional.horizontal(
                                  end: Radius.circular(2),
                                ),
                          ),
                        ),
                      ),
                    ),
                    _Tile(
                      claktory: claktory,
                      name: name,
                      glance: glance,
                      selected: selected,
                      hovered: hovered,
                      size: _railTile,
                    ),
                  ],
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

/// A full phone screen of the app's own, in the sidebar's color: under the
/// status bar, above the home indicator or the keyboard, and beside the
/// notch when the phone is on its side.
class _PhoneScreenFrame extends StatelessWidget {
  const _PhoneScreenFrame({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final safe = MediaQuery.paddingOf(context);
    final keyboard = MediaQuery.viewInsetsOf(context).bottom;
    return ColoredBox(
      color: palette.sidebar,
      child: Padding(
        // Turned on its side, clear of the notch too.
        padding: EdgeInsets.only(
          left: safe.left,
          top: safe.top,
          right: safe.right,
          bottom: keyboard > safe.bottom ? keyboard : safe.bottom,
        ),
        child: MediaQuery.removePadding(
          context: context,
          removeLeft: true,
          removeTop: true,
          removeRight: true,
          removeBottom: true,
          child: DefaultTextStyle(
            style: TextStyles.body.copyWith(color: palette.text),
            child: child,
          ),
        ),
      ),
    );
  }
}

/// The phone's list of Claktories: a 44pt row for each (its tile, its
/// name and how it's doing), the one showing tinted, and "Add a Claktory".
/// A long press renames or removes one.
class ClaktoryList extends StatelessWidget {
  const ClaktoryList({super.key, required this.onChoose, required this.onAdd});

  final ValueChanged<Claktory> onChoose;
  final VoidCallback onAdd;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final claktories = ClaktoriesScope.of(context)!;
    final selected = claktories.selected;
    return Unselectable(
      child: ListView(
        padding: const EdgeInsets.all(Space.s),
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(
              Space.s,
              Space.l,
              Space.s,
              Space.l,
            ),
            child: Text(
              'Claktories',
              style: DenseType.title.copyWith(color: palette.text),
            ),
          ),
          for (final claktory in claktories.all)
            _ListRow(
              key: ValueKey(claktory.key),
              claktory: claktory,
              selected: claktory == selected,
              onTap: () => onChoose(claktory),
            ),
          Semantics(
            button: true,
            label: 'Add a Claktory',
            child: Pressable(
              onTap: onAdd,
              builder: (context, hovered) => Container(
                constraints: BoxConstraints(
                  minHeight: PhonePlatformPolicy.least(context, Touch.denseRow),
                ),
                padding: const EdgeInsets.symmetric(
                  horizontal: Space.s,
                  vertical: Space.xs,
                ),
                decoration: BoxDecoration(
                  color: hovered ? palette.hover : null,
                  borderRadius: BorderRadius.circular(Radii.control),
                ),
                child: Row(
                  children: [
                    SizedBox.square(
                      dimension: 28,
                      child: Center(
                        child: ExtensionGlyph(
                          icon: 'plus',
                          name: '+',
                          color: palette.textMuted,
                        ),
                      ),
                    ),
                    const SizedBox(width: Space.m),
                    Expanded(
                      child: Text(
                        'Add a Claktory',
                        style: DenseType.body.copyWith(
                          color: palette.textMuted,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _ListRow extends StatelessWidget {
  const _ListRow({
    super.key,
    required this.claktory,
    required this.selected,
    required this.onTap,
  });

  final Claktory claktory;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return ListenableBuilder(
      listenable: _Glance.of(claktory),
      builder: (context, _) {
        final glance = _Glance(claktory);
        final name = claktoryName(claktory.backend);
        return Semantics(
          button: true,
          selected: selected,
          label: name,
          child: GestureDetector(
            onLongPress: () => showModal(
              context,
              (context, close) =>
                  ClaktoryForm(claktory: claktory, close: close),
            ),
            child: Pressable(
              onTap: onTap,
              builder: (context, hovered) => Container(
                constraints: BoxConstraints(
                  minHeight: PhonePlatformPolicy.least(context, Touch.denseRow),
                ),
                padding: const EdgeInsets.symmetric(
                  horizontal: Space.s,
                  vertical: Space.xs,
                ),
                decoration: BoxDecoration(
                  color: selected
                      ? palette.selected
                      : hovered
                      ? palette.hover
                      : null,
                  borderRadius: BorderRadius.circular(Radii.control),
                ),
                child: Row(
                  children: [
                    _Tile(
                      claktory: claktory,
                      name: name,
                      glance: _Glance(claktory),
                      selected: selected,
                      hovered: hovered,
                    ),
                    const SizedBox(width: Space.m),
                    Expanded(
                      child: Column(
                        mainAxisAlignment: MainAxisAlignment.center,
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            name,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: DenseType.body.copyWith(
                              color: palette.text,
                              fontWeight: glance.unseen
                                  ? FontWeight.w600
                                  : FontWeight.w500,
                            ),
                          ),
                          Text(
                            glance.connection.link == Link.live &&
                                    glance.working
                                ? 'Working'
                                : glance.connection.short,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: DenseType.caption.copyWith(
                              color: glance.problem != null
                                  ? palette.danger
                                  : palette.textFaint,
                            ),
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}
