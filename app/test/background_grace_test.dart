import 'package:factory_app/background_grace.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// The native side saying [method] over `factory/device/grace`, as
/// BackgroundGrace.swift does.
Future<void> _native(String method) async {
  final message = const StandardMethodCodec().encodeMethodCall(
    MethodCall(method),
  );
  await TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
      .handlePlatformMessage('factory/device/grace', message, (_) {});
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  BackgroundGrace.debugKind = GraceKind.native;
  BackgroundGrace.watch();

  tearDown(() {
    BackgroundGrace.debugKind = null;
    BackgroundGrace.debugReset();
  });

  test('on iOS it holds only while the native side says so', () async {
    BackgroundGrace.debugKind = GraceKind.native;
    const away = Duration(seconds: 10);
    expect(BackgroundGrace.heldThrough(away), isFalse);
    await _native('began');
    expect(BackgroundGrace.heldThrough(away), isTrue);
    // Coming back says nothing: a second return still reads it as held
    // until the next time the app leaves.
    expect(BackgroundGrace.heldThrough(away), isTrue);
    expect(BackgroundGrace.heldThrough(BackgroundGrace.length), isFalse);
    await _native('ended');
    expect(BackgroundGrace.heldThrough(away), isFalse);
  });

  test('iOS giving no time at all is no grace', () async {
    BackgroundGrace.debugKind = GraceKind.native;
    await _native('began');
    await _native('ended');
    expect(BackgroundGrace.heldThrough(const Duration(seconds: 4)), isFalse);
  });

  test('on Android it holds for its length; on a Mac never', () {
    BackgroundGrace.debugKind = GraceKind.timed;
    expect(BackgroundGrace.heldThrough(const Duration(seconds: 24)), isTrue);
    expect(BackgroundGrace.heldThrough(const Duration(seconds: 25)), isFalse);
    BackgroundGrace.debugKind = GraceKind.none;
    expect(BackgroundGrace.heldThrough(const Duration(seconds: 5)), isFalse);
  });
}
