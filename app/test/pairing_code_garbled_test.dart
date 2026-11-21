import 'package:factory_app/connections.dart';
import 'package:factory_app/links.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

/// A garbled paste or link (a broken %-escape, which decoding the query
/// throws on) is not a code: it says so, rather than taking the form away.
void main() {
  const garbled = [
    'factory://box:7420?t=%E0%A4',
    'factory://box?e=%E0%A4&t=%',
    'factory://box?name=%C3',
  ];

  test('a broken escape is not a pairing code', () {
    for (final code in garbled) {
      expect(parsePairingCode(code), isNull, reason: code);
      expect(parsePairingCode(code, fromAccount: true), isNull, reason: code);
    }
  });

  test('a link with a broken escape carries no code', () {
    expect(parseLink(Uri.parse('claktory://add?code=%E0%A4'))!.target, isNull);
    expect(
      parseLink(Uri.parse('claktory://add?x=%C3&code=factory://box?t=%E0'))!
          .target,
      isNull,
    );
  });

  testWidgets('pasting a garbled code on a phone says it isn\'t one', (
    tester,
  ) async {
    final harness = Harness(Workspace(), localBackend: false);
    harness.camera.state = CameraAccess.granted;
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    await tester.ensureVisible(find.text('Paste code instead'));
    await pumpFrames(tester);
    await tester.tap(find.text('Paste code instead'));
    await pumpFrames(tester);
    await tester.enterText(find.byType(EditableText), garbled.first);
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    expect(find.textContaining("That isn't a Claktory code."), findsOneWidget);
    expect(find.byType(EditableText), findsOneWidget, reason: 'the form stays');
  });
}
