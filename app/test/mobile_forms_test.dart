// Forms on a phone (#198, phase 2A): a sheet with Cancel and the action
// along its top as iOS lays one out, the fields scrolling under them, each
// bringing the keyboard it needs; the folder browser's rows a finger's
// size.
import 'package:factory_app/agent_view.dart' show AgentView;
import 'package:factory_app/controls.dart';
import 'package:factory_app/folder_browser.dart';
import 'package:factory_app/forms.dart';
import 'package:factory_app/connectors.dart' show ConnectServiceForm;
import 'package:factory_app/devices.dart' show PairDeviceForm;
import 'package:factory_app/mobile/input.dart';
import 'package:factory_app/mobile/platform_policy.dart';
import 'package:factory_app/mobile/presentation.dart' show showEditor;
import 'package:factory_app/secrets.dart' show EnvForm, SecretForm;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/rendering.dart' show RenderParagraph;
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'phone_sizes_test.dart' show loadGeist;
import 'phone_test.dart' show phone, workspace;

/// Shows [form] in a sheet over a phone's conversation.
Future<Harness> _showForm(
  WidgetTester tester,
  Widget Function(VoidCallback close) form, {
  Size size = phone,
}) async {
  final harness = Harness(workspace());
  await harness.show(tester, size: size);
  await pumpFrames(tester);
  showModal(
    tester.element(find.byType(AgentView)),
    (context, close) => form(close),
  );
  await pumpFrames(tester);
  return harness;
}

Widget _form({
  VoidCallback? onSubmit,
  String? error,
  VoidCallback? onCancel,
  List<Widget>? fields,
}) => FormFrame(
  title: 'New project',
  body: 'A folder of its own in the workspace.',
  fields:
      fields ??
      [
        Field(
          label: 'Name',
          child: TextField(
            controller: TextEditingController(),
            kind: InputKind.text,
          ),
        ),
      ],
  error: error,
  submitLabel: 'Create',
  onSubmit: onSubmit,
  onCancel: onCancel ?? () {},
);

