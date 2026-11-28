import 'package:factory_app/errands/live_input.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('where a point falls on the frame', () {
    test('a frame as wide as the box, letterboxed top and bottom', () {
      const box = Size(800, 800);
      const frame = Size(1280, 800);
      expect(liveFrameRect(box, frame), const Rect.fromLTWH(0, 150, 800, 500));
      expect(
        liveFramePoint(const Offset(400, 400), box, frame),
        const Offset(0.5, 0.5),
      );
      expect(
        liveFramePoint(const Offset(0, 150), box, frame),
        const Offset(0, 0),
      );
      // In the bars, outside the frame.
      expect(liveFramePoint(const Offset(400, 100), box, frame), isNull);
    });

    test('a tall phone frame in a wide box', () {
      const box = Size(1000, 500);
      const frame = Size(390, 844);
      final rect = liveFrameRect(box, frame);
      expect(rect.height, 500);
      expect(rect.center, const Offset(500, 250));
      expect(
        liveFramePoint(rect.bottomRight - const Offset(1, 1), box, frame),
        isNotNull,
      );
    });

    test('a frame smaller than the box drawn no larger than asked', () {
      const box = Size(1600, 1000);
      const frame = Size(1280, 800);
      // One frame pixel to a screen pixel on a 2x screen: 640×400.
      final rect = liveFrameRect(box, frame, maxScale: 0.5);
      expect(rect, const Rect.fromLTWH(480, 300, 640, 400));
      expect(
        liveFramePoint(const Offset(800, 500), box, frame, maxScale: 0.5),
        const Offset(0.5, 0.5),
      );
      expect(
        liveFramePoint(const Offset(100, 500), box, frame, maxScale: 0.5),
        isNull,
      );
    });

    test('nothing yet', () {
      expect(
        liveFramePoint(Offset.zero, const Size(10, 10), Size.zero),
        isNull,
      );
    });
  });

  test('long text goes in pieces, never splitting a character', () {
    expect(liveTextPieces(''), isEmpty);
    expect(liveTextPieces('hello'), ['hello']);
    final long = 'é' * 3000; // 6000 bytes
    final pieces = liveTextPieces(long);
    expect(pieces.length, 2);
    expect(pieces.join(), long);
    expect(pieces.first.length, liveTextMost ~/ 2);
    final emoji = '😀' * 1500; // 4 bytes each
    expect(liveTextPieces(emoji).join(), emoji);
    expect(liveTextPieces(emoji).first.runes.length, liveTextMost ~/ 4);
  });

  group('keys', () {
    KeyDownEvent down(LogicalKeyboardKey key, {String? character}) =>
        KeyDownEvent(
          physicalKey: PhysicalKeyboardKey.keyA,
          logicalKey: key,
          character: character,
          timeStamp: Duration.zero,
        );

    test('a character goes as text, once', () {
      expect(
        liveKeyEvents(down(LogicalKeyboardKey.keyA, character: 'a'), mac: true),
        [const LiveTyped('a')],
      );
      expect(
        liveKeyEvents(
          const KeyUpEvent(
            physicalKey: PhysicalKeyboardKey.keyA,
            logicalKey: LogicalKeyboardKey.keyA,
            timeStamp: Duration.zero,
          ),
          mac: true,
        ),
        isEmpty,
      );
    });

    test('named keys go down and up by name', () {
      expect(liveKeyEvents(down(LogicalKeyboardKey.enter), mac: true), [
        const LiveKey('Enter', down: true),
      ]);
      expect(
        liveKeyEvents(
          const KeyUpEvent(
            physicalKey: PhysicalKeyboardKey.backspace,
            logicalKey: LogicalKeyboardKey.backspace,
            timeStamp: Duration.zero,
          ),
          mac: false,
        ),
        [const LiveKey('Backspace', down: false)],
      );
      // Space types a space.
      expect(
        liveKeyEvents(
          down(LogicalKeyboardKey.space, character: ' '),
          mac: true,
        ),
        [const LiveTyped(' ')],
      );
    });

    test('a modifier alone sends nothing', () {
      expect(
        liveKeyEvents(down(LogicalKeyboardKey.shiftLeft), mac: true),
        isEmpty,
      );
      expect(
        liveKeyEvents(down(LogicalKeyboardKey.metaLeft), mac: true),
        isEmpty,
      );
    });

    testWidgets('⌘A on a Mac is Ctrl+A there', (tester) async {
      await simulateKeyDownEvent(LogicalKeyboardKey.metaLeft);
      expect(
        liveKeyEvents(down(LogicalKeyboardKey.keyA, character: 'a'), mac: true),
        [const LiveKey('a', down: true, control: true)],
      );
      await simulateKeyUpEvent(LogicalKeyboardKey.metaLeft);
      await simulateKeyDownEvent(LogicalKeyboardKey.controlLeft);
      expect(
        liveKeyEvents(
          down(LogicalKeyboardKey.keyA, character: 'a'),
          mac: false,
        ),
        [const LiveKey('a', down: true, control: true)],
      );
      await simulateKeyUpEvent(LogicalKeyboardKey.controlLeft);
    });
  });
}
