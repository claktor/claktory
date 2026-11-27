import 'package:factory_app/connections.dart';
import 'package:flutter_test/flutter_test.dart';

// A device paired again with a Claktory it already had
// (after that Claktory lost its sign-in, say) was listed there twice, and
// kept both here, the old one knocking and turned away.

final _claktory = 'c' * 64;
String _code(String token) => 'factory://?t=$token&e=$_claktory&name=desktop';

void main() {
  test('a new code for a Claktory already here takes its place, with its '
      'key', () async {
    final storage = MemoryConnectionStorage();
    final connections = await Connections.load(storage);
    final first = parsePairingCode(_code('a' * 64))!;
    connections.use(first);
    final key = connections.deviceKey(first, make: () => 'k' * 64);
    final other = parsePairingCode(
      'factory://?t=${'b' * 64}&e=${'d' * 64}&name=nas',
    )!;
    connections.use(other);
    connections.deviceKey(other, make: () => 'n' * 64);

    // The same Claktory's new code: before it's kept, its key is the one
    // this device has there.
    final again = parsePairingCode(_code('e' * 64))!;
    expect(connections.deviceKey(again, make: () => 'x' * 64), key);
    connections.use(again);
    expect(connections.saved.map((target) => target.code), [
      again.code,
      other.code,
    ], reason: 'in its place, once');
    expect(connections.current, again);
    expect(connections.deviceKey(again, make: () => 'x' * 64), key);
    await pumpEventQueue();
    expect(storage.kept.saved, [again.code, other.code]);
    expect(storage.kept.keys, {again.code: key, other.code: 'n' * 64});
  });

  test('another Claktory gets a key of its own', () async {
    final connections = await Connections.load(MemoryConnectionStorage());
    final first = parsePairingCode(_code('a' * 64))!;
    connections.use(first);
    connections.deviceKey(first, make: () => 'k' * 64);
    final other = parsePairingCode(
      'factory://?t=${'b' * 64}&e=${'d' * 64}&name=nas',
    )!;
    expect(connections.deviceKey(other, make: () => 'n' * 64), 'n' * 64);
  });

  test('picked from the account, a Claktory already here shows the one '
      'here', () async {
    final connections = await Connections.load(MemoryConnectionStorage());
    final paired = parsePairingCode(_code('a' * 64))!;
    connections.use(paired);
    final found = parsePairingCode(
      'factory://?account=acct&e=$_claktory&name=desktop',
      fromAccount: true,
    )!;
    expect(found.fromAccount, isTrue);
    connections.use(found);
    expect(connections.saved, [paired]);
    expect(connections.current, paired);
  });
}
