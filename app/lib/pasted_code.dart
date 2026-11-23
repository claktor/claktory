// What a code pasted (or scanned) into a field that connects to a
// Claktory is: a pairing code, which connects, or another of Claktory's
// codes, which goes somewhere else (friend bug #2: a friend pasted one of
// those into Android's pairing field and was told to copy it again).
import 'connections.dart';
import 'links.dart' show linkSchemes, parseLink;

/// Claktory's codes that aren't for connecting a device.
enum OtherCode {
  /// `claktory-pipe://…`, for joining a pipe.
  pipeInvite,

  /// `claktory-move://…`, for moving Claktory to another computer.
  move,

  /// `claktory://add-friend#…` (or its claktory.com page): a friend ID.
  friend,

  /// `claktory-dm-add:…`, for adding a device to the person's messages.
  messagesDevice,

  /// `claktory-safety:…`, for checking a friend's security key.
  safety,

  /// A pairing code cut off or damaged: it starts as one but isn't.
  cutPairing,
}

/// The Claktory [text] connects to: a pairing code, as pasted from a
/// messenger or a note too, or a `claktory://add?code=…` link holding one;
/// null when it's neither.
RemoteFactory? pastedPairingCode(String text) {
  if (parsePairingCode(text) case final target?) return target;
  final link = _link.firstMatch(text);
  if (link == null) return null;
  final uri = Uri.tryParse(_oneWord(text.substring(link.start)));
  if (uri == null || !linkSchemes.contains(uri.scheme)) return null;
  return parseLink(uri)?.target;
}

final _link = RegExp(r'\bclaktory(-dev|-debug)?://add\?', caseSensitive: false);

/// A code is one word, its parts percent-encoded: from where the link
/// starts, its lines joined until a line of words, then the first word.
String _oneWord(String text) {
  final lines = text.split(RegExp('\r\n|[\r\n\u2028\u2029]'));
  var joined = lines.first.trim();
  for (final line in lines.skip(1).map((line) => line.trim())) {
    if (line.isEmpty || line.contains(RegExp(r'\s'))) break;
    joined += line;
  }
  return joined.split(RegExp(r'\s')).first;
}

/// Which other code [text] is, when it isn't one that connects; null when
/// it's none of Claktory's.
OtherCode? otherCode(String text) {
  if (pastedPairingCode(text) != null) return null;
  final lower = text.toLowerCase();
  if (lower.contains('claktory-pipe:')) return OtherCode.pipeInvite;
  if (lower.contains('claktory-move:')) return OtherCode.move;
  if (lower.contains('add-friend#')) return OtherCode.friend;
  if (lower.contains('claktory-dm-add:')) return OtherCode.messagesDevice;
  if (lower.contains('claktory-safety:')) return OtherCode.safety;
  if (_pairingStart.hasMatch(text)) return OtherCode.cutPairing;
  return null;
}

final _pairingStart = RegExp(r'\b(factory|claktory)://', caseSensitive: false);

/// Why [text] can't connect this [device] ("iPhone", "Mac"), when it's
/// another of Claktory's codes: what it is, then [where] to get the right
/// one ([onMac], [whereItRuns]). Null when it isn't one of them, for the
/// field's own words. A [scanned] code is never cut off: a damaged one is
/// just not a code. Trish's words.
String? otherCodeProblem(
  String text, {
  required String device,
  required String where,
  bool scanned = false,
}) {
  final not = 'not a code to connect this $device. To get one, $where';
  return switch (otherCode(text)) {
    null => null,
    OtherCode.cutPairing when scanned => null,
    OtherCode.cutPairing =>
      "That's only part of a Claktory code. Copy all of it again and paste "
          'it here.',
    OtherCode.pipeInvite => "That's a pipe invite, $not",
    OtherCode.move => "That's a code for moving Claktory, $not",
    OtherCode.friend => "That's a friend ID, $not",
    OtherCode.messagesDevice =>
      "That's a code for adding a device to your messages, $not",
    OtherCode.safety => "That's a code for verifying a friend, $not",
  };
}

/// [where] for an iPhone, connecting to a Claktory on a Mac.
const onMac = 'on your Mac open Settings → Devices and choose Add a device.';

/// [where] for a device connecting to a Claktory that may run anywhere.
const whereItRuns =
    'on the computer Claktory runs on open Settings → Devices and choose '
    'Add a device.';
