import 'dart:async';
import 'dart:convert';
import 'dart:developer' as developer;

import 'package:crypto/crypto.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import 'account_match.dart';
import 'app_dock.dart';
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
import 'mobile/back.dart';
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
import 'voice_keep.dart' show keptClaktoryKey, keptVoiceCount;
import 'inbox.dart' show needsYouCount;
import 'waiting.dart' show NeedsYouMark;
import 'your_claktories.dart' show DeviceSignInDialog;

/// The rail of Claktories at the window's far left, as thin as the
/// extension bar.
const railWidth = 40.0;

/// One Claktory this app is connected to: this Mac's, or one on another
/// machine. Each has its own live connection and everything the window
/// shows of it, so switching between them is instant and nothing of one
/// ever shows as another's.
class Claktory {
  Claktory({
    required this.target,
    required this.backend,
    required this.drafts,
    this.match,
  }) : key = target?.code ?? '' {
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

  /// Which Claktories are on the person's account; none in a window with
  /// no rail.
  final AccountMatch Function()? match;

  /// What the person's Claktory account says of it, by the endpoint the
  /// app dials it by; null until it's known (or with no rail).
  AccountCheck? get account => match?.call().check(target?.endpointId);

  /// Known not to be on the person's account (its list read, and it isn't
  /// there): marked wherever it shows. Signed out, or the list unreadable,
  /// nothing is known, so nothing is marked (#529, S2: never a mark the
  /// person can't act on).
  bool get offAccount => account == AccountCheck.notTheirs;

  /// Whether its details (its form) say anything of the account: known not
  /// on it, couldn't be checked, or this app signed out (with Sign in…).
  bool get accountNoted {
    final check = account;
    return check != null && check != AccountCheck.theirs;
  }

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
              unawaited(_accountMatch?.relist());
            },
          )
        : AppLifecycleListener(
            onResume: () {
              for (final claktory in _open.values) {
                claktory.backend.check();
              }
              unawaited(_accountMatch?.relist());
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

  /// Which of them are on the person's Claktory account, read when first
  /// asked.
  AccountMatch get accountMatch => _accountMatch ??= AccountMatch(
    remotes: () => [for (final target in connections.saved) target.endpointId],
    changes: this,
    account: connections.account,
    theirs: () =>
        connections.account.signedIn ? connections.account.claktories() : null,
    whose: () => '${connections.account.service} ${connections.account.email}',
  )..addListener(_feedCheckIns);
  AccountMatch? _accountMatch;

  /// Each Claktory on another machine learns when it last checked in with
  /// the person's account, for what its connection says while it can't
  /// connect (friend bug #37).
  void _feedCheckIns() {
    final match = _accountMatch;
    if (match == null) return;
    for (final claktory in _open.values) {
      final target = claktory.target;
      if (target == null) continue;
      claktory.backend.checkedIn.value = match.checkedIn(target.endpointId);
    }
  }

  /// When the account's list was last read again for a Claktory that lost
  /// its connection: at most every [_relistAfterLoss].
  DateTime? _relistedForLoss;
  static const _relistAfterLoss = Duration(minutes: 5);

  /// A Claktory lost its connection: when it last checked in with the
  /// account says whether it was running since (friend bug #37).
  void _lost(Backend backend) {
    if (backend.connected.value) return;
    final match = _accountMatch;
    if (match == null) return;
    final now = DateTime.now();
    final last = _relistedForLoss;
    if (last != null && now.difference(last) < _relistAfterLoss) return;
    _relistedForLoss = now;
    unawaited(match.relist());
  }

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
        final backend = this.open(target);
        _open[key] = Claktory(
          target: target,
          backend: backend,
          drafts: drafts.of(key),
          match: () => accountMatch,
        );
        if (target != null) {
          backend.connected.addListener(() => _lost(backend));
        }
      }
    }
    _feedCheckIns();
    _markOnScreen();
    notifyListeners();
  }

  /// On a phone or an iPad only the Claktory on screen keeps trying every
  /// few seconds while it can't connect; the others back off, and opening
  /// one tries it again at once (#454). A Mac keeps them all quick.
  void _markOnScreen() {
    final mobile = switch (defaultTargetPlatform) {
      TargetPlatform.iOS || TargetPlatform.android => true,
      _ => false,
    };
    if (!mobile) return;
    final current = connections.current?.code ?? '';
    for (final MapEntry(:key, value: claktory) in _open.entries) {
      claktory.backend.onScreen = key == current;
    }
  }

  @override
  void dispose() {
    _lifecycle.dispose();
    _accountMatch?.dispose();
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
    offAccount = claktory.offAccount;
  }

  /// Known not to be on the person's Claktory account: marked, its tile
  /// ringed in `danger`.
  late final bool offAccount;

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
    ?claktory.match?.call(),
  ]);
}

