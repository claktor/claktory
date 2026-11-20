import 'dart:convert';

import 'package:factory_app/annotations.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/video.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, showPanes;

/// A player that plays nothing: a minute of 1280×720, and what it was told.
class FakePlayback implements VideoPlayback {
  static final made = <FakePlayback>[];

  FakePlayback() {
    made.add(this);
  }

  final _state = ValueNotifier(const PlaybackState());
  String? opened;
  final seeks = <double>[];

  @override
  ValueListenable<PlaybackState> get state => _state;

  PlaybackState _with({bool? playing, double? position}) => PlaybackState(
    ready: true,
    playing: playing ?? _state.value.playing,
    position: position ?? _state.value.position,
    duration: 60,
    size: const Size(1280, 720),
  );

  @override
  Future<void> open(String file) async {
    opened = file;
    _state.value = _with();
  }

  @override
  Future<void> play() async => _state.value = _with(playing: true);

  @override
  Future<void> pause() async => _state.value = _with(playing: false);

  @override
  Future<void> seek(double seconds) async {
    seeks.add(seconds);
    _state.value = _with(position: seconds);
  }

  @override
  Widget view() => const ColoredBox(color: Color(0xFF000000));

  @override
  Future<void> dispose() async {}
}

const _path = '/w/river.mp4';

Thread _moment(
  String id,
  String value, {
  AnchorState anchor = AnchorState.ANCHOR_STATE_ANCHORED,
  String body = 'Too slow here.',
}) => Thread(
  id: id,
  agentId: 'a1',
  path: _path,
  kind: 'video',
  versionSha256: 'v1',
  status: ThreadStatus.THREAD_STATUS_OPEN,
  anchor: anchor,
  selectors: [
    Selector(json: jsonEncode({'type': 'FragmentSelector', 'value': value})),
  ],
  comments: [ThreadComment(id: 'c$id', body: body)],
);

Future<Harness> _showVideo(
  WidgetTester tester, {
  int size = 9 * 1024 * 1024,
  List<Thread> threads = const [],
  bool ranged = true,
}) async {
  final data = List<int>.filled(size, 7);
  final harness = await showPanes(
    tester,
    beside('video', _path),
    files: {
      _path: FileContent(
        path: _path,
        mime: 'video/mp4',
        size: Int64(size),
        data: data,
        sha256: 'v1',
      ),
    },
  );
  harness.backend.rangedReads = ranged;
  tester.view.physicalSize = const Size(2000, 900);
  harness.backend.threads.apply(
    ThreadList(path: _path, versionSha256: 'v1', threads: threads),
  );
  // Writing the copy is real work on the disk.
  for (var i = 0; i < 6; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 100)),
    );
    await tester.pump();
  }
  await settle(tester);
  return harness;
}

Rect _timeline(WidgetTester tester) => tester.getRect(find.byType(Timeline));

CommentsController _controller(WidgetTester tester) =>
    tester.widget<Commented>(find.byType(Commented)).controller;

