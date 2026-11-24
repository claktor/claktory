import 'dart:async';
import 'dart:io';
import 'dart:math';

import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';

import 'attachments.dart';
import 'backend.dart';
import 'protocol/factory/v1/factory.pb.dart' show Attachment;
import 'voice_keep.dart';

/// Whether [failure] is the connection's, not the Claktory refusing: it
/// was lost, or wasn't there. What failed so goes again once it's back.
bool connectionLost(RequestFailure failure) =>
    failure.message == 'Connection lost.' ||
    failure.message == 'Not connected to the backend.' ||
    failure.message == decisionWaiting;

/// What a decision on a card cut off by the connection fails with: the
/// backend sends it again by itself once the connection is back.
const decisionWaiting = "Not sent yet. It's sent when the connection is back.";

/// Why a message waits, said beside "Not sent yet".
const waitingForConnection =
    "Claktory isn't connected. It's sent when the connection is back.";

/// A file the backend has, for the agent: where it put it, under the name
/// it gave it.
class SentFile {
  const SentFile({
    required this.path,
    required this.name,
    required this.size,
    this.mime = '',
  });

  /// On the backend's machine, for the agent to read.
  final String path;
  final String name;
  final int size;
  final String mime;
}

/// What the outbox needs from the backend. Each call throws
/// [RequestFailure] when the backend refuses or the connection drops.
abstract interface class OutboxBackend {
  /// Sends one of the person's messages, tagged with [clientId], with the
  /// files already uploaded; with [voice], files[0] is a recording, for
  /// the Claktory to transcribe.
  Future<void> send(
    String agentId,
    String text,
    String clientId,
    List<SentFile> files, {
    bool voice = false,
  });

  /// Starts sending [file] to [agentId] as [uploadId], or picks it up where
  /// it stopped: answers how many of its bytes the backend has.
  Future<int> startUpload(String uploadId, String agentId, LocalFile file);

  /// Sends [data], from [offset]; answers how many bytes the backend has.
  Future<int> uploadChunk(String uploadId, int offset, Uint8List data);

  /// The whole file is there: answers where the backend put it.
  Future<SentFile> finishUpload(String uploadId);

  /// Drops what the backend has of [uploadId].
  Future<void> cancelUpload(String uploadId);
}

/// A file going with one of the person's messages: uploaded first, a chunk
/// at a time, then sent with the message.
class OutgoingFile {
  OutgoingFile._(this.file, this.uploadId);

  /// The file on this device.
  final LocalFile file;

  /// Made up here, so an upload cut off can be picked up again.
  final String uploadId;

  /// How many bytes the backend has.
  var sent = 0;

  /// Where the backend put it, once it's all there.
  SentFile? done;

  /// 0 to 1.
  double get progress => file.size == 0 ? 1 : sent / file.size;
}

/// One of the person's messages on its way: shown in the conversation at
/// once, until the backend's own (queued, or in the transcript) takes its
/// place.
class Outgoing {
  Outgoing._(
    this.agentId,
    this.text,
    this.clientId,
    this.files,
    this.voice, {
    this.kept,
  });

  final String agentId;
  final String text;

  /// Made up here for this send, and carried by what it becomes, so the
  /// two are matched without guessing.
  final String clientId;

  /// What goes with it, uploading first.
  final List<OutgoingFile> files;

  /// A voice message: how long its recording (files[0]) is; null for any
  /// other message.
  final Duration? voice;

  /// Why it wasn't sent; null while it's on its way.
  String? failure;

  bool get failed => failure != null;

  /// It wasn't sent for want of the connection: it goes by itself once
  /// the connection is back.
  bool waiting = false;

  /// A voice message's recording, kept on this device until the Claktory
  /// has the message (#401); null for any other message, or where the
  /// device keeps none.
  KeptVoice? kept;

  /// Its files are still uploading.
  bool get uploading => files.any((file) => file.done == null);
}

/// The person's messages on their way to the backend, per agent, in the
/// order sent. Kept in memory for as long as a send takes, nothing here
/// being the backend's state, and a message sent leaves at once; except a
/// voice message's recording, kept on the device until the Claktory has
/// the message (#401: [KeptVoices] brings it back after a relaunch). What
/// failed for want of the connection goes again by itself once it's back.
class Outbox extends ChangeNotifier {
  Outbox(this._backend, {this.connected, this.claktory, this.kind})
    : assert(claktory == null || kind != null) {
    connected?.addListener(_connectedChanged);
  }

