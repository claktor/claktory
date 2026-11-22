import 'dart:convert';
import 'dart:typed_data';

import 'package:crypto/crypto.dart';

// D3d: what an agent suggests in a chat, for the person to decide (Mista's
// D4b and Ghiaccio's D4a, crates/claktory-user-apps/README.md, "Agents" and
// "Agents' messages, reviewed"). A proposal is a message an agent wants
// sent: it goes only once the person approved exactly it, here, from this
// device; this device works out what it approves (the content hash) from
// what it shows, never taking the service's word for it, and shows every
// hidden character in the text, so what's read is what's sent. An agent's
// draft is only a suggestion: Edit puts it in the person's own box,
// and their Send is their own message.

/// A message an agent proposed, as the service lists it.
class DmProposal {
  const DmProposal({
    required this.id,
    required this.revision,
    required this.state,
    required this.conversationId,
    required this.generation,
    required this.claktoryId,
    required this.agentId,
    required this.incarnation,
    required this.agentName,
    required this.text,
    this.createdMs = 0,
    this.expiresMs = 0,
  });

  factory DmProposal.fromJson(Map<String, Object?> json) {
    final from = switch (json['from']) {
      final Map<String, Object?> from => from,
      _ => const <String, Object?>{},
    };
    return DmProposal(
      id: _text(json, 'id'),
      revision: _int(json, 'revision'),
      state: _text(json, 'state'),
      conversationId: _text(json, 'conversation_id'),
      generation: _int(json, 'contact_generation'),
      claktoryId: _text(from, 'claktory_id'),
      agentId: _text(from, 'agent_id'),
      incarnation: _text(from, 'agent_incarnation'),
      agentName: _text(from, 'agent_name'),
      text: _text(json, 'text'),
      createdMs: _int(json, 'created_ms'),
      expiresMs: _int(json, 'expires_ms'),
    );
  }

  final String id;

  /// Each change of its text is the next revision, reviewed afresh.
  final int revision;

  /// `pending` while it waits for the person.
  final String state;
  final String conversationId;

  /// The friendship's generation it was written for.
  final int generation;

  /// Which of the person's Claktories, which agent of it and which
  /// incarnation of that agent.
  final String claktoryId;
  final String agentId;
  final String incarnation;

  /// What its Claktory calls the agent: its own words, unchecked.
  final String agentName;
  final String text;
  final int createdMs;
  final int expiresMs;

  bool get pending => state == 'pending';
}

/// An agent's suggested draft, as the service lists it.
class DmAgentDraft {
  const DmAgentDraft({
    required this.id,
    required this.agentName,
    required this.claktoryId,
    required this.revision,
    required this.text,
    this.updatedMs = 0,
    this.conversationId = '',
  });

  factory DmAgentDraft.fromJson(Map<String, Object?> json) {
    final agent = switch (json['agent']) {
      final Map<String, Object?> agent => agent,
      _ => const <String, Object?>{},
    };
    return DmAgentDraft(
      id: _text(json, 'id'),
      agentName: _text(agent, 'name'),
      claktoryId: _text(agent, 'claktory_id'),
      revision: _int(json, 'revision'),
      text: switch (json['body']) {
        final Map<String, Object?> body => _text(body, 'text'),
        _ => '',
      },
      updatedMs: _int(json, 'updated_ms'),
      conversationId: _text(json, 'conversation_id'),
    );
  }

  final String id;

  /// Where it was drafted: only in the sync snapshot's list, which has no
  /// text.
  final String conversationId;

  /// What its Claktory calls the agent: its own words, unchecked.
  final String agentName;
  final String claktoryId;
  final int revision;
  final String text;
  final int updatedMs;
}

