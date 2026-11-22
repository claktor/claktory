import 'dart:io';

import 'package:flutter/foundation.dart';

import 'android/device_identity.dart';
import 'protocol/factory/v1/factory.pb.dart';

/// What kind of device this app runs on, told to the backend in every
/// Hello so agents know where the person's messages come from (a desk, a
/// laptop, a phone) and fit their replies to it. Worked out once.
Future<DeviceKind> thisDeviceKindForHello() =>
    _kind ??= deviceKindOf(defaultTargetPlatform);

Future<DeviceKind>? _kind;

/// Runs a command and answers its output; null if it couldn't run.
typedef CommandRunner = Future<String?> Function(
  String command,
  List<String> args,
);

Future<String?> _run(String command, List<String> args) async {
  try {
    final result = await Process.run(command, args);
    return result.exitCode == 0 ? result.stdout as String : null;
  } on Exception {
    return null;
  }
}

/// iPhone or iPad from the screen's shorter side (an iPad's is 744 points
/// or more, an iPhone's at most 440); on a Mac, a laptop when it has a
/// built-in battery or is a MacBook, else a desktop.
@visibleForTesting
Future<DeviceKind> deviceKindOf(
  TargetPlatform platform, {
  CommandRunner run = _run,
  double? shortestSide,
}) async {
  switch (platform) {
    case TargetPlatform.iOS:
      final side = shortestSide ?? _shortestSide();
      return side >= 600
          ? DeviceKind.DEVICE_KIND_TABLET
          : DeviceKind.DEVICE_KIND_PHONE;
    case TargetPlatform.macOS:
      final battery = await run('/usr/bin/pmset', ['-g', 'batt']);
      if (battery != null && battery.contains('InternalBattery')) {
        return DeviceKind.DEVICE_KIND_LAPTOP;
      }
      final model = await run('/usr/sbin/sysctl', ['-n', 'hw.model']);
      if (model != null && model.trim().startsWith('MacBook')) {
        return DeviceKind.DEVICE_KIND_LAPTOP;
      }
      return DeviceKind.DEVICE_KIND_DESKTOP;
    case TargetPlatform.linux:
    case TargetPlatform.windows:
      return DeviceKind.DEVICE_KIND_DESKTOP;
    case TargetPlatform.android:
      return AndroidDeviceIdentity.forDisplayShortestSide(
        shortestSide ?? androidDisplayShortestSide(),
      ).kind;
    case TargetPlatform.fuchsia:
      return DeviceKind.DEVICE_KIND_UNSPECIFIED;
  }
}

/// The system this app runs on, told to the backend in every Hello, for
/// the counts of how people reach Claktory in its check-ins: "ios",
/// "ipados", "android", "macos", "linux" or "windows". Worked out once.
Future<String> thisPlatformForHello() async =>
    platformOf(defaultTargetPlatform, await thisDeviceKindForHello());

/// [platform]'s name for the backend; an iPad's ([kind] a tablet) is
/// iPadOS. Empty for a system Claktory doesn't run on.
@visibleForTesting
String platformOf(TargetPlatform platform, DeviceKind kind) =>
    switch (platform) {
      TargetPlatform.iOS =>
        kind == DeviceKind.DEVICE_KIND_TABLET ? 'ipados' : 'ios',
      TargetPlatform.android => 'android',
      TargetPlatform.macOS => 'macos',
      TargetPlatform.linux => 'linux',
      TargetPlatform.windows => 'windows',
      TargetPlatform.fuchsia => '',
    };

double _shortestSide() {
  final views = PlatformDispatcher.instance.views;
  if (views.isEmpty) return 0;
  final view = views.first;
  final size = view.physicalSize / view.devicePixelRatio;
  return size.shortestSide;
}

/// This device's own name, told to the backend in every Hello: a device
/// paired with a code made without a name takes it ("Matt's MacBook").
/// The Mac's Computer Name; on an iPhone or iPad, which keep their names
/// from apps, "iPhone" or "iPad". Worked out once.
Future<String> thisDeviceNameForHello() =>
    _name ??= deviceNameOf(defaultTargetPlatform);

Future<String>? _name;

/// This device's own name, worked out afresh: what it signs in to the
/// person's account as.
Future<String> thisDeviceName() => deviceNameOf(defaultTargetPlatform);

@visibleForTesting
Future<String> deviceNameOf(
  TargetPlatform platform, {
  CommandRunner run = _run,
  double? shortestSide,
}) async {
  switch (platform) {
    case TargetPlatform.macOS:
      final name = await run('/usr/sbin/scutil', ['--get', 'ComputerName']);
      final trimmed = name?.trim() ?? '';
      if (trimmed.isNotEmpty) return trimmed;
      return Platform.localHostname.replaceAll(RegExp(r'\.local$'), '');
    case TargetPlatform.iOS:
      final side = shortestSide ?? _shortestSide();
      return side >= 600 ? 'iPad' : 'iPhone';
    case TargetPlatform.linux:
    case TargetPlatform.windows:
      return Platform.localHostname;
    case TargetPlatform.android:
      return AndroidDeviceIdentity.forDisplayShortestSide(
        shortestSide ?? androidDisplayShortestSide(),
      ).name;
    case TargetPlatform.fuchsia:
      return '';
  }
}
