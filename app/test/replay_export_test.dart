import 'dart:async';
import 'dart:io';

import 'package:factory_app/controls.dart' show Toggle;
import 'package:factory_app/history/controller.dart';
import 'package:factory_app/history/export.dart';
import 'package:factory_app/history/scene.dart';
import 'package:factory_app/history/timeline.dart';
import 'package:factory_app/history/view.dart';
import 'package:factory_app/map_layout.dart' show MapBoxKind, MapLayer;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart' show RenderParagraph;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;
import 'map_fixture.dart';
import 'replay_fixture.dart';

/// History's video export (#458 E1): what a share shows, the video's
/// frames, the file's dates, and the sheet that makes it.
void main() {
  HistoryController loaded() {
    final timeline = HistoryTimeline.of(fixtureOverview());
    final scene = HistoryScene.build(timeline);
    return HistoryController(_NoSource())
      ..timeline = timeline
      ..scene = scene
      ..log = (HistoryLog()..add(fixtureEvents()))
      ..loaded = true;
  }

  group('a share shows only the shape', () {
    final scene = HistoryScene.build(HistoryTimeline.of(fixtureOverview()));
    int chip(String id) => scene.chips.indexWhere((chip) => chip.id == id);
    int box(MapBoxKind kind) =>
        scene.boxes.indexWhere((box) => box.box.kind == kind);

    test('by default', () {
      final words = ShareWords(scene, const ReplayPrivacy());
      expect(words.chip(chip('agent:kai'), 'Kai', 'Backend'), ('Agent', ''));
      expect(words.chip(chip('agent:rivet'), 'Rivet', 'Admin'), ('Agent', ''));
      // A secret's name never shows.
      expect(words.chip(chip('secret:gh'), 'GitHub', ''), ('', ''));
      final project = box(MapBoxKind.project);
      expect(words.box(project), ('Project 1', ''));
      // Another Claktory, never named.
      expect(words.pad(0), '');
      // You, without your initials or colours.
      final you = Person(
        name: 'Matt',
        initials: 'MS',
        avatarFrom: '#112233',
        avatarTo: '#445566',
      );
      expect(words.person(you), Person());
      final names = ShareWords(scene, const ReplayPrivacy(names: true));
      expect(names.person(you), you);
    });

    test('with each turned on', () {
      final all = ShareWords(
        scene,
        const ReplayPrivacy(names: true, roles: true, projects: true),
      );
      expect(all.chip(chip('agent:kai'), 'Kai', 'Backend'), ('Kai', 'Backend'));
      expect(all.box(box(MapBoxKind.project)), ('App', ''));
      expect(all.chip(chip('secret:gh'), 'GitHub', ''), ('', ''));
      final roles = ShareWords(scene, const ReplayPrivacy(roles: true));
      expect(roles.chip(chip('agent:kai'), 'Kai', 'Backend'), ('Backend', ''));
      final names = ShareWords(scene, const ReplayPrivacy(names: true));
      expect(names.chip(chip('agent:kai'), 'Kai', 'Backend'), ('Kai', ''));
      // Never a path.
      for (var i = 0; i < scene.boxes.length; i++) {
        expect(all.box(i).$2, '');
      }
    });
  });

  test('the video fits its length, and steps from the start to the end', () {
    final replay = loaded();
    final video = ReplayVideo(replay, length: ReplayLength.half);
    expect(video.frames, inInclusiveRange(30 * replayFps - 2, 30 * replayFps));
    final words = ShareWords(video.scene, const ReplayPrivacy());
    video.step(0, words);
    expect(video.time, video.timeline.start);
    expect(video.camera, isNotNull);
    expect(video.agents, 1);
    expect(video.messages, 0);
    // Kai fades in over a few frames once there.
    final kaiAt = day1 + 2 * hour;
    var i = 0;
    while (video.time < kaiAt) {
      video.step(++i, words);
    }
    final kai = video.still!.chips
        .where((c) => video.scene.chips[c.chip].id == 'agent:kai')
        .single;
    expect(kai.opacity, greaterThan(0));
    expect(kai.opacity, lessThan(1));
    for (var k = 0; k < 12; k++) {
      video.step(++i, words);
    }
    expect(
      video.still!.chips
          .where((c) => video.scene.chips[c.chip].id == 'agent:kai')
          .single
          .opacity,
      1,
    );
    // The last frame is the end, with every message sent.
    video.step(video.frames - 1, words);
    expect(video.time, video.video.end);
    expect(video.messages, 6);
    expect(video.agents, 3);
    // Only the team: no secrets, nor their lines.
    final gh = video.scene.chips.indexWhere((c) => c.id == 'secret:gh');
    expect(video.scene.at(video.time).chips[gh], isNot(hidden));
    expect(video.frame!.chips[gh], hidden);
    expect(video.still!.chips.where((c) => c.chip == gh), isEmpty);
    for (var w = 0; w < video.scene.wires.length; w++) {
      if (video.scene.wires[w].wire.layer == MapLayer.secrets) {
        expect(video.frame!.wires[w], hidden);
      }
    }
    video.dispose();
  });

  test('days count from the first', () {
    final start = DateTime(2026, 11, 18, 23).millisecondsSinceEpoch.toDouble();
    expect(replayDay(start, start), 1);
    expect(
      replayDay(start, DateTime(2026, 11, 19, 1).millisecondsSinceEpoch * 1.0),
      2,
    );
    expect(
      replayDay(start, DateTime(2026, 11, 25, 12).millisecondsSinceEpoch * 1.0),
      8,
    );
  });

  test('the file says nothing of when it was made', () async {
    // app/test/fixtures/replay.mp4: made by ReplayWriter.swift's encoder
    // (RunnerTests/ReplayWriterCheck.swift with REPLAY_KEEP).
    final original = File('test/fixtures/replay.mp4').readAsBytesSync();
    final folder = Directory.systemTemp.createTempSync('replay-dates');
    final file = File('${folder.path}/claktory-replay.mp4')
      ..writeAsBytesSync(original);
    expect(_dates(original), everyElement(isNot(0)));
    await clearMp4Dates(file);
    final cleared = file.readAsBytesSync();
    expect(_dates(cleared), hasLength(6));
    expect(_dates(cleared), everyElement(0));
    // Nothing else changed.
    var differ = 0;
    for (var i = 0; i < original.length; i++) {
      if (original[i] != cleared[i]) differ++;
    }
    expect(differ, lessThanOrEqualTo(6 * 4));
    folder.deleteSync(recursive: true);
  });

  testWidgets('Share replay… makes the video and shares it', (tester) async {
    final calls = <MethodCall>[];
    final shared = <Map<Object?, Object?>>[];
    tester.binding.defaultBinaryMessenger
      ..setMockMethodCallHandler(
        const MethodChannel('claktory/replay-writer'),
        (call) async {
          calls.add(call);
          return switch (call.method) {
            'start' || 'finish' => '/nowhere/claktory-replay.mp4',
            _ => null,
          };
        },
      )
      ..setMockMethodCallHandler(const MethodChannel('factory/device'), (
        call,
      ) async {
        if (call.method == 'shareFiles') {
          shared.add(call.arguments as Map<Object?, Object?>);
          return true;
        }
        return null;
      });
    debugReplayFrames = 3;
    debugReplayCapture = (stage, shape) async =>
        Uint8List(shape.width * shape.height * 4);
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() {
      debugReplayFrames = null;
      debugReplayCapture = null;
      debugDefaultTargetPlatformOverride = null;
    });

    final harness = Harness(
      claktoryWorkspace()..person = Person(name: 'Matt', initials: 'MS'),
    );
    harness.backend
      ..historyReply = fixtureOverview()
      ..historyPages = [fixtureEvents()];
    await harness.show(tester, size: const Size(1440, 900));
    harness.page.value = AppPage.map;
    await pumpFrames(tester);
    await tester.tap(
      find.descendant(
        of: find.byType(MapModeSwitch),
        matching: find.text('History'),
      ),
    );
    await pumpFrames(tester);
    await tester.tap(find.text('Share replay…'));
    await pumpFrames(tester);
    expect(find.text('Share replay'), findsOneWidget);
    expect(find.byType(ReplayStage), findsOneWidget);
    // Everything but the shape is off.
    for (final label in ['Names', 'Roles', 'Project names', 'Dates']) {
      expect(
        tester
            .widget<Toggle>(
              find.byWidgetPredicate(
                (widget) => widget is Toggle && widget.label == label,
              ),
            )
            .value,
        isFalse,
        reason: label,
      );
    }
    // The preview plays.
    for (var i = 0; i < 10; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
    expect(find.textContaining('Day '), findsOneWidget);
    expect(find.text('Made with Claktory'), findsOneWidget);
    // In the preview, no names (the board behind has them), nor your
    // initials.
    expect(
      find.descendant(of: find.byType(ReplayStage), matching: find.text('MS')),
      findsNothing,
    );
    expect(find.text('MS'), findsWidgets);
    expect(
      find.descendant(of: find.byType(ReplayStage), matching: find.text('Kai')),
      findsNothing,
    );
    expect(
      find.descendant(
        of: find.byType(ReplayStage),
        matching: find.text('Agent'),
      ),
      findsWidgets,
    );

    // With dates on, the file's left as made.
    await tester.tap(
      find.byWidgetPredicate(
        (widget) => widget is Toggle && widget.label == 'Dates',
      ),
    );
    await tester.pump();
    await tester.tap(find.text('Make video'));
    for (var i = 0; i < 20 && shared.isEmpty; i++) {
      await tester.pump(const Duration(milliseconds: 16));
    }
    expect(
      [for (final call in calls) call.method],
      ['start', 'frame', 'frame', 'frame', 'finish'],
    );
    expect(calls.first.arguments, {'width': 1080, 'height': 1080, 'fps': 30});
    expect(
      ((calls[1].arguments as Map)['rgba'] as Uint8List).length,
      1080 * 1080 * 4,
    );
    expect(shared.single['paths'], ['/nowhere/claktory-replay.mp4']);
    expect(shared.single['mimes'], ['video/mp4']);
    await pumpFrames(tester);
    expect(find.text('Share replay'), findsNothing);
    expect(tester.takeException(), isNull);
  });

  group('at the largest text', () {
    // The real font's widths: the test font's squares wrap everything.
    setUpAll(loadGeist);

    testWidgets("on a small phone, Shape's and Length's choices read whole "
        "under their names (Zucchero's row 512)", (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 3.12;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final harness = Harness(claktoryWorkspace());
      harness.backend
        ..historyReply = fixtureOverview()
        ..historyPages = [fixtureEvents()];
      await harness.show(tester, size: const Size(375, 667));
      harness.page.value = AppPage.map;
      await pumpFrames(tester);
      await tester.tap(find.bySemanticsLabel('History'));
      await pumpFrames(tester);
      await tester.tap(find.bySemanticsLabel('Share replay…').last);
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
      for (final choice in ['Portrait', '30 seconds']) {
        final words = find.text(choice);
        expect(words, findsOneWidget, reason: choice);
        final paragraph = tester.renderObject<RenderParagraph>(words);
        // One line (two were 104 pt, three 240).
        expect(paragraph.size.height, lessThan(80), reason: choice);
      }
    });
  });
}

/// The mvhd, tkhd and mdhd boxes' two dates each, wherever they are.
List<int> _dates(Uint8List bytes) {
  final data = ByteData.sublistView(bytes);
  final out = <int>[];
  for (final name in ['mvhd', 'tkhd', 'mdhd']) {
    final tag = name.codeUnits;
    for (var i = 4; i + 4 <= bytes.length; i++) {
      if (bytes[i] == tag[0] &&
          bytes[i + 1] == tag[1] &&
          bytes[i + 2] == tag[2] &&
          bytes[i + 3] == tag[3]) {
        final version = bytes[i + 4];
        if (version == 0) {
          out
            ..add(data.getUint32(i + 8))
            ..add(data.getUint32(i + 12));
        }
      }
    }
  }
  return out;
}

class _NoSource implements HistorySource {
  @override
  Future<HistoryOverview> overview() => Completer<HistoryOverview>().future;

  @override
  Future<HistoryEvents> events(List<int> cursor, {double? until}) =>
      Completer<HistoryEvents>().future;

  @override
  Future<void> watch(bool on) async {}

  @override
  Stream<HistoryEvents> get pushes => const Stream.empty();

  @override
  ValueListenable<Int64> get changed => ValueNotifier(Int64.ZERO);
}
