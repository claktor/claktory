// How often the window draws while nothing, or only a working agent,
// changes: live indicators share one low-rate clock, and an animation in
// the message box repaints only itself.
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/main.dart';
import 'package:factory_app/rail.dart';
import 'package:flutter/rendering.dart';
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
    // About 25 a second from the waves and dots; 60 if they ran on vsync.
    expect(frames / 4, lessThan(32));
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