/// What a Claktory known not to be the person's own is marked with: a
/// short label and what it means (Trish's words). Nothing for any other
/// state: signed out or unreadable, nothing is known, so nothing is
/// marked (#529, S2); only its details say so ([OffAccountMark.details]).
({String label, String meaning})? accountWords(AccountCheck? check) =>
    switch (check) {
      AccountCheck.notTheirs => (
        label: 'Not on your Claktory account',
        meaning:
            "It may belong to someone else. If it's yours, sign it in to "
            'your Claktory account.',
      ),
      _ => null,
    };

/// [claktory]'s name as VoiceOver reads it: with its mark ("nixos, not on
/// your Claktory account").
String claktoryLabel(String name, Claktory claktory) {
  final words = accountWords(claktory.account);
  if (words == null) return name;
  final label = words.label;
  return '$name, ${label[0].toLowerCase()}${label.substring(1)}';
}

/// The mark of a Claktory known not to be the person's own, wherever it
/// shows by name (the rail's card, the phone's drawer and list, its form):
/// a 12px shield-alert and the label in caption `danger`, and, [explained],
/// what it means under it in caption `textMuted`. Nothing for one that's
/// theirs, or that can't be checked (#529, S2), except in its [details].
class OffAccountMark extends StatelessWidget {
  const OffAccountMark({
    super.key,
    required this.claktory,
    this.explained = false,
    this.details = false,
  });

  final Claktory claktory;
  final bool explained;

  /// Its details (the Claktory's form): what's known of the account there
  /// too, in muted words. That it couldn't be checked, or, while this app
  /// is signed out, a way to sign in (Trish's words).
  final bool details;

  @override
  Widget build(BuildContext context) {
    final match = claktory.match;
    if (match == null) return const SizedBox.shrink();
    return ListenableBuilder(
      listenable: match(),
      builder: (context, _) {
        final check = claktory.account;
        final palette = Palette.of(context);
        final caption = TypeScale.of(context).caption;
        final muted = caption.copyWith(color: palette.textMuted);
        if (details && check == AccountCheck.unreadable) {
          return Text(
            "Couldn't check whether ${claktoryName(claktory.backend)} is on "
            'your Claktory account. Claktory tries again by itself.',
            style: muted,
          );
        }
        if (details && check == AccountCheck.signedOut) {
          return Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                'Sign in to your Claktory account in this app to check which '
                'Claktories are on it.',
                style: muted,
              ),
              const SizedBox(height: Space.xs),
              Button(
                label: 'Sign in…',
                onPressed: () => showAccountSignIn(context),
              ),
            ],
          );
        }
        final words = accountWords(check);
        if (words == null) return const SizedBox.shrink();
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            // The glyph on the label's first line, growing with the
            // text; the label wraps, never cut short.
            Text.rich(
              TextSpan(
                children: [
                  WidgetSpan(
                    alignment: PlaceholderAlignment.middle,
                    child: Padding(
                      padding: const EdgeInsetsDirectional.only(end: Space.xs),
                      child: ExtensionGlyph(
                        icon: 'shield-alert',
                        name: '!',
                        color: palette.danger,
                        size: 12,
                      ),
                    ),
                  ),
                  TextSpan(text: words.label),
                ],
              ),
              style: caption.copyWith(color: palette.danger),
            ),
            if (explained || details) Text(words.meaning, style: muted),
          ],
        );
      },
    );
  }
}

