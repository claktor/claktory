// Where a Calendar entry's Open lands, drawn in the whole window to check:
// on a Mac, Calendar filling the window, then a monitor (the Monitors page,
// glowing), an item (the checklist beside, glowing), a task (Tasks, its
// output open), a conversation, Calendar opened again, and an entry whose
// monitor was cancelled; on an iPhone, each pushed over Calendar's page,
// and Calendar again after Back. With SHOTS set, each is drawn to a PNG:
//   SHOTS=/tmp/open flutter test test/calendar_open_shots_test.dart
// Without it, each still lays out without an overflow or an exception.
// test-bundle: alone (loads the app's fonts)
import 'dart:convert';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:factory_app/agent_view.dart' show AgentView;
import 'package:factory_app/app_dock.dart' show openApp;
import 'package:factory_app/app_views/calendar.dart' show AppCalendar;
import 'package:factory_app/app_views/transport.dart';
import 'package:factory_app/highlights.dart' show HighlightStore;
import 'package:factory_app/layout.dart' show chatKind;
import 'package:factory_app/mobile/navigation.dart' show PhoneRoutes;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:fixnum/fixnum.dart';

import 'app_calendar_test.dart'
    show
        calendarAt,
        calendarColumns,
        calendarNow,
        calendarRow,
        calendarRows,
        calendarTools;
import 'harness.dart';
import 'monitors_test.dart' show monitor;
import 'phone_sizes_test.dart' show loadGeist;

