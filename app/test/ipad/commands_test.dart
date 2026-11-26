import 'dart:convert';
import 'dart:io';

import 'package:factory_app/ipad/columns.dart';
import 'package:factory_app/ipad/commands.dart';
import 'package:factory_app/ipad/document_host.dart' show DocumentPanel;
import 'package:factory_app/ipad/settings_keys.dart';
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

/// The fixtures the native menu (UIKeyCommand) is built against.
Map<String, Object?> _fixture(String name) =>
    jsonDecode(File('test/ipad/fixtures/commands/$name').readAsStringSync())
        as Map<String, Object?>;

Agent _wren() => Agent(
  id: 'wren',
  name: 'Wren',
  layout: ViewNode(
    id: 'root',
    split: ViewSplit(
      axis: ViewAxis.VIEW_AXIS_ROW,
      children: [
        ViewNode(
          id: 'chat',
          leaf: ViewLeaf(kind: 'chat'),
        ),
        ViewNode(
          id: 'paper',
          leaf: ViewLeaf(kind: 'pdf', path: '/p.pdf'),
        ),
      ],
    ),
  ),
);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel(CommandMenu.channelName);
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

  test('setCommands is what the fixture says, for a wide window with a '
      'paper open', () {
    final focus = WorkbenchFocus(menu: CommandMenu());
    addTearDown(focus.dispose);
    // The paper is being read: Find… searches it.
    focus.focus('wren', 'paper');
    final commands = workbenchCommands(
      tier: WorkbenchTier.full,
      focus: focus,
      agent: _wren(),
      showPanes: () {},
    );
    final fixture = _fixture('set_commands.json');
    expect(fixture['method'], 'setCommands');
    expect(encodeCommands(commands), fixture['arguments']);
  });

  test('Find… opens the search of the PDF being read', () {
    final focus = WorkbenchFocus(menu: CommandMenu());
    addTearDown(focus.dispose);
    final wren = _wren();
    focus.focus('wren', 'paper');
    final find = workbenchCommands(
      tier: WorkbenchTier.full,
      focus: focus,
      agent: wren,
    ).singleWhere((command) => command.id == 'find');
    find.run!();
    final paper = wren.layout.split.children.last;
    expect(focus.documents.of('wren', paper).panel, DocumentPanel.search);

    // The conversation being read: ⌘F finds in it.
    focus.focus('wren', 'chat');
    final inConversation = workbenchCommands(
      tier: WorkbenchTier.full,
      focus: focus,
      agent: wren,
    ).singleWhere((command) => command.id == 'find');
    expect(inConversation.title, 'Find');
    expect(inConversation.enabled, isTrue);
  });

  test('keys that aren\'t a letter go to native as UIKit names them', () {
    final inputs =
        _fixture('special_keys.json')['inputs']! as Map<String, Object?>;
    const keys = {
      'enter': LogicalKeyboardKey.enter,
      'backspace': LogicalKeyboardKey.backspace,
      'tab': LogicalKeyboardKey.tab,
      'arrowUp': LogicalKeyboardKey.arrowUp,
      'arrowDown': LogicalKeyboardKey.arrowDown,
      'arrowLeft': LogicalKeyboardKey.arrowLeft,
      'arrowRight': LogicalKeyboardKey.arrowRight,
    };
    expect(keys.keys.toSet(), inputs.keys.toSet());
    for (final MapEntry(:key, :value) in keys.entries) {
      expect(keyCommandInput(value), inputs[key], reason: key);
    }
    expect(keyCommandInput(LogicalKeyboardKey.keyJ), 'j');
    expect(keyCommandInput(LogicalKeyboardKey.digit1), '1');
  });

  test('with nothing to cycle, the pane commands are disabled, not left '
      'out', () {
    final focus = WorkbenchFocus(menu: CommandMenu());
    addTearDown(focus.dispose);
    final encoded = encodeCommands(
      workbenchCommands(tier: WorkbenchTier.full, focus: focus, agent: null),
    );
    final commands = encoded['commands']! as List;
    expect(commands, hasLength(5));
    expect(
      [for (final c in commands) (c as Map)['enabled']],
      [true, false, false, false, false],
    );
  });

  test('once native takes the commands, it runs them by id and Flutter '
      'stands down', () async {
    final sent = <MethodCall>[];
    messenger.setMockMethodCallHandler(channel, (call) async {
      sent.add(call);
      return true;
    });
    addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
    final menu = CommandMenu();
    addTearDown(menu.dispose);
    var toggled = 0;
    final commands = [
      WorkbenchCommand(
        id: 'sidebar',
        title: 'Show Sidebar',
        keys: const SingleActivator(
          LogicalKeyboardKey.keyS,
          meta: true,
          control: true,
        ),
        run: () => toggled++,
      ),
    ];
    await menu.publish(commands);
    expect(menu.native, isTrue);
    // The same commands again aren't sent twice.
    await menu.publish(commands);
    expect(sent, hasLength(1));

    final run = _fixture('run.json');
    await messenger.handlePlatformMessage(
      CommandMenu.channelName,
      const StandardMethodCodec().encodeMethodCall(
        MethodCall(run['method']! as String, run['arguments']),
      ),
      (_) {},
    );
    expect(toggled, 1);
  });

  test('where nothing answers, Flutter keeps the keys', () async {
    final menu = CommandMenu();
    addTearDown(menu.dispose);
    await menu.publish(const []);
    expect(menu.native, isFalse);
  });

  testWidgets('Settings shown: ⌘F is Search Settings and puts the keys in '
      'its search; read beside another pane, ⌘F is that pane\'s', (
    tester,
  ) async {
    final focus = WorkbenchFocus(menu: CommandMenu());
    addTearDown(focus.dispose);
    var notified = 0;
    focus.addListener(() => notified++);
    var searched = 0;
    // Settings as settings_view.dart builds it: its Actions, then the
    // workbench's hook inside them.
    Widget settings() => Actions(
      actions: {
        FocusSettingsSearchIntent: CallbackAction<FocusSettingsSearchIntent>(
          onInvoke: (_) => searched++,
        ),
      },
      child: const WorkbenchSettings(child: SizedBox()),
    );
    WorkbenchCommand find(Agent? agent) => workbenchCommands(
      tier: WorkbenchTier.full,
      focus: focus,
      agent: agent,
    ).singleWhere((command) => command.id == 'find');

    // No Settings: Find…, with nothing to find.
    expect(find(null).title, 'Find');
    expect(find(null).enabled, isFalse);

    // Settings is the page shown.
    await tester.pumpWidget(WorkbenchScope(focus: focus, child: settings()));
    await tester.pump();
    expect(notified, greaterThan(0), reason: 'the menu hears of it');
    expect(find(null).title, 'Search Settings');
    find(null).run!();
    expect(searched, 1);

    // A pane in an agent's layout: Search Settings while it's the one read.
    final wren = _wren();
    wren.layout.split.children.add(
      ViewNode(
        id: 'settings',
        leaf: ViewLeaf(kind: 'settings'),
      ),
    );
    focus.focus('wren', 'settings');
    expect(find(wren).title, 'Search Settings');
    focus.focus('wren', 'paper');
    expect(find(wren).title, 'Find');
    expect(find(wren).enabled, isTrue, reason: "the PDF's search");
    focus.focus('wren', 'chat');
    expect(find(wren).title, 'Find');

    // Settings gone: Find… again.
    await tester.pumpWidget(
      WorkbenchScope(focus: focus, child: const SizedBox()),
    );
    await tester.pump();
    expect(focus.settings, isNull);
    expect(find(null).title, 'Find');
  });
}
