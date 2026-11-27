// Needs you on an iPhone, drawn: with SHOTS set, PNGs of the list and a
// request tapped there alone with the answer under the thumb (a phone has
// no focus mode), on an iPhone SE and a Pro Max, at the default text size
// and the largest:
//   SHOTS=/tmp/needs-you flutter test test/mobile_needs_you_shots_test.dart
// test-bundle: alone (loads the app's fonts)
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/card_answer.dart' show AnswerDock, CardAnswer;
import 'package:factory_app/inbox.dart' show NeedsYouCard, NeedsYouList;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;

Future<void> _shot(WidgetTester tester, String name) async {
  final folder = Platform.environment['SHOTS'];
  if (folder == null) return;
  final boundary = tester.binding.renderViews.first;
  final layer = boundary.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(
    () => layer.toImage(Offset.zero & boundary.size, pixelRatio: 2),
  );
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}

void main() {
  setUpAll(loadGeist);

  for (final (phone, size, padding) in const [
    ('se', Size(375, 667), FakeViewPadding(top: 20)),
    ('promax', Size(440, 956), FakeViewPadding(top: 62, bottom: 34)),
  ]) {
    for (final scale in const [1.0, 2.35]) {
      testWidgets('$phone at ${scale}x: nothing cut off', (tester) async {
        debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        tester.platformDispatcher.textScaleFactorTestValue = scale;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        tester.view.padding = padding;
        addTearDown(tester.view.resetPadding);
        final now = DateTime.now();
        final harness = Harness(
          Workspace(
            agents: [
              Agent(
                id: 'june',
                name: 'June',
                role: 'Backend',
                modelAvailable: true,
                waitingSeqs: [Int64(1)],
                projectId: 'p',
              ),
              Agent(
                id: 'mista',
                name: 'Mista',
                modelAvailable: true,
                waitingSeqs: [Int64(7)],
              ),
            ],
            projects: [Project(id: 'p', name: 'Claktory')],
            selectedAgentId: 'june',
          ),
        );
        harness.backend.entriesById['june/1'] = Entry(
          seq: Int64(1),
          timestampMs: Int64(
            now.subtract(const Duration(minutes: 12)).millisecondsSinceEpoch,
          ),
          askPerson: AskPerson(
            id: 'q1',
            title: 'Ship the migration tonight?',
            body:
                'It blocks the deploy of #431. The migration adds one column '
                'and backfills it in batches of 10,000; it takes about four '
                'minutes on a copy of production.',
          ),
        );
        harness.backend.entriesById['mista/7'] = Entry(
          seq: Int64(7),
          timestampMs: Int64(now.millisecondsSinceEpoch),
          askPerson: AskPerson(id: 'q7', title: 'Turn on the relay rule?'),
        );
        await harness.show(tester, size: size);
        harness.page.value = AppPage.inbox;
        await pumpFrames(tester);
        await tester.pump();
        await pumpFrames(tester);
        expect(tester.takeException(), isNull);
        await _shot(tester, '$phone-$scale-list');

        // A request tapped: it alone.
        final row = find.descendant(
          of: find.byType(NeedsYouList),
          matching: find.text('Ship the migration tonight?'),
        );
        await tester.ensureVisible(row);
        await pumpFrames(tester);
        await tester.tap(row);
        await pumpFrames(tester);
        expect(tester.takeException(), isNull);
        await _shot(tester, '$phone-$scale-alone');

        // The answer docked at the foot, as the cards publish it.
        final field = TextEditingController();
        addTearDown(field.dispose);
        AnswerDock.of(tester.element(find.byType(NeedsYouCard)))!
            .answer
            .value = CardAnswer(
          docked: true,
          field: field,
          yes: 'Approve',
          no: 'Dismiss',
          onYes: () async {},
          onSubmit: () async {},
          onNo: (_) async {},
        );
        await pumpFrames(tester);
        await tester.pump(const Duration(seconds: 2));
        expect(tester.takeException(), isNull);
        await _shot(tester, '$phone-$scale-dock');
      });
    }
  }
}
