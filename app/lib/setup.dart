// Setting Claktory up on this Mac from the app itself: the app downloaded
// as a disk image carries the backend in Contents/Helpers, and the first
// launch runs its `factoryd install` (app/macos/Runner/Setup.swift). Also
// offers to move the app into Applications when it runs from the disk
// image. Only the Mac's release build; Claktory Dev never installs.
import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:url_launcher/url_launcher.dart';

import 'connections.dart' show hasLocalBackend;
import 'controls.dart';
import 'identity.dart';
import 'main.dart' show isDevBuild;
import 'name_step.dart' show StepCard;
import 'selection.dart' show SelectableArea;

/// Where to download Claktory for Mac again.
final downloadMacUrl = Uri.parse('https://claktory.com/download/mac');

/// Where to download Claktory for Windows again.
final downloadWindowsUrl = Uri.parse('https://claktory.com/download/windows');

/// Whether this app sets this computer up itself: a release build of the
/// real Claktory on a Mac, or on a Windows PC, where the installer laid the
/// backend down beside the app.
bool get setsUpThisMac =>
    (defaultTargetPlatform == TargetPlatform.macOS ||
        defaultTargetPlatform == TargetPlatform.windows) &&
    kReleaseMode &&
    !isDevBuild;

/// This computer, in a sentence: "this PC" on Windows, "this Mac" elsewhere.
String get thisComputer =>
    defaultTargetPlatform == TargetPlatform.windows ? 'this PC' : 'this Mac';

/// Whether this device runs a Claktory of its own: a Mac does, unless
/// it's marked app only (`install.sh --app-only`, or the person chose a
/// Claktory on another computer) and has none installed. Claktory Dev
/// goes by the mark alone: the dev Claktory is always there.
Future<bool> runsClaktoryHere({
  SetupChannel channel = const SetupChannel(),
}) async {
  if (!hasLocalBackend) return false;
  if (defaultTargetPlatform != TargetPlatform.macOS &&
      defaultTargetPlatform != TargetPlatform.windows) {
    return true;
  }
  if (!await channel.appOnly()) return true;
  return !isDevBuild && channel.installedHere();
}

/// Where the app runs from.
class SetupLocation {
  const SetupLocation({
    required this.path,
    required this.needsMove,
    required this.helpers,
    this.fromImage = false,
  });

  final String path;

  /// Run from the disk image itself (else, needing a move, from a
  /// temporary copy macOS made).
  final bool fromImage;

  /// From the disk image, or translocated by macOS: somewhere it can't stay.
  final bool needsMove;

  /// It carries the backend (Contents/Helpers/factoryd).
  final bool helpers;
}

/// What `factoryd install` answered: its one JSON line.
class InstallResult {
  const InstallResult({
    required this.result,
    this.reason = '',
    this.detail = '',
    this.version = '',
    this.running = false,
    this.held = false,
    this.startsAtSignIn,
    this.dataBytes = 0,
    this.dataComplete = true,
    this.answered = true,
  });

  /// Its JSON line, or a failure when there's none.
  factory InstallResult.parse(int code, String stdout, String stderr) {
    for (final line in const LineSplitter().convert(stdout).reversed) {
      if (!line.trim().startsWith('{')) continue;
      try {
        final json = jsonDecode(line) as Map<String, dynamic>;
        return InstallResult(
          result: json['result'] as String? ?? '',
          reason: json['reason'] as String? ?? '',
          detail: json['detail'] as String? ?? '',
          version: json['version'] as String? ?? '',
          running: json['running'] as bool? ?? false,
          held: json['held'] as bool? ?? false,
          startsAtSignIn: json['starts_at_sign_in'] as bool?,
          // What a dry run of uninstall would delete, the backend itself
          // aside (it goes either way).
          dataBytes: [
            for (final item in json['items'] as List? ?? const [])
              if (item is Map && item['what'] != 'backend')
                (item['bytes'] as num?)?.toInt() ?? 0,
          ].fold(0, (sum, bytes) => sum + bytes),
          dataComplete: (json['items'] as List? ?? const []).every(
            (item) => item is! Map || item['complete'] != false,
          ),
        );
      } on FormatException {
        break;
      } on TypeError {
        break;
      }
    }
    return InstallResult(
      result: 'failed',
      answered: false,
      detail: [
        'exit $code',
        if (stderr.trim().isNotEmpty) stderr.trim(),
      ].join('\n'),
    );
  }

