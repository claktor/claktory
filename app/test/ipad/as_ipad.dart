import 'package:factory_app/mobile/device.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

/// Runs the rest of this test as on an iPad: iOS, the pad idiom (so
/// touchFirst, a finger's sizes and the phone's type), reset afterwards.
/// Tests run as Android with the desktop policy otherwise
/// (test/flutter_test_config.dart).
void asIPad() {
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  debugDeviceCapabilities = const DeviceCapabilities(idiom: Idiom.pad);
  addTearDown(() {
    debugDefaultTargetPlatformOverride = null;
    debugDeviceCapabilities = null;
  });
}
