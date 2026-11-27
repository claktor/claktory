// Files dragged onto an iPad's window from Files, Photos or another
// app. The native side (Drop.swift) speaks the Mac window's own
// `factory/window` calls, with one more: an iPad's drop is staged
// (`filesDropping` {kinds, ticket, x, y} at once, `filesDropped` {paths,
// ticket, x, y} once copied), so the box shows "Adding photo…" while a long
// video is copied, as the paperclip's photos do; and `textDropped` {text,
// x, y} for text, which Flutter's own fields don't take on iOS.
import 'dart:io';

import 'package:factory_app/attachments.dart';
import 'package:factory_app/drop.dart';
import 'package:factory_app/identity.dart' show isPhoneLayout;
import 'package:factory_app/rail.dart' show PhoneClaktories;
import 'package:factory_app/scope.dart' show AppPage;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/shell.dart' show Shell;
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import '../attachments_test.dart' show fromWindow;
import '../drafts_test.dart' show two;
import '../harness.dart';
import 'as_ipad.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  late Directory folder;
  late String notes;
  late String video;
  final calls = <MethodCall>[];
  const channel = MethodChannel('factory/window');

  setUp(() {
    folder = Directory.systemTemp.createTempSync('ipad-drop');
    notes = '${folder.path}/notes.txt';
    video = '${folder.path}/Beach.mov';
    File(notes).writeAsStringSync('x' * 2048);
    File(video).writeAsBytesSync(List.filled(4096, 1));
    calls.clear();
    messenger.setMockMethodCallHandler(channel, (call) async {
      calls.add(call);
      return null;
    });
  });

  tearDown(() {
    folder.deleteSync(recursive: true);
    messenger.setMockMethodCallHandler(channel, null);
    FileDrop.dragged(0);
  });

  /// The iPad's window: the shell, the drop overlay over it all.
  Future<Harness> show(
    WidgetTester tester, {
    Size size = const Size(1194, 834),
  }) async {
    asIPad();
    final harness = Harness(two());
    await harness.show(
      tester,
      size: size,
      home: const Stack(
        children: [
          Positioned.fill(child: Shell()),
          Positioned.fill(child: WindowDropOverlay()),
        ],
      ),
    );
    await tester.pump();
    return harness;
  }

  testWidgets('an iPad takes drops while a message box shows, and dims '
      'saying where they go', (tester) async {
    final harness = await show(tester);
    expect(FileDrop.windowDrops, isTrue);
    expect(
      calls.where((call) => call.method == 'takeFiles').last.arguments,
      isTrue,
    );
    await fromWindow(tester, 'filesOver', {'count': 2, 'x': 900, 'y': 400});
    await tester.pump(const Duration(milliseconds: 300));
    final name = harness.backend.workspace.selectedAgent!.name;
    expect(find.text('Drop to add to your message to $name'), findsOneWidget);
    expect(find.text('2 files'), findsOneWidget);
    await fromWindow(tester, 'filesOver', 0);
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.text('2 files'), findsNothing);
  });

  testWidgets('a staged drop shows "Adding video…" at once, then the files '
      'once they\'re copied', (tester) async {
    final harness = await show(tester);
    final agent = harness.backend.workspace.selectedAgent!.id;
    await fromWindow(tester, 'filesOver', {'count': 2, 'x': 900, 'y': 400});
    await fromWindow(tester, 'filesDropping', {
      'kinds': ['video', 'file'],
      'ticket': 7,
      'x': 900,
      'y': 400,
    });
    expect(FileDrop.over.value, isFalse, reason: 'dropped: the dim goes');
    expect(find.text(AddingChip.label('video')), findsOneWidget);
    expect(find.text(AddingChip.label('file')), findsOneWidget);
    expect(harness.backend.attachments.of(agent), isEmpty);

    await fromWindow(tester, 'filesDropped', {
      'paths': [video, notes],
      'ticket': 7,
      'x': 900,
      'y': 400,
    });
    await tester.pump();
    expect(find.text(AddingChip.label('video')), findsNothing);
    expect(find.text('Beach.mov'), findsOneWidget);
    expect(find.text('notes.txt'), findsOneWidget);
    expect(harness.backend.attachments.of(agent), hasLength(2));
  });

  testWidgets('a staged drop whose copies never come stops saying Adding, '
      'and says it wasn\'t added', (tester) async {
    final harness = await show(tester);
    final agent = harness.backend.workspace.selectedAgent!.id;
    await fromWindow(tester, 'filesDropping', {
      'kinds': ['photo'],
      'ticket': 8,
      'x': 900,
      'y': 400,
    });
    expect(find.text(AddingChip.label('photo')), findsOneWidget);
    // The window lost the drag: nothing else comes.
    await fromWindow(tester, 'filesOver', 0);
    expect(find.text(AddingChip.label('photo')), findsOneWidget);

    await tester.pump(FileDrop.stagedWait);
    await tester.pump();
    expect(find.text(AddingChip.label('photo')), findsNothing);
    expect(find.text("Couldn't add this photo. Try again."), findsOneWidget);
    expect(harness.backend.attachments.of(agent), isEmpty);

    // The copy turning up after all still lands in the box.
    await fromWindow(tester, 'filesDropped', {
      'paths': [notes],
      'ticket': 8,
      'x': 900,
      'y': 400,
    });
    await tester.pump();
    expect(find.text('notes.txt'), findsOneWidget);
  });

  testWidgets('copies that failed say so in the paperclip\'s words', (
    tester,
  ) async {
    await show(tester);
    await fromWindow(tester, 'filesDropping', {
      'kinds': ['photo', 'photo'],
      'ticket': 9,
      'x': 900,
      'y': 400,
    });
    await fromWindow(tester, 'filesDropped', {
      'paths': [notes],
      'ticket': 9,
      'x': 900,
      'y': 400,
    });
    await tester.pump();
    expect(find.text(AddingChip.label('photo')), findsNothing);
    expect(find.text('notes.txt'), findsOneWidget);
    expect(find.text("One photo wasn't added. Try again."), findsOneWidget);
  });

  testWidgets('a drop without a ticket attaches as the Mac\'s does', (
    tester,
  ) async {
    final harness = await show(tester);
    await fromWindow(tester, 'filesDropped', {
      'paths': [notes],
      'x': 900,
      'y': 400,
    });
    await tester.pump();
    expect(find.text('notes.txt'), findsOneWidget);
    expect(
      harness.backend.attachments.of(
        harness.backend.workspace.selectedAgent!.id,
      ),
      hasLength(1),
    );
  });

  for (final size in const [Size(320, 1024), Size(507, 834)]) {
    testWidgets('in Slide Over or Split View (${size.width.toInt()} wide) the '
        'overlay shows too, clear of the edges', (tester) async {
      await show(tester, size: size);
      await fromWindow(tester, 'filesOver', {
        'count': 3,
        'x': size.width / 2,
        'y': 300,
      });
      await tester.pump(const Duration(milliseconds: 300));
      expect(find.text('3 files'), findsOneWidget);
      expect(tester.takeException(), isNull);
      final card = tester.getRect(
        find
            .ancestor(
              of: find.text('3 files'),
              matching: find.byType(Container),
            )
            .first,
      );
      expect(card.left, greaterThanOrEqualTo(16));
      expect(card.right, lessThanOrEqualTo(size.width - 16));
    });
  }

  testWidgets('text dropped from another app goes into the message box '
      '(Flutter\'s own fields take no drops on iOS)', (tester) async {
    await show(tester);
    expect(
      calls.where((call) => call.method == 'takeText').last.arguments,
      isTrue,
      reason: 'a message box shows',
    );
    await tester.enterText(find.byType(EditableText).last, 'See ');
    await tester.pump();
    await fromWindow(tester, 'textDropped', {
      'text': 'https://claktory.com',
      'x': 900,
      'y': 700,
    });
    await tester.pump();
    final field = tester.widget<EditableText>(find.byType(EditableText).last);
    expect(field.controller.text, 'See https://claktory.com');
    expect(field.controller.selection.baseOffset, field.controller.text.length);
  });

  testWidgets('in the phone\'s layout, a page pushed over the conversation '
      '(Settings) takes no drops for the box or the request under it', (
    tester,
  ) async {
    asIPad();
    final snapshot = two();
    snapshot.agents.first.waitingSeqs.add(Int64(4));
    final harness = Harness(snapshot);
    harness.backend.transcript.load(
      Transcript(
        agentId: snapshot.selectedAgentId,
        entries: [
          Entry(
            seq: Int64(4),
            timestampMs: Int64(1004),
            askPerson: AskPerson(
              id: 'ask4',
              title: 'Send me the report',
              state: AskPersonState.ASK_PERSON_STATE_PENDING,
              revision: 1,
              wantsText: true,
            ),
          ),
        ],
      ),
    );
    await harness.show(
      tester,
      size: const Size(504, 834),
      home: Stack(
        children: [
          Positioned.fill(
            child: Builder(
              builder: (context) => isPhoneLayout(context)
                  ? const PhoneClaktories(shell: Shell())
                  : const Shell(),
            ),
          ),
          const Positioned.fill(child: WindowDropOverlay()),
        ],
      ),
    );
    await settle(tester);
    final agent = harness.backend.workspace.selectedAgent!.id;
    expect(FileDrop.takesFiles.value, isTrue);
    expect(find.text('Send me the report'), findsWidgets);

    harness.page.value = AppPage.settings;
    await settle(tester);
    expect(FileDrop.takesFiles.value, isNull);
    expect(
      calls.where((call) => call.method == 'takeFiles').last.arguments,
      isFalse,
    );
    await fromWindow(tester, 'filesOver', {'count': 1, 'x': 250, 'y': 400});
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.text('Open a conversation to add files'), findsOneWidget);
    await fromWindow(tester, 'filesDropped', {
      'paths': [notes],
      'x': 250,
      'y': 400,
    });
    await fromWindow(tester, 'textDropped', {
      'text': 'hidden',
      'x': 250,
      'y': 400,
    });
    expect(harness.backend.attachments.of(agent), isEmpty);

    harness.page.value = AppPage.agent;
    await settle(tester);
    expect(FileDrop.takesFiles.value, isTrue);
    await fromWindow(tester, 'filesDropped', {
      'paths': [notes],
      'x': 250,
      'y': 400,
    });
    await tester.pump();
    expect(harness.backend.attachments.of(agent), hasLength(1));
    expect(find.text('hidden'), findsNothing);
  });

  for (final size in const [Size(320, 1024), Size(1194, 834)]) {
    testWidgets('at ${size.width.toInt()} wide and the largest text, a chip '
        'keeps its name, size and × whole', (tester) async {
      asIPad();
      final harness = Harness(two());
      tester.platformDispatcher.textScaleFactorTestValue = 3.1;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      await harness.show(tester, size: size, home: const Shell());
      await settle(tester);
      await fromWindow(tester, 'filesDropped', {
        'paths': [notes],
        'x': 10,
        'y': 10,
      });
      await tester.pump();
      expect(tester.takeException(), isNull);
      final chip = find.byType(AttachmentChip);
      expect(chip, findsOneWidget);
      final box = tester.getRect(chip);
      final name = tester.getRect(find.text('notes.txt'));
      final bytes = tester.getRect(find.text('2.0 KB'));
      expect(name.width, greaterThan(40));
      expect(box.contains(name.bottomRight - const Offset(1, 1)), isTrue);
      expect(box.contains(bytes.bottomRight - const Offset(1, 1)), isTrue);
      final remove = tester.getRect(find.bySemanticsLabel('Remove notes.txt'));
      expect(remove.height, greaterThanOrEqualTo(44));
      expect(box.contains(remove.center), isTrue);
    });
  }
}
