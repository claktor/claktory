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

/// The phone's own camera, through the native side (ios/Runner/Scanner.swift).
/// Elsewhere there's none.
class DeviceCamera implements Camera {
  const DeviceCamera();

  static const _channel = MethodChannel('factory/device');

  static bool get _here =>
      !kIsWeb && defaultTargetPlatform == TargetPlatform.iOS;

  @override
  Future<CameraAccess> access() async {
    if (!_here) return CameraAccess.unavailable;
    final access = await _channel.invokeMethod<String>('cameraAccess');
    return CameraAccess.values.asNameMap()[access] ?? CameraAccess.unavailable;
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

  @override
  Widget build(BuildContext context) =>
      UiKitView(viewType: 'claktory/scanner', onPlatformViewCreated: _created);
}