  final OutboxBackend _backend;

  /// Whether the connection is up: what waited for it goes when it is.
  final ValueListenable<bool>? connected;

  /// The Claktory's key in the kept folder ([keptClaktoryKey]), and what
  /// its voice messages are to ([KeptKind.agent] or [KeptKind.chat]);
  /// without them, recordings aren't kept.
  final String Function()? claktory;
  final KeptKind? kind;

  final _messages = <Outgoing>[];
  final _random = Random.secure();

  /// The most sent in one chunk.
  static const chunk = 1024 * 1024;

  /// [agentId]'s, oldest first.
  List<Outgoing> of(String agentId) => [
    for (final message in _messages)
      if (message.agentId == agentId) message,
  ];

  /// Sends [text] to [agentId] with [files], showing it at once. With
  /// [voice], it's a voice message: files[0] is its recording, that long,
  /// and [text] is empty.
  Outgoing send(
    String agentId,
    String text, {
    List<LocalFile> files = const [],
    Duration? voice,
  }) {
    final message = Outgoing._(agentId, text, _newId(), [
      for (final file in files) OutgoingFile._(file, _newId()),
    ], voice);
    _messages.add(message);
    notifyListeners();
    unawaited(_keepThenTry(message));
    return message;
  }

  /// A voice message's recording is kept first (what it's for, beside it),
  /// when it was made in the kept folder; then it's sent.
  Future<void> _keepThenTry(Outgoing message) async {
    final claktory = this.claktory;
    final kind = this.kind;
    final voice = message.voice;
    if (voice != null && claktory != null && kind != null) {
      final file = message.files.first;
      final folder = await keptVoiceFolder();
      final kept = KeptVoice(
        path: file.file.path,
        duration: voice,
        claktory: claktory(),
        kind: kind,
        target: message.agentId,
        clientId: message.clientId,
        uploadId: file.uploadId,
        kept: folder != null && file.file.path.startsWith('$folder/'),
      );
      await kept.save();
      if (!_messages.contains(message)) {
        // Taken back meanwhile.
        await kept.delete();
        return;
      }
      message.kept = kept;
    }
    await _go(message);
  }

  /// A voice message kept on the device from before (the app quit or was
  /// killed before the Claktory had it): back in its conversation, and
  /// sent again with its own ids, so it's never there twice.
  void restore(KeptVoice kept) {
    if (_messages.any((message) => message.clientId == kept.clientId)) return;
    final file = File(kept.path);
    final message = Outgoing._(
      kept.target,
      '',
      kept.clientId,
      [
        OutgoingFile._(
          LocalFile(
            path: kept.path,
            name: kept.path.split('/').last,
            size: file.existsSync() ? file.lengthSync() : 0,
          ),
          kept.uploadId,
        ),
      ],
      kept.duration,
      kept: kept,
    );
    _messages.add(message);
    notifyListeners();
    unawaited(_go(message));
  }

  /// Sends [message] now if connected; otherwise it waits for the
  /// connection.
  Future<void> _go(Outgoing message) async {
    final connected = this.connected;
    if (connected != null && !connected.value) {
      _wait(message);
      return;
    }
    await _try(message);
  }

  void _wait(Outgoing message) {
    message
      ..failure = waitingForConnection
      ..waiting = true;
    notifyListeners();
  }

  /// The connection is back: what waited for it goes, in the order sent.
  void _connectedChanged() {
    if (connected?.value != true) return;
    for (final message in [..._messages]) {
      if (message.waiting) retry(message);
    }
  }

  /// Sends a message that wasn't sent again, its files from where they
  /// stopped.
  void retry(Outgoing message) {
    if (!_messages.contains(message) || !message.failed) return;
    message
      ..failure = null
      ..waiting = false;
    notifyListeners();
    unawaited(_try(message));
  }

  /// The backend's own version of [message] arrived and takes its place: it
  /// leaves the outbox.
  void remove(Outgoing message) {
    if (!_messages.remove(message)) return;
    notifyListeners();
    // The Claktory has it: its recording needn't be kept any more.
    if (message.kept case final kept?) unawaited(kept.delete());
  }