/// Signs this device in to the person's Claktory account, from a
/// Claktory's details while signed out (#388): it closes once signed in.
Future<void> showAccountSignIn(BuildContext context) => showModal(
  context,
  (context, close) => DeviceSignInDialog(
    title: 'Sign in to your Claktory account',
    intro: 'So this app can recognize the Claktories on your account.',
    close: close,
  ),
);

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
                    // The shown Claktory's pinned apps, at the rail's foot.
                    const AppDock(),
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
        final touch = touchFirst(context);
        void openForm() => unawaited(
          showModal(
            context,
            (context, close) => ClaktoryForm(claktory: claktory, close: close),
          ),
        );
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
                    if (glance.offAccount) ...[
                      const SizedBox(height: Space.xs),
                      OffAccountMark(claktory: claktory, explained: true),
                    ],
                  ],
                ),
                child: Semantics(
                  button: true,
                  selected: selected,
                  label: claktoryLabel(name, claktory),
                  onLongPress: touch ? openForm : null,
                  // Touch and hold: the form on the hold itself, as a
                  // context menu opens, for a finger with no secondary
                  // click (an iPad, Zucchero's row 642); a pointer's
                  // secondary click as before.
                  child: GestureDetector(
                    onLongPress: touch
                        ? () {
                            unawaited(Haptics.selection());
                            openForm();
                          }
                        : null,
                    child: Pressable(
                      onTap: selected ? null : onTap,
                      onSecondaryTap: openForm,
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
                // Not on the person's account: a `danger` ring, whatever
                // else it shows.
                border: Border.all(
                  color: glance.offAccount
                      ? palette.danger
                      : selected || own != null
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
                // They grow with the text size until they fill the tile,
                // and no further: the tile keeps its size in the rail, and
                // its name is read out whole (Sorbet's row 102).
                : Padding(
                    padding: EdgeInsets.all(3 * size / 28),
                    child: FittedBox(
                      fit: BoxFit.scaleDown,
                      child: Text(
                        claktoryInitials(name),
                        maxLines: 1,
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
                  ),
          ),
          // What waits for the person there, counted (#529, MC10), unless
          // its connection says more (can't, or connecting).
          if (glance.problem == null &&
              !glance.connecting &&
              glance.waiting > 0)
            Positioned(
              right: -6,
              top: -6,
              child: NeedsYouMark(
                count: glance.waiting,
                label: '$name, ${glance.waiting} in Needs you',
              ),
            )
          // Its connection first (can't, or connecting), then its news.
          else if (glance.problem != null || glance.connecting || glance.unseen)
            Positioned(
              right: -3,
              top: -3,
              child: Semantics(
                label:
                    glance.problem ??
                    (glance.connecting
                        ? glance.connection.line
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

/// Asks before removing [claktory] from this device, wherever that's
/// chosen (its form, the chooser's rows: one act, one question, MC8), and
/// removes it on Remove: this device leaves its devices when it can be
/// reached, and forgets its key, its kept files and recordings.
Future<void> confirmRemoveClaktory(BuildContext context, Claktory claktory) =>
    showModal(
      context,
      (context, close) => RemoveClaktoryQuestion(
        claktory: claktory,
        onCancel: close,
        onRemoved: close,
      ),
    );

/// "Remove {name}?": what it means, how to connect again, and the
/// recordings not sent yet that go with it (Sale's row 76). When this is
/// the only device connected there, it says so, and the way back that's
/// left (#529, MC9, Trish's words).
class RemoveClaktoryQuestion extends StatefulWidget {
  const RemoveClaktoryQuestion({
    super.key,
    required this.claktory,
    required this.onCancel,
    required this.onRemoved,
  });

  final Claktory claktory;
  final VoidCallback onCancel;
  final VoidCallback onRemoved;

  @override
  State<RemoveClaktoryQuestion> createState() => _RemoveClaktoryQuestionState();
}

class _RemoveClaktoryQuestionState extends State<RemoveClaktoryQuestion> {
  /// Recordings kept on this device for it, not sent yet.
  var _unsent = 0;

  @override
  void initState() {
    super.initState();
    final target = widget.claktory.target;
    if (target != null) unawaited(_count(target));
  }

  Future<void> _count(RemoteFactory target) async {
    final unsent = await keptVoiceCount(keptClaktoryKey(target));
    if (mounted && unsent != _unsent) setState(() => _unsent = unsent);
  }

  void _remove() {
    final target = widget.claktory.target;
    if (target == null) return widget.onCancel();
    // It forgets this device's key, so its pairing is of no more use:
    // leave its devices too, when it can be reached.
    final backend = widget.claktory.backend;
    if (backend.connected.value && backend.deviceId.isNotEmpty) {
      fireAndLog(backend.unpairDevice(backend.deviceId));
    }
    AppScope.scope(context).connections.forget(target);
    widget.onRemoved();
  }

  /// Whether no other device is connected to it (paired, its code used),
  /// this computer's own app counted as one; null when that can't be
  /// known (not connected now).
  bool? get _onlyDevice {
    final backend = widget.claktory.backend;
    if (!backend.connected.value) return null;
    return !backend.workspace.workspace.remote.devices.any(
      (device) => device.id != backend.deviceId && device.codeExpiresMs == 0,
    );
  }

  @override
  Widget build(BuildContext context) {
    final name = claktoryName(widget.claktory.backend);
    final String body;
    if (_onlyDevice == true) {
      body =
          'This $thisDeviceKind is the only device connected to $name. Its '
          'agents and work stay there. '
          '${widget.claktory.account == AccountCheck.theirs ? 'To connect again, choose it under On your account in Add a Claktory.' : 'To connect again, run factoryd pair in a terminal on $name.'}';
    } else if (_onlyDevice == false) {
      body =
          'This $thisDeviceKind disconnects from it. Its agents and work '
          'stay on $name. To connect again, add this $thisDeviceKind on '
          '$name in Settings → Devices.';
    } else {
      // Not connected now, so whether another device is there can't be
      // known: both ways back, since either may be the one left (Squalo's
      // V1). Turned away for another account, neither works until that
      // changes, so it says so.
      final otherAccount = widget.claktory.backend.otherAccount.value != null;
      body =
          'This $thisDeviceKind disconnects from $name. Its agents and work '
          'stay there. To connect again, add this $thisDeviceKind in '
          'Settings → Devices on $name, or run factoryd pair in a terminal '
          'there'
          '${widget.claktory.account == AccountCheck.theirs ? ', or choose $name under On your account in Add a Claktory' : ''}.'
          '${otherAccount ? " While $name is on another Claktory account, it won't let this $thisDeviceKind in." : ''}';
    }
    return FormFrame(
      title: 'Remove $name?',
      body: '$body${unsentGoesToo(_unsent)}',
      fields: const [],
      error: null,
      submitLabel: 'Remove',
      onSubmit: _remove,
      onCancel: widget.onCancel,
    );
  }
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

  @override
  Widget build(BuildContext context) {
    final target = widget.claktory.target;
    final name = claktoryName(_backend);
    if (_confirmingRemove && target != null) {
      return RemoveClaktoryQuestion(
        claktory: widget.claktory,
        onCancel: () => setState(() => _confirmingRemove = false),
        onRemoved: widget.close,
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
        fields: [
          if (widget.claktory.accountNoted)
            OffAccountMark(claktory: widget.claktory, details: true),
        ],
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
        if (widget.claktory.accountNoted)
          OffAccountMark(claktory: widget.claktory, details: true),
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
class ClaktoriesBackStrip extends StatefulWidget {
  const ClaktoriesBackStrip({super.key});

  @override
  State<ClaktoriesBackStrip> createState() => _ClaktoriesBackStripState();
}

class _ClaktoriesBackStripState extends State<ClaktoriesBackStrip> {
  PhoneBack? _back;
  BackHandle? _handle;
  VoidCallback? _backToList;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _backToList = ClaktoriesScope.of(context) == null
        ? null
        : context.dependOnInheritedWidgetOfExactType<_BackToList>()?.back;
    final policy = PhonePlatformPolicy.of(context);
    final next = _backToList != null && policy.touch && !policy.drawerEdgeSwipe
        ? PhoneBack.maybeOf(context)
        : null;
    if (identical(next, _back)) return;
    _handle?.release();
    _back = next;
    // A full-screen step has no PhoneNavigator to pop. Its visible
    // Claktories action is also Android's way out, below sheets and IME.
    // Keep the token through rebuilds so a cancelled or stale gesture
    // cannot change destination or fall through to the system.
    _handle = next?.registerNavigation(
      CallbackBackToken(
        onBack: () {
          if (mounted) _backToList?.call();
        },
      ),
    );
  }

  @override
  void dispose() {
    _handle?.release();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (_backToList == null) return const SizedBox.shrink();
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
        waiting: [for (final other in others) _Glance(other).waiting]
            .fold(0, (sum, count) => sum + count),
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
    this.waiting = 0,
    this.trailing,
  });

  final String label;
  final VoidCallback onTap;
  final bool news;

  /// How much waits for the person's answer there: counted, in place of
  /// the news dot (#529, MC10).
  final int waiting;

  /// At its end, such as [PhoneConnectionWord].
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Unselectable(
      child: Semantics(
        button: true,
        label: waiting > 0
            ? 'Back to $label, $waiting in Needs you'
            : 'Back to $label',
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
                  if (waiting > 0) ...[
                    const SizedBox(width: Space.s),
                    NeedsYouMark(count: waiting, label: ''),
                  ] else if (news) ...[
                    const SizedBox(width: Space.s),
                    Dot(color: palette.text, size: 7),
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
/// Nothing while the Claktory turns this device away: the screen under it
/// says why, and it won't try again, so "Connecting…" would be untrue
/// (Sale row 173).
class PhoneConnectionWord extends StatelessWidget {
  const PhoneConnectionWord(this.backend, {super.key});

  final Backend backend;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    // Follows the connection itself: the drawer's header rebuilds only
    // with the Workspace (#388).
    listenable: Listenable.merge([
      Connection.of(backend),
      backend.signInRequired,
      backend.otherAccount,
    ]),
    builder: (context, _) =>
        backend.signInRequired.value != null ||
            backend.otherAccount.value != null
        ? const SizedBox.shrink()
        : _word(context),
  );

  Widget _word(BuildContext context) {
    final palette = Palette.of(context);
    final connection = Connection(backend);
    return Semantics(
      button: true,
      label: connection.line,
      hint: reconnectFirst(connection.link) ? reconnectAndShowHint : null,
      child: Pressable(
        onTap: () {
          // Not connected: it reconnects at once too (#214).
          if (reconnectFirst(connection.link)) backend.reconnectNow();
          showModal(
            context,
            (context, close) =>
                ConnectionDialog(backend: backend, close: close),
          );
        },
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
                      // On a phone, whole: at the largest text sizes
                      // "Can't connect · last seen 1 min ago" lost its
                      // time to the ellipsis (Zucchero's row 507).
                      child: Text(
                        connection.short,
                        maxLines: isPhoneLayout(context) ? null : 2,
                        overflow: isPhoneLayout(context)
                            ? null
                            : TextOverflow.ellipsis,
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

  /// Something outside the tree chose what to show: leave the list/add
  /// screen and bring the selected conversation in front of drawer/pages.
  static void showConversation() {
    for (final host in _PhoneClaktoriesState._live.toList()) {
      host._go(_PhoneScreen.claktory);
    }
    // The drawer may only be built as the list/add screen leaves.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      PhoneDrawerHost.showConversation();
    });
  }

  @override
  State<PhoneClaktories> createState() => _PhoneClaktoriesState();
}

enum _PhoneScreen { claktory, list, adding }

class _PhoneClaktoriesState extends State<PhoneClaktories> {
  static final _live = <_PhoneClaktoriesState>{};

  @override
  void initState() {
    super.initState();
    _live.add(this);
  }

  var _screen = _PhoneScreen.claktory;
  Claktory? _shown;
  Claktory? _returnTo;
  Claktories? _claktories;
  PhoneBack? _windowBack;
  PhoneBack? _screenBack;
  BackHandle? _screenBackHandle;
  _PhoneScreen? _backScreen;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // A new one paired: it shows. (One removed from the list leaves the
    // list showing, with the next one chosen.)
    final claktories = _claktories = ClaktoriesScope.of(context);
    final selected = claktories?.selected;
    if (_shown != null &&
        selected != _shown &&
        _screen == _PhoneScreen.adding) {
      _screen = _PhoneScreen.claktory;
    }
    _shown = selected;
    final policy = PhonePlatformPolicy.of(context);
    _windowBack = policy.touch && !policy.drawerEdgeSwipe
        ? context.dependOnInheritedWidgetOfExactType<PhoneBackScope>()?.back
        : null;
    _syncBack();
  }

  void _syncBack() {
    final hasList = _claktories?.all.isNotEmpty ?? false;
    final screen = hasList ? _screen : _PhoneScreen.claktory;
    final next = _windowBack?.branch(screen);
    if (identical(next, _screenBack) && screen == _backScreen) return;
    _screenBackHandle?.release();
    _screenBackHandle = null;
    _screenBack = next;
    _backScreen = screen;
    if (next == null || !hasList || screen == _PhoneScreen.claktory) return;
    _screenBackHandle = next.registerNavigation(
      CallbackBackToken(
        onBack: () {
          if (!mounted || _screen != screen) return;
          screen == _PhoneScreen.adding
              ? _go(_PhoneScreen.list)
              : _returnFromList();
        },
      ),
    );
  }

  void _returnFromList() {
    final claktories = _claktories;
    if (claktories == null || claktories.all.isEmpty) return;
    final all = claktories.all;
    final destination = all.contains(_returnTo)
        ? _returnTo!
        : all.contains(claktories.selected)
        ? claktories.selected
        : all.first;
    claktories.select(destination);
    _go(_PhoneScreen.claktory);
  }

  void _go(_PhoneScreen screen) {
    if (_screen == _PhoneScreen.claktory && screen != _PhoneScreen.claktory) {
      _returnTo = _claktories?.selected;
    }
    setState(() {
      _screen = screen;
      // Retire the outgoing screen synchronously, before its animation ends.
      _syncBack();
    });
  }

  @override
  void dispose() {
    _live.remove(this);
    _screenBackHandle?.release();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final claktories = ClaktoriesScope.of(context);
    // Nothing paired yet: the window pairs the first.
    if (claktories == null || claktories.all.isEmpty) {
      return _screenBack == null
          ? widget.shell
          : PhoneBackScope(back: _screenBack!, child: widget.shell);
    }
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
      child: _screenBack == null
          ? child
          : PhoneBackScope(key: child.key, back: _screenBack!, child: child),
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
          label: claktoryLabel(name, claktory),
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
          label: claktoryLabel(name, claktory),
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
                          // Read out with the row's label already.
                          if (glance.offAccount)
                            ExcludeSemantics(
                              child: OffAccountMark(claktory: claktory),
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

/// What removing a Claktory says of the [count] recordings kept for it on
/// this device, not sent yet, which go with it; nothing when there are
/// none. Trish's words: the sheet already names the device.
String unsentGoesToo(int count) => switch (count) {
  0 => '',
  1 => " A voice recording you haven't sent yet is deleted too.",
  _ => " $count voice recordings you haven't sent yet are deleted too.",
};
