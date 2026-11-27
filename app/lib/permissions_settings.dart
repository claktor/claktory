// This device's permissions in Settings: in the device's own section (This
// Mac, This iPhone…), a row for each permission this device can ask for, with
// its state as the system says it now and at most one button: Turn on… while
// it hasn't been asked (the permission's sheet), Open Settings once it's off.
// Read again whenever the app comes back to the front, so a change made in
// the system's Settings shows at once. Only what the system lets the app read
// without asking: Files and Folders, which can't be, shows the backend's last
// check of each place macOS protects, on the Mac that runs the Claktory;
// Local Network joins once the backend keeps its last check.
import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart' show Backend;
import 'controls.dart';
import 'devices.dart' show thisDeviceKind;
import 'identity.dart';
import 'mobile/device_permissions.dart'
    show
        CameraPermission,
        MicrophonePermission,
        NotificationsPermission,
        openSettingsLabel;
import 'mobile/lines.dart' show ActionLine;
import 'mobile/metrics.dart' show TypeScale;
import 'mobile/permission.dart';
import 'mobile/push.dart' show PushRegistrar, notificationsRefusedOnce;
import 'notification_settings.dart' show DeviceNotifications;
import 'protected_places.dart'
    show ProtectedPlacePermission, protectedPlacesAskHere;
import 'protocol/factory/v1/factory.pb.dart';
import 'recorder.dart' show deviceRecorder;
import 'scope.dart';
import 'settings_view.dart' show SettingRow;

/// One row: its label, the permission, what it being off stops, and on
/// Android whether a first no left it still to ask ([refusedOnce]).
typedef _Row = ({
  String label,
  Permission permission,
  String stops,
  Future<bool> Function()? refusedOnce,
});

/// Android's word that the person said no once and it will still ask
/// (shouldShowRequestPermissionRationale), which its access reads as not
/// asked yet: false where the platform doesn't say.
Future<bool> _refused(String channel, String method) async {
  if (defaultTargetPlatform != TargetPlatform.android) return false;
  try {
    return await MethodChannel(channel).invokeMethod<bool>(method) ?? false;
  } on MissingPluginException {
    return false;
  } on PlatformException {
    return false;
  }
}

/// The Permissions rows, under their heading; nothing while none can be
/// read here.
class DevicePermissions extends StatefulWidget {
  const DevicePermissions({super.key});

  @override
  State<DevicePermissions> createState() => _DevicePermissionsState();
}

class _DevicePermissionsState extends State<DevicePermissions>
    with WidgetsBindingObserver {
  List<_Row>? _rows;

  /// What the system said of each, by label, once read.
  final _access = <String, PermissionAccess>{};

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_rows != null) return;
    _rows = _rowsHere(context);
    unawaited(_read());
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    // Back from the system's Settings, say: what it says now.
    if (state == AppLifecycleState.resumed) unawaited(_read());
  }

  List<_Row> _rowsHere(BuildContext context) {
    final platform = defaultTargetPlatform;
    final notifications = DeviceNotifications.of(context);
    final registrar = PushRegistrar.current;
    return [
      if (registrar != null)
        (
          label: 'Notifications',
          permission: NotificationsPermission(registrar),
          stops: _notificationsStop,
          refusedOnce: notificationsRefusedOnce,
        )
      else if (notifications != null)
        (
          label: 'Notifications',
          permission: _DeviceNotificationsPermission(notifications),
          stops: _notificationsStop,
          refusedOnce: null,
        ),
      (
        label: 'Microphone',
        permission: MicrophonePermission(deviceRecorder),
        stops: "You can't record voice messages or say your answers.",
        refusedOnce: () => _refused('factory/recorder', 'refusedOnce'),
      ),
      if (platform == TargetPlatform.iOS || platform == TargetPlatform.android)
        (
          label: 'Camera',
          permission: CameraPermission(
            AppScope.scope(context).camera,
            settings: true,
          ),
          stops: "You can't scan codes or take photos to send.",
          refusedOnce: () => _refused('factory/device', 'cameraRefusedOnce'),
        ),
    ];
  }

  /// Rows Android was refused once in and will still ask: Off, with
  /// Turn on… asking again (permissions spec §2.1).
  final _refusedOnce = <String>{};

  Future<void> _read() async {
    for (final row in _rows ?? const <_Row>[]) {
      final access = await row.permission.access();
      final refused =
          access == PermissionAccess.undetermined &&
          (await row.refusedOnce?.call() ?? false);
      if (!mounted) return;
      setState(() {
        _access[row.label] = access;
        if (refused) {
          _refusedOnce.add(row.label);
        } else {
          _refusedOnce.remove(row.label);
        }
      });
    }
  }

  Future<void> _turnOn(_Row row) async {
    await askPermission(context, row.permission);
    if (mounted) await _read();
  }

  /// Files and Folders: a row for each place the backend has checked, as
  /// it last found it ("Documents · On"), on the Mac whose switch it is;
  /// none before any check. Off opens System Settings at Files and Folders.
  List<({_Row row, PermissionAccess access})> _places(Backend backend) => [
    if (protectedPlacesAskHere(backend))
      for (final place in backend.workspace.workspace.protectedPlaces)
        if (switch (place.state) {
              ProtectedPlaceState.PROTECTED_PLACE_STATE_ALLOWED =>
                PermissionAccess.granted,
              ProtectedPlaceState.PROTECTED_PLACE_STATE_REFUSED =>
                PermissionAccess.denied,
              _ => null,
            }
            case final access?)
          (
            row: (
              label: place.name,
              permission: ProtectedPlacePermission(
                names: [place.name],
                usedBy: place.usedBy,
                read: () => PermissionAccess.denied,
              ),
              stops: [
                place.usedBy.isEmpty
                    ? "Your agents can't work there."
                    : "Your agents can't work in ${_and(place.usedBy)}.",
                'To turn it on: System Settings → Privacy & Security → Files '
                    'and Folders → Claktory Agents.',
              ].join(' '),
              refusedOnce: null,
            ),
            access: access,
          ),
  ];

  @override
  Widget build(BuildContext context) {
    final backend = AppScope.of(context);
    return ListenableBuilder(
      listenable: backend.workspace,
      builder: (context, _) => _build(context, backend),
    );
  }

  Widget _build(BuildContext context, Backend backend) {
    final shown = [
      for (final row in _rows ?? const <_Row>[])
        if (_access[row.label] case final access?)
          if (_readable(access)) (row: row, access: access),
      ..._places(backend),
    ];
    if (shown.isEmpty) return const SizedBox.shrink();
    final palette = Palette.of(context);
    return Padding(
      padding: const EdgeInsets.only(top: Space.m),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'Permissions',
            style: TextStyles.caption.copyWith(
              color: palette.textMuted,
              fontWeight: FontWeight.w500,
            ),
          ),
          const SizedBox(height: Space.s),
          // Each row its own node for VoiceOver and TalkBack, its button
          // apart: a row with no button doesn't take in the next one's
          // .
          for (final (:row, :access) in shown)
            _PermissionRow(
              key: ValueKey('permission:${row.label}'),
              label: row.label,
              access: access,
              refusedOnce: _refusedOnce.contains(row.label),
              stops: row.stops,
              onTurnOn: () => _turnOn(row),
              onOpenSettings: () => unawaited(row.permission.openSettings()),
            ),
        ],
      ),
    );
  }
}

