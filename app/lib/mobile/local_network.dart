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
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../connections.dart' show RemoteFactory;
import '../devices.dart' show thisDeviceKind;
import '../scope.dart';
import 'permission.dart';

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

/// Local Network as a [Permission]: iOS can't say without asking, so it's
/// unknown until the sheet's Continue checks, which brings iOS's prompt
/// when it hasn't been answered.
class LocalNetworkPermission extends Permission {
  const LocalNetworkPermission(this.network);

  final LocalNetwork network;

  @override
  PermissionWords words(BuildContext context) {
    final device = thisDeviceKind;
    return PermissionWords(
      glyph: 'network',
      title: 'Find your Claktory on this network',
      body:
          'On the same Wi-Fi as your Claktory, this $device can connect to '
          'it directly, which is faster. Your $device asks for permission '
          'next.',
      offTitle: 'Local Network is off',
      offBody:
          "Claktory still connects through Claktory's servers: more slowly "
          'on the same Wi-Fi as your Claktory, and not at all if that Wi-Fi '
          'has no internet.',
      offCaption: 'To turn it on: ${iosSettingsPath('Local Network')}.',
      stillWorks:
          "Claktory still connects through Claktory's servers: more slowly "
          'on the same Wi-Fi as your Claktory, and not at all if that Wi-Fi '
          'has no internet.',
    );
  }

  @override
  Future<PermissionAccess> access() async => PermissionAccess.unknown;

  @override
  Future<PermissionAccess> request() async => switch (await network.check()) {
    LocalNetworkAccess.granted => PermissionAccess.granted,
    LocalNetworkAccess.denied => PermissionAccess.denied,
    LocalNetworkAccess.unknown => PermissionAccess.unknown,
  };

  @override
  Future<void> openSettings() => network.openSettings();
}

/// The sheet itself, until it's done. Why Claktory asks has one way on,
/// Continue, which brings iOS's prompt (Apple's rule for a screen before
/// a system prompt); where a dialog can be set aside anyway (an iPad's,
/// with a tap beside it), iOS asks all the same. Declined, the sheet says
/// so, and setting it aside is Not now.
Future<void> showLocalNetwork(
  BuildContext context,
  LocalNetwork network,
) async {
  // Not disposed: the sheet still listens to it as it slides away, after
  // this returns; it goes with the sheet.
  final flow = PermissionFlow(
    LocalNetworkPermission(network),
    from: PermissionAccess.unknown,
  );
  await presentPermission(context, flow);
  if (flow.stage == PermissionStage.before && context.mounted) {
    // Set aside before Continue: iOS asks now, and declined, the sheet
    // says what that means.
    await flow.proceed();
    if (flow.stage == PermissionStage.declined && context.mounted) {
      await presentPermission(context, flow);
    }
  }
}
