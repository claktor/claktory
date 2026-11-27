// Pinned agents on an iPad's keyboard: ⌘1 to ⌘9 open the first nine pins,
// named in the system's menu and its ⌘-hold list ("Open Bruno"), as
// clicking the pin does. Only pins that are there show; one whose agent
// went is skipped.
import 'package:factory_app/ipad/columns.dart';
import 'package:factory_app/ipad/commands.dart';
import 'package:factory_app/ipad/workbench.dart'
    show WorkbenchFocus, WorkbenchScope;
import 'package:factory_app/pinned_agents.dart' show pinnedAgents;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import 'as_ipad.dart';

const _channel = MethodChannel(CommandMenu.channelName);

/// The commands the system's menu was last given, by id.
final _menu = <String, Map<Object?, Object?>>{};

Workspace _workspace(List<String> pins) => Workspace(
  agents: [
    Agent(id: 'rivet', name: 'Rivet'),
    Agent(id: 'bruno', name: 'Bruno', projectId: 'p1'),
    Agent(id: 'june', name: 'June', projectId: 'p1'),
  ],
  projects: [Project(id: 'p1', name: 'Claktory')],
  selectedAgentId: 'rivet',
  settings: Settings(pinnedAgents: pins),
);

Future<Harness> _show(WidgetTester tester, Workspace workspace) async {
  asIPad();
  _menu.clear();
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  messenger.setMockMethodCallHandler(_channel, (call) async {
    if (call.method != 'setCommands') return null;
    _menu.clear();
    for (final command in (call.arguments as Map)['commands'] as List) {
      final map = command as Map;
      _menu[map['id'] as String] = map;
    }
    return true;
  });
  addTearDown(() => messenger.setMockMethodCallHandler(_channel, null));
  final harness = Harness(workspace);
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

Future<void> _run(WidgetTester tester, String id) async {
  await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
    CommandMenu.channelName,
    const StandardMethodCodec().encodeMethodCall(MethodCall('run', {'id': id})),
    (_) {},
  );
  await settle(tester);
}

void main() {
  test('the first nine pins, in order, a gone one skipped', () {
    final focus = WorkbenchFocus(menu: CommandMenu());
    addTearDown(focus.dispose);
    final agents = [
      for (var i = 0; i < 11; i++)
        Agent(id: 'a$i', name: 'Agent $i', projectId: 'p1'),
    ];
    final workspace = Workspace(
      projects: [Project(id: 'p1', name: 'Factory')],
      agents: agents,
      settings: Settings(
        pinnedAgents: ['a3', 'gone', for (var i = 0; i < 11; i++) 'a$i'],
      ),
    );
    final pins = pinnedAgents(workspace);
    expect(pins.first.id, 'a3');
    expect(pins.map((agent) => agent.id), isNot(contains('gone')));
    final opened = <String>[];
    final commands = workbenchCommands(
      tier: WorkbenchTier.full,
      focus: focus,
      agent: null,
      pins: pins,
      openPin: (agent) => opened.add(agent.id),
    ).where((command) => command.id.startsWith('open-pin-')).toList();
    expect(commands, hasLength(9));
    expect(commands.first.title, 'Open Agent 3');
    final encoded = encodeCommands(commands)['commands'] as List;
    expect((encoded.first as Map)['input'], '1');
    expect((encoded.first as Map)['modifiers'], ['command']);
    expect((encoded.last as Map)['input'], '9');
    commands[1].run!();
    expect(opened, ['a0']);
  });

  testWidgets('no pins: none in the menu', (tester) async {
    await _show(tester, _workspace(const []));
    expect(_menu.keys.where((id) => id.startsWith('open-pin-')), isEmpty);
    expect(_menu.containsKey('sidebar'), isTrue);
  });

  testWidgets('⌘1 and ⌘2 open Bruno and June by name; a pin made on '
      'another device shows at once', (tester) async {
    final harness = await _show(tester, _workspace(const ['june', 'bruno']));
    expect(_menu['open-pin-1']?['title'], 'Open June');
    expect(_menu['open-pin-2']?['title'], 'Open Bruno');
    expect(_menu['open-pin-1']?['enabled'], isTrue);

    await _run(tester, 'open-pin-2');
    expect(
      harness.backend.sent
          .where((message) => message.hasSelectAgent())
          .map((message) => message.selectAgent.agentId),
      contains('bruno'),
    );

    // Unpinned elsewhere: its command goes, and the next moves up.
    await harness.apply(tester, _workspace(const ['bruno']));
    await settle(tester);
    expect(_menu['open-pin-1']?['title'], 'Open Bruno');
    expect(_menu.containsKey('open-pin-2'), isFalse);
  });
}
