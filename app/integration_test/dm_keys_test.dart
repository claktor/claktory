// factory/dm-keys (E6) end to end, through the Dart contract
// (people/dm_keys.dart) and the platform's native half, on a Simulator or a
// device: keys kept, given back, deleted, absent as null, and the store's
// folder.
//
//   cd app && flutter test integration_test/dm_keys_test.dart -d <device>
//
// iPhone and iPad only. Never run it on macOS: `flutter test -d macos`
// launches the app under the real Claktory's bundle id (dev.factory.app).
// The Mac's half is checked without the app, by
// app/macos/RunnerTests/DmKeysCheck.swift.
import 'dart:io';

import 'package:factory_app/people/dm_keys.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('the keys of direct messages, kept on this device', (_) async {
    const keys = DmKeys();
    final available = await keys.available();
    if (!Platform.isIOS) return;
    expect(available, isTrue);
    await keys.delete(DmKeyName.store);
    expect(await keys.get(DmKeyName.store), isNull);
    final key = Uint8List.fromList(List.generate(32, (i) => i * 7 % 256));
    await keys.put(DmKeyName.store, key);
    expect(await keys.get(DmKeyName.store), key);
    await keys.delete(DmKeyName.store);
    expect(await keys.get(DmKeyName.store), isNull);

    final folder = await keys.folder();
    expect(Directory(folder).existsSync(), isTrue);
    expect(await keys.folder(), folder);
    expect(folder, endsWith('/dm'));
  });
}
