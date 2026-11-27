// The payment request: Claktory's heading with the site it read, the
// agent's line quoted, Open on this device (the page in the person's own
// browser), then I've paid on every device once it's opened.
import 'package:factory_app/ask_card.dart';
import 'package:factory_app/devices.dart' show thisDeviceKind;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace() => Workspace(
  projects: [Project(id: 'p1', name: 'Factory')],
  agents: [
    Agent(
      id: 'koichi',
      name: 'Koichi',
      projectId: 'p1',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
      waitingSeqs: [Int64(4)],
    ),
  ],
  selectedAgentId: 'koichi',
);

const _url = 'https://pay.stripe.com/c/pay/cs_live_a1#fid=x';

Entry _payment({
  String url = _url,
  int openedMs = 0,
  AskPersonState state = AskPersonState.ASK_PERSON_STATE_PENDING,
}) => Entry(
  seq: Int64(4),
  timestampMs: Int64(1004),
  askPerson: AskPerson(
    id: 'ask4',
    title: 'Pay for Dev Days',
    state: state,
    revision: 1,
    decidedMs: Int64(state == AskPersonState.ASK_PERSON_STATE_PENDING ? 0 : 9),
    handoff: ErrandHandoff(
      errandId: 'e1',
      kind: ErrandHandoffKind.ERRAND_HANDOFF_KIND_PAYMENT,
      what: 'Dev Days registration, €120.',
      origin: 'https://pay.stripe.com',
      url: url,
      domain: 'stripe.com',
      seq: Int64(4),
      openedMs: Int64(openedMs),
    ),
  ),
);

Future<(Harness, List<Uri>)> _show(
  WidgetTester tester,
  Entry card, {
  bool opens = true,
}) async {
  final opened = <Uri>[];
  final before = openPaymentPage;
  openPaymentPage = (url) async {
    opened.add(url);
    return opens;
  };
  addTearDown(() => openPaymentPage = before);
  final harness = Harness(_workspace());
  harness.backend.transcript.load(
    Transcript(agentId: 'koichi', entries: [card]),
  );
  await harness.show(tester, size: const Size(1200, 900));
  await settle(tester);
  await settle(tester);
  return (harness, opened);
}

Finder get _open => find.textContaining('Open on this ');

Finder _rich(String text) => find.text(text, findRichText: true);

void main() {
  testWidgets("Claktory's heading, the agent's line quoted; Open on this Mac "
      'tells Claktory, then opens the page here as it came', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final (harness, opened) = await _show(tester, _payment());
    expect(find.text('Open on this Mac'), findsOneWidget);
    expect(_rich('Koichi asks you to pay on stripe.com'), findsOneWidget);
    expect(find.text('Koichi: “Dev Days registration, €120.”'), findsOneWidget);
    expect(find.text('Koichi waits until you pay.'), findsOneWidget);
    expect(find.text("Can't do this…"), findsOneWidget);
    expect(find.text("I've paid"), findsNothing);
    await tester.tap(_open);
    await settle(tester);
    final told = harness.backend.sent.lastWhere((m) => m.hasPaymentOpened());
    expect(told.paymentOpened.agentId, 'koichi');
    expect(told.paymentOpened.seq, Int64(4));
    expect(opened, [Uri.parse(_url)]);
    expect(harness.backend.sent.where((m) => m.hasAnswerAsk()), isEmpty);
  });

  testWidgets("opened on any device: I've paid answers it, with no words; "
      'Open again opens the page only', (tester) async {
    final (harness, opened) = await _show(tester, _payment(openedMs: 5000));
    expect(
      find.text("Opened in your browser. Come back here once you've paid."),
      findsOneWidget,
    );
    expect(_open, findsNothing);
    await tester.tap(find.text('Open again'));
    await settle(tester);
    expect(opened, [Uri.parse(_url)]);
    expect(harness.backend.sent.where((m) => m.hasPaymentOpened()), isEmpty);
    await tester.tap(find.text("I've paid"));
    await settle(tester);
    final answer = harness.backend.sent.lastWhere((m) => m.hasAnswerAsk());
    expect(answer.answerAsk.seq, Int64(4));
    expect(answer.answerAsk.dismiss, isFalse);
    expect(answer.answerAsk.text, isEmpty);
  });

  testWidgets("refused by Claktory: why, and nothing opens", (tester) async {
    final (harness, opened) = await _show(tester, _payment());
    harness.backend.refuse = "This Claktory can't open payments yet.";
    await tester.tap(_open);
    await settle(tester);
    expect(find.text("This Claktory can't open payments yet."), findsOneWidget);
    expect(opened, isEmpty);
  });

  testWidgets("the page didn't open: it says so", (tester) async {
    await _show(tester, _payment(), opens: false);
    await tester.tap(_open);
    await settle(tester);
    expect(find.text("The payment page didn't open here."), findsOneWidget);
  });

  testWidgets("no https page to open: only Can't do this…", (tester) async {
    await _show(tester, _payment(url: 'http://pay.example/checkout'));
    expect(_open, findsNothing);
    expect(find.text("Can't do this…"), findsOneWidget);
  });

  testWidgets("Can't do this… sends the reason chosen", (tester) async {
    final (harness, _) = await _show(tester, _payment());
    await tester.tap(find.text("Can't do this…"));
    await settle(tester);
    await tester.tap(find.text('Not now').last);
    await settle(tester);
    final answer = harness.backend.sent.lastWhere((m) => m.hasAnswerAsk());
    expect(answer.answerAsk.dismiss, isTrue);
    expect(answer.answerAsk.text, 'Not now');
  });

  testWidgets("Can't do this… in your own words", (tester) async {
    final (harness, _) = await _show(tester, _payment());
    await tester.tap(find.text("Can't do this…"));
    await settle(tester);
    await tester.tap(find.text('In my own words…').last);
    await settle(tester);
    await tester.enterText(find.byType(EditableText).first, 'Too expensive');
    await tester.pump();
    await tester.tap(find.text("Can't do this"));
    await settle(tester);
    final answer = harness.backend.sent.lastWhere((m) => m.hasAnswerAsk());
    expect(answer.answerAsk.dismiss, isTrue);
    expect(answer.answerAsk.text, 'Too expensive');
  });

  testWidgets('ended: Paid on the site, or not', (tester) async {
    await _show(
      tester,
      _payment(state: AskPersonState.ASK_PERSON_STATE_ANSWERED),
    );
    expect(
      find.textContaining('Paid on stripe.com', findRichText: true),
      findsOneWidget,
    );
  });

  testWidgets("an iPhone or iPad's: Opened in Safari", (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final (harness, _) = await _show(tester, _payment());
    expect(find.text('Open on this $thisDeviceKind'), findsOneWidget);
    await tester.tap(_open);
    await settle(tester);
    harness.backend.transcript.load(
      Transcript(agentId: 'koichi', entries: [_payment(openedMs: 5000)]),
    );
    await settle(tester);
    expect(
      find.text("Opened in Safari. Come back here once you've paid."),
      findsOneWidget,
    );
  });

  test('the Needs you row and how it ended say the site', () {
    final ask = _payment().askPerson;
    expect(askSummary(ask), 'Asks you to pay on stripe.com');
    expect(
      askOutcome(
        ask..state = AskPersonState.ASK_PERSON_STATE_DISMISSED,
        agent: 'Koichi',
      ),
      "Couldn't pay",
    );
  });
}
