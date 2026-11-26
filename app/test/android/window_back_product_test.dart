// Actual shared phone/list/add/sheet surfaces under the new root placement.
// Recording backends and framework channel events, not native acceptance.
import 'dart:async';
import 'dart:io';

import 'package:factory_app/agent_view.dart';
import 'package:factory_app/android/back_host.dart';
import 'package:factory_app/android/incoming_share_host.dart';
import 'package:factory_app/android/incoming_shares.dart';
import 'package:factory_app/chats.dart';
import 'package:factory_app/controls.dart';
import 'package:factory_app/message_box.dart';
import 'package:factory_app/android/window_back_host.dart';
import 'package:factory_app/connections.dart';
import 'package:factory_app/device_account.dart';
import 'package:factory_app/devices.dart' show thisDeviceKind;
import 'package:factory_app/drafts.dart';
import 'package:factory_app/mobile/back.dart';
import 'package:factory_app/mobile/drawer.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/mobile/presentation.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/rail.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/shell.dart';
import 'package:factory_app/update.dart';
import 'package:factory_app/zoom.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../harness.dart';
import 'support/channel_fakes.dart';

final _first = 'factory://?t=${'a' * 64}&name=Origin&e=${'c' * 64}';
final _second = 'factory://?t=${'b' * 64}&name=Other&e=${'d' * 64}';

Future<void> _button() async {
  await sendNativeMethod(
    SystemChannels.navigation,
    const MethodCall('popRoute'),
  );
}

Future<Object?> _gesture(String method, {double progress = 0}) =>
    sendNativeMethod(
      SystemChannels.backGesture,
      MethodCall(
        method,
        method == 'startBackGesture' || method == 'updateBackGestureProgress'
            ? {
                'touchOffset': [1.0, 300.0],
                'progress': progress,
                'swipeEdge': 0,
              }
            : null,
      ),
    );

class _SlowSelectionBackend extends RecordingBackend {
  _SlowSelectionBackend(this.gate) : super(localBackend: false);
  final Future<void> gate;
  @override
  Future<void> selectAgent(String id) async {
    await super.selectAgent(id);
    await gate;
  }
}

class _App {
  _App({this.selectionGate});
  final Future<void>? selectionGate;
  final connections = Connections(
    MemoryConnectionStorage(),
    null,
    null,
    null,
    DeviceAccount(),
  );
  final drafts = DeviceDrafts(MemoryDraftStorage());
  final backends = <String, RecordingBackend>{};
  final camera = FakeCamera();
  late final claktories = Claktories(
    connections: connections,
    localBackend: false,
    open: (target) {
      final backend = selectionGate == null
          ? RecordingBackend(localBackend: false)
          : _SlowSelectionBackend(selectionGate!);
      backend.connectTo(target);
      backend.workspace.apply(
        Workspace(
          agents: [
            Agent(
              id: 'agent',
              name: target?.code == _first ? 'Origin agent' : 'Other agent',
              modelAvailable: true,
            ),
          ],
          selectedAgentId: 'agent',
          settings: Settings(userName: 'QA', name: target?.name ?? 'Unpaired'),
        ),
      );
      return backends[target?.code ?? ''] = backend;
    },
    drafts: drafts,
  );