/// "A", "A and B", "A, B and C".
String _and(List<String> names) => switch (names.length) {
  0 => '',
  1 => names.single,
  _ => '${names.sublist(0, names.length - 1).join(', ')} and ${names.last}',
};

const _notificationsStop =
    "You'll see what needs you only when you open Claktory.";

/// Whether a row says something true of [access]: no row for nothing to
/// ask for (no microphone, no camera), or for what can't be read.
bool _readable(PermissionAccess access) => switch (access) {
  PermissionAccess.unavailable || PermissionAccess.unknown => false,
  _ => true,
};

/// A row: the state in a word, at most one button, and a caption when
/// it's off or not allowed.
class _PermissionRow extends StatelessWidget {
  const _PermissionRow({
    super.key,
    required this.label,
    required this.access,
    this.refusedOnce = false,
    required this.stops,
    required this.onTurnOn,
    required this.onOpenSettings,
  });

  final String label;
  final PermissionAccess access;

  /// Android was told no once and will still ask: Off, with Turn on….
  final bool refusedOnce;
  final String stops;
  final VoidCallback onTurnOn;
  final VoidCallback onOpenSettings;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final (
      String state,
      Color color,
      Widget? action,
      String? caption,
    ) = switch (access) {
      PermissionAccess.undetermined when refusedOnce => (
        'Off',
        palette.danger,
        Button(label: 'Turn on…', onPressed: onTurnOn),
        stops,
      ),
      PermissionAccess.undetermined => (
        'Not asked yet',
        palette.textMuted,
        Button(label: 'Turn on…', onPressed: onTurnOn),
        null,
      ),
      PermissionAccess.granted => ('On', palette.text, null, null),
      PermissionAccess.restricted => (
        'Not allowed',
        palette.textMuted,
        null,
        restrictedFallback(),
      ),
      _ => (
        'Off',
        palette.danger,
        Button(label: openSettingsLabel(), onPressed: onOpenSettings),
        stops,
      ),
    };
    final said = Text(state, style: type.body.copyWith(color: color));
    final row = SettingRow(
      label: label,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (action == null) said else ActionLine(lead: said, action: action),
          if (caption != null) ...[
            const SizedBox(height: Space.xs),
            Text(
              caption,
              style: type.caption.copyWith(color: palette.textMuted),
            ),
          ],
        ],
      ),
    );
    return Semantics(container: true, explicitChildNodes: true, child: row);
  }
}

/// Notifications where the system shows them while Claktory is open (a
/// Mac, a Windows PC), through [DeviceNotifications]: the words an
/// iPhone's sheet has ([NotificationsPermission]), for this device.
class _DeviceNotificationsPermission extends Permission {
  const _DeviceNotificationsPermission(this.device);

  final DeviceNotifications device;

  @override
  PermissionWords words(BuildContext context) => PermissionWords(
    glyph: 'bell',
    title: 'Know when an agent needs you',
    body:
        'Claktory tells you when an agent needs an answer, finishes '
        "something you're waiting for, or gets stuck. Your $thisDeviceKind "
        'asks next.',
    offTitle: 'Notifications are off',
    offBody: "You'll see what needs you only when you open Claktory.",
    stillWorks: "You'll see what needs you only when you open Claktory.",
    openSettings: openSettingsLabel(),
  );

  @override
  Future<PermissionAccess> access() async => _of(await device.permission());

  @override
  Future<PermissionAccess> request() async => _of(await device.ask());

  @override
  Future<void> openSettings() => device.openSettings();

  static PermissionAccess _of(String permission) => switch (permission) {
    'allowed' => PermissionAccess.granted,
    'denied' => PermissionAccess.denied,
    'not_determined' => PermissionAccess.undetermined,
    _ => PermissionAccess.unavailable,
  };
}
