import 'package:factory_app/message_time.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show builder, workspace;
import 'harness.dart';

int _ms(DateTime at) => at.millisecondsSinceEpoch;

Entry _user(int seq, String text, DateTime at) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(_ms(at)),
  user: UserText(text: text),
);

Entry _reply(int seq, String text, DateTime at) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(_ms(at)),
  assistant: AssistantText(text: text),
);

Entry _tool(int seq, DateTime at) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(_ms(at)),
  toolCall: ToolCall(
    callId: 'c$seq',
    name: 'bash',
    input: '{"command": "ls"}',
    state: ToolCallState.TOOL_CALL_STATE_DONE,
  ),
);

Future<Harness> _show(
  WidgetTester tester,
  List<Entry> entries, {
  bool h24 = true,
}) async {
  tester.platformDispatcher.alwaysUse24HourFormatTestValue = h24;
  addTearDown(tester.platformDispatcher.clearAlwaysUse24HourTestValue);
  final harness = Harness(workspace(agents: [builder(working: false)]));
  harness.backend.transcript.load(Transcript(agentId: 'a1', entries: entries));
  await harness.show(tester);
  await settle(tester);
  return harness;
}

RenderTimeFoot _foot(WidgetTester tester, String text) =>
    tester.renderObject<RenderTimeFoot>(
      find
          .ancestor(
            of: find.text(text, findRichText: true),
            matching: find.byWidgetPredicate(
              (w) => w.runtimeType.toString() == '_TimeFoot',
            ),
          )
          .first,
    );

void main() {
  test('the time is a 24-hour clock; the day line names the day', () {
    final now = DateTime(2026, 11, 21, 16, 30);
    expect(clockTime(_ms(DateTime(2026, 11, 21, 9, 5))), '09:05');
    expect(dayLine(_ms(DateTime(2026, 11, 21, 0, 1)), now), 'Today');
    expect(dayLine(_ms(DateTime(2026, 11, 20, 23, 59)), now), 'Yesterday');
    expect(dayLine(_ms(DateTime(2026, 11, 17, 12)), now), 'Tuesday');
    expect(dayLine(_ms(DateTime(2026, 11, 14, 12)), now), 'Sat 14 Nov');
    expect(dayLine(_ms(DateTime(2025, 12, 30, 12)), now), 'Tue 30 Dec 2025');
  });

  testWidgets('each message has its time at its foot, beside a short last '
      'line and under a full one', (tester) async {
    final today = DateTime.now();
    final at = DateTime(today.year, today.month, today.day, 9, 41);
    await _show(tester, [
      _user(1, 'Fix it', at),
      _reply(2, 'Done.', at.add(const Duration(minutes: 1))),
      _reply(
        3,
        List.filled(60, 'word').join(' '),
        at.add(const Duration(minutes: 2)),
      ),
      _tool(4, at.add(const Duration(minutes: 3))),
    ]);
    expect(find.text('09:41'), findsOneWidget);
    expect(find.text('09:42'), findsOneWidget);
    expect(find.text('09:43'), findsOneWidget);
    // Not on a tool call: messages only.
    expect(find.text('09:44'), findsNothing);
    expect(find.bySemanticsLabel(RegExp('at 09:41')), findsWidgets);
    // A short message keeps its height: the time sits on its line.
    expect(_foot(tester, 'Fix it').inline, isTrue);
    expect(_foot(tester, 'Done.').inline, isTrue);
    final time = tester.getRect(find.text('09:42'));
    final words = tester.getRect(find.text('Done.', findRichText: true));
    // On the words' one line, at its end.
    expect((time.center.dy - words.center.dy).abs(), lessThan(6));
    expect(time.left, greaterThan(words.left + 30));
  });

  testWidgets("on a 12-hour clock it's the device's", (tester) async {
    final today = DateTime.now();
    final at = DateTime(today.year, today.month, today.day, 14, 5);
    await _show(tester, [_user(1, 'Fix it', at)], h24: false);
    expect(find.text('2:05 PM'), findsOneWidget);
    expect(clockTime(_ms(DateTime(2026, 1, 1, 0, 7)), h24: false), '12:07 AM');
  });

  testWidgets('the time is no part of a selection', (tester) async {
    final today = DateTime.now();
    await _show(tester, [_user(1, 'Fix it', today)]);
    expect(
      find.ancestor(
        of: find.text(clockTime(_ms(today))),
        matching: find.byType(SelectionContainer),
      ),
      findsWidgets,
    );
    final container = tester.widget<SelectionContainer>(
      find
          .ancestor(
            of: find.text(clockTime(_ms(today))),
            matching: find.byType(SelectionContainer),
          )
          .first,
    );
    expect(container.delegate, isNull, reason: 'disabled');
  });

  testWidgets('a day line marks where the day changes', (tester) async {
    final now = DateTime.now();
    final today = DateTime(now.year, now.month, now.day, 0, 30);
    final yesterday = today.subtract(const Duration(hours: 2));
    await _show(tester, [
      _user(1, 'Late one', yesterday),
      _reply(2, 'Still here.', yesterday.add(const Duration(minutes: 5))),
      _user(3, 'Morning', today),
      _reply(4, 'Hi.', today.add(const Duration(minutes: 1))),
    ]);
    expect(find.text('Yesterday'), findsOneWidget);
    expect(find.text('Today'), findsOneWidget);
    // Above the first message of each day.
    expect(
      tester.getRect(find.text('Yesterday')).bottom,
      lessThan(tester.getRect(find.text('Late one', findRichText: true)).top),
    );
    final day = tester.getRect(find.text('Today'));
    expect(
      day.top,
      greaterThan(
        tester.getRect(find.text('Still here.', findRichText: true)).bottom,
      ),
    );
    expect(
      day.bottom,
      lessThan(tester.getRect(find.text('Morning', findRichText: true)).top),
    );
  });

  testWidgets('a last line with no room left puts the time under it', (
    tester,
  ) async {
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Align(
          alignment: Alignment.topLeft,
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 200),
            child: TimeFoot(
              ms: _ms(DateTime(2026, 1, 1, 9, 41)),
              color: const Color(0xFF888888),
              // Its last line ends at the right edge.
              child: const SizedBox(
                width: 200,
                child: Text('Done', textAlign: TextAlign.right),
              ),
            ),
          ),
        ),
      ),
    );
    final foot = tester.renderObject<RenderTimeFoot>(
      find.byWidgetPredicate((w) => w.runtimeType.toString() == '_TimeFoot'),
    );
    expect(foot.inline, isFalse);
    final words = tester.getRect(find.text('Done'));
    final time = tester.getRect(find.text('09:41'));
    expect(time.top, greaterThanOrEqualTo(words.bottom));
    expect(time.right, 200);
  });
}
