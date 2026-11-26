import 'dart:async';

import 'package:factory_app/android/background_connection.dart';
import 'package:factory_app/android/share_browser_lifecycle.dart';
import 'package:factory_app/backend.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import 'support/channel_fakes.dart';

class _Backend extends RecordingBackend {
  var stops = 0;
  var hiddenEdges = 0;
  var resumeEdges = 0;
  late final agentAck = Completer<void>();
  late final chatAck = Completer<void>();

  @override
  void suspend() => stops++;

  @override
  void hidden() {
    hiddenEdges++;
    super.hidden();
  }

  @override
  void resumed({DateTime? now}) {
    resumeEdges++;
    super.resumed(now: now);
  }

  @override
  Future<void> sendMessage(
    String agentId,
    String text, {
    String clientId = '',
    List<Attachment> attachments = const [],
    bool voice = false,
    int checklistItem = 0,
    String checklistAgentId = '',
  }) => agentAck.future;

  @override
  Future<void> sendChatMessage(
    String chatId,
    String text, {
    String clientId = '',
    List<Attachment> attachments = const [],
    bool voice = false,
  }) => chatAck.future;
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late _Backend backend;
  late AndroidBackgroundConnection policy;

  setUp(() {
    backend = _Backend();
  });

  tearDown(() async {
    policy.dispose();
    await backend.close();
  });

  testWidgets('agent and chat sends both protect their whole acknowledgement', (
    tester,
  ) async {
    policy = AndroidBackgroundConnection(backend)..hidden();
    backend.outbox.send('agent-not-in-workspace', 'unsent agent text');
    backend.chatOutbox.send('chat-not-in-workspace', 'unsent chat text');
    await tester.pump(const Duration(minutes: 10));
    expect(backend.stops, 0);
    backend.agentAck.complete();
    await tester.pump(Duration.zero);
    expect(backend.outbox.sending, isFalse);
    expect(backend.stops, 0, reason: 'the other outbox is still on its way');
    backend.chatAck.complete();
    await tester.pump(Duration.zero);
    expect(backend.chatOutbox.sending, isFalse);
    expect(backend.stops, 1);
  });

  testWidgets(
    'an explicit upload/transcription lease protects all async gaps',
    (tester) async {
      policy = AndroidBackgroundConnection(backend)..hidden();
      final upload = Completer<void>();
      final acknowledged = Completer<void>();
      final sending = backend.holding(() async {
        await upload.future;
        await acknowledged.future;
      });
      expect(backend.holds.value, isTrue);
      await tester.pump(const Duration(minutes: 10));
      expect(backend.stops, 0);
      upload.complete();
      await tester.pump(const Duration(minutes: 10));
      expect(
        backend.stops,
        0,
        reason: 'waiting for the final send, not a chunk',
      );
      acknowledged.complete();
      await sending;
      await tester.pump(Duration.zero);
      expect(backend.holds.value, isFalse);
      expect(backend.stops, 1);
    },
  );

  testWidgets(
    'failed messages stay in the outbox without holding a connection',
    (tester) async {
      policy = AndroidBackgroundConnection(backend)..hidden();
      final outgoing = backend.outbox.send('agent', 'keep my text');
      backend.agentAck.completeError(const RequestFailure('Refused.'));
      await tester.pump(const Duration(seconds: 25));
      expect(outgoing.failed, isTrue);
      expect(backend.outbox.of('agent'), [outgoing]);
      expect(outgoing.text, 'keep my text');
      expect(backend.stops, 1);
    },
  );

  testWidgets('a failed lease releases its hold without another grace period', (
    tester,
  ) async {
    policy = AndroidBackgroundConnection(backend)..hidden();
    final operation = Completer<void>();
    final result = backend.holding(() => operation.future);
    final error = expectLater(result, throwsStateError);
    await tester.pump(const Duration(seconds: 25));
    expect(backend.stops, 0);
    operation.completeError(StateError('file disappeared'));
    await error;
    await tester.pump(Duration.zero);
    expect(backend.holds.value, isFalse);
    expect(backend.stops, 1);
  });

  testWidgets(
    'initial hidden app sets the Backend timestamp and starts grace',
    (tester) async {
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
      addTearDown(() {
        tester.binding.handleAppLifecycleStateChanged(
          AppLifecycleState.resumed,
        );
      });
      policy = AndroidBackgroundConnection(backend);
      expect(backend.hiddenEdges, 1);
      await tester.pump(const Duration(seconds: 25));
      expect(backend.stops, 1);
    },
  );

  testWidgets('disposing the binding twice cancels late work callbacks', (
    tester,
  ) async {
    policy = AndroidBackgroundConnection(backend)..hidden();
    policy.dispose();
    policy.dispose();
    await backend.holding(() async {});
    await tester.pump(const Duration(minutes: 10));
    expect(backend.stops, 0);
  });

