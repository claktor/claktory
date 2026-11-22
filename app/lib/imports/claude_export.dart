import 'dart:convert';
import 'dart:typed_data';

import 'package:crypto/crypto.dart';

import 'zip.dart';

// Reading a Claude export, on this device only.
//
// NOT VERIFIED: Anthropic doesn't document its export's layout, and no
// real export has been checked here yet. This reads the layout commonly
// described (a ZIP, or the JSON alone, holding `conversations.json`: a
// list of conversations, each `uuid`, `name`, `created_at`, `account`,
// `chat_messages` of `sender`/`text`/`created_at`) as a provisional
// adapter. Anything else is "not a Claude export", never guessed at.
// Check it against a sanitized real export before claiming it reads one.

/// The parser's version, kept with what it read.
const claudeExportParser = 'claude-export/0 (provisional)';

/// How much an export may be: the file as chosen, and what's read in it.
class ExportLimits {
  const ExportLimits({
    this.maxInput = 100 * 1024 * 1024,
    this.archive = const ArchiveLimits(),
    this.maxDepth = 64,
    this.maxConversations = 20000,
    this.maxMessages = 100000,
  });

  /// The file chosen.
  final int maxInput;
  final ArchiveLimits archive;

  /// How deeply its JSON may nest.
  final int maxDepth;
  final int maxConversations;

  /// Messages in all its conversations together.
  final int maxMessages;
}

/// Why an export wasn't read.
enum ExportProblem {
  /// Bigger than [ExportLimits.maxInput].
  tooBig,

  /// Not something this reads as a Claude export.
  unsupported,

  /// An archive it won't open: unsafe paths, links, nesting, a bomb.
  unsafe,

  /// It looks like several people's conversations (an organization's).
  severalPeople,
}

class ExportFailure implements Exception {
  const ExportFailure(this.problem, [this.detail = '']);

  final ExportProblem problem;

  /// For the log: what exactly.
  final String detail;

  @override
  String toString() => 'ExportFailure(${problem.name}): $detail';
}

/// One conversation in an export, as its preview shows it.
class ConversationPreview {
  const ConversationPreview({
    required this.id,
    required this.title,
    required this.createdMs,
    required this.fromYou,
    required this.fromClaude,
    required this.snippet,
  });

  /// Its id in the export; "#" and its place in the file when it had
  /// none (or one another had).
  final String id;

  /// Its name, or empty.
  final String title;

  /// When it began; 0 when the export doesn't say.
  final int createdMs;

  /// Its messages from the person, and from Claude.
  final int fromYou;
  final int fromClaude;

  /// Its first message from the person, cut short.
  final String snippet;

  int get messages => fromYou + fromClaude;
}

/// One of a conversation's messages, whole: [role] "user" (the person)
/// or "assistant" (Claude).
class ImportedMessage {
  const ImportedMessage({
    required this.role,
    required this.text,
    required this.createdMs,
  });

  final String role;
  final String text;

  /// 0 when the export doesn't say.
  final int createdMs;
}

/// A conversation the person chose, whole: its words, never its files.
class ImportedConversation {
  const ImportedConversation({
    required this.id,
    required this.title,
    required this.createdMs,
    required this.messages,
    this.leftOutFiles = 0,
  });

  final String id;
  final String title;
  final int createdMs;
  final List<ImportedMessage> messages;

  /// Files attached to its messages, which don't come in.
  final int leftOutFiles;
}

/// What an export holds, and what was left out of it.
class ExportPreview {
  const ExportPreview({
    required this.conversations,
    required this.sha256,
    this.account = false,
    this.voice = 0,
    this.other = 0,
  });

  final List<ConversationPreview> conversations;

  /// The chosen file's SHA-256, hex.
  final String sha256;

  /// It held the account's own details (left out).
  final bool account;

  /// Voice recordings in it (left out).
  final int voice;

  /// Files in it this doesn't read (left out).
  final int other;
}

const _voice = ['.mp3', '.m4a', '.wav', '.webm', '.ogg', '.aac', '.opus'];
const _accountFiles = ['users.json', 'user.json', 'account.json'];

