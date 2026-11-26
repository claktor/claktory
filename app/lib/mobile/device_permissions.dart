// The permissions a phone, iPad or Android device asks for, as the
// permission sheet ([Permission], permission.dart) needs them: the
// microphone (voice messages), the camera (scanning a pairing code) and
// notifications (iPhone and iPad). Local Network is local_network.dart's.
// Words are Trish's (ux/permissions.md §3); each names the device as
// itself.
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../devices.dart' show thisDeviceKind;
import '../controls.dart' show svgPath;
import '../identity.dart' show Space;
import '../recorder.dart' show MicAccess, VoiceRecorder, deviceRecorder;
import '../scanner.dart' show Camera, CameraAccess;
import 'permission.dart';
import 'popover.dart' show presentPopover;
import 'push.dart' show PushRegistrar;

/// Where the person turns a permission on, in a sentence: "Settings on
/// this iPhone" (Trish 44387), "Android's Settings", "System Settings".
String settingsAppName() => switch (defaultTargetPlatform) {
  TargetPlatform.iOS => 'Settings on this $thisDeviceKind',
  TargetPlatform.android => "Android's Settings",
  _ => 'System Settings',
};

/// "To turn it on: …" for [item], where the path is known (iOS); null
/// elsewhere, where the body already says where.
String? turnOnCaption(String item) =>
    defaultTargetPlatform == TargetPlatform.iOS
    ? 'To turn it on: ${iosSettingsPath(item)}.'
    : null;

/// The app's page in the system's Settings (factory/device), where all its
/// switches are; nothing where there's no native side.
Future<void> openAppSettingsPage() async {
  try {
    await const MethodChannel('factory/device')
        .invokeMethod<void>('openAppSettings');
  } on PlatformException {
    // Nothing to open: the sheet still says where it is.
  } on MissingPluginException {
    // The same.
  }
}

/// The way to Settings on this device.
String openSettingsLabel() => defaultTargetPlatform == TargetPlatform.macOS
    ? 'Open System Settings'
    : 'Open Settings';

/// The microphone, for voice messages (P5): a popover over the mic before
/// the first recording ([askMicrophone]).
class MicrophonePermission extends Permission {
  const MicrophonePermission(this.recorder);

  final VoiceRecorder recorder;

  @override
  PermissionWords words(BuildContext context) {
    final device = thisDeviceKind;
    return PermissionWords(
      glyph: 'mic',
      title: 'Talk instead of typing',
      body:
          'Your Claktory turns what you say into text itself. Your $device '
          'asks next.',
      offTitle: 'The microphone is off for Claktory',
      offBody:
          "Claktory can't use the microphone. Turn it on in "
          '${settingsAppName()}.',
      offCaption: turnOnCaption('Microphone'),
      openSettings: openSettingsLabel(),
    );
  }

  @override
  Future<PermissionAccess> access() async => switch (await recorder.access()) {
    MicAccess.granted => PermissionAccess.granted,
    MicAccess.undetermined => PermissionAccess.undetermined,
    MicAccess.denied => PermissionAccess.denied,
    MicAccess.restricted => PermissionAccess.restricted,
    MicAccess.unavailable => PermissionAccess.unavailable,
  };

  @override
  Future<PermissionAccess> request() async => await recorder.request()
      ? PermissionAccess.granted
      : PermissionAccess.denied;

  @override
  Future<void> openSettings() => recorder.openSettings();
}

/// The camera, for scanning a pairing code (P7): the scan screen explains
/// before the camera first starts, and refused, offers pasting the code.
class CameraPermission extends Permission {
  const CameraPermission(
    this.camera, {
    this.pairing = true,
    this.paste = true,
    this.photo = false,
  });

  final Camera camera;

  /// Taking a photo to send (the paperclip's Take Photo), not scanning.
  final bool photo;

  /// Scanning the code a Claktory shows to pair with it; else any code
  /// (someone's invite, a pipe's): its words don't say whose (Zucchero
  /// 44139).
  final bool pairing;

  /// Whether Paste code instead is beside it, so its words can offer it.
  final bool paste;

  @override
  PermissionWords words(BuildContext context) {
    final device = thisDeviceKind;
    if (photo) {
      return PermissionWords(
        glyph: 'camera',
        drawGlyph: (color, size) => CameraGlyph(color: color, size: size),
        title: 'Take a photo',
        body:
            'Claktory uses the camera when you take a photo to send, such '
            'as of a screen an agent should see. Your $device asks next.',
        offTitle: 'The camera is off for Claktory',
        offBody:
            'Turn it on in ${settingsAppName()}, or choose a photo you '
            'took already.',
        openSettings: openSettingsLabel(),
      );
    }
    return PermissionWords(
      glyph: 'camera',
      drawGlyph: (color, size) => CameraGlyph(color: color, size: size),
      title: 'Scan the code',
      body: pairing
          ? 'Claktory uses the camera to read the code your Claktory '
                'shows. Your $device asks next.'
          : 'Claktory uses the camera to read the code. '
                'Your $device asks next.',
      offTitle: 'The camera is off for Claktory',
      offBody: paste
          ? 'Turn it on in ${settingsAppName()}, or paste the code instead.'
          : 'Turn it on in ${settingsAppName()}.',
      openSettings: openSettingsLabel(),
    );
  }

