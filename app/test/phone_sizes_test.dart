import 'dart:convert';
import 'dart:io';

import 'package:factory_app/agent_view.dart';
import 'package:factory_app/phone.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scanner.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/sidebar.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

/// The iPhones the app is checked on: their screens in points, and their
/// safe areas (status bar or Dynamic Island, home indicator).
const _phones =
    <
      String,
      ({Size size, double top, double bottom, double keyboard, double side})
    >{
      'iPhone SE': (
        size: Size(375, 667),
        top: 20.0,
        bottom: 0.0,
        keyboard: 260.0,
        side: 0.0,
      ),
      'iPhone 16': (
        size: Size(393, 852),
        top: 59.0,
        bottom: 34.0,
        keyboard: 336.0,
        side: 0.0,
      ),
      // On its side: the notch at the left, the home indicator below.
      'iPhone 16 on its side': (
        size: Size(852, 393),
        top: 0.0,
        bottom: 21.0,
        keyboard: 209.0,
        side: 59.0,
      ),
      'iPhone 16 Pro Max': (
        size: Size(440, 956),
        top: 62.0,
        bottom: 34.0,
        keyboard: 346.0,
        side: 0.0,
      ),
    };

/// Geist, as the app ships it, so text is as wide as on the phone (the
/// test font's squares overflow where Geist doesn't, and the reverse).
Future<void> loadGeist() async {
  Future<void> family(String name, List<String> files) async {
    final loader = FontLoader(name);
    for (final file in files) {
      loader.addFont(
        Future.value(
          ByteData.sublistView(File('fonts/$file').readAsBytesSync()),
        ),
      );
    }
    await loader.load();
  }

  await family('Geist', [
    'Geist-Regular.ttf',
    'Geist-Medium.ttf',
    'Geist-SemiBold.ttf',
    'Geist-Italic.ttf',
  ]);
  await family('GeistMono', ['GeistMono-Regular.ttf', 'GeistMono-Medium.ttf']);
}

const _long = 'Bartholomew-Alexander Montgomery';
const _role = 'Principal infrastructure and release engineer';

Workspace _workspace() => Workspace(
  projects: [
    Project(
      id: 'p1',
      name: 'A project with a rather long name for a phone',
      path: '/w',
    ),
  ],
  agents: [
    Agent(id: 'rivet', name: 'Rivet', role: 'Admin', modelAvailable: true),
    Agent(
      id: 'long',
      projectId: 'p1',
      name: _long,
      role: _role,
      modelAvailable: true,
      model: 'anthropic/claude-opus-5-5',
      status: AgentStatus.AGENT_STATUS_WORKING,
      extensions: [AgentExtension(id: 'checklist', enabled: true)],
      layout: ViewNode(
        id: 's1',
        split: ViewSplit(
          axis: ViewAxis.VIEW_AXIS_ROW,
          children: [
            ViewNode(
              id: 'chat',
              leaf: ViewLeaf(kind: 'chat'),
            ),
            ViewNode(
              id: 'v1',
              leaf: ViewLeaf(
                kind: 'text',
                path: '/w/a-file-with-a-long-name-that-goes-on.md',
                title: 'a-file-with-a-long-name-that-goes-on.md',
              ),
            ),
          ],
        ),
      ),
    ),
  ],
  extensions: [
    Extension(id: 'checklist', name: 'Checklist', icon: 'list-checks'),
  ],
  selectedAgentId: 'long',
);

