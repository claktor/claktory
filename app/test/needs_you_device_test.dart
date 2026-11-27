// A device waiting to be allowed on this Claktory is in Needs you: named
// as it calls itself, with what to do if it isn't the person's, and
// allowed only with the number its screen shows, typed here and checked
// by the backend.
import 'package:factory_app/controls.dart' show Button, ExtensionGlyph;
import 'package:factory_app/inbox.dart';
import 'package:factory_app/pairing.dart' show DeviceNumberField;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/mobile/metrics.dart' show TypeScale;
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

Workspace _workspace({List<PendingDevice>? devices}) => Workspace(
  agents: [
    Agent(
      id: 'rivet',
      name: 'Rivet',
      modelAvailable: true,
      status: AgentStatus.AGENT_STATUS_IDLE,
    ),
  ],
  remote: RemoteAccess(
    on: true,
    pendingDevices:
        devices ??
        [
          PendingDevice(
            deviceId: 'd4',
            name: 'Evil\u202ENa',
            number: '0427',
            requestedMs: Int64(DateTime.now().millisecondsSinceEpoch),
          ),
        ],
  ),
);

Finder _inNeedsYou(Finder finder) =>
    find.descendant(of: find.byType(InboxView), matching: finder);

Finder get _numberField => _inNeedsYou(
  find.descendant(
    of: find.byType(DeviceNumberField),
    matching: find.byType(EditableText),
  ),
);

