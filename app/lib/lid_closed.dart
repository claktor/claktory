// Keeping a laptop's agents working with its lid closed (#371,
// coordination/security/lid-closed.md): Settings → General → Sleep's
// switch for it, its own question before it's turned on, the helper's
// real state (Workspace.keep_awake.lid_closed), and the Mac app's side of
// installing the helper (LidHelper.swift's factory/lid_helper channel).
// Only the Mac app on the Mac Claktory runs on installs or removes it; any
// other device shows its state, and turning it on there says to do it from
// the Mac.
import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'controls.dart';
import 'forms.dart';
import 'identity.dart';
import 'keep_awake_card.dart' show keepAwakeMac;
import 'mobile/metrics.dart' show TypeScale;
import 'protocol/factory/v1/factory.pb.dart';

/// What macOS says of the helper (SMAppService.Status), as the channel
/// names it; [unavailable] before macOS 14 and off the Mac. [notFound] is
/// read from the bundle (no helper in it: Claktory Dev, ad hoc builds); one
/// carried but never registered is [notRegistered].
enum LidHelperStatus {
  unavailable,
  notFound,
  notRegistered,
  requiresApproval,
  enabled;

  static LidHelperStatus parse(String? name) => switch (name) {
    'notFound' => notFound,
    'notRegistered' => notRegistered,
    'requiresApproval' => requiresApproval,
    'enabled' => enabled,
    _ => unavailable,
  };

  /// This app's bundle carries a helper it can install.
  bool get present => this != unavailable && this != notFound;

  /// Installed, approved or waiting for approval: turning it off removes it.
  bool get registered => this == enabled || this == requiresApproval;

  /// What the app tells the backend (SetLidClosed.registration).
  LidClosedRegistration get registration => switch (this) {
    enabled => LidClosedRegistration.LID_CLOSED_REGISTRATION_ENABLED,
    requiresApproval =>
      LidClosedRegistration.LID_CLOSED_REGISTRATION_REQUIRES_APPROVAL,
    notRegistered =>
      LidClosedRegistration.LID_CLOSED_REGISTRATION_NOT_REGISTERED,
    notFound => LidClosedRegistration.LID_CLOSED_REGISTRATION_NOT_FOUND,
    unavailable => LidClosedRegistration.LID_CLOSED_REGISTRATION_UNKNOWN,
  };
}

/// macOS refused to install or remove the helper: its words.
class LidHelperFailure implements Exception {
  const LidHelperFailure(this.message);
  final String message;
  @override
  String toString() => message;
}

/// The Mac app's channel to SMAppService for the helper (LidHelper.swift).
class LidHelper {
  const LidHelper([this.channel = const MethodChannel('factory/lid_helper')]);

  final MethodChannel channel;

  Future<LidHelperStatus> status() async {
    if (defaultTargetPlatform != TargetPlatform.macOS) {
      return LidHelperStatus.unavailable;
    }
    try {
      return LidHelperStatus.parse(
        await channel.invokeMethod<String>('status'),
      );
    } on MissingPluginException {
      return LidHelperStatus.unavailable;
    } on PlatformException {
      return LidHelperStatus.unavailable;
    }
  }

  /// Installs it; macOS then waits for the person's approval
  /// ([LidHelperStatus.requiresApproval]). Throws [LidHelperFailure].
  Future<LidHelperStatus> register() => _change('register');

  /// Removes it. Throws [LidHelperFailure].
  Future<LidHelperStatus> unregister() => _change('unregister');

  /// System Settings → General → Login Items, where the person allows it.
  Future<void> openLoginItems() async {
    try {
      await channel.invokeMethod<void>('openLoginItems');
    } on MissingPluginException {
      // Nowhere to open it.
    }
  }

  Future<LidHelperStatus> _change(String method) async {
    try {
      return LidHelperStatus.parse(await channel.invokeMethod<String>(method));
    } on PlatformException catch (error) {
      throw LidHelperFailure(error.message ?? 'macOS gave no reason.');
    } on MissingPluginException {
      throw const LidHelperFailure('This Mac app has no helper to install.');
    }
  }
}