/// Reads [bytes], a file the person chose, as a Claude export; throws
/// [ExportFailure]. [checkpoint] is called as it goes, to stop it.
ExportPreview readClaudeExport(
  Uint8List bytes, {
  ExportLimits limits = const ExportLimits(),
  void Function()? checkpoint,
}) {
  final (json, hash, account, voice, other) = _open(bytes, limits, checkpoint);
  return ExportPreview(
    conversations: [
      for (final item in _items(json, limits, checkpoint)) item.preview,
    ]..sort((a, b) => b.createdMs.compareTo(a.createdMs)),
    sha256: hash,
    account: account,
    voice: voice,
    other: other,
  );
}

/// The conversations [ids] (ConversationPreview.id) of the export
/// [bytes], whole, in the order of [ids]: what the person chose from its
/// preview. Throws [ExportFailure]: [ExportProblem.unsupported] when the
/// file isn't the one previewed ([sha256]) or holds none of them.
List<ImportedConversation> readSelected(
  Uint8List bytes, {
  required String sha256,
  required List<String> ids,
  ExportLimits limits = const ExportLimits(),
  void Function()? checkpoint,
}) {
  final (json, hash, _, _, _) = _open(bytes, limits, checkpoint);
  if (hash != sha256) {
    throw const ExportFailure(
      ExportProblem.unsupported,
      'not the file previewed',
    );
  }
  final byId = {
    for (final item in _items(json, limits, checkpoint))
      item.preview.id: item.whole,
  };
  return [
    for (final id in ids)
      byId[id] ??
          (throw ExportFailure(
            ExportProblem.unsupported,
            'no conversation $id',
          )),
  ];
}

/// [bytes]' conversations JSON, the file's SHA-256, and what else it
/// held: account details, voice recordings, other files.
(Uint8List, String, bool, int, int) _open(
  Uint8List bytes,
  ExportLimits limits,
  void Function()? checkpoint,
) {
  if (bytes.length > limits.maxInput) {
    throw const ExportFailure(ExportProblem.tooBig);
  }
  final hash = sha256.convert(bytes).toString();
  if (!BoundedZip.looksLikeZip(bytes)) {
    // The conversations' JSON on its own.
    return (bytes, hash, false, 0, 0);
  }
  final BoundedZip zip;
  try {
    zip = BoundedZip.open(bytes, limits: limits.archive);
  } on UnsafeArchive catch (error) {
    throw ExportFailure(ExportProblem.unsafe, error.reason);
  }
  ZipEntry? found;
  var account = false;
  var voice = 0;
  var other = 0;
  for (final entry in zip.entries) {
    final base = entry.baseName;
    if (base == 'conversations.json' && found == null) {
      found = entry;
    } else if (_accountFiles.contains(base)) {
      account = true;
    } else if (_voice.any(base.endsWith)) {
      voice++;
    } else {
      other++;
    }
  }
  if (found == null) {
    throw const ExportFailure(
      ExportProblem.unsupported,
      'no conversations.json',
    );
  }
  final Uint8List json;
  try {
    json = zip.read(found, checkpoint: checkpoint);
  } on UnsafeArchive catch (error) {
    throw ExportFailure(ExportProblem.unsafe, error.reason);
  } on FormatException catch (error) {
    throw ExportFailure(ExportProblem.unsafe, 'broken data: ${error.message}');
  }
  return (json, hash, account, voice, other);
}

