import 'dart:async';
import 'dart:typed_data';

import 'package:factory_app/history/controller.dart';
import 'package:factory_app/history/export.dart' show agentsWorking;
import 'package:factory_app/history/pulses.dart';
import 'package:factory_app/history/routes.dart';
import 'package:factory_app/history/scene.dart';
import 'package:factory_app/history/timeline.dart';
import 'package:factory_app/history/warp.dart';
import 'package:factory_app/map_layout.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart' show ValueListenable, ValueNotifier;
import 'package:flutter_test/flutter_test.dart';

import 'replay_fixture.dart';

void main() {
  group('timeline', () {
    test('reads spans, labels and events', () {
      final timeline = HistoryTimeline.of(fixtureOverview());
      expect(timeline.indexOf['agent:kai'], kai);
      expect(timeline.reconstructed, isTrue);
      expect(timeline.labelAt(kai, day1 + 3.0 * hour).label, 'Kai');
      expect(timeline.labelAt(kai, day1 + 36.0 * hour).label, 'Kaito');
      // Before its first change: the earliest known.
      expect(timeline.labelAt(kai, day1.toDouble()).label, 'Kai');
      expect(timeline.renamedAt(kai, day1 + 36.0 * hour), day1 + 35.0 * hour);
      expect(timeline.renamedAt(kai, day1 + 34.0 * hour), isNull);

      final log = HistoryLog()..add(fixtureEvents());
      expect(log.length, 6);
      expect(log.time(0), day1 + 3.0 * hour);
      expect(log.kind(2), EventKind.person);
      expect(log.toCount(3), 2);
      expect([log.to(3, 0), log.to(3, 1)], [ana, rivet]);
      expect(log.to(4, 0), you);
      expect(log.flags(1) & eventQuiet, eventQuiet);
      expect(log.running(kai, day1 + 3.0 * hour + 60000), isTrue);
      expect(log.running(kai, day1 + 4.0 * hour), isFalse);
      expect(log.lowerBound(day1 + 6.0 * hour), 2);
      expect(log.upperBound(day1 + 6.0 * hour), 3);

      // A push that repeats what's loaded adds nothing.
      log.add(fixtureEvents(), push: true);
      expect(log.length, 6);

      final counts = log.countsFor(kai, day1 + 7.0 * hour);
      expect(counts.sent, 1);
      expect(counts.received, 3);
      expect(counts.partner, rivet);
    });

    test('pages loaded out of order land in their places', () {
      final pages = fixturePages();
      final whole = HistoryLog()..add(fixtureEvents());
      final log = HistoryLog()
        ..add(pages[2])
        ..add(pages[0], before: day1 + 41.0 * hour);
      expect(log.length, 4);
      // The middle, read on into the last page's: that part's here.
      final reached = log.add(pages[1], before: day1 + 41.0 * hour);
      expect(reached, isFalse);
      expect(log.add(pages[2], before: day1 + 41.0 * hour), isTrue);
      expect(log.length, whole.length);
      for (var i = 0; i < whole.length; i++) {
        expect(log.time(i), whole.time(i));
        expect(log.from(i), whole.from(i));
        expect(log.kind(i), whole.kind(i));
        expect(log.flags(i), whole.flags(i));
        expect(
          [for (var k = 0; k < log.toCount(i); k++) log.to(i, k)],
          [for (var k = 0; k < whole.toCount(i); k++) whole.to(i, k)],
        );
      }
      expect(log.running(kai, day1 + 3.0 * hour + 60000), isTrue);
    });

    test('runs come in order whenever they load', () {
      HistoryEvents runs(List<(int, int)> list) {
        final page = HistoryEvents(baseMs: Int64(day1));
        var last = day1;
        for (final (start, ms) in list) {
          page
            ..runNode.add(kai)
            ..runStart.add(Int64(start - last))
            ..runMs.add(Int64(ms));
          last = start;
        }
        return page;
      }

      final log = HistoryLog()
        ..add(runs([(day1 + 10 * hour, 0)]))
        ..add(runs([(day1 + 2 * hour, hour), (day1 + 5 * hour, 10 * hour)]));
      expect(log.runStarts(kai), [
        day1 + 2.0 * hour,
        day1 + 5.0 * hour,
        day1 + 10.0 * hour,
      ]);
      // One still open when the next began ended by then.
      expect(log.running(kai, day1 + 9.0 * hour), isTrue);
      expect(log.running(kai, day1 + 10.0 * hour), isTrue);
      expect(log.runEnds(kai)[1], day1 + 10.0 * hour);
      expect(log.running(kai, day1 + 4.0 * hour), isFalse);
    });
  });

  group('warp', () {
    test('squeezes quiet stretches and keeps busy ones at speed', () {
      final timeline = HistoryTimeline.of(fixtureOverview());
      final log = HistoryLog()..add(fixtureEvents());
      final warp = TimeWarp.build(
        start: timeline.start,
        end: timeline.end,
        speed: HistorySpeed.hour,
        times: log.times,
        loadedUntil: timeline.end,
      );
      // Start to 3:00, 3:10 to 6:00, 6:05 to 41:00, 41:00 to 41:30 and
      // 41:30 to the end.
      expect(warp.gaps, hasLength(5));
      expect(warp.rate, hour / 10);
      // A quiet stretch plays in 0.4 s at most.
      final a = warp.playAt(day1 + 3.0 * hour + 600000);
      final b = warp.playAt(day1 + 6.0 * hour);
      expect(b - a, closeTo(gapPlay, 1e-9));
      // Ten busy minutes at an hour per 10 s: 1/6 of 10 s.
      expect(
        warp.playAt(day1 + 3.0 * hour + 600000) -
            warp.playAt(day1 + 3.0 * hour),
        closeTo(10 / 6, 1e-9),
      );
      // Its inverse.
      for (final h in [day1 + 1.0 * hour, day1 + 6.0 * hour + 60000]) {
        expect(warp.historyAt(warp.playAt(h)), closeTo(h, 1));
      }
      expect(
        warp.gapBefore(day1 + 6.0 * hour + 1)!.length,
        closeTo(2 * hour + 50 * 60000, 1),
      );
    });

    test("re-timed, it's the warp built for that length, quiet stretches "
        'and all (a video tries many lengths)', () {
      final timeline = HistoryTimeline.of(fixtureOverview());
      final log = HistoryLog()..add(fixtureEvents());
      TimeWarp build(double seconds) => TimeWarp.build(
        start: timeline.start,
        end: timeline.end,
        speed: HistorySpeed.auto,
        times: log.times,
        loadedUntil: timeline.end,
        seconds: seconds,
        limited: false,
      );
      final found = build(autoSeconds);
      for (final seconds in [0.5, 7.0, 28.0, 58.0]) {
        final built = build(seconds);
        final retimed = found.retimed(seconds);
        expect(retimed.duration, closeTo(built.duration, 1e-9));
        expect(retimed.rate, built.rate);
        expect(retimed.gaps, hasLength(built.gaps.length));
        for (final h in [day1 + 1.0 * hour, day1 + 20.0 * hour]) {
          expect(retimed.playAt(h), closeTo(built.playAt(h), 1e-9));
        }
      }
    });

    test('auto plays the whole history in about a minute', () {
      final warp = TimeWarp.build(
        start: 0,
        end: 30.0 * 24 * hour,
        speed: HistorySpeed.auto,
        times: Float64List.fromList([
          for (var i = 0; i < 30 * 24 * 6; i++) i * 600000.0,
        ]),
        loadedUntil: 30.0 * 24 * hour,
      );
      expect(warp.duration, closeTo(autoSeconds, 1));
    });

    test('past what is loaded, the histogram finds the quiet hours', () {
      final timeline = HistoryTimeline.of(fixtureOverview());
      final warp = TimeWarp.build(
        start: timeline.start,
        end: timeline.end,
        speed: HistorySpeed.hour,
        histogram: timeline.histogram,
        histogramStart: timeline.histogramStart,
      );
      // Hours 0–2, 4–5, 7–40 and 42–47 are quiet.
      expect(warp.gaps.map((gap) => gap.length ~/ hour), [3, 2, 34, 6]);
    });

    test('between stretches loaded, the histogram fills in', () {
      final timeline = HistoryTimeline.of(fixtureOverview());
      final log = HistoryLog()..add(fixtureEvents());
      final warp = TimeWarp.build(
        start: timeline.start,
        end: timeline.end,
        speed: HistorySpeed.hour,
        times: log.times,
        // The first page, and from hour 41 on.
        loaded: [
          (double.negativeInfinity, day1 + 3 * hour + 600000),
          (day1 + 41.0 * hour, double.infinity),
        ],
        histogram: timeline.histogram,
        histogramStart: timeline.histogramStart,
      );
      expect(
        [for (final gap in warp.gaps) (gap.from - day1) / hour],
        [0, 4, 7, 41, 41.5],
      );
      expect(
        [for (final gap in warp.gaps) (gap.to - day1) / hour],
        [3, 6, 41, 41.5, 48],
      );
    });
  });

  group('controller', () {
    test('seeking ahead loads there first, then the rest in order', () async {
      final source = _Source();
      final controller = HistoryController(source);
      unawaited(controller.open());
      await pumpEventQueue();
      expect(source.said, ['[] until -']);
      // Far ahead, before the first page is in: from the cursor before it.
      final target = day1 + 41.2 * hour;
      controller.seek(target);
      expect(controller.fetching, isTrue);
      source.answer();
      await pumpEventQueue();
      expect(source.said.last, '[2] until -');
      expect(controller.time.value, target);
      source.answer();
      await pumpEventQueue();
      expect(controller.fetching, isFalse);
      expect(controller.covered(target), isTrue);
      expect(controller.time.value, target);
      // Then the rest, in order, up to where it went.
      expect(source.said.last, '[1] until ${day1 + 41 * hour}');
      source.answer();
      await pumpEventQueue();
      source.answer();
      await pumpEventQueue();
      expect(controller.loaded, isTrue);
      expect(controller.log.length, 6);
      expect(controller.log.times, (HistoryLog()..add(fixtureEvents())).times);
      expect(controller.time.value, target);
      controller.dispose();
    });

    test("one project's history is asked of the backend, from where the "
        'playhead is, and an empty page with more to come goes on', () async {
      final source = _Source();
      final controller = HistoryController(source);
      unawaited(controller.open());
      await pumpEventQueue();
      source.answer();
      await pumpEventQueue();
      final target = day1 + 41.2 * hour;
      controller.seek(target);
      expect(source.watched, [(true, '')]);

      controller.showProject('app');
      await pumpEventQueue();
      expect(source.overviews, ['', 'app']);
      expect(source.watched.last, (true, 'app'));
      expect(controller.time.value, target, reason: 'the playhead stays');
      // The playhead's stretch first, that project's only.
      expect(source.projects.last, 'app');
      expect(source.said.last, '[2] until -');
      // What was drawn stays until the first page comes.
      expect(controller.log.length, greaterThan(0));
      source.answerNone([2]);
      await pumpEventQueue();
      expect(source.said.last, '[2] until -', reason: 'nothing moved on');
      // A page with none, more to come: asked on.
      final asked = source.asked.length;
      controller.showProject('app');
      expect(source.asked.length, asked, reason: 'the same project again');
      controller.dispose();
    });

    test('an empty page with more to come is asked on', () async {
      final source = _Source();
      final controller = HistoryController(source);
      unawaited(controller.open());
      await pumpEventQueue();
      source.answerNone([1]);
      await pumpEventQueue();
      expect(source.said.last, '[1] until -');
      expect(controller.loaded, isFalse);
      source.answer();
      await pumpEventQueue();
      source.answer();
      await pumpEventQueue();
      expect(controller.loaded, isTrue);
      expect(controller.log.length, 4);
      controller.dispose();
    });

    test('playing into what is still loading waits for it', () async {
      final source = _Source();
      final controller = HistoryController(source);
      unawaited(controller.open());
      await pumpEventQueue();
      source.answer();
      await pumpEventQueue();
      // The first page in: up to 3:10. Playing on past it waits.
      controller
        ..seek(day1 + 3.0 * hour + 600000)
        ..play();
      expect(controller.fetching, isFalse);
      final at = controller.time.value;
      controller.advance(1);
      expect(controller.time.value, at);
      expect(controller.fetching, isTrue);
      source.answer();
      await pumpEventQueue();
      controller.advance(1);
      expect(controller.time.value, greaterThan(at));
      expect(controller.fetching, isFalse);
      controller.dispose();
    });
  });

  group('scene', () {
    late HistoryTimeline timeline;
    late HistoryScene scene;
    setUp(() {
      timeline = HistoryTimeline.of(fixtureOverview());
      scene = HistoryScene.build(timeline);
    });

    int chip(String id) => scene.chips.indexWhere((chip) => chip.id == id);

    test("one project's chapters name nothing else (Abbacchio's row 1584)", () {
      HistoryChapter chapter(
        double hours,
        HistoryChapterKind kind,
        String title, [
        List<String> nodes = const [],
      ]) => HistoryChapter(
        atMs: Int64((day1 + hours * hour).round()),
        kind: kind,
        title: title,
        nodeIds: nodes,
      );
      final all = [
        chapter(
          2,
          HistoryChapterKind.HISTORY_CHAPTER_KIND_PROJECT,
          'App began',
          ['project:app'],
        ),
        chapter(
          2.5,
          HistoryChapterKind.HISTORY_CHAPTER_KIND_AGENTS,
          'Rivet and Kai joined',
          ['agent:rivet', 'agent:kai'],
        ),
        chapter(
          6,
          HistoryChapterKind.HISTORY_CHAPTER_KIND_AGENTS,
          'Ana joined',
          ['agent:ana'],
        ),
        chapter(
          10,
          HistoryChapterKind.HISTORY_CHAPTER_KIND_SECRET,
          'The first secret: GitHub',
          ['secret:gh'],
        ),
        chapter(
          12,
          HistoryChapterKind.HISTORY_CHAPTER_KIND_PROJECT,
          'Shop began',
          ['project:shop'],
        ),
        chapter(
          21,
          HistoryChapterKind.HISTORY_CHAPTER_KIND_PIPE,
          'A pipe to another Claktory opened',
          ['pipe:p1'],
        ),
        chapter(
          30,
          HistoryChapterKind.HISTORY_CHAPTER_KIND_RECORDING,
          'Recording began',
        ),
        chapter(
          41,
          HistoryChapterKind.HISTORY_CHAPTER_KIND_BUSIEST,
          'The busiest hour: 12 messages',
        ),
        chapter(
          44,
          HistoryChapterKind.HISTORY_CHAPTER_KIND_RELEASE,
          'Claktory v0.1.24',
        ),
      ];
      expect(
        [for (final c in scene.chaptersIn(all, 'app')) c.title],
        [
          'App began',
          'Ana joined',
          'A pipe to another Claktory opened',
          'Recording began',
          'Claktory v0.1.24',
        ],
      );
      expect(
        [for (final c in scene.chaptersIn(all, 'shop')) c.title],
        ['Shop began', 'Recording began', 'Claktory v0.1.24'],
      );
      // The board's and the video's, through the controller: all of them
      // for all projects.
      final controller = HistoryController(_Source())
        ..scene = scene
        ..timeline = HistoryTimeline.of(
          fixtureOverview()
            ..chapters.clear()
            ..chapters.addAll(all),
        );
      expect(controller.chapters, hasLength(all.length));
      controller.project = 'shop';
      expect(
        [for (final c in controller.chapters) c.title],
        ['Shop began', 'Recording began', 'Claktory v0.1.24'],
      );
      controller.dispose();
    });

    test('a moved agent has a chip in each place', () {
      expect(chip('agent:kai'), isNot(-1));
      expect(chip('agent:kai@1'), isNot(-1));
      expect(scene.originOf('agent:kai@1'), 'agent:kai');
      expect(scene.chipAt(kai, day1 + 3.0 * hour), chip('agent:kai'));
      expect(scene.chipAt(kai, day1 + 41.0 * hour), chip('agent:kai@1'));
    });

    test('shows what was there at each moment', () {
      final early = scene.at(day1 + 1.0 * hour);
      expect(early.chips[chip('agent:rivet')], dotted);
      expect(early.chips[chip('agent:kai')], hidden);
      expect(early.chips[chip('agent:ana')], hidden);
      final project = scene.boxes.indexWhere(
        (box) => box.box.kind == MapBoxKind.project,
      );
      expect(early.boxes[project], hidden);

      final later = scene.at(day1 + 12.0 * hour);
      expect(later.chips[chip('agent:kai')], dotted);
      expect(later.chips[chip('agent:ana')], dotted);
      expect(later.chips[chip('secret:gh')], shown);
      expect(later.boxes[project], dotted);
      expect(later.pads, [hidden]);
      // The secret's line to Ana, recorded: solid between a reconstructed
      // Ana and the secret is still dotted, as Ana is.
      final secretLines = [
        for (var i = 0; i < scene.wires.length; i++)
          if (scene.wires[i].wire.layer == MapLayer.secrets) later.wires[i],
      ];
      expect(secretLines, isNotEmpty);
      expect(secretLines.every((s) => s == dotted), isTrue);
      expect(scene.at(day1 + 9.0 * hour).wires, isNot(contains(shown)));

      // After recording began everything's solid; Kai works in the web
      // folder by hour 40, renamed.
      final now = scene.at(day1 + 41.0 * hour);
      expect(now.chips[chip('agent:kai')], hidden);
      expect(now.chips[chip('agent:kai@1')], shown);
      expect(now.labels[chip('agent:kai@1')], 'Kaito');
      expect(now.pads.single, shown);
      expect(now.wires, isNot(contains(dotted)));
      expect(now.sameAs(scene.at(day1 + 42.0 * hour)), isTrue);
      expect(now.sameAs(later), isFalse);
    });

    test('the camera frames only what was there', () {
      final early = scene.bounds(scene.at(day1 + 1.0 * hour))!;
      final later = scene.bounds(scene.at(day1 + 12.0 * hour))!;
      expect(
        later.width * later.height,
        greaterThan(early.width * early.height),
      );
      expect(early.overlaps(scene.chips[chip('agent:rivet')].rect), isTrue);
    });

    test('a board hugs who is on it, and grows as they come', () {
      final project = scene.boxes.indexWhere(
        (box) => box.box.kind == MapBoxKind.project,
      );
      final whole = scene.boxes[project].box.rect;
      // Kai alone, below where Ana will be: the board starts at Kai, its
      // header kept above him.
      final kaiOnly = scene.at(day1 + 3.0 * hour);
      final tight = kaiOnly.boxRects[project];
      final kaiRect = scene.chips[chip('agent:kai')].rect;
      expect(tight.top, greaterThan(whole.top));
      expect(tight.top, lessThan(kaiRect.top - 32));
      expect(tight.contains(kaiRect.topLeft), isTrue);
      expect(tight.contains(kaiRect.bottomRight), isTrue);
      expect(whole.contains(tight.topLeft), isTrue);
      // With Ana, as laid out; a picture that changes.
      final both = scene.at(day1 + 6.0 * hour);
      expect(both.boxRects[project].top, whole.top);
      expect(both.sameAs(kaiOnly), isFalse);
      // The pipe's pad, on the board's top edge, keeps it there.
      final piped = scene.at(day1 + 41.0 * hour);
      expect(piped.boxRects[project].top, whole.top);
      expect(
        piped.boxRects[project].contains(scene.pads.single.rect.center),
        isTrue,
      );
    });
  });

  group('pulses', () {
    late HistoryTimeline timeline;
    late HistoryScene scene;
    late HistoryLog log;
    late TimeWarp warp;
    late Routes routes;
    setUp(() {
      timeline = HistoryTimeline.of(fixtureOverview());
      scene = HistoryScene.build(timeline);
      log = HistoryLog()..add(fixtureEvents());
      warp = TimeWarp.build(
        start: timeline.start,
        end: timeline.end,
        speed: HistorySpeed.hour,
        times: log.times,
        loadedUntil: timeline.end,
      );
      routes = Routes(scene);
    });

    int chip(String id) => scene.chips.indexWhere((chip) => chip.id == id);

    ({List<Pulse> pulses, List<Tick> ticks}) at(double t) => pulsesAt(
      scene: scene,
      log: log,
      routes: routes,
      playAt: warp.playAt,
      historyAt: warp.historyAt,
      t: t,
    );

    test('runs along the messaging lines, edge to edge', () {
      final route = routes.chips(chip('agent:ana'), chip('agent:kai'));
      expect(route.length, greaterThan(0));
      expect(
        scene.chips[chip('agent:ana')].rect.inflate(1).contains(route.start),
        isTrue,
      );
      expect(
        scene.chips[chip('agent:kai')].rect.inflate(1).contains(route.end),
        isTrue,
      );
      expect(route.at(0), route.start);
      expect(route.at(1), route.end);
    });

    test('each message travels for a moment, a list send to each', () {
      // A tenth of a second of play after Rivet's message to Kai.
      final t = warp.historyAt(warp.playAt(day1 + 3.0 * hour) + 0.1);
      final first = at(t).pulses.single;
      expect(first.kind, PulseKind.agent);
      expect(first.at, greaterThan(0));
      expect(first.at, lessThan(0.25));
      // Gone after a second.
      expect(
        at(warp.historyAt(warp.playAt(day1 + 3.0 * hour) + 1)).pulses,
        isEmpty,
      );
      // The person's message, then Kai's to Ana and Rivet.
      final list = at(
        warp.historyAt(warp.playAt(day1 + 6.0 * hour + 300000) + 0.05),
      ).pulses;
      expect(
        list.where((pulse) => pulse.kind == PulseKind.agent),
        hasLength(2),
      );
      // Ana's card goes to the person; the pipe's message comes from its pad.
      final card = at(warp.historyAt(warp.playAt(day1 + 41.0 * hour) + 0.05))
          .pulses;
      // (Kai's list send, a squeezed quiet stretch before, is still going.)
      expect(card.where((pulse) => pulse.kind == PulseKind.asks), hasLength(1));
      final piped = at(warp.historyAt(warp.playAt(day1 + 41.5 * hour) + 0.05))
          .pulses;
      final pipe = piped.singleWhere((pulse) => pulse.kind == PulseKind.pipe);
      // From the ring above it, where the other Claktory is: seen coming in,
      // down to the pad's top over its portal.
      final pad = scene.pads.single.rect;
      expect(
        (pipe.route.start - (pipeTop(pad) - const Offset(0, pipeReach)))
            .distance,
        lessThan(0.01),
      );
      expect((pipe.route.points[1] - pipeTop(pad)).distance, lessThan(0.01));
    });

    test('an app\'s event leaves from its chip; a node never filled is '
        'skipped', () {
      final overview = fixtureOverview();
      overview.nodes.addAll([
        HistoryNode(),
        node(
          'app:telegram',
          MapNodeKind.MAP_NODE_KIND_APP,
          'Telegram',
          present: [span(day1)],
        ),
      ]);
      overview.edges.add(
        edge('app:telegram', 'agent:kai', MapEdgeKind.MAP_EDGE_KIND_HEARS, [
          span(day1),
        ]),
      );
      final timeline = HistoryTimeline.of(overview);
      expect(timeline.indexOf.containsKey(''), isFalse);
      final telegram = timeline.indexOf['app:telegram']!;
      expect(telegram, overview.nodes.length);
      final scene = HistoryScene.build(timeline);
      expect(scene.chips.where((chip) => chip.id.isEmpty), isEmpty);
      final app = scene.chips.indexWhere((chip) => chip.id == 'app:telegram');
      expect(app, isNot(-1));
      final log = HistoryLog()
        ..add(
          HistoryEvents(
            baseMs: Int64(day1),
            dt: [Int64(3 * hour)],
            from: [telegram],
            kind: [HistoryEventKind.HISTORY_EVENT_KIND_APP],
            flags: [0],
            toCount: [1],
            to: [kai],
          ),
        );
      final warp = TimeWarp.build(
        start: timeline.start,
        end: timeline.end,
        speed: HistorySpeed.hour,
        times: log.times,
        loadedUntil: timeline.end,
      );
      final pulse = pulsesAt(
        scene: scene,
        log: log,
        routes: Routes(scene),
        playAt: warp.playAt,
        historyAt: warp.historyAt,
        t: warp.historyAt(warp.playAt(day1 + 3.0 * hour) + 0.1),
      ).pulses.single;
      expect(pulse.kind, PulseKind.app);
      expect(
        scene.chips[app].rect.inflate(2).contains(pulse.route.start),
        isTrue,
      );
    });

    test('a message with no line known between them goes straight across', () {
      final overview = fixtureOverview();
      overview.edges.removeWhere(
        (edge) =>
            edge.edge.fromId == 'agent:rivet' && edge.edge.toId == 'agent:ana',
      );
      final timeline = HistoryTimeline.of(overview);
      expect(timeline.messagingAt(rivet, kai, day1 + 7.0 * hour), isTrue);
      expect(timeline.messagingAt(kai, rivet, day1 + 7.0 * hour), isTrue);
      expect(timeline.messagingAt(rivet, kai, day1 + 1.0 * hour), isFalse);
      expect(timeline.messagingAt(rivet, ana, day1 + 7.0 * hour), isFalse);
      final scene = HistoryScene.build(timeline);
      final routes = Routes(scene);
      final log = HistoryLog()
        ..add(
          HistoryEvents(
            baseMs: Int64(day1),
            dt: [Int64(7 * hour)],
            from: [rivet],
            kind: [HistoryEventKind.HISTORY_EVENT_KIND_AGENT],
            flags: [0],
            toCount: [1],
            to: [ana],
          ),
        );
      final warp = TimeWarp.build(
        start: timeline.start,
        end: timeline.end,
        speed: HistorySpeed.hour,
        times: log.times,
        loadedUntil: timeline.end,
      );
      int chipOf(String id) => scene.chips.indexWhere((chip) => chip.id == id);
      final pulse = pulsesAt(
        scene: scene,
        log: log,
        routes: routes,
        playAt: warp.playAt,
        historyAt: warp.historyAt,
        t: warp.historyAt(warp.playAt(day1 + 7.0 * hour) + 0.1),
      ).pulses.single;
      expect(
        pulse.route,
        same(routes.across(chipOf('agent:rivet'), chipOf('agent:ana'))),
      );
    });

    test('over budget, pulses on one route merge with a count', () {
      final route = routes.chips(chip('agent:ana'), chip('agent:kai'));
      final many = [
        for (var i = 0; i < 1000; i++) Pulse(route, i / 1000, PulseKind.agent),
      ];
      final merged = merge(many, 150);
      expect(merged.length, lessThanOrEqualTo(150));
      expect(merged.fold(0, (sum, pulse) => sum + pulse.count), 1000);
      expect(merge(many.sublist(0, 10), 150), hasLength(10));
    });
  });

  group('one project (Bruno 44372)', () {
    late HistoryScene scene;
    late HistoryLog log;
    setUp(() {
      scene = HistoryScene.build(HistoryTimeline.of(fixtureOverview()));
      log = HistoryLog()..add(fixtureEvents());
    });

    int chip(String id) => scene.chips.indexWhere((chip) => chip.id == id);

    test(
      "shows only its box, folders, pipes and agents while they're in it",
      () {
        expect(scene.projects, [(id: 'app', label: 'App')]);
        final t = (day1 + 42 * hour).toDouble();
        final all = scene.at(t);
        final only = scene.at(t, project: 'app');
        expect(all.chips[chip('agent:rivet')], isNot(hidden));
        expect(only.chips[chip('agent:rivet')], hidden);
        expect(only.chips[chip('secret:gh')], hidden);
        expect(only.chips[chip('agent:ana')], isNot(hidden));
        // Kai, moved to the Web folder: still in the project, where he is.
        final kaiChip = scene.chipAt(kai, t);
        expect(scene.chips[kaiChip].place, greaterThanOrEqualTo(0));
        expect(only.chips[kaiChip], isNot(hidden));
        final project = scene.projectBox('app');
        expect(only.boxes[project], isNot(hidden));
        for (var b = 0; b < scene.boxes.length; b++) {
          if (b == project || scene.boxes[b].parent == project) continue;
          expect(only.boxes[b], hidden, reason: scene.boxes[b].box.id);
        }
        expect(only.pads.single, isNot(hidden));
        // The Map's lines to what's outside go with it.
        for (var w = 0; w < scene.wires.length; w++) {
          final wire = scene.wires[w];
          if (wire.a == chip('agent:rivet') || wire.b == chip('agent:rivet')) {
            expect(only.wires[w], hidden);
          }
        }
      },
    );

    test('counts its agents, and those working', () {
      final t = (day1 + 3 * hour + 5 * 60000).toDouble();
      expect(scene.agentsAt(log, t), (working: 1, total: 2));
      expect(scene.agentsAt(log, t, project: 'app'), (working: 1, total: 1));
      expect(agentsWorking((working: 1, total: 2)), '1 of 2 agents working');
      expect(agentsWorking((working: 0, total: 1)), '1 agent');
      expect(agentsWorking((working: 0, total: 23)), '23 agents');
      expect(agentsWorking((working: 1, total: 1)), '1 agent working');
      expect(
        agentsWorking((working: 4, total: 1023)),
        '4 of 1,023 agents working',
      );
    });

    test('a message from outside it comes in at its edge', () {
      final warp = TimeWarp.build(
        start: scene.timeline.start,
        end: scene.timeline.end,
        speed: HistorySpeed.hour,
        times: log.times,
        loadedUntil: scene.timeline.end,
      );
      final routes = Routes(scene);
      // Rivet, outside the project, to Kai, in it.
      final t = warp.historyAt(warp.playAt(day1 + 3.0 * hour) + 0.1);
      final pulse = pulsesAt(
        scene: scene,
        log: log,
        routes: routes,
        playAt: warp.playAt,
        historyAt: warp.historyAt,
        t: t,
        project: 'app',
      ).pulses.single;
      final box = scene.boxes[scene.projectBox('app')].box.rect;
      expect(box.inflate(1).contains(pulse.route.start), isTrue);
      expect(box.deflate(1).contains(pulse.route.start), isFalse);
      expect(
        scene.chips[chip('agent:kai')].rect
            .inflate(1)
            .contains(pulse.route.end),
        isTrue,
      );
    });
  });

  test('a Claktory-sized history lays out once and plays fast', () {
    final overview = bigOverview();
    final timeline = HistoryTimeline.of(overview);
    final watch = Stopwatch()..start();
    final scene = HistoryScene.build(timeline);
    final built = watch.elapsedMilliseconds;
    // 25,000 events over 8 days, between agents of a project.
    final indexes = [
      for (var i = 0; i < timeline.nodes.length; i++)
        if (timeline.nodes[i].node.kind == MapNodeKind.MAP_NODE_KIND_AGENT)
          i + 1,
    ];
    final dt = <Int64>[];
    final from = <int>[];
    final to = <int>[];
    var last = day1 + 3 * hour;
    for (var i = 0; i < 25000; i++) {
      dt.add(Int64(i == 0 ? last - day1 : 20000));
      last += 20000;
      from.add(indexes[i % indexes.length]);
      to.add(indexes[(i * 7 + 3) % indexes.length]);
    }
    final log = HistoryLog()
      ..add(
        HistoryEvents(
          baseMs: Int64(day1),
          dt: dt,
          from: from,
          to: to,
          toCount: List.filled(25000, 1),
          kind: List.filled(25000, HistoryEventKind.HISTORY_EVENT_KIND_AGENT),
        ),
      );
    final warp = TimeWarp.build(
      start: timeline.start,
      end: timeline.end,
      speed: HistorySpeed.day,
      times: log.times,
      loadedUntil: timeline.end,
    );
    final routes = Routes(scene);
    watch.reset();
    var frames = 0;
    var pulses = 0;
    for (var p = 0.0; p < warp.duration; p += 1 / 6) {
      final t = warp.historyAt(p);
      scene.at(t);
      pulses += pulsesAt(
        scene: scene,
        log: log,
        routes: routes,
        playAt: warp.playAt,
        historyAt: warp.historyAt,
        t: t,
      ).pulses.length;
      frames++;
    }
    final perFrame = watch.elapsedMicroseconds / frames / 1000;
    // ignore: avoid_print
    print(
      'layout ${built}ms, ${scene.chips.length} chips, '
      '${scene.wires.length} lines; '
      '${perFrame.toStringAsFixed(2)}ms a frame over $frames frames, '
      '$pulses pulses',
    );
    expect(pulses, greaterThan(0));
    expect(perFrame, lessThan(16));
  });
}