/// The helper's channel; tests put a fake in its place.
LidHelper lidHelper = const LidHelper();

/// Installs or removes under way here: [LidClosedWatcher] waits for them,
/// so it doesn't undo a step half done.
var _changing = 0;

Future<T> _guarded<T>(Future<T> Function() change) async {
  _changing++;
  try {
    return await change();
  } finally {
    _changing--;
  }
}

/// This app is the Mac app on the Mac [backend]'s Claktory runs on: the
/// only place the helper is installed or removed.
bool lidClosedHere(Backend backend) =>
    defaultTargetPlatform == TargetPlatform.macOS &&
    backend.localBackend &&
    backend.target == null;

// The words (Trish's to polish).

const lidClosedLabel = 'Keep agents working with the lid closed';
const lidClosedNote =
    "Only while it's plugged in. It sleeps on low battery or when it gets "
    'too hot.';
const lidClosedFromMac = 'Turn it on from the Mac.';

const lidClosedQuestionTitle = 'Keep agents working with the lid closed?';
String lidClosedQuestionBody(Backend backend) =>
    '${_capital(keepAwakeMac(backend))} stays awake while agents work and '
    "it's plugged in, even with its lid closed.";
const lidClosedQuestionBag =
    "Don't put it in a bag while it's on: a Mac that stays awake with its "
    'lid closed gets hot. Claktory lets it sleep when the lid closes on '
    'battery, when the battery drops to 20%, and when it gets too hot.';
const lidClosedQuestionPassword =
    'macOS asks you to allow Claktory in System Settings → Login Items, '
    'with an administrator password.';
const lidClosedQuestionOff = 'You can turn it off here at any time.';

/// What the setting says under its switch, by where it stands; null for
/// nothing. [here]: on the Mac it's installed from.
({String text, bool holding, bool failed})? lidClosedStatus(
  Backend backend,
  KeepAwake keep, {
  required bool here,
}) {
  final mac = keepAwakeMac(backend);
  final status = switch (keep.lidClosed) {
    LidClosedState.LID_CLOSED_NEEDS_APPROVAL =>
      here
          ? 'Waiting for you to allow Claktory in System Settings → Login '
                'Items.'
          : 'Waiting for you to allow Claktory in System Settings on $mac.',
    LidClosedState.LID_CLOSED_ON =>
      keep.lidClosedHolding
          ? 'Keeping $mac awake now, even with its lid closed'
          : 'On: agents keep working while $mac is plugged in, even with its '
                'lid closed.',
    LidClosedState.LID_CLOSED_PAUSED_BATTERY =>
      'Paused: the battery is low. It starts again at 30% or when you plug '
          '$mac in.',
    LidClosedState.LID_CLOSED_PAUSED_HEAT =>
      'Paused: $mac is too hot. It starts again 10 minutes after it cools '
          'down.',
    LidClosedState.LID_CLOSED_PAUSED_LID_ON_BATTERY =>
      'Paused: the lid is closed on battery. It starts again when you plug '
          '$mac in.',
    LidClosedState.LID_CLOSED_MISSING =>
      "Claktory's helper isn't running: it may have been turned off in "
          'System Settings → Login Items.'
          '${here ? '' : ' Open Claktory on $mac to install it again.'}',
    LidClosedState.LID_CLOSED_OUTDATED =>
      "Claktory's helper is from another version of Claktory."
          '${here ? '' : ' Open Claktory on $mac to update it.'}',
    LidClosedState.LID_CLOSED_FAILED =>
      keep.lidClosedError.isEmpty
          ? "Couldn't install Claktory's helper."
          : "Couldn't install Claktory's helper: ${keep.lidClosedError}",
    _ => null,
  };
  if (status == null) return null;
  return (
    text: status,
    holding:
        keep.lidClosed == LidClosedState.LID_CLOSED_ON && keep.lidClosedHolding,
    failed: keep.lidClosed == LidClosedState.LID_CLOSED_FAILED,
  );
}