  /// not_installed, installed, already_installed, incomplete, not_running,
  /// stopped, started, damaged or failed.
  final String result;

  /// A sentence to show as it is.
  final String reason;

  /// Technical: an error, the log's path; for Show details.
  final String detail;

  /// The helpers' version.
  final String version;

  /// For already_installed: whether the backend answered then.
  final bool running;

  /// For stopped: the person turned it off (Stop Claktory), so it stays
  /// off until they start it; else the app quitting stopped it, and
  /// opening the app starts it.
  final bool held;

  /// On Windows: whether it starts by itself when the person signs in to
  /// Windows, as they last chose (`install --consent`), whatever [held]
  /// says now. Null where the backend doesn't say (a Mac's always does).
  final bool? startsAtSignIn;

  /// For a dry run of uninstall: how much data it would delete (0 when
  /// not said), and whether that's all of it (else at least that much).
  final int dataBytes;
  final bool dataComplete;

  /// Whether `factoryd` answered with its line at all: one from before a
  /// command (uninstall, say) doesn't.
  final bool answered;
}

/// The app's native side of setting up, a channel to Setup.swift; tests
/// give another.
class SetupChannel {
  const SetupChannel();

  static const _channel = MethodChannel('factory/setup');

  Future<SetupLocation> location() async {
    final answer = await _channel.invokeMapMethod<String, Object?>('location');
    return SetupLocation(
      path: answer?['path'] as String? ?? '',
      needsMove: answer?['needsMove'] as bool? ?? false,
      fromImage: answer?['fromImage'] as bool? ?? false,
      helpers: answer?['helpers'] as bool? ?? false,
    );
  }

  /// Runs the bundled `factoryd` with [arguments].
  Future<InstallResult> install(List<String> arguments) =>
      _factoryd('install', arguments);

  /// Runs `factoryd uninstall` (the installed one, else the bundled one)
  /// with [arguments].
  Future<InstallResult> uninstall(List<String> arguments) =>
      _factoryd('uninstall', arguments);

  /// Puts this app in the Trash and quits; with [forget], what it keeps on
  /// this Mac (its window, drafts and connections) goes too.
  Future<void> removeApp({required bool forget}) =>
      _channel.invokeMethod<void>('removeApp', forget);

  Future<InstallResult> _factoryd(String method, List<String> arguments) async {
    try {
      final answer = await _channel.invokeMapMethod<String, Object?>(
        method,
        arguments,
      );
      return InstallResult.parse(
        answer?['code'] as int? ?? -1,
        answer?['stdout'] as String? ?? '',
        answer?['stderr'] as String? ?? '',
      );
    } on PlatformException catch (error) {
      return InstallResult(
        result: 'failed',
        detail: '${error.code}: ${error.message ?? ''}',
      );
    }
  }

  /// Whether this Mac has no Claktory of its own, only the app
  /// (`ClaktoryAppOnly`: `install.sh --app-only`, or "Use a Claktory on
  /// another computer"). False where the app can't say.
  Future<bool> appOnly() async {
    try {
      return await _channel.invokeMethod<bool>('appOnly') ?? false;
    } on PlatformException {
      return false;
    } on MissingPluginException {
      return false;
    }
  }

  /// Keeps whether this Mac has no Claktory of its own.
  Future<void> setAppOnly(bool appOnly) async {
    try {
      await _channel.invokeMethod<void>('setAppOnly', appOnly);
    } on PlatformException {
      // Asked again at the next launch.
    } on MissingPluginException {
      // The same, where there's no channel.
    }
  }

