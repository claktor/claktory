// The login request and the payment request drawn as they look, dark, in
// the agent's conversation on a Mac, an iPhone and an iPad. With SHOTS set,
// each is drawn to a PNG there:
//   SHOTS=/tmp/browser flutter test test/browser_requests_shots_test.dart
// Without it, each still lays out without an overflow or an exception.
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/controls.dart' show TextField;
import 'package:factory_app/mobile/device.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;

Workspace _workspace() => Workspace(
  projects: [Project(id: 'p1', name: 'Social')],
  agents: [
    Agent(
      id: 'yukako',
      name: 'Yukako',
      role: 'Social',
      color: 'pink',
      projectId: 'p1',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
      waitingSeqs: [Int64(4)],
    ),
  ],
  selectedAgentId: 'yukako',
  vault: VaultState(exists: true, unlocked: true),
);

SecretRequest _login({
  SecretLoginOutcome outcome =
      SecretLoginOutcome.SECRET_LOGIN_OUTCOME_UNSPECIFIED,
  String site = 'x.com',
  bool fits = false,
  bool lookalike = false,
  bool replaces = false,
  SecretRequestState state = SecretRequestState.SECRET_REQUEST_STATE_PENDING,
}) => SecretRequest(
  id: 'r1',
  agentId: 'yukako',
  state: state,
  newSecret: NewSecret(
    name: 'X login',
    variables: [
      SecretVariable(name: 'USERNAME'),
      SecretVariable(name: 'PASSWORD'),
    ],
  ),
  sites: ['https://$site'],
  fillOnly: true,
  login: true,
  loginSite: site,
  loginOutcome: outcome,
  fittingSecretId: fits ? 's1' : '',
  fittingSecretName: fits ? 'X login' : '',
  lookalikeSecretName: lookalike ? 'X login' : '',
  lookalikeSite: lookalike ? 'x.com' : '',
  givenSecretId: replaces ? 's1' : '',
  givenSecretName:
      replaces || state != SecretRequestState.SECRET_REQUEST_STATE_PENDING
      ? 'X login'
      : '',
  added: !fits,
  decidedMs: Int64(
    state == SecretRequestState.SECRET_REQUEST_STATE_PENDING
        ? 0
        : 1764240000000,
  ),
);

Entry _secret(SecretRequest request) =>
    Entry(seq: Int64(4), timestampMs: Int64(1004), secretRequest: request);

Entry _payment({int openedMs = 0}) => Entry(
  seq: Int64(4),
  timestampMs: Int64(1004),
  askPerson: AskPerson(
    id: 'ask4',
    title: 'Pay for Dev Days',
    state: AskPersonState.ASK_PERSON_STATE_PENDING,
    revision: 1,
    handoff: ErrandHandoff(
      errandId: 'e1',
      kind: ErrandHandoffKind.ERRAND_HANDOFF_KIND_PAYMENT,
      what: 'Dev Days registration, €120. Everything else is filled in.',
      origin: 'https://pay.stripe.com',
      url: 'https://pay.stripe.com/c/pay/cs_live_a1',
      domain: 'stripe.com',
      seq: Int64(4),
      openedMs: Int64(openedMs),
    ),
  ),
);

Future<void> _shot(WidgetTester tester, String name) async {
  expect(tester.takeException(), isNull);
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

enum _Device { mac, iphone, ipad }

Size _size(_Device device) => switch (device) {
  _Device.mac => const Size(1200, 800),
  _Device.iphone => const Size(393, 852),
  _Device.ipad => const Size(1180, 820),
};

void main() {
  setUpAll(loadGeist);
  setUp(() {
    final dispatcher = TestWidgetsFlutterBinding.instance.platformDispatcher;
    dispatcher.platformBrightnessTestValue = Brightness.dark;
    addTearDown(dispatcher.clearPlatformBrightnessTestValue);
    WidgetsApp.debugAllowBannerOverride = false;
    addTearDown(() => WidgetsApp.debugAllowBannerOverride = true);
  });

  Future<Harness> open(WidgetTester tester, _Device device, Entry card) async {
    debugDefaultTargetPlatformOverride = device == _Device.mac
        ? TargetPlatform.macOS
        : TargetPlatform.iOS;
    if (device == _Device.ipad) {
      debugDeviceCapabilities = const DeviceCapabilities(idiom: Idiom.pad);
    }
    addTearDown(() {
      debugDefaultTargetPlatformOverride = null;
      debugDeviceCapabilities = null;
    });
    final harness = Harness(_workspace());
    harness.backend.transcript.load(
      Transcript(
        agentId: 'yukako',
        entries: [
          Entry(
            seq: Int64(2),
            timestampMs: Int64(1000),
            assistant: AssistantText(
              text: card.hasAskPerson()
                  ? "Everything's filled in; only the payment is left."
                  : "X wants a login. I've asked for it below; Claktory types "
                        'it in, and I never see the password.',
            ),
          ),
          card,
        ],
      ),
    );
    await harness.show(tester, size: _size(device));
    await pumpFrames(tester);
    return harness;
  }

  for (final device in _Device.values) {
    final at = device.name;
    testWidgets('first login, $at', (tester) async {
      await open(tester, device, _secret(_login()));
      await tester.enterText(
        find.byWidgetPredicate(
          (widget) =>
              widget is TextField &&
              widget.semanticsLabel == 'Email or username for x.com',
        ),
        '@claktory',
      );
      await pumpFrames(tester);
      await _shot(tester, 'login-first-$at');
    });

    testWidgets('one of yours fits, $at', (tester) async {
      await open(tester, device, _secret(_login(fits: true)));
      await _shot(tester, 'login-fits-$at');
    });

    testWidgets('payment, $at', (tester) async {
      await open(tester, device, _payment());
      await _shot(tester, 'payment-$at');
    });

    testWidgets('payment opened, $at', (tester) async {
      await open(tester, device, _payment(openedMs: 5000));
      await _shot(tester, 'payment-opened-$at');
    });
  }

  for (final (name, request) in [
    (
      'refused',
      _login(outcome: SecretLoginOutcome.SECRET_LOGIN_OUTCOME_REFUSED),
    ),
    (
      'refused-saved',
      _login(
        outcome: SecretLoginOutcome.SECRET_LOGIN_OUTCOME_REFUSED,
        replaces: true,
      ),
    ),
    ('lookalike', _login(site: 'x-login.com', lookalike: true)),
    (
      'signing-in',
      _login(
        outcome: SecretLoginOutcome.SECRET_LOGIN_OUTCOME_SIGNING_IN,
        state: SecretRequestState.SECRET_REQUEST_STATE_GRANTED,
      ),
    ),
    (
      'signed-in',
      _login(
        outcome: SecretLoginOutcome.SECRET_LOGIN_OUTCOME_SIGNED_IN,
        state: SecretRequestState.SECRET_REQUEST_STATE_GRANTED,
      ),
    ),
    (
      'code-next',
      _login(
        outcome: SecretLoginOutcome.SECRET_LOGIN_OUTCOME_CODE_NEXT,
        state: SecretRequestState.SECRET_REQUEST_STATE_GRANTED,
      ),
    ),
  ]) {
    testWidgets('$name, mac', (tester) async {
      await open(tester, _Device.mac, _secret(request));
      await _shot(tester, 'login-$name-mac');
    });
  }
}
