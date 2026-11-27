// A Needs you card's action row and field on a phone (needs-you-cards.md
// §5.2, Bruno 42060): the thumb row (two buttons share it, the primary at
// the right and wider; three put the primary full width under the quiet
// two; the largest text sizes give each its own row, the primary last),
// the field's growth capped at the largest text sizes, and the field and
// buttons kept in sight above the keyboard in a conversation (Squalo's
// B10). On a Mac, the line at the left and the buttons at its right.
import 'package:factory_app/controls.dart' show Button;
import 'package:factory_app/mobile/lines.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'phone_sizes_test.dart' show loadGeist;

Widget _app(Widget child, {double scale = 1}) => Directionality(
  textDirection: TextDirection.ltr,
  child: Builder(
    builder: (context) => MediaQuery(
      data: MediaQuery.of(context)
          .copyWith(textScaler: TextScaler.linear(scale)),
      child: Center(
        child: SizedBox(
          width: 361,
          child: Builder(builder: (context) => child),
        ),
      ),
    ),
  ),
);

Widget _row(int quiet, {bool primary = true}) => CardActions(
  line: const Text('Step 2 stays open until Rivet answers it.'),
  quiet: [
    if (quiet > 0) Button(label: "Can't do this", onPressed: () {}),
    if (quiet > 1) Button(label: 'Done', onPressed: () {}),
  ],
  primary: primary
      ? Button(label: 'Send', primary: true, onPressed: () {})
      : null,
);

Rect _rect(WidgetTester tester, String label) =>
    tester.getRect(find.widgetWithText(Button, label));

Future<void> _phone(WidgetTester tester) async {
  debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
  tester.view.physicalSize = const Size(393 * 3, 852 * 3);
  tester.view.devicePixelRatio = 3;
  addTearDown(tester.view.reset);
}