  /// Whether macOS lets this Mac's Claktory run in the background: its
  /// switch under System Settings → General → Login Items → Allow in the
  /// Background. [BackgroundStatus.unavailable] where the
  /// app can't say.
  Future<BackgroundStatus> backgroundStatus() async {
    try {
      return BackgroundStatus.parse(
        await _channel.invokeMethod<String>('backgroundStatus'),
      );
    } on PlatformException {
      return BackgroundStatus.unavailable;
    } on MissingPluginException {
      return BackgroundStatus.unavailable;
    }
  }

  /// Opens System Settings → General → Login Items.
  Future<void> openLoginItems() async {
    try {
      await _channel.invokeMethod<void>('openLoginItems');
    } on PlatformException {
      // Nothing more to do: the step says where it is.
    } on MissingPluginException {
      // The same.
    }
  }

  /// Moves the app to Applications and quits; the error's words when it
  /// can't.
  Future<String?> moveToApplications() =>
      _channel.invokeMethod<String>('moveToApplications');

  /// Whether this Mac has a Claktory installed already, for an app without
  /// helpers: its backend in ~/.factory/bin.
  bool installedHere() {
    if (Platform.isWindows) {
      final local = Platform.environment['LOCALAPPDATA'] ?? '';
      return local.isNotEmpty &&
          File('$local\\Claktory\\factory\\bin\\factoryd.exe').existsSync();
    }
    final home = Platform.environment['HOME'] ?? '';
    return home.isNotEmpty && File('$home/.factory/bin/factoryd').existsSync();
  }
}

/// What macOS says of the LaunchAgent that runs this Mac's Claktory
/// (Setup.swift's backgroundStatus, from SMAppService).
enum BackgroundStatus {
  /// Allowed to run.
  enabled,

  /// Its switch under Allow in the Background is off: launchd won't start
  /// it.
  requiresApproval,

  /// macOS knows no such job.
  notRegistered,
  notFound,

  /// Before macOS 13, or off the Mac: it can't say.
  unavailable;

  static BackgroundStatus parse(String? name) => switch (name) {
    'enabled' => enabled,
    'requiresApproval' => requiresApproval,
    'notRegistered' => notRegistered,
    'notFound' => notFound,
    _ => unavailable,
  };

  /// macOS keeps it from running.
  bool get blocked => this == requiresApproval;
}

/// Whether this Mac's own Claktory can run, as setting it up last found
/// it: its connection says so while it doesn't answer (connection.dart).
enum LocalEngine {
  /// Running, or nothing known against it.
  running,

  /// The person turned it off: it waits for Start Claktory.
  off,

  /// macOS keeps it from running in the background.
  blocked,
}

/// Where Claktory may show under Allow in the Background: a Mac that set
/// it up before the release that names it there keeps its developer's
/// name for good. No person's name in the app's
/// words.
const backgroundItemNote =
    'If you had Claktory before this version, it may be listed under its '
    "developer's name instead.";

/// This Mac's own Claktory's [LocalEngine], kept by [MacSetup].
final localEngine = ValueNotifier(LocalEngine.running);

/// Where setting up is.
enum SetupStage {
  /// Asking the app where it runs and what's installed.
  checking,

  /// Offering to move the app to Applications.
  offerMove,

  /// Moving it; the app quits when it's done.
  moving,

  /// Asking where Claktory runs: on this Mac, or on another computer.
  choosing,

  /// `factoryd install` (or --repair) is running.
  settingUp,

  /// `factoryd install --start` is starting it.
  starting,

  /// The person turned it off (Stop Claktory): it waits for Start.
  stopped,

  /// It couldn't, or did only part of it.
  failed,

  /// Installed, but the backend doesn't answer.
  notRunning,

  /// macOS keeps it from running in the background: its switch under
  /// Login Items is off.
  blocked,

