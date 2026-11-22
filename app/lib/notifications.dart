import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'backend.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'rail.dart' show Claktories, Claktory;
import 'scope.dart' show AppPage;

/// Notifications on a Mac (#273 phase A): what each Claktory's backend
/// says the person should hear of (Notify) shows as the system's
/// notification while the person isn't using that Claktory here, and a
/// click on one brings the window forward at what it's about. Every rule
/// (which kinds, the agent's Notify me, quiet hours, the hourly cap, the
/// person using Claktory elsewhere) is the backend's; the app only says
/// whether the person is using it (SetActive).

/// A notification for the system to show.
@immutable
class PostedNotification {
  const PostedNotification({
    required this.id,
    required this.title,
    required this.body,
    required this.thread,
    required this.urgent,
    required this.target,
    this.deliveryId = '',
    this.kind = NotifyKind.NOTIFY_KIND_UNSPECIFIED,
  });

  /// Its identifier: one with the same replaces it.
  final String id;

  /// This delivery's own id (Notify.id), the same however it reached the
  /// device (this connection or a push), so a device showing both drops
  /// the second; [id] is what replaces an earlier one.
  final String deliveryId;

  /// What it's about (a channel on Android).
  final NotifyKind kind;
  final String title;
  final String body;

  /// What it's grouped with.
  final String thread;

  /// Shown as time-sensitive.
  final bool urgent;

  /// What a click opens.
  final NotificationTarget target;
}

/// What a click on a notification opens: a Claktory, and in it an agent
/// (at its Needs-you item), a pipe (at its held message) or a chat; none
/// of them, just that Claktory.
@immutable
class NotificationTarget {
  const NotificationTarget({
    required this.claktory,
    this.agentId = '',
    this.pipeId = '',
    this.chatId = '',
    this.seq = 0,
  });

  /// [Claktory] by [claktoryId].
  final String claktory;
  final String agentId;
  final String pipeId;
  final String chatId;
  final int seq;

  Map<String, Object> toMap() => {
    'claktory': claktory,
    'agentId': agentId,
    'pipeId': pipeId,
    'chatId': chatId,
    'seq': seq,
  };

  static NotificationTarget? fromMap(Object? map) {
    if (map is! Map) return null;
    String text(String key) => map[key] is String ? map[key] as String : '';
    final seq = map['seq'];
    return NotificationTarget(
      claktory: text('claktory'),
      agentId: text('agentId'),
      pipeId: text('pipeId'),
      chatId: text('chatId'),
      seq: seq is int ? seq : 0,
    );
  }
}

/// The system's notifications on this device, and whether the person is
/// using the app: a Mac's (Notifications.swift), a fake in tests.
abstract interface class SystemNotifications {
  /// The app's window is in front and in use: the app active, its window
  /// key and on screen, the screen awake and unlocked.
  ValueListenable<bool> get using;

  /// Shows [notification]; the system asks the person first, once.
  Future<void> post(PostedNotification notification);

  /// What the person clicked, as it's clicked (also one that opened the
  /// app).
  Stream<NotificationTarget> get clicked;

  /// Starts: the state now, and clicks from now on.
  Future<void> start();

  void dispose();
}

/// A Claktory's id in a notification: never its pairing code, which holds
/// its token and would be kept with the system's notifications. This
/// Mac's own is "local"; another, its tunnel's public key, or where it is.
String claktoryId(Claktory claktory) {
  final target = claktory.target;
  if (target == null) return 'local';
  if (target.endpointId.isNotEmpty) return target.endpointId;
  return '${target.host}:${target.port}';
}

/// Shows each Claktory's notifications while the person isn't using it,
/// and tells each backend whether they are.
class ClaktoryNotifications {
  ClaktoryNotifications({required this.claktories, required this.system}) {
    claktories.addListener(_sync);
    system.using.addListener(_sayActive);
    _clicks = system.clicked.listen(open);
    _sync();
    unawaited(system.start());
  }

  final Claktories claktories;
  final SystemNotifications system;

  late final StreamSubscription<NotificationTarget> _clicks;
  final _listening = <Claktory, StreamSubscription<Notify>>{};

  void _sync() {
    final open = claktories.all.toSet();
    for (final gone in [
      for (final claktory in _listening.keys)
        if (!open.contains(claktory)) claktory,
    ]) {
      unawaited(_listening.remove(gone)!.cancel());
    }
    for (final claktory in open) {
      _listening[claktory] ??= claktory.backend.notified.listen(
        (notify) => _show(claktory, notify),
      );
    }
    _sayActive();
  }

