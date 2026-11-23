// Friend bug #2: a code pasted into a field that connects to a Claktory.
// A pairing code is taken as a messenger or a note gives it (wrapped over
// lines, among words, in brackets); another of Claktory's codes says what
// it is and where the right one is, rather than "copy it again".
import 'package:factory_app/connections.dart';
import 'package:factory_app/pasted_code.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

/// A code as a Claktory's Settings → Devices → Add a device copies it
/// (factoryd's pairing_code): token, name, endpoint, color, then a relay
/// and an address, each percent-encoded.
final _token = 'a1' * 32;
final _endpoint = 'b2' * 32;
final _code =
    'factory://?t=$_token&name=Matt%27s%20Mac&e=$_endpoint&color=blue'
    '&r=https%3A%2F%2Frelay.example.com%2F&a=192.168.1.4%3A7420';

void _expectCode(String pasted) {
  final target = parsePairingCode(pasted);
  expect(target, isNotNull, reason: pasted);
  expect(target!.code, _code, reason: pasted);
  expect(target.token, _token);
  expect(target.endpointId, _endpoint);
  expect(target.name, "Matt's Mac");
  expect(target.relays, ['https://relay.example.com/']);
  expect(target.addresses, ['192.168.1.4:7420']);
}

void main() {
  test('a code as Add a device copies it is one', () => _expectCode(_code));

  test('a code from a messenger or a note is one too', () {
    final half = _code.length ~/ 2;
    for (final pasted in [
      '  $_code\n',
      '$_code\r\n',
      // Wrapped over lines, in the token, the endpoint and between parts.
      _code.replaceFirst('a1a1', 'a1\na1'),
      _code.replaceFirst('b2b2', 'b2 \r\n b2'),
      '${_code.substring(0, half)}\n${_code.substring(half)}',
      // Among words, in brackets or quotes, before a full stop.
      'Here is the code: $_code thanks!',
      'Code:\n$_code\nSent from my iPhone',
      '<$_code>',
      '"$_code"',
      '($_code).',
      // Invisible characters a messenger leaves in.
      _code.replaceFirst('&e=', '\u200B&e='),
      _code.replaceFirst('&color', '\u00AD&color'),
      // A messenger that showed the name decoded.
      _code.replaceFirst('%20', ' '),
    ]) {
      _expectCode(pasted);
    }
  });

  test('a code cut short is not one', () {
    expect(parsePairingCode(_code.substring(0, 80)), isNull);
    expect(parsePairingCode('see factory://?t=$_token and more'), isNull);
  });

  test('a claktory://add link holding a code connects too', () {
    final link = 'claktory://add?code=${Uri.encodeComponent(_code)}';
    expect(pastedPairingCode(link)?.endpointId, _endpoint);
    expect(pastedPairingCode('Open this: $link')?.endpointId, _endpoint);
    expect(otherCode(link), isNull);
  });

  test('which other code it is', () {
    expect(otherCode(_code), isNull);
    expect(otherCode('hello'), isNull);
    expect(otherCode('claktory-pipe://?i=abc'), OtherCode.pipeInvite);
    expect(otherCode(' claktory-move://?e=1&s=2'), OtherCode.move);
    expect(otherCode('claktory://add-friend#K7Q2'), OtherCode.friend);
    expect(otherCode('https://claktory.com/add-friend#K7Q2'), OtherCode.friend);
    expect(otherCode('claktory-dm-add:1:AAAA'), OtherCode.messagesDevice);
    expect(otherCode('claktory-safety:1:AAAA'), OtherCode.safety);
    expect(otherCode(_code.substring(0, 80)), OtherCode.cutPairing);
    expect(otherCode('Claktory://?t=1'), OtherCode.cutPairing);
  });

  test('a scanned code is never cut off', () {
    expect(
      otherCodeProblem(
        _code.substring(0, 80),
        device: 'iPhone',
        where: onMac,
        scanned: true,
      ),
      isNull,
    );
    expect(
      otherCodeProblem(
        'claktory-safety:1:AAAA',
        device: 'iPhone',
        where: onMac,
        scanned: true,
      ),
      "That's a code for verifying a friend, not a code to connect this "
      'iPhone. To get one, on your Mac open Settings → Devices and choose '
      'Add a device.',
    );
  });

  group('the phone\'s pairing screen', () {
    Future<void> paste(
      WidgetTester tester,
      TargetPlatform platform,
      String text,
    ) async {
      debugDefaultTargetPlatformOverride = platform;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final harness = Harness(Workspace(), localBackend: false);
      harness.camera.state = CameraAccess.unavailable;
      await harness.show(tester, size: const Size(320, 568));
      await pumpFrames(tester);
      if (find.byType(EditableText).evaluate().isEmpty) {
        await tester.ensureVisible(find.text('Paste code instead'));
        await tester.tap(find.text('Paste code instead'));
        await pumpFrames(tester);
      }
      await tester.enterText(find.byType(EditableText), text);
      await pumpFrames(tester);
      expect(tester.takeException(), isNull);
    }

    testWidgets('on Android, a pipe invite says what it is', (tester) async {
      await paste(tester, TargetPlatform.android, 'claktory-pipe://?i=abc');
      expect(
        find.text(
          "That's a pipe invite, not a code to connect this phone. To get "
          'one, on the computer Claktory runs on open Settings → Devices and '
          'choose Add a device.',
        ),
        findsOneWidget,
      );
      expect(find.textContaining('Copy'), findsNothing);
    });

    testWidgets('on Android, a code cut short is copied again', (tester) async {
      await paste(tester, TargetPlatform.android, _code.substring(0, 80));
      expect(
        find.text(
          "That's only part of a Claktory code. Copy all of it again and "
          'paste it here.',
        ),
        findsOneWidget,
      );
    });

    testWidgets('on an iPhone, a move code names the Mac', (tester) async {
      await paste(tester, TargetPlatform.iOS, 'claktory-move://?e=1&s=2');
      expect(
        find.text(
          "That's a code for moving Claktory, not a code to connect this "
          'iPhone. To get one, on your Mac open Settings → Devices and '
          'choose Add a device.',
        ),
        findsOneWidget,
      );
    });

    testWidgets('a code wrapped by a messenger connects', (tester) async {
      await paste(
        tester,
        TargetPlatform.android,
        'Code:\n${_code.replaceFirst('a1a1', 'a1\na1')}\nthanks',
      );
      expect(find.textContaining("That's"), findsNothing);
      expect(find.textContaining("isn't"), findsNothing);
      await tester.tap(find.text('Connect'));
      await pumpFrames(tester);
      expect(find.text('Connect to this Claktory?'), findsOneWidget);
    });
  });

  test('pairingProblem says what a scanned code is', () {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    expect(
      pairingProblem('claktory://add-friend#K7Q2', scanned: true),
      startsWith("That's a friend ID, not a code to connect this "),
    );
  });
}
