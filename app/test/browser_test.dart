// B5a: the Mac's side of browser control, over fakes: the broker's channel
// (factory/browser) and the backend (b3a's messages). They prove the
// routing and the rules here, not Chrome, the helper or the backend.
import 'dart:async';
import 'dart:convert';

import 'package:factory_app/browser/bridge.dart';
import 'package:factory_app/browser/browser_view.dart';
import 'package:factory_app/browser/session.dart';
import 'package:factory_app/device_account.dart';
import 'package:factory_app/inbox.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

/// The broker, as the app sees it: what was asked of it, and a way to
/// tell the app things.
class _Broker extends BrowserBridge {
  final log = <String>[];
  void Function(BrowserBrokerEvent event)? _tell;
  var sendWorks = true;
  String? startFails;

  void tell(BrowserBrokerEvent event) => _tell!(event);

  @override
  void listen(void Function(BrowserBrokerEvent event) onEvent) =>
      _tell = onEvent;

  @override
  Future<String> start() async {
    if (startFails case final code?) {
      throw BrowserStartFailure(code, '/Users/someone/very/long');
    }
    log.add('start');
    return '/tmp/b.sock';
  }

  @override
  Future<void> stop() async => log.add('stop');

  var isRegistered = true;
  String? registerFails;

  @override
  Future<bool> registered() async => isRegistered;

  @override
  Future<void> register({bool replace = false}) async {
    if (registerFails case final detail?) {
      throw BrowserRegisterFailure(detail);
    }
    log.add('register');
    isRegistered = true;
  }

  @override
  Future<void> unregister() async {
    log.add('unregister');
    isRegistered = false;
  }

  @override
  Future<bool> send(int id, Map<String, Object?> frame) async {
    log.add('send $id ${jsonEncode(frame)}');
    return sendWorks;
  }

  @override
  Future<bool> showReview(int id) async {
    log.add('showReview $id');
    return true;
  }

  @override
  Future<bool> choose(int id) async {
    log.add('choose $id');
    return true;
  }

  @override
  Future<void> end(int id, String code) async => log.add('end $id $code');
}

/// The backend, as the broker reaches it.
class _Port implements BrowserBackendPort {
  final log = <String>[];

  @override
  Future<void> capability({
    required bool helperReady,
    required String brokerInstance,
  }) async => log.add('capability $helperReady $brokerInstance');

  @override
  Future<void> grant({
    required String sessionId,
    required String brokerInstance,
    required String origin,
    required String lease,
  }) async => log.add('grant $sessionId $brokerInstance $origin');

  @override
  Future<void> result({
    required String sessionId,
    required int commandId,
    required String resultJson,
    required String failure,
    required int macMs,
  }) async => log.add('result $sessionId $commandId [$failure] $resultJson');

  @override
  Future<void> heartbeat({
    required String sessionId,
    required String lease,
  }) async => log.add('heartbeat $sessionId');

  @override
  Future<void> stop({
    required String sessionId,
    required String reason,
  }) async => log.add('stop $sessionId $reason');
}

BrowserSession _session({String state = 'pending'}) => BrowserSession(
  id: 's1',
  agentId: 'june',
  purpose: 'Read the release notes',
  origin: 'https://example.com',
  mode: 'read',
  state: state,
  deadlineMs: Int64(
    DateTime.now().add(const Duration(minutes: 15)).millisecondsSinceEpoch,
  ),
  commandsLimit: 100,
);

BrowserCommand _command({
  required String lease,
  int id = 1,
  int sequence = 1,
  String op = '{"op":"read","limit":4096}',
}) => BrowserCommand(
  sessionId: 's1',
  lease: lease,
  commandId: Int64(id),
  sequence: Int64(sequence),
  deadlineMs: 10000,
  opJson: op,
);