  /// The app carries no backend and none is installed.
  missing,

  /// Nothing more to do (or put off with Not now).
  done,
}

/// The first launch's setting up of this Mac, as a step over the window
/// while it's needed.
class MacSetup extends ChangeNotifier {
  MacSetup({
    required this.channel,
    required this.connected,
    required this.reconnect,
    this.startWithin = const Duration(seconds: 30),
    this.lostFor = const Duration(seconds: 5),
    this.lookEvery = const Duration(seconds: 2),
    this.stopCheckEvery = const Duration(milliseconds: 500),
    this.elsewhere,
    this.ask = true,
  }) {
    connected.addListener(_connectedChanged);
  }

  final SetupChannel channel;

  /// Whether the app is connected to this Mac's Claktory.
  final ValueListenable<bool> connected;

  /// Tries to connect again at once.
  final VoidCallback reconnect;

  /// How long an installed backend may take to answer before Repair is
  /// offered.
  final Duration startWithin;

  /// How long this Mac's Claktory may be out of reach, after it answered,
  /// before the app asks macOS whether it's keeping it from running.
  final Duration lostFor;

  /// How often the app looks again while macOS keeps it from running, so
  /// it starts as soon as it's allowed.
  final Duration lookEvery;

  /// How often, once it was asked to stop (Turn off, Stop Claktory) and
  /// went, the app asks whether it has stopped, so it says so at once
  /// rather than Reconnecting….
  final Duration stopCheckEvery;

  /// The person chose a Claktory on another computer instead: this Mac
  /// gets none. Null where that isn't offered.
  final VoidCallback? elsewhere;

  /// Whether a Mac with nothing installed is asked first: set up here, or
  /// use a Claktory on another computer ([elsewhere]). Not when the
  /// person already chose this Mac.
  final bool ask;

  var _stage = SetupStage.checking;
  SetupStage get stage => _stage;

  /// What went wrong, to show as it is; and its technical detail.
  String reason = '';
  String detail = '';

  /// Whether Repair is the way out (else Try again).
  bool repairable = false;

  /// Whether the way out is downloading the app again: this copy is
  /// damaged.
  bool download = false;

  /// Why moving to Applications failed.
  String? moveError;

  Timer? _waitForStart;
  Timer? _lost;
  Timer? _look;

  /// It took a stop the person asked for (BackendService.stopping): gone,
  /// it's looked at at once.
  var _stopExpected = false;
  var _looking = false;
  var _disposed = false;

  /// Whether it puts a step over the window now. Once this Mac's Claktory
  /// answers, only the offer to move stays.
  bool get showing => switch (_stage) {
    SetupStage.offerMove || SetupStage.moving || SetupStage.choosing => true,
    SetupStage.settingUp ||
    SetupStage.starting ||
    SetupStage.stopped ||
    SetupStage.failed ||
    SetupStage.notRunning ||
    SetupStage.blocked ||
    SetupStage.missing => !connected.value,
    SetupStage.checking || SetupStage.done => false,
  };

  void _set(SetupStage stage) {
    if (_disposed) return;
    _stage = stage;
    localEngine.value = switch (stage) {
      SetupStage.stopped => LocalEngine.off,
      SetupStage.blocked => LocalEngine.blocked,
      _ => LocalEngine.running,
    };
    notifyListeners();
  }

  void _connectedChanged() {
    if (!connected.value) {
      if (_stopExpected) {
        _stopExpected = false;
        unawaited(_untilStopped());
      } else {
        _watchLost();
      }
      return notifyListeners();
    }
    _stopExpected = false;
    _waitForStart?.cancel();
    _lost?.cancel();
    _look?.cancel();
    // Connected after all: nothing left to fix.
    if (_stage == SetupStage.failed ||
        _stage == SetupStage.notRunning ||
        _stage == SetupStage.blocked ||
        _stage == SetupStage.stopped ||
        _stage == SetupStage.starting ||
        _stage == SetupStage.missing) {
      _set(SetupStage.done);
    } else {
      notifyListeners();
    }
  }

