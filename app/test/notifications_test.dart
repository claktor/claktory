import 'dart:async';

import 'package:factory_app/connections.dart';
import 'package:factory_app/drafts.dart';
import 'package:factory_app/notifications.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final _nixos = 'factory://?t=${'a' * 64}&name=nixos&e=${'c' * 64}';

/// A Claktory's backend whose Notify messages the test sends, and which
/// records what it says of the person using it.
class _Backend extends RecordingBackend {
  _Backend() : super(localBackend: true);

  final notifies = StreamController<Notify>.broadcast(sync: true);
  final said = <bool>[];

  @override
  Stream<Notify> get notified => notifies.stream;

  @override
  Future<void> sendActive(bool active) async => said.add(active);
}

/// The system's notifications as the test says: whether the person uses
/// the app, what was posted, and clicks.
class _System implements SystemNotifications {
  @override
  final ValueNotifier<bool> using = ValueNotifier(false);
  final posted = <PostedNotification>[];
  final clicks = StreamController<NotificationTarget>.broadcast(sync: true);

  @override
  Stream<NotificationTarget> get clicked => clicks.stream;

  @override
  Future<void> post(PostedNotification notification) async =>
      posted.add(notification);

  @override
  Future<void> start() async {}

  @override
  void dispose() {}
}

