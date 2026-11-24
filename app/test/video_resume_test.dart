// A video's preview coming on a slow link when the connection drops (Sale's
// row 87, Android, 512:2048): back, it carried on from 0, a minute lost on
// that link, since the preview came in one read of up to 8 MB and nothing
// of a read given up is kept. It comes a megabyte at a time now, each
// written as it comes, and back it carries on from the last one.
import 'package:factory_app/backend.dart' show RequestFailure;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/viewers/video.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'panes_test.dart' show beside, snapshot;
import 'video_test.dart' show FakePlayback;

const _path = '/w/clip-b.mp4';
const _size = 30 * 1024 * 1024;
const _previewSize = 4400000;
const _mb = 1024 * 1024;

Future<void> _loading(WidgetTester tester) async {
  for (var i = 0; i < 8; i++) {
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 100)),
    );
    await tester.pump();
  }
  await settle(tester);
}

void main() {
  setUp(() {
    FakePlayback.made.clear();
    newVideoPlayback = FakePlayback.new;
  });

  testWidgets('a preview cut off by a dropped connection carries on from '
      'where it got once it is back', (tester) async {
    final harness = Harness(snapshot(layout: beside('video', _path)));
    var dropped = false;
    harness.backend
      ..rangedReads = true
      // A slow link: the preview, not the original.
      ..linkTime = ((bytes) => Duration(microseconds: bytes * 1000000 ~/ 64000))
      ..files[_path] = FileContent(
        path: _path,
        mime: 'video/mp4',
        size: Int64(_size),
        data: List<int>.filled(_size, 1),
        sha256: 'v1',
      )
      ..previews[_path] = FileContent(
        path: _path,
        mime: 'video/mp4',
        size: Int64(_previewSize),
        data: List<int>.filled(_previewSize, 2),
        sha256: 'v1',
        preview: FilePreview.FILE_PREVIEW_MADE,
        originalSize: Int64(_size),
      )
      // The connection goes as the preview's third megabyte comes.
      ..holdRead = (request) async {
        if (!dropped && request.previewWidth > 0 && request.offset >= 2 * _mb) {
          dropped = true;
          harness.backend.connected.value = false;
          throw const RequestFailure('The connection was lost.');
        }
      };
    await harness.show(tester);
    tester.view.physicalSize = const Size(2000, 900);
    await _loading(tester);
    expect(dropped, isTrue);
    final before = harness.backend.sent.length;

    // Back.
    harness.backend.connected.value = true;
    await _loading(tester);
    final after = [
      for (final m in harness.backend.sent.skip(before))
        if (m.hasReadFile()) m.readFile,
    ];
    // From where it got, the preview's: never its start again, nor the
    // original's.
    final pieces = [
      for (final r in after)
        if (r.maxBytes > 1) r,
    ];
    expect(pieces.every((r) => r.previewWidth > 0), isTrue);
    expect(pieces.first.offset.toInt(), 2 * _mb);
    expect(FakePlayback.made.last.opened, contains('-preview-'));
  });
}
