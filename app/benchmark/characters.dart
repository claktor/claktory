// The agent characters' performance gate (fuzzy-agents.md, "Performance
// gate"): the whole app, in profile mode, with a made-up workspace of 24, 48
// and 96 agents, characters live and then frozen, waves on. It never talks
// to a backend. Run it with app/benchmark/run.sh, which writes the results
// to /tmp/factory-characters-bench.json.
import 'dart:async';
import 'dart:convert';
import 'dart:ffi' as ffi;
import 'dart:io';
import 'dart:math' as math;

import 'package:factory_app/backend.dart';
import 'package:factory_app/characters/character.dart';
import 'package:factory_app/characters/clock.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/history.dart';
import 'package:factory_app/identity.dart';
import 'package:factory_app/login.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart';
import 'package:factory_app/state.dart';
import 'package:factory_app/update.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';

const _warmUp = Duration(seconds: 3);
const _measure = Duration(seconds: 12);
const _turns = 4;

/// Only the scenes whose names contain one of these, comma-separated.
const _only = String.fromEnvironment('BENCH_ONLY');
const _turn = Duration(seconds: 4);
const _soakMinutes = int.fromEnvironment('BENCH_SOAK_MINUTES', defaultValue: 5);
final _out =
    Platform.environment['BENCH_OUT'] ?? '/tmp/factory-characters-bench.json';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  final bench = _Bench();
  runApp(bench.app());
  await WidgetsBinding.instance.endOfFrame;
  try {
    await bench.run();
  } catch (error, stack) {
    bench.results['error'] = '$error\n$stack';
  }
  File(_out).writeAsStringSync(
    const JsonEncoder.withIndent('  ').convert(bench.results),
  );
  exit(0);
}

/// A backend that's never connected anywhere, but says it is.
class _OfflineBackend extends Backend {
  _OfflineBackend()
    : super(
        workspace: WorkspaceStore(),
        transcript: TranscriptStore(),
        explorer: ExplorerStore(),
        extensionView: ExtensionViewStore(),
      ) {
    connected.value = true;
  }
}

enum _Moods { mixed, thinking, idle }

const _names = [
  'Builder',
  'Writer',
  'Tester',
  'Designer',
  'Coordinator',
  'Researcher',
  'Reviewer',
  'Scout',
  'Planner',
  'Editor',
  'Analyst',
  'Keeper',
];
const _colors = [
  'blue',
  'violet',
  'green',
  'pink',
  'teal',
  'amber',
  'red',
  'slate',
];

Workspace _workspace(
  int count,
  _Moods moods, {
  bool grouped = false,
  Set<int> collapsed = const {},
}) {
  final messaging = AgentExtension(id: 'agents', enabled: true);
  return Workspace(
    projects: [Project(id: 'p1', name: 'Factory', path: '/tmp/bench')],
    agents: [
      for (var i = 0; i < count; i++)
        Agent(
          id: 'a$i',
          name: '${_names[i % _names.length]} ${i ~/ _names.length + 1}',
          projectId: 'p1',
          color: _colors[i % _colors.length],
          model: 'anthropic/claude-opus-5-5',
          modelAvailable: moods != _Moods.mixed || i % 4 != 3,
          status: switch (moods) {
            _Moods.thinking => AgentStatus.AGENT_STATUS_WORKING,
            _Moods.idle => AgentStatus.AGENT_STATUS_IDLE,
            _Moods.mixed =>
              i % 4 == 0
                  ? AgentStatus.AGENT_STATUS_WORKING
                  : AgentStatus.AGENT_STATUS_IDLE,
          },
          unseen: moods == _Moods.mixed && i % 4 == 2,
          parentId: grouped && i % 6 != 0 ? 'a${i - i % 6}' : null,
          collapsed: grouped && collapsed.contains(i),
          extensions: [messaging],
          openPanel: i == 0 ? 'agents' : null,
        ),
    ],
    extensions: [Extension(id: 'agents', name: 'Messaging', icon: 'users')],
    selectedAgentId: 'a0',
    openPanel: 'agents',
  );
}

class _Bench {
  final backend = _OfflineBackend();
  final results = <String, Object?>{};
  final _timings = <FrameTiming>[];
  var _recording = false;

