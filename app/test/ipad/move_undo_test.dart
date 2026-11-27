// An iPad with a keyboard: after a move in the sidebar, ⌘Z puts it back
// while its row offers Undo, and the system's ⌘ menu lists Undo Move
// then; a text field with the keys keeps ⌘Z for its own undo.
import 'package:factory_app/ipad/commands.dart' show CommandMenu;
import 'package:factory_app/ipad/workbench.dart'
    show WorkbenchFocus, WorkbenchScope;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import 'as_ipad.dart';

const _channel = MethodChannel(CommandMenu.channelName);

/// What the system's menu was last given, by id: enabled or not.
final _menu = <String, bool>{};

Agent _agent(String id, String name, {String lead = ''}) => Agent(
  id: id,
  name: name,
  projectId: 'p1',
  leadId: lead,
  parentId: lead,
  modelAvailable: true,
);

Workspace _workspace() => Workspace(
  projects: [Project(id: 'p1', name: 'Claktory')],
  agents: [
    _agent('mike', 'Mike'),
    _agent('sam', 'Sam', lead: 'mike'),
    _agent('solo', 'Solo'),
  ],
  selectedAgentId: 'mike',
);

Future<Harness> _show(WidgetTester tester) async {
  asIPad();
  _menu.clear();
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  messenger.setMockMethodCallHandler(_channel, (call) async {
    if (call.method != 'setCommands') return null;
    _menu.clear();
    for (final command in (call.arguments as Map)['commands'] as List) {
      final map = command as Map;
      _menu[map['id'] as String] = map['enabled'] as bool;
    }
    return true;
  });
  addTearDown(() => messenger.setMockMethodCallHandler(_channel, null));
  final harness = Harness(_workspace());
  harness.backend.arranged = Arranged(token: 'tok-1');
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  await harness.show(
    tester,
    size: const Size(1366, 1024),
    home: WorkbenchScope(
      focus: focus,
      child: const Shell(rail: SizedBox(width: 56)),
    ),
  );
  await settle(tester);
  return harness;
}

/// Drags [name] onto [onto]'s row with a trackpad's pointer.
Future<void> _drag(WidgetTester tester, String name, String onto) async {
  final from = tester.getCenter(find.text(name).first);
  final target = tester.getCenter(find.text(onto).first);
  final gesture = await tester.startGesture(
    from,
    kind: PointerDeviceKind.mouse,
  );
  await tester.pump(const Duration(milliseconds: 50));
  for (var i = 1; i <= 10; i++) {
    await gesture.moveTo(Offset.lerp(from, target, i / 10)!);
    await tester.pump(const Duration(milliseconds: 16));
  }
  await gesture.up();
  await settle(tester);
}

Future<void> _commandZ(WidgetTester tester) async {
  await tester.sendKeyDownEvent(LogicalKeyboardKey.metaLeft);
  await tester.sendKeyEvent(LogicalKeyboardKey.keyZ);
  await tester.sendKeyUpEvent(LogicalKeyboardKey.metaLeft);
  await settle(tester);
}

void main() {
  testWidgets('⌘Z and the menu\'s Undo Move put a move back while it\'s '
      'offered, never from a text field', (tester) async {
    final harness = await _show(tester);
    expect(_menu.containsKey('undo-move'), isFalse);

    await _drag(tester, 'Solo', 'Sam');
    expect(find.text('Now reports to Sam'), findsOneWidget);
    expect(_menu['undo-move'], isTrue);

    // The message box has the keys: ⌘Z is its own.
    await tester.tap(find.byType(EditableText).first);
    await tester.pump();
    await tester.pump();
    expect(_menu['undo-move'], isFalse);
    final before = harness.backend.sent.length;
    await _commandZ(tester);
    expect(
      harness.backend.sent.skip(before).where((m) => m.hasRestoreArrangement()),
      isEmpty,
    );

    FocusManager.instance.primaryFocus?.unfocus();
    await tester.pump();
    await tester.pump();
    expect(_menu['undo-move'], isTrue);
    await _commandZ(tester);
    expect(harness.last.restoreArrangement.token, 'tok-1');
    expect(find.text('Now reports to Sam'), findsNothing);
    await tester.pump();
    expect(_menu.containsKey('undo-move'), isFalse);
  });

  testWidgets('Undo Move leaves the menu once the 8 seconds are up', (
    tester,
  ) async {
    await _show(tester);
    await _drag(tester, 'Solo', 'Sam');
    expect(_menu['undo-move'], isTrue);
    await tester.pump(const Duration(seconds: 8));
    await settle(tester);
    expect(_menu.containsKey('undo-move'), isFalse);
  });

  testWidgets('a message box let go of leaves the iPad nothing to undo, so '
      '⌘Z isn\'t spent on it', (tester) async {
    final undoState = <Object?>[];
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(SystemChannels.undoManager, (
      call,
    ) async {
      if (call.method == 'UndoManager.setUndoState') {
        undoState.add(call.arguments);
      }
      return null;
    });
    addTearDown(
      () =>
          messenger.setMockMethodCallHandler(SystemChannels.undoManager, null),
    );
    await _show(tester);
    await tester.tap(find.byType(EditableText).first);
    await tester.pump();
    await tester.enterText(find.byType(EditableText).first, 'Hello');
    await tester.pump(const Duration(milliseconds: 600));
    expect(undoState.last, {'canUndo': true, 'canRedo': false});

    FocusManager.instance.primaryFocus?.unfocus();
    await tester.pump();
    await tester.pump();
    expect(undoState.last, {'canUndo': false, 'canRedo': false});
  });
}
