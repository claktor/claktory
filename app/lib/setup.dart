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

/// Whether this app sets this Mac up itself: the Mac's release build of
/// the real Claktory.
bool get setsUpThisMac =>
    defaultTargetPlatform == TargetPlatform.macOS &&
    kReleaseMode &&
    !isDevBuild;

/// Whether this device runs a Claktory of its own: a Mac does, unless
/// it's marked app only (`install.sh --app-only`, or the person chose a
/// Claktory on another computer) and has none installed. Claktory Dev
/// goes by the mark alone: the dev Claktory is always there.
Future<bool> runsClaktoryHere({
  SetupChannel channel = const SetupChannel(),
}) async {
  if (!hasLocalBackend) return false;
  if (defaultTargetPlatform != TargetPlatform.macOS) return true;
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
  /// damaged or failed.
  final String result;

  /// A sentence to show as it is.
  final String reason;

  /// Technical: an error, the log's path; for Show details.
  final String detail;

  /// The helpers' version.
  final String version;

  /// For already_installed: whether the backend answered then.
  final bool running;

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

  /// Moves the app to Applications and quits; the error's words when it
  /// can't.
  Future<String?> moveToApplications() =>
      _channel.invokeMethod<String>('moveToApplications');

  /// Whether this Mac has a Claktory installed already, for an app without
  /// helpers: its backend in ~/.factory/bin.
  bool installedHere() {
    final home = Platform.environment['HOME'] ?? '';
    return home.isNotEmpty && File('$home/.factory/bin/factoryd').existsSync();
  }
}

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

  /// It couldn't, or did only part of it.
  failed,

  /// Installed, but the backend doesn't answer.
  notRunning,

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
  var _disposed = false;

  /// Whether it puts a step over the window now. Once this Mac's Claktory
  /// answers, only the offer to move stays.
  bool get showing => switch (_stage) {
    SetupStage.offerMove || SetupStage.moving || SetupStage.choosing => true,
    SetupStage.settingUp ||
    SetupStage.failed ||
    SetupStage.notRunning ||
    SetupStage.missing => !connected.value,
    SetupStage.checking || SetupStage.done => false,
  };

  void _set(SetupStage stage) {
    if (_disposed) return;
    _stage = stage;
    notifyListeners();
  }

  void _connectedChanged() {
    if (!connected.value) return notifyListeners();
    _waitForStart?.cancel();
    // Connected after all: nothing left to fix.
    if (_stage == SetupStage.failed ||
        _stage == SetupStage.notRunning ||
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
    _set(SetupStage.settingUp);
    _settled(await channel.install(arguments));
  }

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

  void _settled(InstallResult result) {
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
        if (!result.running) _waitToStart();
      case 'incomplete':
        repairable = true;
        _set(SetupStage.failed);
      case 'not_running':
        repairable = true;
        _set(SetupStage.notRunning);
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
      _set(SetupStage.notRunning);
    });
  }

  @override
  void dispose() {
    _disposed = true;
    _waitForStart?.cancel();
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
          'Claktory runs your agents on this Mac. If it already runs on '
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
            label: 'Set up on this Mac',
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
      SetupStage.missing => [
        title('Download Claktory again'),
        const SizedBox(height: Space.s),
        Text(
          "This copy is incomplete, so it can't set up this Mac.",
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
    onPressed: () => unawaited(launchUrl(downloadMacUrl)),
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