/// A Claktory-sized history: 60 agents in 6 projects, every pair in a
/// project able to message, secrets, 25,000 events.
HistoryOverview bigOverview() {
  const agent = MapNodeKind.MAP_NODE_KIND_AGENT;
  final nodes = <HistoryNode>[
    node(
      'workspace',
      MapNodeKind.MAP_NODE_KIND_WORKSPACE,
      'Workspace',
      present: [span(day1)],
    ),
    node('agent:rivet', agent, 'Rivet', admin: true, present: [span(day1)]),
  ];
  final edges = <HistoryEdge>[
    edge('agent:rivet', 'workspace', MapEdgeKind.MAP_EDGE_KIND_WRITES, [
      span(day1),
    ]),
  ];
  for (var p = 0; p < 6; p++) {
    final at = day1 + p * 20 * hour;
    nodes.add(
      node(
        'project:p$p',
        MapNodeKind.MAP_NODE_KIND_PROJECT,
        'P$p',
        present: [span(at)],
      ),
    );
    final ids = [for (var a = 0; a < 10; a++) 'agent:p${p}a$a'];
    for (var a = 0; a < 10; a++) {
      nodes.add(
        node(
          ids[a],
          agent,
          'Agent $p.$a',
          project: 'p$p',
          parent: 'project:p$p',
          color: 'green',
          present: [span(at + a * hour)],
        ),
      );
      edges
        ..add(
          edge(ids[a], 'project:p$p', MapEdgeKind.MAP_EDGE_KIND_WRITES, [
            span(at + a * hour),
          ]),
        )
        ..add(
          edge('agent:rivet', ids[a], MapEdgeKind.MAP_EDGE_KIND_MESSAGES, [
            span(at + a * hour),
          ]),
        );
      for (var b = a + 1; b < 10; b++) {
        edges.add(
          edge(ids[a], ids[b], MapEdgeKind.MAP_EDGE_KIND_MESSAGES, [
            span(at + b * hour),
          ], both: true),
        );
      }
    }
    if (p > 0) {
      edges.add(
        edge(
          'agent:p${p}a0',
          'agent:p${p - 1}a0',
          MapEdgeKind.MAP_EDGE_KIND_MESSAGES,
          [span(at)],
          both: true,
        ),
      );
    }
    nodes.add(
      node(
        'secret:s$p',
        MapNodeKind.MAP_NODE_KIND_SECRET,
        'S$p',
        present: [span(at)],
      ),
    );
    edges.add(
      edge('secret:s$p', ids[1], MapEdgeKind.MAP_EDGE_KIND_SECRET, [span(at)]),
    );
  }
  return HistoryOverview(
    startMs: Int64(day1),
    endMs: Int64(day1 + 8 * 24 * hour),
    recordingSinceMs: Int64(day1),
    nodes: nodes,
    edges: edges,
  );
}

