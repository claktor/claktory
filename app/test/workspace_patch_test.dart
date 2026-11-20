import 'dart:async';
import 'dart:io';

import 'package:factory_app/connections.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/workspace_patch.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:protobuf/protobuf.dart';

import 'tunnel_test.dart' show FakeTunnel, FakeTunnels, backendWith;

/// The patch the backend sends for [next] after [prev]: every Workspace
/// field that didn't change is kept; an agent that didn't change is kept
/// whole (only its id in the patch), one that did carries only its changed
/// fields with the rest kept, and a new one comes whole.
WorkspacePatch patchFor(Workspace prev, Workspace next) {
  final patch = WorkspacePatch(workspace: Workspace());
  final agents = Workspace().info_.byName['agents']!.tagNumber;
  patch.kept.addAll(_diff(prev, next, patch.workspace, except: agents));
  final before = {for (final agent in prev.agents) agent.id: agent};
  final id = Agent().info_.byName['id']!.tagNumber;
  for (final agent in next.agents) {
    final was = before[agent.id];
    if (was == agent) {
      patch.workspace.agents.add(Agent(id: agent.id));
      patch.keptAgents.add(agent.id);
    } else if (was != null) {
      final changed = Agent(id: agent.id);
      patch.workspace.agents.add(changed);
      patch.keptAgentFields.add(
        KeptFields(
          id: agent.id,
          kept: _diff(was, agent, changed, except: id),
        ),
      );
    } else {
      patch.workspace.agents.add(agent);
    }
  }
  // As it travels.
  return WorkspacePatch.fromBuffer(patch.writeToBuffer());
}

/// Sets in [into] the fields of [next] that differ from [prev]'s, and
/// returns the numbers of those that don't, but never [except].
List<int> _diff(
  GeneratedMessage prev,
  GeneratedMessage next,
  GeneratedMessage into, {
  required int except,
}) {
  final kept = <int>[];
  for (final field in next.info_.fieldInfo.values) {
    final n = field.tagNumber;
    if (n == except) continue;
    if (_same(prev.getField(n), next.getField(n)) &&
        prev.hasField(n) == next.hasField(n)) {
      kept.add(n);
    } else if (next.getField(n) is List) {
      (into.getField(n) as List).addAll(next.getField(n) as List);
    } else if (next.getField(n) is Map) {
      (into.getField(n) as Map).addAll(next.getField(n) as Map);
    } else if (next.hasField(n)) {
      into.setField(n, next.getField(n));
    }
  }
  return kept;
}

bool _same(Object? a, Object? b) {
  if (a is List && b is List) {
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (a[i] != b[i]) return false;
    }
    return true;
  }
  if (a is Map && b is Map) {
    return a.length == b.length &&
        a.keys.every((key) => b.containsKey(key) && b[key] == a[key]);
  }
  return a == b;
}

Workspace _before() => Workspace(
  projects: [Project(id: 'p1', name: 'Factory')],
  agents: [
    Agent(id: 'rivet', name: 'Rivet'),
    Agent(id: 'a1', name: 'Builder', projectId: 'p1'),
    Agent(id: 'a2', name: 'Tester', projectId: 'p1', unseen: true),
  ],
  selectedAgentId: 'a1',
  focusedProjectId: 'p1',
  settings: Settings(userName: 'Matt'),
  version: 'v0.1.4',
);

/// [prev] as it arrived, whole, on the wire.
Workspace _wire(Workspace workspace) =>
    Workspace.fromBuffer(workspace.writeToBuffer());