  Widget app() {
    final drafts = Drafts(MemoryDraftStorage())..watch(backend.workspace);
    final login = LoginLauncher(
      workspace: backend.workspace,
      send: backend.startLogin,
    );
    final history = AgentHistory(workspace: backend.workspace, select: (_) {});
    return WidgetsApp(
      color: signal,
      debugShowCheckedModeBanner: false,
      builder: (context, _) => AppScope(
        backend: backend,
        login: login,
        page: ValueNotifier(AppPage.agent),
        appBuild: AppBuild(version: 'bench', bundlePath: '', quit: () {}),
        drafts: drafts,
        history: history,
        connections: Connections(MemoryConnectionStorage()),
        child: const Shell(),
      ),
    );
  }

  /// Shows [count] agents: the sidebar, the first one's header and its
  /// Messaging panel (everyone, both ways), and a conversation with a
  /// message from each of 24 others (compact references).
  void _show(Workspace workspace) {
    backend.workspace.apply(workspace);
    final count = workspace.agents.length;
    backend.extensionView.apply(
      ExtensionView(
        extensionId: 'agents',
        agentId: 'a0',
        title: 'Messaging',
        messaging: Messaging(
          reaches: [for (var i = 1; i < count; i++) 'a$i'],
          reachedBy: [for (var i = 1; i < count; i++) 'a$i'],
        ),
      ),
    );
    if (backend.transcript.agentId != 'a0') {
      backend.transcript.load(
        Transcript(
          agentId: 'a0',
          entries: [
            for (var i = 1; i <= 24; i++)
              Entry(
                seq: Int64(i),
                event: Event(
                  source: EventSource.EVENT_SOURCE_AGENT,
                  from:
                      '${_names[i % _names.length]} ${i ~/ _names.length + 1}',
                  fromId: 'a$i',
                  text: 'Step $i is done.',
                ),
              ),
          ],
        ),
      );
    }
  }

  CharacterClock get _clock {
    CharacterClock? found;
    void visit(Element element) {
      if (found != null) return;
      if (element.widget is AgentCharacter) {
        found = CharacterHost.of(element);
        return;
      }
      element.visitChildren(visit);
    }

    WidgetsBinding.instance.rootElement!.visitChildren(visit);
    return found!;
  }

  /// The sidebar's list: the vertical scrollable nearest the window's left.
  ScrollPosition get _sidebarScroll {
    ScrollPosition? found;
    void visit(Element element) {
      if (found != null) return;
      if (element is StatefulElement && element.state is ScrollableState) {
        final state = element.state as ScrollableState;
        final box = element.renderObject as RenderBox?;
        if (state.position.axis == Axis.vertical &&
            box != null &&
            box.localToGlobal(Offset.zero).dx < 50) {
          found = state.position;
          return;
        }
      }
      element.visitChildren(visit);
    }

    WidgetsBinding.instance.rootElement!.visitChildren(visit);
    return found!;
  }

  void _onTimings(List<FrameTiming> timings) {
    if (_recording) _timings.addAll(timings);
  }

