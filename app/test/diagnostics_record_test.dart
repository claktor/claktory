import 'dart:convert';
import 'dart:io';

import 'package:factory_app/diagnostics_record.dart';
import 'package:flutter/foundation.dart' show TargetPlatform;
import 'package:flutter_test/flutter_test.dart';

void main() {
  late Directory folder;
  setUp(() => folder = Directory.systemTemp.createTempSync('record_test'));
  tearDown(() => folder.deleteSync(recursive: true));

  File at(String name) => File('${folder.path}/$name');

  /// Event [n] of [session]'s seq.
  int seq(int session, int n) => (session << 32) + n;

  test('a detail holds only its keys, with numbers or the lists\' words; '
      'any text from outside is refused', () {
    expect(
      recordDetail({
        ConnectionKey.route: ConnectionRoute.relayed,
        ConnectionKey.awayS: 812,
        ConnectionKey.why: ConnectionFailure.timeout,
        ConnectionKey.up: true,
      }),
      'route=relayed away_s=812 why=timeout up=1',
    );
    for (final outside in [
      '192.168.1.10:7443',
      'https://relay-eu.example.com',
      'factory://?t=abc',
      'a' * 64,
    ]) {
      expect(
        () => recordDetail({ConnectionKey.why: outside}),
        throwsArgumentError,
      );
    }
  });

  test('the emitter allows only these keys, and only numbers, yes or no, '
      'and lowercase words from fixed lists as values (Fugo)', () {
    // A new key changes this list: a change Fugo reviews.
    expect(ConnectionKey.values.map((key) => key.wire), [
      'app',
      'away_s',
      'net',
      'route',
      'why',
      'reason',
      'code',
      'up',
      'attempt',
      'welcomed',
      // #388: why the relay can't be reached, a word from RelayTrouble.
      'err',
      // Polpo's efficiency study §4: what a direct path crosses, a word
      // from PathReach (never the address), and a count of bytes.
      'reach',
      'bytes',
    ]);
    expect(ConnectionKind.values.map((kind) => kind.name), [
      'lifecycle',
      'network',
      'connect',
      'connected',
      'disconnected',
      'relay',
      'path',
      'reconnect',
      'ready',
    ]);
    // The notifications area's (setting up pushes on an iPhone).
    expect(NotificationsKey.values.map((key) => key.wire), [
      'step',
      'status',
      'fresh',
    ]);
    expect(NotificationsKind.values.map((kind) => kind.name), [
      'registered',
      'failed',
    ]);
    expect(DiagnosticArea.values.map((area) => area.name), [
      'connection',
      'notifications',
    ]);
    expect(
      recordDetail({
        NotificationsKey.step: PushStep.key,
        NotificationsKey.status: 34018,
        NotificationsKey.fresh: true,
      }),
      'step=key status=34018 fresh=1',
    );
    final word = RegExp(r'^[a-z]{1,16}$');
    for (final words in <List<Enum>>[
      AppLife.values,
      NetKind.values,
      ConnectionRoute.values,
      ConnectionFailure.values,
      ConnectionReason.values,
      PushStep.values,
    ]) {
      for (final value in words) {
        expect(value.name, matches(word));
        expect(recordDetail({ConnectionKey.why: value}), 'why=${value.name}');
      }
    }
    for (final outside in <Object>[
      'wifi',
      1.5,
      const ['a'],
      const {'a': 1},
      Uri.parse('https://example.com'),
      InternetAddress.loopbackIPv4,
      // An enum that isn't one of the lists.
      FileSystemEntityType.file,
      ConnectionKind.connect,
      TargetPlatform.iOS,
    ]) {
      expect(
        () => recordDetail({ConnectionKey.why: outside}),
        throwsArgumentError,
        reason: '$outside',
      );
    }
    // Every key with every kind of value it may take: key=value tokens.
    final detail = recordDetail({
      for (final key in ConnectionKey.values)
        key: key.index.isEven ? key.index : ConnectionRoute.direct,
    });
    for (final token in detail.split(' ')) {
      expect(token, matches(RegExp(r'^[a-z_]+=([0-9]+|[a-z]+)$')));
    }
  });

  test('every event is on disk as it happens: a force-quit loses nothing, '
      'and a line cut short is dropped', () async {
    final file = at('mac-mini.jsonl');
    final record = DiagnosticsRecord(file, session: 3);
    record.add(ConnectionKind.lifecycle, {
      ConnectionKey.app: AppLife.background,
    });
    record.add(ConnectionKind.connect, {ConnectionKey.attempt: 1});
    record.add(ConnectionKind.connected, {
      ConnectionKey.route: ConnectionRoute.direct,
    }, const Duration(milliseconds: 812));
    await record.settled;
    // Killed mid-write: half a line at the end.
    file.writeAsStringSync('{"at": 1, "se', mode: FileMode.append);

    final after = DiagnosticsRecord(file, session: 4);
    expect(after.waiting.map((e) => e.kind), [
      'lifecycle',
      'connect',
      'connected',
    ]);
    expect(after.waiting.last.durationMs, 812);
    expect(after.waiting.last.session, 3);
    // Its numbers carry on, past every earlier session's.
    after.add(ConnectionKind.lifecycle, {ConnectionKey.app: AppLife.started});
    expect(after.waiting.last.seq, seq(4, 1));
    expect(after.waiting.last.seq, greaterThan(after.waiting.first.seq));
    expect(after.waiting.last.session, 4);
  });

  test('what the Claktory kept leaves the device', () async {
    final file = at('r.jsonl');
    final record = DiagnosticsRecord(file, session: 1);
    for (var i = 0; i < 5; i++) {
      record.add(ConnectionKind.connect, {ConnectionKey.attempt: i});
    }
    record.sent(seq(1, 3));
    expect(record.waiting.map((e) => e.seq), [seq(1, 4), seq(1, 5)]);
    await record.settled;
    expect(DiagnosticsRecord(file, session: 2).waiting.map((e) => e.seq), [
      seq(1, 4),
      seq(1, 5),
    ]);
  });

  test('capped: the oldest go first, by count and by size', () async {
    final file = at('cap.jsonl');
    final record = DiagnosticsRecord(file, session: 1, maxEvents: 3);
    for (var i = 0; i < 5; i++) {
      record.add(ConnectionKind.connect, {ConnectionKey.attempt: i});
    }
    expect(record.waiting.map((e) => e.seq), [seq(1, 3), seq(1, 4), seq(1, 5)]);
    await record.settled;
    expect(DiagnosticsRecord(file, session: 2).waiting.length, 3);

    final small = DiagnosticsRecord(
      at('bytes.jsonl'),
      session: 1,
      maxBytes: 400,
    );
    for (var i = 0; i < 20; i++) {
      small.add(ConnectionKind.connect, {ConnectionKey.attempt: i});
    }
    await small.settled;
    expect(at('bytes.jsonl').lengthSync(), lessThanOrEqualTo(400));
    expect(small.waiting.last.seq, seq(1, 20));
  });

  test('cleared, nothing is left on the device', () async {
    final file = at('gone.jsonl');
    final record = DiagnosticsRecord(file, session: 1);
    record.add(ConnectionKind.connect);
    await record.settled;
    expect(file.existsSync(), isTrue);
    record.clear();
    await record.settled;
    expect(file.existsSync(), isFalse);
    expect(record.waiting, isEmpty);
  });

  test("each run of the app is the next session", () {
    final file = at('session');
    expect(nextSession(file), 1);
    expect(nextSession(file), 2);
    expect(nextSession(file), 3);
  });

  test(
    'every event kept is JSON with only its seven fields, its area among them',
    () async {
      final file = at('shape.jsonl');
      final record = DiagnosticsRecord(file, session: 1);
      record.add(ConnectionKind.network, {ConnectionKey.net: NetKind.wifi});
      await record.settled;
      final line = jsonDecode(file.readAsLinesSync().single) as Map;
      expect(line.keys.toSet(), {
        'at',
        'seq',
        'area',
        'kind',
        'detail',
        'ms',
        'session',
      });
      expect(line['area'], 'connection');
    },
  );
}
