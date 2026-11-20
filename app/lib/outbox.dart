import 'dart:async';
import 'dart:math';

import 'package:flutter/foundation.dart';

import 'backend.dart' show RequestFailure;

/// Sends one of the person's messages, tagged with [clientId]: completes
/// when the backend has it, throws [RequestFailure] when it doesn't.
typedef MessageSender = Future<void> Function(
  String agentId,
  String text,
  String clientId,
);

/// One of the person's messages on its way: shown in the conversation at
/// once, until the backend's own (queued, or in the transcript) takes its
/// place.
class Outgoing {
  Outgoing._(this.agentId, this.text, this.clientId);

  final String agentId;
  final String text;

  /// Made up here for this send, and carried by what it becomes, so the
  /// two are matched without guessing.
  final String clientId;

  /// Why it wasn't sent; null while it's on its way.
  String? failure;

  bool get failed => failure != null;
}

/// The person's messages on their way to the backend, per agent, in the
/// order sent. Kept only in memory, for as long as a send takes: nothing
/// here is the backend's state, and a message sent leaves at once.
class Outbox extends ChangeNotifier {
  Outbox(this._send);

  final MessageSender _send;

  final _messages = <Outgoing>[];
  final _random = Random.secure();

  /// [agentId]'s, oldest first.
  List<Outgoing> of(String agentId) => [
    for (final message in _messages)
      if (message.agentId == agentId) message,
  ];

  /// Sends [text] to [agentId], showing it at once.
  Outgoing send(String agentId, String text) {
    final message = Outgoing._(agentId, text, _newId());
    _messages.add(message);
    notifyListeners();
    unawaited(_try(message));
    return message;
  }

  /// Sends a message that wasn't sent again.
  void retry(Outgoing message) {
    if (!_messages.contains(message) || !message.failed) return;
    message.failure = null;
    notifyListeners();
    unawaited(_try(message));
  }

  /// The backend's own version of [message] arrived and takes its place,
  /// or the person took it back to edit: it's no longer shown.
  void remove(Outgoing message) {
    if (_messages.remove(message)) notifyListeners();
  }

  Future<void> _try(Outgoing message) async {
    try {
      await _send(message.agentId, message.text, message.clientId);
    } on RequestFailure catch (failure) {
      if (!_messages.contains(message)) return;
      message.failure = failure.message;
      notifyListeners();
      return;
    }
    // The backend has it, and sends what it became (queued, or in the
    // transcript) before it answers, so that has taken its place by now
    // where it shows; anywhere else (another agent selected), it just goes.
    remove(message);
  }

  String _newId() =>
      List.generate(16, (_) => _random.nextInt(16).toRadixString(16)).join();
}