  Future<void> start() async {
    final SetupLocation location;
    try {
      location = await channel.location();
    } on Object {
      // An app without the channel: nothing to set up from here.
      return _set(SetupStage.done);
    }
    _location = location;
    if (location.needsMove) return _set(SetupStage.offerMove);
    await _check();
  }

  SetupLocation? _location;

  /// Whether the app runs from the disk image (else from a temporary copy).
  bool get fromImage => _location?.fromImage ?? false;

  /// Not now: it runs from where it is.
  Future<void> stayHere() async {
    moveError = null;
    _set(SetupStage.checking);
    await _check();
  }

  Future<void> moveToApplications() async {
    moveError = null;
    _set(SetupStage.moving);
    String? error;
    try {
      error = await channel.moveToApplications();
    } on PlatformException catch (failure) {
      error = failure.message ?? failure.code;
    }
    // On success the app quits.
    if (error != null) {
      moveError = error;
      _set(SetupStage.offerMove);
    }
  }

  Future<void> _check() async {
    final location = _location;
    if (location == null || !location.helpers) {
      // Nothing to install with: fine when Claktory is here already.
      return _set(
        channel.installedHere() ? SetupStage.done : SetupStage.missing,
      );
    }
    final check = await channel.install(const ['install', '--check']);
    if (check.result == 'not_installed') {
      if (ask && elsewhere != null) return _set(SetupStage.choosing);
      return _run(const ['install']);
    }
    _settled(check);
  }

  Future<void> _run(List<String> arguments) async {
    _set(
      arguments.contains('--start')
          ? SetupStage.starting
          : SetupStage.settingUp,
    );
    final result = await channel.install(arguments);
    // Not started because macOS keeps it from running: that, not why
    // launchd refused.
    if ((result.result == 'failed' || result.result == 'not_running') &&
        await _blocked()) {
      return _block();
    }
    _settled(result);
  }

  /// Starts this Mac's Claktory: at launch when the app quitting stopped
  /// it, with Start when the person did, and once macOS allows it again.
  Future<void> startEngine() => _run(const ['install', '--start']);

  /// Sets Claktory up on this Mac, as chosen.
  Future<void> setUpHere() => _run(const ['install']);

  /// Uses a Claktory on another computer instead: nothing is installed.
  void useElsewhere() {
    _set(SetupStage.done);
    elsewhere?.call();
  }

  /// Repair: completes an install, or starts one that doesn't run.
  Future<void> repair() => _run(const ['install', '--repair']);

  Future<void> tryAgain() => _run(const ['install']);

  /// Not now, while it doesn't answer: the app keeps trying.
  void putOff() => _set(SetupStage.done);

  /// Whether this computer's Claktory starts by itself when the person
  /// signs in, as its backend last said; null where it doesn't say.
  bool? startsAtSignIn;

  /// Whether a change to [startsAtSignIn] is on its way.
  var changingSignIn = false;

  /// Starts it when the person signs in to Windows, or not
  /// (`install --consent`); why it couldn't, or null.
  Future<String?> setStartsAtSignIn(bool on) async {
    changingSignIn = true;
    notifyListeners();
    try {
      final result = await channel.install([
        'install',
        '--consent=${on ? 'yes' : 'no'}',
      ]);
      _heard(result);
      if (result.result == 'failed') {
        return result.reason.isEmpty
            ? "Claktory couldn't change this."
            : result.reason;
      }
      return null;
    } finally {
      changingSignIn = false;
      if (!_disposed) notifyListeners();
    }
  }

  /// What every answer says beside its result.
  void _heard(InstallResult result) {
    if (result.startsAtSignIn case final starts?) startsAtSignIn = starts;
  }

