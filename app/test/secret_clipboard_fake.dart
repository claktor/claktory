import 'package:factory_app/people/messages_setup.dart' show secretClipboard;
import 'package:factory_app/people/secret_clipboard.dart';
import 'package:flutter/services.dart' show MethodChannel;
import 'package:flutter_test/flutter_test.dart' show addTearDown;

/// The sensitive clipboard (pipe invites' codes, row 166) as a test sees
/// it: what was copied and for how long, and which copies were cleared.
class FakeSecretClipboard implements SecretClipboard {
  final copies = <(String, int)>[];
  final cleared = <int>[];

  /// Stands in for the app's, until the test ends.
  static FakeSecretClipboard install() {
    final fake = FakeSecretClipboard();
    secretClipboard = fake;
    addTearDown(() => secretClipboard = const SecretClipboard());
    return fake;
  }

  @override
  MethodChannel get channel => throw UnimplementedError();

  @override
  Future<bool> get available async => true;

  @override
  Future<int?> copy(String text, {int lasts = SecretClipboard.seconds}) async {
    copies.add((text, lasts));
    return copies.length;
  }

  @override
  Future<void> clear(int token) async => cleared.add(token);

  @override
  Future<String?> paste() async => null;
}