/// A started broker with one helper, introduced on [origin].
Future<(BrowserLocal, _Broker, _Port)> _started({
  String origin = 'https://example.com',
}) async {
  final broker = _Broker();
  final port = _Port();
  final local = BrowserLocal(
    bridge: broker,
    backend: port,
    brokerInstance: 'bi',
  );
  await local.start();
  broker
    ..tell(
      const HelperConnected(id: 7, version: 1, caller: 'x', code: '123456'),
    )
    ..tell(
      HelperMessage(7, {
        'type': 'hello',
        'v': 1,
        'tab': 3,
        'origin': origin,
        'title': 'Release notes',
      }),
    );
  return (local, broker, port);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('the channel', () {
    const channel = MethodChannel('factory/browser');
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    tearDown(() => messenger.setMockMethodCallHandler(channel, null));

    test('start, its refusals in words, and frames sent as JSON', () async {
      final calls = <String>[];
      var fail = false;
      messenger.setMockMethodCallHandler(channel, (call) async {
        calls.add('${call.method} ${jsonEncode(call.arguments)}');
        if (call.method == 'start' && fail) {
          throw PlatformException(code: 'unsigned', message: 'ad hoc');
        }
        return switch (call.method) {
          'start' => {'path': '/tmp/b.sock'},
          'send' || 'choose' => true,
          _ => null,
        };
      });
      final bridge = BrowserBridge();
      expect(await bridge.start(), '/tmp/b.sock');
      // The review panel's labels go with start; nothing that decides.
      expect(calls.first, startsWith('start {"version":"dev","words":{'));
      expect(calls.first, contains('"unshown_details":'));
      expect(calls.first, contains('"deny":"Don\'t allow"'));
      fail = true;
      await expectLater(
        bridge.start(),
        throwsA(
          isA<BrowserStartFailure>().having(
            (f) => f.message,
            'message',
            'Browser control works only in the Claktory app from '
                'claktory.com.',
          ),
        ),
      );
      expect(await bridge.send(4, {'type': 'bound'}), isTrue);
      await bridge.end(4, 'stop');
      expect(calls.sublist(2), [
        'send {"id":4,"json":"{\\"type\\":\\"bound\\"}"}',
        'end {"id":4,"code":"stop"}',
      ]);
    });

    test('what the broker says, read', () async {
      final events = <BrowserBrokerEvent>[];
      BrowserBridge().listen(events.add);
      Future<void> say(String method, Map<String, Object?> args) =>
          messenger.handlePlatformMessage(
            'factory/browser',
            const StandardMethodCodec().encodeMethodCall(
              MethodCall(method, args),
            ),
            (_) {},
          );
      await say('helperConnected', {
        'id': 1,
        'version': 1,
        'caller': 'c',
        'code': '654321',
      });
      await say('helperMessage', {'id': 1, 'json': '{"type":"hello"}'});
      await say('helperMessage', {'id': 1, 'json': '[1]'});
      await say('helperGone', {'id': 1, 'code': 'closed', 'detail': 'd'});
      await say('refused', {'code': 'not_helper', 'detail': '/a/path'});
      await say('decided', {'id': 1, 'command': 4, 'allow': true});
      expect(events[0], isA<HelperConnected>());
      expect((events[1] as HelperMessage).frame, {'type': 'hello'});
      expect((events[2] as HelperGone).code, 'unexpected');
      expect((events[3] as HelperGone).code, 'closed');
      expect(events[4], isA<HelperRefused>());
      expect(
        events[5],
        isA<HelperDecided>()
            .having((d) => d.command, 'command', 4)
            .having((d) => d.allow, 'allow', isTrue),
      );
    });

    test('every code in words; anything unknown is the unexpected', () {
      expect(
        browserGoneReason('not_chosen'),
        "You didn't confirm this tab in Claktory within 5 minutes. Click the "
        'Claktory button in Chrome to try again.',
      );
      expect(
        browserGoneReason('something new'),
        "Claktory got something it didn't expect from Chrome.",
      );
      expect(
        browserEndReason('agent_changed', agent: 'June'),
        "June's settings changed.",
      );
      expect(browserEndReason('expired'), "you didn't choose a tab in time.");
      expect(
        browserStartProblem('dev_no_helper'),
        "This build of Claktory Dev doesn't include browser control.",
      );
    });
  });

  group('this Mac', () {
    test(
      'a helper makes it ready; its tab offered only on its origin',
      () async {
        final (local, _, port) = await _started();
        expect(port.log, ['capability false bi', 'capability true bi']);
        final tab = local.tabsFor(_session()).single;
        expect(
          (tab.code, tab.origin, tab.title),
          ('123456', 'https://example.com', 'Release notes'),
        );
        final other = BrowserSession(
          id: 's2',
          origin: 'https://example.org',
          state: 'pending',
        );
        expect(local.tabsFor(other), isEmpty);
      },
    );

    test('a tab says which it is once', () async {
      final (local, broker, _) = await _started();
      broker.tell(
        const HelperMessage(7, {
          'type': 'hello',
          'origin': 'https://evil.example',
        }),
      );
      await Future<void>.delayed(Duration.zero);
      expect(local.tabs[7]!.origin, 'https://example.com');
    });

    test('allowed here: chosen, told whom for, then granted', () async {
      final (local, broker, port) = await _started();
      final tab = local.tabs[7]!;
      expect(await local.allow(_session(), tab, agentName: 'June'), isTrue);
      expect(broker.log[1], 'choose 7');
      final bound = jsonDecode(broker.log[2].substring('send 7 '.length));
      expect(bound['type'], 'bound');
      expect(bound['agent'], 'June');
      expect(bound['purpose'], 'Read the release notes');
      expect(port.log.last, 'grant s1 bi https://example.com');
      // A second session can't take it.
      expect(await local.allow(_session(), tab, agentName: 'June'), isFalse);
      await local.stopSession();
    });

    test('a reviewed action: waits in the panel, decided only there, then '
        'done', () async {
      final (local, broker, port) = await _started();
      await local.allow(_session(), local.tabs[7]!, agentName: 'June');
      final lease = local.active!.lease;
      await local.command(
        _command(
          lease: lease,
          op: '{"op":"click","document":"d1","revision":1,"target":"r1"}',
        ),
      );
      // A decision for another command, or before its review, is nothing.
      broker.tell(const HelperDecided(7, 1, allow: true));
      expect(local.active!.review, BrowserReview.none);
      broker.tell(
        const HelperMessage(7, {'type': 'review', 'tab': 3, 'id': 1}),
      );
      await Future<void>.delayed(Duration.zero);
      expect(local.active!.review, BrowserReview.waiting);
      broker.tell(const HelperDecided(7, 2, allow: true));
      expect(local.active!.review, BrowserReview.waiting);
      broker.tell(const HelperDecided(7, 1, allow: true));
      expect(local.active!.review, BrowserReview.allowed);
      broker.tell(
        const HelperMessage(7, {
          'type': 'result',
          'tab': 3,
          'id': 1,
          'result': {'done': true},
        }),
      );
      await Future<void>.delayed(Duration.zero);
      expect(port.log.last, 'result s1 1 [] {"done":true}');
      expect(local.active!.review, BrowserReview.none);
      expect([
        for (final line in local.activity) line.text,
      ], containsAllInOrder(['You allowed it once', 'Clicked']));
      // Nothing here ever sends a decision to the helper.
      expect(broker.log.where((line) => line.contains('decision')), isEmpty);
      await local.stopSession();
    });

    test('commands: relayed in order with the lease, one at a time', () async {
      final (local, broker, port) = await _started();
      await local.allow(_session(), local.tabs[7]!, agentName: 'June');
      final lease = local.active!.lease;
      await local.command(_command(lease: 'not it'));
      expect(port.log.last, 'result s1 1 [wrong_device] ');
      await local.command(_command(lease: lease));
      expect(
        broker.log.last,
        'send 7 {"id":1,"session":"s1","sequence":1,'
        '"cmd":{"op":"read","limit":4096}}',
      );
      // Another before it's answered.
      await local.command(_command(lease: lease, id: 2, sequence: 2));
      expect(port.log.last, 'result s1 2 [interrupted] ');
      broker.tell(
        const HelperMessage(7, {
          'type': 'result',
          'tab': 3,
          'id': 1,
          'result': {'text': 'Hello'},
        }),
      );
      await Future<void>.delayed(Duration.zero);
      expect(port.log.last, 'result s1 1 [] {"text":"Hello"}');
      expect(local.activity.last.text, 'Read page');
      // An old sequence again.
      await local.command(_command(lease: lease, id: 3));
      expect(port.log.last, 'result s1 3 [interrupted] ');
      await local.stopSession();
    });

    test('Stop ends it here first, then tells the backend', () async {
      final (local, broker, port) = await _started();
      await local.allow(_session(), local.tabs[7]!, agentName: 'June');
      await local.stopSession();
      expect(broker.log.last, 'end 7 stop');
      expect(port.log.last, 'stop s1 stopped');
      expect(local.active, isNull);
      expect(local.activity.last.text, 'Ended: you stopped it.');
      // A command after it is refused.
      await local.command(_command(lease: 'x', id: 9, sequence: 9));
      expect(port.log.last, 'result s1 9 [interrupted] ');
    });

    test('the tab gone ends it; the backend ending it ends it here', () async {
      final (local, broker, port) = await _started();
      await local.allow(_session(), local.tabs[7]!, agentName: 'June');
      broker.tell(const HelperGone(7, 'closed', '/private/path'));
      await Future<void>.delayed(Duration.zero);
      expect(port.log, contains('stop s1 chrome_closed'));
      // No tab left: not ready any more.
      expect(port.log.last, 'capability false bi');
      // The detail never leaves the Mac.
      expect(port.log.join(), isNot(contains('/private/path')));

      final (again, broker2, port2) = await _started();
      await again.allow(_session(), again.tabs[7]!, agentName: 'June');
      await again.ended('s1', 'time_limit');
      expect(broker2.log.last, 'end 7 stop');
      expect(port2.log.last, startsWith('grant'));
      expect(again.activity.last.text, 'Ended: it reached its 15 minutes.');
    });

    test('not starting, or something else knocking, says why', () async {
      final broker = _Broker()..startFails = 'path_too_long';
      final local = BrowserLocal(bridge: broker, backend: _Port());
      await local.start();
      expect(local.running, isFalse);
      expect(
        local.problem,
        "Browser control can't start because your Mac's user folder name is "
        'too long.',
      );
      final (ok, okBroker, _) = await _started();
      okBroker.tell(const HelperRefused('not_helper', '/some/path'));
      expect(ok.problem, browserRefusedLine);
    });
  });

  group('the card', () {
    Workspace workspace() => Workspace(
      agents: [Agent(id: 'june', name: 'June', color: 'green')],
    );

    testWidgets('waiting: who, why in its words, the site, and the tab', (
      tester,
    ) async {
      final (local, broker, port) = await _started();
      await Harness(workspace()).show(
        tester,
        home: Center(
          child: SizedBox(
            width: 480,
            child: BrowserSessionCard(session: _session(), local: local),
          ),
        ),
      );
      await settle(tester);
      expect(find.text('June wants to read a page in Chrome'), findsOneWidget);
      expect(find.text('June says: “Read the release notes”'), findsOneWidget);
      expect(find.text('Only example.com'), findsOneWidget);
      expect(find.text('123 456'), findsOneWidget);
      expect(find.text('“Release notes”'), findsOneWidget);
      await tester.tap(find.text('Allow reading'));
      await settle(tester);
      expect(port.log.last, 'grant s1 bi https://example.com');
      expect(broker.log, contains('choose 7'));
      await local.stopSession();
    });

    testWidgets('asked to use the page too: what else it may do, and Allow', (
      tester,
    ) async {
      final (local, broker, port) = await _started();
      await Harness(workspace()).show(
        tester,
        home: Center(
          child: SizedBox(
            width: 480,
            child: BrowserSessionCard(
              session: _session()..mode = 'assist',
              local: local,
            ),
          ),
        ),
      );
      await settle(tester);
      expect(find.text('June wants to use a page in Chrome'), findsOneWidget);
      expect(
        find.text(
          'June can read and scroll that tab for up to 15 minutes. It can '
          'also click, type and open pages on example.com, one action at a '
          'time, each only after you allow it on this Mac. June has no model '
          'available right now. It ends if the tab leaves example.com.',
        ),
        findsOneWidget,
      );
      expect(find.text('Allow reading'), findsNothing);
      await tester.tap(find.text('Allow'));
      await settle(tester);
      expect(port.log.last, 'grant s1 bi https://example.com');
      expect(broker.log, contains('choose 7'));
      await local.stopSession();
    });

    for (final (state, words) in [
      ('active', 'June is using example.com on a Mac'),
      ('ended', 'June used example.com'),
    ]) {
      testWidgets('$state, using it: in its words', (tester) async {
        await Harness(workspace()).show(
          tester,
          home: Center(
            child: SizedBox(
              width: 480,
              child: BrowserSessionCard(
                session: _session(state: state)
                  ..mode = 'assist'
                  ..deviceId = 'd1',
              ),
            ),
          ),
        );
        await settle(tester);
        expect(find.text(words), findsOneWidget);
      });
    }

    testWidgets('on this Mac before Chrome is set up: whose, and Set up', (
      tester,
    ) async {
      final broker = _Broker()..isRegistered = false;
      final local = BrowserLocal(
        bridge: broker,
        backend: _Port(),
        claktory: 'Studio',
      );
      await local.start();
      await Harness(workspace()).show(
        tester,
        home: Center(
          child: SizedBox(
            width: 480,
            child: BrowserSessionCard(session: _session(), local: local),
          ),
        ),
      );
      await settle(tester);
      expect(
        find.text('June in Studio wants to read a page in Chrome'),
        findsOneWidget,
      );
      await tester.tap(find.text('Set up Chrome on this Mac'));
      await settle(tester);
      expect(broker.log, ['start', 'register']);
      expect(find.text('Set up Chrome on this Mac'), findsNothing);
      expect(find.textContaining('click the Claktory button'), findsOneWidget);
    });

    testWidgets('running here, an action under review: Show panel brings '
        'the panel forward, nothing more', (tester) async {
      final (local, broker, _) = await _started();
      await local.allow(_session(), local.tabs[7]!, agentName: 'June');
      await local.command(
        _command(
          lease: local.active!.lease,
          op: '{"op":"click","document":"d1","revision":1,"target":"r1"}',
        ),
      );
      broker.tell(
        const HelperMessage(7, {'type': 'review', 'tab': 3, 'id': 1}),
      );
      await Harness(workspace()).show(
        tester,
        home: Center(
          child: SizedBox(
            width: 480,
            child: BrowserSessionCard(
              session: _session(state: 'active'),
              local: local,
            ),
          ),
        ),
      );
      await settle(tester);
      expect(
        find.text('Review it in the Claktory panel on this Mac'),
        findsOneWidget,
      );
      await tester.tap(find.text('Show panel'));
      await settle(tester);
      expect(broker.log.last, 'showReview 7');
      broker.tell(const HelperDecided(7, 1, allow: true));
      await settle(tester);
      expect(
        find.text('Go back to the tab in Chrome within 10 seconds to finish.'),
        findsOneWidget,
      );
      await local.stopSession();
    });

    testWidgets('elsewhere: no tab to choose here; Decline asks the backend', (
      tester,
    ) async {
      final harness = Harness(workspace());
      await harness.show(
        tester,
        home: Center(
          child: SizedBox(
            width: 480,
            child: BrowserSessionCard(session: _session()),
          ),
        ),
      );
      await settle(tester);
      expect(
        find.text('Choose the tab on the Mac where Chrome is open.'),
        findsOneWidget,
      );
      await tester.tap(find.text('Decline'));
      await settle(tester);
      expect(harness.last.browserStop.sessionId, 's1');
    });

    testWidgets('running elsewhere: what it is and Stop', (tester) async {
      final harness = Harness(workspace());
      await harness.show(
        tester,
        home: Center(
          child: SizedBox(
            width: 480,
            child: BrowserSessionCard(
              session: _session(state: 'active')
                ..commandsUsed = 3
                ..deviceId = 'd1',
            ),
          ),
        ),
      );
      await settle(tester);
      expect(find.text('June is reading example.com on a Mac'), findsOneWidget);
      expect(find.textContaining('3 of 100 actions'), findsOneWidget);
      await tester.tap(find.text('Stop'));
      await settle(tester);
      expect(harness.last.browserStop.reason, 'stopped');
    });

    testWidgets('ended: why, in the app’s words', (tester) async {
      await Harness(workspace()).show(
        tester,
        home: Center(
          child: SizedBox(
            width: 480,
            child: BrowserSessionCard(
              session: _session(state: 'ended')..reason = 'origin_changed',
            ),
          ),
        ),
      );
      await settle(tester);
      expect(find.text('Ended: the tab left the site.'), findsOneWidget);
    });
  });

  test(
    'it runs while browser control is on for an agent, and relays',
    () async {
      Workspace workspace({required bool on}) => Workspace(
        agents: [
          Agent(
            id: 'june',
            name: 'June',
            extensions: [AgentExtension(id: 'browser_control', enabled: on)],
          ),
        ],
      );
      final harness = Harness(workspace(on: false));
      final broker = _Broker();
      final port = _Port();
      final local = BrowserLocal(bridge: broker, backend: port);
      final messages = StreamController<ServerMessage>.broadcast(sync: true);
      final control = BrowserControl(
        harness.backend,
        local,
        messages: messages.stream,
      );
      await Future<void>.delayed(Duration.zero);
      expect(broker.log, isEmpty);
      harness.backend.workspace.apply(workspace(on: true));
      await Future<void>.delayed(Duration.zero);
      expect(broker.log, ['start']);
      // The backend ending a session that isn't running here does nothing.
      messages.add(
        ServerMessage(
          browserEnded: BrowserEnded(sessionId: 'x', reason: 'idle'),
        ),
      );
      // A command with no session here is refused.
      messages.add(ServerMessage(browserCommand: _command(lease: 'l')));
      await Future<void>.delayed(Duration.zero);
      expect(port.log.last, 'result s1 1 [interrupted] ');
      harness.backend.workspace.apply(workspace(on: false));
      await Future<void>.delayed(Duration.zero);
      expect(broker.log.last, 'stop');
      control.dispose();
      await messages.close();
    },
  );

  test('Chrome learns where the helper is only by the person setting it '
      'up, and forgets it when browser control is turned off', () async {
    final broker = _Broker()..isRegistered = false;
    final local = BrowserLocal(bridge: broker, backend: _Port());
    await local.start();
    expect(broker.log, ['start']);
    expect(local.chromeSetUp, isFalse);
    expect(await local.setUpChrome(), isNull);
    expect(local.chromeSetUp, isTrue);
    await local.turnOff();
    expect(broker.log, ['start', 'register', 'stop', 'unregister']);
    final other = _Broker()
      ..isRegistered = false
      ..registerFails = 'the file belongs to another program: /Users/x';
    final refused = BrowserLocal(bridge: other, backend: _Port());
    await refused.start();
    final failed = await refused.setUpChrome();
    expect(failed?.otherProgram, isTrue);
    expect(refused.problem, contains('another app is using'));
    expect(refused.problem, isNot(contains('/Users')));
  });

  group('which Claktory this Mac serves', () {
    Workspace on() => Workspace(
      agents: [
        Agent(
          id: 'june',
          name: 'June',
          extensions: [AgentExtension(id: 'browser_control', enabled: true)],
        ),
      ],
    );

    test('a paired Claktory that isn\'t the person\'s gets no broker and no '
        'manifest; one their account lists does', () async {
      final paired = Harness(on()).backend;
      final broker = _Broker()..isRegistered = false;
      final changes = ChangeNotifier();
      final account = ChangeNotifier();
      var listed = <AccountClaktory>[];
      final host = BrowserHost(
        candidates: () => [
          (backend: paired, endpointId: 'E2', name: 'Peer-chosen name'),
        ],
        changes: changes,
        account: account,
        theirs: () async => listed,
        bridge: broker,
      );
      await Future<void>.delayed(Duration.zero);
      expect(host.localFor(paired), isNull);
      expect(broker.log, isEmpty);
      listed = [
        const AccountClaktory(id: 'c2', name: 'Studio', endpointId: 'e2'),
      ];
      account.notifyListeners();
      await Future<void>.delayed(Duration.zero);
      final local = host.localFor(paired);
      expect(local, isNotNull);
      expect(local!.claktory, 'Studio');
      expect(broker.log, ['start']);
      host.dispose();
    });

    test(
      "this Mac's own is served, and writes no manifest by itself",
      () async {
        final own = Harness(on()).backend;
        final broker = _Broker()..isRegistered = false;
        final host = BrowserHost(
          candidates: () => [
            (backend: own, endpointId: null, name: 'Matt’s Mac'),
          ],
          changes: ChangeNotifier(),
          account: ChangeNotifier(),
          theirs: () => null,
          bridge: broker,
        );
        await Future<void>.delayed(Duration.zero);
        expect(host.localFor(own)?.claktory, 'Matt’s Mac');
        expect(broker.log, ['start']);
        host.dispose();
      },
    );
  });

  group('mounted', () {
    Workspace workspace(List<BrowserSession> sessions) => Workspace(
      agents: [
        Agent(id: 'june', name: 'June', color: 'green'),
        Agent(id: 'mike', name: 'Mike', color: 'blue'),
      ],
      browserSessions: sessions,
    );
    final now = DateTime.now().millisecondsSinceEpoch;

    test('Needs you holds a session only while it waits for a tab', () {
      List<String> keys(List<BrowserSession> sessions) => [
        for (final item in needsYouItems(workspace(sessions))) item.key,
      ];
      expect(keys([]), isEmpty);
      expect(keys([_session()]), ['browser:s1']);
      expect(keys([_session(state: 'active')]), isEmpty);
      expect(keys([_session(state: 'ended')]), isEmpty);
    });

    testWidgets('Needs you opens to the card itself', (tester) async {
      await Harness(workspace([_session()]))
          .show(tester, home: const NeedsYouList());
      await settle(tester);
      expect(find.text('Allow reading example.com in Chrome?'), findsOneWidget);
      await tester.tap(find.text('Allow reading example.com in Chrome?'));
      await settle(tester);
      expect(find.byType(BrowserSessionCard), findsOneWidget);
      expect(find.text('June says: “Read the release notes”'), findsOneWidget);
    });

    testWidgets('Needs you says when it would use the page too', (
      tester,
    ) async {
      await Harness(workspace([_session()..mode = 'assist']))
          .show(tester, home: const NeedsYouList());
      await settle(tester);
      expect(
        find.text('Allow reading and using example.com in Chrome?'),
        findsOneWidget,
      );
    });

    testWidgets("the conversation shows its agent's sessions, waiting, "
        'running and just ended, and nothing otherwise', (tester) async {
      final harness = Harness(workspace([]));
      await harness.show(
        tester,
        home: const Center(child: BrowserSessionStrip(agentId: 'june')),
      );
      await settle(tester);
      expect(find.byType(BrowserSessionCard), findsNothing);
      harness.backend.workspace.apply(
        workspace([
          _session(state: 'active')..deviceId = 'd1',
          _session()
            ..id = 's2'
            ..agentId = 'mike',
        ]),
      );
      await settle(tester);
      expect(find.byType(BrowserSessionCard), findsOneWidget);
      expect(find.text('June is reading example.com on a Mac'), findsOneWidget);
      harness.backend.workspace.apply(
        workspace([
          _session(state: 'ended')
            ..reason = 'tab_closed'
            ..endedMs = Int64(now - 60000),
        ]),
      );
      await settle(tester);
      expect(find.byType(BrowserSessionCard), findsOneWidget);
      harness.backend.workspace.apply(
        workspace([
          _session(state: 'ended')
            ..reason = 'tab_closed'
            ..endedMs = Int64(now - 11 * 60000),
        ]),
      );
      await settle(tester);
      expect(find.byType(BrowserSessionCard), findsNothing);
    });
  });
}
