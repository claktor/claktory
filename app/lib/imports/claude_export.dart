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

  /// Its id in the export; empty when it had none.
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
  if (bytes.length > limits.maxInput) {
    throw const ExportFailure(ExportProblem.tooBig);
  }
  final hash = sha256.convert(bytes).toString();
  if (!BoundedZip.looksLikeZip(bytes)) {
    // The conversations' JSON on its own.
    return ExportPreview(
      conversations: _conversations(bytes, limits, checkpoint),
      sha256: hash,
    );
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
  return ExportPreview(
    conversations: _conversations(json, limits, checkpoint),
    sha256: hash,
    account: account,
    voice: voice,
    other: other,
  );
}

List<ConversationPreview> _conversations(
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
  final previews = <ConversationPreview>[];
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
    for (final message in list) {
      if (message is! Map) continue;
      switch (message['sender']) {
        case 'human':
          you++;
          if (snippet.isEmpty) snippet = _cut(_string(message['text']), 140);
        case 'assistant':
          claude++;
      }
    }
    messages += list.length;
    if (messages > limits.maxMessages) {
      throw const ExportFailure(ExportProblem.tooBig, 'too many messages');
    }
    previews.add(
      ConversationPreview(
        id: _cut(_string(item['uuid']), 100),
        title: _cut(_string(item['name']).trim(), 200),
        createdMs: _time(item['created_at']),
        fromYou: you,
        fromClaude: claude,
        snippet: snippet,
      ),
    );
  }
  // Conversations from more than one account: an organization's export,
  // say. Not proof of whose they are, but reason enough to stop.
  if (accounts.length > 1) {
    throw ExportFailure(
      ExportProblem.severalPeople,
      '${accounts.length} accounts',
    );
  }
  previews.sort((a, b) => b.createdMs.compareTo(a.createdMs));
  return previews;
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
