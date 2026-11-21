import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// A strict wire transcript. Unexpected calls fail even if the production
/// adapter catches the resulting exception; assertDone checks the full trace.
class ChannelReply {
  const ChannelReply(this.method, {this.arguments, this.result, this.error});
  final String method;
  final Object? arguments;
  final Object? result;
  final PlatformException? error;
}

class ChannelScript {
  ChannelScript(this.name, List<ChannelReply> replies)
    : _replies = List.of(replies) {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(MethodChannel(name), _handle);
  }

  final String name;
  final List<ChannelReply> _replies;
  final calls = <MethodCall>[];
  final _mismatches = <String>[];

  Future<Object?> _handle(MethodCall call) async {
    calls.add(call);
    if (_replies.isEmpty) {
      _mismatches.add('Unexpected ${call.method}');
      throw PlatformException(code: 'unexpected_test_call');
    }
    final reply = _replies.removeAt(0);
    if (call.method != reply.method ||
        !equals(reply.arguments).matches(call.arguments, {})) {
      _mismatches.add(
        'Expected ${reply.method}, got ${call.method} or wrong arguments',
      );
      throw PlatformException(code: 'unexpected_test_call');
    }
    if (reply.error case final error?) throw error;
    return reply.result;
  }

  void assertDone() {
    expect(_mismatches, isEmpty, reason: name);
    expect(_replies, isEmpty, reason: 'Uncalled replies on $name');
  }

  void dispose() => TestDefaultBinaryMessengerBinding
      .instance
      .defaultBinaryMessenger
      .setMockMethodCallHandler(MethodChannel(name), null);
}

/// A real StandardMethodCodec event stream, including listen/cancel messages.
class ChannelEvents {
  ChannelEvents(this.name) {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(MethodChannel(name), (call) async {
          expect(call.arguments, isNull);
          switch (call.method) {
            case 'listen':
              listening = true;
            case 'cancel':
              listening = false;
            default:
              fail('Unexpected event-channel method: ${call.method}');
          }
          return null;
        });
  }

  final String name;
  bool listening = false;
  static const _codec = StandardMethodCodec();

  Future<void> emit(Object? value) =>
      _send(_codec.encodeSuccessEnvelope(value));
  Future<void> error(String code, String message) =>
      _send(_codec.encodeErrorEnvelope(code: code, message: message));

  Future<void> _send(ByteData data) async {
    expect(listening, isTrue, reason: '$name must have a subscriber');
    final delivered = Completer<void>();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .handlePlatformMessage(name, data, (_) => delivered.complete());
    await delivered.future;
  }

  void dispose() => TestDefaultBinaryMessengerBinding
      .instance
      .defaultBinaryMessenger
      .setMockMethodCallHandler(MethodChannel(name), null);
}

/// Synthetic contract data, not usable credentials or a real QR encoding.
abstract final class AndroidChannelFixtures {
  static const connections = <String, Object?>{
    'current': 'fixture-code',
    'saved': ['fixture-code', 'other-fixture-code'],
    'keys': {'fixture-code': 'fake-endpoint-seed'},
    'account': {
      'key': 'fake-account-seed',
      'service': 'https://account.example.invalid',
      'email': 'person@example.invalid',
    },
  };
  static const wifi = <String, Object?>{
    'satisfied': true,
    'interfaces': ['wifi', 'cellular'],
    'expensive': false,
    'constrained': false,
  };
  static const cellular = <String, Object?>{
    'satisfied': true,
    'interfaces': ['cellular'],
    'expensive': true,
    'constrained': true,
  };
  static const recorderLevel = <String, Object?>{'level': 0.5, 'ms': 1234};
  static const recording = <String, Object?>{
    'path': '/fixture/voice.m4a',
    'durationMs': 1234,
  };
  static Map<String, Object?> get qr => {
    'size': 2,
    'modules': Uint8List.fromList([1, 0, 0, 1]),
  };
}

/// Delivers a native-to-Dart method with the channel's real codec. In
/// particular flutter/navigation is JSON, flutter/backgesture is Standard.
Future<Object?> sendNativeMethod(MethodChannel channel, MethodCall call) async {
  final response = Completer<ByteData?>();
  TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
      .handlePlatformMessage(
        channel.name,
        channel.codec.encodeMethodCall(call),
        response.complete,
      );
  final data = await response.future;
  return data == null ? null : channel.codec.decodeEnvelope(data);
}
