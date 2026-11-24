// #378: a connection that comes back carries on from the Workspace it holds
// (Hello.workspace_revision) and gets only what changed while it was away,
// as a patch; it must show exactly what a fresh app would, every time: after
// a run of changes made while it was away, after a backend restart in
// between, and when a patch names a Workspace it doesn't hold.
import 'dart:async';
import 'dart:io';
import 'dart:math';

import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/workspace_patch.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

import 'tunnel_test.dart' show FakeTunnel, FakeTunnels, backendWith;
import 'workspace_patch_test.dart' show patchFor;

/// A Claktory as Secco's contract has it: every Workspace it sends has a
/// revision ("process-counter"); it keeps those it sent the device, and
/// one coming back with one it holds gets a patch to it, even with nothing
/// changed; else, after a restart say, a whole Workspace.
class _Claktory {
  _Claktory(this.server, this.current, this.random) {
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      // The device's (a fresh app's connection comes and goes meanwhile).
      if (_fresh == 0) _socket = socket;
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (!message.hasHello()) return;
        hellos.add(message.hello);
        socket.add(
          ServerMessage(
            replyTo: message.id,
            welcome: Welcome(serverVersion: 'test'),
          ).writeToBuffer(),
        );
        final had = sent[message.hello.workspaceRevision];
        final patch = had != null && message.hello.workspacePatches;
        if (patch) {
          _patch(socket, message.hello.workspaceRevision, had);
        } else {
          _whole(socket);
        }
        if (identical(socket, _socket)) firsts.add(patch ? 'patch' : 'whole');
      });
    });
  }

  final HttpServer server;
  final Random random;
  Workspace current;
  WebSocket? _socket;

  /// A fresh app is connecting: its connection isn't the device's.
  var _fresh = 0;

  final hellos = <Hello>[];

  /// What it sent first on each of the device's connections: 'whole' or
  /// 'patch'.
  final firsts = <String>[];

  /// The Workspaces it sent the device, by revision, and the last one.
  final sent = <String, Workspace>{};
  var _last = '';

  late var _process = _hex();
  var _counter = 0;

  String _hex() => random.nextInt(1 << 32).toRadixString(16);

  String _next() => '$_process-${++_counter}';

  void _whole(WebSocket socket) {
    final revision = _next();
    sent[revision] = current.deepCopy();
    if (identical(socket, _socket)) _last = revision;
    socket.add(
      ServerMessage(
        workspace: current,
        workspaceRevision: revision,
      ).writeToBuffer(),
    );
  }

  void _patch(WebSocket socket, String base, Workspace from) {
    final revision = _next();
    sent[revision] = current.deepCopy();
    if (identical(socket, _socket)) _last = revision;
    socket.add(
      ServerMessage(
        workspacePatch: patchFor(from, current),
        workspaceRevision: revision,
        workspaceBaseRevision: base,
      ).writeToBuffer(),
    );
  }

  /// Something changed: a patch to whoever is connected.
  void change(Workspace next) {
    current = next;
    final socket = _socket;
    if (socket != null && socket.closeCode == null) {
      _patch(socket, _last, sent[_last]!);
    }
  }

  /// The connection drops (the phone went to the background).
  Future<void> drop() async {
    final socket = _socket;
    _socket = null;
    await socket?.close();
  }

  /// The backend restarted: a new process holds nothing it sent.
  void restart() {
    _process = _hex();
    _counter = 0;
    sent.clear();
    _last = '';
  }

  /// A patch naming a Workspace the app doesn't hold.
  void sendStrayPatch() {
    _socket!.add(
      ServerMessage(
        workspacePatch: patchFor(current, current),
        workspaceRevision: _next(),
        workspaceBaseRevision: 'ffff-1',
      ).writeToBuffer(),
    );
  }
}

/// A Workspace of [agents] agents in two projects.
Workspace _start(int agents) => Workspace(
  projects: [
    Project(id: 'p1', name: 'Factory'),
    Project(id: 'p2', name: 'Site'),
  ],
  agents: [
    Agent(id: 'rivet', name: 'Rivet', prompt: 'You set Claktory up.'),
    for (var i = 0; i < agents; i++)
      Agent(
        id: 'agent$i',
        name: 'Agent $i',
        projectId: i.isEven ? 'p1' : 'p2',
        prompt: 'Prompt $i ' * 20,
        status: AgentStatus.AGENT_STATUS_IDLE,
      ),
  ],
  selectedAgentId: 'agent0',
  settings: Settings(userName: 'Matt'),
  version: 'v0.1.20',
);