  Future<void> run() async {
    SchedulerBinding.instance.addTimingsCallback(_onTimings);
    final view = WidgetsBinding.instance.platformDispatcher.views.first;
    results['window'] = {
      'logical':
          '${view.physicalSize.width / view.devicePixelRatio} x '
          '${view.physicalSize.height / view.devicePixelRatio}',
      'devicePixelRatio': view.devicePixelRatio,
      'refreshRate': view.display.refreshRate,
    };
    final phases = <Map<String, Object?>>[];
    results['phases'] = phases;
    // In the background, as an app open all day mostly is: the window shows
    // but isn't the active one (no caret blinking in the message box).
    WidgetsBinding.instance.handleAppLifecycleStateChanged(
      AppLifecycleState.inactive,
    );

    Future<void> both(
      String name,
      Workspace Function() workspace, {
      Future<void> Function(Completer<void> done)? drive,
    }) async {
      if (_only.isNotEmpty && !_only.split(',').any(name.contains)) return;
      // Live and frozen take turns, so whatever else the machine is doing
      // weighs on both alike.
      final runs = {false: _Run(), true: _Run()};
      _show(workspace());
      await Future<void>.delayed(_warmUp);
      for (var turn = 0; turn < _turns; turn++) {
        for (final frozen in [false, true]) {
          await _phase(runs[frozen]!, frozen: frozen, drive: drive);
        }
      }
      for (final frozen in [false, true]) {
        phases.add({
          'name': '$name, ${frozen ? 'frozen' : 'live'}',
          ...runs[frozen]!.result(_clock),
        });
      }
    }

    await both('24 agents, mixed', () => _workspace(24, _Moods.mixed));
    await both('48 agents, mixed', () => _workspace(48, _Moods.mixed));
    await both(
      '48 agents, all thinking',
      () => _workspace(48, _Moods.thinking),
    );
    await both(
      '48 agents, all idle (no waves)',
      () => _workspace(48, _Moods.idle),
    );
    await both(
      '96 agents, mixed, sidebar scrolling',
      () => _workspace(96, _Moods.mixed),
      drive: (done) async {
        final position = _sidebarScroll;
        var down = true;
        while (!done.isCompleted) {
          await position.animateTo(
            down ? position.maxScrollExtent : 0,
            duration: const Duration(milliseconds: 1500),
            curve: Curves.easeInOut,
          );
          down = !down;
        }
      },
    );
    await both(
      '48 agents, mixed, folding groups',
      () => _workspace(48, _Moods.mixed, grouped: true),
      drive: (done) async {
        var folded = false;
        while (!done.isCompleted) {
          await Future<void>.delayed(const Duration(milliseconds: 500));
          folded = !folded;
          _show(
            _workspace(
              48,
              _Moods.mixed,
              grouped: true,
              collapsed: folded ? {6, 18, 30, 42} : {},
            ),
          );
        }
      },
    );
    if (_only.isEmpty) {
      phases.add(await _hidden());
      results['soak'] = await _soak();
    }
  }

  /// One turn of [run]: a second to settle into live or frozen, then
  /// [_turn] measured.
  Future<void> _phase(
    _Run run, {
    required bool frozen,
    Future<void> Function(Completer<void> done)? drive,
  }) async {
    _clock.still = frozen;
    final done = Completer<void>();
    final driving = drive?.call(done);
    await Future<void>.delayed(const Duration(seconds: 1));
    final cpu = _cpuSeconds();
    final ticks = _clock.frame;
    final watch = Stopwatch()..start();
    _timings.clear();
    _recording = true;
    await Future<void>.delayed(_turn);
    _recording = false;
    run.timings.addAll(_timings);
    run.cpu += _cpuSeconds() - cpu;
    run.ticks += _clock.frame - ticks;
    run.seconds += watch.elapsedMicroseconds / 1e6;
    done.complete();
    await driving;
    _clock.still = false;
  }

  /// The app hidden (as when its window is minimized): no frames, and the
  /// characters' clock stopped.
  Future<Map<String, Object?>> _hidden() async {
    _show(_workspace(48, _Moods.mixed));
    await Future<void>.delayed(_warmUp);
    final binding = WidgetsBinding.instance;
    binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    await Future<void>.delayed(const Duration(seconds: 1));
    final sample = _Sample.start(_clock);
    _timings.clear();
    _recording = true;
    await Future<void>.delayed(_measure);
    _recording = false;
    final result = {
      'name': '48 agents, mixed, app hidden',
      ...sample.end(_clock),
      'frames': _timings.length,
    };
    binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    return result;
  }

  /// [_soakMinutes] of 48 mixed agents live, sampled every 30 seconds, to
  /// see that frame times, subscriptions and memory hold steady.
  Future<List<Map<String, Object?>>> _soak() async {
    _show(_workspace(48, _Moods.mixed));
    await Future<void>.delayed(_warmUp);
    final samples = <Map<String, Object?>>[];
    const every = Duration(seconds: 30);
    for (var i = 0; i < _soakMinutes * 2; i++) {
      final sample = _Sample.start(_clock);
      _timings.clear();
      _recording = true;
      await Future<void>.delayed(every);
      _recording = false;
      samples.add({
        'at': '${(i + 1) * 30}s',
        ...sample.end(_clock),
        ..._frameStats(_timings, every),
      });
    }
    return samples;
  }
}