List<({ConversationPreview preview, ImportedConversation whole})> _items(
  Uint8List json,
  ExportLimits limits,
  void Function()? checkpoint,
) {
  _checkDepth(json, limits.maxDepth, checkpoint);
  checkpoint?.call();
  final Object? decoded;
  try {
    decoded = jsonDecode(utf8.decode(json));
  } on FormatException catch (error) {
    throw ExportFailure(ExportProblem.unsupported, error.message);
  }
  if (decoded is! List) {
    throw const ExportFailure(ExportProblem.unsupported, 'not a list');
  }
  if (decoded.length > limits.maxConversations) {
    throw const ExportFailure(ExportProblem.tooBig, 'too many conversations');
  }
  final accounts = <String>{};
  final ids = <String>{};
  final items = <({ConversationPreview preview, ImportedConversation whole})>[];
  var messages = 0;
  for (final item in decoded) {
    checkpoint?.call();
    if (item is! Map) {
      throw const ExportFailure(ExportProblem.unsupported, 'not an object');
    }
    final list = item['chat_messages'];
    if (list is! List) {
      throw const ExportFailure(
        ExportProblem.unsupported,
        'a conversation with no chat_messages',
      );
    }
    final owner = item['account'];
    if (owner is Map && owner['uuid'] is String) {
      accounts.add(owner['uuid'] as String);
    }
    var you = 0;
    var claude = 0;
    var snippet = '';
    var leftOut = 0;
    final whole = <ImportedMessage>[];
    for (final message in list) {
      if (message is! Map) continue;
      final text = _textOf(message);
      final role = switch (message['sender']) {
        'human' => 'user',
        'assistant' => 'assistant',
        _ => null,
      };
      if (role == null) continue;
      if (role == 'user') {
        you++;
        if (snippet.isEmpty) snippet = _cut(text, 140);
      } else {
        claude++;
      }
      // Files attached to it are left out: only its words come in.
      for (final key in const ['attachments', 'files']) {
        if (message[key] case final List files) leftOut += files.length;
      }
      whole.add(
        ImportedMessage(
          role: role,
          text: text,
          createdMs: _time(message['created_at']),
        ),
      );
    }
    messages += list.length;
    if (messages > limits.maxMessages) {
      throw const ExportFailure(ExportProblem.tooBig, 'too many messages');
    }
    // Its id, or where it is in the file when it has none (or one an
    // earlier conversation has), so each can be chosen on its own.
    final uuid = _cut(_string(item['uuid']), 100);
    final id = uuid.isEmpty || !ids.add(uuid) ? '#${items.length}' : uuid;
    final preview = ConversationPreview(
      id: id,
      title: _cut(_string(item['name']).trim(), 200),
      createdMs: _time(item['created_at']),
      fromYou: you,
      fromClaude: claude,
      snippet: snippet,
    );
    items.add((
      preview: preview,
      whole: ImportedConversation(
        id: preview.id,
        title: preview.title,
        createdMs: preview.createdMs,
        messages: whole,
        leftOutFiles: leftOut,
      ),
    ));
  }
  // Conversations from more than one account: an organization's export,
  // say. Not proof of whose they are, but reason enough to stop.
  if (accounts.length > 1) {
    throw ExportFailure(
      ExportProblem.severalPeople,
      '${accounts.length} accounts',
    );
  }
  return items;
}

/// A message's words: its `text`, or (in newer exports) its `content`'s
/// text parts.
String _textOf(Map<Object?, Object?> message) {
  final text = _string(message['text']);
  if (text.isNotEmpty) return text;
  return [
    for (final part in message['content'] as List? ?? const [])
      if (part is Map && part['type'] == 'text') _string(part['text']),
  ].join('\n\n');
}

/// Throws when [json] nests deeper than [max], before it's decoded (the
/// decoder recurses).
void _checkDepth(Uint8List json, int max, void Function()? checkpoint) {
  var depth = 0;
  var inString = false;
  var escaped = false;
  for (var i = 0; i < json.length; i++) {
    if (i & 0xFFFFF == 0) checkpoint?.call();
    final byte = json[i];
    if (inString) {
      if (escaped) {
        escaped = false;
      } else if (byte == 0x5c) {
        escaped = true;
      } else if (byte == 0x22) {
        inString = false;
      }
      continue;
    }
    switch (byte) {
      case 0x22:
        inString = true;
      case 0x5b || 0x7b:
        if (++depth > max) {
          throw ExportFailure(ExportProblem.unsafe, 'nested deeper than $max');
        }
      case 0x5d || 0x7d:
        depth--;
    }
  }
}

String _string(Object? value) => value is String ? value : '';

String _cut(String text, int most) {
  final line = text.replaceAll(RegExp(r'\s+'), ' ').trim();
  if (line.length <= most) return line;
  return '${line.substring(0, most - 1).trimRight()}…';
}

int _time(Object? value) {
  if (value is! String) return 0;
  return DateTime.tryParse(value)?.millisecondsSinceEpoch ?? 0;
}