  void _settled(InstallResult result) {
    _heard(result);
    reason = result.reason;
    detail = result.detail;
    repairable = false;
    download = false;
    switch (result.result) {
      case 'installed':
        _set(SetupStage.done);
        reconnect();
      case 'already_installed':
        _set(SetupStage.done);
        if (!result.running) unawaited(_startingOrBlocked());
      case 'started':
        _set(SetupStage.done);
        reconnect();
      // Stopped: by the app quitting, so opening it starts it again; or
      // by the person, so it waits for them.
      case 'stopped':
        if (result.held) {
          _set(SetupStage.stopped);
        } else {
          unawaited(startEngine());
        }
      case 'incomplete':
        repairable = true;
        _set(SetupStage.failed);
      case 'not_running':
        repairable = true;
        unawaited(_notAnswering());
      case 'damaged':
        download = true;
        _set(SetupStage.failed);
      default:
        _set(SetupStage.failed);
    }
  }

  /// launchd may still be starting it: Repair is offered only if it doesn't
  /// answer in time.
  void _waitToStart() {
    _waitForStart?.cancel();
    if (connected.value) return;
    _waitForStart = Timer(startWithin, () {
      if (connected.value) return;
      reason = '';
      detail = '';
      repairable = true;
      unawaited(_notAnswering());
    });
  }

  /// Installed and not answering yet: macOS may be keeping it from
  /// running, which waiting won't change; else launchd may still be
  /// starting it.
  Future<void> _startingOrBlocked() async {
    if (await _blocked()) return _block();
    _waitToStart();
  }

  /// It doesn't answer: macOS keeps it from running, or else Repair.
  Future<void> _notAnswering() async {
    if (await _blocked()) return _block();
    if (_disposed || connected.value) return;
    _set(SetupStage.notRunning);
  }

  Future<bool> _blocked() async => (await channel.backgroundStatus()).blocked;

  /// macOS keeps it from running: says so, and looks again until it's
  /// allowed, then starts it.
  void _block() {
    if (_disposed || connected.value) return;
    _waitForStart?.cancel();
    _set(SetupStage.blocked);
    _look?.cancel();
    _look = Timer.periodic(lookEvery, (_) => unawaited(_lookAgain()));
  }

  Future<void> _lookAgain() async {
    if (_looking) return;
    if (_disposed || _stage != SetupStage.blocked) return _look?.cancel();
    _looking = true;
    try {
      if (await _blocked()) return;
    } finally {
      _looking = false;
    }
    _look?.cancel();
    if (_disposed || _stage != SetupStage.blocked || connected.value) return;
    // Allowed now: started.
    await startEngine();
  }

  /// Out of reach after it answered: once it's been a moment, macOS may
  /// have stopped it (its switch turned off while it ran), or the person
  /// turned it off.
  void _watchLost() {
    if (_stage != SetupStage.done) return;
    _lost?.cancel();
    _lost = Timer(lostFor, () async {
      if (connected.value || _stage != SetupStage.done) return;
      if (await _blocked()) {
        if (_stage == SetupStage.done && !connected.value) _block();
        return;
      }
      // Turned off (Stop Claktory, here or from another device's app on
      // this Mac): it says so, with Start. Anything else (restarting for
      // an update, say) it waits out.
      final check = await channel.install(const ['install', '--check']);
      _heard(check);
      if (_disposed || connected.value || _stage != SetupStage.done) return;
      if (check.result == 'stopped' && check.held) {
        _set(SetupStage.stopped);
      }
    });
  }

  /// It took a stop the person asked for: once it goes, the app says it's
  /// turned off as soon as it is.
  void expectStop() {
    if (connected.value) _stopExpected = true;
  }

  /// Asks until it says it's stopped (a moment, while launchd stops it),
  /// for a few seconds at most; then as for any lost connection.
  Future<void> _untilStopped() async {
    for (var tries = 0; tries < 20; tries++) {
      if (_disposed || connected.value || _stage != SetupStage.done) return;
      final check = await channel.install(const ['install', '--check']);
      _heard(check);
      if (_disposed || connected.value || _stage != SetupStage.done) return;
      if (check.result == 'stopped' && check.held) {
        return _set(SetupStage.stopped);
      }
      await Future<void>.delayed(stopCheckEvery);
    }
    _watchLost();
  }