/// What turns of live or frozen add up to.
class _Run {
  final timings = <FrameTiming>[];
  var cpu = 0.0;
  var ticks = 0;
  var seconds = 0.0;

  Map<String, Object?> result(CharacterClock clock) => {
    'cpuPercent': _round(cpu / seconds * 100),
    'characterTicksPerSecond': _round(ticks / seconds),
    'charactersMounted': clock.subscribers,
    'charactersWoken': clock.activeSubscribers,
    'rssMB': _round(ProcessInfo.currentRss / (1 << 20)),
    ..._frameStats(timings, Duration(microseconds: (seconds * 1e6).round())),
  };
}

/// CPU time, character clock and memory over a stretch of the run.
class _Sample {
  _Sample.start(CharacterClock clock)
    : _cpu = _cpuSeconds(),
      _ticks = clock.frame,
      _watch = Stopwatch()..start();

  final double _cpu;
  final int _ticks;
  final Stopwatch _watch;

  Map<String, Object?> end(CharacterClock clock) {
    final seconds = _watch.elapsedMicroseconds / 1e6;
    return {
      'cpuPercent': _round((_cpuSeconds() - _cpu) / seconds * 100),
      'characterTicksPerSecond': _round((clock.frame - _ticks) / seconds),
      'charactersMounted': clock.subscribers,
      'charactersWoken': clock.activeSubscribers,
      'clockTicking': clock.ticking,
      'rssMB': _round(ProcessInfo.currentRss / (1 << 20)),
    };
  }
}

Map<String, Object?> _frameStats(List<FrameTiming> timings, Duration over) {
  double ms(Duration d) => d.inMicroseconds / 1000;
  List<double> sorted(Iterable<double> values) => values.toList()..sort();
  double at(List<double> values, double p) => values.isEmpty
      ? 0
      : values[math.min(values.length - 1, (values.length * p).floor())];
  final build = sorted(timings.map((t) => ms(t.buildDuration)));
  final raster = sorted(timings.map((t) => ms(t.rasterDuration)));
  return {
    'frames': timings.length,
    'fps': _round(timings.length / (over.inMicroseconds / 1e6)),
    'uiMs': {
      'p50': _round(at(build, 0.5)),
      'p95': _round(at(build, 0.95)),
      'p99': _round(at(build, 0.99)),
      'max': _round(build.isEmpty ? 0 : build.last),
    },
    'rasterMs': {
      'p50': _round(at(raster, 0.5)),
      'p95': _round(at(raster, 0.95)),
      'p99': _round(at(raster, 0.99)),
      'max': _round(raster.isEmpty ? 0 : raster.last),
    },
    'over16ms': timings
        .where((t) => ms(t.buildDuration) > 16.7 || ms(t.rasterDuration) > 16.7)
        .length,
  };
}

double _round(double value) => (value * 100).round() / 100;

// getrusage(2), for the process's CPU time on every thread.
final class _Timeval extends ffi.Struct {
  @ffi.Int64()
  external int seconds;
  @ffi.Int32()
  external int microseconds;
}

final class _Rusage extends ffi.Struct {
  external _Timeval user;
  external _Timeval system;
  @ffi.Array(14)
  external ffi.Array<ffi.Int64> rest;
}

final _getrusage = ffi.DynamicLibrary.process()
    .lookupFunction<
      ffi.Int32 Function(ffi.Int32, ffi.Pointer<_Rusage>),
      int Function(int, ffi.Pointer<_Rusage>)
    >('getrusage');
final _usage = ffi.DynamicLibrary.process()
    .lookupFunction<
      ffi.Pointer<ffi.Void> Function(ffi.Size, ffi.Size),
      ffi.Pointer<ffi.Void> Function(int, int)
    >('calloc')(1, ffi.sizeOf<_Rusage>())
    .cast<_Rusage>();

double _cpuSeconds() {
  _getrusage(0, _usage);
  final usage = _usage.ref;
  return usage.user.seconds +
      usage.user.microseconds / 1e6 +
      usage.system.seconds +
      usage.system.microseconds / 1e6;
}
