// Local Network on an iPhone or iPad: before the first connection to a
// Claktory, a short sheet says why Claktory asks, and its Continue brings
// iOS's prompt; declined, it says what that means (Claktory still connects
// through its relay, only more slowly on the same Wi-Fi, and not at all
// where the Claktory's network has no internet) and how to turn it on,
// noticing on its own when it is. Never a dead end: Not now goes on.
//
// iOS has no API that reads the permission, and any check asks the person
// when they haven't answered (ios/Runner/LocalNetwork.swift), so it's
// checked only here: in the sheet, and on coming back to it from Settings.
import 'dart:async';
import 'dart:io' show Platform;

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../backend.dart' show fireAndLog;
import '../connections.dart' show RemoteFactory;
import '../controls.dart';
import '../devices.dart' show thisDeviceKind;
import '../identity.dart';
import '../scope.dart';
import 'haptics.dart';
import 'metrics.dart' show Touch, TypeScale;
import 'presentation.dart' show presentSheet;

/// What the check found.
enum LocalNetworkAccess {
  granted,
  denied,

  /// It couldn't tell: no network to look on, no answer in time, or no
  /// native side.
  unknown,
}

/// iOS's Local Network permission, as far as it can be found out.
abstract interface class LocalNetwork {
  /// Whether it's allowed, asking the person first (iOS's prompt) when
  /// they haven't answered; the answer comes once they have.
  Future<LocalNetworkAccess> check();

  /// The app's page in Settings, where its Local Network switch is.
  Future<void> openSettings();
}

/// The device's own, through factory/device (LocalNetwork.swift).
class DeviceLocalNetwork implements LocalNetwork {
  const DeviceLocalNetwork();

  static const _channel = MethodChannel('factory/device');

  @override
  Future<LocalNetworkAccess> check() async {
    try {
      final answer = await _channel.invokeMethod<String>('localNetwork');
      return LocalNetworkAccess.values.asNameMap()[answer] ??
          LocalNetworkAccess.unknown;
    } on PlatformException {
      return LocalNetworkAccess.unknown;
    } on MissingPluginException {
      return LocalNetworkAccess.unknown;
    }
  }

  @override
  Future<void> openSettings() async {
    try {
      await _channel.invokeMethod<void>('openAppSettings');
    } on PlatformException {
      // Nothing to open: the sheet still says where it is.
    } on MissingPluginException {
      // The same.
    }
  }
}

/// This device's Local Network permission: an iPhone's or iPad's
/// (main.dart); null where there's none to ask for, so nothing is shown.
LocalNetwork? localNetwork;

/// Before this device's first connection to a Claktory (a code scanned or
/// pasted, one of the person's own from their account, a link): the
/// Local Network sheet, once it's answered or set aside. Nothing when the
/// device has a Claktory already (it was asked then, or iOS asks as it
/// always did) or no Local Network to ask for.
Future<void> localNetworkBeforeFirst(BuildContext context) async {
  final network = localNetwork;
  if (network == null) return;
  if (AppScope.scope(context).connections.saved.isNotEmpty) return;
  await showLocalNetwork(context, network);
}

/// Connects this device to [target], a Claktory it's just been given (a
/// code scanned or pasted, one of the person's own from their account, a
/// link), once the person said so: the one way a first connection is made,
/// so the Local Network sheet comes before it wherever it comes from.
Future<void> connectNewClaktory(
  BuildContext context,
  RemoteFactory target,
) async {
  await localNetworkBeforeFirst(context);
  if (context.mounted) AppScope.switchFactory(context, target);
}

/// The sheet itself, until it's done. Why Claktory asks has one way on,
/// Continue, which brings iOS's prompt (Apple's rule for a screen before
/// a system prompt); where a dialog can be set aside anyway (an iPad's,
/// with Esc or a tap beside it), iOS asks all the same. Declined, the
/// sheet says so, and setting it aside is Not now.
Future<void> showLocalNetwork(
  BuildContext context,
  LocalNetwork network,
) async {
  // Not disposed: the sheet still listens to it as it slides away, after
  // this returns; it goes with the sheet.
  final stage = ValueNotifier(_Stage.explain);
  await _present(context, network, stage);
  if (stage.value == _Stage.explain && context.mounted) {
    // Set aside before Continue: iOS asks now, and declined, the sheet
    // says what that means.
    if (await network.check() == LocalNetworkAccess.denied && context.mounted) {
      stage.value = _Stage.off;
      await _present(context, network, stage);
    }
  }
}

