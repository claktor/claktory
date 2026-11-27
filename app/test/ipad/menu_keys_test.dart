// A hardware keyboard's ⌘-commands on an iPad while the system's menu
// shows them (Zucchero's row 701): in the Simulator its key commands never
// fire, so ⌃⌘S left the sidebar as it was and focus mode's ⌘⇧C, ⌥⌘↓ and
// ⌘↩ did nothing, Flutter's own bindings having stood down. They're bound
// in Flutter whether or not the menu takes them, and a `run` the menu sends
// for the press Flutter already had isn't run again.
import 'package:factory_app/card_answer.dart' show AnswerDock, CardAnswer;
import 'package:factory_app/inbox.dart' show NeedsYouCard;
import 'package:factory_app/ipad/commands.dart' show CommandMenu;
import 'package:factory_app/ipad/workbench.dart'
    show WorkbenchFocus, WorkbenchScope;
import 'package:factory_app/needs_you_focus.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart' show AppPage;
import 'package:factory_app/shell.dart' show Shell;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import 'as_ipad.dart';

const _channel = MethodChannel(CommandMenu.channelName);

/// What the system's menu was last given, by id: enabled or not.
final _menu = <String, bool>{};

final _base = DateTime(2026, 11, 27, 9);

Entry _ask(int seq, int minutesAgo) => Entry(
  seq: Int64(seq),
  timestampMs: Int64(
    _base.subtract(Duration(minutes: minutesAgo)).millisecondsSinceEpoch,
  ),
  askPerson: AskPerson(id: 'q$seq', title: 'Question $seq'),
);

/// June waits on two cards, Mista on one (the newest).
Workspace _waiting() => Workspace(
  agents: [
    Agent(
      id: 'june',
      name: 'June',
      projectId: 'p1',
      waitingSeqs: [Int64(1), Int64(2)],
    ),
    Agent(id: 'mista', name: 'Mista', waitingSeqs: [Int64(7)]),
  ],
  projects: [Project(id: 'p1', name: 'Factory')],
);

/// The window as main.dart builds it, with a system's menu that takes the
/// commands, as the iPad's does.
Future<(Harness, WorkbenchFocus)> _show(WidgetTester tester, Size size) async {
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
  final harness = Harness(_waiting());
  harness.backend.entriesById
    ..['june/1'] = _ask(1, 30)
    ..['june/2'] = _ask(2, 60)
    ..['mista/7'] = _ask(7, 5);
  final focus = WorkbenchFocus();
  addTearDown(focus.dispose);
  await harness.show(
    tester,
    size: size,
    home: WorkbenchScope(
      focus: focus,
      child: const Shell(rail: SizedBox(width: 56)),
    ),
  );
  await settle(tester);
  return (harness, focus);
}

/// [key] with [modifiers] held, as a hardware keyboard sends it: the
/// left-hand modifier keys, as a real keyboard does (isMetaPressed and the
/// like know only the sided ones).
Future<void> _press(
  WidgetTester tester,
  LogicalKeyboardKey key, {
  List<LogicalKeyboardKey> modifiers = const [LogicalKeyboardKey.metaLeft],
}) async {
  for (final modifier in modifiers) {
    await tester.sendKeyDownEvent(modifier);
  }
  await tester.sendKeyEvent(key);
  for (final modifier in modifiers.reversed) {
    await tester.sendKeyUpEvent(modifier);
  }
  await settle(tester);
}

/// The system's menu running [id], as a key command or a menu item does.
Future<void> _menuRuns(WidgetTester tester, String id) async {
  await TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
      .handlePlatformMessage(
        CommandMenu.channelName,
        const StandardMethodCodec().encodeMethodCall(
          MethodCall('run', {'id': id}),
        ),
        (_) {},
      );
  await settle(tester);
}

