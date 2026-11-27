import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/controls.dart' show Button, GlyphButton, Toggle;
import 'package:factory_app/history/board.dart' show HistoryBoard;
import 'package:factory_app/history/controller.dart';
import 'package:factory_app/history/export.dart';
import 'package:factory_app/history/export_job.dart';
import 'package:factory_app/history/scene.dart';
import 'package:factory_app/history/timeline.dart';
import 'package:factory_app/history/view.dart';
import 'package:factory_app/identity.dart' show TextStyles;
import 'package:factory_app/map_layout.dart' show MapBoxKind, MapLayer;
import 'package:factory_app/panes.dart' show ExpandedPane, Pane;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/share_files.dart' show debugSharesFolder;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart'
    show OffsetLayer, RenderParagraph, RenderRepaintBoundary;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'panes_test.dart' show leaf, row, snapshot, split;
import 'phone_sizes_test.dart' show loadGeist;
import 'map_fixture.dart';
import 'replay_fixture.dart';
import 'replay_test.dart' show bigOverview;

/// History's video export: what a share shows, the video's
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
      // A pipe, named only with names.
      expect(names.pad(0), scene.pads[0].label);
      expect(
        ShareWords(
          scene,
          const ReplayPrivacy(roles: true, projects: true),
        ).pad(0),
        '',
      );
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

  testWidgets('Share replay… makes the video in the background, saves it in '
      'Movies › Claktory and shares it from there', (tester) async {
    final calls = <MethodCall>[];
    final shared = <Map<Object?, Object?>>[];
    final temp = Directory.systemTemp.createTempSync('replay-job');
    final made = File('${temp.path}/share/ABC/claktory-replay.mp4');
    final movies = Directory('${temp.path}/Movies/Claktory');
    tester.binding.defaultBinaryMessenger
      ..setMockMethodCallHandler(
        const MethodChannel('claktory/replay-writer'),
        (call) async {
          calls.add(call);
          switch (call.method) {
            case 'start':
              made.parent.createSync(recursive: true);
              made.writeAsBytesSync(const []);
              return made.path;
            case 'finish':
              made.writeAsBytesSync(const [0, 0, 0, 8, 102, 114, 101, 101]);
              return made.path;
          }
          return null;
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
    debugReplayFolder = movies;
    debugReplayCapture = (stage, shape) async =>
        Uint8List(shape.width * shape.height * 4);
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() {
      debugReplayFrames = null;
      debugReplayCapture = null;
      debugReplayFolder = null;
      replayJobs.dismiss();
      temp.deleteSync(recursive: true);
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
    expect(find.byType(ReplayWatermark), findsOneWidget);
    expect(
      find.textContaining('Made with Claktory · claktory.com'),
      findsOneWidget,
    );
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
    await pumpFrames(tester);
    // Made in the background: the export closes at once, and the window
    // says how far it is, wherever the person goes.
    expect(find.text('Share replay'), findsNothing);
    expect(replayJobs.current, isNotNull);
    for (var i = 0; i < 100 && !replayJobs.current!.done; i++) {
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 10)),
      );
      await tester.pump();
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
    // Saved first, where everyone knows to look, with its date in its name.
    final job = replayJobs.current!;
    expect(job.state, ReplayJobState.saved);
    expect(job.savedTo, ReplaySavedTo.movies);
    final saved = movies.listSync().whereType<File>().single;
    expect(saved.path, job.file!.path);
    expect(
      saved.uri.pathSegments.last,
      matches(
        RegExp(r'^Claktory History \d{4}-\d\d-\d\d at \d\d\.\d\d\.\d\d\.mp4$'),
      ),
    );
    // Nothing's left in the temporary folder.
    for (var i = 0; i < 50 && made.parent.existsSync(); i++) {
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 10)),
      );
    }
    expect(made.parent.existsSync(), isFalse);
    await tester.pump();
    expect(
      find.text('Your History video is in Movies › Claktory.'),
      findsOneWidget,
    );
    expect(find.text(saved.uri.pathSegments.last), findsOneWidget);
    expect(find.text('Show in Finder'), findsOneWidget);
    await tester.tap(find.text('Share…'));
    for (var i = 0; i < 20 && shared.isEmpty; i++) {
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 10)),
      );
      await tester.pump();
    }
    expect(shared.single['paths'], [saved.path]);
    expect(shared.single['mimes'], ['video/mp4']);
    // Shared, it's still there.
    expect(saved.existsSync(), isTrue);
    await tester.tap(find.byKey(const ValueKey('replay-job-close')));
    await tester.pump();
    expect(replayJobs.current, isNull);
    expect(
      find.text('Your History video is in Movies › Claktory.'),
      findsNothing,
    );
    expect(saved.existsSync(), isTrue);
    expect(tester.takeException(), isNull);
  });

  group('a video being made', () {
    late Directory temp;
    late File made;
    late List<MethodCall> calls;

    /// The writer, its frames held until [hold] completes, if given;
    /// [device] answers factory/device, [photos] saveToPhotos.
    void writer(
      WidgetTester tester, {
      Completer<void>? hold,
      Future<Object?> Function(MethodCall call)? device,
      Object? Function()? photos,
      bool files = true,
    }) {
      tester.binding.defaultBinaryMessenger
        ..setMockMethodCallHandler(
          const MethodChannel('claktory/replay-writer'),
          (call) async {
            calls.add(call);
            switch (call.method) {
              case 'start':
                made.parent.createSync(recursive: true);
                made.writeAsBytesSync(const []);
                return made.path;
              case 'frame':
                await hold?.future;
              case 'finish':
                made.writeAsBytesSync(const [0, 0, 0, 8]);
                return made.path;
              case 'saveToPhotos':
                return photos?.call();
              case 'saveToFiles':
                return files;
            }
            return null;
          },
        )
        ..setMockMethodCallHandler(
          const MethodChannel('factory/device'),
          device == null ? null : (call) => device(call),
        );
    }

    ReplayJob job() => ReplayJob(
      video: ReplayVideo(loaded(), length: ReplayLength.half),
      shape: ReplayShape.x,
      privacy: const ReplayPrivacy(dates: true),
      person: Person(),
      media: const MediaQueryData(),
    );

    /// Real time for the files, then a frame.
    Future<void> settle(WidgetTester tester, bool Function() until) async {
      for (var i = 0; i < 100 && !until(); i++) {
        await tester.runAsync(
          () => Future<void>.delayed(const Duration(milliseconds: 10)),
        );
        await tester.pump();
      }
      expect(until(), isTrue);
    }

    setUp(() {
      temp = Directory.systemTemp.createTempSync('replay-jobs');
      made = File('${temp.path}/share/ABC/claktory-replay.mp4');
      calls = [];
      debugReplayFrames = 3;
      debugReplayFolder = Directory('${temp.path}/Movies/Claktory');
      debugReplayCapture = (stage, shape) async =>
          Uint8List(shape.width * shape.height * 4);
    });

    tearDown(() {
      replayJobs.dismiss();
      debugReplayFrames = null;
      debugReplayFolder = null;
      debugReplayCapture = null;
      temp.deleteSync(recursive: true);
    });

    testWidgets('shows how far it is wherever the person is, and Cancel '
        'stops it and deletes what was made', (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final hold = Completer<void>();
      writer(tester, hold: hold);
      await Harness(claktoryWorkspace()).show(tester);
      replayJobs.start(job());
      await settle(tester, () => calls.any((c) => c.method == 'frame'));
      expect(find.text('Making your History video… 0%'), findsOneWidget);
      expect(find.byType(ReplayProgressBar), findsOneWidget);
      // A file in the making is kept from the sweep however long it takes.
      expect(File('${made.parent.path}/.keep').existsSync(), isTrue);
      await tester.tap(find.text('Cancel'));
      hold.complete();
      await settle(tester, () => calls.any((c) => c.method == 'cancel'));
      expect(replayJobs.current, isNull);
      expect(find.textContaining('Making your History video'), findsNothing);
      expect(calls.where((c) => c.method == 'finish'), isEmpty);
      expect(tester.takeException(), isNull);
    });

    for (final ipad in [false, true]) {
      testWidgets('on ${ipad ? 'an iPad' : 'a Mac'}, it shows in a pane '
          'expanded over the window (the Map), above the pane rather than '
          'over its controls (Matt, Bruno 52981)', (tester) async {
        if (ipad) {
          asIPad();
        } else {
          debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
          addTearDown(() => debugDefaultTargetPlatformOverride = null);
        }
        final hold = Completer<void>();
        addTearDown(() {
          if (!hold.isCompleted) hold.complete();
        });
        writer(tester, hold: hold);
        final workspace = snapshot(
          layout: split('s1', row, [leaf('chat', 'chat'), leaf('m1', 'map')]),
        );
        workspace.agents.last.layoutFocus = 'm1';
        final harness = Harness(workspace);
        harness.backend
          ..historyReply = fixtureOverview()
          ..historyPages = [fixtureEvents()];
        await harness.show(tester, size: const Size(1194, 834));
        await pumpFrames(tester);
        final expanded = find.byType(ExpandedPane);
        expect(expanded, findsOneWidget);
        replayJobs.start(job());
        await settle(tester, () => calls.any((c) => c.method == 'frame'));
        await tester.pump();
        final strip = find.descendant(
          of: expanded,
          matching: find.text('Making your History video… 0%'),
        );
        expect(strip, findsOneWidget);
        final pane = find.descendant(of: expanded, matching: find.byType(Pane));
        expect(
          tester.getRect(strip).bottom,
          lessThanOrEqualTo(tester.getRect(pane.first).top),
        );
        expect(
          find.descendant(of: expanded, matching: find.text('Cancel')),
          findsOneWidget,
        );
        expect(tester.takeException(), isNull);
      });
    }

    testWidgets('while one is made, Share replay says why Make video waits, '
        "at its top on the smallest iPhone (Zucchero's row 662)", (
      tester,
    ) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final hold = Completer<void>();
      addTearDown(() {
        if (!hold.isCompleted) hold.complete();
      });
      writer(tester, hold: hold);
      final harness = Harness(claktoryWorkspace());
      harness.backend
        ..historyReply = fixtureOverview()
        ..historyPages = [fixtureEvents()];
      await harness.show(tester, size: const Size(375, 667));
      replayJobs.start(job());
      await settle(tester, () => calls.any((c) => c.method == 'frame'));
      harness.page.value = AppPage.map;
      await pumpFrames(tester);
      await tester.tap(find.bySemanticsLabel('History'));
      await pumpFrames(tester);
      // The strip shows over the phone's History page too.
      expect(find.text('Making your History video… 0%'), findsOneWidget);
      await tester.tap(find.bySemanticsLabel('Share replay…').last);
      await pumpFrames(tester);
      final why = find.text(
        "Another video is being made. You can make this one once it's done.",
      );
      expect(why, findsOneWidget);
      expect(tester.getRect(why).bottom, lessThan(667 / 2));
      expect(
        tester.getRect(why).top,
        lessThan(tester.getRect(find.byType(ReplayStage)).top),
      );
      expect(find.textContaining('made in the background'), findsNothing);
      expect(tester.takeException(), isNull);
    });

    testWidgets("on an iPhone, Photos refused: it's kept, never deleted "
        'without asking, and Save to Files… saves it', (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      writer(tester, photos: () => throw PlatformException(code: 'denied'));
      await Harness(claktoryWorkspace())
          .show(tester, size: const Size(390, 844));
      replayJobs.start(job());
      await settle(tester, () => replayJobs.current!.done);
      expect(replayJobs.current!.state, ReplayJobState.unsaved);
      await tester.pump();
      expect(find.text("Your History video isn't saved yet."), findsOneWidget);
      expect(
        find.text(
          "Claktory isn't allowed to add to your Photos. Save it to Files, "
          'or allow it for Claktory in the Settings app.',
        ),
        findsOneWidget,
      );
      expect(made.existsSync(), isTrue);
      expect(File('${made.parent.path}/.keep').existsSync(), isTrue);
      // Closing it asks first; kept, it stays.
      // (The phone's list of agents is open over the strip here.)
      tester
          .widget<GlyphButton>(find.byKey(const ValueKey('replay-job-close')))
          .onTap();
      await pumpFrames(tester);
      expect(find.text('Delete video'), findsOneWidget);
      await tester.tap(find.text('Keep it'));
      for (var i = 0; i < 10; i++) {
        await tester.pump(const Duration(milliseconds: 100));
      }
      expect(find.text('Keep it'), findsNothing);
      expect(replayJobs.current, isNotNull);
      expect(made.existsSync(), isTrue);
      // The phone's list of agents is open over it here.
      tester
          .widget<Button>(find.widgetWithText(Button, 'Save to Files…'))
          .onPressed!();
      await settle(
        tester,
        () => replayJobs.current!.state == ReplayJobState.saved,
      );
      final save = calls.singleWhere((c) => c.method == 'saveToFiles');
      expect((save.arguments as Map)['path'], made.path);
      expect(
        (save.arguments as Map)['name'] as String,
        startsWith('Claktory History '),
      );
      await tester.pump();
      expect(find.text('Your History video is in Files.'), findsOneWidget);
      // Saved, the sweep may take the copy here again.
      expect(File('${made.parent.path}/.keep').existsSync(), isFalse);
      expect(tester.takeException(), isNull);
    });

    testWidgets('without Dates, Share… sends a copy whose name has no date '
        '(Tiziano)', (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      debugSharesFolder = Directory('${temp.path}/shares');
      addTearDown(() => debugSharesFolder = null);
      final shared = <String>[];
      writer(
        tester,
        device: (call) async {
          if (call.method != 'shareFiles') return null;
          shared.addAll(
            ((call.arguments as Map)['paths'] as List).cast<String>(),
          );
          return true;
        },
      );
      await Harness(claktoryWorkspace()).show(tester);
      replayJobs.start(
        ReplayJob(
          video: ReplayVideo(loaded(), length: ReplayLength.half),
          shape: ReplayShape.x,
          privacy: const ReplayPrivacy(),
          person: Person(),
          media: const MediaQueryData(),
        ),
      );
      await settle(tester, () => replayJobs.current!.done);
      await tester.pump();
      await tester.tap(find.text('Share…'));
      await settle(tester, () => shared.isNotEmpty);
      final copy = File(shared.single);
      expect(copy.uri.pathSegments.last, 'Claktory History.mp4');
      expect(copy.path, startsWith(debugSharesFolder!.path));
      expect(File('${copy.parent.path}/.sent').existsSync(), isTrue);
      // The one in Movies keeps its dated name.
      expect(
        replayJobs.current!.file!.uri.pathSegments.last,
        startsWith('Claktory History 2'),
      );
      expect(tester.takeException(), isNull);
    });

    testWidgets("says so when the share menu can't open", (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      writer(
        tester,
        device: (call) async => call.method == 'shareFiles'
            ? throw PlatformException(
                code: 'unavailable',
                message: "Claktory's window isn't open.",
              )
            : null,
      );
      await Harness(claktoryWorkspace()).show(tester);
      replayJobs.start(job());
      await settle(tester, () => replayJobs.current!.done);
      await tester.pump();
      await tester.tap(find.text('Share…'));
      await settle(tester, () => replayJobs.current!.error != null);
      await tester.pump();
      expect(
        find.text(
          "Couldn't open the share menu: Claktory's window isn't open.",
        ),
        findsOneWidget,
      );
      // Still saved, and still there.
      expect(replayJobs.current!.file!.existsSync(), isTrue);
      expect(tester.takeException(), isNull);
    });
  });

  group("the video's numbers hold still (Matt, Bruno 53194)", () {
    test('counts read as Trish wrote them (53232)', () {
      final cases = {
        0: '0',
        1: '1',
        1204: '1,204',
        9999: '9,999',
        10000: '10K',
        12400: '12.4K',
        99960: '100K',
        124000: '124K',
        999600: '1M',
        2300000: '2.3M',
        23000000: '23M',
        999600000: '1B',
        1200000000: '1.2B',
        12000000000: '12B',
      };
      for (final MapEntry(key: n, value: shown) in cases.entries) {
        expect(compactCount(n), shown, reason: '$n');
      }
      expect(countOf(1, 'message', 'messages'), '1 message');
      expect(countOf(0, 'token', 'tokens'), '0 tokens');
      expect(countShapes(1204), ['1,204']);
      expect(countShapes(2300000), contains('99.9K'));
    });

    Future<void> stats(
      WidgetTester tester, {
      required int working,
      int agents = 23,
      required int messages,
    }) => tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Align(
          alignment: Alignment.topLeft,
          child: SizedBox(
            width: 500,
            child: ReplayStats(
              working: working,
              agents: agents,
              mostAgents: 23,
              messages: messages,
              mostMessages: 1204,
              style: TextStyles.caption,
            ),
          ),
        ),
      ),
    );

    testWidgets('none working still reads "0 of 23 agents working"', (
      tester,
    ) async {
      await stats(tester, working: 0, messages: 1);
      expect(find.text('0'), findsOneWidget);
      expect(find.text(' of '), findsOneWidget);
      expect(find.text('23'), findsOneWidget);
      expect(find.text(' agents working'), findsOneWidget);
      expect(find.text(' message'), findsOneWidget);
      await stats(tester, working: 1, agents: 1, messages: 0);
      expect(find.text(' agent working'), findsOneWidget, reason: '1 of 1');
      expect(find.text(' messages'), findsOneWidget);
    });

    test("the tokens each hour used count up through it, from the "
        "Claktory's History", () {
      final overview = fixtureOverview()
        ..tokens.addAll([Int64(1000), Int64(3000)]);
      final timeline = HistoryTimeline.of(overview);
      final replay = HistoryController(_NoSource())
        ..timeline = timeline
        ..scene = HistoryScene.build(timeline)
        ..log = (HistoryLog()..add(fixtureEvents()))
        ..loaded = true;
      final video = ReplayVideo(replay, length: ReplayLength.half);
      const hour = Duration.millisecondsPerHour;
      final start = timeline.histogramStart;
      expect(video.tokensAt(start - hour), 0);
      expect(video.tokensAt(start + hour / 2), 500);
      expect(video.tokensAt(start + hour), 1000);
      expect(video.tokensAt(start + hour * 1.5), 2500);
      expect(video.tokensAt(start + hour * 5), 4000);
      // A Claktory that doesn't count them: no tokens line at all.
      final none = HistoryController(_NoSource())
        ..timeline = HistoryTimeline.of(fixtureOverview())
        ..scene = replay.scene
        ..log = replay.log
        ..loaded = true;
      expect(
        ReplayVideo(none, length: ReplayLength.half).tokensAt(start),
        isNull,
      );
    });

    testWidgets('tokens follow the messages on the second line', (
      tester,
    ) async {
      await tester.pumpWidget(
        const Directionality(
          textDirection: TextDirection.ltr,
          child: Align(
            alignment: Alignment.topLeft,
            child: ReplayStats(
              working: 1,
              agents: 1,
              mostAgents: 1,
              messages: 1204,
              mostMessages: 1204,
              tokens: 2300000,
              mostTokens: 2300000,
              lines: 2,
              style: TextStyles.caption,
            ),
          ),
        ),
      );
      expect(find.text(' agent working'), findsOneWidget);
      expect(find.text('2.3M'), findsOneWidget);
      expect(find.text(' tokens'), findsOneWidget);
      final tokens = tester.getRect(find.text(' tokens'));
      expect(
        tokens.center.dy,
        closeTo(tester.getCenter(find.text(' messages')).dy, 0.5),
      );
      expect(
        tokens.top,
        greaterThanOrEqualTo(tester.getRect(find.text(' of ')).bottom - 0.5),
      );
    });

    testWidgets('nothing moves sideways as the numbers grow', (tester) async {
      Map<String, double> at() => {
        for (final text in [' of ', ' · '])
          text: tester.getTopLeft(find.text(text).first).dx,
        'agents': tester.getTopLeft(find.textContaining('agent')).dx,
        'messages': tester.getTopLeft(find.textContaining('message')).dx,
      };
      await stats(tester, working: 0, messages: 1);
      final first = at();
      for (final (working, messages) in const [
        (3, 12),
        (12, 980),
        (23, 1204),
      ]) {
        await stats(tester, working: working, messages: messages);
        expect(at(), first, reason: '$working working, $messages messages');
      }
      // A number inside the line ends where its room does: right-aligned.
      await stats(tester, working: 3, messages: 12);
      final twelve = tester.getRect(find.text('12'));
      await stats(tester, working: 3, messages: 980);
      expect(tester.getRect(find.text('980')).right, twelve.right);
      // One starting the line starts where the line does.
      final line = tester.getTopLeft(find.byType(ReplayStats)).dx;
      expect(tester.getTopLeft(find.text('3')).dx, line);
    });
  });

  group('made for a place (history-video.md, Lucy 53440)', () {
    setUpAll(loadGeist);

    test('the camera scales a small team up to 2.5 times, to fill the '
        'band, and a big one down (Lucy 53709)', () {
      double scale(Rect camera, Rect area) =>
          ReplayStage.fit(camera, area).entry(0, 0);
      const area = Rect.fromLTWH(60, 120, 400, 200);
      expect(scale(const Rect.fromLTWH(0, 0, 100, 50), area), 2.5);
      expect(scale(const Rect.fromLTWH(0, 0, 200, 50), area), 2);
      expect(scale(const Rect.fromLTWH(0, 0, 100, 160), area), 1.25);
      expect(scale(const Rect.fromLTWH(0, 0, 800, 100), area), 0.5);
      // Centred in the band.
      final m = ReplayStage.fit(const Rect.fromLTWH(10, 10, 100, 50), area);
      expect(MatrixUtils.transformPoint(m, const Offset(60, 35)), area.center);
    });

    for (final team in ['small', 'big']) {
      for (final shape in [ReplayShape.x, ReplayShape.vertical]) {
        testWidgets('${shape.title}: a $team team '
            '${team == 'small' ? 'fills the band, its names large enough to read' : 'fits it, no larger than life'} '
            '(Lucy 53709)', (tester) async {
          final HistoryController replay;
          if (team == 'small') {
            replay = loaded();
          } else {
            final timeline = HistoryTimeline.of(bigOverview());
            replay = HistoryController(_NoSource())
              ..timeline = timeline
              ..scene = HistoryScene.build(timeline)
              ..log = HistoryLog()
              ..loaded = true;
          }
          const privacy = ReplayPrivacy(names: true, projects: true);
          final video = ReplayVideo(replay, length: ReplayLength.half);
          final words = ShareWords(video.scene, privacy);
          video.step(video.frames - 1, words);
          final boundary = GlobalKey();
          await Harness(claktoryWorkspace()).show(
            tester,
            size: const Size(1000, 1000),
            home: Align(
              alignment: Alignment.topLeft,
              child: RepaintBoundary(
                key: boundary,
                child: ReplayStage(
                  video: video,
                  shape: shape,
                  privacy: privacy,
                  person: Person(name: 'Matt', initials: 'MS'),
                ),
              ),
            ),
          );
          await tester.pump();
          final label = find.descendant(
            of: find.byType(HistoryBoard),
            matching: find.text(team == 'small' ? 'Ana' : 'Agent 0.0'),
          );
          final height = tester.getRect(label.first).height;
          expect(tester.takeException(), isNull);
          final folder = Platform.environment['SHOTS'];
          final context = boundary.currentContext;
          if (folder != null && context != null) {
            final render = context.findRenderObject()! as RenderRepaintBoundary;
            final image = await tester.runAsync(
              () => render.toImage(pixelRatio: 2),
            );
            final bytes = await tester.runAsync(
              () => image!.toByteData(format: ui.ImageByteFormat.png),
            );
            Directory(folder).createSync(recursive: true);
            File('$folder/${shape.name}-$team-team.png')
                .writeAsBytesSync(bytes!.buffer.asUint8List());
          }
          // A big team no larger than life; a small one as large as the
          // band lets it be (the fixture's is tall, so its height decides).
          if (team == 'big') expect(height, lessThanOrEqualTo(20));
        });
      }
    }

    for (final talks in [true, false]) {
      testWidgets(
        talks
            ? 'You, sending and getting messages, are framed with the team'
            : "You, in none of its messages, aren't drawn rather than cut at "
                  'the edge',
        (tester) async {
          final replay = loaded();
          if (!talks) replay.log = HistoryLog();
          final video = ReplayVideo(replay, length: ReplayLength.half);
          const privacy = ReplayPrivacy();
          final words = ShareWords(video.scene, privacy);
          video.step(video.frames - 1, words);
          expect(video.youTalk, talks);
          final you = video.scene.layout.you!;
          expect(video.camera!.contains(you.center), talks);
          await Harness(claktoryWorkspace()).show(
            tester,
            size: const Size(1000, 1000),
            home: Align(
              alignment: Alignment.topLeft,
              child: ReplayStage(
                video: video,
                shape: ReplayShape.x,
                privacy: privacy,
                person: Person(name: 'Matt', initials: 'MS'),
              ),
            ),
          );
          await tester.pump();
          expect(
            find.descendant(
              of: find.byType(HistoryBoard),
              matching: find.text('You'),
            ),
            talks ? findsOneWidget : findsNothing,
          );
        },
      );
    }

    for (final shape in ReplayShape.values) {
      for (final named in [true, false]) {
        testWidgets(
          '${shape.title}, ${named ? 'with dates, names and the '
                    'project' : 'Day N, nothing named'}: every word, the watermark '
          'and the numbers stay inside its box, never cut',
          (tester) async {
            final replay = loaded();
            if (named) replay.project = 'app';
            final video = ReplayVideo(replay, length: ReplayLength.half);
            final privacy = ReplayPrivacy(
              names: named,
              roles: named,
              projects: named,
              dates: named,
            );
            final words = ShareWords(video.scene, privacy);
            video
              ..step(0, words)
              ..step(video.frames * 2 ~/ 3, words);
            final boundary = GlobalKey();
            await Harness(claktoryWorkspace()).show(
              tester,
              size: const Size(1000, 1000),
              home: Align(
                alignment: Alignment.topLeft,
                child: RepaintBoundary(
                  key: boundary,
                  child: ReplayStage(
                    video: video,
                    shape: shape,
                    privacy: privacy,
                    person: Person(name: 'Matt', initials: 'MS'),
                  ),
                ),
              ),
            );
            await tester.pump();
            final stage = find.byType(ReplayStage);
            expect(tester.getSize(stage), shape.logical);
            final safe = shape.safe.shift(tester.getTopLeft(stage));
            final board = find.byType(HistoryBoard);
            final lines = [
              for (final element
                  in find
                      .descendant(of: stage, matching: find.byType(RichText))
                      .evaluate())
                if (find
                    .descendant(
                      of: board,
                      matching: find.byWidget(element.widget),
                    )
                    .evaluate()
                    .isEmpty)
                  element,
            ];
            expect(lines, isNotEmpty);
            for (final element in lines) {
              final paragraph = element.renderObject! as RenderParagraph;
              final rect = tester.getRect(find.byWidget(element.widget));
              final text = paragraph.text.toPlainText();
              if (text.trim().isEmpty) continue;
              expect(
                rect.left,
                greaterThanOrEqualTo(safe.left - 0.5),
                reason: text,
              );
              expect(
                rect.top,
                greaterThanOrEqualTo(safe.top - 0.5),
                reason: text,
              );
              expect(
                rect.bottom,
                lessThanOrEqualTo(safe.bottom + 0.5),
                reason: text,
              );
              expect(
                rect.left + paragraph.getMaxIntrinsicWidth(double.infinity),
                lessThanOrEqualTo(safe.right + 0.5),
                reason: '$text, whole',
              );
            }
            final mark = tester.getRect(find.byType(ReplayWatermark));
            expect(mark.left, safe.left, reason: 'left, under the chapter');
            expect(mark.bottom, closeTo(safe.bottom, 0.5));
            // The numbers on two lines in a social network's box.
            final team = tester.getRect(find.text(' of '));
            final talk = tester.getRect(find.textContaining('message'));
            if (shape.statLines == 2) {
              expect(talk.top, greaterThanOrEqualTo(team.bottom - 0.5));
            } else {
              expect(talk.center.dy, closeTo(team.center.dy, 0.5));
            }
            expect(tester.takeException(), isNull);
            // SHOTS=/tmp/replay flutter test test/replay_export_test.dart
            final folder = Platform.environment['SHOTS'];
            if (folder != null) {
              final render =
                  boundary.currentContext!.findRenderObject()!
                      as RenderRepaintBoundary;
              final image = await tester.runAsync(
                () => render.toImage(pixelRatio: 2),
              );
              final bytes = await tester.runAsync(
                () => image!.toByteData(format: ui.ImageByteFormat.png),
              );
              Directory(folder).createSync(recursive: true);
              File('$folder/${shape.name}-${named ? 'named' : 'plain'}.png')
                  .writeAsBytesSync(bytes!.buffer.asUint8List());
            }
          },
        );
      }
    }

    for (final phone in [false, true]) {
      testWidgets('the sheet offers Made for with its caption, X by default '
          '${phone ? 'on an iPhone' : 'on a Mac'}', (tester) async {
        debugDefaultTargetPlatformOverride = phone
            ? TargetPlatform.iOS
            : TargetPlatform.macOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        final harness = Harness(claktoryWorkspace());
        harness.backend
          ..historyReply = fixtureOverview()
          ..historyPages = [fixtureEvents()];
        final size = phone ? const Size(390, 844) : const Size(1440, 900);
        await harness.show(tester, size: size);
        harness.page.value = AppPage.map;
        await pumpFrames(tester);
        if (phone) {
          await tester.tap(find.bySemanticsLabel('History'));
          await pumpFrames(tester);
          await tester.tap(find.bySemanticsLabel('Share replay…').last);
        } else {
          await tester.tap(
            find.descendant(
              of: find.byType(MapModeSwitch),
              matching: find.text('History'),
            ),
          );
          await pumpFrames(tester);
          await tester.tap(find.text('Share replay…'));
        }
        await pumpFrames(tester);
        expect(find.text('Made for'), findsOneWidget);
        expect(find.text('Shape'), findsNothing);
        expect(find.text('X'), findsOneWidget);
        expect(find.text(ReplayShape.x.caption), findsOneWidget);
        expect(
          tester.widget<ReplayStage>(find.byType(ReplayStage)).shape,
          ReplayShape.x,
        );
        expect(tester.takeException(), isNull);
        final folder = Platform.environment['SHOTS'];
        if (folder != null) {
          final view = tester.binding.renderViews.first;
          final layer = view.debugLayer! as OffsetLayer;
          final image = await tester.runAsync(
            () => layer.toImage(Offset.zero & size, pixelRatio: 2),
          );
          final bytes = await tester.runAsync(
            () => image!.toByteData(format: ui.ImageByteFormat.png),
          );
          Directory(folder).createSync(recursive: true);
          File('$folder/sheet-${phone ? 'iphone' : 'mac'}.png')
              .writeAsBytesSync(bytes!.buffer.asUint8List());
        }
      });
    }
  });

  group('at the largest text', () {
    // The real font's widths: the test font's squares wrap everything.
    setUpAll(loadGeist);

    testWidgets("on a small phone, Made for's and Length's choices read whole "
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
      for (final choice in ['X', '30 seconds']) {
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
  Future<HistoryOverview> overview({String project = ''}) =>
      Completer<HistoryOverview>().future;

  @override
  Future<HistoryEvents> events(
    List<int> cursor, {
    double? until,
    String project = '',
  }) => Completer<HistoryEvents>().future;

  @override
  Future<void> watch(bool on, {String project = ''}) async {}

  @override
  Stream<HistoryEvents> get pushes => const Stream.empty();

  @override
  ValueListenable<Int64> get changed => ValueNotifier(Int64.ZERO);
}