Future<void> _present(
  BuildContext context,
  LocalNetwork network,
  ValueNotifier<_Stage> stage,
) {
  Widget sheet(VoidCallback close) =>
      _LocalNetworkSheet._(network: network, stage: stage, onDone: close);
  if (isPhoneLayout(context)) {
    return presentSheet<void>(
      context,
      closeButton: false,
      // Why it asks can't be swiped away, nor read out as closing (no
      // grabber); declined, swiping is Not now.
      shouldClose: () async => stage.value == _Stage.off,
      held: _Held(stage),
      builder: (context, close) => sheet(close),
    );
  }
  return showModal(context, (context, close) => sheet(close));
}

/// The app's name in Settings: Claktory Dev for any build but the App
/// Store's (#501).
String get _appName => kReleaseMode ? 'Claktory' : 'Claktory Dev';

/// Where the app's Local Network switch is: under Apps in Settings since
/// iOS 18, at Settings' top level before.
String get _settingsPath {
  final major = int.tryParse(
    RegExp(r'(\d+)\.').firstMatch(Platform.operatingSystemVersion)?[1] ?? '',
  );
  final apps = major == null || major >= 18 ? 'Apps → ' : '';
  return 'Settings → $apps$_appName → Local Network';
}

enum _Stage {
  explain,

  /// iOS is asking.
  asking,
  off,

  /// Allowed, set aside, or not to be known.
  done,
}

/// Why Claktory asks, then iOS's prompt; declined, what that means and how
/// to turn it on.
class _LocalNetworkSheet extends StatefulWidget {
  const _LocalNetworkSheet._({
    required this.network,
    required this.stage,
    required this.onDone,
  });

  final LocalNetwork network;
  final ValueNotifier<_Stage> stage;
  final VoidCallback onDone;

  @override
  State<_LocalNetworkSheet> createState() => _LocalNetworkSheetState();
}