/// The fixture's pages, each answered when the test says.
class _Source implements HistorySource {
  final asked = <(List<int>, double?)>[];

  /// [asked], to read: "[2] until 1795046400000".
  List<String> get said => [
    for (final (cursor, until) in asked)
      '$cursor until ${until?.toInt() ?? '-'}',
  ];
  final _pending = <Completer<HistoryEvents>>[];
  final _pages = fixturePages();

  /// Answers the oldest request waiting with no events, more to come
  /// from [next], as one project's page can.
  void answerNone(List<int> next) =>
      _pending.removeAt(0).complete(HistoryEvents(nextCursor: next));

  /// Answers the oldest request waiting.
  void answer() {
    final (cursor, _) = asked[asked.length - _pending.length];
    final page = cursor.isEmpty ? 0 : cursor.first;
    _pending.removeAt(0).complete(_pages[page]);
  }

  /// The projects the overview, the pages and the watch were asked for.
  final overviews = <String>[];
  final projects = <String>[];
  final watched = <(bool, String)>[];

  @override
  Future<HistoryOverview> overview({String project = ''}) async {
    overviews.add(project);
    return fixtureOverview()..cursors.addAll(fixtureCursors());
  }

  @override
  Future<HistoryEvents> events(
    List<int> cursor, {
    double? until,
    String project = '',
  }) {
    asked.add((cursor, until));
    projects.add(project);
    final done = Completer<HistoryEvents>();
    _pending.add(done);
    return done.future;
  }

  @override
  Future<void> watch(bool on, {String project = ''}) async =>
      watched.add((on, project));

  @override
  Stream<HistoryEvents> get pushes => const Stream.empty();

  @override
  ValueListenable<Int64> get changed => ValueNotifier(Int64.ZERO);
}