void main() {
  test('kept fields and agents come from the last Workspace; the rest '
      'as the patch has them', () {
    final prev = _wire(_before());
    final next = _before()
      ..agents[1].status = AgentStatus.AGENT_STATUS_WORKING
      ..agents.removeAt(2)
      ..agents.add(Agent(id: 'a3', name: 'Writer'))
      ..focusedProjectId = '';
    final patch = patchFor(prev, next);
    expect(patch.keptAgents, ['rivet']);
    expect(patch.kept, isNot(contains(2)));
    // Builder carries only its status; its name comes from before.
    expect(patch.keptAgentFields.single.id, 'a1');
    expect(patch.workspace.agents[1].name, isEmpty);
    expect(patch.workspace.agents[2].name, 'Writer', reason: 'new: whole');
    final patched = patchWorkspace(prev, patch)!;
    expect(patched, next);
    // Unchanged, the same objects: nothing copied.
    expect(identical(patched.agents[0], prev.agents[0]), isTrue);
    expect(identical(patched.settings, prev.settings), isTrue);
    expect(patched.projects.single.name, 'Factory');
  });

  test('a field unset in the patch and not kept is empty now', () {
    final prev = _wire(_before());
    final next = _before()
      ..clearSettings()
      ..projects.clear()
      ..selectedAgentId = '';
    final patched = patchWorkspace(prev, patchFor(prev, next))!;
    expect(patched, next);
    expect(patched.hasSettings(), isFalse);
    expect(patched.projects, isEmpty);
  });

  test('patches on patches, in a new order', () {
    var shown = _wire(_before());
    final steps = [
      _before()..agents[0].unseen = true,
      _before()
        ..agents[0].unseen = true
        ..agents.insert(0, _before().agents.removeAt(2)),
      _before()
        ..settings.userName = 'Lee'
        ..agents.removeAt(1),
      _before(),
    ];
    for (final step in steps) {
      shown = patchWorkspace(shown, patchFor(shown, step))!;
      expect(shown, step);
    }
  });

  test("a kept agent the last Workspace doesn't have: it doesn't fit", () {
    final patch = WorkspacePatch(
      workspace: Workspace(agents: [Agent(id: 'ghost')]),
      keptAgents: ['ghost'],
    );
    expect(patchWorkspace(_wire(_before()), patch), isNull);
    final partly = WorkspacePatch(
      workspace: Workspace(agents: [Agent(id: 'ghost')]),
      keptAgentFields: [
        KeptFields(id: 'ghost', kept: [2]),
      ],
    );
    expect(patchWorkspace(_wire(_before()), partly), isNull);
  });

  test("an agent's field unset in the patch and not kept is empty now", () {
    final prev = _wire(_before());
    final next = _before()..agents[2].unseen = false;
    final patch = patchFor(prev, next);
    expect(
      patch.keptAgentFields.single.kept,
      isNot(contains(Agent().info_.byName['unseen']!.tagNumber)),
    );
    expect(patchWorkspace(prev, patch), next);
  });

  test('over a connection: Hello asks for patches, a whole Workspace then '
      'patches, and a patch with nothing to patch reconnects', () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    final hellos = <Hello>[];
    final sockets = <WebSocket>[];
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      sockets.add(socket);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (message.hasHello()) {
          hellos.add(message.hello);
          socket.add(
            ServerMessage(
              replyTo: message.id,
              welcome: Welcome(serverVersion: 'test'),
            ).writeToBuffer(),
          );
        }
      });
    });
    final tunnels = FakeTunnels(FakeTunnel(server.port));
    final backend = backendWith(tunnels)..deviceKey = (_) => 'k' * 64;
    backend.connectTo(
      parsePairingCode('factory://?t=${'a' * 64}&name=mac-mini&e=${'c' * 64}'),
    );
    unawaited(backend.run());
    Future<void> until(bool Function() done) async {
      for (var i = 0; i < 250 && !done(); i++) {
        await Future<void>.delayed(const Duration(milliseconds: 20));
      }
      expect(done(), isTrue);
    }

    await until(() => backend.connected.value);
    expect(hellos.single.workspacePatches, isTrue);
    void send(ServerMessage message) =>
        sockets.last.add(message.writeToBuffer());

    final first = _before();
    send(ServerMessage(workspace: first));
    await until(() => backend.workspace.workspace == first);
    final second = _before()..agents[2].unseen = false;
    send(ServerMessage(workspacePatch: patchFor(first, second)));
    await until(() => backend.workspace.workspace == second);

    // Reconnected, a patch before a whole Workspace can't be applied: it
    // starts again.
    await sockets.last.close();
    await until(() => hellos.length == 2 && backend.connected.value);
    send(ServerMessage(workspacePatch: patchFor(second, first)));
    await until(() => hellos.length == 3);
    expect(backend.workspace.workspace, second, reason: 'left as it was');

    await backend.close();
    await server.close(force: true);
  });
}
