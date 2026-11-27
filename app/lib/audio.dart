// Playing voice messages back: the recording a voice message
// carries, fetched from its Claktory into a temporary file and played
// natively (AudioPlayer.swift, on the factory/audio channel). One plays at
// a time, app-wide.
import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'backend.dart';

/// Where a recording is in its playing.
enum AudioState { playing, paused, ended, stopped }

/// What the native player says as it plays: which file, where it is in it,
/// how long it is, and whether it's playing.
class AudioPosition {
  const AudioPosition({
    required this.path,
    required this.position,
    required this.duration,
    required this.state,
  });

  final String path;
  final Duration position;
  final Duration duration;
  final AudioState state;
}

/// Plays sound files on this device.
abstract class AudioOutput {
  /// Plays the file at [path] from [from]; what played before stops.
  Future<void> play(String path, {Duration from = Duration.zero});
  Future<void> pause();
  Future<void> seek(Duration position);
  Future<void> stop();
  Stream<AudioPosition> get events;
}

/// The native player (AudioPlayer.swift).
class NativeAudio implements AudioOutput {
  static const _channel = MethodChannel('factory/audio');
  static const _events = EventChannel('factory/audio/events');

  @override
  late final Stream<AudioPosition> events = _events
      .receiveBroadcastStream()
      .map((event) {
        final map = (event as Map).cast<String, Object?>();
        return AudioPosition(
          path: map['path']! as String,
          position: Duration(
            milliseconds: (map['positionMs'] as num?)?.toInt() ?? 0,
          ),
          duration: Duration(
            milliseconds: (map['durationMs'] as num?)?.toInt() ?? 0,
          ),
          state: switch (map['state']) {
            'playing' => AudioState.playing,
            'paused' => AudioState.paused,
            'ended' => AudioState.ended,
            _ => AudioState.stopped,
          },
        );
      });

  @override
  Future<void> play(String path, {Duration from = Duration.zero}) =>
      _channel.invokeMethod<void>('play', {
        'path': path,
        'fromMs': from.inMilliseconds,
      });

  @override
  Future<void> pause() => _channel.invokeMethod<void>('pause');

  @override
  Future<void> seek(Duration position) =>
      _channel.invokeMethod<void>('seek', position.inMilliseconds);

  @override
  Future<void> stop() => _channel.invokeMethod<void>('stop');
}

/// The voice message playing now, if any, app-wide: which recording (by
/// its path on its Claktory), where it is, and whether it's on its way
/// here first.
class VoicePlayer extends ChangeNotifier {
  VoicePlayer({AudioOutput? output, this.temp})
    : _output = output ?? NativeAudio();

  final AudioOutput _output;

  /// Where fetched recordings go; the system's temporary folder unless
  /// given.
  final Directory? temp;
  StreamSubscription<AudioPosition>? _events;

  /// The recording shown as playing or paused, by its path on its
  /// Claktory; null when none is.
  String? get current => _current;
  String? _current;

  /// Fetching [current] from its Claktory before it plays.
  bool get fetching => _fetching;
  var _fetching = false;

  bool get playing => _state == AudioState.playing;
  AudioState _state = AudioState.stopped;

  Duration get position => _position;
  var _position = Duration.zero;

  /// Why the last recording couldn't be played, for the person.
  String? get error => _error;
  String? _error;

  /// The local copy of each recording fetched, by its path on its
  /// Claktory.
  final _copies = <String, String>{};

  /// Plays [path], a recording on [backend]'s machine, from where it was
  /// paused; pauses it if it's playing.
  Future<void> toggle(Backend backend, String path) async {
    if (_current == path && _fetching) return;
    if (_current == path && playing) {
      await _output.pause();
      return;
    }
    _listen();
    final resume = _current == path && _state == AudioState.paused
        ? _position
        : Duration.zero;
    _current = path;
    _error = null;
    _position = resume;
    var copy = _copies[path];
    if (copy == null || !File(copy).existsSync()) {
      _fetching = true;
      notifyListeners();
      try {
        copy = await _fetch(backend, path);
        _copies[path] = copy;
      } on RequestFailure catch (failure) {
        if (_current == path) {
          _error = failure.message.isEmpty
              ? "This recording can't be played."
              : failure.message;
          _current = null;
        }
        return;
      } finally {
        _fetching = false;
        notifyListeners();
      }
      // Another was chosen meanwhile.
      if (_current != path) return;
    } else {
      notifyListeners();
    }
    try {
      await _output.play(copy, from: resume);
    } on PlatformException catch (error) {
      _error = error.message ?? "This recording can't be played.";
      _current = null;
      notifyListeners();
    } on MissingPluginException {
      _error = "This device can't play recordings.";
      _current = null;
      notifyListeners();
    }
  }

  /// Plays [path], a recording on this device (one kept until it's sent),
  /// from where it was paused; pauses it if it's playing.
  Future<void> toggleHere(Backend backend, String path) {
    _copies[path] = path;
    return toggle(backend, path);
  }

  /// Stops what's playing.
  Future<void> stop() async {
    if (_current == null) return;
    _current = null;
    _state = AudioState.stopped;
    notifyListeners();
    await _output.stop();
  }

  void _listen() {
    _events ??= _output.events.listen((event) {
      final current = _current;
      if (current == null || _copies[current] != event.path) return;
      _position = event.position;
      _state = event.state;
      if (event.state case AudioState.ended || AudioState.stopped) {
        _current = null;
        _position = Duration.zero;
      }
      notifyListeners();
    });
  }

  /// A local copy of [path] on [backend]'s machine, read a piece at a time.
  Future<String> _fetch(Backend backend, String path) async {
    const piece = 1024 * 1024;
    final first = await backend.readFile(path, maxBytes: piece);
    if (first.error.isNotEmpty) throw RequestFailure(first.error);
    final name = path.split('/').last;
    final version = first.sha256.isEmpty
        ? '${DateTime.now().microsecondsSinceEpoch}'
        : first.sha256.substring(0, first.sha256.length.clamp(0, 16));
    final folder = Directory(
      '${(temp ?? Directory.systemTemp).path}/claktory-voice-played/$version',
    );
    await folder.create(recursive: true);
    final copy = File('${folder.path}/$name');
    final bytes = BytesBuilder(copy: false)..add(first.data);
    final size = first.size.toInt();
    while (bytes.length < size) {
      final chunk = await backend.readFile(
        path,
        maxBytes: piece,
        offset: bytes.length,
      );
      if (chunk.error.isNotEmpty) throw RequestFailure(chunk.error);
      if (chunk.offset.toInt() != bytes.length || chunk.data.isEmpty) {
        throw const RequestFailure(
          "To play recordings here, update the Claktory you're connected to.",
        );
      }
      bytes.add(chunk.data);
    }
    await copy.writeAsBytes(bytes.takeBytes());
    return copy.path;
  }

  @override
  void dispose() {
    unawaited(_events?.cancel());
    super.dispose();
  }
}

/// The app's player for voice messages; tests set another.
VoicePlayer voicePlayer = VoicePlayer();
