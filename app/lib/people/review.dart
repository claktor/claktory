import 'dart:convert';

import 'package:crypto/crypto.dart';

// D3d, end to end since E5 (dms-e2ee §6; crates/claktory-user-apps/README.md,
// "Agents' messages, reviewed" and "End to end"): what an agent suggests in
// a chat, for the person to decide. A proposal is a message an agent wants
// sent, sealed by its Claktory's leaf in the person's channel with it: this
// device opens it (E2ee.openProposal), shows every hidden character in
// what it opened, and approving seals exactly that again in the
// conversation's group, by its agent, with the digest of what it opened;
// so what's read is what's sent, and the service never had the text. An
// agent's draft is only a suggestion: Edit puts it in the person's own
// box, and their Send is their own message.

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
    this.envelope = '',
    this.epoch = 0,
    this.clientMessageId = '',
    this.proposalDigest = '',
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
      envelope: json['envelope'] == null ? '' : jsonEncode(json['envelope']),
      epoch: _int(json, 'epoch'),
      proposalDigest: _text(json, 'proposal_digest'),
      clientMessageId: _text(json, 'client_message_id'),
    );
  }

  /// This proposal as this device opened it from its Claktory's channel:
  /// who and what from inside the encryption, never the service's word.
  DmProposal opened({
    required String claktoryId,
    required String agentId,
    required String incarnation,
    required String agentName,
    required String text,
    required String clientMessageId,
  }) => DmProposal(
    id: id,
    revision: revision,
    state: state,
    conversationId: conversationId,
    generation: generation,
    claktoryId: claktoryId,
    agentId: agentId,
    incarnation: incarnation,
    agentName: agentName,
    text: text,
    createdMs: createdMs,
    expiresMs: expiresMs,
    envelope: envelope,
    epoch: epoch,
    // The service's: what the home knows it by; the sealed
    // one's before the service listed it.
    clientMessageId: this.clientMessageId.isEmpty
        ? clientMessageId
        : this.clientMessageId,
    proposalDigest: proposalDigest,
  );

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

  /// What the service keeps of it: sealed in the Claktory's channel (an
  /// envelope's JSON), and the channel's epoch then.
  final String envelope;
  final int epoch;

  /// Its sending device's id for the message, once opened: the message
  /// approving it sends goes by it.
  final String clientMessageId;

  /// The service's word for which channel message it is: SHA-256,
  /// lowercase hex, of its MLS bytes. Opened only from the message this
  /// device took from its channel under that digest, from that Claktory's
  /// leaf, so the word is checked by what it opens.
  final String proposalDigest;

  bool get pending => state == 'pending';

  /// What approving it names: SHA-256, lowercase hex, of the envelope's
  /// bytes as this device fetched and opened them; through the home, the
  /// digest it was found and opened by.
  String get digest {
    if (envelope.isEmpty) return proposalDigest;
    final json = jsonDecode(envelope) as Map<String, Object?>;
    final bytes = json['bytes'] as String? ?? '';
    return sha256
        .convert(
          base64Url.decode(bytes.padRight((bytes.length + 3) ~/ 4 * 4, '=')),
        )
        .toString();
  }
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
    this.envelope = '',
    this.draftDigest = '',
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
      envelope: json['envelope'] == null ? '' : jsonEncode(json['envelope']),
      draftDigest: _text(json, 'draft_digest'),
    );
  }

  /// This draft as this device opened it from its Claktory's channel.
  DmAgentDraft opened({
    required String claktoryId,
    required String agentName,
    required String text,
  }) => DmAgentDraft(
    id: id,
    agentName: agentName,
    claktoryId: claktoryId,
    revision: revision,
    text: text,
    updatedMs: updatedMs,
    conversationId: conversationId,
    envelope: envelope,
    draftDigest: draftDigest,
  );

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

  /// Sealed in the Claktory's channel: an envelope's JSON.
  final String envelope;

  /// Through the home, no [envelope]: which channel message it is,
  /// SHA-256 lowercase hex of its MLS bytes.
  final String draftDigest;
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
    (rune >= 0xfff0 && rune <= 0xfffb) ||
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

/// Whether the zero width joiner at [at] joins two emoji (👩‍💻, ❤️‍🔥,
/// 🧑🏽‍💻): the character after it is a pictograph, and so is the one before
/// it, with at most one U+FE0F and one skin tone (U+1F3FB–1F3FF) between.
/// Anywhere else it could hide something. The same rule as the Claktories'
/// and the certificates' (claktory-dm-crypto's joins_pictographs).
bool _joinsPictographs(List<int> runes, int at) {
  if (at + 1 >= runes.length || !pictographic(runes[at + 1])) return false;
  var selector = false, tone = false;
  for (var i = at - 1; i >= 0; i--) {
    final before = runes[i];
    if (before == 0xfe0f && !selector) {
      selector = true;
    } else if (before >= 0x1f3fb && before <= 0x1f3ff && !tone) {
      tone = true;
    } else {
      return pictographic(before);
    }
  }
  return false;
}

