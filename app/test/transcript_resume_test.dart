// Back after the connection dropped, the
// conversation shown carries on from what the app holds
// (Hello.transcript_agent_id and transcript_revision): the backend sends
// only the entries added or changed since (Transcript.resumed), put in
// place, and the rest is kept as it was, earlier entries paged in and the
// place in it too. Otherwise (another process, another agent shown) the
// latest page, as before. The backend's side: factoryd's tests/all/resume.rs.
import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

import 'tunnel_test.dart' show FakeTunnel, FakeTunnels, backendWith;

Entry _said(int seq, String text) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(1000 + seq),
  assistant: AssistantText(text: text),
);

/// A Claktory as the backend's resume has it: the agent shown's conversation,
/// a revision ("process-counter") on each whole page and each change, and
/// what it held at each revision, to send one coming back with it only
/// what changed since.
class _Claktory {
  _Claktory(this.server) {
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      // The device's (a fresh app's connection comes and goes meanwhile).
      if (!fresh) _socket = socket;
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (!message.hasHello()) return;
        final hello = message.hello;
        hellos.add(hello);
        socket
          ..add(
            ServerMessage(
              replyTo: message.id,
              welcome: Welcome(serverVersion: 'test'),
            ).writeToBuffer(),
          )
          ..add(ServerMessage(workspace: _workspace).writeToBuffer());
        final had = _sent[hello.transcriptRevision];
        if (hello.transcriptAgentId == shown && had != null) {
          _resume(socket, had);
        } else {
          _page(socket);
        }
      });
    });
  }

  final HttpServer server;
  WebSocket? _socket;

  /// A fresh app is connecting: its connection isn't the device's.
  var fresh = false;
  final hellos = <Hello>[];

  /// 'resumed' or 'page', for each connection.
  final firsts = <String>[];

  var shown = 'ops';
  final conversations = <String, List<Entry>>{
    'ops': [for (var seq = 1; seq <= 12; seq++) _said(seq, 'Ops $seq')],
    'ana': [_said(1, 'Ana 1')],
  };

  Workspace get _workspace => Workspace(
    agents: [
      Agent(id: 'ops', name: 'Ops'),
      Agent(id: 'ana', name: 'Ana'),
    ],
    selectedAgentId: shown,
  );

  /// What it held at each revision it gave: entries by seq.
  final _sent = <String, Map<int, Entry>>{};
  var _process = 'aaaa';
  var _counter = 0;

  String _stamp() {
    final revision = '$_process-${++_counter}';
    _sent[revision] = {
      for (final entry in conversations[shown]!)
        entry.seq.toInt(): entry.deepCopy(),
    };
    return revision;
  }

  /// The latest 5 entries.
  void _page(WebSocket socket) {
    final all = conversations[shown]!;
    final page = all.sublist(all.length > 5 ? all.length - 5 : 0);
    firsts.add('page');
    socket.add(
      ServerMessage(
        transcript: Transcript(
          agentId: shown,
          entries: page,
          hasEarlier: all.length > page.length,
        ),
        transcriptRevision: _stamp(),
      ).writeToBuffer(),
    );
  }

  void _resume(WebSocket socket, Map<int, Entry> had) {
    firsts.add('resumed');
    socket.add(
      ServerMessage(
        transcript: Transcript(
          agentId: shown,
          resumed: true,
          entries: [
            for (final entry in conversations[shown]!)
              if (had[entry.seq.toInt()] != entry) entry,
          ],
        ),
        transcriptRevision: _stamp(),
      ).writeToBuffer(),
    );
  }

  /// Adds or changes an entry of the agent shown; told to whoever is
  /// connected.
  void put(Entry entry) {
    final all = conversations[shown]!;
    final at = all.indexWhere((known) => known.seq == entry.seq);
    if (at >= 0) {
      all[at] = entry;
    } else {
      all.add(entry);
    }
    _socket?.add(
      ServerMessage(
        entry: EntryUpdate(agentId: shown, entry: entry),
        transcriptRevision: _stamp(),
      ).writeToBuffer(),
    );
  }

  /// Streams [text] onto the newest entry.
  void stream(String text) {
    final last = conversations[shown]!.last;
    last.assistant.text += text;
    _socket?.add(
      ServerMessage(
        delta: EntryDelta(agentId: shown, seq: last.seq, text: text),
        transcriptRevision: _stamp(),
      ).writeToBuffer(),
    );
  }

  Future<void> drop() async {
    final socket = _socket;
    _socket = null;
    await socket?.close();
  }

  /// The backend restarted: a new process holds nothing it sent.
  void restart() {
    _process = 'bbbb';
    _counter = 0;
    _sent.clear();
  }
}