class _LocalNetworkSheetState extends State<_LocalNetworkSheet>
    with WidgetsBindingObserver {
  _Stage get _stage => widget.stage.value;

  set _stage(_Stage stage) {
    setState(() => widget.stage.value = stage);
  }

  /// A check is running (one at a time: iOS answers each once).
  var _checking = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  // Back from Settings: turned on, it goes on by itself.
  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed && _stage == _Stage.off) {
      unawaited(_check());
    }
  }

  void _done() {
    _stage = _Stage.done;
    widget.onDone();
  }

  Future<void> _check() async {
    if (_checking) return;
    _checking = true;
    if (_stage == _Stage.explain) _stage = _Stage.asking;
    final access = await widget.network.check();
    _checking = false;
    if (!mounted) return;
    switch (access) {
      case LocalNetworkAccess.granted:
        unawaited(Haptics.success());
        _done();
      case LocalNetworkAccess.denied:
        if (_stage != _Stage.off) unawaited(Haptics.warning());
        _stage = _Stage.off;
      case LocalNetworkAccess.unknown:
        // Nothing to say for sure: on, as connecting would find out.
        if (_stage != _Stage.off) _done();
    }
  }

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final device = thisDeviceKind;
    final off = _stage == _Stage.off;
    final phone = isPhoneLayout(context);
    final scale = MediaQuery.textScalerOf(context).scale(1);
    // Centred while it's a few lines; larger, it reads from the start, and
    // at the accessibility sizes the glyph gives the words its room.
    final start = scale >= 1.3;
    final align = start ? TextAlign.start : TextAlign.center;
    final title = off
        ? 'Local Network is off'
        : 'Find your Claktory on this network';
    final body = off
        ? "Claktory still connects through Claktory's servers: more "
              'slowly on the same Wi-Fi as your Claktory, and not at all '
              'if that Wi-Fi has no internet.'
        : 'On the same Wi-Fi as your Claktory, this $device can connect '
              'to it directly, which is faster. Your $device asks for '
              'permission next.';
    final words = Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: start
          ? CrossAxisAlignment.start
          : CrossAxisAlignment.center,
      children: [
        if (scale < 2) ...[
          ExtensionGlyph(
            icon: 'network',
            name: '',
            color: palette.textMuted,
            size: 44,
          ),
          const SizedBox(height: Space.l),
        ],
        Semantics(
          header: true,
          child: Text(
            title,
            textAlign: align,
            style: type.title.copyWith(color: palette.text),
          ),
        ),
        const SizedBox(height: Space.m),
        Text(
          body,
          textAlign: align,
          style: type.body.copyWith(color: palette.textMuted),
        ),
        if (off) ...[
          const SizedBox(height: Space.m),
          Text(
            'To turn it on: $_settingsPath.',
            textAlign: align,
            style: type.caption.copyWith(color: palette.textMuted),
          ),
        ],
      ],
    );
    final openSettings = Button(
      label: 'Open Settings',
      primary: true,
      onPressed: () => fireAndLog(widget.network.openSettings()),
    );
    final notNow = _QuietLink('Not now', onTap: _done);
    final proceed = Button(
      label: 'Continue',
      primary: true,
      onPressed: _stage == _Stage.asking ? null : () => unawaited(_check()),
    );
    // The way on at the foot, under the thumb on a phone; a dialog's
    // buttons at its end on an iPad, the quiet one first.
    final Widget buttons = phone
        ? Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: off ? [openSettings, Center(child: notNow)] : [proceed],
          )
        : Wrap(
            alignment: WrapAlignment.end,
            crossAxisAlignment: WrapCrossAlignment.center,
            spacing: Space.m,
            runSpacing: Space.s,
            children: off ? [notNow, openSettings] : [proceed],
          );
    void primary() {
      if (off) {
        fireAndLog(widget.network.openSettings());
      } else if (_stage == _Stage.explain) {
        unawaited(_check());
      }
    }

    // A keyboard's Return presses the way on, as an alert's default button
    // does; Esc, which closes a dialog, does nothing while it explains (the
    // way on is Continue) and is Not now once it's off.
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.enter): primary,
        const SingleActivator(LogicalKeyboardKey.numpadEnter): primary,
        const SingleActivator(LogicalKeyboardKey.escape): () {
          if (off) _done();
        },
      },
      child: Focus(autofocus: true, child: _layout(phone, off, words, buttons)),
    );
  }

  Widget _layout(bool phone, bool off, Widget words, Widget buttons) {
    final scale = MediaQuery.textScalerOf(context).scale(1);
    final start = scale >= 1.3;
    return Padding(
      padding: EdgeInsets.fromLTRB(
        phone ? Space.l : 0,
        phone ? Space.l : 0,
        phone ? Space.l : 0,
        phone ? Space.m : 0,
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // The words scroll at the largest sizes; the buttons stay.
          Flexible(
            child: SingleChildScrollView(
              child: Align(
                alignment: start ? Alignment.centerLeft : Alignment.center,
                child: words,
              ),
            ),
          ),
          SizedBox(height: off ? Space.l : Space.xl),
          buttons,
        ],
      ),
    );
  }
}

/// The quiet way out: a finger's height, its label a caption.
class _QuietLink extends StatelessWidget {
  const _QuietLink(this.label, {required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Semantics(
      button: true,
      label: label,
      onTap: onTap,
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => ConstrainedBox(
          constraints: const BoxConstraints(minHeight: Touch.target),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: Space.s),
            child: Center(
              widthFactor: 1,
              child: Text(
                label,
                style: TypeScale.of(context).body.copyWith(
                  color: hovered ? palette.text : palette.textMuted,
                  fontWeight: FontWeight.w500,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// Held while the sheet says why it asks (or iOS is asking): only
/// Continue goes on. It follows the stage; nothing of its own to let go.
class _Held implements ValueListenable<bool> {
  const _Held(this.stage);

  final ValueNotifier<_Stage> stage;

  @override
  bool get value =>
      stage.value == _Stage.explain || stage.value == _Stage.asking;

  @override
  void addListener(VoidCallback listener) => stage.addListener(listener);

  @override
  void removeListener(VoidCallback listener) => stage.removeListener(listener);
}
