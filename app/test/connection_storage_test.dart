import 'package:factory_app/connections.dart';
import 'package:factory_app/unreadable.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

final _token = 'a' * 64;
final _code = 'factory://?t=$_token&e=${'c' * 64}&name=mac-mini';
final _other = 'factory://?t=$_token&e=${'d' * 64}&name=nas';

/// What the device kept: one Claktory, its key, and an account.
StoredConnections _kept() => StoredConnections(
  current: _code,
  saved: [_code],
  keys: {_code: 'k' * 64},
  account: const {'key': 'seed', 'email': 'matt@example.com'},
);

const _locked = ConnectionStorageFailure(
  'credentials_unavailable',
  'User interaction is not allowed.',
);
const _broken = ConnectionStorageFailure(
  'credentials_unreadable',
  'The data could not be read.',
);

void main() {
  group('a store that fails to read', () {
    test('is never written over, whatever changes meanwhile', () async {
      final storage = MemoryConnectionStorage()
        ..kept = _kept()
        ..readFails = _locked;
      final connections = await Connections.load(storage);
      expect(connections.readFailure, _locked);
      expect(connections.saved, isEmpty);

      // Every way something gets kept.
      connections.use(parsePairingCode(_other));
      connections.deviceKey(parsePairingCode(_other)!);
      // The account kept something (a sign-in, say).
      connections.account.onKept?.call();
      connections.keep();
      await pumpEventQueue();

      expect(storage.writes, 0);
      expect(storage.kept.current, _code);
      expect(storage.kept.keys, {_code: 'k' * 64});
      expect(storage.kept.account['key'], 'seed');
    });

    test('read again once it can be, it has what was kept', () async {
      final storage = MemoryConnectionStorage()
        ..kept = _kept()
        ..readFails = _locked;
      expect((await Connections.load(storage)).readFailure, isNotNull);
      storage.readFails = null;
      final connections = await Connections.load(storage);
      expect(connections.readFailure, isNull);
      expect(connections.current?.code, _code);
      expect(connections.deviceKey(connections.current!), 'k' * 64);
      expect(connections.account.toKept()['key'], 'seed');
    });

    test('started over, it is empty and kept again', () async {
      final storage = MemoryConnectionStorage()
        ..kept = _kept()
        ..readFails = _broken;
      final connections = await Connections.load(storage);
      await connections.startOver();
      expect(storage.resets, 1);
      expect(connections.readFailure, isNull);
      connections.use(parsePairingCode(_other));
      await pumpEventQueue();
      expect(storage.kept.saved, [_other]);
    });
  });

  test(
    'a failed write says so until one works, and can be tried again',
    () async {
      final storage = MemoryConnectionStorage();
      final connections = await Connections.load(storage);
      storage.writeFails = const ConnectionStorageFailure(
        'credentials_write_failed',
        'The disk is full.',
      );
      connections.use(parsePairingCode(_code));
      await pumpEventQueue();
      expect(connections.writeFailure.value?.code, 'credentials_write_failed');
      expect(connections.current?.code, _code, reason: 'what shows is right');

      storage.writeFails = null;
      connections.keep();
      await pumpEventQueue();
      expect(connections.writeFailure.value, isNull);
      expect(storage.kept.current, _code);
    },
  );

  group("the device's store", () {
    const channel = MethodChannel('factory/device');
    final calls = <String>[];
    Object? Function(MethodCall call) answer = (_) => null;

    setUp(() {
      calls.clear();
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(channel, (call) async {
            calls.add(call.method);
            return answer(call);
          });
    });
    tearDown(() {
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(channel, null);
    });

    test("says why it can't read, write or start over", () async {
      final storage = DeviceConnectionStorage();
      answer = (call) => throw PlatformException(
        code: switch (call.method) {
          'connections' => 'credentials_unavailable',
          'setConnections' => 'credentials_read_failed',
          _ => 'credentials_reset_failed',
        },
        message: 'said by ${call.method}',
      );
      final connections = await Connections.load(storage);
      expect(connections.readFailure?.code, 'credentials_unavailable');
      expect(connections.readFailure?.message, 'said by connections');
      expect(connections.readFailure!.transient, isTrue);
      await expectLater(
        storage.write(const StoredConnections(current: '', saved: [])),
        throwsA(
          isA<ConnectionStorageFailure>().having(
            (failure) => failure.code,
            'code',
            'credentials_read_failed',
          ),
        ),
      );
      await expectLater(
        storage.reset(),
        throwsA(isA<ConnectionStorageFailure>()),
      );
      expect(calls, ['connections', 'setConnections', 'resetConnections']);
    });

    test('nothing kept yet is an empty store, not a failure', () async {
      answer = (_) => <String, Object?>{};
      final connections = await Connections.load(DeviceConnectionStorage());
      expect(connections.readFailure, isNull);
      expect(connections.saved, isEmpty);
    });
  });

  group('the screen', () {
    Future<List<Connections>> show(
      WidgetTester tester,
      MemoryConnectionStorage storage,
      ConnectionStorageFailure failure,
    ) async {
      tester.view.physicalSize = const Size(1000, 700);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      final read = <Connections>[];
      await tester.pumpWidget(
        UnreadableApp(storage: storage, failure: failure, onRead: read.add),
      );
      await tester.pump();
      return read;
    }

    testWidgets('says what failed, and Try again starts once it reads', (
      tester,
    ) async {
      final storage = MemoryConnectionStorage()
        ..kept = _kept()
        ..readFails = _broken;
      final read = await show(tester, storage, _broken);
      expect(find.textContaining("sign-in and connections"), findsOneWidget);
      expect(find.text('The data could not be read.'), findsOneWidget);

      await tester.tap(find.text('Try again'));
      await tester.pump();
      expect(read, isEmpty, reason: 'still unreadable');

      storage.readFails = null;
      await tester.tap(find.text('Try again'));
      await tester.pump();
      expect(read.single.current?.code, _code);
      expect(storage.resets, 0);
    });

    testWidgets('Start over asks first, then clears it and starts', (
      tester,
    ) async {
      final storage = MemoryConnectionStorage()
        ..kept = _kept()
        ..readFails = _broken;
      final read = await show(tester, storage, _broken);
      await tester.tap(find.text('Start over…'));
      await tester.pump();
      expect(find.text('Start over?'), findsOneWidget);
      await tester.tap(find.text('Cancel'));
      await tester.pump();
      expect(storage.resets, 0);

      await tester.tap(find.text('Start over…'));
      await tester.pump();
      await tester.tap(find.text('Start over'));
      await tester.pump();
      expect(storage.resets, 1);
      expect(read.single.readFailure, isNull);
      expect(read.single.saved, isEmpty);
    });

    testWidgets("a start over that fails says so, and changes nothing", (
      tester,
    ) async {
      final storage = MemoryConnectionStorage()
        ..kept = _kept()
        ..readFails = _broken
        ..resetFails = const ConnectionStorageFailure(
          'credentials_reset_failed',
          'Not allowed.',
        );
      final read = await show(tester, storage, _broken);
      await tester.tap(find.text('Start over…'));
      await tester.pump();
      await tester.tap(find.text('Start over'));
      await tester.pump();
      expect(find.text("Couldn't start over: Not allowed."), findsOneWidget);
      expect(read, isEmpty);
      expect(storage.kept.current, _code);
    });

    testWidgets('locked, it tries again by itself when the app is back', (
      tester,
    ) async {
      final storage = MemoryConnectionStorage()
        ..kept = _kept()
        ..readFails = _locked;
      final read = await show(tester, storage, _locked);
      expect(find.textContaining('Unlock it'), findsOneWidget);
      storage.readFails = null;
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await tester.pump();
      expect(read.single.current?.code, _code);
    });
  });

  testWidgets('a change that was not saved says so at the foot', (
    tester,
  ) async {
    final storage = MemoryConnectionStorage();
    final connections = await Connections.load(storage);
    await tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xFF000000),
        builder: (context, _) => Align(
          alignment: Alignment.bottomCenter,
          child: UnsavedNote(connections: connections),
        ),
      ),
    );
    expect(find.textContaining("Couldn't save"), findsNothing);
    storage.writeFails = const ConnectionStorageFailure(
      'credentials_write_failed',
      'The disk is full.',
    );
    connections.use(parsePairingCode(_code));
    await tester.pump();
    await tester.pump();
    expect(find.textContaining("Couldn't save"), findsOneWidget);
    storage.writeFails = null;
    await tester.tap(find.text('Try again'));
    await tester.pump();
    await tester.pump();
    expect(find.textContaining("Couldn't save"), findsNothing);
  });
}