  /// Opens System Settings at Login Items, where its switch is.
  Future<void> openLoginItems() => channel.openLoginItems();

  @override
  void dispose() {
    _disposed = true;
    localEngine.value = LocalEngine.running;
    _waitForStart?.cancel();
    _lost?.cancel();
    _look?.cancel();
    connected.removeListener(_connectedChanged);
    super.dispose();
  }
}

/// The app's [MacSetup], where there is one.
class SetupScope extends InheritedNotifier<MacSetup> {
  /// Always there, [setup] or not, so the window under it stays put as
  /// setting up starts or ends.
  const SetupScope({super.key, required MacSetup? setup, required super.child})
    : super(notifier: setup);

  static MacSetup? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<SetupScope>()?.notifier;
}

/// The step over the window while this Mac is set up: a card like the
/// first launch's other steps.
class SetupStep extends StatefulWidget {
  const SetupStep({super.key, required this.setup});

  final MacSetup setup;

  @override
  State<SetupStep> createState() => _SetupStepState();
}

class _SetupStepState extends State<SetupStep> {
  var _details = false;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final setup = widget.setup;
    final body = TextStyles.body.copyWith(color: palette.textMuted);
    Widget title(String text) =>
        Text(text, style: TextStyles.emphasis.copyWith(color: palette.text));
    Widget buttons(List<Widget> buttons) => Wrap(
      alignment: WrapAlignment.end,
      spacing: Space.s,
      runSpacing: Space.s,
      children: buttons,
    );
    final children = switch (setup.stage) {
      SetupStage.offerMove || SetupStage.moving => [
        title('Move Claktory to Applications?'),
        const SizedBox(height: Space.s),
        Text(
          setup.fromImage
              ? 'Claktory is running from the disk image. Move it to '
                    'Applications so it stays on this Mac and can update '
                    'itself.'
              : 'macOS is running a temporary copy of Claktory. Move it to '
                    'Applications so it stays on this Mac and can update '
                    'itself.',
          style: body,
        ),
        if (setup.moveError case final error?) ...[
          const SizedBox(height: Space.s),
          Text(
            error,
            style: TextStyles.caption.copyWith(color: palette.danger),
          ),
        ],
        const SizedBox(height: Space.xl),
        buttons([
          Button(
            label: 'Not now',
            onPressed: setup.stage == SetupStage.moving ? null : setup.stayHere,
          ),
          Button(
            label: 'Move to Applications',
            primary: true,
            onPressed: setup.stage == SetupStage.moving
                ? null
                : setup.moveToApplications,
          ),
        ]),
      ],
      SetupStage.choosing => [
        title('Set up Claktory'),
        const SizedBox(height: Space.s),
        Text(
          'Claktory runs your agents on $thisComputer. If it already runs on '
          'another computer, such as a server, use that one instead.',
          style: body,
        ),
        const SizedBox(height: Space.xl),
        buttons([
          Button(
            label: 'Use a Claktory on another computer',
            onPressed: setup.useElsewhere,
          ),
          Button(
            label: 'Set up on $thisComputer',
            primary: true,
            onPressed: setup.setUpHere,
          ),
        ]),
      ],
      SetupStage.settingUp => [
        Row(
          children: [
            const LiveDot(),
            const SizedBox(width: Space.s),
            Expanded(child: title('Setting up Claktory…')),
          ],
        ),
        const SizedBox(height: Space.s),
        Text('This takes a few seconds.', style: body),
      ],
      SetupStage.starting => [
        Row(
          children: [
            const LiveDot(),
            const SizedBox(width: Space.s),
            Expanded(child: title('Starting Claktory…')),
          ],
        ),
      ],
      SetupStage.stopped => [
        title('Claktory is turned off'),
        const SizedBox(height: Space.s),
        Text(
          "Your agents aren't running on $thisComputer. Start Claktory and "
          'they carry on where they left off.',
          style: body,
        ),
        const SizedBox(height: Space.xl),
        buttons([
          Button(
            label: 'Start Claktory',
            primary: true,
            onPressed: () => unawaited(setup.startEngine()),
          ),
        ]),
      ],
      SetupStage.failed => [
        title("Claktory couldn't finish setting up"),
        const SizedBox(height: Space.s),
        if (setup.reason.isNotEmpty) Text(setup.reason, style: body),
        ..._detail(palette),
        const SizedBox(height: Space.xl),
        buttons([
          if (setup.download)
            _download()
          else if (setup.repairable)
            Button(label: 'Repair', primary: true, onPressed: setup.repair)
          else
            Button(
              label: 'Try again',
              primary: true,
              onPressed: setup.tryAgain,
            ),
        ]),
      ],
      SetupStage.notRunning => [
        title("Claktory isn't responding"),
        const SizedBox(height: Space.s),
        Text(
          setup.reason.isNotEmpty
              ? setup.reason
              : "Claktory is installed but hasn't started. Repair usually "
                    'fixes this.',
          style: body,
        ),
        ..._detail(palette),
        const SizedBox(height: Space.xl),
        buttons([
          Button(label: 'Not now', onPressed: setup.putOff),
          Button(label: 'Repair', primary: true, onPressed: setup.repair),
        ]),
      ],
      SetupStage.blocked => [
        title('macOS is blocking Claktory from running in the background'),
        const SizedBox(height: Space.s),
        Text(
          'Claktory runs your agents as a background service, and macOS '
          'has turned that off. To let it run, open System Settings and '
          'turn on Claktory under Allow in the Background. '
          '$backgroundItemNote Claktory starts as soon as you do.',
          style: body,
        ),
        const SizedBox(height: Space.xl),
        // No Not now: nothing here runs until it's allowed.
        buttons([
          Button(
            label: 'Open System Settings',
            primary: true,
            onPressed: () => unawaited(setup.openLoginItems()),
          ),
        ]),
      ],
      SetupStage.missing => [
        title('Download Claktory again'),
        const SizedBox(height: Space.s),
        Text(
          defaultTargetPlatform == TargetPlatform.windows
              ? "This copy of Claktory is missing some files, so it can't set "
                    'up this PC.'
              : "This copy is incomplete, so it can't set up this Mac.",
          style: body,
        ),
        const SizedBox(height: Space.xl),
        buttons([_download()]),
      ],
      SetupStage.checking || SetupStage.done => const <Widget>[],
    };
    return StepCard(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: children,
      ),
    );
  }

  /// Download, for an app to set up with.
  Widget _download() => Button(
    label: 'Download',
    primary: true,
    onPressed: () => unawaited(
      launchUrl(
        defaultTargetPlatform == TargetPlatform.windows
            ? downloadWindowsUrl
            : downloadMacUrl,
      ),
    ),
  );

  /// A quiet Show details, and the detail under it once opened.
  List<Widget> _detail(Palette palette) {
    final detail = widget.setup.detail.trim();
    if (detail.isEmpty) return const [];
    return [
      const SizedBox(height: Space.s),
      Align(
        alignment: Alignment.centerLeft,
        child: Pressable(
          onTap: () => setState(() => _details = !_details),
          builder: (context, hovered) => Text(
            _details ? 'Hide details' : 'Show details',
            style: TextStyles.caption.copyWith(
              color: hovered ? palette.text : palette.textMuted,
            ),
          ),
        ),
      ),
      if (_details) ...[
        const SizedBox(height: Space.xs),
        SelectableArea(
          child: Text(
            detail,
            style: TextStyles.caption.copyWith(color: palette.textFaint),
          ),
        ),
      ],
    ];
  }
}
