// What Settings' search finds, and where a
// link into Settings goes: kept apart from the widgets, so it reads the
// same on every platform and is tested alone.

/// Where in Settings a link or a search result goes: [section]'s id, and
/// within it a setting's id, or empty for the section's top.
typedef SettingsPlace = ({String section, String setting});

/// [section] and [setting] as one string, as [Backend.settingsSection]
/// carries it: "general#move".
String settingsPlace(String section, [String setting = '']) =>
    setting.isEmpty ? section : '$section#$setting';

/// The place a [settingsPlace] string names, with the names show_screen and
/// older links used for a section that moved or was renamed.
SettingsPlace settingsPlaceOf(String place) {
  final at = place.indexOf('#');
  final section = (at < 0 ? place : place.substring(0, at)).toLowerCase();
  final setting = at < 0 ? '' : place.substring(at + 1);
  return (section: settingsSectionAliases[section] ?? section, setting: setting);
}

/// Section names Settings had before, or that agents may use, for the
/// section that holds them now.
const settingsSectionAliases = {
  'report types': 'pipes',
  'report-types': 'pipes',
  'storage': 'device',
  'this mac': 'device',
  'this iphone': 'device',
  'this ipad': 'device',
  'this phone': 'device',
};

/// A setting search finds: its [label] as it reads on screen, the [id] of
/// the [SettingAnchor] that holds it in its section (empty: the section's
/// top) and the other [words] people call it by.
class SettingEntry {
  const SettingEntry(this.label, {this.id = '', this.words = const []});

  final String label;
  final String id;
  final List<String> words;
}

/// What a section gives search: its id and title, the settings in it, and
/// its own [items] (secret, device, account and report type names), which
/// lead to its top.
class SearchableSection {
  const SearchableSection({
    required this.id,
    required this.title,
    this.entries = const [],
    this.items = const [],
    this.words = const [],
  });

  final String id;
  final String title;
  final List<SettingEntry> entries;
  final List<String> items;

  /// Words that find the section itself.
  final List<String> words;
}

/// One search result: a setting, an item or a section, by its [label],
/// in the section titled [sectionTitle]; or, with [section] empty, an
/// answer that leads nowhere ("Light and dark follow your Mac's
/// appearance.").
class SettingsResult {
  const SettingsResult({
    required this.label,
    this.section = '',
    this.sectionTitle = '',
    this.setting = '',
  });

  final String label;
  final String section;
  final String sectionTitle;
  final String setting;

  bool get isAnswer => section.isEmpty;

  @override
  bool operator ==(Object other) =>
      other is SettingsResult &&
      other.label == label &&
      other.section == section &&
      other.setting == setting;

  @override
  int get hashCode => Object.hash(label, section, setting);

  @override
  String toString() => 'SettingsResult($label, $section#$setting)';
}

/// What [query] finds in [sections], best first: labels and titles that
/// start with it, then those with a word that does, then the rest that
/// hold it; a setting's other words after its label. Each setting once.
/// An answer ([settingsAnswer]) comes first when there is one. [device]
/// names this device in an answer ("Mac", "iPhone").
List<SettingsResult> searchSettings(
  List<SearchableSection> sections,
  String query, {
  required String device,
}) {
  final words = _normal(query);
  if (words.isEmpty) return const [];
  final scored = <(int, int, SettingsResult)>[];
  var order = 0;
  void consider(SettingsResult result, String label, List<String> others) {
    var best = _score(label, words);
    for (final other in others) {
      final score = _score(other, words);
      // A word people use finds it, after what its label finds.
      if (score != null && (best == null || score + 3 < best)) {
        best = score + 3;
      }
    }
    if (best != null) scored.add((best, order++, result));
  }

  for (final section in sections) {
    consider(
      SettingsResult(
        label: section.title,
        section: section.id,
        sectionTitle: section.title,
      ),
      section.title,
      section.words,
    );
    for (final entry in section.entries) {
      consider(
        SettingsResult(
          label: entry.label,
          section: section.id,
          sectionTitle: section.title,
          setting: entry.id,
        ),
        entry.label,
        entry.words,
      );
    }
    for (final item in section.items) {
      consider(
        SettingsResult(
          label: item,
          section: section.id,
          sectionTitle: section.title,
        ),
        item,
        const [],
      );
    }
  }
  scored.sort((a, b) => a.$1 != b.$1 ? a.$1 - b.$1 : a.$2 - b.$2);
  final seen = <SettingsResult>{};
  return [
    if (settingsAnswer(query, device: device) case final answer?)
      SettingsResult(label: answer),
    for (final (_, _, result) in scored)
      if (seen.add(result)) result,
  ];
}

/// An answer to what Settings has no setting for, by what people type:
/// "dark mode" → light and dark follow the device.
String? settingsAnswer(String query, {required String device}) {
  final text = _normal(query).join(' ');
  // A few letters aren't a question yet ("d" isn't "dark mode").
  if (text.length < 4) return null;
  for (final (asks, answer) in _answers) {
    if (asks.any((ask) => ask.startsWith(text) || text.startsWith(ask))) {
      return answer(device);
    }
  }
  return null;
}

final _answers = <(List<String>, String Function(String device))>[
  (
    ['dark mode', 'light mode', 'appearance', 'theme', 'dark', 'night mode'],
    (device) => "Light and dark follow your $device's appearance.",
  ),
];

/// How well [text] matches every one of [words]: 0 when it starts with
/// the first, 1 when a word of it does, 2 when it holds it somewhere;
/// null when one of [words] isn't in it.
int? _score(String text, List<String> words) {
  final normal = _normal(text);
  if (normal.isEmpty) return null;
  final joined = normal.join(' ');
  var worst = 0;
  for (final (i, word) in words.indexed) {
    final int score;
    if (i == 0 && joined.startsWith(word)) {
      score = 0;
    } else if (normal.any((each) => each.startsWith(word))) {
      score = 1;
    } else if (joined.contains(word)) {
      score = 2;
    } else {
      return null;
    }
    if (score > worst) worst = score;
  }
  return worst;
}

final _between = RegExp(r"[^\p{L}\p{N}'.$]+", unicode: true);

/// [text]'s words in lower case, without punctuation: "Add a device…" →
/// [add, a, device].
List<String> _normal(String text) => [
  for (final word in text.toLowerCase().split(_between))
    if (word.replaceAll(RegExp(r"^[.']+|[.']+$"), '') case final w
        when w.isNotEmpty)
      w,
];