/// Unicode's Extended_Pictographic (emoji-data.txt, Unicode 15.1), which
/// emoji sequences are made of: not letters, digits, flags' regional
/// indicators or skin tones.
bool pictographic(int rune) {
  var low = 0, high = _pictographs.length ~/ 2 - 1;
  while (low <= high) {
    final middle = (low + high) ~/ 2;
    if (_pictographs[middle * 2 + 1] < rune) {
      low = middle + 1;
    } else if (_pictographs[middle * 2] > rune) {
      high = middle - 1;
    } else {
      return true;
    }
  }
  return false;
}

/// [pictographic]'s ranges, first and last of each, in order.
const _pictographs = [
  0x00a9, 0x00a9, 0x00ae, 0x00ae, 0x203c, 0x203c, 0x2049, 0x2049, //
  0x2122, 0x2122, 0x2139, 0x2139, 0x2194, 0x2199, 0x21a9, 0x21aa,
  0x231a, 0x231b, 0x2328, 0x2328, 0x2388, 0x2388, 0x23cf, 0x23cf,
  0x23e9, 0x23f3, 0x23f8, 0x23fa, 0x24c2, 0x24c2, 0x25aa, 0x25ab,
  0x25b6, 0x25b6, 0x25c0, 0x25c0, 0x25fb, 0x25fe, 0x2600, 0x2605,
  0x2607, 0x2612, 0x2614, 0x2685, 0x2690, 0x2705, 0x2708, 0x2712,
  0x2714, 0x2714, 0x2716, 0x2716, 0x271d, 0x271d, 0x2721, 0x2721,
  0x2728, 0x2728, 0x2733, 0x2734, 0x2744, 0x2744, 0x2747, 0x2747,
  0x274c, 0x274c, 0x274e, 0x274e, 0x2753, 0x2755, 0x2757, 0x2757,
  0x2763, 0x2767, 0x2795, 0x2797, 0x27a1, 0x27a1, 0x27b0, 0x27b0,
  0x27bf, 0x27bf, 0x2934, 0x2935, 0x2b05, 0x2b07, 0x2b1b, 0x2b1c,
  0x2b50, 0x2b50, 0x2b55, 0x2b55, 0x3030, 0x3030, 0x303d, 0x303d,
  0x3297, 0x3297, 0x3299, 0x3299, 0x1f000, 0x1f0ff, 0x1f10d, 0x1f10f,
  0x1f12f, 0x1f12f, 0x1f16c, 0x1f171, 0x1f17e, 0x1f17f, 0x1f18e, 0x1f18e,
  0x1f191, 0x1f19a, 0x1f1ad, 0x1f1e5, 0x1f201, 0x1f20f, 0x1f21a, 0x1f21a,
  0x1f22f, 0x1f22f, 0x1f232, 0x1f23a, 0x1f23c, 0x1f23f, 0x1f249, 0x1f3fa,
  0x1f400, 0x1f53d, 0x1f546, 0x1f64f, 0x1f680, 0x1f6ff, 0x1f774, 0x1f77f,
  0x1f7d5, 0x1f7ff, 0x1f80c, 0x1f80f, 0x1f848, 0x1f84f, 0x1f85a, 0x1f85f,
  0x1f888, 0x1f88f, 0x1f8ae, 0x1f8ff, 0x1f90c, 0x1f93a, 0x1f93c, 0x1f945,
  0x1f947, 0x1faff, 0x1fc00, 0x1fffd,
];

/// [text] in runs: what shows as it is, and each hidden character on its
/// own ([hidden]): the ones [hiddenCharacter] names, the braille blank, a
/// variation selector anywhere but right after a visible character, and a
/// zero width joiner anywhere but between two emoji.
List<({String text, bool hidden})> reviewRuns(String text) {
  final runs = <({String text, bool hidden})>[];
  final plain = StringBuffer();
  final runes = text.runes.toList();
  // Whether the last character shown as it is may take a selector.
  var takesSelector = false;
  for (var at = 0; at < runes.length; at++) {
    final rune = runes[at];
    final hidden = rune == 0x200d
        ? !_joinsPictographs(runes, at)
        : hiddenCharacter(rune) ||
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
