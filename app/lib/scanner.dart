import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

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

class _NativePreviewState extends State<_NativePreview> {
  MethodChannel? _channel;

  @override
  void dispose() {
    _channel?.setMethodCallHandler(null);
    super.dispose();
  }

  void _created(int id) {
    _channel = MethodChannel('claktory/scanner/$id')
      ..setMethodCallHandler((call) async {
        if (call.method == 'scanned' && call.arguments is String) {
          widget.onCode(call.arguments as String);
        }
      });
  }

  // The same view type and per-view channel on both; nowhere else (a Mac,
  // say) is there a native view to make.
  @override
  Widget build(BuildContext context) => switch (defaultTargetPlatform) {
    TargetPlatform.android => AndroidView(
      viewType: 'claktory/scanner',
      onPlatformViewCreated: _created,
    ),
    TargetPlatform.iOS => UiKitView(
      viewType: 'claktory/scanner',
      onPlatformViewCreated: _created,
    ),
    _ => const SizedBox.shrink(),
  };
}
