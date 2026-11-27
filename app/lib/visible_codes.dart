/// Text an agent or a file chose, shown so nothing in it can make it read
/// as something else (Fugo): characters that change the text's direction
/// or show nothing are written out as their codes, "\u{202E}".
library;

/// Whether [rune] changes how text reads without showing itself: controls,
/// the direction marks, embeddings, overrides and isolates, zero-width and
/// other invisible characters, variation selectors and tag characters
/// (which can carry whole hidden words).
bool hiddenRune(int rune) =>
    rune < 0x20 ||
    (rune >= 0x7f && rune <= 0x9f) ||
    (rune >= 0x200b && rune <= 0x200f) ||
    (rune >= 0x202a && rune <= 0x202e) ||
    (rune >= 0x2060 && rune <= 0x2069) ||
    rune == 0x061c ||
    rune == 0xfeff ||
    (rune >= 0xfe00 && rune <= 0xfe0f) ||
    (rune >= 0xe0000 && rune <= 0xe007f) ||
    rune == 0x2028 ||
    rune == 0x2029 ||
    rune == 0x00ad ||
    rune == 0x034f ||
    rune == 0x115f ||
    rune == 0x1160 ||
    rune == 0x3164 ||
    rune == 0xffa0 ||
    rune == 0x180e;

/// A file's or folder's name as the app shows it (a pane's title, Files'
/// rows, a message's files): its hidden characters written out as their
/// codes, so `invoice\u202Efdp.exe` can't read as a PDF (Fugo's row 1026).
/// Never for opening or sharing it: the name with them is the file's.
String shownName(String name) => visibleCodes(name);

/// How a hidden character shows: "\u{202E}".
String runeCode(int rune) =>
    '\\u{${rune.toRadixString(16).toUpperCase().padLeft(4, '0')}}';

/// [text] with every [hiddenRune] shown by its code, but for what text and
/// emoji are made of: line breaks, tabs and carriage returns, one variation
/// selector right after an emoji or symbol (❤️) or in a keycap (1️⃣), a
/// zero-width joiner between two of them (👩‍💻), and the tags of the three
/// flags made of them ([_flagTags]). Everything that reorders text, and
/// every other invisible character, shows. For tool calls, which show what
/// an agent chose as it is.
String visibleCodes(String text) {
  // Nothing to do for almost every string: no allocation.
  if (!text.runes.any(
    (rune) => hiddenRune(rune) && rune != 0x0a && rune != 0x09 && rune != 0x0d,
  )) {
    return text;
  }
  final runes = text.runes.toList();
  final out = StringBuffer();
  // The last rune shown as it is, when it's part of an emoji so far.
  var emoji = false;
  // Whether that emoji already took its variation selector.
  var selected = false;
  for (var i = 0; i < runes.length; i++) {
    final rune = runes[i];
    if (_flagTags(runes, i) case final tags when tags > 0) {
      out.write(String.fromCharCodes(runes, i, i + 1 + tags));
      i += tags;
      emoji = true;
      selected = true;
      continue;
    }
    if (rune == 0x0a || rune == 0x09 || rune == 0x0d) {
      out.writeCharCode(rune);
      emoji = false;
      continue;
    }
    final next = i + 1 < runes.length ? runes[i + 1] : -1;
    final kept =
        !hiddenRune(rune) ||
        (rune >= 0xfe00 && rune <= 0xfe0f && emoji && !selected) ||
        (rune == 0xfe0f &&
            _keycap(i > 0 ? runes[i - 1] : -1) &&
            next == 0x20e3) ||
        (rune == 0x200d && emoji && _pictographic(next));
    if (!kept) {
      out.write(runeCode(rune));
      emoji = false;
      continue;
    }
    out.writeCharCode(rune);
    if (rune >= 0xfe00 && rune <= 0xfe0f) {
      selected = true;
    } else if (rune == 0x200d) {
      selected = false;
    } else {
      emoji = _pictographic(rune);
      selected = false;
    }
  }
  return out.toString();
}