void main() {
  final system = newVideoPlayback;

  group('the system player', () {
    const channel = MethodChannel('claktory/video');
    final calls = <MethodCall>[];
    Object? opened;
    TestWidgetsFlutterBinding.ensureInitialized();

    setUp(() {
      calls.clear();
      opened = {
        'texture': 7,
        'playing': false,
        'position': 0.0,
        'duration': 12.5,
        'width': 640.0,
        'height': 360.0,
        'quarterTurns': 0,
      };
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(channel, (call) async {
            calls.add(call);
            if (call.method == 'open') {
              final answer = opened;
              if (answer is PlatformException) throw answer;
              return answer;
            }
            return null;
          });
    });

    tearDown(
      () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(channel, null),
    );

    Future<void> fromNative(Map<String, Object?> state) =>
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
            .handlePlatformMessage(
              channel.name,
              channel.codec.encodeMethodCall(MethodCall('state', state)),
              (_) {},
            );

    test('opens the file, follows what it says, and closes it', () async {
      final playback = system();
      await playback.open('/tmp/clip.mp4');
      expect(calls.single.method, 'open');
      expect(calls.single.arguments, {'path': '/tmp/clip.mp4'});
      var state = playback.state.value;
      expect(state.ready, isTrue);
      expect(state.duration, 12.5);
      expect(state.size, const Size(640, 360));
      expect(playback.view(), isA<RotatedBox>());

      await playback.play();
      await playback.seek(3);
      expect(calls[1].method, 'play');
      expect(calls[1].arguments, {'texture': 7});
      expect(calls[2].method, 'seek');
      expect(calls[2].arguments, {'texture': 7, 'seconds': 3.0});

      // Another player's news isn't this one's.
      await fromNative({'texture': 8, 'playing': true, 'position': 9.0});
      expect(playback.state.value.playing, isFalse);
      await fromNative({
        'texture': 7,
        'playing': true,
        'position': 3.5,
        'duration': 12.5,
        'width': 640.0,
        'height': 360.0,
        'quarterTurns': 1,
      });
      state = playback.state.value;
      expect(state.playing, isTrue);
      expect(state.position, 3.5);
      // Turned a quarter: shown upright.
      expect(state.size, const Size(360, 640));
      expect((playback.view() as RotatedBox).quarterTurns, 1);

      await playback.dispose();
      expect(calls.last.method, 'close');
      expect(calls.last.arguments, {'texture': 7});
    });

    test("a file it can't play says why", () async {
      opened = PlatformException(
        code: 'failed',
        message: 'The file isn’t compatible.',
      );
      final playback = system();
      await playback.open('/tmp/bad.mp4');
      expect(playback.state.value.error, 'The file isn’t compatible.');
      expect(playback.state.value.ready, isFalse);
      await playback.dispose();
      expect(calls.map((c) => c.method), ['open']);
    });
  });

  setUp(() {
    FakePlayback.made.clear();
    newVideoPlayback = FakePlayback.new;
  });

  test('a moment or a stretch is a media fragment, and reads back', () {
    final stretch = Moment(31.25, end: 45);
    final json = jsonDecode(stretch.selectors.single.json) as Map;
    expect(json['value'], 't=31.3,45');
    expect(stretch.label, '0:31–0:45');
    final framed = Moment(5, region: const Rect.fromLTWH(0.1, 0.2, 0.3, 0.25));
    final value = (jsonDecode(framed.selectors.single.json) as Map)['value'];
    expect(value, 't=5&xywh=percent:10,20,30,25');
    final back = momentOf(Thread(selectors: framed.selectors))!;
    expect(back.start, 5);
    expect(back.end, isNull);
    expect(back.region!.left, closeTo(0.1, 1e-9));
    expect(formatTime(3725), '1:02:05');
    expect(formatTime(7.9), '0:07');
  });

  testWidgets('a video comes a chunk at a time, then plays', (tester) async {
    final harness = await _showVideo(tester);
    final reads = [
      for (final m in harness.backend.sent)
        if (m.hasReadFile()) m.readFile.offset.toInt(),
    ];
    expect(reads, [0, videoChunk]);
    final player = FakePlayback.made.single;
    expect(player.opened, contains('river.mp4'));
    expect(find.text('0:00 / 1:00'), findsOneWidget);
    expect(find.textContaining('1280 × 720'), findsOneWidget);
    await tester.tap(find.bySemanticsLabel('Play').first);
    await settle(tester);
    expect(player.state.value.playing, isTrue);
  });

  testWidgets("a backend that can't read ranges says to update it", (
    tester,
  ) async {
    await _showVideo(tester, ranged: false);
    // Read again, from a backend that answers every read from the start.
    await tester.tap(find.bySemanticsLabel('Reload').first);
    for (var i = 0; i < 4; i++) {
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 100)),
      );
      await tester.pump();
    }
    expect(
      find.text(
        "To play videos here, update the Claktory you're connected to.",
      ),
      findsOneWidget,
    );
  });

  testWidgets('comments show on the timeline, and a click plays from one', (
    tester,
  ) async {
    await _showVideo(
      tester,
      threads: [
        _moment('t1', 't=30,45'),
        _moment('t2', 't=6', body: 'Cut here.'),
      ],
    );
    // In time order down the margin, each with when.
    expect(find.text('0:30–0:45'), findsOneWidget);
    expect(find.text('0:06'), findsOneWidget);
    expect(
      tester.getTopLeft(find.text('Cut here.')).dy,
      lessThan(tester.getTopLeft(find.text('Too slow here.')).dy),
    );
    // The stretch's bar, from half to three quarters along.
    final track = _timeline(tester);
    await tester.tapAt(Offset(track.left + track.width * 0.62, track.top + 4));
    await settle(tester);
    expect(_controller(tester).focused, 't1');
    expect(FakePlayback.made.single.seeks.last, 30);
  });

  testWidgets('Comment, then a drag along the timeline, makes a comment on '
      'that stretch', (tester) async {
    final harness = await _showVideo(tester);
    // The Comment button over the frame.
    await tester.tap(
      find.descendant(
        of: find.byType(CommentPick),
        matching: find.text('Comment'),
      ),
    );
    await settle(tester);
    // To begin with, the moment it's at.
    expect(_controller(tester).selection.value, Moment(0));
    final track = _timeline(tester);
    await tester.dragFrom(
      Offset(track.left + track.width * 0.25, track.center.dy),
      Offset(track.width * 0.25, 0),
    );
    await settle(tester);
    final marked = _controller(tester).selection.value! as Moment;
    expect(marked.start, closeTo(15, 0.5));
    expect(marked.end, closeTo(30, 0.5));
    await tester.tap(find.text('Comment').last);
    await settle(tester);
    expect(find.text(marked.label), findsOneWidget);
    await tester.enterText(
      find.descendant(
        of: find.byType(Commented),
        matching: find.byType(EditableText),
      ),
      'Hold this shot longer.',
    );
    await tester.pump();
    await tester.tap(find.text('Comment').last);
    await settle(tester);
    final made = harness.backend.sent.lastWhere((m) => m.hasCreateThread());
    expect(made.createThread.kind, 'video');
    expect(made.createThread.versionSha256, 'v1');
    final back = momentOf(Thread(selectors: made.createThread.selectors))!;
    expect(back.start, closeTo(15, 0.5));
    expect(back.end, closeTo(30, 0.5));
  });

  testWidgets('a comment made on an earlier version says so', (tester) async {
    await _showVideo(
      tester,
      threads: [
        _moment('t1', 't=10,20', anchor: AnchorState.ANCHOR_STATE_OUTDATED),
      ],
    );
    expect(
      find.text('Made on an earlier version of this video.'),
      findsOneWidget,
    );
    expect(find.text('0:10–0:20'), findsOneWidget);
  });
}
