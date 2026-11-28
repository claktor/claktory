import 'dart:async';
import 'dart:ui' as ui;

import 'package:factory_app/errands/live_input.dart';
import 'package:factory_app/errands/live_view.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

/// Frames whose "JPEG" is its width and height, decoded when [release]
/// says so (all at once unless held).
class _Decoder {
  final held = <int, Completer<void>>{};
  var hold = false;

  Future<ui.Image> call(Uint8List bytes) async {
    if (hold) {
      final wait = Completer<void>();
      held[bytes[0]] = wait;
      await wait.future;
    }
    return createTestImage(width: bytes[1] * 10, height: bytes[2] * 10);
  }
}

Uint8List _jpeg(int tag, int width, int height) =>
    Uint8List.fromList([tag, width, height]);

void main() {
  group('frames', () {
    testWidgets('each is acknowledged once ready; the newest wins', (
      tester,
    ) async {
      final acked = <int>[];
      final decoder = _Decoder();
      final frames = LiveFrames(ack: acked.add, decode: decoder.call);
      addTearDown(frames.dispose);
      await tester.runAsync(() async {
        await frames.take(1, _jpeg(1, 128, 80), origin: 'https://a.example');
      });
      expect(acked, [1]);
      expect(frames.frame!.seq, 1);
      expect(frames.frame!.size, const Size(1280, 800));
      expect(frames.frame!.origin, 'https://a.example');

      // Two arrive; the older finishes decoding last and is dropped.
      decoder.hold = true;
      late Future<void> second, third;
      await tester.runAsync(() async {
        second = frames.take(2, _jpeg(2, 64, 40));
        third = frames.take(3, _jpeg(3, 32, 20));
        await Future<void>.delayed(Duration.zero);
        decoder.held[3]!.complete();
        await third;
        decoder.held[2]!.complete();
        await second;
      });
      expect(frames.frame!.seq, 3);
      expect(acked, [1, 3]);
      // One older than what's shown is ignored.
      await tester.runAsync(() => frames.take(2, _jpeg(2, 1, 1)));
      expect(frames.frame!.seq, 3);

      frames.clear();
      expect(frames.frame, isNull);
    });
  });

  group('the screen', () {
    Future<(LiveFrames, List<LiveEvent>)> show(
      WidgetTester tester, {
      required bool acting,
    }) async {
      final frames = LiveFrames(ack: (_) {}, decode: _Decoder().call);
      addTearDown(frames.dispose);
      await tester.runAsync(() => frames.take(1, _jpeg(1, 128, 80)));
      final sent = <LiveEvent>[];
      await tester.pumpWidget(
        Directionality(
          textDirection: TextDirection.ltr,
          child: Center(
            child: SizedBox(
              width: 640,
              height: 600,
              child: LiveScreen(
                frames: frames,
                acting: acting,
                mac: true,
                onInput: sent.addAll,
              ),
            ),
          ),
        ),
      );
      return (frames, sent);
    }

    testWidgets('a click lands where it is on the frame', (tester) async {
      final (_, sent) = await show(tester, acting: true);
      // Fitted in a 640×600 box, a 1280×800 frame is 640×400, centred vertically.
      final box = tester.getTopLeft(find.byType(LiveScreen));
      await tester.tapAt(box + const Offset(160, 200));
      await tester.pump();
      expect(sent, [
        const LivePointer(LivePointerKind.down, 0.25, 0.25),
        const LivePointer(LivePointerKind.up, 0.25, 0.25),
      ]);
      sent.clear();
      // In the bars above the frame: nothing.
      await tester.tapAt(box + const Offset(160, 60));
      await tester.pump();
      expect(sent, isEmpty);
    });

    testWidgets('the wheel scrolls in the screen\'s pixels', (tester) async {
      final (_, sent) = await show(tester, acting: true);
      final center = tester.getCenter(find.byType(LiveScreen));
      final pointer = TestPointer(1, PointerDeviceKind.mouse);
      await tester.sendEventToBinding(pointer.hover(center));
      await tester.sendEventToBinding(pointer.scroll(const Offset(0, 50)));
      await tester.pump();
      // Drawn at half size, so 50 points are 100 of the screen's pixels.
      expect(sent.whereType<LiveWheel>(), [const LiveWheel(0, 100, 0.5, 0.5)]);
    });

    testWidgets('nothing goes while it isn\'t the person\'s turn', (
      tester,
    ) async {
      final (_, sent) = await show(tester, acting: false);
      await tester.tap(find.byType(LiveScreen));
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.pump();
      expect(sent, isEmpty);
    });

    testWidgets('keys go once it has the focus; ⌘V is the page\'s own, '
        'never this device\'s clipboard', (tester) async {
      final (_, sent) = await show(tester, acting: true);
      await tester.tap(find.byType(LiveScreen));
      await tester.pump();
      sent.clear();
      await tester.sendKeyEvent(LogicalKeyboardKey.enter);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.keyQ, character: 'q');
      await tester.pump();
      expect(sent, [
        const LiveKey('Enter', down: true),
        const LiveKey('Enter', down: false),
        const LiveTyped('q'),
      ]);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.keyQ);
      sent.clear();
      await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
      await tester.sendKeyEvent(LogicalKeyboardKey.keyV);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
      await tester.pump();
      expect(sent, [
        const LiveKey('v', down: true, control: true),
        const LiveKey('v', down: false, control: true),
      ]);
    });
  });
}
