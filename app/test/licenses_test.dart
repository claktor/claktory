// The licences the app ships that aren't Dart's or Flutter's own
// (licenses.dart): ZXing core, once, whole.
import 'package:factory_app/licenses.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('ZXing core 3.5.3 is listed once, with its copyright and the whole '
      'Apache 2.0 text', () async {
    registerLicenses();
    registerLicenses();
    final zxing = [
      await for (final entry in LicenseRegistry.licenses)
        if (entry.packages.contains('ZXing core 3.5.3')) entry,
    ];
    expect(zxing, hasLength(1));
    final text = [
      for (final paragraph in zxing.single.paragraphs) paragraph.text,
    ].join('\n');
    expect(text, contains('Copyright (C) 2007 ZXing authors'));
    expect(text, contains('Version 2.0, January 2004'));
    expect(text, contains('2. Grant of Copyright License.'));
    expect(text, contains('7. Disclaimer of Warranty.'));
    expect(text, contains('END OF TERMS AND CONDITIONS'));
  });
}