/// [text] with what reorders it shown by its code: the embeddings,
/// overrides and isolates (U+202A–202E, U+2066–2069), which right-to-left
/// text never needs, and tag characters but those of the three flags made
/// of them. Marks, joiners, selectors and the rest stay as they are, since
/// Persian, Arabic and Indic text need them (Fugo). For what agents and
/// events write in a conversation, which can repeat what came from outside.
String proseCodes(String text) {
  bool reorders(int rune) =>
      (rune >= 0x202a && rune <= 0x202e) ||
      (rune >= 0x2066 && rune <= 0x2069) ||
      (rune >= 0xe0000 && rune <= 0xe007f);
  if (!text.runes.any(reorders)) return text;
  final runes = text.runes.toList();
  final out = StringBuffer();
  for (var i = 0; i < runes.length; i++) {
    final rune = runes[i];
    out.write(reorders(rune) ? runeCode(rune) : String.fromCharCode(rune));
    final tags = _flagTags(runes, i);
    if (tags > 0) {
      out.write(String.fromCharCodes(runes, i + 1, i + 1 + tags));
      i += tags;
    }
  }
  return out.toString();
}

/// The subdivision flags written with tag characters (🏴 then the tags of
/// "gbeng", "gbsct" or "gbwls", then the cancel tag): the only tags shown as
/// they are. Any other run of them after 🏴 is how hidden words get in.
const _flags = ['gbeng', 'gbsct', 'gbwls'];

/// How many tag characters after [runes]' [i]th make one of [_flags] with
/// it: 0 unless it's 🏴 and they do.
int _flagTags(List<int> runes, int i) {
  if (runes[i] != 0x1f3f4) return 0;
  for (final flag in _flags) {
    final length = flag.length + 1;
    if (i + length >= runes.length) continue;
    var matches = true;
    for (var j = 0; j < flag.length && matches; j++) {
      matches = runes[i + 1 + j] == 0xe0000 + flag.codeUnitAt(j);
    }
    if (matches && runes[i + length] == 0xe007f) return length;
  }
  return 0;
}

/// Whether [rune] can start a keycap emoji (1️⃣): a digit, # or *.
bool _keycap(int rune) =>
    (rune >= 0x30 && rune <= 0x39) || rune == 0x23 || rune == 0x2a;

/// Whether [rune] is an emoji or a symbol that can be one: what a
/// variation selector or a joiner may follow.
bool _pictographic(int rune) =>
    (rune >= 0x1f000 && rune <= 0x1faff) ||
    (rune >= 0x2190 && rune <= 0x2bff) ||
    rune == 0x00a9 ||
    rune == 0x00ae ||
    rune == 0x203c ||
    rune == 0x2049 ||
    rune == 0x2122 ||
    rune == 0x2139 ||
    rune == 0x3030 ||
    rune == 0x303d ||
    rune == 0x3297 ||
    rune == 0x3299;

/// Every string in [value] (a decoded JSON value: maps, lists, strings)
/// through [visibleCodes], but those under a key that holds words
/// ([proseKeys]: a message, a checklist item's text) through [proseCodes],
/// so the writing in them reads as written; keys and other values as they
/// are.
Object? visibleCodesIn(Object? value, {bool prose = false}) => switch (value) {
  final String text => prose ? proseCodes(text) : visibleCodes(text),
  final Map<String, Object?> map => {
    for (final entry in map.entries)
      entry.key: visibleCodesIn(
        entry.value,
        prose: proseKeys.contains(entry.key),
      ),
  },
  final List<Object?> list => [
    for (final item in list) visibleCodesIn(item, prose: prose),
  ],
  _ => value,
};

/// The keys in a tool call's input and details that hold words, not code
/// (Fugo): what agents_send says, an ask's title and body, a checklist
/// item's text and note, reasons and summaries. Paths, commands, addresses
/// and ids are under other keys.
const proseKeys = {
  'text',
  'message',
  'title',
  'body',
  'note',
  'reason',
  'summary',
  'description',
  'rule',
  'question',
};

/// A name as text shows it, whoever chose it (a Claktory's, a device's, a
/// pipe's other side; Fugo 50896, Abbacchio's row 1730): what reorders it
/// shown by its code ([proseCodes]), line breaks as spaces, and anything
/// but plain ASCII set apart in a first-strong isolate (U+2068…U+2069), so
/// no name can turn the words around it ("since 06:10. Private and
/// encrypted.") backwards, right-to-left names included. Marks and joiners
/// stay, for the scripts that need them.
String nameInText(String name) {
  final shown = proseCodes(
    name.replaceAll(RegExp('[\r\n\t\u0085\u2028\u2029]+'), ' '),
  );
  return shown.runes.every((rune) => rune >= 0x20 && rune < 0x7f)
      ? shown
      : '\u2068$shown\u2069';
}