String _capital(String words) =>
    words.isEmpty ? words : words[0].toUpperCase() + words.substring(1);

/// Installs the helper (again, when it's there: macOS wants a changed one
/// removed first) and tells the backend what macOS said. With [ask], the
/// person chose it: System Settings opens where they allow it. Throws
/// [LidHelperFailure] when macOS refuses, having told the backend.
Future<void> installLidHelper(Backend backend, {bool ask = true}) =>
    _guarded(() async {
      final LidHelperStatus status;
      try {
        if ((await lidHelper.status()).registered) {
          await lidHelper.unregister();
        }
        status = await lidHelper.register();
      } on LidHelperFailure catch (failure) {
        await backend.setLidClosed(
          on: true,
          registration: LidClosedRegistration.LID_CLOSED_REGISTRATION_FAILED,
          error: failure.message,
        );
        rethrow;
      }
      await backend.setLidClosed(on: true, registration: status.registration);
      if (ask && status == LidHelperStatus.requiresApproval) {
        await lidHelper.openLoginItems();
      }
    });

/// Turns it off: the backend lets go first, then (on the Mac) the helper
/// is removed and the backend told so (lid-closed.md I5). Throws
/// [LidHelperFailure] when macOS won't remove it, having told the backend.
Future<void> removeLidHelper(Backend backend) => _guarded(() async {
  await backend.setLidClosed(on: false);
  if (!lidClosedHere(backend)) return;
  try {
    final status = await lidHelper.unregister();
    await backend.setLidClosed(on: false, registration: status.registration);
  } on LidHelperFailure catch (failure) {
    await backend.setLidClosed(
      on: false,
      registration: LidClosedRegistration.LID_CLOSED_REGISTRATION_FAILED,
      error: failure.message,
    );
    rethrow;
  }
});

/// Asks before turning it on (its own question), and turns it on if the
/// person says so.
Future<void> askLidClosed(BuildContext context, Backend backend) => showModal(
  context,
  (context, close) => _LidClosedQuestion(close: close, backend: backend),
);

/// Its own question, before it's turned on (Matt's ruling: never bundled
/// with keeping the Mac awake): what it does, the bag, the limits, and the
/// administrator password macOS asks for. Turn on installs the helper and
/// opens System Settings where the person allows it.
class _LidClosedQuestion extends StatefulWidget {
  const _LidClosedQuestion({required this.close, required this.backend});

  final VoidCallback close;
  final Backend backend;

  @override
  State<_LidClosedQuestion> createState() => _LidClosedQuestionState();
}

class _LidClosedQuestionState extends State<_LidClosedQuestion> {
  var _busy = false;
  String? _error;

  Future<void> _turnOn() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    final backend = widget.backend;
    try {
      // The first install: macOS refusing leaves it off, saying why here.
      final status = await _guarded(() async {
        final status = await lidHelper.register();
        await backend.setLidClosed(on: true, registration: status.registration);
        return status;
      });
      widget.close();
      if (status == LidHelperStatus.requiresApproval) {
        await lidHelper.openLoginItems();
      }
    } on LidHelperFailure catch (failure) {
      if (mounted) {
        setState(() => _error = "Couldn't install Claktory's helper: $failure");
      }
    } on RequestFailure catch (failure) {
      // Installed, but the backend refused: removed again.
      await _guarded(() async {
        try {
          await lidHelper.unregister();
        } on LidHelperFailure {
          // Its line in Settings says it's there when it's on.
        }
      });
      if (mounted) setState(() => _error = failure.message);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    Widget line(String text) => Padding(
      padding: const EdgeInsets.only(bottom: Space.m),
      child: Text(text, style: type.body.copyWith(color: palette.text)),
    );
    return FormFrame(
      title: lidClosedQuestionTitle,
      body: lidClosedQuestionBody(widget.backend),
      fields: [
        line(lidClosedQuestionBag),
        line(lidClosedQuestionPassword),
        line(lidClosedQuestionOff),
      ],
      error: _error,
      submitLabel: 'Turn on',
      cancelLabel: 'Not now',
      onSubmit: _busy ? null : () => unawaited(_turnOn()),
      onCancel: widget.close,
    );
  }
}

