import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../ipad/columns.dart' show Idiom;

export '../ipad/columns.dart' show Idiom;

/// What the device the app runs on is and can do, from the native side
/// (`factory/device` → `capabilities`, once before the first frame), never
/// from the window's size: a narrow iPad window is still an iPad
/// (docs/ipad-workbench.md, §1). The shape is
/// app/test/ipad/fixtures/bridge/capabilities_*.json.
class DeviceCapabilities {
  const DeviceCapabilities({
    required this.idiom,
    this.iosAppOnMac = false,
    this.hardwareKeyboard = false,
    this.osVersion = '',
    this.raw = const {},
  });

  /// From `capabilities`' answer.
  factory DeviceCapabilities.fromMap(Map<Object?, Object?> map) =>
      DeviceCapabilities(
        idiom: switch (map['idiom']) {
          'pad' => Idiom.pad,
          'phone' => Idiom.phone,
          _ => Idiom.other,
        },
        iosAppOnMac: map['ios_app_on_mac'] == true,
        hardwareKeyboard: map['hardware_keyboard'] == true,
        osVersion: map['os_version'] as String? ?? '',
        raw: map,
      );

  final Idiom idiom;

  /// The iPad app running on a Mac with Apple silicon: a pointer and a
  /// keyboard, no touch.
  final bool iosAppOnMac;

  /// Whether a hardware keyboard was attached when asked
  /// ([hardwareKeyboard] follows it from then on).
  final bool hardwareKeyboard;
  final String osVersion;

  /// The whole answer, for what reads more of it (the Pencil's, the
  /// document's gates).
  final Map<Object?, Object?> raw;
}

const _channel = MethodChannel('factory/device');
const _keyboardEvents = EventChannel('factory/device/keyboard');

DeviceCapabilities? _loaded;

/// What this device is: read by [loadDeviceCapabilities] before the first
/// frame; until then, or where the native side doesn't say, worked out
/// from the platform and the screen ([fallbackCapabilities]).
DeviceCapabilities get deviceCapabilities =>
    debugDeviceCapabilities ?? _loaded ?? fallbackCapabilities();

/// In tests, what [deviceCapabilities] says.
@visibleForTesting
DeviceCapabilities? debugDeviceCapabilities;

/// Whether a hardware keyboard is attached now: on an iPad with one,
/// Return sends, as in Messages.
final ValueNotifier<bool> hardwareKeyboard = ValueNotifier(false);

StreamSubscription<Object?>? _keyboard;

/// How far an iPad window's controls (close, minimize and arrange, at its
/// top left) reach into the app, from the native side
/// (`factory/device/window_controls`): [leading], the room a toolbar's
/// leading items move right by to clear them, as Apple's apps' toolbars do
/// (UIKit's horizontal corner adaptation); [top], how much of the view's
/// top padding is theirs rather than the status bar's (the vertical
/// adaptation, which the native side adds to the safe area, so a screen
/// with no toolbar of its own starts below them). Zero where there are
/// none: an iPhone, a Mac, iPadOS before 26.
class WindowControls {
  const WindowControls({this.leading = 0, this.top = 0});

  static const none = WindowControls();

  final double leading;
  final double top;

  @override
  bool operator ==(Object other) =>
      other is WindowControls && other.leading == leading && other.top == top;

  @override
  int get hashCode => Object.hash(leading, top);
}

/// This window's controls, as the native side last said.
final ValueNotifier<WindowControls> windowControls = ValueNotifier(
  WindowControls.none,
);

const _windowControlsEvents = EventChannel('factory/device/window_controls');
StreamSubscription<Object?>? _windowControls;

/// Asks the native side what this device is, once, before the app's first
/// frame, so nothing lays out as another device first. Never fails: what
/// it can't learn it works out ([fallbackCapabilities]).
Future<void> loadDeviceCapabilities() async {
  if (_loaded != null) return;
  if (defaultTargetPlatform != TargetPlatform.iOS) {
    _loaded = fallbackCapabilities();
    return;
  }
  try {
    final answer = await _channel
        .invokeMapMethod<Object?, Object?>('capabilities')
        .timeout(const Duration(seconds: 1));
    _loaded = answer == null
        ? fallbackCapabilities()
        : DeviceCapabilities.fromMap(answer);
  } on Object {
    // An app built before `capabilities` (MissingPluginException), or a
    // native side that didn't answer in time.
    _loaded = fallbackCapabilities();
  }
  hardwareKeyboard.value = _loaded!.hardwareKeyboard;
  try {
    _keyboard ??= _keyboardEvents.receiveBroadcastStream().listen((event) {
      if (event is Map && event['hardware_keyboard'] is bool) {
        hardwareKeyboard.value = event['hardware_keyboard'] as bool;
      }
    }, onError: (Object _) {});
  } on Object {
    // No keyboard events from this native side.
  }
  try {
    _windowControls ??= _windowControlsEvents.receiveBroadcastStream().listen((
      event,
    ) {
      if (event is! Map) return;
      windowControls.value = WindowControls(
        leading: (event['leading'] as num?)?.toDouble() ?? 0,
        top: (event['top'] as num?)?.toDouble() ?? 0,
      );
    }, onError: (Object _) {});
  } on Object {
    // No window controls from this native side (an app built before them).
  }
}

/// What this device is without the native side's word: on iOS an iPad
/// when its screen's shorter side is 600 points or more (an iPhone's is at
/// most 440; the screen, not the window, which Split View narrows); a
/// phone on Android likewise; else neither.
DeviceCapabilities fallbackCapabilities() {
  final platform = defaultTargetPlatform;
  if (platform != TargetPlatform.iOS && platform != TargetPlatform.android) {
    return const DeviceCapabilities(idiom: Idiom.other);
  }
  final side = _screenShortestSide();
  if (side == null) return const DeviceCapabilities(idiom: Idiom.phone);
  return DeviceCapabilities(idiom: side >= 600 ? Idiom.pad : Idiom.phone);
}

double? _screenShortestSide() {
  final dispatcher = WidgetsBinding.instance.platformDispatcher;
  final displays = dispatcher.displays;
  if (displays.isNotEmpty) {
    final display = displays.first;
    if (display.size != Size.zero && display.devicePixelRatio > 0) {
      return (display.size / display.devicePixelRatio).shortestSide;
    }
  }
  final views = dispatcher.views;
  if (views.isEmpty) return null;
  final view = views.first;
  return (view.physicalSize / view.devicePixelRatio).shortestSide;
}
