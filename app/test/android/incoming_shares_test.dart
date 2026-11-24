import 'dart:async';

import 'package:factory_app/android/incoming_shares.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support/channel_fakes.dart';

Map<String, Object?> envelope(int id, {String? error}) => {
  'id': id,
  'text': 'Shared $id 日本語 👩🏽‍💻',
  'subject': 'Title',
  'files': ['/cache/Attachments/$id/example.png'],
  'error': error,
  'text_cut': false,
};

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('native envelope', () {
    test('retains subject, Unicode, files and truncation marker', () {
      final share = parseAndroidIncomingShare({
        ...envelope(1),
        'text_cut': true,
      })!;
      expect(share.id, 1);
      expect(share.draftText, 'Title\nShared 1 日本語 👩🏽‍💻');
      expect(share.files.single, '/cache/Attachments/1/example.png');
      expect(share.textCut, isTrue);
      expect(share.hasContent, isTrue);
      expect(() => share.files.add('/other'), throwsUnsupportedError);
    });

    test('does not duplicate a shared title identical to text', () {
      final share = parseAndroidIncomingShare({
        ...envelope(1),
        'subject': 'same',
        'text': 'same',
      })!;
      expect(share.draftText, 'same');
    });

    test('retains error-only shares and unknown future errors', () {
      for (final error in [
        'too_many',
        'refused',
        'too_large',
        'no_space',
        'copy_failed',
        'future',
      ]) {
        final share = parseAndroidIncomingShare({
          ...envelope(1, error: error),
          'text': null,
          'subject': null,
          'files': <String>[],
        })!;
        expect(share.hasContent, isFalse);
        expect(share.error, error);
      }
    });

    test('drops truly empty intake without inventing content', () {
      expect(
        parseAndroidIncomingShare({
          ...envelope(1),
          'text': '',
          'subject': null,
          'files': <String>[],
        }),
        isNull,
      );
    });

    test('deduplicates paths without changing order', () {
      final share = parseAndroidIncomingShare({
        ...envelope(1),
        'files': ['/cache/a', '/cache/b', '/cache/a'],
      })!;
      expect(share.files, ['/cache/a', '/cache/b']);
    });

    final malformed = <String, Object?>{
      'null': null,
      'list': [],
      'missing keys': {},
      'id zero': {...envelope(1), 'id': 0},
      'id negative': {...envelope(1), 'id': -1},
      'id float': {...envelope(1), 'id': 1.5},
      'id string': {...envelope(1), 'id': '1'},
      'id too large': {...envelope(1), 'id': 0x80000000},
      'text type': {...envelope(1), 'text': 1},
      'text too long': {...envelope(1), 'text': 'a' * 100001},
      'subject type': {...envelope(1), 'subject': true},
      'subject too long': {...envelope(1), 'subject': 'a' * 1001},
      'files null': {...envelope(1), 'files': null},
      'files wrong type': {...envelope(1), 'files': '/cache/a'},
      'files over limit': {
        ...envelope(1),
        'files': List.filled(21, '/cache/a'),
      },
      'file not string': {
        ...envelope(1),
        'files': [1],
      },
      'relative path': {
        ...envelope(1),
        'files': ['relative.png'],
      },
      'provider URI': {
        ...envelope(1),
        'files': ['content://provider/file'],
      },
      'file URI': {
        ...envelope(1),
        'files': ['file:///cache/file'],
      },
      'null byte path': {
        ...envelope(1),
        'files': ['/cache/\u0000.png'],
      },
      'overlong path': {
        ...envelope(1),
        'files': ['/${'a' * 4096}'],
      },
      'cut missing': {...envelope(1)}..remove('text_cut'),
      'cut not bool': {...envelope(1), 'text_cut': 1},
      'error not string': {...envelope(1), 'error': {}},
      'error empty': {...envelope(1), 'error': ''},
    };
    for (final entry in malformed.entries) {
      test('rejects ${entry.key}', () {
        expect(parseAndroidIncomingShare(entry.value), isNull);
      });
    }
  });

  group('channel queue', () {
    const channel = MethodChannel('factory/shared');
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    late AndroidIncomingShares shares;
    late List<MethodCall> calls;
    late Future<Object?> Function(MethodCall) handle;

    setUp(() {
      calls = [];
      handle = (call) async => call.method == 'listen' ? [] : null;
      messenger.setMockMethodCallHandler(channel, (call) {
        calls.add(call);
        return handle(call);
      });
      shares = AndroidIncomingShares(channel: channel);
    });
    tearDown(() {
      shares.dispose();
      messenger.setMockMethodCallHandler(channel, null);
    });

    test(
      'listen once, cold/warm race ordered and deduplicated before ready',
      () async {
        final cold = Completer<Object?>();
        handle = (_) => cold.future;
        var notifications = 0;
        shares.addListener(() => notifications++);
        final started = shares.start();
        expect(shares.start(), same(started));
        await sendNativeMethod(channel, MethodCall('received', envelope(3)));
        expect(shares.ready, isFalse);
        expect(notifications, 0);
        expect(shares.claim(3), isNull);
        cold.complete([envelope(2), envelope(1), envelope(3)]);
        await started;
        expect(shares.ready, isTrue);
        expect(shares.pending.map((s) => s.id), [1, 2, 3]);
        expect(notifications, 1);
        expect(calls.where((c) => c.method == 'listen'), hasLength(1));
        await sendNativeMethod(channel, MethodCall('received', envelope(4)));
        await sendNativeMethod(channel, MethodCall('received', envelope(2)));
        expect(shares.pending.map((s) => s.id), [1, 2, 3, 4]);
        expect(notifications, 2);
      },
    );

    test(
      'claim transfers once, and cancellation cannot delete accepted files',
      () async {
        handle = (call) async => call.method == 'listen' ? [envelope(1)] : null;
        await shares.start();
        expect(shares.claim(1)!.id, 1);
        expect(shares.claim(1), isNull);
        await shares.discard(1);
        await sendNativeMethod(channel, MethodCall('received', envelope(1)));
        expect(shares.pending, isEmpty);
        expect(calls.map((c) => c.method), ['listen']);
      },
    );

    test(
      'vanished destination returns the same intake for another choice',
      () async {
        handle = (call) async => call.method == 'listen' ? [envelope(1)] : null;
        await shares.start();
        final accepted = shares.claim(1)!;
        shares.retryClaimed(accepted);
        shares.retryClaimed(accepted);
        expect(shares.pending.single, same(accepted));
        await sendNativeMethod(channel, MethodCall('received', envelope(1)));
        expect(shares.pending, hasLength(1));
        final retried = shares.claim(1)!;
        expect(retried.files, accepted.files);
        expect(calls.map((call) => call.method), ['listen']);
      },
    );

    test('cancel removes only chosen intake and discards once', () async {
      handle = (call) async =>
          call.method == 'listen' ? [envelope(1), envelope(2)] : null;
      await shares.start();
      await shares.discard(1);
      await shares.discard(1);
      expect(shares.pending.single.id, 2);
      expect(calls.where((c) => c.method == 'discard').single.arguments, {
        'id': 1,
      });
      expect(() => shares.pending.clear(), throwsUnsupportedError);
      shares.claim(2);
    });

    test(
      'unknown methods and malformed payloads do not affect the queue',
      () async {
        handle = (call) async =>
            call.method == 'listen' ? [null, {}, envelope(1)] : null;
        await shares.start();
        await sendNativeMethod(channel, MethodCall('other', envelope(2)));
        await sendNativeMethod(
          channel,
          const MethodCall('received', {'id': 3}),
        );
        expect(shares.pending.single.id, 1);
        shares.claim(1);
      },
    );

    for (final missing in [true, false]) {
      test(
        'unavailable native listen stays quiet (missing=$missing)',
        () async {
          handle = (_) async => missing
              ? throw MissingPluginException()
              : throw PlatformException(code: 'unavailable');
          await shares.start();
          expect(shares.ready, isTrue);
          expect(shares.pending, isEmpty);
        },
      );
    }

    test('failed discard still finishes cancellation', () async {
      handle = (call) async {
        if (call.method == 'listen') return [envelope(1)];
        throw PlatformException(code: 'cleanup_failed');
      };
      await shares.start();
      await shares.discard(1);
      expect(shares.pending, isEmpty);
    });

    test('dispose during cold read cleans late unaccepted copies only', () async {
      final cold = Completer<Object?>();
      handle = (call) async => call.method == 'listen' ? cold.future : null;
      final started = shares.start();
      await sendNativeMethod(channel, MethodCall('received', envelope(2)));
      shares.dispose();
      // This case manages disposal itself; replace the test's tearDown object.
      shares = AndroidIncomingShares(channel: channel);
      cold.complete([envelope(1), envelope(2)]);
      await started;
      expect(
        calls.where((c) => c.method == 'discard').map((c) => c.arguments),
        [
          {'id': 2},
          {'id': 1},
        ],
      );
    });
  });
}