void main() {
  // Geist, as the app ships it: whether the bar's words fit depends on it.
  setUpAll(loadGeist);
  testWidgets('Cancel, the title and the action share the top row; the '
      'fields come under them', (tester) async {
    var created = 0;
    var cancelled = 0;
    await _showForm(
      tester,
      (close) => _form(onSubmit: () => created++, onCancel: () => cancelled++),
    );
    final cancel = tester.getRect(find.text('Cancel'));
    final title = tester.getRect(find.text('New project'));
    final create = tester.getRect(find.text('Create'));
    final field = tester.getRect(find.byType(EditableText).last);
    expect(cancel.center.dy, closeTo(title.center.dy, 1));
    expect(create.center.dy, closeTo(title.center.dy, 1));
    expect(cancel.right, lessThan(title.left));
    expect(create.left, greaterThan(title.right));
    expect(field.top, greaterThan(title.bottom));
    // Each a finger's size.
    expect(
      tester.getSize(find.bySemanticsLabel('Create')).height,
      greaterThanOrEqualTo(44),
    );
    await tester.tap(find.text('Create'));
    await tester.tap(find.text('Cancel'));
    expect(created, 1);
    expect(cancelled, 1);
  });

  testWidgets("an action too long for the bar's quarter takes half of it, "
      'the title under it, so no word is cut short', (tester) async {
    await _showForm(
      tester,
      (close) => FormFrame(
        title: 'Save your recovery key',
        fields: [
          Field(
            label: 'Name',
            child: TextField(controller: TextEditingController()),
          ),
        ],
        error: null,
        submitLabel: "I've saved it",
        onSubmit: () {},
        cancelLabel: 'Not now',
        onCancel: () {},
      ),
      size: const Size(375, 667),
    );
    final action = find.text("I've saved it");
    final title = tester.getRect(find.text('Save your recovery key'));
    expect(tester.getRect(action).top, lessThan(title.top));
    // Whole: one line, not ellipsized.
    final paragraph = tester.firstRenderObject<RenderParagraph>(
      find.descendant(of: action, matching: find.byType(RichText)),
    );
    expect(paragraph.didExceedMaxLines, isFalse);
    // A short one still shares the row with the title.
    expect(
      tester.getRect(find.text('Not now')).center.dy,
      closeTo(tester.getRect(action).center.dy, 1),
    );
  });

  testWidgets("the top row's words are iOS's 44pt targets, Android's 48dp", (
    tester,
  ) async {
    await _showForm(tester, (close) => _form(onSubmit: () {}));
    for (final label in ['Cancel', 'Create']) {
      final size = tester.getSize(find.bySemanticsLabel(label));
      expect(size.height, 44, reason: label);
      expect(size.width, greaterThanOrEqualTo(44), reason: label);
    }
    await tester.pumpWidget(const SizedBox());

    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    PhonePlatformPolicy.debugAndroidPolicy = true;
    addTearDown(() {
      debugDefaultTargetPlatformOverride = null;
      PhonePlatformPolicy.debugAndroidPolicy = false;
    });
    await _showForm(tester, (close) => _form(onSubmit: () {}));
    for (final label in ['Cancel', 'Create']) {
      final size = tester.getSize(find.bySemanticsLabel(label));
      expect(size.height, greaterThanOrEqualTo(48), reason: label);
      expect(size.width, greaterThanOrEqualTo(48), reason: label);
    }
  });

  testWidgets('the action reads as unavailable while it is', (tester) async {
    await _showForm(tester, (close) => _form());
    expect(
      tester.getSemantics(find.bySemanticsLabel('Create')),
      matchesSemantics(label: 'Create', isButton: true, hasEnabledState: true),
    );
  });

  testWidgets('why it was refused shows under the title', (tester) async {
    await _showForm(
      tester,
      (close) => _form(onSubmit: () {}, error: 'A project has that name.'),
    );
    final error = tester.getRect(find.text('A project has that name.'));
    expect(error.top, greaterThan(tester.getRect(find.text('Cancel')).bottom));
    expect(error.bottom, lessThan(tester.getRect(find.text('Name')).top));
  });

  testWidgets('a dialog with nothing to fill in keeps its buttons at its '
      'foot', (tester) async {
    await _showForm(
      tester,
      (close) => FormFrame(
        title: 'Remove this device?',
        fields: const [],
        error: null,
        submitLabel: 'Remove',
        onSubmit: () {},
        onCancel: () {},
      ),
    );
    expect(
      tester.getRect(find.text('Remove')).top,
      greaterThan(tester.getRect(find.text('Remove this device?')).bottom),
    );
  });

  testWidgets('at the largest text, a dialog with nothing to fill in '
      'stacks its buttons, the action first, and scrolls', (tester) async {
    tester.platformDispatcher.textScaleFactorTestValue = 3.1;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    await _showForm(
      tester,
      (close) => FormFrame(
        title: 'Connected to mac-mini',
        body:
            'Directly, since 09:41. Reconnect connects afresh at once, '
            'which can help when messages seem stuck.',
        fields: const [],
        error: "Can't reach mac-mini: timed out",
        submitLabel: 'Reconnect',
        cancelLabel: 'Close',
        onSubmit: () {},
        onCancel: () {},
        secondary: Button(label: 'Switch Claktory…', onPressed: () {}),
      ),
      size: const Size(320, 568),
    );
    expect(tester.takeException(), isNull);
    final action = tester.getRect(find.text('Reconnect'));
    final cancel = tester.getRect(find.text('Close').last);
    expect(action.bottom, lessThanOrEqualTo(cancel.top), reason: 'stacked');
    // All of it is reachable, scrolling.
    await tester.dragUntilVisible(
      find.text('Switch Claktory…'),
      find.byType(SingleChildScrollView).last,
      const Offset(0, -200),
    );
    expect(
      tester.getRect(find.text('Switch Claktory…')).bottom,
      lessThanOrEqualTo(568),
    );
  });

  for (final width in [320.0, 440.0]) {
    testWidgets('at the largest text its top row fits $width points', (
      tester,
    ) async {
      tester.platformDispatcher.textScaleFactorTestValue = 3.1;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await _showForm(
        tester,
        (close) => _form(onSubmit: () {}),
        size: Size(width, 700),
      );
      expect(tester.takeException(), isNull);
      expect(find.text('Cancel'), findsOneWidget);
      expect(find.text('Create'), findsOneWidget);
    });
  }

  testWidgets('fields bring the keyboard their content needs', (tester) async {
    await _showForm(
      tester,
      (close) => _form(
        onSubmit: () {},
        fields: [
          TextField(controller: TextEditingController(), kind: InputKind.text),
          TextField(
            controller: TextEditingController(),
            kind: InputKind.secret,
          ),
          TextField(
            controller: TextEditingController(),
            kind: InputKind.multiline,
            maxLines: 3,
          ),
        ],
      ),
    );
    final fields = tester
        .widgetList<EditableText>(find.byType(EditableText))
        .toList()
        .reversed
        .take(3)
        .toList()
        .reversed
        .toList();
    expect(fields[0].textCapitalization, TextCapitalization.sentences);
    expect(fields[1].obscureText, isTrue);
    expect(fields[1].autocorrect, isFalse);
    expect(fields[2].keyboardType, TextInputType.multiline);
  });

  testWidgets("the folder browser's rows and crumbs are a finger's size", (
    tester,
  ) async {
    final harness = Harness(workspace());
    harness.backend.folderLists['~/code'] = FolderList(
      path: '/Users/me/code',
      home: '/Users/me',
      parent: '/Users/me',
      folders: [
        FolderEntry(name: 'factory'),
        FolderEntry(name: 'website'),
      ],
    );
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    showModal(
      tester.element(find.byType(AgentView)),
      (context, close) => FolderBrowser(
        backend: harness.backend,
        from: '~/code',
        machine: 'mac-mini',
        onChoose: (_) {},
        onCancel: close,
      ),
    );
    await pumpFrames(tester);
    expect(
      tester
          .getSize(
            find.ancestor(
              of: find.text('factory'),
              matching: find.byType(Pressable),
            ),
          )
          .height,
      greaterThanOrEqualTo(44),
    );
    expect(
      tester
          .getSize(
            find.ancestor(of: find.text('~'), matching: find.byType(Pressable)),
          )
          .height,
      greaterThanOrEqualTo(44),
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('a form with changes asks before Cancel or a drag drops '
      'them; Keep editing keeps it', (tester) async {
    var cancelled = 0;
    var dirty = false;
    late StateSetter setForm;
    await _showForm(
      tester,
      (close) => StatefulBuilder(
        builder: (context, setState) {
          setForm = setState;
          return FormFrame(
            title: 'Project settings',
            fields: [
              TextField(controller: TextEditingController(text: 'Factory')),
            ],
            error: null,
            submitLabel: 'Save',
            onSubmit: () {},
            onCancel: () {
              cancelled++;
              close();
            },
            dirty: dirty,
          );
        },
      ),
    );
    setForm(() => dirty = true);
    await pumpFrames(tester);

    await tester.tap(find.text('Cancel'));
    await pumpFrames(tester);
    expect(find.text('Discard changes'), findsOneWidget);
    await tester.tap(find.text('Keep editing'));
    await pumpFrames(tester);
    expect(cancelled, 0);
    expect(find.text('Project settings'), findsOneWidget);

    // Tapping the dimmed conversation asks too.
    await tester.tapAt(const Offset(200, 20));
    await pumpFrames(tester);
    expect(find.text('Discard changes'), findsOneWidget);
    await tester.tap(find.text('Keep editing'));
    await pumpFrames(tester);
    expect(find.text('Project settings'), findsOneWidget);

    await tester.tap(find.text('Cancel'));
    await pumpFrames(tester);
    await tester.tap(find.text('Discard changes'));
    await pumpFrames(tester);
    expect(cancelled, 1);
    expect(find.text('Project settings'), findsNothing);
  });

  testWidgets('without changes, Cancel and the dimmed screen close it at '
      'once', (tester) async {
    await _showForm(tester, (close) => _form(onSubmit: () {}, onCancel: close));
    await tester.tapAt(const Offset(200, 20));
    await pumpFrames(tester);
    expect(find.text('Discard changes'), findsNothing);
    expect(find.text('New project'), findsNothing);
  });

  testWidgets("a sheet's shouldClose holds Close and the dimmed screen, "
      'not close from inside', (tester) async {
    final harness = Harness(workspace());
    await harness.show(tester, size: phone);
    await pumpFrames(tester);
    var allow = false;
    var asked = 0;
    late VoidCallback inside;
    showEditor(
      tester.element(find.byType(AgentView)),
      title: 'Comment',
      shouldClose: () async {
        asked++;
        return allow;
      },
      builder: (context, close) {
        inside = close;
        return const Text('A comment');
      },
    );
    await pumpFrames(tester);
    await tester.tap(find.bySemanticsLabel('Close').last);
    await pumpFrames(tester);
    // A tall sheet leaves the dimmed screen only above it.
    await tester.tapAt(const Offset(200, 3));
    await pumpFrames(tester);
    expect(asked, 2);
    expect(find.text('A comment'), findsOneWidget);
    inside();
    await pumpFrames(tester);
    expect(asked, 2);
    expect(find.text('A comment'), findsNothing);
  });

  // Each form at the largest text, on the smallest and largest phones:
  // nothing overflows anywhere in it.
  final forms = <String, Widget Function(Workspace, VoidCallback)>{
    'new project': (w, close) => CreateProjectForm(close: close),
    'project settings': (w, close) =>
        ProjectSettingsForm(project: w.projects.first, close: close),
    'new agent': (w, close) =>
        CreateAgentForm(projectId: w.projects.first.id, close: close),
    'agent settings': (w, close) =>
        AgentSettingsForm(agent: w.agents.first, close: close),
    'new secret': (w, close) => SecretForm(close: close),
    'secret from env': (w, close) => EnvForm(close: close),
    'connect a service': (w, close) => ConnectServiceForm(close: close),
    'add a device': (w, close) => PairDeviceForm(close: close),
  };
  for (final MapEntry(key: name, value: form) in forms.entries) {
    for (final width in [320.0, 440.0]) {
      testWidgets('$name fits $width points at the largest text', (
        tester,
      ) async {
        tester.platformDispatcher.textScaleFactorTestValue = 3.1;
        addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
        final errors = <String>[];
        final old = FlutterError.onError;
        FlutterError.onError = (details) {
          errors.add(
            details
                .toString()
                .split('\n')
                .where((line) => line.contains('.dart:'))
                .take(2)
                .join(' '),
          );
          old?.call(details);
        };
        addTearDown(() => FlutterError.onError = old);
        final snapshot = workspace();
        final harness = Harness(snapshot);
        await harness.show(tester, size: Size(width, 700));
        await pumpFrames(tester);
        showModal(
          tester.element(find.byType(AgentView)),
          (context, close) => form(snapshot, close),
        );
        await pumpFrames(tester);
        final scrollables = find.descendant(
          of: find.byType(FormFrame),
          matching: find.byType(Scrollable),
        );
        for (var i = 0; i < 10 && scrollables.evaluate().isNotEmpty; i++) {
          await tester.drag(
            scrollables.first,
            const Offset(0, -300),
            warnIfMissed: false,
          );
          await pumpFrames(tester);
        }
        tester.takeException();
        expect(errors, isEmpty);
      });
    }
  }

  testWidgets('an error that comes after the person scrolled down to the '
      'end is scrolled into sight (Zucchero, row 56)', (tester) async {
    String? error;
    await _showForm(
      tester,
      (close) => StatefulBuilder(
        builder: (context, setState) => _form(
          error: error,
          onSubmit: () => setState(
            () => error =
                'This file isn’t the one '
                'you reviewed.',
          ),
          fields: [
            for (var i = 0; i < 30; i++)
              Field(label: 'Line $i', child: const SizedBox(height: 40)),
          ],
        ),
      ),
    );
    await tester.dragUntilVisible(
      find.text('Line 29'),
      find.text('Line 0'),
      const Offset(0, -300),
    );
    await pumpFrames(tester);
    await tester.tap(find.text('Create'));
    await tester.pumpAndSettle();
    final shown = tester.getRect(
      find.text(
        'This file isn’t the one you '
        'reviewed.',
      ),
    );
    expect(shown.top, greaterThan(0));
    expect(shown.bottom, lessThan(phone.height));
    expect(
      shown.top,
      greaterThan(tester.getRect(find.text('Create')).bottom),
      reason: 'under the bar, not behind it',
    );
  });
}