void main() {
  setUpAll(loadGeist);

  for (final MapEntry(key: name, value: phone) in _phones.entries) {
    group(name, () {
      Future<Harness> show(WidgetTester tester, Workspace snapshot) async {
        final harness = Harness(snapshot);
        final body = utf8.encode(
          '# A heading long enough to wrap on a phone\n\n'
          '${'A line of text in the file. ' * 12}\n',
        );
        harness.backend.files['/w/a-file-with-a-long-name-that-goes-on.md'] =
            FileContent(
              path: '/w/a-file-with-a-long-name-that-goes-on.md',
              mime: 'text/markdown',
              size: Int64(body.length),
              data: body,
            );
        tester.view.padding = FakeViewPadding(
          top: phone.top,
          bottom: phone.bottom,
          left: phone.side,
          right: phone.side,
        );
        await harness.show(tester, size: phone.size);
        await pumpFrames(tester);
        expect(find.byType(PhoneShell), findsOneWidget, reason: 'a phone');
        return harness;
      }

      testWidgets('the list, a conversation with the keyboard up, its ⋯ '
          'sheet and a pushed viewer fit', (tester) async {
        final harness = await show(
          tester,
          _workspace()..selectedAgentId = 'rivet',
        );
        // The drawer, with the long name and role in it.
        await tester.tap(find.bySemanticsLabel(RegExp(r'^(Menu|Back)$')));
        await pumpFrames(tester);
        final row = find.descendant(
          of: find.byType(Sidebar),
          matching: find.text(_long),
        );
        expect(row, findsOneWidget);
        await tester.tap(row);
        await pumpFrames(tester);
        await harness.apply(tester, _workspace()..selectedAgentId = 'long');
        await pumpFrames(tester);
        expect(find.byType(AgentView), findsOneWidget);

        tester.view.viewInsets = FakeViewPadding(bottom: phone.keyboard);
        await pumpFrames(tester);
        final box = find
            .descendant(
              of: find.byType(AgentView),
              matching: find.byType(EditableText),
            )
            .last;
        expect(
          tester.getBottomLeft(box).dy,
          lessThan(phone.size.height - phone.keyboard),
          reason: 'the message box stays above the keyboard',
        );
        tester.view.viewInsets = FakeViewPadding.zero;
        await pumpFrames(tester);

        // The header starts below the status bar.
        final header = tester.getTopLeft(
          find.descendant(
            of: find.byType(AgentView),
            matching: find.text(_long),
          ),
        );
        expect(header.dy, greaterThanOrEqualTo(phone.top));

        await tester.tap(find.bySemanticsLabel(RegExp('^More options for ')));
        await pumpFrames(tester);
        expect(find.text('Files'), findsOneWidget);
        await tester.tap(find.text('a-file-with-a-long-name-that-goes-on.md'));
        await pumpFrames(tester);
        expect(
          find.text(
            'A heading long enough to wrap on a phone',
            findRichText: true,
          ),
          findsOneWidget,
        );
        // The viewer's own actions are in its More.
        await tester.tap(find.bySemanticsLabel('More').last);
        await pumpFrames(tester);
        await tester.tap(find.text('Plain text'));
        await pumpFrames(tester);
        await tester.tap(find.bySemanticsLabel('Back').last);
        await pumpFrames(tester);
        expect(harness.backend.sent, isNotEmpty);
      });

      testWidgets('Settings fits, all the way down', (tester) async {
        final harness = await show(tester, _workspace());
        harness.page.value = AppPage.settings;
        await pumpFrames(tester);
        for (var i = 0; i < 30; i++) {
          await tester.drag(
            find.byType(Scrollable).first,
            const Offset(0, -400),
            warnIfMissed: false,
          );
          await tester.pump(const Duration(milliseconds: 50));
        }
        await pumpFrames(tester);
      });

      testWidgets('pairing fits, scanning and pasting', (tester) async {
        final harness = Harness(Workspace(), localBackend: false);
        harness.camera.state = CameraAccess.granted;
        tester.view.padding = FakeViewPadding(
          top: phone.top,
          bottom: phone.bottom,
        );
        await harness.show(tester, size: phone.size);
        await pumpFrames(tester);
        expect(find.text('Connect to your Claktory'), findsOneWidget);
        await tester.ensureVisible(find.text('Paste code instead'));
        await pumpFrames(tester);
        await tester.tap(find.text('Paste code instead'));
        await pumpFrames(tester);
        tester.view.viewInsets = FakeViewPadding(bottom: phone.keyboard);
        await pumpFrames(tester);
        // Scrolled to if need be (on its side), never under the keyboard.
        await tester.ensureVisible(find.text('Connect'));
        await pumpFrames(tester);
        expect(
          tester.getBottomLeft(find.text('Connect')).dy,
          lessThan(phone.size.height - phone.keyboard),
          reason: 'Connect stays above the keyboard',
        );
      });
    });
  }
}
