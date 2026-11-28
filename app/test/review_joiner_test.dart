// A zero width joiner on the review card: kept between two emoji, by the
// same rule the Claktories and the certificates use, and shown by its code
// anywhere else.
import 'package:factory_app/people/review.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('a joiner between two emoji is theirs', () {
    for (final joined in [
      '👩\u200d💻',
      '🏳\ufe0f\u200d🌈',
      '❤\ufe0f\u200d🔥',
      '🧑🏽\u200d💻',
      '🧑\ufe0f🏽\u200d💻',
      '👨\u200d👩\u200d👧',
      'Ana 👩\u200d💻',
    ]) {
      expect(hasHidden(joined), isFalse, reason: joined.runes.toString());
    }
  });

  test('anywhere else it shows by its code', () {
    for (final stray in [
      'A\u200dna',
      '\u200d👍',
      '👍\u200d',
      '👍\u200d\u200d💻',
      'a\u200d💻',
      '💻\u200da',
      '👍\ufe0f\ufe0f\u200d💻',
      '👍🏽🏽\u200d💻',
    ]) {
      expect(
        reviewRuns(stray).where((run) => run.hidden).map((run) => run.text),
        contains('‹U+200D›'),
        reason: stray.runes.toString(),
      );
    }
    expect(hasHidden('a\ufff0b'), isTrue, reason: 'an unassigned special');
    expect(reviewRuns('👍\u200d\u200d💻'), [
      (text: '👍', hidden: false),
      (text: '‹U+200D›', hidden: true),
      (text: '‹U+200D›', hidden: true),
      (text: '💻', hidden: false),
    ]);
  });

  test('pictographs are Unicode\'s, not letters or skin tones', () {
    expect(pictographic('©'.runes.single), isTrue);
    expect(pictographic(0x1fffd), isTrue);
    expect(pictographic(0x2600), isTrue);
    expect(pictographic(0x2606), isFalse);
    expect(pictographic('A'.runes.single), isFalse);
    expect(pictographic(0x1f3fb), isFalse);
    expect(pictographic(0x1f1e6), isFalse);
  });
}
