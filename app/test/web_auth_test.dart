import 'dart:io';

import 'package:factory_app/web_auth.dart';
import 'package:flutter_test/flutter_test.dart';

Future<(int, String)> _get(Uri url) async {
  final client = HttpClient();
  try {
    final response = await (await client.getUrl(url)).close();
    final body = await response
        .transform(const SystemEncoding().decoder)
        .join();
    return (response.statusCode, body);
  } finally {
    client.close(force: true);
  }
}

void main() {
  test(
    'the loopback receiver takes the first visit to its path and stops',
    () async {
      final receiver = await LoopbackReceiver.start(path: '/oauth/cloudflare');
      expect(receiver.address.host, '127.0.0.1');
      expect(receiver.address.path, '/oauth/cloudflare');
      expect(receiver.address.port, greaterThan(0));

      // Another path changes nothing.
      final other = await _get(receiver.address.replace(path: '/favicon.ico'));
      expect(other.$1, 404);

      final (status, body) = await _get(
        receiver.address.replace(query: 'code=abc&state=xyz'),
      );
      expect(status, 200);
      expect(body, contains('go back to Claktory'));
      final back = await receiver.back;
      expect(back!.queryParameters, {'code': 'abc', 'state': 'xyz'});
      expect(back.path, '/oauth/cloudflare');

      // It stopped listening.
      await expectLater(
        _get(receiver.address),
        throwsA(isA<SocketException>()),
      );
    },
  );

  test('closing the loopback receiver answers null', () async {
    final receiver = await LoopbackReceiver.start();
    await receiver.close();
    expect(await receiver.back, isNull);
  });

  test('a loopback receiver on a port that is taken fails to start', () async {
    final first = await LoopbackReceiver.start();
    final port = first.address.port;
    await expectLater(
      LoopbackReceiver.start(port: port),
      throwsA(isA<SocketException>()),
    );
    await first.close();
    // Free again: it takes that port.
    final second = await LoopbackReceiver.start(port: port);
    expect(second.address.port, port);
    await second.close();
  });
}
