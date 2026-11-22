import 'package:factory_app/attachments.dart';
import 'package:factory_app/audio.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/network.dart';
import 'package:factory_app/recorder.dart';
import 'package:factory_app/scanner.dart';
import 'package:factory_app/viewers/video.dart';
import 'package:factory_app/web_auth.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/channel_fakes.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  ChannelScript script(String name, List<ChannelReply> replies) {
    final fake = ChannelScript(name, replies);
    addTearDown(() {
      try {
        fake.assertDone();
      } finally {
        fake.dispose();
      }
    });
    return fake;
  }

  test(
    'connections preserves endpoint AND account maps on read and write',
    () async {
      script('factory/device', const [
        ChannelReply('connections', result: AndroidChannelFixtures.connections),
        ChannelReply(
          'setConnections',
          arguments: AndroidChannelFixtures.connections,
        ),
      ]);
      final storage = DeviceConnectionStorage();
      final kept = await storage.read();
      expect(kept.current, 'fixture-code');
      expect(kept.saved, ['fixture-code', 'other-fixture-code']);
      expect(kept.keys, {'fixture-code': 'fake-endpoint-seed'});
      expect(kept.account['key'], 'fake-account-seed');
      await storage.write(kept);
    },
  );

  for (final method in ['connections', 'setConnections']) {
    test(
      '$method storage failures propagate rather than resetting identity',
      () async {
        script('factory/device', [
          ChannelReply(
            method,
            arguments: method == 'connections'
                ? null
                : AndroidChannelFixtures.connections,
            error: PlatformException(
              code: 'store_unavailable',
              message: 'Fixture failure',
            ),
          ),
        ]);
        final storage = DeviceConnectionStorage();
        final result = method == 'connections'
            ? storage.read()
            : storage.write(
                const StoredConnections(
                  current: 'fixture-code',
                  saved: ['fixture-code', 'other-fixture-code'],
                  keys: {'fixture-code': 'fake-endpoint-seed'},
                  account: {
                    'key': 'fake-account-seed',
                    'service': 'https://account.example.invalid',
                    'email': 'person@example.invalid',
                  },
                ),
              );
        await expectLater(
          result,
          throwsA(
            isA<ConnectionStorageFailure>().having(
              (failure) => failure.code,
              'code',
              'store_unavailable',
            ),
          ),
        );
      },
    );
  }

  test('drafts are separate maps; Unicode/newlines survive the wire', () async {
    const drafts = {'fixture-code/agent': '未送信 👩🏽‍💻\nsecond line'};
    script('factory/device', const [
      ChannelReply('drafts', result: drafts),
      ChannelReply('setDrafts', arguments: drafts),
    ]);
    final storage = DeviceDraftStorage();
    expect(await storage.read(), drafts);
    await storage.write(drafts);
  });

  test('QR is a scalar text argument and size² typed module bytes', () async {
    script('factory/device', [
      ChannelReply(
        'qrCode',
        arguments: 'synthetic-qr-text',
        result: AndroidChannelFixtures.qr,
      ),
      const ChannelReply(
        'qrCode',
        arguments: 'bad',
        result: {
          'size': 2,
          'modules': [1, 0, 0, 1],
        },
      ),
    ]);
    final storage = DeviceConnectionStorage();
    final qr = await storage.qrCode('synthetic-qr-text');
    expect(qr!.size, 2);
    expect(qr.at(0, 0), isTrue);
    expect(qr.at(1, 0), isFalse);
    expect(
      await storage.qrCode('bad'),
      isNull,
      reason: 'plain List is not Uint8List',
    );
  });

  test('Android camera permission methods reach factory/device', () async {
    // The wire only (test/scanner_test.dart has every access state and the
    // preview's view); not a real camera reading a real QR code.
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    script('factory/device', const [
      ChannelReply('cameraAccess', result: 'undetermined'),
      ChannelReply('requestCamera', result: true),
      ChannelReply('openCameraSettings'),
    ]);
    const camera = DeviceCamera();
    expect(await camera.access(), CameraAccess.undetermined);
    expect(await camera.request(), isTrue);
    await camera.openSettings();
  });

  test(
    'camera permission methods keep their Apple wire shape for Android host',
    () async {
      // Exercise existing channel methods under their current gate. This is not
      // Android camera availability or proof of an Android platform view.
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      script('factory/device', const [
        ChannelReply('cameraAccess', result: 'denied'),
        ChannelReply('requestCamera', result: false),
        ChannelReply('openCameraSettings'),
      ]);
      const camera = DeviceCamera();
      expect(await camera.access(), CameraAccess.denied);
      expect(await camera.request(), isFalse);
      await camera.openSettings();
    },
  );

  test('picker returns private filesystem paths; cancel is empty', () async {
    script('factory/device', const [
      ChannelReply('pickAttachments', result: ['/fixture/private/photo.jpg']),
      ChannelReply('pickAttachments', result: []),
    ]);
    // A native content:// URI must be copied before this response. This test
    // asserts the expected shape, not that Kotlin has copied any bytes.
    expect(await pickAttachments(), ['/fixture/private/photo.jpg']);
    expect(await pickAttachments(), isEmpty);
  });

  test('browser uses URL/scheme map, cancellation is null', () async {
    script('factory/device', const [
      ChannelReply(
        'webAuth',
        arguments: {
          'url': 'https://service.example.invalid/authorize?state=fixture',
          'scheme': 'claktory-debug',
        },
        result: 'claktory-debug://callback?code=fixture&state=fixture',
      ),
      ChannelReply('cancelWebAuth'),
      ChannelReply(
        'webAuth',
        arguments: {
          'url': 'https://service.example.invalid/authorize',
          'scheme': 'claktory-debug',
        },
      ),
    ]);
    final back = await webAuth(
      Uri.parse('https://service.example.invalid/authorize?state=fixture'),
      'claktory-debug',
    );
    expect(back!.queryParameters['state'], 'fixture');
    await cancelWebAuth();
    expect(
      await webAuth(
        Uri.parse('https://service.example.invalid/authorize'),
        'claktory-debug',
      ),
      isNull,
    );
  });

  test('recorder methods use scalar path, durationMs, nullable stop', () async {
    script('factory/recorder', const [
      ChannelReply('access', result: 'undetermined'),
      ChannelReply('request', result: true),
      ChannelReply('openSettings'),
      ChannelReply('start', arguments: '/fixture/voice.m4a'),
      ChannelReply('stop', result: AndroidChannelFixtures.recording),
      ChannelReply('stop'),
      ChannelReply('cancel'),
    ]);
    final recorder = NativeRecorder();
    expect(await recorder.access(), MicAccess.undetermined);
    expect(await recorder.request(), isTrue);
    await recorder.openSettings();
    await recorder.start('/fixture/voice.m4a');
    final recording = await recorder.stop();
    expect(recording!.path, '/fixture/voice.m4a');
    expect(recording.duration, const Duration(milliseconds: 1234));
    expect(await recorder.stop(), isNull);
    await recorder.cancel();
    expect(VoiceRecorder.maxDuration, const Duration(minutes: 10));
  });

  test(
    'recorder denial retains structured failure; no plugin is unavailable',
    () async {
      script('factory/recorder', [
        ChannelReply(
          'start',
          arguments: '/fixture/voice.m4a',
          error: PlatformException(code: 'denied', message: 'Fixture denied'),
        ),
      ]);
      await expectLater(
        NativeRecorder().start('/fixture/voice.m4a'),
        throwsA(
          isA<RecordingFailure>().having((e) => e.code, 'code', 'denied'),
        ),
      );
    },
  );

  test('absent recorder does not pretend permission was granted', () async {
    expect(await NativeRecorder().access(), MicAccess.unavailable);
  });

  test(
    'recorder event channel reports levels, end reasons, errors and cancels',
    () async {
      final events = ChannelEvents('factory/recorder/events');
      addTearDown(events.dispose);
      final heard = <RecorderEvent>[];
      final errors = <Object>[];
      final sub = NativeRecorder().events.listen(
        heard.add,
        onError: errors.add,
      );
      await Future<void>.delayed(Duration.zero);
      await events.emit(AndroidChannelFixtures.recorderLevel);
      for (final reason in ['limit', 'interrupted', 'error']) {
        await events.emit({'ms': 600000, 'ended': reason});
      }
      await events.error('interrupted', 'Fixture channel error');
      expect(heard.first.level, 0.5);
      expect(heard.first.elapsed, const Duration(milliseconds: 1234));
      expect(heard.skip(1).map((e) => e.ended), [
        RecordingEnd.limit,
        RecordingEnd.interrupted,
        RecordingEnd.error,
      ]);
      expect(errors.single, isA<PlatformException>());
      await sub.cancel();
      expect(events.listening, isFalse);
    },
  );

  test(
    'network is an event channel: ordered interfaces and movement debounce',
    () async {
      final events = ChannelEvents('factory/network');
      addTearDown(events.dispose);
      final moves = <NetworkPath>[];
      // The global is deliberately empty in flutter test. Bind the actual wire
      // explicitly instead of replacing production transport with another fake.
      final sub = networkMoves(
        const EventChannel('factory/network')
            .receiveBroadcastStream()
            .map(NetworkPath.fromEvent),
        settle: Duration.zero,
      ).listen(moves.add);
      await Future<void>.delayed(Duration.zero);
      await events.emit(AndroidChannelFixtures.wifi);
      await Future<void>.delayed(Duration.zero);
      expect(moves, isEmpty, reason: 'initial path is not a movement');
      await events.emit(AndroidChannelFixtures.cellular);
      await Future<void>.delayed(Duration.zero);
      expect(moves.single.primary, 'cellular');
      expect(moves.single.expensive, isTrue);
      expect(moves.single.constrained, isTrue);
      await sub.cancel();
      expect(events.listening, isFalse);
    },
  );

  test('audio method/event fixtures preserve milliseconds and state', () async {
    script('factory/audio', const [
      ChannelReply(
        'play',
        arguments: {'path': '/fixture/voice.m4a', 'fromMs': 20},
      ),
      ChannelReply('pause'),
      ChannelReply('seek', arguments: 123),
      ChannelReply('stop'),
    ]);
    final events = ChannelEvents('factory/audio/events');
    addTearDown(events.dispose);
    final audio = NativeAudio();
    final heard = <AudioPosition>[];
    final sub = audio.events.listen(heard.add);
    await Future<void>.delayed(Duration.zero);
    await audio.play(
      '/fixture/voice.m4a',
      from: const Duration(milliseconds: 20),
    );
    await events.emit({
      'path': '/fixture/voice.m4a',
      'positionMs': 20,
      'durationMs': 1234,
      'state': 'playing',
    });
    expect(heard.single.state, AudioState.playing);
    expect(heard.single.position.inMilliseconds, 20);
    expect(heard.single.duration.inMilliseconds, 1234);
    await audio.pause();
    await audio.seek(const Duration(milliseconds: 123));
    await audio.stop();
    await sub.cancel();
    expect(events.listening, isFalse);
  });

  test('video is texture-scoped with seconds, rotation, close', () async {
    script('claktory/video', const [
      ChannelReply(
        'open',
        arguments: {'path': '/fixture/video.mp4'},
        result: {
          'texture': 7,
          'width': 1920,
          'height': 1080,
          'quarterTurns': 1,
          'position': 0.0,
          'duration': 12.5,
          'playing': false,
        },
      ),
      ChannelReply('play', arguments: {'texture': 7}),
      ChannelReply('seek', arguments: {'texture': 7, 'seconds': 2.5}),
      ChannelReply('pause', arguments: {'texture': 7}),
      ChannelReply('close', arguments: {'texture': 7}),
    ]);
    final video = newVideoPlayback();
    await video.open('/fixture/video.mp4');
    expect(video.state.value.ready, isTrue);
    expect(video.state.value.size.width, 1080);
    expect(video.state.value.size.height, 1920);
    await video.play();
    await video.seek(2.5);
    await video.pause();
    await video.dispose();
  });
}