void main() {
  testWidgets('⌃⌘S hides and shows the sidebar while the system\'s menu has '
      'it; the menu\'s run for that same press isn\'t a second one', (
    tester,
  ) async {
    final (_, focus) = await _show(tester, const Size(1194, 834));
    expect(_menu['sidebar'], isTrue, reason: 'the menu took the commands');
    expect(focus.menu.native, isTrue);
    expect(focus.sidebarHidden, isFalse);

    const chord = [LogicalKeyboardKey.controlLeft, LogicalKeyboardKey.metaLeft];
    await _press(tester, LogicalKeyboardKey.keyS, modifiers: chord);
    expect(focus.sidebarHidden, isTrue);
    // A device whose menu also fired for it: the same press, once.
    await _menuRuns(tester, 'sidebar');
    expect(focus.sidebarHidden, isTrue);
    // Chosen in the menu bar later: that runs.
    await _menuRuns(tester, 'sidebar');
    expect(focus.sidebarHidden, isFalse);

    await _press(tester, LogicalKeyboardKey.keyS, modifiers: chord);
    expect(focus.sidebarHidden, isTrue);
  });

  testWidgets('focus mode: ⌘⇧C shows and hides Context, ⌥⌘↓ and ⌥⌘↑ move '
      'between cards, ⌘↩ answers; in a field ⌘⇧C is the field\'s', (
    tester,
  ) async {
    // 11" portrait: Context isn't beside the card, so it starts closed.
    final (harness, _) = await _show(tester, const Size(834, 1194));
    setFocusMode(harness.backend, true);
    await settle(tester);
    final session = tester.widget<FocusStage>(find.byType(FocusStage)).session;
    expect(_menu['focus.context'], isTrue);
    expect(session.contextOpen, isFalse);

    const shiftCommand = [
      LogicalKeyboardKey.metaLeft,
      LogicalKeyboardKey.shiftLeft,
    ];
    await _press(tester, LogicalKeyboardKey.keyC, modifiers: shiftCommand);
    expect(session.contextOpen, isTrue);
    expect(find.byType(FocusContext), findsOneWidget);
    await _menuRuns(tester, 'focus.context');
    expect(session.contextOpen, isTrue, reason: 'the same press, once');
    await _press(tester, LogicalKeyboardKey.keyC, modifiers: shiftCommand);
    expect(session.contextOpen, isFalse);

    const altCommand = [
      LogicalKeyboardKey.metaLeft,
      LogicalKeyboardKey.altLeft,
    ];
    final first = session.current?.key;
    await _press(tester, LogicalKeyboardKey.arrowDown, modifiers: altCommand);
    final second = session.current?.key;
    expect(second, isNot(first));
    await _press(tester, LogicalKeyboardKey.arrowUp, modifiers: altCommand);
    expect(session.current?.key, first);

    // ⌘↩: the card's own yes, its answer sent.
    await _press(tester, LogicalKeyboardKey.enter);
    expect(harness.last.hasAnswerAsk(), isTrue);

    // The card's answer docked in the stage with its field (a stand-in, as
    // in focus_test).
    final field = TextEditingController();
    addTearDown(field.dispose);
    tester
        .element(find.byType(NeedsYouCard).first)
        .getInheritedWidgetOfExactType<AnswerDock>()!
        .answer
        .value = CardAnswer(
      docked: true,
      field: field,
      yes: 'Approve',
      no: 'Dismiss',
      onYes: () async {},
      onSubmit: () async {},
      onNo: (reason) async {},
    );
    await settle(tester);
    // In the card's field: ⌘⇧C is left to it, Context stays as it was.
    await tester.tap(
      find.descendant(
        of: find.byType(FocusStage),
        matching: find.byType(EditableText),
      ),
    );
    await settle(tester);
    expect(_menu['focus.context'], isFalse);
    await _press(tester, LogicalKeyboardKey.keyC, modifiers: shiftCommand);
    expect(session.contextOpen, isFalse);
  });

  testWidgets('⌘, opens Settings, from the keyboard or the system\'s menu '
      '(projects-sidebar.md §9)', (tester) async {
    final (harness, _) = await _show(tester, const Size(1194, 834));
    expect(_menu['settings'], isTrue, reason: 'the menu lists Settings');
    expect(harness.page.value, isNot(AppPage.settings));

    await _press(tester, LogicalKeyboardKey.comma);
    expect(harness.page.value, AppPage.settings);
    // The menu's run for that same press isn't a second one.
    await _menuRuns(tester, 'settings');
    expect(harness.page.value, AppPage.settings);

    harness.page.value = AppPage.agent;
    await settle(tester);
    // Chosen in the menu bar later: that opens it.
    await _menuRuns(tester, 'settings');
    expect(harness.page.value, AppPage.settings);
  });
}
