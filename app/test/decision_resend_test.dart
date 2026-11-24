// A decision on a card cut off by the connection (#401): the card is told
// "Not sent yet", and the backend gets it again by itself once the
// connection is back; anything else isn't sent again.
import 'dart:async';
import 'dart:io';

import 'package:factory_app/backend.dart';
import 'package:factory_app/outbox.dart' show decisionWaiting;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:flutter_test/flutter_test.dart';

/// A Claktory that drops the connection, without answering, at the first
/// request [drop] picks, and answers everything else.
class _Claktory {
  final got = <ClientMessage>[];
  bool Function(ClientMessage message) drop = (_) => false;
  var dropped = false;
  var welcomes = 0;

  /// Down: every connection is closed as it comes.
  var down = false;
}

Future<HttpServer> _serve(_Claktory claktory) async {
  final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
  server.listen((request) async {
    final socket = await WebSocketTransformer.upgrade(request);
    socket.listen((frame) {
      final message = ClientMessage.fromBuffer(frame as List<int>);
      void reply(ServerMessage answer) =>
          socket.add((answer..replyTo = message.id).writeToBuffer());
      if (claktory.down && message.hasHello()) {
        unawaited(socket.close());
        return;
      }
      if (message.hasHello()) {
        claktory.welcomes++;
        return reply(ServerMessage(welcome: Welcome(serverVersion: 'test')));
      }
      claktory.got.add(message);
      if (!claktory.dropped && claktory.drop(message)) {
        claktory.dropped = true;
        unawaited(socket.close());
        return;
      }
      reply(ServerMessage(ack: Ack()));
    });
  });
  return server;
}

void main() {
  late _Claktory claktory;
  late HttpServer server;
  late Backend backend;

  Future<void> until(bool Function() done) async {
    for (var i = 0; i < 500 && !done(); i++) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
  }

  setUp(() async {
    claktory = _Claktory();
    server = await _serve(claktory);
    backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: ExtensionViewStore(),
      localPairing: () async => null,
      localEndpoint: 'ws://127.0.0.1:${server.port}',
    );
    unawaited(backend.run());
    await until(() => backend.connected.value);
    expect(backend.connected.value, isTrue);
  });

  tearDown(() async {
    await backend.close();
    await server.close(force: true);
  });

  test("an answer cut off by the connection says it's not sent yet, and "
      'goes again by itself once the connection is back', () async {
    claktory.drop = (message) => message.hasAnswerAsk();
    await expectLater(
      backend.answerAsk('june', 7, text: 'Ship it'),
      throwsA(
        isA<RequestFailure>().having(
          (failure) => failure.message,
          'message',
          decisionWaiting,
        ),
      ),
    );
    await until(() => claktory.got.where((m) => m.hasAnswerAsk()).length > 1);
    final answers = claktory.got.where((m) => m.hasAnswerAsk()).toList();
    expect(answers, hasLength(2));
    expect(answers.last.answerAsk.text, 'Ship it');
    expect(answers.last.answerAsk.seq.toInt(), 7);
    expect(claktory.welcomes, 2);
  });

  test('a decision made while not connected goes once it connects, '
      'however long that takes', () async {
    claktory
      ..down = true
      ..drop = (_) => true;
    await expectLater(
      backend.decideProposal('rivet', 'p3', approve: true),
      throwsA(isA<RequestFailure>()),
    );
    await until(() => !backend.connected.value);
    expect(backend.connected.value, isFalse);
    // Made while it's down: refused at once, and kept.
    await expectLater(
      backend.decideSecretApproval('june', 9, true),
      throwsA(
        isA<RequestFailure>().having(
          (failure) => failure.message,
          'message',
          decisionWaiting,
        ),
      ),
    );
    claktory.down = false;
    await until(
      () =>
          claktory.got.any((m) => m.hasDecideSecretApproval()) &&
          claktory.got.where((m) => m.hasDecideProposal()).length > 1,
    );
    expect(claktory.got.where((m) => m.hasDecideProposal()), hasLength(2));
    expect(
      claktory.got.where((m) => m.hasDecideSecretApproval()),
      hasLength(1),
    );
  });

  test("anything else cut off isn't sent again", () async {
    claktory.drop = (message) => message.hasCreateProject();
    await expectLater(
      backend.createProject('Site'),
      throwsA(
        isA<RequestFailure>().having(
          (failure) => failure.message,
          'message',
          'Connection lost.',
        ),
      ),
    );
    await until(() => claktory.welcomes > 1);
    await Future<void>.delayed(const Duration(milliseconds: 200));
    expect(claktory.got.where((m) => m.hasCreateProject()), hasLength(1));
  });
}