  Future<void> show(
    WidgetTester tester, {
    AndroidIncomingShares? incoming,
    bool wide = false,
    bool paired = true,
  }) async {
    if (paired) {
      connections
        ..use(parsePairingCode(_second))
        ..use(parsePairingCode(_first));
    }
    final roots = claktories;
    tester.view.physicalSize = wide
        ? const Size(1000, 800)
        : const Size(393, 852);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    addTearDown(roots.dispose);
    tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    // This is main's placement, not a replacement set of screens. Polnareff
    // wires the same root above the real window Overlay in main.dart.
    final entry = OverlayEntry(
      builder: (context) {
        final selected = ClaktoriesScope.of(context)!.selected;
        final window = wide
            ? Shell(key: ObjectKey(selected))
            : PhoneClaktories(shell: Shell(key: ObjectKey(selected)));
        return incoming == null
            ? window
            : AndroidIncomingShareHost(shares: incoming, child: window);
      },
    );
    await tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xff202020),
        localizationsDelegates: PhonePlatformPolicy.current.localizations,
        builder: (_, _) => ClaktoriesScope(
          claktories: roots,
          child: Builder(
            builder: (context) {
              final selected = ClaktoriesScope.of(context)!.selected;
              return AppScope(
                backend: selected.backend,
                login: selected.login,
                page: selected.page,
                appBuild: AppBuild(version: 'dev', bundlePath: '', quit: () {}),
                drafts: selected.drafts,
                history: selected.history,
                connections: connections,
                camera: camera,
                child: AppZoom(
                  child: AndroidWindowBackHost(
                    window: selected,
                    child: Overlay(initialEntries: [entry]),
                  ),
                ),
              );
            },
          ),
        ),
      ),
    );
    await pumpFrames(tester);
    expect(find.byType(paired ? AgentView : PairScreen), findsOneWidget);
    expect(find.byWidgetPredicate((w) => w is AndroidBackHost), findsOneWidget);
  }

  Future<void> list(WidgetTester tester) async {
    tester
        .widget<PhoneDrawerHost>(find.byType(PhoneDrawerHost))
        .onClaktories!();
    await pumpFrames(tester);
    expect(find.byType(ClaktoryList), findsOneWidget);
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  var systemPops = 0;
  setUp(() {
    PhonePlatformPolicy.debugAndroidPolicy = true;
    systemPops = 0;
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'SystemNavigator.pop') systemPops++;
      return null;
    });
    addTearDown(() {
      PhonePlatformPolicy.debugAndroidPolicy = false;
      messenger.setMockMethodCallHandler(SystemChannels.platform, null);
    });
  });

  testWidgets(
    'one host; Back from Add to list, then original Claktory, then system',
    (tester) async {
      final app = _App();
      await app.show(tester);
      await app.list(tester);
      await tester.ensureVisible(find.text('Add a Claktory'));
      await tester.tap(find.text('Add a Claktory'));
      await pumpFrames(tester);
      expect(find.byType(PairScreen), findsOneWidget);
      expect(
        find.byWidgetPredicate((w) => w is AndroidBackHost),
        findsOneWidget,
      );
      await _button();
      await pumpFrames(tester);
      expect(find.byType(PairScreen), findsNothing);
      expect(find.byType(ClaktoryList), findsOneWidget);
      expect(systemPops, 0);
      await _button();
      await pumpFrames(tester);
      expect(find.byType(ClaktoryList), findsNothing);
      expect(find.byType(AgentView), findsOneWidget);
      expect(app.connections.current?.code, _first);
      expect(systemPops, 0);
      await _button();
      await tester.pump();
      expect(systemPops, 1);
    },
  );

  testWidgets(
    'Add keeps typed code on cancel and first Back only hides the keyboard',
    (tester) async {
      final app = _App();
      await app.show(tester);
      await app.list(tester);
      await tester.ensureVisible(find.text('Add a Claktory'));
      await tester.tap(find.text('Add a Claktory'));
      await pumpFrames(tester);
      final field = find.descendant(
        of: find.byType(PairScreen),
        matching: find.byType(EditableText),
      );
      expect(field, findsOneWidget);
      const code = 'Unsent 日本語 👩🏽‍💻';
      await tester.enterText(field, code);
      await tester.pump();
      expect(await _gesture('startBackGesture'), isTrue);
      await _gesture('updateBackGestureProgress', progress: .8);
      await _gesture('cancelBackGesture');
      await tester.pump();
      expect(tester.widget<EditableText>(field).controller.text, code);
      expect(find.byType(PairScreen), findsOneWidget);
      tester.view.viewInsets = const FakeViewPadding(bottom: 300);
      await tester.pump();
      tester.testTextInput.log.clear();
      await _button();
      await tester.pump();
      expect(
        tester.testTextInput.log.where(
          (call) => call.method == 'TextInput.hide',
        ),
        hasLength(1),
      );
      expect(tester.widget<EditableText>(field).controller.text, code);
      expect(find.byType(PairScreen), findsOneWidget);
      expect(systemPops, 0);
      tester.view.viewInsets = FakeViewPadding.zero;
      await tester.pump();
      await _button();
      await pumpFrames(tester);
      expect(find.byType(PairScreen), findsNothing);
      expect(find.byType(ClaktoryList), findsOneWidget);
      expect(systemPops, 0);
    },
  );

  testWidgets(
    'list Back returns to origin even if selection changed while list shows',
    (tester) async {
      final app = _App();
      await app.show(tester);
      await app.list(tester);
      app.connections.use(parsePairingCode(_second));
      await pumpFrames(tester);
      expect(find.byType(ClaktoryList), findsOneWidget);
      await _button();
      await pumpFrames(tester);
      expect(app.connections.current?.code, _first);
      expect(find.byType(AgentView), findsOneWidget);
      expect(systemPops, 0);
    },
  );

  testWidgets('list without its old Claktory returns to the remaining one', (
    tester,
  ) async {
    final app = _App();
    await app.show(tester);
    await app.list(tester);
    app.connections.forget(parsePairingCode(_first)!);
    await pumpFrames(tester);
    await _button();
    await pumpFrames(tester);
    expect(app.connections.current?.code, _second);
    expect(find.byType(AgentView), findsOneWidget);
    expect(systemPops, 0);
  });

  testWidgets(
    'predictive list cancellation preserves selection and visible list',
    (tester) async {
      final app = _App();
      await app.show(tester);
      await app.list(tester);
      expect(await _gesture('startBackGesture'), isTrue);
      await _gesture('updateBackGestureProgress', progress: .7);
      await _gesture('cancelBackGesture');
      await pumpFrames(tester);
      expect(find.byType(ClaktoryList), findsOneWidget);
      expect(app.connections.current?.code, _first);
      expect(systemPops, 0);
    },
  );

  testWidgets(
    'A sheet belongs to A window, not B; its token releases on replacement',
    (tester) async {
      final app = _App();
      await app.show(tester);
      final context = tester.element(find.byType(AgentView));
      final back = PhoneBack.maybeOf(context)!;
      var completed = false;
      final future = presentSheet<void>(
        context,
        title: 'A window sheet',
        builder: (_, _) => const Text('A sheet content'),
      );
      future.whenComplete(() => completed = true);
      await pumpFrames(tester);
      expect(find.text('A sheet content'), findsOneWidget);
      expect(back.target, isA<TokenTarget>());
      app.connections.use(parsePairingCode(_second));
      await pumpFrames(tester);
      expect(find.text('A sheet content'), findsNothing);
      expect(find.byType(PhoneSheet), findsNothing);
      expect(completed, isTrue);
      final bContext = tester.element(find.byType(AgentView));
      expect(PhoneBack.maybeOf(bContext)!.target, isNull);
      expect(app.connections.current?.code, _second);
      final bHost = tester.state<PhoneDrawerHostState>(
        find.byType(PhoneDrawerHost),
      );
      unawaited(
        bHost.push<void>(
          PhoneRoute<void>(
            id: 'b-page',
            builder: (_) => const Text('B pushed page fixture'),
          ),
        ),
      );
      await pumpFrames(tester);
      expect(find.text('B pushed page fixture'), findsOneWidget);
      await _button();
      await pumpFrames(tester);
      expect(find.text('B pushed page fixture'), findsNothing);
      expect(systemPops, 0);
      await _button();
      await tester.pump();
      expect(systemPops, 1);
      expect(tester.takeException(), isNull);
    },
  );
  group('incoming shares fill the real shared composer, never Send', () {
    const channel = MethodChannel('factory/shared');
    late AndroidIncomingShares incoming;
    late List<int> discarded;

    setUp(() {
      discarded = [];
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      messenger.setMockMethodCallHandler(channel, (call) async {
        if (call.method == 'listen') return [];
        if (call.method == 'discard') {
          discarded.add((call.arguments as Map)['id'] as int);
        }
        return null;
      });
      incoming = AndroidIncomingShares(channel: channel);
      addTearDown(() {
        incoming.dispose();
        messenger.setMockMethodCallHandler(channel, null);
      });
    });

    Future<void> share(
      WidgetTester tester, {
      String? text = 'Shared 日本語 👩🏽‍💻',
      List<String> files = const [],
      String? error,
      int id = 1,
    }) async {
      await sendNativeMethod(
        channel,
        MethodCall('received', {
          'id': id,
          'text': text,
          'subject': null,
          'files': files,
          'text_cut': false,
          'error': error,
        }),
      );
      await pumpFrames(tester);
      expect(find.byType(AndroidSharePicker), findsOneWidget);
    }

    Finder choice(String name) => find.descendant(
      of: find.byType(AndroidSharePicker),
      matching: find.widgetWithText(Button, name),
    );

    void noSend(_App app) {
      expect(find.byType(MessageBox), findsOneWidget);
      for (final backend in app.backends.values) {
        expect(
          backend.sent.where(
            (message) =>
                message.hasSendMessage() || message.hasSendChatMessage(),
          ),
          isEmpty,
        );
      }
    }

    Future<void> choose(
      WidgetTester tester,
      String name, {
      Future<void>? staged,
    }) async {
      await tester.ensureVisible(choice(name));
      await tester.pump();
      await tester.tap(choice(name));
      if (staged != null) {
        var complete = false;
        staged.then((_) => complete = true);
        final deadline = Stopwatch()..start();
        await tester.runAsync(() async {
          // FileStat is real IO; pump its fake-zone continuations while giving
          // the host event loop a turn. Assert the actual completion signal.
          while (!complete && deadline.elapsed < const Duration(seconds: 5)) {
            await tester.pump();
            await Future<void>.delayed(Duration.zero);
          }
        });
        expect(complete, isTrue, reason: 'shared attachment staging completed');
      }
      await pumpFrames(tester);
    }

    for (final origin in ['list', 'add', 'page', 'drawer']) {
      testWidgets(
        'from $origin: selected conversation is in front, root Back',
        (tester) async {
          final app = _App();
          await app.show(tester, incoming: incoming);
          if (origin == 'list' || origin == 'add') {
            await app.list(tester);
            if (origin == 'add') {
              await tester.ensureVisible(find.text('Add a Claktory'));
              await tester.tap(find.text('Add a Claktory'));
              await pumpFrames(tester);
              expect(find.byType(PairScreen), findsOneWidget);
            }
          } else {
            final drawer = tester.state<PhoneDrawerHostState>(
              find.byType(PhoneDrawerHost),
            );
            if (origin == 'page') {
              unawaited(
                drawer.push(
                  PhoneRoute<void>(
                    id: 'covered',
                    builder: (_) => const Text('Covered page fixture'),
                  ),
                ),
              );
            } else {
              drawer.openDrawer();
            }
            await pumpFrames(tester);
          }
          final chosen = app.claktories.selected;
          chosen.drafts.set('agent', 'Keep my existing draft');
          await share(tester);
          await choose(tester, 'Origin agent');
          expect(find.byType(AndroidSharePicker), findsNothing);
          expect(find.byType(ClaktoryList), findsNothing);
          expect(find.byType(PairScreen), findsNothing);
          expect(find.text('Covered page fixture'), findsNothing);
          final box = find.descendant(
            of: find.byType(AgentView),
            matching: find.byType(EditableText),
          );
          expect(box, findsOneWidget);
          expect(
            tester.widget<EditableText>(box).controller.text,
            'Keep my existing draft\nShared 日本語 👩🏽‍💻',
          );
          expect(
            chosen.drafts.of('agent'),
            'Keep my existing draft\nShared 日本語 👩🏽‍💻',
          );
          expect(
            PhoneBack.maybeOf(tester.element(find.byType(AgentView)))!.target,
            isNull,
          );
          noSend(app);
          expect(discarded, isEmpty);
          await _button();
          await tester.pump();
          expect(systemPops, 1);
        },
      );
    }

    testWidgets(
      'another Claktory: preserves existing text/files and ownership on retirement',
      (tester) async {
        final app = _App();
        await app.show(tester, incoming: incoming);
        final destination = app.claktories.all.firstWhere(
          (root) => root.target?.code == _second,
        );
        final folder = Directory.systemTemp.createTempSync(
          'claktory-share-test-',
        );
        addTearDown(() => folder.deleteSync(recursive: true));
        final before = File('${folder.path}/existing.txt')
          ..writeAsStringSync('before');
        final shared = File('${folder.path}/shared.txt')
          ..writeAsStringSync('shared');
        destination.drafts.set('agent', 'Existing');
        await tester.runAsync(
          () => destination.backend.attachments.add('agent', [before.path]),
        );
        await app.list(tester);
        await share(tester, files: [shared.path]);
        final staged = Completer<void>();
        destination.backend.attachments.addListener(() {
          if (!staged.isCompleted) staged.complete();
        });
        await choose(tester, 'Other agent', staged: staged.future);
        expect(app.connections.current?.code, _second);
        expect(destination.drafts.of('agent'), 'Existing\nShared 日本語 👩🏽‍💻');
        expect(destination.backend.attachments.of('agent').map((f) => f.path), [
          before.path,
          shared.path,
        ]);
        expect(find.byType(AgentView), findsOneWidget);
        expect(find.byType(AndroidSharePicker), findsNothing);
        expect(discarded, isEmpty);
        expect(shared.existsSync(), isTrue);
        noSend(app);
      },
    );

    testWidgets('files only do not add a blank line to an existing draft', (
      tester,
    ) async {
      final app = _App();
      await app.show(tester, incoming: incoming);
      final folder = Directory.systemTemp.createTempSync(
        'claktory-share-test-',
      );
      addTearDown(() => folder.deleteSync(recursive: true));
      final file = File('${folder.path}/image.txt')..writeAsStringSync('image');
      final chosen = app.claktories.selected;
      chosen.drafts.set('agent', 'Unchanged');
      await share(tester, text: null, files: [file.path]);
      final staged = Completer<void>();
      chosen.backend.attachments.addListener(() {
        if (!staged.isCompleted) staged.complete();
      });
      await choose(tester, 'Origin agent', staged: staged.future);
      expect(chosen.drafts.of('agent'), 'Unchanged');
      expect(chosen.backend.attachments.of('agent').single.path, file.path);
      noSend(app);
    });

    testWidgets(
      'chat receives text in existing shared chat MessageBox; files disable it',
      (tester) async {
        final app = _App();
        await app.show(tester, incoming: incoming);
        final root = app.claktories.selected;
        final backend = app.backends[_first]!;
        backend.workspace.apply(
          backend.workspace.workspace.deepCopy()
            ..chats.add(Chat(id: 'chat', title: 'Trip', modelAvailable: true)),
        );
        backend.onSend = (message) {
          if (message.hasSelectChat()) {
            backend.workspace.apply(
              backend.workspace.workspace.deepCopy()
                ..selectedChatId = message.selectChat.chatId,
            );
          }
        };
        root.drafts.set('chat:chat', 'Chat draft');
        await share(tester, files: ['/cache/shared.png']);
        expect(tester.widget<Button>(choice('Trip')).onPressed, isNull);
        expect(find.text('Chats take text only'), findsOneWidget);
        await tester.tap(find.widgetWithText(Button, 'Cancel'));
        await pumpFrames(tester);
        expect(discarded, [1]);
        await share(tester, id: 2);
        await choose(tester, 'Trip');
        expect(find.byType(ChatView), findsOneWidget);
        final box = find.descendant(
          of: find.byType(ChatView),
          matching: find.byType(EditableText),
        );
        expect(
          tester.widget<EditableText>(box).controller.text,
          'Chat draft\nShared 日本語 👩🏽‍💻',
        );
        noSend(app);
      },
    );

    testWidgets(
      'cancel and predictive cancel do not write/send or lose queued intake',
      (tester) async {
        final app = _App();
        await app.show(tester, incoming: incoming);
        await share(tester);
        expect(await _gesture('startBackGesture'), isTrue);
        await _gesture('updateBackGestureProgress', progress: .7);
        await _gesture('cancelBackGesture');
        await tester.pump();
        expect(find.byType(AndroidSharePicker), findsOneWidget);
        expect(incoming.pending.single.id, 1);
        expect(discarded, isEmpty);
        expect(app.claktories.selected.drafts.of('agent'), isEmpty);
        await _button();
        await pumpFrames(tester);
        expect(find.byType(AndroidSharePicker), findsNothing);
        expect(discarded, [1]);
        expect(systemPops, 0);
        noSend(app);
      },
    );

    testWidgets('error-only native share is explained, not an empty composer', (
      tester,
    ) async {
      final app = _App();
      await app.show(tester, incoming: incoming);
      await share(tester, text: null, error: 'too_many');
      expect(find.text('Nothing to add'), findsOneWidget);
      expect(
        find.text(
          'You can share up to 20 files at once. Share fewer and try again.',
        ),
        findsOneWidget,
      );
      expect(choice('Origin agent'), findsNothing);
      await tester.tap(find.widgetWithText(Button, 'Close'));
      await pumpFrames(tester);
      expect(discarded, [1]);
      expect(app.claktories.selected.drafts.of('agent'), isEmpty);
      noSend(app);
    });

    for (final wide in [false, true]) {
      for (final back in [false, true]) {
        testWidgets(
          'repeated unreadable shares need one dismissal (wide=$wide, back=$back)',
          (tester) async {
            final app = _App();
            await app.show(tester, incoming: incoming, wide: wide);
            await share(tester, text: null, error: 'refused');
            await share(tester, text: null, error: 'refused', id: 2);
            expect(find.text('Nothing to add'), findsOneWidget);
            expect(find.text("Some files couldn't be opened."), findsOneWidget);
            expect(incoming.pending.map((share) => share.id), [1, 2]);
            if (back) {
              await _button();
            } else {
              await tester.tap(find.widgetWithText(Button, 'Close'));
            }
            await pumpFrames(tester);
            expect(find.byType(AndroidSharePicker), findsNothing);
            expect(incoming.pending, isEmpty);
            expect(discarded, [1, 2]);
            expect(systemPops, 0);
            expect(app.claktories.selected.drafts.of('agent'), isEmpty);
            noSend(app);
          },
        );
      }
    }

    testWidgets(
      'new unreadable share replaces the warning in the open notice',
      (tester) async {
        final app = _App();
        await app.show(tester, incoming: incoming);
        await share(tester, text: null, error: 'refused');
        await share(tester, text: null, error: 'too_large', id: 2);
        expect(find.text('Nothing to add'), findsOneWidget);
        expect(find.text("Some files couldn't be opened."), findsNothing);
        expect(find.text("Files over 2 GB can't be added."), findsOneWidget);
        await tester.tap(find.widgetWithText(Button, 'Close'));
        await pumpFrames(tester);
        expect(find.byType(AndroidSharePicker), findsNothing);
        expect(incoming.pending, isEmpty);
        expect(discarded, [1, 2]);
        noSend(app);
      },
    );

    testWidgets('unreadable burst before the first frame has one notice', (
      tester,
    ) async {
      final app = _App();
      await app.show(tester, incoming: incoming);
      for (final id in [1, 2, 3]) {
        await sendNativeMethod(
          channel,
          MethodCall('received', {
            'id': id,
            'text': null,
            'subject': null,
            'files': <String>[],
            'text_cut': false,
            'error': 'refused',
          }),
        );
      }
      await pumpFrames(tester);
      expect(find.byType(AndroidSharePicker), findsOneWidget);
      expect(find.text('Nothing to add'), findsOneWidget);
      await tester.tap(find.widgetWithText(Button, 'Close'));
      await pumpFrames(tester);
      expect(find.byType(AndroidSharePicker), findsNothing);
      expect(incoming.pending, isEmpty);
      expect(discarded, [1, 2, 3]);
      noSend(app);
    });

    testWidgets('coalescing never skips real content or its partial warning', (
      tester,
    ) async {
      final app = _App();
      await app.show(tester, incoming: incoming);
      await share(tester, text: null, error: 'refused');
      await share(
        tester,
        text: 'Keep this shared text',
        error: 'too_large',
        id: 2,
      );
      await share(tester, text: null, error: 'no_space', id: 3);
      expect(find.text("Some files couldn't be opened."), findsOneWidget);
      expect(find.text("Files over 2 GB can't be added."), findsNothing);
      await tester.tap(find.widgetWithText(Button, 'Close'));
      await pumpFrames(tester);
      expect(discarded, [1]);
      expect(incoming.pending.map((share) => share.id), [2, 3]);
      expect(find.text('Keep this shared text'), findsOneWidget);
      expect(find.text("Files over 2 GB can't be added."), findsOneWidget);
      await choose(tester, 'Origin agent');
      expect(
        app.claktories.selected.drafts.of('agent'),
        'Keep this shared text',
      );
      expect(discarded, [1], reason: 'the accepted text is not discarded');
      expect(find.text('Nothing to add'), findsOneWidget);
      expect(find.textContaining('Not enough space on this'), findsOneWidget);
      await tester.tap(find.widgetWithText(Button, 'Close'));
      await pumpFrames(tester);
      expect(find.byType(AndroidSharePicker), findsNothing);
      expect(discarded, [1, 3]);
      expect(incoming.pending, isEmpty);
      noSend(app);
    });

    testWidgets(
      'failed selection is truthful/retryable and keeps the payload',
      (tester) async {
        final app = _App();
        await app.show(tester, incoming: incoming);
        final backend = app.backends[_first]!;
        await share(tester);
        backend.refuse = 'offline';
        await choose(tester, 'Origin agent');
        expect(
          find.text("Couldn't open that conversation. Try again."),
          findsOneWidget,
        );
        expect(incoming.pending, hasLength(1));
        expect(app.claktories.selected.drafts.of('agent'), isEmpty);
        backend.refuse = null;
        await choose(tester, 'Origin agent');
        expect(find.byType(AndroidSharePicker), findsNothing);
        expect(
          app.claktories.selected.drafts.of('agent'),
          'Shared 日本語 👩🏽‍💻',
        );
        noSend(app);
      },
    );

    testWidgets('wide shared shell also reaches its real composer', (
      tester,
    ) async {
      final app = _App();
      await app.show(tester, incoming: incoming, wide: true);
      expect(find.byType(PhoneDrawerHost), findsNothing);
      await share(tester);
      await choose(tester, 'Origin agent');
      final box = find.descendant(
        of: find.byType(AgentView),
        matching: find.byType(EditableText),
      );
      expect(
        tester.widget<EditableText>(box).controller.text,
        'Shared 日本語 👩🏽‍💻',
      );
      noSend(app);
    });
    testWidgets(
      'cancel while selection awaits never accepts the share on late success',
      (tester) async {
        final gate = Completer<void>();
        final app = _App(selectionGate: gate.future);
        await app.show(tester, incoming: incoming);
        await share(tester);
        await choose(tester, 'Origin agent');
        expect(find.byType(AndroidSharePicker), findsOneWidget);
        expect(find.text('Loading…'), findsOneWidget);
        await tester.tap(find.widgetWithText(Button, 'Cancel'));
        await pumpFrames(tester);
        expect(discarded, [1]);
        gate.complete();
        await pumpFrames(tester);
        expect(app.claktories.selected.drafts.of('agent'), isEmpty);
        expect(incoming.pending, isEmpty);
        noSend(app);
      },
    );

    testWidgets(
      'a destination disappearing before selection acknowledgement stays retryable',
      (tester) async {
        final gate = Completer<void>();
        final app = _App(selectionGate: gate.future);
        await app.show(tester, incoming: incoming);
        await share(tester);
        await choose(tester, 'Origin agent');
        final backend = app.backends[_first]!;
        backend.workspace.apply(
          backend.workspace.workspace.deepCopy()..agents.clear(),
        );
        gate.complete();
        await pumpFrames(tester);
        expect(
          find.text('That conversation is gone. Choose another.'),
          findsOneWidget,
        );
        expect(incoming.pending.single.id, 1);
        await choose(tester, 'Other agent');
        expect(app.connections.current?.code, _second);
        expect(
          app.claktories.selected.drafts.of('agent'),
          'Shared 日本語 👩🏽‍💻',
        );
        expect(discarded, isEmpty);
        noSend(app);
      },
    );

    testWidgets('two warm shares stay ordered through window replacement', (
      tester,
    ) async {
      final app = _App();
      await app.show(tester, incoming: incoming);
      await share(tester, text: 'First');
      await share(tester, text: 'Second', id: 2);
      expect(find.text('First'), findsOneWidget);
      expect(find.text('Second'), findsNothing);
      await choose(tester, 'Other agent');
      expect(app.connections.current?.code, _second);
      expect(find.byType(AndroidSharePicker), findsOneWidget);
      expect(find.text('Second'), findsOneWidget);
      await choose(tester, 'Origin agent');
      expect(app.connections.current?.code, _first);
      expect(app.claktories.selected.drafts.of('agent'), 'Second');
      final other = app.claktories.all.firstWhere(
        (root) => root.target?.code == _second,
      );
      expect(other.drafts.of('agent'), 'First');
      expect(discarded, isEmpty);
      expect(incoming.pending, isEmpty);
      noSend(app);
    });

    testWidgets(
      'offline cold share names each connection, then shows real destinations on recovery',
      (tester) async {
        final app = _App();
        await app.show(tester, incoming: incoming);
        final origin = app.backends[_first]!;
        final saved = origin.workspace.workspace.deepCopy();
        for (final backend in app.backends.values) {
          backend.connected.value = false;
          backend.workspace.clear();
        }
        await share(tester);
        Finder status(String text) => find.descendant(
          of: find.byType(AndroidSharePicker),
          matching: find.text(text),
        );
        expect(status('Connecting to Origin…'), findsOneWidget);
        expect(status('Connecting to Other…'), findsOneWidget);
        expect(status('Loading…'), findsNothing);
        expect(status('No agents or chats yet.'), findsNothing);
        expect(choice('Origin agent'), findsNothing);
        await tester.pump(const Duration(seconds: 31));
        expect(status('Connecting to Origin…'), findsOneWidget);
        expect(incoming.pending.single.id, 1);
        expect(discarded, isEmpty);

        origin.reconnecting.value = true;
        await tester.pump();
        expect(status('Reconnecting…'), findsOneWidget);
        expect(status('Connecting to Origin…'), findsNothing);
        origin.connected.value = true;
        await tester.pump();
        expect(status('Loading…'), findsOneWidget);
        expect(choice('Origin agent'), findsNothing);
        origin.workspace.apply(saved);
        await pumpFrames(tester);
        expect(status('Loading…'), findsNothing);
        expect(choice('Origin agent'), findsOneWidget);
        expect(choice('Other agent'), findsNothing);
        expect(status('Connecting to Other…'), findsOneWidget);
        await choose(tester, 'Origin agent');
        expect(
          app.claktories.selected.drafts.of('agent'),
          'Shared 日本語 👩🏽‍💻',
        );
        expect(discarded, isEmpty);
        noSend(app);
      },
    );

    testWidgets(
      'one unavailable Claktory does not hold up another Claktory share choice',
      (tester) async {
        final app = _App();
        await app.show(tester, incoming: incoming);
        final other = app.backends[_second]!;
        other.connected.value = false;
        other.workspace.clear();
        await share(tester);
        expect(choice('Origin agent'), findsOneWidget);
        expect(choice('Other agent'), findsNothing);
        expect(
          find.descendant(
            of: find.byType(AndroidSharePicker),
            matching: find.text('Loading…'),
          ),
          findsNothing,
        );
        await choose(tester, 'Origin agent');
        expect(
          app.claktories.selected.drafts.of('agent'),
          'Shared 日本語 👩🏽‍💻',
        );
        expect(incoming.pending, isEmpty);
        noSend(app);
      },
    );

    testWidgets(
      'cached destinations show live connection changes without a workspace update',
      (tester) async {
        final app = _App();
        await app.show(tester, incoming: incoming);
        await share(tester);
        final origin = app.backends[_first]!;
        Finder status(String text) => find.descendant(
          of: find.byType(AndroidSharePicker),
          matching: find.text(text),
        );
        origin.connected.value = false;
        await tester.pump();
        expect(status('Connecting to Origin…'), findsOneWidget);
        origin.refused.value = 'unknown device';
        await tester.pump();
        expect(status("Origin doesn't allow this device"), findsOneWidget);
        expect(status('Connecting to Origin…'), findsNothing);
        origin.refused.value = null;
        origin.connected.value = true;
        await tester.pump();
        expect(status("Origin doesn't allow this device"), findsNothing);
        expect(status('Loading…'), findsNothing);
        await tester.tap(find.widgetWithText(Button, 'Cancel'));
        await pumpFrames(tester);
        origin.connected.value = false;
        await tester.pump();
        expect(find.byType(AndroidSharePicker), findsNothing);
        expect(tester.takeException(), isNull);
        expect(discarded, [1]);
        expect(app.claktories.selected.drafts.of('agent'), isEmpty);
        noSend(app);
      },
    );

    testWidgets(
      'unpaired share explains connection without creating a conversation',
      (tester) async {
        final app = _App();
        await app.show(tester, incoming: incoming, paired: false);
        await share(tester);
        expect(
          find.text('Connect this $thisDeviceKind to your Claktory first.'),
          findsOneWidget,
        );
        expect(choice('Other agent'), findsNothing);
        await tester.tap(find.widgetWithText(Button, 'Cancel'));
        await pumpFrames(tester);
        expect(find.byType(PairScreen), findsOneWidget);
        expect(discarded, [1]);
        expect(app.connections.saved, isEmpty);
      },
    );

    testWidgets('320x568 AX5 keeps destinations and Cancel reachable', (
      tester,
    ) async {
      final app = _App();
      await app.show(tester, incoming: incoming);
      tester.view.physicalSize = const Size(320, 568);
      tester.platformDispatcher.textScaleFactorTestValue = 3.12;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await tester.pump();
      await share(tester, text: 'A long shared text ' * 1000);
      expect(tester.takeException(), isNull);
      final cancel = find.widgetWithText(Button, 'Cancel');
      expect(tester.getSize(cancel).height, greaterThanOrEqualTo(48));
      await tester.scrollUntilVisible(
        choice('Other agent'),
        160,
        scrollable: find.descendant(
          of: find.byType(AndroidSharePicker),
          matching: find.byType(Scrollable),
        ),
      );
      await tester.pump();
      expect(choice('Other agent'), findsOneWidget);
      expect(
        tester.getSize(choice('Other agent')).height,
        greaterThanOrEqualTo(48),
      );
      expect(tester.takeException(), isNull);
      await tester.tap(cancel);
      await pumpFrames(tester);
      expect(discarded, [1]);
      noSend(app);
    });
    testWidgets(
      'unavailable model is marked and cannot hide a new draft behind a blocked composer',
      (tester) async {
        final app = _App();
        await app.show(tester, incoming: incoming);
        final backend = app.backends[_first]!;
        backend.workspace.apply(
          backend.workspace.workspace.deepCopy()
            ..agents[0].modelAvailable = false,
        );
        await share(tester);
        expect(tester.widget<Button>(choice('Origin agent')).onPressed, isNull);
        expect(
          find.descendant(
            of: find.byType(AndroidSharePicker),
            matching: find.text('No model available'),
          ),
          findsOneWidget,
        );
        expect(
          backend.sent.where((message) => message.hasSelectAgent()),
          isEmpty,
        );
        await choose(tester, 'Other agent');
        expect(app.connections.current?.code, _second);
        expect(
          app.claktories.selected.drafts.of('agent'),
          'Shared 日本語 👩🏽‍💻',
        );
        noSend(app);
      },
    );

    testWidgets(
      'a missing cache copy is explained after preserving the text draft',
      (tester) async {
        final app = _App();
        await app.show(tester, incoming: incoming);
        final chosen = app.claktories.selected;
        await share(tester, files: ['/cache/claktory-test-missing/file.txt']);
        final staged = Completer<void>();
        chosen.backend.attachments.addListener(() {
          if (!staged.isCompleted) staged.complete();
        });
        await choose(tester, 'Origin agent', staged: staged.future);
        expect(
          find.text("Some files couldn't be added. Try sharing them again."),
          findsOneWidget,
        );
        expect(chosen.drafts.of('agent'), 'Shared 日本語 👩🏽‍💻');
        expect(chosen.backend.attachments.of('agent'), isEmpty);
        await tester.tap(find.widgetWithText(Button, 'Close'));
        await pumpFrames(tester);
        expect(discarded, isEmpty);
        noSend(app);
      },
    );
  });
}
