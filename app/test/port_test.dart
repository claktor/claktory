import 'dart:io';

import 'package:factory_app/backend.dart' show localPort;
import 'package:flutter_test/flutter_test.dart';

void main() {
  test(
    "a debug build or a test talks to the dev Claktory, not the person's",
    () {
      // Tests run in debug mode: only a release build defaults to 7420.
      expect(localPort, '7421');
    },
  );

  test('nothing else in the app names the real port', () {
    final named = [
      for (final file in Directory('lib').listSync(recursive: true))
        if (file is File &&
            file.path.endsWith('.dart') &&
            !file.path.contains('/protocol/') &&
            !file.path.endsWith('/backend.dart') &&
            file.readAsStringSync().contains('7420'))
          file.path,
    ];
    expect(named, isEmpty);
  });
}
