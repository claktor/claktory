// Every test here runs as flutter_test's default platform, Android, unless
// it says otherwise; the app's tests are written for the iPhone and a Mac,
// so Android's own behavior (mobile/platform_policy.dart) applies only in a
// test that turns it on (PhonePlatformPolicy.debugAndroidPolicy = true).
import 'dart:async';

import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/voice_keep.dart';

Future<void> testExecutable(FutureOr<void> Function() testMain) async {
  PhonePlatformPolicy.debugAndroidPolicy = false;
  // No native folder for kept recordings here (its channel never answers
  // in a test): they live in the temporary folder, as before #401, unless
  // a test gives one.
  keptVoiceFolder = () async => null;
  await testMain();
}