/// [workspace] after one change of the kinds agents' work makes: a status,
/// a needs-you mark, a new or removed agent, a rename, a new order, the
/// selection, settings, a project.
Workspace _changed(Workspace workspace, Random random, int step) {
  final next = workspace.deepCopy();
  final agents = next.agents;
  Agent pick() => agents[random.nextInt(agents.length)];
  switch (random.nextInt(9)) {
    case 0:
      final agent = pick();
      agent.status = agent.status == AgentStatus.AGENT_STATUS_WORKING
          ? AgentStatus.AGENT_STATUS_IDLE
          : AgentStatus.AGENT_STATUS_WORKING;
    case 1:
      pick().unseen = random.nextBool();
    case 2:
      agents.add(Agent(id: 'new$step', name: 'New $step', projectId: 'p1'));
    case 3:
      if (agents.length > 2) {
        agents.removeAt(1 + random.nextInt(agents.length - 1));
      }
    case 4:
      pick().name = 'Renamed $step';
    case 5:
      final agent = agents.removeAt(random.nextInt(agents.length));
      agents.insert(random.nextInt(agents.length + 1), agent);
    case 6:
      next.selectedAgentId = pick().id;
    case 7:
      next.settings.userName = 'Matt $step';
    case 8:
      if (random.nextBool()) {
        next.projects.add(Project(id: 'p$step', name: 'Project $step'));
      } else if (next.projects.length > 1) {
        next.projects.removeLast();
      }
  }
  // A status line changes with almost everything.
  pick().waitingSeqs
    ..clear()
    ..addAll([if (random.nextBool()) Int64(step)]);
  return next;
}

Future<void> _until(bool Function() done, [String Function()? reason]) async {
  for (var i = 0; i < 500 && !done(); i++) {
    await Future<void>.delayed(const Duration(milliseconds: 20));
  }
  expect(done(), isTrue, reason: reason?.call());
}

/// Secco's fixtures from the real backend (factoryd's tests/all/resume.rs),
/// three rounds of changes made while the app was away: the Workspace it
/// held, the ServerMessage the backend resumed with, and what a fresh app
/// got then. Its home's paths and this Mac's names are taken out.
({Workspace held, ServerMessage resume, Workspace fresh}) _fixture(int round) {
  List<int> read(String name) =>
      File('test/fixtures/resume/round$round/$name.pb').readAsBytesSync();
  return (
    held: Workspace.fromBuffer(read('held_workspace')),
    resume: ServerMessage.fromBuffer(read('resume_server_message')),
    fresh: Workspace.fromBuffer(read('fresh_workspace')),
  );
}

