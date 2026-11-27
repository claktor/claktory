// The secret clipboard's sweeps and late clears:
// a copied code is taken off the clipboard once dead, even when the app was
// away at that moment or stopped since.
import 'package:factory_app/people/secret_clipboard.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'secret_clipboard_fake.dart';

void main() {
  testWidgets('a clear due while the app is away is tried again when it '
      'comes back, then let go', (tester) async {
    final clipboard = FakeSecretClipboard();
    clearWhenDue(clipboard, 7, const Duration(minutes: 10));
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
    await tester.pump(const Duration(minutes: 10));
    // Tried, but Android can't check from behind: kept.
    expect(clipboard.cleared, [7]);

    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pump(const Duration(milliseconds: 500));
    expect(clipboard.cleared, [7, 7]);

    // Settled in front: not again.
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pump(const Duration(milliseconds: 500));
    expect(clipboard.cleared, [7, 7]);
  });

  testWidgets('due in front, it is cleared then and let go', (tester) async {
    final clipboard = FakeSecretClipboard();
    clearWhenDue(clipboard, 3, const Duration(minutes: 30));
    await tester.pump(const Duration(minutes: 29));
    expect(clipboard.cleared, isEmpty);
    await tester.pump(const Duration(minutes: 1));
    expect(clipboard.cleared, [3]);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pump(const Duration(milliseconds: 500));
    expect(clipboard.cleared, [3]);
  });

  testWidgets('the platform sweeps at launch and on each return; one without '
      'a sweep is fine', (tester) async {
    const channel = MethodChannel('test/secret-clipboard');
    var sweeps = 0;
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(channel, (
      call,
    ) async {
      if (call.method == 'sweep') sweeps++;
      return null;
    });
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        channel,
        null,
      ),
    );
    final stop = startClipboardSweeps(channel: channel);
    addTearDown(stop);
    await tester.pump(const Duration(milliseconds: 500));
    expect(sweeps, 1);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pump(const Duration(milliseconds: 500));
    expect(sweeps, 2);

    // A native half without it: nothing thrown.
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      channel,
      (call) async => throw MissingPluginException(),
    );
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    await tester.pump(const Duration(milliseconds: 500));
    expect(tester.takeException(), isNull);
  });
}
