// How often the window draws while nothing, or only a working agent,
// changes: live indicators share one low-rate clock, and an animation in
// the message box repaints only itself.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/controls.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/main.dart';
import 'package:factory_app/rail.dart';
import 'package:factory_app/sight.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Agent _agent(
  String id,
  String name, {
  bool working = false,
  String project = '',
  String hint = '',
}) => Agent(
  id: id,
  name: name,
  projectId: project,
  modelAvailable: true,
  placeholder: hint,
  status: working
      ? AgentStatus.AGENT_STATUS_WORKING
      : AgentStatus.AGENT_STATUS_IDLE,
);

Workspace _workspace({bool working = false, String hint = ''}) => Workspace(
  projects: [Project(id: 'p1', name: 'Factory', path: '/w')],
  agents: [
    _agent('rivet', 'Rivet', hint: hint),
    _agent('a1', 'Builder', project: 'p1', working: working),
  ],
  selectedAgentId: working ? 'a1' : 'rivet',
  settings: Settings(userName: 'Matt'),
);

/// Over [seconds] at 60 Hz: how many frames drew anything, and how often
/// each kind of render object painted.
Future<(int, Map<String, int>)> _watch(WidgetTester tester, int seconds) async {
  final paints = <String, int>{};
  debugProfilePaintsEnabled = true;
  debugOnProfilePaint = (object) {
    final kind = object.runtimeType.toString();
    paints[kind] = (paints[kind] ?? 0) + 1;
  };
  addTearDown(() {
    debugProfilePaintsEnabled = false;
    debugOnProfilePaint = null;
  });
  var frames = 0;
  for (var i = 0; i < seconds * 60; i++) {
    final before = paints.values.fold(0, (a, b) => a + b);
    // A frame asked for before the pump (by a ticker) counts too.
    final asked = tester.binding.hasScheduledFrame;
    await tester.pump(const Duration(microseconds: 16667));
    final after = paints.values.fold(0, (a, b) => a + b);
    if (after > before || asked) frames++;
  }
  debugProfilePaintsEnabled = false;
  debugOnProfilePaint = null;
  return (frames, paints);
}

/// The whole app, as it starts, showing [snapshot].
Future<void> _show(WidgetTester tester, Workspace snapshot) async {
  tester.view.physicalSize = const Size(1400, 900);
  tester.view.devicePixelRatio = 1;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(
    FactoryApp(
      drafts: DeviceDrafts(MemoryDraftStorage()),
      connections: Connections(MemoryConnectionStorage()),
      open: (target) {
        final backend = RecordingBackend()..connectTo(target);
        backend.workspace.apply(snapshot.deepCopy());
        return backend;
      },
      localBackend: true,
    ),
  );
  await pumpFrames(tester, const Duration(seconds: 1));
}