  @override
  Future<PermissionAccess> access() async => switch (await camera.access()) {
    CameraAccess.granted => PermissionAccess.granted,
    CameraAccess.undetermined => PermissionAccess.undetermined,
    CameraAccess.denied => PermissionAccess.denied,
    CameraAccess.restricted => PermissionAccess.restricted,
    CameraAccess.unavailable => PermissionAccess.unavailable,
  };

  @override
  Future<PermissionAccess> request() async => await camera.request()
      ? PermissionAccess.granted
      : PermissionAccess.denied;

  @override
  Future<void> openSettings() => camera.openSettings();
}

/// Notifications (P3), after the device's first pairing: an iPhone's or
/// iPad's, through its push registrar.
class NotificationsPermission extends Permission {
  const NotificationsPermission(
    this.pushes, {
    this.openAppSettings = openAppSettingsPage,
  });

  final PushRegistrar pushes;

  /// The app's page in Settings, where its Notifications are.
  final Future<void> Function() openAppSettings;

  @override
  PermissionWords words(BuildContext context) {
    final device = thisDeviceKind;
    return PermissionWords(
      glyph: 'bell',
      title: 'Know when an agent needs you',
      body:
          'Claktory tells you when an agent needs an answer, finishes '
          "something you're waiting for, or gets stuck. Your $device asks "
          'next.',
      offTitle: 'Notifications are off',
      offBody: "You'll see what needs you only when you open Claktory.",
      stillWorks: "You'll see what needs you only when you open Claktory.",
      offCaption: turnOnCaption('Notifications'),
      openSettings: openSettingsLabel(),
    );
  }

  @override
  Future<PermissionAccess> access() async =>
      switch ((await pushes.refresh())?.permission) {
        'allowed' => PermissionAccess.granted,
        'denied' => PermissionAccess.denied,
        'not_determined' => PermissionAccess.undetermined,
        _ => PermissionAccess.unavailable,
      };

  @override
  Future<PermissionAccess> request() async =>
      (await pushes.ask())?.allowed == true
      ? PermissionAccess.granted
      : PermissionAccess.denied;

  @override
  Future<void> openSettings() => openAppSettings();
}

/// What [askMicrophone] found.
enum MicReady {
  /// Allowed already: record now.
  ready,

  /// Allowed just now, in the popover: the next tap or hold records (the
  /// finger that asked has long lifted).
  allowedNow,

  /// Refused: the mic's own line says so, with Open Settings.
  declined,

  /// Off, and Settings can't change it here.
  restricted,

  /// No microphone here.
  unavailable,

  /// The popover was set aside before Continue: nothing asked.
  setAside,
}

/// Text at least this large shows the mic's explainer as the sheet: a
/// popover can't hold its words with a full-height Continue.
const _popoverTextLimit = 1.35;

/// Before the first recording (P5): while the microphone hasn't been
/// asked, a popover over [anchor] (the mic, or the field a hold dictates
/// into) says why, and its Continue brings the system's prompt; at the
/// accessibility text sizes, the permission sheet instead. Either closes
/// once the system answers; a no is the mic's own line to say. Nothing
/// shows when it's been answered already.
Future<MicReady> askMicrophone(
  BuildContext anchor, {
  VoiceRecorder? recorder,
}) async {
  final permission = MicrophonePermission(recorder ?? deviceRecorder);
  final access = await permission.access();
  final known = _micReady(access);
  if (known != null) return known;
  if (!anchor.mounted) return MicReady.setAside;
  final flow = PermissionFlow(permission, from: access);
  if (MediaQuery.textScalerOf(anchor).scale(1) >= _popoverTextLimit) {
    await presentPermission(anchor, flow, untilAnswered: true);
  } else {
    await presentPopover<void>(
      anchor,
      builder: (context, close) => Padding(
        padding: const EdgeInsets.fromLTRB(Space.l, Space.m, Space.l, Space.m),
        child: PermissionContent(
          flow: flow,
          compact: true,
          untilAnswered: true,
          onDone: close,
        ),
      ),
    );
  }
  return switch (flow.result) {
    PermissionAccess.granted => MicReady.allowedNow,
    final now => _micReady(now) ?? MicReady.setAside,
  };
}

MicReady? _micReady(PermissionAccess access) => switch (access) {
  PermissionAccess.granted => MicReady.ready,
  PermissionAccess.denied => MicReady.declined,
  PermissionAccess.restricted => MicReady.restricted,
  PermissionAccess.unavailable => MicReady.unavailable,
  PermissionAccess.undetermined || PermissionAccess.unknown => null,
};

/// A camera, as Lucide draws it, in the app's line weight: the app's own
/// set has none yet.
class CameraGlyph extends StatelessWidget {
  const CameraGlyph({super.key, required this.color, this.size = 16});

  final Color color;
  final double size;

  @override
  Widget build(BuildContext context) =>
      CustomPaint(size: Size.square(size), painter: _CameraPainter(color));
}

class _CameraPainter extends CustomPainter {
  _CameraPainter(this.color);

  final Color color;

  static final _paths = [
    svgPath(
      'M14.5 4h-5L7 7H4a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V9'
      'a2 2 0 0 0-2-2h-3l-2.5-3z',
    ),
    svgPath('M9 13a3 3 0 1 0 6 0a3 3 0 1 0-6 0'),
  ];

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
    for (final path in _paths) {
      canvas.drawPath(path, paint);
    }
  }

  @override
  bool shouldRepaint(_CameraPainter old) => old.color != color;
}
