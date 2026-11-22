import 'package:factory_app/visible_codes.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('what reorders or hides text shows by its code', () {
    expect(visibleCodes('evil\u202Etxt.exe'), r'evil\u{202E}txt.exe');
    expect(visibleCodes('a\u2066b\u2069c'), r'a\u{2066}b\u{2069}c');
    expect(visibleCodes('a\u061Cb'), r'a\u{061C}b');
    expect(visibleCodes('pay\u200Bpal'), r'pay\u{200B}pal');
    expect(visibleCodes('a\u200Db'), r'a\u{200D}b');
    expect(visibleCodes('x\u{E0041}'), r'x\u{E0041}');
    expect(visibleCodes('\u001B[31m'), r'\u{001B}[31m');
    expect(visibleCodes('ab\uFE0F'), r'ab\u{FE0F}');
  });

  test('line breaks, tabs and emoji stay as they are', () {
    const plain = 'line one\n\tline two\r\n';
    expect(identical(visibleCodes(plain), plain), isTrue);
    for (final emoji in ['❤️', '👩‍💻', '🏳️‍🌈', '👍🏽', '1️⃣', '™️']) {
      expect(visibleCodes(emoji), emoji);
    }
    // One selector is the emoji's; a second carries something else.
    expect(
      visibleCodes('❤️\uFE0F'),
      '❤️'
      r'\u{FE0F}',
    );
    // A joiner that joins nothing shows.
    expect(
      visibleCodes('👩\u200Da'),
      '👩'
      r'\u{200D}a',
    );
  });

  const scotland = '🏴\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F}';
  // 🏴 with tags that make no flag: hidden words.
  const smuggled = '🏴\u{E0068}\u{E0069}\u{E007F}';

  test('the three tag flags stay; other tags show', () {
    for (final codes in [visibleCodes, proseCodes]) {
      expect(codes('a $scotland b'), 'a $scotland b');
      expect(codes(smuggled), r'🏴\u{E0068}\u{E0069}\u{E007F}');
      expect(codes('\u{E0067}'), r'\u{E0067}');
      // A flag cut short is no flag.
      expect(
        codes('🏴\u{E0067}\u{E0062}\u{E0073}'),
        r'🏴\u{E0067}\u{E0062}\u{E0073}',
      );
    }
  });

  test('prose: only what reorders text shows', () {
    expect(proseCodes('evil\u202Etxt.exe'), r'evil\u{202E}txt.exe');
    expect(
      proseCodes('\u202Aa\u202C \u2067b\u2069'),
      r'\u{202A}a\u{202C} \u{2067}b\u{2069}',
    );
    // What right-to-left and Indic text need stays.
    for (final text in [
      'می\u200Cخواهم',
      'مرحبا \u200Fabc',
      'a\u200Eb\u061Cc',
      'क्\u200Dष',
      '❤️ 👩‍💻 soft\u00ADhyphen',
      'line\n\ttab',
    ]) {
      expect(identical(proseCodes(text), text), isTrue, reason: text);
    }
  });

  test('words under a prose key keep what writing needs', () {
    expect(
      visibleCodesIn({
        'text': 'می\u200Cخواهم \u202E',
        'path': 'a\u200Cb',
        'items': [
          {'text': 'x\u200Cy', 'id': 'i\u200Cd'},
        ],
      }),
      {
        'text':
            'می\u200Cخواهم '
            r'\u{202E}',
        'path': r'a\u{200C}b',
        'items': [
          {'text': 'x\u200Cy', 'id': r'i\u{200C}d'},
        ],
      },
    );
  });

  test('every string in a decoded value', () {
    expect(
      visibleCodesIn({
        'path': 'a\u202Eb',
        'list': ['c\u200Bd', 1],
        'n': 2,
      }),
      {
        'path': r'a\u{202E}b',
        'list': [r'c\u{200B}d', 1],
        'n': 2,
      },
    );
  });
}