  /// The person takes [message], not sent, back to change it, or deletes
  /// it (a voice message): what the backend has of its files is dropped.
  void takeBack(Outgoing message) {
    remove(message);
    for (final file in message.files) {
      // A voice message's may be there from before a relaunch, finished
      // or not.
      if (file.sent > 0 || message.voice != null) {
        unawaited(
          _backend
              .cancelUpload(file.uploadId)
              .catchError((Object _) {}, test: (e) => e is RequestFailure),
        );
      }
    }
  }

  /// Sends [message]; [again] when it's tried again at once, the
  /// connection back by the time it failed.
  Future<void> _try(Outgoing message, {bool again = false}) async {
    try {
      for (final file in message.files) {
        if (file.done == null) await _upload(message, file);
        if (!_messages.contains(message)) return;
      }
      await _backend.send(message.agentId, message.text, message.clientId, [
        for (final file in message.files) file.done!,
      ], voice: message.voice != null);
    } on RequestFailure catch (failure) {
      if (!_messages.contains(message)) return;
      if (connectionLost(failure) && connected != null) {
        // The connection may be back already (it dropped and came back
        // while this was under way): then it goes again now, once.
        if (connected!.value && !again) return _try(message, again: true);
        _wait(message);
        return;
      }
      message.failure = failure.message;
      notifyListeners();
      return;
    } on FileSystemException catch (error) {
      if (!_messages.contains(message)) return;
      final name = error.path?.split(Platform.pathSeparator).last;
      message.failure =
          "Couldn't read ${name ?? 'a file'}. It may have been moved or "
          'deleted.';
      notifyListeners();
      return;
    } on Object catch (error) {
      // Anything else: it mustn't say "Sending…" for ever.
      if (!_messages.contains(message)) return;
      message.failure = '$error';
      notifyListeners();
      return;
    }
    // The backend has it, and sends what it became (queued, or in the
    // transcript) before it answers, so that has taken its place by now
    // where it shows; anywhere else (another agent selected), it just goes.
    remove(message);
  }

  /// Uploads [file] from where the backend says it has got to.
  Future<void> _upload(Outgoing message, OutgoingFile file) async {
    file.sent = await _backend.startUpload(
      file.uploadId,
      message.agentId,
      file.file,
    );
    notifyListeners();
    final source = await File(file.file.path).open();
    try {
      while (file.sent < file.file.size) {
        if (!_messages.contains(message)) return;
        await source.setPosition(file.sent);
        final data = await source.read(min(chunk, file.file.size - file.sent));
        if (data.isEmpty) {
          throw FileSystemException('It got shorter', file.file.path);
        }
        file.sent = await _backend.uploadChunk(file.uploadId, file.sent, data);
        notifyListeners();
      }
    } finally {
      await source.close();
    }
    file.done = await _backend.finishUpload(file.uploadId);
    notifyListeners();
  }

  String _newId() =>
      List.generate(16, (_) => _random.nextInt(16).toRadixString(16)).join();

  @override
  void dispose() {
    connected?.removeListener(_connectedChanged);
    super.dispose();
  }
}

/// The outbox's way to the backend.
class BackendOutbox implements OutboxBackend {
  BackendOutbox(this._backend);

  final Backend _backend;

  @override
  Future<void> send(
    String agentId,
    String text,
    String clientId,
    List<SentFile> files, {
    bool voice = false,
  }) => _backend.sendMessage(
    agentId,
    text,
    clientId: clientId,
    voice: voice,
    attachments: [
      for (final file in files)
        Attachment(
          path: file.path,
          name: file.name,
          size: Int64(file.size),
          mime: file.mime,
        ),
    ],
  );

  @override
  Future<int> startUpload(String uploadId, String agentId, LocalFile file) =>
      _backend.startUpload(
        uploadId,
        agentId,
        name: file.name,
        size: file.size,
        mime: mimeOf(file.name),
      );

  @override
  Future<int> uploadChunk(String uploadId, int offset, Uint8List data) =>
      _backend.uploadChunk(uploadId, offset, data);

  @override
  Future<SentFile> finishUpload(String uploadId) async {
    final file = await _backend.finishUpload(uploadId);
    return SentFile(
      path: file.path,
      name: file.name,
      size: file.size.toInt(),
      mime: file.mime,
    );
  }

  @override
  Future<void> cancelUpload(String uploadId) => _backend.cancelUpload(uploadId);
}