/// On the Mac Claktory runs on, keeps the backend told what macOS says of
/// the helper while the app is open: once at the start, whenever it comes
/// back to the front (the person allowed it in System Settings, or turned
/// it off there), every few seconds while it waits for approval or is
/// missing, and when where it stands changes. It also finishes what the
/// person did elsewhere: turned off from a phone, the helper is removed
/// here; from another version (OUTDATED), installed again, once a session.
class LidClosedWatcher {
  LidClosedWatcher({required this.local, required Listenable changes})
    : _changes = changes {
    changes.addListener(_attach);
    _lifecycle = AppLifecycleListener(onResume: () => unawaited(_check()));
    _attach();
  }

  /// This Mac's own Claktory, when it runs one.
  final Backend? Function() local;
  final Listenable _changes;
  late final AppLifecycleListener _lifecycle;

  Backend? _backend;
  LidClosedState? _state;
  LidHelperStatus? _reported;
  Timer? _poll;
  var _checking = false;
  var _reinstalled = false;

  /// How often it asks macOS while it waits for the person's approval.
  static const pollEvery = Duration(seconds: 5);

  void _attach() {
    final backend = local();
    if (identical(backend, _backend)) return;
    _backend?.workspace.removeListener(_changed);
    _backend = backend;
    _state = null;
    _reported = null;
    backend?.workspace.addListener(_changed);
    _changed();
  }

  void _changed() {
    final backend = _backend;
    final keep = backend?.workspace.workspace.keepAwake;
    if (backend == null || keep == null || !keep.lidClosedAvailable) {
      _poll?.cancel();
      _poll = null;
      return;
    }
    final waits =
        keep.lidClosed == LidClosedState.LID_CLOSED_NEEDS_APPROVAL ||
        keep.lidClosed == LidClosedState.LID_CLOSED_MISSING;
    if (waits) {
      _poll ??= Timer.periodic(pollEvery, (_) => unawaited(_check()));
    } else {
      _poll?.cancel();
      _poll = null;
    }
    if (keep.lidClosed != _state) {
      _state = keep.lidClosed;
      unawaited(_check());
    }
  }

  /// Asks macOS, and tells the backend what changed.
  Future<void> _check() async {
    final backend = _backend;
    if (backend == null || _checking || _changing > 0) return;
    final keep = backend.workspace.workspace.keepAwake;
    if (!keep.lidClosedAvailable) return;
    _checking = true;
    try {
      final status = await lidHelper.status();
      if (!status.present || _changing > 0) return;
      final state = backend.workspace.workspace.keepAwake.lidClosed;
      if (state == LidClosedState.LID_CLOSED_OFF) {
        // Turned off elsewhere (a phone): removed here.
        if (status.registered) {
          await _guarded(() async {
            final now = await lidHelper.unregister();
            await backend.setLidClosed(
              on: false,
              registration: now.registration,
            );
          });
        }
        _reported = null;
        return;
      }
      if (state == LidClosedState.LID_CLOSED_OUTDATED && !_reinstalled) {
        _reinstalled = true;
        await installLidHelper(backend, ask: false);
        _reported = await lidHelper.status();
        return;
      }
      if (status != _reported) {
        await backend.setLidClosed(on: true, registration: status.registration);
        _reported = status;
      }
    } on LidHelperFailure {
      // Told the backend where it could; its line in Settings shows it.
    } on RequestFailure {
      // Told again at the next check.
    } finally {
      _checking = false;
    }
  }

  void dispose() {
    _changes.removeListener(_attach);
    _backend?.workspace.removeListener(_changed);
    _poll?.cancel();
    _lifecycle.dispose();
  }
}