  /// Only the Claktory the window shows is in use, and only while the
  /// window is.
  void _sayActive() {
    final shown = claktories.selected;
    for (final claktory in claktories.all) {
      claktory.backend.setActive(
        system.using.value && identical(claktory, shown),
      );
    }
  }

  void _show(Claktory claktory, Notify notify) {
    // In front, at this very Claktory: the window says it already. A test
    // the person just sent from Settings shows all the same.
    if (notify.kind != NotifyKind.NOTIFY_KIND_TEST &&
        system.using.value &&
        identical(claktory, claktories.selected)) {
      return;
    }
    final id = claktoryId(claktory);
    unawaited(
      system.post(
        PostedNotification(
          id: '$id:${notify.collapse.isNotEmpty ? notify.collapse : notify.id}',
          title: notify.title.isEmpty ? 'Claktory' : notify.title,
          body: notify.body,
          thread: notify.thread.isEmpty ? id : '$id:${notify.thread}',
          urgent: notify.urgent,
          deliveryId: notify.id,
          kind: notify.kind,
          target: NotificationTarget(
            claktory: id,
            agentId: notify.agentId,
            pipeId: notify.pipeId,
            chatId: notify.chatId,
            seq: notify.seq.toInt(),
          ),
        ),
      ),
    );
  }

  /// A click: its Claktory in the window, at what it's about.
  void open(NotificationTarget target) {
    final claktory = claktories.all
        .where((each) => claktoryId(each) == target.claktory)
        .firstOrNull;
    // One removed since: the window comes forward as it is.
    if (claktory == null) return;
    claktories.select(claktory);
    final backend = claktory.backend;
    if (target.agentId.isEmpty &&
        target.pipeId.isEmpty &&
        target.chatId.isEmpty) {
      return;
    }
    claktory.page.value = AppPage.agent;
    fireAndLog(_openIn(backend, target));
  }

  Future<void> _openIn(Backend backend, NotificationTarget target) async {
    final workspace = backend.workspace.workspace;
    if (target.chatId.isNotEmpty) {
      await backend.selectChat(target.chatId);
    } else if (target.agentId.isNotEmpty) {
      final agent = workspace.agents
          .where((agent) => agent.id == target.agentId)
          .firstOrNull;
      // A pipe's liaison's conversation shows in its pipe.
      if (agent != null && agent.pipeId.isNotEmpty) {
        await backend.selectPipe(agent.pipeId);
      } else {
        await backend.selectAgent(target.agentId);
      }
      if (target.seq > 0) backend.reveal.reachSeq(target.agentId, target.seq);
    } else if (target.pipeId.isNotEmpty) {
      await backend.selectPipe(target.pipeId);
    }
  }

  void dispose() {
    claktories.removeListener(_sync);
    system.using.removeListener(_sayActive);
    unawaited(_clicks.cancel());
    for (final listening in _listening.values) {
      unawaited(listening.cancel());
    }
    _listening.clear();
    system.dispose();
  }
}

/// A Mac's notifications, through factory/notifications
/// (Notifications.swift): "listen" answers whether the person is using the
/// app and starts the calls back, "using" (a bool) as it changes and
/// "clicked" (a target's map) for each click; "post" shows one.
class MacNotifications implements SystemNotifications {
  MacNotifications({
    this.channel = const MethodChannel('factory/notifications'),
  });

  final MethodChannel channel;

  @override
  final ValueNotifier<bool> using = ValueNotifier(false);

  final _clicked = StreamController<NotificationTarget>.broadcast();

  @override
  Stream<NotificationTarget> get clicked => _clicked.stream;

  @override
  Future<void> start() async {
    channel.setMethodCallHandler((call) async {
      switch (call.method) {
        case 'using':
          using.value = call.arguments == true;
        case 'clicked':
          if (NotificationTarget.fromMap(call.arguments) case final target?) {
            _clicked.add(target);
          }
      }
    });
    try {
      using.value = await channel.invokeMethod<bool>('listen') ?? false;
    } on MissingPluginException {
      // No native side (tests, another platform): never in use, nothing
      // shown.
    }
  }

  @override
  Future<void> post(PostedNotification notification) async {
    try {
      await channel.invokeMethod<void>('post', {
        'id': notification.id,
        'title': notification.title,
        'body': notification.body,
        'thread': notification.thread,
        'urgent': notification.urgent,
        'target': notification.target.toMap(),
      });
    } on MissingPluginException {
      // Nothing to show it with.
    } on PlatformException {
      // Not allowed, or the system refused it: nothing more to do here.
    }
  }

  @override
  void dispose() {
    channel.setMethodCallHandler(null);
    unawaited(_clicked.close());
    using.dispose();
  }
}
