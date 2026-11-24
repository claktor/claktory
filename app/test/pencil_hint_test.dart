// Apple Pencil on an image pane (#362 I2): drawing is only on PDFs for
// now, so a Pencil's touch on an image says so at the pane's foot, and how
// to comment instead. A finger, a mouse and anywhere but an iPad: nothing.
import 'package:factory_app/ipad/pencil_hint.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, showPanes;
import 'viewers_test.dart' show png;

const _path = '/w/shot.png';
const _line =
    'You can draw only on PDFs. To comment on this image, tap Comment, then '
    'drag across it.';

Future<void> _showImage(WidgetTester tester) async {
  final data = await png(tester, 400, 200);
  await showPanes(
    tester,
    beside('image', _path),
    files: {
      _path: FileContent(
        path: _path,
        mime: 'image/png',
        size: Int64(data.length),
        data: data,
        sha256: 'v1',
      ),
    },
  );
  await tester.runAsync(
    () => Future<void>.delayed(const Duration(milliseconds: 300)),
  );
  await settle(tester);
}

Future<void> _touch(WidgetTester tester, PointerDeviceKind kind) async {
  final at = tester.getCenter(find.byType(PencilHint));
  final gesture = await tester.startGesture(at, kind: kind);
  await gesture.up();
  await tester.pump();
  await tester.pump(const Duration(milliseconds: 300));
}

double _opacity(WidgetTester tester) => tester
    .widget<AnimatedOpacity>(
      find.ancestor(
        of: find.text(_line),
        matching: find.byType(AnimatedOpacity),
      ),
    )
    .opacity;

void main() {
  final real = PencilHint.onIpad;
  setUp(() => PencilHint.onIpad = () => true);
  tearDown(() => PencilHint.onIpad = real);

  testWidgets("on an iPad, the Pencil's touch on an image says to comment "
      'instead, for a few seconds', (tester) async {
    await _showImage(tester);
    expect(find.byType(PencilHint), findsOneWidget);
    expect(_opacity(tester), 0, reason: 'nothing until the Pencil');
    await _touch(tester, PointerDeviceKind.touch);
    await _touch(tester, PointerDeviceKind.mouse);
    expect(_opacity(tester), 0, reason: 'a finger and a mouse say nothing');
    await _touch(tester, PointerDeviceKind.stylus);
    expect(_opacity(tester), 1);
    await tester.pump(PencilHint.shownFor);
    await tester.pump(const Duration(milliseconds: 300));
    expect(_opacity(tester), 0);
  });

  testWidgets('not on a Mac', (tester) async {
    PencilHint.onIpad = () => false;
    await _showImage(tester);
    expect(find.text(_line), findsNothing);
  });
}