Future<void> _until(bool Function() done, [String Function()? reason]) async {
  for (var i = 0; i < 500 && !done(); i++) {
    await Future<void>.delayed(const Duration(milliseconds: 20));
  }
  if (!done()) fail('Never came: ${reason?.call() ?? ''}');
}

List<Entry> _held(TranscriptStore store) => [
  for (var i = 0; i < store.length; i++) store[i].entry,
];

void main() {
  group('the store', () {
    test('a whole one holds its revision; changes move it on', () {
      final store = TranscriptStore();
      addTearDown(store.dispose);
      store.load(
        Transcript(agentId: 'ops', entries: [_said(1, 'One')]),
        revision: 'p-1',
      );
      expect(store.heldRevision, 'p-1');
      store.update(
        EntryUpdate(agentId: 'ops', entry: _said(2, 'Two')),
        revision: 'p-2',
      );
      store.delta(
        EntryDelta(agentId: 'ops', seq: Int64(2), text: '!'),
        revision: 'p-3',
      );
      expect(store.heldRevision, 'p-3');
      // Another agent's: not this conversation's.
      store.update(
        EntryUpdate(agentId: 'ana', entry: _said(9, 'Nine')),
        revision: 'p-9',
      );
      expect(store.heldRevision, 'p-3');
      // An older Claktory gives none: nothing to carry on from.
      store.update(EntryUpdate(agentId: 'ops', entry: _said(3, 'Three')));
      expect(store.heldRevision, isEmpty);
    });

    test('resumed: what changed goes in place, what was held stays, '
        'earlier ones and the view too', () {
      final store = TranscriptStore();
      addTearDown(store.dispose);
      store.load(
        Transcript(
          agentId: 'ops',
          entries: [_said(5, 'Five'), _said(6, 'Six')],
          hasEarlier: true,
        ),
        revision: 'p-1',
      );
      store.prepend('ops', [_said(4, 'Four')], hasEarlier: true);
      final loads = store.loads;
      final six = store[2];
      store.load(
        Transcript(
          agentId: 'ops',
          resumed: true,
          entries: [_said(5, 'Five, answered'), _said(7, 'Seven')],
        ),
        revision: 'p-4',
      );
      expect(
        [for (final e in _held(store)) e.assistant.text],
        ['Four', 'Five, answered', 'Six', 'Seven'],
      );
      expect(store.hasEarlier, isTrue);
      expect(store.loads, loads, reason: 'the view keeps its place');
      expect(identical(store[2], six), isTrue);
      expect(store.heldRevision, 'p-4');
      // Nothing new: nothing changes but the revision.
      store.load(Transcript(agentId: 'ops', resumed: true), revision: 'p-5');
      expect(store.length, 4);
      expect(store.heldRevision, 'p-5');
    });

    test('resumed for a conversation it doesn\'t hold: shown, but the next '
        'reconnect asks for a whole one', () {
      final store = TranscriptStore();
      addTearDown(store.dispose);
      store.load(
        Transcript(agentId: 'ops', entries: [_said(1, 'One')]),
        revision: 'p-1',
      );
      store.load(
        Transcript(agentId: 'ana', resumed: true, entries: [_said(3, 'x')]),
        revision: 'p-2',
      );
      expect(store.agentId, 'ana');
      expect(store.heldRevision, isEmpty);
    });
  });

  group('over a connection', () {
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

    /// What a fresh app, connecting now, holds of the conversation.
    Future<List<Entry>> fresh() async {
      final hellos = claktory.hellos.length;
      claktory.fresh = true;
      final app = await connect();
      await _until(
        () => claktory.hellos.length > hellos && app.transcript.length > 0,
        () =>
            'fresh: ${claktory.hellos.length}/$hellos '
            '${app.transcript.length} ${app.connected.value}',
      );
      expect(claktory.hellos.last.transcriptRevision, isEmpty);
      final held = _held(app.transcript);
      await app.close();
      claktory.fresh = false;
      return held;
    }

    /// Drops the connection, runs [meanwhile], and waits for it back.
    Future<void> away([void Function()? meanwhile]) async {
      final hellos = claktory.hellos.length;
      await claktory.drop();
      meanwhile?.call();
      await _until(
        () =>
            claktory.hellos.length > hellos && claktory.firsts.length > hellos,
        () => 'hellos ${claktory.hellos.length}, firsts ${claktory.firsts}',
      );
      await Future<void>.delayed(const Duration(milliseconds: 50));
    }

    setUp(() async {
      server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      claktory = _Claktory(server);
      backend = await connect();
      await _until(
        () => backend.transcript.length == 5,
        () =>
            'setUp: ${backend.transcript.length} ${claktory.hellos.length} '
            '${claktory.firsts} ${backend.connected.value}',
      );
    });

    tearDown(() async {
      await backend.close();
      await server.close(force: true);
    });

    test(
      'back after the agent carried on: only what changed comes, and it '
      'holds what a fresh app would, earlier ones and its place kept',
      () async {
        expect(claktory.hellos.single.transcriptRevision, isEmpty);
        // Live changes carry their revisions.
        claktory.put(_said(13, 'Thirteen'));
        claktory.stream(', at length');
        await _until(
          () => backend.transcript.length == 6,
          () =>
              '${[for (final e in _held(backend.transcript)) e.seq]} '
              '${claktory.hellos.length} ${backend.transcript.heldRevision}',
        );
        // Scrolled up: two earlier ones paged in.
        backend.transcript.prepend('ops', [
          for (final entry in claktory.conversations['ops']!.sublist(4, 7))
            entry,
        ], hasEarlier: true);
        final loads = backend.transcript.loads;

        // Away: an earlier entry changes (a card answered) and two come.
        await away(() {
          claktory
            ..put(_said(9, 'Nine, answered'))
            ..put(_said(14, 'Fourteen'))
            ..put(_said(15, 'Fifteen'));
        });
        final hello = claktory.hellos.last;
        expect(hello.transcriptAgentId, 'ops');
        expect(hello.transcriptRevision, isNotEmpty);
        expect(claktory.firsts.last, 'resumed');
        await _until(
          () => backend.transcript.length == 11,
          () => '${[for (final e in _held(backend.transcript)) e.seq]}',
        );
        expect(backend.transcript.loads, loads);
        expect(backend.transcript.hasEarlier, isTrue);
        final held = _held(backend.transcript);
        expect(held, claktory.conversations['ops']!.sublist(4));
        // A fresh app's page is the newest of what it holds.
        final page = await fresh();
        expect(held.sublist(held.length - page.length), page);

        // Back with nothing new: nothing, and it still holds the same.
        await away();
        expect(claktory.firsts.last, 'resumed');
        expect(_held(backend.transcript), held);
      },
    );

    test('back after the backend restarted: the latest page, and from there '
        'it carries on again', () async {
      await away(() {
        claktory
          ..restart()
          ..put(_said(13, 'Thirteen'));
      });
      expect(claktory.hellos.last.transcriptRevision, isNotEmpty);
      expect(claktory.firsts.last, 'page');
      await _until(
        () =>
            backend
                .transcript[backend.transcript.length - 1]
                .entry
                .assistant
                .text ==
            'Thirteen',
      );
      // The same conversation, so it's taken in place, as before:
      // what a fresh app holds, after the one it held before.
      final held = _held(backend.transcript);
      final page = await fresh();
      expect(held.sublist(held.length - page.length), page);

      await away(() => claktory.put(_said(14, 'Fourteen')));
      expect(claktory.firsts.last, 'resumed');
      await _until(() => backend.transcript.length == held.length + 1);
      expect(_held(backend.transcript).last.assistant.text, 'Fourteen');
    });

    test('back with another agent shown meanwhile: its page', () async {
      await away(() => claktory.shown = 'ana');
      expect(claktory.hellos.last.transcriptAgentId, 'ops');
      expect(claktory.firsts.last, 'page');
      await _until(() => backend.transcript.agentId == 'ana');
      expect(_held(backend.transcript), claktory.conversations['ana']);
    });
  });
}