  testWidgets('native tab holds only its origin and close starts fresh grace', (
    tester,
  ) async {
    final events = ChannelEvents('factory/share-browser/events');
    final browser = AndroidShareBrowserLifecycle();
    final other = _Backend();
    final otherPolicy = AndroidBackgroundConnection(other, browser: browser)
      ..hidden();
    addTearDown(() async {
      otherPolicy.dispose();
      policy.dispose();
      browser.dispose();
      await other.close();
      events.dispose();
    });
    policy = AndroidBackgroundConnection(backend, browser: browser)..hidden();
    await tester.pump(Duration.zero);
    browser.opening(backend);
    expect(backend.keepTryingWhileHidden, isFalse);
    await events.emit({'visible': true});
    await tester.pump(const Duration(minutes: 5));
    expect(backend.keepTryingWhileHidden, isTrue);
    expect(backend.stops, 0);
    expect(other.keepTryingWhileHidden, isFalse);
    expect(other.stops, 1);
    await events.emit({'visible': false});
    expect(backend.keepTryingWhileHidden, isFalse);
    await tester.pump(const Duration(milliseconds: 24999));
    expect(backend.stops, 0);
    await tester.pump(const Duration(milliseconds: 1));
    expect(backend.stops, 1);
  });

  testWidgets('screen off then on rearms the tab without resuming Flutter', (
    tester,
  ) async {
    final events = ChannelEvents('factory/share-browser/events');
    final browser = AndroidShareBrowserLifecycle();
    addTearDown(() {
      policy.dispose();
      browser.dispose();
      events.dispose();
    });
    policy = AndroidBackgroundConnection(backend, browser: browser)..hidden();
    await tester.pump(Duration.zero);
    browser.opening(backend);
    await events.emit({'visible': true});
    await tester.pump(const Duration(minutes: 5));
    expect(backend.stops, 0);
    // Chrome unbinds on screen-off. Flutter's activity stays stopped.
    await events.emit({'visible': false});
    await tester.pump(const Duration(seconds: 25));
    expect(backend.stops, 1);
    expect(backend.keepTryingWhileHidden, isFalse);
    // On screen-on Chrome binds again, with no MainActivity resume.
    await events.emit({'visible': true});
    expect(backend.keepTryingWhileHidden, isTrue);
    expect(backend.resumeEdges, 0);
    await tester.pump(const Duration(minutes: 5));
    expect(backend.stops, 1);
    await events.emit({'visible': false});
    await tester.pump(const Duration(seconds: 25));
    expect(backend.stops, 2, reason: 'the second trip away has its own grace');
    expect(backend.resumeEdges, 0);
  });

  testWidgets('replacement tab transfers retry permission to its own origin', (
    tester,
  ) async {
    final events = ChannelEvents('factory/share-browser/events');
    final browser = AndroidShareBrowserLifecycle();
    final other = _Backend();
    final otherPolicy = AndroidBackgroundConnection(other, browser: browser)
      ..hidden();
    addTearDown(() async {
      otherPolicy.dispose();
      policy.dispose();
      browser.dispose();
      await other.close();
      events.dispose();
    });
    policy = AndroidBackgroundConnection(backend, browser: browser)..hidden();
    await tester.pump(Duration.zero);
    final first = browser.opening(backend);
    await events.emit({'visible': true});
    expect(backend.keepTryingWhileHidden, isTrue);
    browser.opening(other);
    browser.openFailed(first);
    expect(backend.keepTryingWhileHidden, isFalse);
    expect(other.keepTryingWhileHidden, isTrue);
    await tester.pump(const Duration(seconds: 25));
    expect(backend.stops, 1);
    expect(other.stops, 0);
  });

  testWidgets(
    'disposing the origin clears permission and ignores late events',
    (tester) async {
      final events = ChannelEvents('factory/share-browser/events');
      final browser = AndroidShareBrowserLifecycle();
      // Another watcher keeps the native stream alive after this Backend goes.
      final unwatch = browser.watch(() {});
      addTearDown(() {
        policy.dispose();
        unwatch();
        browser.dispose();
        events.dispose();
      });
      policy = AndroidBackgroundConnection(backend, browser: browser)..hidden();
      await tester.pump(Duration.zero);
      browser.opening(backend);
      await events.emit({'visible': true});
      expect(backend.keepTryingWhileHidden, isTrue);
      policy.dispose();
      expect(backend.keepTryingWhileHidden, isFalse);
      await events.emit({'visible': false});
      await events.emit({'visible': true});
      expect(backend.keepTryingWhileHidden, isFalse);
      expect(browser.visibleFor(backend), isFalse);
      await tester.pump(const Duration(minutes: 5));
      expect(backend.stops, 0);
    },
  );

  testWidgets('channel failure releases retry permission with ordinary grace', (
    tester,
  ) async {
    final events = ChannelEvents('factory/share-browser/events');
    final browser = AndroidShareBrowserLifecycle();
    addTearDown(() {
      policy.dispose();
      browser.dispose();
      events.dispose();
    });
    policy = AndroidBackgroundConnection(backend, browser: browser)..hidden();
    await tester.pump(Duration.zero);
    browser.opening(backend);
    await events.emit({'visible': true});
    await tester.pump(const Duration(minutes: 5));
    expect(backend.stops, 0);
    await events.error('failed', 'Fixture native failure');
    expect(backend.keepTryingWhileHidden, isFalse);
    await tester.pump(const Duration(seconds: 25));
    expect(backend.stops, 1);
  });

  testWidgets('inactive alone does not start a background timer', (
    tester,
  ) async {
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
    addTearDown(() {
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    });
    policy = AndroidBackgroundConnection(backend);
    await tester.pump(const Duration(minutes: 10));
    expect(backend.hiddenEdges, 0);
    expect(backend.stops, 0);
  });
}