void main() {
  test('one item per waiting device, while it waits', () {
    expect(needsYouCount(_workspace()), 1);
    expect(
      needsYouCount(
        _workspace(
          devices: [
            PendingDevice(deviceId: 'd4'),
            PendingDevice(deviceId: 'd5'),
          ],
        ),
      ),
      2,
    );
    expect(needsYouCount(_workspace(devices: [])), 0);
    expect(
      needsYouItems(_workspace()).single.key,
      'device:d4',
      reason: 'one of its own',
    );
  });

  testWidgets('it opens to Allow with the number typed; a wrong one says '
      'so, the right one allows it', (tester) async {
    final harness = Harness(_workspace());
    await harness.show(tester);
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);

    final line = _inNeedsYou(
      find.textContaining('Allow a new device?', findRichText: true),
    );
    expect(line, findsOneWidget);
    await tester.tap(line);
    await pumpFrames(tester);
    // Its own name, quoted as its words, with what reorders text shown by
    // its code; never the number it shows.
    expect(
      _inNeedsYou(find.textContaining('A device that calls itself “Evil')),
      findsWidgets,
    );
    expect(_inNeedsYou(find.textContaining('\u202E')), findsNothing);
    expect(_inNeedsYou(find.textContaining('0427')), findsNothing);
    expect(
      _inNeedsYou(
        find.text(
          "If you didn't just set up a new device, don't allow it. Someone "
          'may be using your email: secure it.',
        ),
      ),
      findsOneWidget,
    );
    expect(_inNeedsYou(find.text('Open conversation')), findsNothing);

    final allow = _inNeedsYou(find.widgetWithText(Button, 'Allow'));
    expect(tester.widget<Button>(allow).onPressed, isNull);
    harness.backend.refuse = "That's not the number Evil shows. 2 tries left.";
    await tester.enterText(_numberField, '1111');
    await pumpFrames(tester);
    await tester.tap(allow);
    await pumpFrames(tester);
    expect(harness.last.approveDevice.number, '1111');
    expect(
      _inNeedsYou(find.text("That's not the number Evil shows. 2 tries left.")),
      findsOneWidget,
    );

    harness.backend.refuse = null;
    await tester.enterText(_numberField, '0427');
    await pumpFrames(tester);
    await tester.tap(allow);
    await pumpFrames(tester);
    expect(harness.last.approveDevice.deviceId, 'd4');
    expect(harness.last.approveDevice.number, '0427');
    // Let in: it no longer waits, and says how it ended here.
    harness.backend.workspace.apply(_workspace(devices: []));
    await pumpFrames(tester);
    expect(needsYouCount(harness.backend.workspace.workspace), 0);
    expect(_inNeedsYou(find.text('Allowed.')), findsOneWidget);
  });

  testWidgets("Don't allow turns it away", (tester) async {
    final harness = Harness(_workspace());
    await harness.show(tester);
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    await tester.tap(
      _inNeedsYou(
        find.textContaining('Allow a new device?', findRichText: true),
      ),
    );
    await pumpFrames(tester);
    await tester.tap(_inNeedsYou(find.text("Don't allow")));
    await pumpFrames(tester);
    expect(harness.last.denyDevice.deviceId, 'd4');
  });

  testWidgets('a third wrong number: the backend turned it away, and its '
      'words stay once it no longer waits', (tester) async {
    final harness = Harness(_workspace());
    await harness.show(tester);
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    await tester.tap(
      _inNeedsYou(
        find.textContaining('Allow a new device?', findRichText: true),
      ),
    );
    await pumpFrames(tester);
    const away =
        "That's not the number Evil shows, a third time, so it was turned "
        'away. To add it, make a new code.';
    harness.backend.refuse = away;
    await tester.enterText(_numberField, '9999');
    await pumpFrames(tester);
    await tester.tap(_inNeedsYou(find.widgetWithText(Button, 'Allow')));
    await pumpFrames(tester);
    harness.backend.workspace.apply(_workspace(devices: []));
    await pumpFrames(tester);
    expect(_inNeedsYou(find.text(away)), findsOneWidget);
  });

  testWidgets('the same device asking again starts afresh: nothing typed, '
      'no error or ending from the last request (row 264)', (tester) async {
    final harness = Harness(_workspace());
    await harness.show(tester);
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    await tester.tap(
      _inNeedsYou(
        find.textContaining('Allow a new device?', findRichText: true),
      ),
    );
    await pumpFrames(tester);
    harness.backend.refuse = "That's not the number Evil shows. 2 tries left.";
    await tester.enterText(_numberField, '1111');
    await pumpFrames(tester);
    await tester.tap(_inNeedsYou(find.widgetWithText(Button, 'Allow')));
    await pumpFrames(tester);
    harness.backend.refuse = null;
    await tester.enterText(_numberField, '3903');
    await pumpFrames(tester);
    expect(tester.widget<EditableText>(_numberField).controller.text, '3903');

    // It stops waiting, then pairs again: a new number, a new time.
    harness.backend.workspace.apply(_workspace(devices: []));
    await pumpFrames(tester);
    harness.backend.workspace.apply(
      _workspace(
        devices: [
          PendingDevice(
            deviceId: 'd4',
            name: 'Evil',
            number: '1857',
            requestedMs: Int64(DateTime.now().millisecondsSinceEpoch + 1000),
          ),
        ],
      ),
    );
    await pumpFrames(tester);
    expect(tester.widget<EditableText>(_numberField).controller.text, isEmpty);
    expect(
      tester
          .widget<Button>(_inNeedsYou(find.widgetWithText(Button, 'Allow')))
          .onPressed,
      isNull,
    );
    expect(_inNeedsYou(find.textContaining('tries left')), findsNothing);
  });

  testWidgets('a tablet asking shows as a tablet', (tester) async {
    final harness = Harness(
      _workspace(
        devices: [
          PendingDevice(
            deviceId: 'd4',
            name: 'iPad',
            kind: DeviceKind.DEVICE_KIND_TABLET,
            number: '0427',
          ),
        ],
      ),
    );
    await harness.show(tester);
    await settle(tester);
    harness.page.value = AppPage.inbox;
    await settle(tester);
    expect(
      _inNeedsYou(
        find.byWidgetPredicate(
          (w) => w is ExtensionGlyph && w.icon == 'tablet',
        ),
      ),
      findsOneWidget,
    );
  });

  testWidgets('at the largest text size the field still shows all four '
      'digits (row 265)', (tester) async {
    late double plain;
    late double largest;
    late double digits;
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Builder(
          builder: (context) {
            plain = DeviceNumberField.widthFor(context, least: 120);
            return MediaQuery(
              data: MediaQuery.of(context)
                  .copyWith(textScaler: const TextScaler.linear(3.1)),
              child: Builder(
                builder: (context) {
                  largest = DeviceNumberField.widthFor(context, least: 120);
                  final painter = TextPainter(
                    text: TextSpan(
                      text: '0000',
                      style: TypeScale.of(context).body,
                    ),
                    textDirection: TextDirection.ltr,
                    textScaler: MediaQuery.textScalerOf(context),
                  )..layout();
                  digits = painter.width;
                  painter.dispose();
                  return const SizedBox();
                },
              ),
            );
          },
        ),
      ),
    );
    expect(plain, 120, reason: 'as before at the usual size');
    expect(largest, greaterThan(digits + 2 * 16));
  });
}
