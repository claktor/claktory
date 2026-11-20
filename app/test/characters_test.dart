import 'dart:math' as math;

import 'package:factory_app/characters/character.dart';
import 'package:factory_app/characters/clock.dart';
import 'package:factory_app/characters/identity.dart';
import 'package:factory_app/characters/pose.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/transcript.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'events_test.dart' show writer;
import 'harness.dart';

Agent agent(String id, {bool working = false, bool unseen = false}) => Agent(
  id: id,
  name: id,
  color: 'violet',
  modelAvailable: true,
  unseen: unseen,
  status: working
      ? AgentStatus.AGENT_STATUS_WORKING
      : AgentStatus.AGENT_STATUS_IDLE,
);

/// The window's clock around [child], as the app has it.
Widget hosted(Widget child, {bool reduced = false}) => MediaQuery(
  data: MediaQueryData(disableAnimations: reduced),
  child: Directionality(
    textDirection: TextDirection.ltr,
    child: CharacterHost(child: child),
  ),
);

CharacterClock clockOf(WidgetTester tester) =>
    CharacterHost.of(tester.element(find.byType(AgentCharacter).first))!;

void main() {
  group('shapes', () {
    test('every shape fits its box and is round enough to trace', () {
      for (final shape in CharacterShape.values) {
        final profile = shapeProfile(shape);
        expect(profile, hasLength(profileSamples));
        for (final r in profile) {
          expect(r, greaterThan(0.2), reason: '$shape');
          // Inside the padded box along the ray's own direction.
          expect(r, lessThanOrEqualTo(0.5 * math.sqrt2), reason: '$shape');
        }
        final bounds = bodyPath(shape, 100).getBounds();
        expect(
          bounds.left,
          greaterThanOrEqualTo(100 * characterInset - 1.5),
          reason: '$shape',
        );
        expect(
          bounds.top,
          greaterThanOrEqualTo(100 * characterInset - 1.5),
          reason: '$shape',
        );
        expect(
          bounds.right,
          lessThanOrEqualTo(100 * (1 - characterInset) + 1.5),
          reason: '$shape',
        );
        expect(
          bounds.bottom,
          lessThanOrEqualTo(100 * (1 - characterInset) + 1.5),
          reason: '$shape',
        );
      }
    });

    test('the six shapes are different', () {
      final outlines = {
        for (final shape in CharacterShape.values)
          shapeProfile(shape).map((r) => r.toStringAsFixed(2)).join(','),
      };
      expect(outlines, hasLength(CharacterShape.values.length));
    });

    test('eyes stay inside every body, at every gaze and size', () {
      for (final shape in CharacterShape.values) {
        {
          for (final small in [false, true]) {
            var layout = eyeLayout(shape);
            if (small) layout = layout.scaled(smallEyeScale);
            // The biggest an eye gets: a thinking lift and news widening.
            const grow = 1.1 * 1.12;
            for (final (at, height) in [
              (layout.left, layout.height),
              (layout.right, layout.height),
            ]) {
              for (final gx in [-gazeLimit, 0.0, gazeLimit]) {
                for (final gy in [-gazeLimit, 0.0, gazeLimit]) {
                  final cx = at.dx + gx - 0.5, cy = at.dy + gy - 0.5;
                  final w = layout.width * grow / 2;
                  final h = height * grow / 2;
                  // The capsule's extremes: its sides' ends and its tips.
                  for (final (x, y) in [
                    (cx - w, cy - h + w),
                    (cx + w, cy - h + w),
                    (cx - w, cy + h - w),
                    (cx + w, cy + h - w),
                    (cx, cy - h),
                    (cx, cy + h),
                  ]) {
                    expect(
                      insideShape(shape, x, y),
                      isTrue,
                      reason: '$shape small=$small gaze=($gx, $gy)',
                    );
                  }
                }
              }
            }
          }
        }
      }
    });

    test('identity seeds come from the id alone (FNV-1a)', () {
      expect(fnv1a(''), 0x811c9dc5);
      expect(fnv1a('a'), 0xe40c292c);
      expect(fnv1a('foobar'), 0xbf9cf968);
      expect(specOf(agent('x')).seed, fnv1a('x'));
    });
  });

  group('poses', () {
    test('sampling is pure', () {
      for (final mood in Mood.values) {
        expect(
          samplePose(mood, 12.34, 99).toString(),
          samplePose(mood, 12.34, 99).toString(),
        );
      }
    });

    test('idle moves only in calm windows: sparse, smooth, a little', () {
      var last = samplePose(Mood.idle, 0, 7);
      var blinks = <double>[];
      var wasShut = false;
      for (var i = 1; i < 60 * 60; i++) {
        final t = i / 60;
        final pose = samplePose(Mood.idle, t, 7);
        expect(pose.gazeX.abs(), lessThanOrEqualTo(0.0401));
        expect(pose.gazeY.abs(), lessThanOrEqualTo(0.0401));
        expect(pose.breath, closeTo(1, 0.0051));
        // Never a one-frame cut.
        expect((pose.open - last.open).abs(), lessThan(0.4));
        // Outside the windows (after the frame that ends one) it holds
        // perfectly still.
        if (!inCalmWindow(t) && !inCalmWindow(t - 1 / 60)) {
          expect(poseDelta(pose, last, 64), 0, reason: 'at $t');
        }
        last = pose;
        final shut = pose.open < 0.5;
        if (shut && !wasShut) blinks.add(t);
        wasShut = shut;
      }
      for (var i = 1; i < blinks.length; i++) {
        expect(blinks[i] - blinks[i - 1], greaterThan(calmBeat - calmWindow));
      }
      // A blink every several seconds, not a flicker and not never.
      expect(blinks.length, inInclusiveRange(3, 14));
    });

    test('thinking keeps moving between the calm windows', () {
      final a = samplePose(Mood.thinking, 1.0, 7);
      final b = samplePose(Mood.thinking, 1.9, 7);
      expect(inCalmWindow(1.0) || inCalmWindow(1.9), isFalse);
      expect(poseDelta(a, b, 26), greaterThan(0.1));
    });

    test('different agents blink at different moments', () {
      double firstBlink(int seed) {
        for (var i = 0; i < 600; i++) {
          if (samplePose(Mood.idle, i / 60, seed).open < 0.5) {
            return i / 60;
          }
        }
        return -1;
      }

      final moments = {for (var seed = 0; seed < 24; seed++) firstBlink(seed)};
      expect(moments.length, greaterThan(12));
    });

    test('asleep never blinks; no model barely moves', () {
      for (var i = 0; i < 600; i++) {
        expect(samplePose(Mood.asleep, i / 60, 3).open, 1);
        final noModel = samplePose(Mood.noModel, i / 60, 3);
        expect(noModel.breath, 1);
        expect(noModel.gazeX, 0);
      }
    });

    test('a mood change blends from what showed, without a jump', () {
      final track = MoodTrack(seed: 5, mood: Mood.idle);
      final before = track.pose(10);
      track.change(Mood.thinking, 10);
      final at = track.pose(10);
      expect(at.gazeX, closeTo(before.gazeX, 1e-9));
      expect(at.gazeY, closeTo(before.gazeY, 1e-9));
      // Interrupted halfway by working, then thinking again: continuous.
      track.pose(10.1);
      final mid = track.pose(10.11);
      track.change(Mood.working, 10.11);
      final retarget = track.pose(10.11);
      // No jump over 0.5px at 26px.
      expect(((retarget.gazeX - mid.gazeX) * 26).abs(), lessThan(0.5));
      expect(((retarget.gazeY - mid.gazeY) * 26).abs(), lessThan(0.5));
      var last = retarget;
      for (var i = 1; i <= 30; i++) {
        final pose = track.pose(10.11 + i / 60);
        expect(((pose.gazeY - last.gazeY) * 26).abs(), lessThan(0.5));
        last = pose;
      }
      expect(track.settling(10.5), isFalse);
    });

    test('news arrives with a blink and a widening, once', () {
      final track = MoodTrack(seed: 1, mood: Mood.idle);
      track.change(Mood.news, 20);
      expect(track.pose(20.09).open, lessThan(0.2));
      expect(track.pose(20.3).scaleLeft, greaterThan(1.05));
      expect(track.pose(21).scaleLeft, 1);
    });

    test('a change the app missed shows without its flourish', () {
      final track = MoodTrack(seed: 1, mood: Mood.idle);
      track.change(Mood.news, 20, fresh: false);
      for (var i = 0; i < 30; i++) {
        expect(track.pose(20 + i / 60).scaleLeft, 1);
      }
      expect(track.settling(20.3), isFalse);
    });

    test('a new shape blinks while it eases in', () {
      final track = MoodTrack(seed: 1, mood: Mood.idle);
      track.reshape(30);
      expect(track.pose(30.09).open, lessThan(0.2));
      expect(track.settling(30.2), isTrue);
      expect(track.settling(30.3), isFalse);
    });

    test('pose distance is in pixels at the drawn size', () {
      const a = Pose();
      const b = Pose(gazeX: 0.01);
      expect(poseDelta(a, b, 18), closeTo(0.18, 1e-9));
      expect(poseDelta(a, a, 64), 0);
      expect(poseDelta(a, const Pose(open: 0.5), 20), closeTo(3, 1e-9));
    });

    test('the first mood just shows; compact faces only rest and blink', () {
      final track = MoodTrack(seed: 1, mood: Mood.failed);
      expect(track.settling(0), isFalse);
      final compact = MoodTrack(seed: 1, mood: Mood.failed, compact: true);
      compact.change(Mood.thinking, 3);
      for (var i = 0; i < 600; i++) {
        final pose = compact.pose(i / 60);
        expect(pose.gazeX, 0);
        expect(pose.droop, 0);
      }
    });
  });

  group('the clock', () {
    testWidgets('live faces tick; reduced motion stops everything', (
      tester,
    ) async {
      await tester.pumpWidget(
        hosted(
          Row(
            children: [
              for (var i = 0; i < 3; i++)
                AgentCharacter(agent: agent('a$i'), size: 18, live: true),
            ],
          ),
        ),
      );
      final clock = clockOf(tester);
      expect(clock.ticking, isTrue);
      await tester.pump(const Duration(milliseconds: 100));
      expect(clock.activeSubscribers, 3);
      expect(find.bySemanticsLabel('a0 — idle'), findsOneWidget);

      await tester.pumpWidget(
        hosted(
          Row(
            children: [
              for (var i = 0; i < 3; i++)
                AgentCharacter(agent: agent('a$i'), size: 18, live: true),
            ],
          ),
          reduced: true,
        ),
      );
      expect(clock.ticking, isFalse);
    });

    testWidgets('faces that aren\'t painted stop being woken', (tester) async {
      await tester.pumpWidget(
        hosted(
          Column(
            children: [
              AgentCharacter(agent: agent('shown'), size: 18, live: true),
              Offstage(
                child: AgentCharacter(
                  agent: agent('hidden'),
                  size: 18,
                  live: true,
                ),
              ),
            ],
          ),
        ),
      );
      for (var i = 0; i < 5; i++) {
        await tester.pump(const Duration(milliseconds: 40));
      }
      final clock = clockOf(tester);
      expect(clock.activeSubscribers, 1);
      expect(clock.ticking, isTrue);
    });

    testWidgets('with nothing on screen moving, the clock sleeps', (
      tester,
    ) async {
      await tester.pumpWidget(
        hosted(
          Offstage(
            child: AgentCharacter(agent: agent('x'), size: 18, live: true),
          ),
        ),
      );
      for (var i = 0; i < 5; i++) {
        await tester.pump(const Duration(milliseconds: 40));
      }
      final clock = CharacterHost.of(
        tester.element(find.byType(AgentCharacter, skipOffstage: false)),
      )!;
      expect(clock.activeSubscribers, 0);
      expect(clock.ticking, isFalse);
    });

    testWidgets('idle faces tick only in calm windows, at 20fps', (
      tester,
    ) async {
      await tester.pumpWidget(
        hosted(
          Row(
            children: [
              for (var i = 0; i < 8; i++)
                AgentCharacter(agent: agent('idle$i'), size: 18, live: true),
            ],
          ),
        ),
      );
      final clock = clockOf(tester);
      var ticking = 0;
      for (var i = 0; i < 60 * 10; i++) {
        await tester.pump(const Duration(milliseconds: 1000 ~/ 60));
        if (clock.ticking) ticking++;
        if (clock.ticking) {
          // Ticking only at, or just after, a window.
          expect(
            inCalmWindow(clock.now) || inCalmWindow(clock.now - 0.06),
            isTrue,
          );
        }
      }
      // Ten seconds: three windows of 0.4s at 20fps.
      expect(clock.frame, inInclusiveRange(20, 35));
      expect(ticking, lessThan(600 ~/ 3));
      expect(clock.repaints, lessThanOrEqualTo(clock.frame * 8));
      expect(clock.repaints, greaterThan(3));
    });

    testWidgets('a working face keeps ticking; a big one at 30fps', (
      tester,
    ) async {
      await tester.pumpWidget(
        hosted(
          AgentCharacter(
            agent: agent('busy', working: true),
            size: 26,
            live: true,
          ),
        ),
      );
      final clock = clockOf(tester);
      for (var i = 0; i < 60 * 2; i++) {
        await tester.pump(const Duration(milliseconds: 1000 ~/ 60));
        expect(clock.ticking, isTrue);
      }
      expect(clock.frame, inInclusiveRange(55, 61));
    });

    testWidgets('compact faces only wake the clock to blink', (tester) async {
      await tester.pumpWidget(
        hosted(AgentCharacter(agent: agent('ref'), size: 14)),
      );
      final clock = clockOf(tester);
      var ticking = 0, frames = 0;
      for (var i = 0; i < 60 * 8; i++) {
        await tester.pump(const Duration(milliseconds: 1000 ~/ 60));
        frames++;
        if (clock.ticking) ticking++;
      }
      // Blinks take a fraction of the time, so mostly it sleeps.
      expect(ticking, lessThan(frames ~/ 4));
      expect(ticking, greaterThan(0));
    });

    testWidgets('a working agent thinks; news and no model show', (
      tester,
    ) async {
      await tester.pumpWidget(
        hosted(
          Row(
            children: [
              AgentCharacter(
                agent: agent('w', working: true),
                size: 18,
                live: true,
              ),
              AgentCharacter(
                agent: agent('n', unseen: true),
                size: 18,
                live: true,
              ),
              AgentCharacter(
                agent: agent('m')..modelAvailable = false,
                size: 18,
                live: true,
              ),
              AgentCharacter(agent: agent('r', working: true), size: 14),
            ],
          ),
        ),
      );
      expect(find.bySemanticsLabel('w — thinking'), findsOneWidget);
      expect(find.bySemanticsLabel('n — has news'), findsOneWidget);
      expect(find.bySemanticsLabel('m — no model'), findsOneWidget);
      // A reference says only the name.
      expect(find.bySemanticsLabel('r'), findsOneWidget);
    });
  });

  testWidgets('agents are their characters in the sidebar and header', (
    tester,
  ) async {
    final harness = Harness(workspace(agents: [builder(working: false)]));
    await harness.show(tester);
    final sizes = [
      for (final character in tester.widgetList<AgentCharacter>(
        find.byType(AgentCharacter),
      ))
        (character.size, character.live),
    ];
    // The sidebar's row and the header, both live.
    expect(sizes, containsAll([(24.0, true), (34.0, true)]));
    expect(tester.getSize(find.byType(AgentCharacter).first).height, 24);
    // The header is still 44px, with the character centred in it.
    final header = tester.getRect(
      find.byWidgetPredicate((w) => w is AgentCharacter && w.size == 34),
    );
    expect(header.height, 34);
    expect(header.center.dy, closeTo(22, 1));
  });

  testWidgets("a message from an agent shows its character at rest", (
    tester,
  ) async {
    final harness = Harness(
      workspace(
        agents: [
          builder(working: false),
          writer()..status = AgentStatus.AGENT_STATUS_WORKING,
        ],
      ),
    );
    Event from(String id, String name) => Event(
      source: EventSource.EVENT_SOURCE_AGENT,
      from: name,
      fromId: id,
      text: 'Hello from $name.',
    );
    harness.backend.transcript.load(
      Transcript(
        agentId: 'a1',
        entries: [
          Entry(seq: Int64(1), event: from('a2', 'Writer')),
          Entry(seq: Int64(2), event: from('gone', 'Ghost')),
        ],
      ),
    );
    await harness.show(tester);
    final inTranscript = find.descendant(
      of: find.byType(TranscriptView),
      matching: find.byType(AgentCharacter),
    );
    // Only the known sender, 22px, not live (it's working, but the
    // message is history).
    expect(inTranscript, findsOneWidget);
    final character = tester.widget<AgentCharacter>(inTranscript);
    expect(character.size, 22);
    expect(character.live, isFalse);
    expect(character.agent.id, 'a2');
  });

  Finder labeled(String label) => find.byWidgetPredicate(
    (w) => w is Semantics && w.properties.label == label,
  );

  test("the backend's mood and identity come first", () {
    final from = agent('a1', working: true)
      ..characterMood = CharacterMood.CHARACTER_MOOD_WORKING
      ..avatarShape = 'triangle'
      ..characterSeed = 42;
    expect(moodOf(from), Mood.working);
    expect(moodLabel(moodOf(from)), 'tool running');
    final spec = specOf(from);
    expect(spec.shape, CharacterShape.triangle);
    expect(spec.seed, 42);
    for (final (mood, expected) in [
      (CharacterMood.CHARACTER_MOOD_FAILED, Mood.failed),
      (CharacterMood.CHARACTER_MOOD_ASLEEP, Mood.asleep),
      (CharacterMood.CHARACTER_MOOD_NO_MODEL, Mood.noModel),
      (CharacterMood.CHARACTER_MOOD_NEWS, Mood.news),
      (CharacterMood.CHARACTER_MOOD_IDLE, Mood.idle),
      (CharacterMood.CHARACTER_MOOD_THINKING, Mood.thinking),
    ]) {
      expect(moodOf(agent('a2')..characterMood = mood), expected);
    }
    // An older backend: what the app knows, and a circle for a shape it
    // doesn't know.
    final old = agent('a3', working: true)..avatarShape = 'cloud';
    expect(moodOf(old), Mood.thinking);
    expect(specOf(old).shape, CharacterShape.circle);
    expect(specOf(old).seed, fnv1a('a3'));
  });

  testWidgets('settings pick a shape, or Automatic', (tester) async {
    final harness = Harness(
      workspace(
        agents: [
          builder(working: false)
            ..avatarShape = 'square'
            ..avatarShapeOverride = 'square',
        ],
      ),
    );
    await harness.show(tester);
    await tester.tap(find.text('Opus 5.5 · High'));
    await settle(tester);
    bool selected(String label) => tester
        .widgetList<Semantics>(labeled(label))
        .any((w) => w.properties.selected ?? false);
    // The shape was chosen.
    expect(selected('Square'), isTrue);
    expect(selected('Automatic'), isFalse);
    // Every agent has the same eyes: there's nothing else to pick.
    expect(labeled('Bright'), findsNothing);
    expect(find.text('Face'), findsNothing);
    await tester.ensureVisible(labeled('Hexagon'));
    await tester.tap(labeled('Hexagon'));
    await pumpFrames(tester);
    // The preview shows it before it's saved.
    final preview = tester.widget<AgentCharacter>(
      find.byWidgetPredicate((w) => w is AgentCharacter && w.size == 80),
    );
    expect(preview.agent.avatarShape, 'hexagon');
    await tester.tap(find.text('Save'));
    await settle(tester);
    final update = harness.last.updateAgent;
    expect(update.avatarShape, 'hexagon');
    expect(update.hasAvatarFace(), isFalse);
    expect(update.hasName(), isFalse);

    // Back to automatic: an empty choice.
    await tester.tap(find.text('Opus 5.5 · High'));
    await settle(tester);
    await tester.ensureVisible(labeled('Automatic').first);
    await tester.tap(labeled('Automatic').first);
    await pumpFrames(tester);
    await tester.tap(find.text('Save'));
    await settle(tester);
    expect(harness.last.updateAgent.hasAvatarShape(), isTrue);
    expect(harness.last.updateAgent.avatarShape, '');
    expect(harness.last.updateAgent.hasAvatarFace(), isFalse);
  });

  testWidgets('settings preview the character in each mood', (tester) async {
    final harness = Harness(workspace(agents: [builder(working: false)]));
    await harness.show(tester);
    await tester.tap(find.text('Opus 5.5 · High'));
    await settle(tester);
    final preview = find.byWidgetPredicate(
      (w) => w is AgentCharacter && w.size == 80,
    );
    expect(preview, findsOneWidget);
    expect(labeled('Builder — idle'), findsWidgets);
    await tester.tap(find.text('Asleep'));
    await tester.pump(const Duration(milliseconds: 300));
    expect(labeled('Builder — asleep (idle)'), findsOneWidget);
    // A sample doesn't change the agent's own face in the sidebar.
    expect(
      tester.widget<AgentCharacter>(find.byType(AgentCharacter).first).mood,
      isNull,
    );
    await tester.tap(find.text('As now'));
    await tester.pump(const Duration(milliseconds: 300));
    expect(labeled('Builder — asleep (idle)'), findsNothing);
  });
}
