// A computer asking to join the account, drawn as it looks in Needs you on
// a Mac and on an iPhone, to check: waiting with Approve, on a device that
// can't approve, two at once, and approved. With SHOTS set, each is drawn
// to a PNG there:
//   SHOTS=/tmp/join flutter test test/account_join_shots_test.dart
// Without it, each still lays out without an overflow or an exception.
// test-bundle: alone (loads the app's fonts)
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/account_join.dart';
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/device_account.dart'
    show AccountClaktory, DeviceAccount;
import 'package:factory_app/device_approval.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;

Future<void> _shot(WidgetTester tester, String name) async {
  expect(tester.takeException(), isNull, reason: name);
  final folder = Platform.environment['SHOTS'];
  if (folder == null) return;
  final boundary = tester.binding.renderViews.first;
  final layer = boundary.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(
    () => layer.toImage(Offset.zero & boundary.size, pixelRatio: 2),
  );
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}

class _Joins implements JoinApprovals {
  _Joins({this.state = 'waiting', this.mayApprove = true});

  final String state;
  final bool mayApprove;
  var decided = false;

  @override
  bool get signedIn => true;

  @override
  Listenable get changes => const _Never();

  @override
  String get email => 'matt@example.com';

  @override
  Future<List<JoinRequest>> requests() async => [
    JoinRequest(
      id: 'req-1',
      state: state,
      expiresIn: 530,
      ip: '203.0.113.4',
      network: 'Hetzner Online',
      place: 'Falkenstein, Germany',
      name: 'nixos',
      platform: 'linux-x86_64',
      version: 'v0.2.0',
      fingerprint: '7f3a9c01d2e4b5a6',
      mayApprove: mayApprove,
    ),
  ];

  @override
  Future<void> decide(String id, Map<String, Object?> decision) async =>
      decided = true;

  @override
  Future<List<AccountClaktory>> claktories() async => [
    if (decided)
      const AccountClaktory(
        id: 'new-1',
        name: 'nixos',
        fingerprint: '7f3a9c01d2e4b5a6',
        endpointId:
            'cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34cd34',
      ),
  ];
}

class _Never implements Listenable {
  const _Never();

  @override
  void addListener(VoidCallback listener) {}

  @override
  void removeListener(VoidCallback listener) {}
}

class _Approver implements DeviceApprover {
  @override
  Future<void> confirmOwner({required String reason}) async {}

  @override
  Future<String> approveMove({
    required DeviceAccount account,
    required String requestId,
    required String accountId,
    required String sourceClaktoryId,
    required String endpoint,
    required String fingerprint,
    required int iat,
    required int exp,
    required String reason,
  }) => throw UnimplementedError();
}

void main() {
  setUpAll(loadGeist);
  setUp(() {
    forgetSettledJoins();
    // Dark, as Lucy checks them; no debug banner.
    final dispatcher = TestWidgetsFlutterBinding.instance.platformDispatcher;
    dispatcher.platformBrightnessTestValue = Brightness.dark;
    addTearDown(dispatcher.clearPlatformBrightnessTestValue);
    WidgetsApp.debugAllowBannerOverride = false;
    addTearDown(() => WidgetsApp.debugAllowBannerOverride = true);
  });

  for (final phone in [false, true]) {
    final where = phone ? 'iphone' : 'mac';
    for (final (scene, joins) in [
      ('waiting', _Joins()),
      ('cant-approve', _Joins(mayApprove: false)),
      ('two-at-once', _Joins(state: 'two_at_once')),
      ('approved', _Joins()),
    ]) {
      testWidgets('$scene on the $where', (tester) async {
        debugDefaultTargetPlatformOverride = phone
            ? TargetPlatform.iOS
            : TargetPlatform.macOS;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        deviceApprover = _Approver();
        final harness = Harness(
          Workspace(
            settings: Settings(name: 'Studio'),
            account: ClaktoryAccount(
              state: ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_IN,
              email: 'matt@example.com',
              authorizationsWaiting: ['req-1'],
            ),
          ),
        );
        JoinWatch.of(harness.backend).watchWith(joins);
        await harness.show(
          tester,
          size: phone ? const Size(393, 852) : const Size(1100, 800),
        );
        await pumpFrames(tester);
        harness.page.value = AppPage.inbox;
        await pumpFrames(tester);
        await tester.pump();
        await pumpFrames(tester);
        await _shot(tester, '$where-$scene-row');
        await tester.tap(
          find.text('A new Claktory asks to join your account').first,
        );
        await pumpFrames(tester, const Duration(seconds: 1));
        if (scene == 'approved') {
          final approve = find.widgetWithText(Button, 'Approve');
          await tester.ensureVisible(approve);
          await pumpFrames(tester);
          await tester.tap(approve);
          await pumpFrames(tester, const Duration(seconds: 1));
        }
        await _shot(tester, '$where-$scene');
      });
    }
  }
}