void main() {
  // The app's font, so each label is as wide as on a phone.
  setUpAll(loadGeist);

  testWidgets('on a phone two buttons share a row, the primary at the '
      'right and wider', (tester) async {
    await _phone(tester);
    await tester.pumpWidget(_app(_row(1)));
    final no = _rect(tester, "Can't do this");
    final yes = _rect(tester, 'Send');
    expect(no.top, yes.top);
    expect(yes.left, greaterThan(no.right));
    expect(yes.width, greaterThan(no.width));
    expect(no.height, greaterThanOrEqualTo(44));
    expect(yes.height, no.height);
    // Under the line, the row's whole width.
    final line = tester.getRect(find.textContaining('Step 2'));
    expect(no.top, greaterThan(line.bottom));
    expect(yes.right - no.left, closeTo(361, 0.5));
  });

  testWidgets('on a phone three buttons: the quiet two share a row, the '
      'primary the whole width under them', (tester) async {
    await _phone(tester);
    await tester.pumpWidget(_app(_row(2)));
    final cant = _rect(tester, "Can't do this");
    final done = _rect(tester, 'Done');
    final send = _rect(tester, 'Send');
    expect(cant.top, done.top);
    expect(cant.width, closeTo(done.width, 0.5));
    expect(send.top, greaterThan(done.bottom));
    expect(send.width, closeTo(361, 0.5));
  });

  testWidgets('on a phone two quiet buttons and no primary share a row, '
      'with no line over them when there is none', (tester) async {
    await _phone(tester);
    await tester.pumpWidget(
      _app(
        CardActions(
          quiet: [
            Button(label: 'Dismiss', onPressed: () {}),
            Button(label: 'Add it here instead', onPressed: () {}),
          ],
        ),
      ),
    );
    final no = _rect(tester, 'Dismiss');
    final other = _rect(tester, 'Add it here instead');
    expect(no.top, other.top);
    // Halves, but a label never wraps: the longer takes what it needs.
    expect(other.width, greaterThanOrEqualTo(no.width));
    expect(
      tester.getSize(find.text('Add it here instead')).height,
      lessThan(30),
      reason: 'one line',
    );
    expect(other.right - no.left, closeTo(361, 0.5));
    expect(no.top, tester.getRect(find.byType(CardActions)).top);
  });

  testWidgets("on a phone a label never wraps: the no takes its label's "
      'width, the primary the rest, and when both cannot fit they stack, '
      'the primary at the bottom (Lucy)', (tester) async {
    await _phone(tester);
    Widget pair(String no, String yes) => _app(
      CardActions(
        quiet: [Button(label: no, onPressed: () {})],
        primary: Button(label: yes, primary: true, onPressed: () {}),
      ),
    );
    // Wider than its two fifths: it takes what its label needs.
    await tester.pumpWidget(pair('Not this time, thanks', 'Give'));
    var no = _rect(tester, 'Not this time, thanks');
    var yes = _rect(tester, 'Give');
    expect(no.top, yes.top);
    expect(no.width, greaterThan((361 - 8) * 2 / 5));
    expect(no.height, yes.height);
    expect(
      tester.getSize(find.text('Not this time, thanks')).height,
      lessThan(30),
      reason: 'one line',
    );
    // Both too long for one row: one a row, the primary under.
    await tester.pumpWidget(
      pair('Not this time, thanks', 'Add and give to Cioccolata'),
    );
    no = _rect(tester, 'Not this time, thanks');
    yes = _rect(tester, 'Add and give to Cioccolata');
    expect(yes.top, greaterThan(no.bottom));
    expect(no.width, closeTo(361, 0.5));
    expect(yes.width, closeTo(361, 0.5));
  });

  testWidgets('on a phone a primary alone takes the whole width', (
    tester,
  ) async {
    await _phone(tester);
    await tester.pumpWidget(_app(_row(0)));
    expect(_rect(tester, 'Send').width, closeTo(361, 0.5));
  });

  testWidgets('at the largest text sizes each button has its own row, the '
      'primary at the bottom, and the field grows to three lines', (
    tester,
  ) async {
    await _phone(tester);
    late BuildContext inside;
    await tester.pumpWidget(
      _app(
        Builder(
          builder: (context) {
            inside = context;
            return _row(2);
          },
        ),
        scale: 2,
      ),
    );
    final cant = _rect(tester, "Can't do this");
    final done = _rect(tester, 'Done');
    final send = _rect(tester, 'Send');
    expect(done.top, greaterThan(cant.bottom));
    expect(send.top, greaterThan(done.bottom));
    for (final rect in [cant, done, send]) {
      expect(rect.width, closeTo(361, 0.5));
    }
    expect(cardFieldMaxLines(inside), 3);
  });

  testWidgets('at the usual text size the field grows to six lines', (
    tester,
  ) async {
    await _phone(tester);
    late BuildContext inside;
    await tester.pumpWidget(
      _app(
        Builder(
          builder: (context) {
            inside = context;
            return const SizedBox();
          },
        ),
      ),
    );
    expect(cardFieldMaxLines(inside), 6);
  });

  testWidgets('on a Mac the line is at the left and the buttons together '
      'at its right, the primary last', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.view.physicalSize = const Size(1200, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      _app(
        SizedBox(
          width: 600,
          child: CardActions(
            line: const Text('Gives Hetzner.'),
            quiet: [Button(label: 'Dismiss', onPressed: () {})],
            primary: Button(label: 'Give', primary: true, onPressed: () {}),
          ),
        ),
      ),
    );
    final line = tester.getRect(find.text('Gives Hetzner.'));
    final no = _rect(tester, 'Dismiss');
    final yes = _rect(tester, 'Give');
    expect(no.center.dy, closeTo(line.center.dy, 1));
    expect(no.left, greaterThan(line.left));
    expect(yes.left, greaterThan(no.right));
    expect(no.width, lessThan(120));
  });

  testWidgets('in a conversation, a card field taking the keyboard brings '
      'its last button just above it (B10)', (tester) async {
    await _phone(tester);
    final focus = FocusNode();
    addTearDown(focus.dispose);
    final controller = TextEditingController();
    addTearDown(controller.dispose);
    // A conversation: newest at the bottom, its room what the keyboard
    // leaves, as the phone's page pads itself by it.
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Builder(
          builder: (context) => Padding(
            padding: EdgeInsets.only(
              bottom: MediaQuery.viewInsetsOf(context).bottom,
            ),
            child: ListView(
              reverse: true,
              children: [
                const SizedBox(height: 60, key: Key('composer')),
                KeepInSight(
                  focus: focus,
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      EditableText(
                        controller: controller,
                        focusNode: focus,
                        style: const TextStyle(fontSize: 17),
                        cursorColor: const Color(0xFF000000),
                        backgroundCursorColor: const Color(0xFF888888),
                      ),
                      _row(2),
                    ],
                  ),
                ),
                for (var i = 0; i < 30; i++)
                  SizedBox(height: 80, key: Key('$i')),
              ],
            ),
          ),
        ),
      ),
    );
    // Scrolled back so the card sits low, then the keyboard comes up over
    // its buttons.
    await tester.drag(find.byType(ListView), const Offset(0, 300));
    await tester.pumpAndSettle();
    focus.requestFocus();
    tester.view.viewInsets = const FakeViewPadding(bottom: 336 * 3);
    addTearDown(tester.view.resetViewInsets);
    for (var i = 0; i < 10; i++) {
      await tester.pump(const Duration(milliseconds: 50));
    }
    await tester.pumpAndSettle();
    final keyboardTop = 852 - 336.0;
    final send = _rect(tester, 'Send');
    expect(send.bottom, lessThanOrEqualTo(keyboardTop + 0.5));
    // Just above it: not scrolled further than it needs.
    expect(send.bottom, greaterThan(keyboardTop - 120));
  });

  testWidgets('on a small iPhone at large text, the buttons come first, then '
      'the line, then the field: a hint steps aside and the field shrinks '
      '(Zucchero row 631, path 5; Lucy 45566)', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.view.physicalSize = const Size(375 * 2, 667 * 2);
    tester.view.devicePixelRatio = 2;
    addTearDown(tester.view.reset);
    final focus = FocusNode();
    addTearDown(focus.dispose);
    final controller = TextEditingController(
      text: List.generate(8, (i) => 'Line ${i + 1} of what I wrote').join('\n'),
    );
    addTearDown(controller.dispose);
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Builder(
          builder: (context) => MediaQuery(
            data: MediaQuery.of(context)
                .copyWith(textScaler: const TextScaler.linear(2.35)),
            child: Builder(
              builder: (context) => Padding(
                padding: EdgeInsets.only(
                  bottom: MediaQuery.viewInsetsOf(context).bottom,
                ),
                child: ListView(
                  reverse: true,
                  children: [
                    KeepInSight(
                      focus: focus,
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          CardFieldRoom(
                            child: EditableText(
                              controller: controller,
                              focusNode: focus,
                              maxLines: null,
                              style: const TextStyle(fontSize: 17),
                              cursorColor: const Color(0xFF000000),
                              backgroundCursorColor: const Color(0xFF888888),
                            ),
                          ),
                          CardActions(
                            line: const Text('June sees what it prints.'),
                            quietLine: true,
                            quiet: [Button(label: 'Dismiss', onPressed: () {})],
                            primary: Button(
                              label: 'Execute',
                              primary: true,
                              onPressed: () {},
                            ),
                          ),
                        ],
                      ),
                    ),
                    for (var i = 0; i < 10; i++)
                      SizedBox(height: 80, key: Key('$i')),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
    expect(find.text('June sees what it prints.'), findsOneWidget);
    focus.requestFocus();
    tester.view.viewInsets = const FakeViewPadding(bottom: 260 * 2);
    addTearDown(tester.view.resetViewInsets);
    for (var i = 0; i < 10; i++) {
      await tester.pump(const Duration(milliseconds: 50));
    }
    await tester.pumpAndSettle();
    const keyboardTop = 667 - 260.0;
    // The hint stepped aside; the buttons sit above the keyboard.
    expect(find.text('June sees what it prints.'), findsNothing);
    final execute = _rect(tester, 'Execute');
    final dismiss = _rect(tester, 'Dismiss');
    expect(execute.bottom, lessThanOrEqualTo(keyboardTop + 0.5));
    expect(dismiss.top, greaterThanOrEqualTo(-0.5));
    // The field keeps at least a line in sight, above the buttons.
    final field = tester.getRect(find.byType(EditableText));
    final shown = field.intersect(const Rect.fromLTWH(0, 0, 375, keyboardTop));
    expect(shown.height, greaterThanOrEqualTo(17 * 2.35));
    // Keyboard down: the hint is back, the field whole again.
    focus.unfocus();
    tester.view.resetViewInsets();
    await tester.pumpAndSettle();
    expect(find.text('June sees what it prints.'), findsOneWidget);
  });
}
