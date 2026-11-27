// The message box's recording strip: the waveform centered on the
// whole box however wide it is, newest in the middle.
import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/recorder.dart';
import 'package:factory_app/voice.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'voice_test.dart' show FakeRecorder, io, mic, withVoice;

final _wave = find.byWidgetPredicate(
  (w) => w is CustomPaint && w.painter is CenteredWaveform,
);

/// The box's frame around the strip: what the waveform centers on.
Rect _box(WidgetTester tester) => tester.getRect(
  find
      .ancestor(
        of: find.byType(RecordingStrip),
        matching: find.byType(AnimatedContainer),
      )
      .first,
);

/// Geist, as the app ships it, so the time and the buttons are as wide as
/// on the device.
Future<void> _loadFonts() async {
  final loader = FontLoader('Geist');
  for (final file in [
    'Geist-Regular.ttf',
    'Geist-Medium.ttf',
    'Geist-SemiBold.ttf',
  ]) {
    loader.addFont(
      Future.value(ByteData.sublistView(File('fonts/$file').readAsBytesSync())),
    );
  }
  await loader.load();
}

void main() {
  setUpAll(_loadFonts);
  late FakeRecorder recorder;
  late VoiceRecorder was;
  setUp(() {
    voiceMessagesOn = true;
    was = deviceRecorder;
    recorder = FakeRecorder();
    deviceRecorder = recorder;
  });
  tearDown(() => deviceRecorder = was);

  Future<void> record(WidgetTester tester, double width) async {
    await Harness(withVoice(SpeechModelState.SPEECH_MODEL_STATE_READY))
        .show(tester, size: Size(width, 800));
    await tester.tap(mic);
    await io(tester);
    for (var i = 0; i < 30; i++) {
      recorder.hear(i.isEven ? 0.8 : 0.2, Duration(milliseconds: 50 * i));
    }
    await tester.pump();
  }

  testWidgets('in a very wide box the waveform is centered on the whole box '
      'and takes most of it', (tester) async {
    await record(tester, 2400);
    final box = _box(tester);
    final wave = tester.getRect(_wave);
    expect(wave.center.dx, moreOrLessEquals(box.center.dx, epsilon: 2));
    expect(wave.width, greaterThan(box.width * 0.7));
    // Send went into the strip, after Cancel; the box shows it once.
    expect(find.text('Send'), findsOneWidget);
    expect(
      tester.getRect(find.text('Send')).left,
      greaterThan(tester.getRect(find.text('Cancel')).right),
    );
  });

  testWidgets('in a medium box too', (tester) async {
    await record(tester, 1000);
    final box = _box(tester);
    expect(
      tester.getRect(_wave).center.dx,
      moreOrLessEquals(box.center.dx, epsilon: 2),
    );
  });

  testWidgets(
    "on a narrow iPad, where centering would leave it little room, it "
    'takes the room between the time and the buttons',
    (tester) async {
      asIPad();
      await record(tester, 694);
      final wave = tester.getRect(_wave);
      expect(wave.width, greaterThan(60));
      expect(
        wave.left,
        greaterThan(tester.getRect(find.textContaining('0:0')).right),
      );
      expect(wave.right, lessThan(tester.getRect(find.text('Cancel')).left));
    },
  );

  testWidgets('Send shows once recording starts, after the microphone was '
      'asked for first (Abbacchio 38993)', (tester) async {
    // Asked for the first time: the recording starts only once it's
    // allowed, with no other change in the box meanwhile.
    final allowed = Completer<bool>();
    deviceRecorder = _AskingRecorder(recorder, allowed.future);
    await Harness(withVoice(SpeechModelState.SPEECH_MODEL_STATE_READY))
        .show(tester, size: const Size(1200, 800));
    await tester.tap(mic);
    await io(tester);
    // The microphone's popover first (permissions spec, P5): Continue
    // brings the system's prompt; allowed, the next click records.
    expect(find.text('Talk instead of typing'), findsOneWidget);
    expect(find.byType(RecordingStrip), findsNothing);
    await tester.tap(find.text('Continue'));
    await io(tester);
    allowed.complete(true);
    await io(tester);
    expect(find.text('Talk instead of typing'), findsNothing);
    expect(find.byType(RecordingStrip), findsNothing);
    await tester.tap(mic);
    await io(tester);
    expect(find.byType(RecordingStrip), findsOneWidget);
    recorder.hear(0.5, const Duration(milliseconds: 50));
    await tester.pump();
    expect(find.text('Send'), findsOneWidget);
    expect(find.bySemanticsLabel('Record a voice message'), findsNothing);
  });

  test('the newest level is in the middle, the ones before spread to both '
      'sides alike, fading toward the ends', () {
    final recorder = ui.PictureRecorder();
    final canvas = _Lines(Canvas(recorder));
    CenteredWaveform([
      0.1,
      0.5,
      1,
    ], color: const Color(0xFF000000)).paint(canvas, const Size(49, 20));
    recorder.endRecording();
    // 49 wide: the middle at 24.5, a bar every 4 points out to each side.
    final byX = {for (final line in canvas.lines) line.x: line};
    expect(byX.keys, hasLength(11));
    expect(byX[24.5]!.height, greaterThan(byX[28.5]!.height));
    expect(byX[20.5]!.height, byX[28.5]!.height);
    expect(byX[16.5]!.height, byX[32.5]!.height);
    // Silent beyond what was heard: dots, fainter toward the ends.
    expect(byX[4.5]!.height, lessThan(2));
    expect(byX[4.5]!.alpha, lessThan(byX[24.5]!.alpha));
  });
}

class _Line {
  _Line(this.x, this.height, this.alpha);
  final double x;
  final double height;
  final double alpha;
}

/// Keeps the vertical lines drawn, passing everything on.
class _Lines implements Canvas {
  _Lines(this._canvas);
  final Canvas _canvas;
  final lines = <_Line>[];

  @override
  void drawLine(Offset p1, Offset p2, Paint paint) {
    lines.add(_Line(p1.dx, (p2.dy - p1.dy).abs(), paint.color.a));
    _canvas.drawLine(p1, p2, paint);
  }

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}

/// A microphone asked for the first time: it starts only once the person
/// answers.
class _AskingRecorder implements VoiceRecorder {
  _AskingRecorder(this.inner, this.answer);
  final FakeRecorder inner;
  final Future<bool> answer;
  var _asked = false;

  @override
  Future<MicAccess> access() async =>
      _asked ? inner.access() : MicAccess.undetermined;

  @override
  Future<bool> request() {
    _asked = true;
    return answer;
  }

  @override
  Future<void> openSettings() => inner.openSettings();

  @override
  Future<void> start(String path) => inner.start(path);

  @override
  Future<Recording?> stop() => inner.stop();

  @override
  Future<void> cancel() => inner.cancel();

  @override
  Stream<RecorderEvent> get events => inner.events;
}
