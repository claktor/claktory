// Stopping and starting Claktory on this Mac from its app. Quitting the app
// (⌘Q, or closing its last window) stops Claktory first when the person
// chose that (Settings.when_app_quits): the native side asks "quitting" and
// waits a few seconds at most. The app menu has Stop Claktory while it runs
// under its background service (BackendService.can_stop), and Start
// Claktory while the person has it turned off. macos/Runner/Setup.swift's
// Engine is the other end of factory/engine. Starting goes through setting
// up ([MacSetup.startEngine]: the bundled `factoryd install --start`),
// since a stopped Claktory can't be asked anything.
import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;
import 'name_step.dart' show StepCard;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'settings_view.dart'
    show keepRunningLabel, quitModeNote, stopAgentsLabel;
import 'setup.dart';
import 'tell_rivet.dart' show ErrorLine;

/// The app menu's item, and quitting, for this Mac's own Claktory.
class EngineControl {
  EngineControl({
    required this.backend,
    required this.setup,
    this.channel = const MethodChannel('factory/engine'),
  }) {
    channel.setMethodCallHandler(_call);
    _watched.addListener(_update);
    _update();
  }

  /// This Mac's own Claktory.
  final Backend backend;

  /// Setting it up, which starts it.
  final MacSetup setup;

  final MethodChannel channel;

  /// How long quitting waits for Claktory to take the stop; the native
  /// side quits after a few seconds whatever happens.
  static const quitWithin = Duration(seconds: 4);

  late final _watched = Listenable.merge([
    backend.workspace,
    backend.connected,
    setup,
  ]);

  (String, bool)? _shown;
  var _shownOnce = false;

  BackendService get _service => backend.workspace.workspace.backendService;

  /// Running under its background service, and answering: it can stop.
  bool get _canStop => backend.connected.value && _service.canStop;

  Future<Object?> _call(MethodCall call) async {
    switch (call.method) {
      case 'quitting':
        await quitting();
        return null;
      case 'menuChosen':
        // A refusal's words: the native side shows them in an alert.
        return menuChosen();
    }
    throw MissingPluginException();
  }

  /// The app quits: Claktory stops first when the person chose that. The
  /// backend says when it has taken it; whatever happens, the app quits.
  Future<void> quitting() async {
    if (!_canStop) return;
    if (backend.workspace.workspace.settings.whenAppQuits !=
        QuitMode.QUIT_MODE_STOP) {
      return;
    }
    try {
      await backend.stopEngine(quitting: true).timeout(quitWithin);
    } on Object {
      // Quitting anyway: it was asked.
    }
  }

  /// Stop Claktory or Start Claktory, chosen in the app menu: why it was
  /// refused, or null.
  Future<String?> menuChosen() async {
    if (_canStop) {
      if (_service.stopping) return null;
      try {
        await backend.stopEngine(quitting: false);
      } on RequestFailure catch (failure) {
        return failure.message;
      }
    } else if (setup.stage == SetupStage.stopped) {
      await setup.startEngine();
    }
    return null;
  }

  /// The menu's item now: its title and whether it can be chosen; null
  /// hides it.
  (String, bool)? get item {
    if (_canStop) {
      return _service.stopping
          ? ('Stopping Claktory…', false)
          : ('Stop Claktory', true);
    }
    return switch (setup.stage) {
      SetupStage.stopped => ('Start Claktory', true),
      SetupStage.starting => ('Starting Claktory…', false),
      _ => null,
    };
  }

  void _update() {
    // Stopping as the person asked: the app says it's off once it is.
    if (_canStop && _service.stopping) setup.expectStop();
    final now = item;
    if (_shownOnce && now == _shown) return;
    _shownOnce = true;
    _shown = now;
    unawaited(_tell(now));
  }

  Future<void> _tell((String, bool)? item) async {
    try {
      await channel.invokeMethod<void>(
        'menu',
        item == null ? null : {'title': item.$1, 'enabled': item.$2},
      );
    } on MissingPluginException {
      // No native side (tests, other platforms): nothing to show.
    } on PlatformException {
      // The same.
    }
  }

  void dispose() {
    _watched.removeListener(_update);
    channel.setMethodCallHandler(null);
  }
}

/// Whether the first launch asks what quitting the app does: a new
/// Claktory its Mac's background service runs
/// (Onboarding.when_app_quits), in the app on that Mac. Other devices
/// don't ask: it's about this app quitting.
bool asksWhenAppQuits(BuildContext context) {
  if (defaultTargetPlatform != TargetPlatform.macOS) return false;
  final backend = AppScope.of(context);
  if (backend.target != null) return false;
  final store = backend.workspace;
  if (!backend.connected.value || !store.received) return false;
  return store.workspace.settings.onboarding.whenAppQuits ==
      OnboardingStep.ONBOARDING_STEP_NEEDED;
}

/// The first launch's question: keep agents running after the app quits,
/// or stop them. Settings → General changes it later.
class WhenAppQuitsStep extends StatefulWidget {
  const WhenAppQuitsStep({super.key});

  @override
  State<WhenAppQuitsStep> createState() => _WhenAppQuitsStepState();
}

class _WhenAppQuitsStepState extends State<WhenAppQuitsStep> {
  QuitMode? _choosing;
  String? _error;

  Future<void> _choose(QuitMode mode) async {
    final backend = AppScope.of(context);
    setState(() {
      _choosing = mode;
      _error = null;
    });
    try {
      // Answered, the step is done on every device.
      await backend.updateSettings(UpdateSettings(whenAppQuits: mode));
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _error = failure.message);
    }
    if (mounted) setState(() => _choosing = null);
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final body = type.body.copyWith(color: palette.textMuted);
    Widget choice(String label, QuitMode mode) => Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(label, style: type.body.copyWith(color: palette.text)),
        const SizedBox(height: 2),
        Text(quitModeNote(mode), style: body),
      ],
    );
    return StepCard(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'When you quit Claktory',
            style: type.emphasis.copyWith(color: palette.text),
          ),
          const SizedBox(height: Space.s),
          Text(
            'Choose what happens to your agents when you quit the app.',
            style: body,
          ),
          const SizedBox(height: Space.l),
          choice(keepRunningLabel, QuitMode.QUIT_MODE_KEEP_RUNNING),
          const SizedBox(height: Space.m),
          choice(stopAgentsLabel, QuitMode.QUIT_MODE_STOP),
          const SizedBox(height: Space.m),
          Text(
            'You can change this at any time in Settings → General.',
            style: type.caption.copyWith(color: palette.textFaint),
          ),
          const SizedBox(height: Space.xs),
          // Where macOS lets it be turned off, and what that does.
          Text(
            'macOS lists Claktory Agents under Login Items. If you turn it '
            'off there, your agents stop when you quit Claktory.',
            style: type.caption.copyWith(color: palette.textFaint),
          ),
          if (_error case final error?) ...[
            const SizedBox(height: Space.s),
            ErrorLine(error, about: 'Choosing what quitting does:'),
          ],
          const SizedBox(height: Space.xl),
          Wrap(
            alignment: WrapAlignment.end,
            spacing: Space.s,
            runSpacing: Space.s,
            children: [
              Button(
                label: stopAgentsLabel,
                onPressed: _choosing != null
                    ? null
                    : () => unawaited(_choose(QuitMode.QUIT_MODE_STOP)),
              ),
              Button(
                label: keepRunningLabel,
                primary: true,
                onPressed: _choosing != null
                    ? null
                    : () => unawaited(_choose(QuitMode.QUIT_MODE_KEEP_RUNNING)),
              ),
            ],
          ),
        ],
      ),
    );
  }
}