/// What approving [proposal] approves (v1): SHA-256, lowercase hex, of
/// "claktory-dm-review/1\n" and each field the person reviews, as
/// `name:<UTF-8 length>:<value>\n`, in the service's order. Worked out
/// here from what this device shows, so a service that lists one thing
/// can't get another approved.
String reviewHash(DmProposal proposal) {
  final bytes = BytesBuilder()..add(utf8.encode('claktory-dm-review/1\n'));
  for (final (name, value) in [
    ('proposal', proposal.id),
    ('revision', '${proposal.revision}'),
    ('conversation', proposal.conversationId),
    ('generation', '${proposal.generation}'),
    ('claktory', proposal.claktoryId),
    ('agent', proposal.agentId),
    ('incarnation', proposal.incarnation),
    ('agent_name', proposal.agentName),
    ('text', proposal.text),
  ]) {
    final encoded = utf8.encode(value);
    bytes
      ..add(utf8.encode('$name:${encoded.length}:'))
      ..add(encoded)
      ..addByte(0x0a);
  }
  return sha256.convert(bytes.takeBytes()).toString();
}

/// Whether [rune] changes how text reads without showing itself: controls
/// (but a line break or tab), the direction marks and overrides, zero-width
/// and other invisible characters, and tag characters (which can carry
/// whole hidden words).
bool hiddenCharacter(int rune) =>
    (rune < 0x20 && rune != 0x0a && rune != 0x09) ||
    (rune >= 0x7f && rune <= 0x9f) ||
    rune == 0x00ad ||
    rune == 0x034f ||
    rune == 0x061c ||
    rune == 0x115f ||
    rune == 0x1160 ||
    rune == 0x17b4 ||
    rune == 0x17b5 ||
    (rune >= 0x180b && rune <= 0x180f) ||
    (rune >= 0x200b && rune <= 0x200f) ||
    (rune >= 0x2028 && rune <= 0x202e) ||
    (rune >= 0x2060 && rune <= 0x206f) ||
    rune == 0x3164 ||
    rune == 0xfeff ||
    rune == 0xffa0 ||
    (rune >= 0xfff9 && rune <= 0xfffb) ||
    (rune >= 0x1d173 && rune <= 0x1d17a) ||
    (rune >= 0xe0000 && rune <= 0xe0fff);

/// How a hidden character shows: its code, "‹U+202E›".
String hiddenLabel(int rune) =>
    '‹U+${rune.toRadixString(16).toUpperCase().padLeft(4, '0')}›';

/// Whether [rune] is a variation selector (U+FE00–FE0F): it shows nothing
/// itself, and only one, right after a visible character (❤️), is that
/// character's; anywhere else, or more of them, can carry hidden words.
bool _selector(int rune) => rune >= 0xfe00 && rune <= 0xfe0f;

bool _space(int rune) =>
    rune == 0x20 ||
    rune == 0x09 ||
    rune == 0x0a ||
    rune == 0x0d ||
    rune == 0xa0 ||
    rune == 0x1680 ||
    (rune >= 0x2000 && rune <= 0x200a) ||
    rune == 0x202f ||
    rune == 0x205f ||
    rune == 0x3000;

/// [text] in runs: what shows as it is, and each hidden character on its
/// own ([hidden]): the ones [hiddenCharacter] names, the braille blank, and
/// a variation selector anywhere but right after a visible character.
List<({String text, bool hidden})> reviewRuns(String text) {
  final runs = <({String text, bool hidden})>[];
  final plain = StringBuffer();
  // Whether the last character shown as it is may take a selector.
  var takesSelector = false;
  for (final rune in text.runes) {
    final hidden =
        hiddenCharacter(rune) ||
        rune == 0x2800 ||
        (_selector(rune) && !takesSelector);
    if (hidden) {
      if (plain.isNotEmpty) {
        runs.add((text: plain.toString(), hidden: false));
        plain.clear();
      }
      runs.add((text: hiddenLabel(rune), hidden: true));
      takesSelector = false;
    } else {
      plain.writeCharCode(rune);
      takesSelector = !_selector(rune) && !_space(rune);
    }
  }
  if (plain.isNotEmpty) runs.add((text: plain.toString(), hidden: false));
  return runs;
}

/// Whether [text] has a character [reviewRuns] shows by its code.
bool hasHidden(String text) => reviewRuns(text).any((run) => run.hidden);

String _text(Map<String, Object?> json, String key) => switch (json[key]) {
  final String text => text,
  _ => '',
};

int _int(Map<String, Object?> json, String key) => switch (json[key]) {
  final int number => number,
  _ => 0,
};