void main() {
  test("the real backend's resume patches give exactly a fresh app's "
      'Workspace', () {
    for (final round in [0, 1, 2]) {
      final (:held, :resume, :fresh) = _fixture(round);
      expect(resume.hasWorkspacePatch(), isTrue);
      expect(resume.workspaceBaseRevision, isNotEmpty);
      expect(resume.workspaceRevision, isNot(resume.workspaceBaseRevision));
      // Much less than the whole one (before it's applied: applying fills
      // the patch's Workspace in).
      expect(
        resume.writeToBuffer().length * 5,
        lessThan(fresh.writeToBuffer().length),
      );
      final patched = patchWorkspace(held, resume.workspacePatch)!;
      expect(patched, fresh, reason: 'round $round');
      expect(patched.writeToBuffer(), fresh.writeToBuffer());
    }
  });

  late HttpServer server;
  late _Claktory claktory;
  late Backend backend;
  final code = parsePairingCode(
    'factory://?t=${'a' * 64}&name=mac-mini&e=${'c' * 64}',
  );

  Future<Backend> connect() async {
    final app = backendWith(FakeTunnels(FakeTunnel(server.port)))
      ..deviceKey = (_) => 'k' * 64;
    app.connectTo(code);
    unawaited(app.run());
    return app;
  }

  /// What a fresh app, connecting now, shows.
  Future<Workspace> fresh() async {
    final hellos = claktory.hellos.length;
    claktory._fresh++;
    final app = await connect();
    await _until(
      () =>
          claktory.hellos.length > hellos &&
          app.workspace.workspace == claktory.current,
    );
    expect(claktory.hellos.last.workspaceRevision, isEmpty);
    final shown = app.workspace.workspace;
    await app.close();
    claktory._fresh--;
    return shown;
  }

  setUp(() async {
    server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    claktory = _Claktory(server, _start(48), Random(378));
    backend = await connect();
    await _until(() => backend.workspace.workspace == claktory.current);
  });

  tearDown(() async {
    await backend.close();
    await server.close(force: true);
  });

  test('back after changes made while away: a patch to what it held, and '
      'it shows what a fresh app shows', () async {
    final random = Random(1);
    for (var round = 0; round < 5; round++) {
      // Changes while connected, as patches.
      for (var step = 0; step < 5; step++) {
        claktory.change(_changed(claktory.current, random, round * 100 + step));
      }
      await _until(
        () => backend.workspace.workspace == claktory.current,
        () => 'connected round $round',
      );
      final held = claktory.hellos.length;
      await claktory.drop();
      // Changes while it's away.
      for (var step = 5; step < 5 + round * 3; step++) {
        claktory.change(_changed(claktory.current, random, round * 100 + step));
      }
      await _until(
        () =>
            claktory.hellos.length > held &&
            backend.connected.value &&
            backend.workspace.workspace == claktory.current,
        () =>
            'back round $round: hellos ${claktory.hellos.length}/$held, '
            'firsts ${claktory.firsts}, connected ${backend.connected.value}',
      );
      // It said what it held, and got a patch to it.
      expect(claktory.hellos.last.workspaceRevision, isNotEmpty);
      expect(claktory.firsts.last, 'patch');
      final resumed = backend.workspace.workspace;
      final shown = await fresh();
      expect(resumed, shown);
      expect(resumed.writeToBuffer(), shown.writeToBuffer());
    }
  });

  test('back after the backend restarted: a whole Workspace, the same as a '
      "fresh app's", () async {
    final random = Random(2);
    claktory.change(_changed(claktory.current, random, 1));
    await _until(() => backend.workspace.workspace == claktory.current);
    final held = claktory.hellos.length;
    await claktory.drop();
    claktory.restart();
    for (var step = 2; step < 12; step++) {
      claktory.change(_changed(claktory.current, random, step));
    }
    await _until(
      () =>
          claktory.hellos.length > held &&
          backend.workspace.workspace == claktory.current,
    );
    // It asked to carry on; the new process didn't know the revision.
    expect(claktory.hellos.last.workspaceRevision, isNotEmpty);
    expect(claktory.firsts.last, 'whole');
    final shown = await fresh();
    expect(backend.workspace.workspace, shown);

    // And from there it carries on again.
    final again = claktory.hellos.length;
    await claktory.drop();
    claktory.change(_changed(claktory.current, random, 20));
    await _until(
      () =>
          claktory.hellos.length > again &&
          backend.workspace.workspace == claktory.current,
    );
    expect(claktory.firsts.last, 'patch');
    expect(backend.workspace.workspace, await fresh());
  });

  test(
    'back with nothing changed: an all-kept patch, the same Workspace',
    () async {
      final before = backend.workspace.workspace;
      final held = claktory.hellos.length;
      await claktory.drop();
      await _until(
        () => claktory.hellos.length > held && claktory.firsts.length > held,
      );
      await _until(() => !identical(backend.workspace.workspace, before));
      expect(claktory.firsts.last, 'patch');
      expect(backend.workspace.workspace, before);
    },
  );

  test("a patch to a Workspace it doesn't hold: it starts again from a whole "
      'one, asking for nothing to carry on from', () async {
    final before = backend.workspace.workspace;
    final held = claktory.hellos.length;
    claktory.sendStrayPatch();
    await _until(() => claktory.hellos.length > held);
    expect(claktory.hellos.last.workspaceRevision, isEmpty);
    await _until(() => claktory.firsts.length > held);
    expect(claktory.firsts.last, 'whole');
    expect(backend.workspace.workspace, before);
  });
}