void main() {
  testWidgets('a working agent draws at the live clock, not the display', (
    tester,
  ) async {
    await _show(tester, _workspace(working: true));
    final (frames, _) = await _watch(tester, 4);
    // 25 a second from the waves and dots, with the working face moving
    // in the same frames; 60 if they ran on vsync, and about 29 when the
    // faces had a timer of their own.
    expect(frames / 4, lessThanOrEqualTo(LiveClock.waveFps + 0.5));
  });

  testWidgets('a window no one can see draws nothing, an agent working or '
      'not; seen again, it moves as before', (tester) async {
    addTearDown(Sight.debugReset);
    await _show(tester, _workspace(working: true));
    // Covered by other windows, minimized or hidden (the Mac says so).
    Sight.shown.value = false;
    await tester.pump();
    final (hidden, _) = await _watch(tester, 2);
    expect(hidden, 0);
    expect(LiveClock.instance.running.value, isFalse);

    Sight.shown.value = true;
    await tester.pump();
    final (shown, _) = await _watch(tester, 2);
    expect(shown / 2, greaterThan(LiveClock.waveFps - 2));
  });

  testWidgets("the Mac's Reduce motion stills the waves and faces", (
    tester,
  ) async {
    addTearDown(Sight.debugReset);
    await _show(tester, _workspace(working: true));
    Sight.reduceMotion.value = true;
    await pumpFrames(tester);
    final (frames, _) = await _watch(tester, 2);
    expect(frames, 0);
    expect(LiveClock.instance.running.value, isFalse);
  });

  testWidgets('the Mac tells whether the window shows and its Reduce '
      'motion, at launch and on every change', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    addTearDown(Sight.debugReset);
    const channel = MethodChannel('factory/sight');
    final messenger = tester.binding.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(channel, (call) async {
      expect(call.method, 'state');
      return {'shown': true, 'reduceMotion': true};
    });
    addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
    Sight.start();
    await tester.pump();
    expect(Sight.shown.value, isTrue);
    expect(Sight.reduceMotion.value, isTrue);

    await messenger.handlePlatformMessage(
      'factory/sight',
      const StandardMethodCodec().encodeMethodCall(
        const MethodCall('sight', {'shown': false, 'reduceMotion': false}),
      ),
      (_) {},
    );
    expect(Sight.shown.value, isFalse);
    expect(Sight.reduceMotion.value, isFalse);
    debugDefaultTargetPlatformOverride = null;
  });

  testWidgets('clocks that count seconds tick together, in one frame', (
    tester,
  ) async {
    final built = <String, List<int>>{'a': [], 'b': []};
    var frame = 0;
    Widget clock(String name) => Ticking(
      active: true,
      builder: (context, now) {
        built[name]!.add(frame);
        return const SizedBox.shrink();
      },
    );
    tester.binding.addPersistentFrameCallback((_) => frame++);
    await tester.pumpWidget(Column(children: [clock('a')]));
    // The second one starts later, out of step with the first's mount.
    await tester.pump(const Duration(milliseconds: 300));
    await tester.pumpWidget(Column(children: [clock('a'), clock('b')]));
    for (final list in built.values) {
      list.clear();
    }
    await pumpFrames(tester, const Duration(seconds: 3));
    expect(built['a'], hasLength(3));
    expect(built['b'], built['a']);
  });

  testWidgets('a counting clock no one can see counts nothing', (tester) async {
    var built = 0;
    Widget window({required bool seen}) => TickerMode(
      enabled: seen,
      child: Ticking(
        active: true,
        builder: (context, now) {
          built++;
          return const SizedBox.shrink();
        },
      ),
    );
    await tester.pumpWidget(window(seen: false));
    built = 0;
    await pumpFrames(tester, const Duration(seconds: 3));
    expect(built, 0);
    await tester.pumpWidget(window(seen: true));
    built = 0;
    await pumpFrames(tester, const Duration(seconds: 3));
    expect(built, 3);
  });

  testWidgets('while the live clock runs, counting clocks tick on its '
      'beat: no frames of their own', (tester) async {
    var drew = false;
    Widget window({required bool counting}) => Directionality(
      textDirection: TextDirection.ltr,
      child: Column(
        children: [
          const SizedBox(
            width: 200,
            height: 20,
            child: WorkingWave(color: Color(0xFF2F6FEB)),
          ),
          if (counting)
            Ticking(
              active: true,
              builder: (context, now) {
                drew = true;
                return const SizedBox.shrink();
              },
            ),
        ],
      ),
    );
    await tester.pumpWidget(window(counting: false));
    // Out of step with the live clock's ticks.
    await tester.pump(const Duration(milliseconds: 15));
    await tester.pumpWidget(window(counting: true));
    // Frames that built or painted anything over 4 s, in 5 ms steps so
    // no two ticks share one.
    debugProfilePaintsEnabled = true;
    debugOnProfilePaint = (_) => drew = true;
    addTearDown(() {
      debugProfilePaintsEnabled = false;
      debugOnProfilePaint = null;
    });
    var frames = 0;
    for (var i = 0; i < 800; i++) {
      drew = false;
      await tester.pump(const Duration(milliseconds: 5));
      if (drew) frames++;
    }
    debugProfilePaintsEnabled = false;
    debugOnProfilePaint = null;
    expect(frames, lessThanOrEqualTo(4 * LiveClock.waveFps + 1));
  });

  testWidgets('a focused message box blinks its caret twice a second and '
      'repaints only the caret', (tester) async {
    await _show(tester, _workspace());
    await tester.tap(find.byType(EditableText).last);
    await pumpFrames(tester, const Duration(seconds: 1));
    final (frames, paints) = await _watch(tester, 4);
    // On and off each half second, never a fade at the display's rate.
    expect(frames / 4, lessThanOrEqualTo(2.5));
    expect(paints.keys, everyElement('RenderCustomPaint'));
  });

  testWidgets("a rotating hint's slide repaints only the hint", (tester) async {
    await _show(
      tester,
      _workspace(hint: 'Ask me anything\nSet up a project\nAdd an agent'),
    );
    final (_, paints) = await _watch(tester, 8);
    // Two slides of 27 frames ran. The window around the hint redraws at
    // most as each starts and ends (now not at all), never every frame.
    expect(paints['_RenderTheater'] ?? 0, lessThanOrEqualTo(2 * 2 * 2));
  });
}
