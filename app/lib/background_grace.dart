import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// How the device keeps the app running for a moment after it leaves the
/// screen ([BackgroundGrace]).
enum GraceKind {
  /// The iPhone and the iPad: iOS suspends the app within seconds unless
  /// it asks for time, which the native side does (BackgroundGrace.swift)
  /// and says whether it got.
  native,

  /// Android: the app runs on in the background by itself for a while.
  timed,

  /// A Mac, or anything else: the app is never suspended, so there's no
  /// grace to speak of.
  none,
}

/// A short grace after leaving the app: a
/// connection that stayed alive through a quick trip to another app
/// (Messages and back) is only checked on coming back, one message the
/// backend answers, rather than made afresh, which costs a new endpoint
/// and the whole sync. If the check goes unanswered, it's made afresh as
/// before, a few seconds later.
///
/// On iOS the native side asks for the time on going to the background
/// and says over `factory/device/grace`: `began` once iOS gives it, `ended`
/// once it's over (after [length], iOS's time up, or none given at all).
/// Coming back ends it without a word, so it still reads as held then.
abstract final class BackgroundGrace {
  /// How long the grace lasts at most (the native side's too).
  static const length = Duration(seconds: 25);

  static const _channel = MethodChannel('factory/device/grace');

  static var _watching = false;
  static var _held = false;

  /// Starts hearing the native side, before the app first leaves the
  /// screen: on iOS, or in tests that say it's iOS. Once is enough; later
  /// calls do nothing.
  static void watch() {
    if (_watching) return;
    if (!Platform.isIOS && debugKind != GraceKind.native) return;
    _watching = true;
    _channel.setMethodCallHandler((call) async {
      switch (call.method) {
        case 'began':
          _held = true;
        case 'ended':
          _held = false;
      }
    });
  }

  /// Whether the app, back after [away] in the background, was kept
  /// running all along, so its connections may still be alive.
  static bool heldThrough(Duration away) {
    if (away >= length) return false;
    return switch (debugKind ?? _kind) {
      GraceKind.native => _held,
      GraceKind.timed => true,
      GraceKind.none => false,
    };
  }

  static GraceKind get _kind => Platform.isIOS
      ? GraceKind.native
      : Platform.isAndroid
      ? GraceKind.timed
      : GraceKind.none;

  /// In tests, the device's kind of grace.
  @visibleForTesting
  static GraceKind? debugKind;

  /// In tests, forgets what the native side said.
  @visibleForTesting
  static void debugReset() {
    _held = false;
  }
}