void main() {
  late Connections connections;
  late Claktories claktories;
  late _System system;
  late ClaktoryNotifications notifications;
  final backends = <String, _Backend>{};

  Future<void> start(WidgetTester tester) async {
    // Claktories follow the app's lifecycle, which needs the binding.
    await tester.pumpWidget(const SizedBox());
    connections = Connections(MemoryConnectionStorage());
    claktories = Claktories(
      connections: connections,
      localBackend: true,
      drafts: DeviceDrafts(MemoryDraftStorage()),
      open: (target) {
        final backend = _Backend()..connectTo(target);
        backend.workspace.apply(
          Workspace(
            agents: [
              Agent(id: 'rivet', name: 'Rivet'),
              Agent(id: 'kai', name: 'Kai', pipeId: 'pipe1'),
            ],
          ),
        );
        return backends[target?.code ?? ''] = backend;
      },
    );
    connections.use(parsePairingCode(_nixos));
    connections.use(null);
    system = _System();
    notifications = ClaktoryNotifications(
      claktories: claktories,
      system: system,
    );
    addTearDown(() {
      notifications.dispose();
      claktories.dispose();
      backends.clear();
    });
  }

  _Backend local() => backends['']!;
  _Backend nixos() => backends[_nixos]!;

  testWidgets('only the Claktory the window shows is in use, and only while '
      'the window is', (tester) async {
    await start(tester);
    expect(local().active, isFalse);
    expect(nixos().active, isFalse);

    system.using.value = true;
    expect(local().said, [true]);
    expect(nixos().said, isEmpty);

    connections.use(parsePairingCode(_nixos));
    expect(local().said, [true, false]);
    expect(nixos().said, [true]);

    system.using.value = false;
    expect(nixos().said, [true, false]);
  });

  testWidgets("a Claktory's notification shows unless the person is at it; "
      'it names the Claktory by its key, never its code', (tester) async {
    await start(tester);
    system.using.value = true;
    local().notifies.add(
      Notify(id: 'n1', title: 'Rivet', body: 'Allow a secret?'),
    );
    expect(system.posted, isEmpty, reason: 'in front, at that Claktory');
    // A test the person sent from Settings shows all the same.
    local().notifies.add(
      Notify(
        id: 't1',
        kind: NotifyKind.NOTIFY_KIND_TEST,
        title: 'Claktory',
        body: 'This is a test notification. Notifications work.',
        collapse: 'test',
      ),
    );
    expect(system.posted.single.id, 'local:test');
    system.posted.clear();

    nixos().notifies.add(
      Notify(
        id: 'n2',
        kind: NotifyKind.NOTIFY_KIND_NEEDS_YOU,
        title: 'Rivet',
        body: 'Which project?',
        agentId: 'rivet',
        seq: Int64(7),
        thread: 'agent:rivet',
        urgent: true,
      ),
    );
    final posted = system.posted.single;
    expect(posted.id, '${'c' * 64}:n2');
    expect(posted.title, 'Rivet');
    expect(posted.body, 'Which project?');
    expect(posted.thread, '${'c' * 64}:agent:rivet');
    expect(posted.urgent, isTrue);
    expect(posted.deliveryId, 'n2');
    expect(posted.kind, NotifyKind.NOTIFY_KIND_NEEDS_YOU);
    expect(posted.target.claktory, 'c' * 64);
    expect(posted.target.agentId, 'rivet');
    expect(posted.target.seq, 7);
    expect('${posted.id}${posted.thread}', isNot(contains('a' * 64)));

    // Away from the window, this Mac's too; one that replaces another
    // keeps its key.
    system.using.value = false;
    local().notifies.add(
      Notify(id: 'n3', title: 'Pericolo', body: 'Down', collapse: 'alert:p'),
    );
    expect(system.posted.last.id, 'local:alert:p');

    // A device waiting to be allowed: its click goes to Needs you.
    local().notifies.add(
      Notify(
        id: 'n4',
        kind: NotifyKind.NOTIFY_KIND_NEEDS_YOU,
        title: 'Studio',
        body: 'A device is waiting to be allowed on Studio.',
        deviceId: 'd4',
      ),
    );
    expect(system.posted.last.target.deviceId, 'd4');

    // What waited out quiet hours (#364 O3): Needs you, about no one thing.
    local().notifies.add(
      Notify(
        id: 'n5',
        kind: NotifyKind.NOTIFY_KIND_NEEDS_YOU,
        title: '2 things need you',
        body: 'They came during your quiet hours.',
        thread: 'quiet',
      ),
    );
    expect(system.posted.last.target.kind, 'needs_you');
    expect(system.posted.last.target.agentId, isEmpty);
  });

  testWidgets('a click shows its Claktory at what it is about: an agent at '
      'its item, a liaison in its pipe, a chat', (tester) async {
    await start(tester);
    final page = claktories.all.last.page..value = AppPage.settings;
    system.clicks.add(
      NotificationTarget(claktory: 'c' * 64, agentId: 'rivet', seq: 7),
    );
    await tester.pump();
    expect(claktories.selected.backend, same(nixos()));
    expect(page.value, AppPage.agent);
    expect(nixos().sent.last.selectAgent.agentId, 'rivet');

    system.clicks.add(NotificationTarget(claktory: 'local', agentId: 'kai'));
    await tester.pump();
    expect(claktories.selected.backend, same(local()));
    expect(local().sent.last.selectPipe.pipeId, 'pipe1');

    system.clicks.add(NotificationTarget(claktory: 'local', chatId: '3'));
    await tester.pump();
    expect(local().sent.last.selectChat.chatId, '3');

    // A device waiting to be allowed: Needs you, where it's allowed.
    system.clicks.add(NotificationTarget(claktory: 'c' * 64, deviceId: 'd4'));
    await tester.pump();
    expect(claktories.selected.backend, same(nixos()));
    expect(page.value, AppPage.inbox);

    // What waited out quiet hours: Needs you too (#364 O3). A Needs-you
    // click about an agent still goes to the agent.
    page.value = AppPage.settings;
    system.clicks.add(
      NotificationTarget(claktory: 'c' * 64, kind: 'needs_you'),
    );
    await tester.pump();
    expect(page.value, AppPage.inbox);
    system.clicks.add(
      NotificationTarget(
        claktory: 'c' * 64,
        agentId: 'rivet',
        kind: 'needs_you',
      ),
    );
    await tester.pump();
    expect(page.value, AppPage.agent);
    // Nothing to go to and not Needs you (a test): the window only.
    page.value = AppPage.settings;
    system.clicks.add(NotificationTarget(claktory: 'c' * 64));
    await tester.pump();
    expect(page.value, AppPage.settings);

    // One that's gone: nothing changes.
    final sent = local().sent.length;
    system.clicks.add(NotificationTarget(claktory: 'gone', agentId: 'rivet'));
    await tester.pump();
    expect(local().sent, hasLength(sent));
  });

  testWidgets("a Mac's: whether in use, posting, and clicks, through "
      'factory/notifications', (tester) async {
    const channel = MethodChannel('factory/notifications');
    final calls = <MethodCall>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(channel, (
      call,
    ) async {
      calls.add(call);
      return call.method == 'listen' ? true : null;
    });
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        channel,
        null,
      ),
    );
    final mac = MacNotifications();
    addTearDown(mac.dispose);
    final clicked = <NotificationTarget>[];
    mac.clicked.listen(clicked.add);
    await mac.start();
    expect(mac.using.value, isTrue);

    await mac.post(
      const PostedNotification(
        id: 'local:n1',
        title: 'Rivet',
        body: 'Which project?',
        thread: 'local',
        urgent: false,
        target: NotificationTarget(claktory: 'local', agentId: 'rivet'),
      ),
    );
    expect(calls.last.method, 'post');
    expect(calls.last.arguments, {
      'id': 'local:n1',
      'title': 'Rivet',
      'body': 'Which project?',
      'thread': 'local',
      'urgent': false,
      'target': {
        'claktory': 'local',
        'agentId': 'rivet',
        'pipeId': '',
        'chatId': '',
        'seq': 0,
        'deviceId': '',
        'kind': '',
      },
    });

    Future<void> native(String method, Object? arguments) =>
        tester.binding.defaultBinaryMessenger.handlePlatformMessage(
          channel.name,
          channel.codec.encodeMethodCall(MethodCall(method, arguments)),
          (_) {},
        );
    await native('using', false);
    expect(mac.using.value, isFalse);
    await native('clicked', {'claktory': 'local', 'chatId': '3', 'seq': 0});
    await tester.pump();
    expect(clicked.single.chatId, '3');
  });
}