Future<void> _shot(WidgetTester tester, String name) async {
  expect(tester.takeException(), isNull, reason: name);
  final folder = Platform.environment['SHOTS'];
  if (folder == null) return;
  final boundary = tester.binding.renderViews.first;
  final layer = boundary.debugLayer! as OffsetLayer;
  final image = await tester.runAsync(
    () => layer.toImage(Offset.zero & boundary.size, pixelRatio: 2),
  );
  final bytes = await tester.runAsync(
    () => image!.toByteData(format: ui.ImageByteFormat.png),
  );
  Directory(folder).createSync(recursive: true);
  File('$folder/$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
}

final _calendar = AppInfo(
  id: 'calendar',
  name: 'Calendar',
  icon: 'calendar',
  state: AppState.APP_STATE_RUNNING,
  views: [AppViewSpec(id: 'main', title: 'Calendar')],
);

Agent _agent(String id, String name, String role, String color) => Agent(
  id: id,
  name: name,
  role: role,
  color: color,
  modelAvailable: true,
  extensions: [
    AgentExtension(id: 'checklist', enabled: true),
    AgentExtension(id: 'tasks', enabled: true),
  ],
  layout: ViewNode(
    id: 'chat',
    leaf: ViewLeaf(kind: chatKind),
  ),
);

Workspace _workspace() => Workspace(
  agents: [
    _agent('pericolo', 'Pericolo', 'DevOps', 'teal'),
    _agent('giorno', 'Giorno', 'Backend', 'pink'),
    _agent('lucy', 'Lucy', 'UX Designer', 'blue'),
    _agent('fugo', 'Fugo', 'Security', 'red'),
    _agent('formaggio', 'Formaggio', 'Frontend', 'green'),
  ],
  selectedAgentId: 'giorno',
  settings: Settings(userName: 'Matt'),
  extensions: [
    Extension(id: 'checklist', name: 'Checklist', icon: 'list-checks'),
    Extension(id: 'tasks', name: 'Tasks', icon: 'terminal'),
  ],
  apps: [_calendar],
  appsAvailable: true,
  pinnedApps: ['calendar'],
  highlights: true,
);

/// Lucy's mock's week, with a task running, a conversation to open and a
/// monitor that was cancelled since.
final _rows = [
  ...calendarRows,
  calendarRow(
    id: 'signing',
    start: calendarAt(3, 14, 5),
    title: 'Signing the release',
    kind: 'task',
    live: true,
    open: 'task:pericolo:task-4',
  ),
  calendarRow(
    id: 'signin',
    start: calendarAt(3, 17),
    end: calendarAt(3, 17, 30),
    title: 'Sign-in review',
    agent: 'fugo',
    detail: 'Fugo goes through the sign-in paths again.',
    open: 'agent:fugo',
  ),
  calendarRow(
    id: 'backup',
    start: calendarAt(3, 20),
    title: 'Nightly backup',
    agent: 'giorno',
    kind: 'monitor',
    repeat: 'every day at 20:00',
    detail: "From Giorno's monitor.",
    open: 'monitor:giorno:m7',
  ),
];

AppSurfaceData _surface(double height) => AppSurfaceData(
  surfaceId: 'calendar-1',
  appId: 'calendar',
  title: 'Calendar',
  state: AppViewState.open,
  snapshot: jsonEncode({
    'version': '1.0',
    'createSurface': {
      'surfaceId': 'calendar-1',
      'catalogId': appViewCatalogV1,
      'components': [
        {
          'id': 'root',
          'component': 'Column',
          'children': ['cal'],
        },
        {
          'id': 'cal',
          'component': 'Calendar',
          'data': {'@path': '/rows'},
          'view': 'week',
          'anchor': calendarAt(3, 12),
          'weekStart': 'monday',
          'height': height,
          'tools': ['agent_menu', 'show_menu', 'more_menu'],
          'action': {
            'event': {
              'name': 'calendar',
              'context': {'board': 'main'},
            },
          },
        },
        ...calendarTools,
      ],
      'dataModel': {
        'rows': {'columns': calendarColumns, 'rows': _rows},
      },
    },
  }),
  seq: 0,
  error: '',
);

BackgroundTask _signing() {
  final started = DateTime.now().subtract(const Duration(minutes: 15));
  return BackgroundTask(
    id: 'task-4',
    agentId: 'pericolo',
    command: 'scripts/sign-release.sh ~/.factory/releases/v0.1.28',
    started: Int64(started.millisecondsSinceEpoch),
    running: true,
    logPath: '/Users/matt/.factory/agents/pericolo/tasks/task-4.log',
    logTail:
        'Signing with the Developer ID…\n'
        "Sent to Apple's notary service.\n"
        'Waiting for its answer…',
  );
}

BackgroundTask _build() {
  final started = DateTime.now().subtract(const Duration(minutes: 102));
  return BackgroundTask(
    id: 'task-3',
    agentId: 'pericolo',
    command: 'scripts/release.sh --out /tmp/claktory-rc',
    started: Int64(started.millisecondsSinceEpoch),
    ended: Int64(
      started
          .add(const Duration(minutes: 1, seconds: 42))
          .millisecondsSinceEpoch,
    ),
    status: 'exit 0',
    logPath: '/Users/matt/.factory/agents/pericolo/tasks/task-3.log',
    logTail:
        'Building factoryd (release)…\n'
        'Building Claktory.app…\n'
        'Signing with the Developer ID…\n'
        'Notarized and stapled.\n'
        'Release v0.1.28 is in ~/.factory/releases.',
  );
}

final _today = Int64(DateTime.now().millisecondsSinceEpoch);

Checklist _checklist() => Checklist(
  items: [
    ChecklistItem(
      id: 17,
      text: 'Calendar: week view goldens',
      status: ChecklistStatus.CHECKLIST_STATUS_DONE,
      created: _today,
    ),
    ChecklistItem(
      id: 18,
      text: 'History video, check again',
      status: ChecklistStatus.CHECKLIST_STATUS_BLOCKED,
      blockedOn: 'Formaggio',
      blockedRef: 'formaggio',
      blockedKind: BlockedKind.BLOCKED_KIND_AGENT,
      created: _today,
    ),
    ChecklistItem(
      id: 19,
      text: 'Focus mode on Mac: QA notes',
      status: ChecklistStatus.CHECKLIST_STATUS_OPEN,
      created: _today,
    ),
  ],
);

/// What each agent and the person said, by agent.
final _said = {
  'fugo': [
    Entry(
      seq: Int64(1),
      timestampMs: Int64(calendarAt(3, 13, 2)),
      user: UserText(
        text:
            'At 17:00, go through the sign-in paths again: the code '
            'screen, the passkey and the recovery key.',
      ),
    ),
    Entry(
      seq: Int64(2),
      timestampMs: Int64(calendarAt(3, 13, 3)),
      assistant: AssistantText(
        text: "I'll start at 17:00 and tell you what I find, path by path.",
      ),
    ),
  ],
};

Harness _harness() {
  final harness = Harness(_workspace())..backend.appliesLayouts = true;
  final backend = harness.backend;
  backend.appSurfaces['calendar'] = _surface(760);
  backend.monitorList.addAll([
    AgentMonitor(
      agentId: 'pericolo',
      monitor: monitor(
        3,
        'Production logs',
        when: 'every 6h',
        nextIn: const Duration(hours: 3, minutes: 40),
        fires: 12,
      ),
    ),
    AgentMonitor(
      agentId: 'fugo',
      monitor: monitor(1, 'Logs', when: 'every 5m', fires: 288),
    ),
    AgentMonitor(
      agentId: 'pericolo',
      monitor: monitor(4, 'Morning sweep', when: '0 9 * * 1-5'),
    ),
  ]);
  final views = [
    ExtensionView(
      extensionId: 'checklist',
      agentId: 'lucy',
      title: 'Checklist',
      checklist: _checklist(),
    ),
    ExtensionView(
      extensionId: 'tasks',
      agentId: 'pericolo',
      title: 'Tasks',
      tasks: Tasks(tasks: [_signing(), _build()]),
    ),
  ];
  // As the backend answers: the agent chosen shows, its conversation, its
  // panel open and what's in it.
  backend.onSend = (message) {
    final next = backend.workspace.workspace.deepCopy();
    if (message.hasSelectAgent()) {
      final id = message.selectAgent.agentId;
      next.selectedAgentId = id;
      backend.transcript.load(
        Transcript(agentId: id, entries: _said[id] ?? const []),
      );
    } else if (message.hasOpenPanel()) {
      final open = message.openPanel;
      for (final agent in next.agents) {
        if (agent.id == open.agentId) agent.openPanel = open.extensionId;
      }
    } else {
      return;
    }
    next.openPanel = next.agents
        .firstWhere((agent) => agent.id == next.selectedAgentId)
        .openPanel;
    backend.workspace.apply(next);
    for (final view in views) {
      if (view.agentId == next.selectedAgentId &&
          view.extensionId == next.openPanel) {
        backend.extensionView.apply(view);
      }
    }
  };
  backend.checklists['lucy'] = _checklist();
  backend.taskLists['pericolo'] = Tasks(tasks: [_signing(), _build()]);
  return harness;
}

/// The Calendar entry titled [title] (the last of that name).
Finder _entry(String title) => find
    .byWidgetPredicate(
      (w) =>
          w is Semantics && (w.properties.label ?? '').startsWith('$title, '),
    )
    .last;

/// A second after Open: there, and glowing at its strongest.
Future<void> _landed(WidgetTester tester) async {
  await pumpFrames(tester, const Duration(milliseconds: 1000));
}

void main() {
  setUpAll(loadGeist);
  setUp(() {
    AppCalendar.clock = () => calendarNow;
    addTearDown(() => AppCalendar.clock = DateTime.now);
    final dispatcher = TestWidgetsFlutterBinding.instance.platformDispatcher;
    dispatcher.platformBrightnessTestValue = Brightness.dark;
    addTearDown(dispatcher.clearPlatformBrightnessTestValue);
    WidgetsApp.debugAllowBannerOverride = false;
    addTearDown(() => WidgetsApp.debugAllowBannerOverride = true);
  });

  testWidgets('on a Mac', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = _harness();
    await harness.show(tester, size: const Size(1440, 900));
    await pumpFrames(tester);

    Future<void> calendar() async {
      openApp(tester.element(find.byType(AgentView).first), _calendar);
      await pumpFrames(tester, const Duration(seconds: 1));
    }

    Future<void> open(String entry, String button, String name) async {
      await calendar();
      await tester.tap(_entry(entry));
      await pumpFrames(tester, const Duration(seconds: 1));
      await _shot(tester, 'mac-$name-details');
      await tester.tap(find.text(button).last);
      await _landed(tester);
      await _shot(tester, 'mac-$name');
    }

    await calendar();
    await _shot(tester, 'mac-0-calendar');
    await open('Production logs', 'Open monitor', '1-monitor');
    await calendar();
    await _shot(tester, 'mac-2-calendar-again');
    await open('#18 History video, check again', 'Open item', '3-item');
    await open('Release build', 'Open task', '4-task');
    await open('Sign-in review', 'Open conversation', '5-agent');

    await calendar();
    await tester.tap(_entry('Nightly backup'));
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(find.text('This monitor was cancelled.'), findsOneWidget);
    await _shot(tester, 'mac-6-gone');
    await tester.pump(HighlightStore.duration);
  });

  testWidgets('on an iPhone', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = _harness();
    harness.backend.appSurfaces['calendar'] = _surface(700);
    await harness.show(tester, size: const Size(393, 852));
    await pumpFrames(tester);
    final routes = PhoneRoutes.of(
      tester.element(find.byType(AgentView, skipOffstage: false).first),
    )!;

    openApp(
      tester.element(find.byType(AgentView, skipOffstage: false).first),
      _calendar,
    );
    await pumpFrames(tester, const Duration(seconds: 1));
    await _shot(tester, 'iphone-0-calendar');

    for (final (entry, button, name) in [
      ('Production logs', 'Open monitor', '1-monitor'),
      ('#18 History video, check again', 'Open item', '2-item'),
      ('Signing the release', 'Open task', '3-task'),
      ('Sign-in review', 'Open conversation', '4-agent'),
    ]) {
      await tester.dragUntilVisible(
        find.text(entry).last,
        find.byType(Scrollable).last,
        const Offset(0, -100),
      );
      await pumpFrames(tester);
      await tester.tap(find.text(entry).last);
      await pumpFrames(tester, const Duration(seconds: 1));
      await _shot(tester, 'iphone-$name-sheet');
      await tester.tap(find.text(button));
      await _landed(tester);
      await _shot(tester, 'iphone-$name');
      routes.pop<void>();
      await pumpFrames(tester, const Duration(seconds: 1));
      if (name == '1-monitor') await _shot(tester, 'iphone-1-monitor-back');
    }
    await tester.pump(HighlightStore.duration);
  });
}
