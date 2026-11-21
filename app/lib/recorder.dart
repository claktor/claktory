// Recording voice messages on this device (#197): the native recorder
// (app/macos/Runner/Recorder.swift, the iPhone's Runner too) over the
// factory/recorder channel. AAC in .m4a at 16 kHz mono, at most ten
// minutes; the file is then sent like any attachment, as a voice message.
import 'dart:io';

import 'package:flutter/services.dart';

/// Whether this app may use the microphone.
enum MicAccess { granted, denied, undetermined, restricted, unavailable }

/// A finished recording: the file and how long it runs.
class Recording {
  const Recording({required this.path, required this.duration});

  final String path;
  final Duration duration;
}

/// Why a recording ended by itself.
enum RecordingEnd {
  /// It reached [VoiceRecorder.maxDuration]; the file is kept.
  limit,

  /// A call or Siri took the microphone (iPhone); the file is kept.
  interrupted,

  /// It failed; the file is gone.
  error,
}

/// While recording: how loud it is now and how long it's run; or how it
/// ended by itself ([ended]).
class RecorderEvent {
  const RecorderEvent({
    this.level = 0,
    this.elapsed = Duration.zero,
    this.ended,
  });

  final double level;
  final Duration elapsed;
  final RecordingEnd? ended;
}

/// The device's microphone, one recording at a time. Tests give a fake.
abstract class VoiceRecorder {
  /// The longest recording: it stops by itself then.
  static const maxDuration = Duration(minutes: 10);

  Future<MicAccess> access();

  /// Asks the person; true if allowed.
  Future<bool> request();

  /// The system's settings for this app, where access is given back
  /// (iPhone).
  Future<void> openSettings();

  /// Starts recording into [path]; throws [RecordingFailure] if it can't.
  Future<void> start(String path);

  /// Stops, keeping the file; null if nothing was recording.
  Future<Recording?> stop();

  /// Stops and deletes the file.
  Future<void> cancel();

  /// Loudness and time while recording, and how it ended by itself.
  Stream<RecorderEvent> get events;
}

class RecordingFailure implements Exception {
  const RecordingFailure(this.message, {this.code = 'failed'});

  final String message;

  /// "denied", "busy" or "failed".
  final String code;

  @override
  String toString() => message;
}

/// The native recorder (Recorder.swift).
class NativeRecorder implements VoiceRecorder {
  static const _channel = MethodChannel('factory/recorder');
  static const _events = EventChannel('factory/recorder/events');

  @override
  late final Stream<RecorderEvent> events = _events
      .receiveBroadcastStream()
      .map((event) {
        final map = (event as Map).cast<String, Object?>();
        final ended = map['ended'];
        return RecorderEvent(
          level: (map['level'] as num?)?.toDouble() ?? 0,
          elapsed: Duration(milliseconds: (map['ms'] as num?)?.toInt() ?? 0),
          ended: switch (ended) {
            'limit' => RecordingEnd.limit,
            'interrupted' => RecordingEnd.interrupted,
            null => null,
            _ => RecordingEnd.error,
          },
        );
      });

  @override
  Future<MicAccess> access() async {
    try {
      return switch (await _channel.invokeMethod<String>('access')) {
        'granted' => MicAccess.granted,
        'undetermined' => MicAccess.undetermined,
        'restricted' => MicAccess.restricted,
        _ => MicAccess.denied,
      };
    } on MissingPluginException {
      return MicAccess.unavailable;
    } on PlatformException {
      return MicAccess.unavailable;
    }
  }

  @override
  Future<bool> request() async =>
      await _channel.invokeMethod<bool>('request') ?? false;

  @override
  Future<void> openSettings() => _channel.invokeMethod<void>('openSettings');

  @override
  Future<void> start(String path) async {
    try {
      await _channel.invokeMethod<void>('start', path);
    } on PlatformException catch (error) {
      throw RecordingFailure(
        error.message ?? "The microphone couldn't start.",
        code: error.code,
      );
    }
  }

  @override
  Future<Recording?> stop() async {
    final answer = await _channel.invokeMapMethod<String, Object?>('stop');
    if (answer == null) return null;
    return Recording(
      path: answer['path']! as String,
      duration: Duration(milliseconds: answer['durationMs']! as int),
    );
  }

  @override
  Future<void> cancel() => _channel.invokeMethod<void>('cancel');
}

/// Where a new recording goes: a temporary file named for when it was
/// made, "Voice 2026-11-21 14.02.m4a".
String voiceFilePath(DateTime now, {String? folder}) {
  String two(int n) => n.toString().padLeft(2, '0');
  final name =
      'Voice ${now.year}-${two(now.month)}-${two(now.day)} '
      '${two(now.hour)}.${two(now.minute)}.m4a';
  final base = folder ?? Directory.systemTemp.path;
  return '$base/claktory-voice/${now.microsecondsSinceEpoch}/$name';
}

/// This device's microphone.
/// Tests set another.
VoiceRecorder deviceRecorder = NativeRecorder();
