// An iPad's paperclip menu (Choose from Photos, Choose from Files, Paste
// image) is a popover, which iOS ends the app for when it has nothing to
// point at (Zucchero row 630). The message box's paperclip and a card's
// say where they are: pickAttachments carries the button's rect in the
// window, which the native side points the menu at (and the middle,
// arrowless, when there's none).
import 'package:factory_app/card_files.dart' show CardAttachButton;
import 'package:factory_app/controls.dart' show GlyphButton;
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/shell.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'ipad/as_ipad.dart';
import 'panes_test.dart' show leaf, snapshot;

/// factory/device answering pickAttachments with nothing (cancelled); the
/// arguments of each call.
List<Object?> _picker() {
  final calls = <Object?>[];
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  messenger.setMockMethodCallHandler(const MethodChannel('factory/device'), (
    call,
  ) async {
    if (call.method != 'pickAttachments') return null;
    calls.add(call.arguments);
    return <Object?>[];
  });
  addTearDown(
    () => messenger.setMockMethodCallHandler(
      const MethodChannel('factory/device'),
      null,
    ),
  );
  return calls;
}

List<double> _rectOf(WidgetTester tester, Finder finder) {
  final rect = tester.getRect(finder);
  return [rect.left, rect.top, rect.width, rect.height];
}

void main() {
  testWidgets("an iPad's message box paperclip: the menu points at it", (
    tester,
  ) async {
    asIPad();
    final calls = _picker();
    final focus = WorkbenchFocus();
    addTearDown(focus.dispose);
    await Harness(snapshot(layout: leaf('chat', 'chat'))).show(
      tester,
      size: const Size(1376, 1032),
      home: WorkbenchScope(
        focus: focus,
        child: const Shell(rail: SizedBox(width: 56)),
      ),
    );
    await pumpFrames(tester);
    final target = find.byWidgetPredicate(
      (w) => w is GlyphButton && w.label == 'Attach files',
    );
    expect(target, findsOneWidget);
    await tester.tap(target);
    await pumpFrames(tester);
    expect(calls, hasLength(1));
    expect((calls.single! as Map)['rect'], _rectOf(tester, target));
  });

  testWidgets("an iPad's card paperclip: the menu points at it", (
    tester,
  ) async {
    asIPad();
    final calls = _picker();
    await Harness(Workspace()).show(
      tester,
      size: const Size(1376, 1032),
      home: Align(
        alignment: const Alignment(0.6, 0.4),
        child: CardAttachButton(card: 'card', enabled: true, onNote: (_) {}),
      ),
    );
    await pumpFrames(tester);
    await tester.tap(find.byType(CardAttachButton));
    await pumpFrames(tester);
    expect(calls, hasLength(1));
    expect(
      (calls.single! as Map)['rect'],
      _rectOf(tester, find.byType(CardAttachButton)),
    );
  });
}
