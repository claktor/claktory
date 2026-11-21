import 'package:factory_app/connections.dart';
import 'package:factory_app/links.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

final _code = 'factory://?t=${'a' * 64}&name=box&e=${'c' * 64}';

/// claktory://add?code=… adds the Claktory the code is for, once the
/// person says so.
void main() {
  tearDown(() {
    while (appLinks.take() != null) {}
  });

  String link(String code) =>
      'claktory://add?code=${Uri.encodeComponent(code)}';

  test('a link to add a Claktory holds its code, encoded or not', () {
    expect(parseLink(Uri.parse(link(_code)))!.target!.name, 'box');
    // Pasted in as it is, the code runs on into the link's own query.
    expect(
      parseLink(Uri.parse('claktory://add?code=$_code'))!.target!.name,
      'box',
    );
    expect(parseLink(Uri.parse('claktory-dev://add?code=$_code')), isNotNull);
    expect(
      parseLink(Uri.parse('claktory://add?code=nonsense'))!.target,
      isNull,
    );
    // Not a link of this app's.
    expect(parseLink(Uri.parse('claktory://open?code=$_code')), isNull);
    expect(
      parseLink(Uri.parse('https://claktory.com/add?code=$_code')),
      isNull,
    );
  });

  testWidgets('a link asks before connecting, and Connect connects', (
    tester,
  ) async {
    // A Mac's words ("this Mac"); tests run as Android otherwise.
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace());
    await harness.show(tester);
    appLinks.receive(link(_code));
    await settle(tester);
    expect(find.text('Connect to box?'), findsOneWidget);
    expect(
      find.text(
        'A link asks to connect this Mac to the Claktory “box”. Connect '
        "only if you or one of your agents opened it. You'll still need to "
        'allow this Mac on box.',
      ),
      findsOneWidget,
    );
    // Nothing yet.
    expect(harness.connections.current, isNull);
    await tester.tap(find.text('Connect'));
    await settle(tester);
    expect(harness.connections.current?.code, _code);
    expect(find.text('Connect to box?'), findsNothing);
  });

  testWidgets('Cancel leaves everything as it was', (tester) async {
    final harness = Harness(Workspace());
    await harness.show(tester);
    appLinks.receive(link(_code));
    await settle(tester);
    await tester.tap(find.text('Cancel'));
    await settle(tester);
    expect(harness.connections.current, isNull);
    expect(harness.connections.saved, isEmpty);
  });

  testWidgets('a Claktory added already shows at once', (tester) async {
    final harness = Harness(Workspace());
    harness.connections.use(parsePairingCode(_code));
    harness.connections.use(null);
    await harness.show(tester);
    appLinks.receive(link(_code));
    await settle(tester);
    expect(find.text('Connect to box?'), findsNothing);
    expect(harness.connections.current?.code, _code);
  });

  testWidgets("a link without a code says it can't be used", (tester) async {
    final harness = Harness(Workspace());
    await harness.show(tester);
    appLinks.receive('claktory://add?code=nonsense');
    await settle(tester);
    expect(find.text("Can't use this link"), findsOneWidget);
    await tester.tap(find.text('OK'));
    await settle(tester);
    expect(find.text("Can't use this link"), findsNothing);
    expect(harness.connections.saved, isEmpty);
  });
}
