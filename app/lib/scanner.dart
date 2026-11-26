import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'identity.dart' show Space, TextStyles;

/// Whether the app may use the camera.
enum CameraAccess {
  granted,

  /// Not asked yet: asking shows the system's prompt.
  undetermined,

  /// The person said no; only Settings changes it now.
  denied,

  /// Not theirs to allow (Screen Time, a managed phone).
  restricted,

  /// No camera here, such as in the Simulator or on a Mac.
  unavailable,
}

/// The camera that scans a pairing code's QR code on a phone.
abstract interface class Camera {
  Future<CameraAccess> access();

  /// Asks for the camera (the system's prompt, saying why); whether it's
  /// allowed.
  Future<bool> request();

  /// The app's page in the system's Settings, where the camera is allowed.
  Future<void> openSettings();

  /// The camera's picture, calling [onCode] with the text of each QR code
  /// it reads.
  Widget preview(ValueChanged<String> onCode);
}

/// The phone's own camera, through the native side (ios/Runner/Scanner.swift
/// on an iPhone, android/…/QrScanner.kt on Android). Elsewhere there's none.
class DeviceCamera implements Camera {
  const DeviceCamera();

  static const _channel = MethodChannel('factory/device');

  static bool get _here =>
      !kIsWeb &&
      (defaultTargetPlatform == TargetPlatform.iOS ||
          defaultTargetPlatform == TargetPlatform.android);

  @override
  Future<CameraAccess> access() async {
    if (!_here) return CameraAccess.unavailable;
    // A native side that doesn't answer (none, or one without a camera
    // yet) is no camera: the code is pasted instead.
    try {
      final access = await _channel.invokeMethod<String>('cameraAccess');
      return CameraAccess.values.asNameMap()[access] ??
          CameraAccess.unavailable;
    } on PlatformException {
      return CameraAccess.unavailable;
    } on MissingPluginException {
      return CameraAccess.unavailable;
    }
  }

  @override
  Future<bool> request() async =>
      _here && (await _channel.invokeMethod<bool>('requestCamera') ?? false);

  @override
  Future<void> openSettings() async {
    if (_here) await _channel.invokeMethod<void>('openCameraSettings');
  }

  @override
  Widget preview(ValueChanged<String> onCode) => _NativePreview(onCode: onCode);
}

class _NativePreview extends StatefulWidget {
  const _NativePreview({required this.onCode});

  final ValueChanged<String> onCode;

  @override
  State<_NativePreview> createState() => _NativePreviewState();
}

/// What the camera's square says while iPadOS keeps the camera from the
/// app ([reason] as the native view sends it): sharing the screen (Split
/// View, Slide Over, Stage Manager beside another app), or another app
/// using it. Null for none.
@visibleForTesting
String? cameraUnavailableWords(Object? reason) => switch (reason) {
  'multitasking' =>
    "The camera isn't available in this window. Make Claktory full "
        'screen, or paste the code instead.',
  'inUse' =>
    'Another app is using the camera. Close it, or paste the code '
        'instead.',
  _ => null,
};

/// Sent up the tree by the camera's picture as the camera is kept from the
/// app and comes back: [unavailable] is what its square says, null once
/// the camera's back. A screen around it offers its way out (Paste code
/// instead) while it's unavailable.
class CameraAvailability extends Notification {
  const CameraAvailability(this.unavailable);

  final String? unavailable;
}

/// The camera's picture as [_NativePreview] shows it, with the words for
/// the camera kept from it laid over the black square. Public for tests,
/// which have no native view: [view] stands in for it, and [id] is the
/// channel's.
@visibleForTesting
class CameraPreviewFor extends StatefulWidget {
  const CameraPreviewFor({
    super.key,
    required this.view,
    required this.id,
    required this.onCode,
  });

  final Widget view;
  final int id;
  final ValueChanged<String> onCode;

  @override
  State<CameraPreviewFor> createState() => _PreviewState();
}

class _NativePreviewState extends State<_NativePreview> with _Previewing {
  @override
  ValueChanged<String> get onCode => widget.onCode;

  // The same view type and per-view channel on both; nowhere else (a Mac,
  // say) is there a native view to make.
  @override
  Widget build(BuildContext context) => over(switch (defaultTargetPlatform) {
    TargetPlatform.android => AndroidView(
      viewType: 'claktory/scanner',
      onPlatformViewCreated: created,
    ),
    TargetPlatform.iOS => UiKitView(
      viewType: 'claktory/scanner',
      onPlatformViewCreated: created,
    ),
    _ => const SizedBox.shrink(),
  });
}

class _PreviewState extends State<CameraPreviewFor> with _Previewing {
  @override
  ValueChanged<String> get onCode => widget.onCode;

  @override
  void initState() {
    super.initState();
    created(widget.id);
  }

  @override
  Widget build(BuildContext context) => over(widget.view);
}

/// The native view's channel: each code read, and whether the camera is
/// kept from the app now.
mixin _Previewing<T extends StatefulWidget> on State<T> {
  ValueChanged<String> get onCode;

  MethodChannel? _channel;

  /// Why the camera is kept from the app, as the native view says; null
  /// while it isn't.
  String? _unavailable;

  @override
  void dispose() {
    _channel?.setMethodCallHandler(null);
    super.dispose();
  }

  void created(int id) {
    _channel = MethodChannel('claktory/scanner/$id')
      ..setMethodCallHandler((call) async {
        switch (call.method) {
          case 'scanned' when call.arguments is String:
            onCode(call.arguments as String);
          case 'unavailable':
            final words = cameraUnavailableWords(
              (call.arguments as Map?)?['reason'],
            );
            if (mounted && words != _unavailable) _say(words);
          case 'available':
            if (mounted && _unavailable != null) _say(null);
        }
      });
  }

  /// Shows [words] over the picture, and tells the screen around it.
  void _say(String? words) {
    setState(() => _unavailable = words);
    CameraAvailability(words).dispatch(context);
  }

  /// [view], with what keeps the camera away said over it.
  Widget over(Widget view) {
    final words = _unavailable;
    return Stack(
      fit: StackFit.expand,
      children: [
        view,
        if (words != null)
          Semantics(
            liveRegion: true,
            child: ColoredBox(
              color: const Color(0xFF000000),
              child: Center(
                child: Padding(
                  padding: const EdgeInsets.all(Space.l),
                  child: Text(
                    words,
                    key: const ValueKey('camera-unavailable'),
                    textAlign: TextAlign.center,
                    style: TextStyles.body.copyWith(
                      color: const Color(0xFFFFFFFF),
                    ),
                  ),
                ),
              ),
            ),
          ),
      ],
    );
  }
}
