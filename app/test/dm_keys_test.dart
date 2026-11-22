// The Dart half of `factory/dm-keys` (dm_keys.dart): absent and unreadable
// apart, and no native half means unavailable.
import 'package:factory_app/people/dm_keys.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('factory/dm-keys-test');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  tearDown(() => messenger.setMockMethodCallHandler(channel, null));

  test('kept, absent and unreadable are three answers', () async {
    final kept = <String, Uint8List>{};
    messenger.setMockMethodCallHandler(channel, (call) async {
      final args = (call.arguments as Map?)?.cast<String, Object?>();
      switch (call.method) {
        case 'available':
          return true;
        case 'put':
          kept[args!['name']! as String] = args['bytes']! as Uint8List;
          return null;
        case 'get':
          final name = args!['name']! as String;
          if (name == 'store') {
            throw PlatformException(code: 'unreadable', message: 'damaged');
          }
          return kept[name];
        case 'delete':
          kept.remove(args!['name']);
          return null;
      }
      return null;
    });
    const keys = DmKeys(channel: channel);
    expect(await keys.available(), isTrue);
    expect(await keys.get(DmKeyName.identity), isNull);
    await keys.put(DmKeyName.identity, Uint8List.fromList([1, 2, 3]));
    expect(await keys.get(DmKeyName.identity), [1, 2, 3]);
    await expectLater(
      keys.get(DmKeyName.store),
      throwsA(isA<DmKeyUnreadable>()),
    );
    await keys.delete(DmKeyName.identity);
    expect(await keys.get(DmKeyName.identity), isNull);
  });

  test('without a native half, the keys are unavailable', () async {
    expect(await const DmKeys(channel: channel).available(), isFalse);
  });
}
