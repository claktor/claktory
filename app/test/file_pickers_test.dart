// Which picker an app's .clapp (Settings → Apps) and a Claude export ask
// for: a touch device's own (factory/device pickAttachments: iPhone, iPad,
// Android), a Mac's open panel (factory/window pickFiles), as the message
// box chooses. These fake the channels: they prove the routing, not the
// system's pickers.
import 'dart:io';

import 'package:factory_app/apps.dart';
import 'package:factory_app/imports/import_view.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// The two channels, answering [paths] and recording what was asked.
class _Pickers {
  _Pickers(this.paths) {
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    for (final name in ['factory/device', 'factory/window']) {
      messenger.setMockMethodCallHandler(MethodChannel(name), (call) async {
        calls.add('$name ${call.method}');
        return paths;
      });
    }
  }

  final List<String> paths;
  final calls = <String>[];

  void dispose() {
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    for (final name in ['factory/device', 'factory/window']) {
      messenger.setMockMethodCallHandler(MethodChannel(name), null);
    }
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late Directory folder;
  late File clapp;

  setUp(() {
    // The real pickers, not a test's stand-in.
    appFilePicker = null;
    exportFilePicker = null;
    folder = Directory.systemTemp.createTempSync('pickers_test');
    clapp = File('${folder.path}/charts.clapp')..writeAsBytesSync([1, 2, 3]);
  });

  tearDown(() {
    debugDefaultTargetPlatformOverride = null;
    PhonePlatformPolicy.debugAndroidPolicy = false;
    folder.deleteSync(recursive: true);
  });

  for (final (name, platform, android, channel) in [
    ('Android', TargetPlatform.android, true, 'factory/device pickAttachments'),
    (
      'iPhone and iPad',
      TargetPlatform.iOS,
      false,
      'factory/device pickAttachments',
    ),
    ('a Mac', TargetPlatform.macOS, false, 'factory/window pickFiles'),
  ]) {
    test('$name: an app file through $channel, its bytes read', () async {
      debugDefaultTargetPlatformOverride = platform;
      PhonePlatformPolicy.debugAndroidPolicy = android;
      final pickers = _Pickers([clapp.path]);
      addTearDown(pickers.dispose);
      expect(await pickAppFile(), [1, 2, 3]);
      expect(pickers.calls, [channel]);
    });

    test('$name: the export through $channel, its path', () async {
      debugDefaultTargetPlatformOverride = platform;
      PhonePlatformPolicy.debugAndroidPolicy = android;
      final pickers = _Pickers([clapp.path]);
      addTearDown(pickers.dispose);
      expect(await pickExportFile(), clapp.path);
      expect(pickers.calls, [channel]);
    });

    test('$name: cancelled, nothing, and no other picker asked', () async {
      debugDefaultTargetPlatformOverride = platform;
      PhonePlatformPolicy.debugAndroidPolicy = android;
      final pickers = _Pickers([]);
      addTearDown(pickers.dispose);
      expect(await pickAppFile(), isNull);
      expect(await pickExportFile(), isNull);
      expect(pickers.calls, [channel, channel]);
    });
  }
}
