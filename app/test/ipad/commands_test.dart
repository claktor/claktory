import 'dart:convert';
import 'dart:io';

import 'package:factory_app/ipad/columns.dart';
import 'package:factory_app/ipad/commands.dart';
import 'package:factory_app/ipad/workbench.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart' show SingleActivator;
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

  test('with nothing to cycle, the pane commands are disabled, not left '
      'out', () {
    final focus = WorkbenchFocus(menu: CommandMenu());
    addTearDown(focus.dispose);
    final encoded = encodeCommands(
      workbenchCommands(tier: WorkbenchTier.full, focus: focus, agent: null),
    );
    final commands = encoded['commands']! as List;
    expect(commands, hasLength(4));
    expect(
      [for (final c in commands) (c as Map)['enabled']],
      [true, false, false, false],
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
}
