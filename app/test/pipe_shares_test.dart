// Websites shared over pipes (pipe-port-sharing.md's revised plan, Bruno
// 30866; Secco's contract 30935): Share a website… from a pipe's settings or
// a group's menu (a port the Claktory checks, a name, the pipes or the
// group, Claktory's one warning naming the port); the shares on a pipe in
// its settings, this side's with Stop and the other side's with Hide or
// Show; the other side's under the pipe's header with Open (Safari at
// 127.0.0.1), Copy address and Hide; and a pipe moved into or out of a
// group with shares, or a group deleted, saying what that changes.
import 'package:factory_app/connections.dart' show parsePairingCode;
import 'package:factory_app/pipe_shares.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';
import 'selection_test.dart' show watchClipboard;

Pipe _pipe(
  String id,
  String name, {
  String group = '',
  PipeState state = PipeState.PIPE_STATE_OPEN,
}) => Pipe(
  id: id,
  name: name,
  projectId: 'p1',
  groupId: group,
  createdMs: Int64(int.parse(id.substring(1))),
  state: state,
  online: true,
  peer: PipePeer(ownerName: name.split(' · ').last),
);

PipeShare _theirs({
  bool live = true,
  bool hidden = false,
  String problem = '',
}) => PipeShare(
  id: 's1',
  name: 'Dashboard',
  port: 3000,
  pipeIds: ['p3'],
  live: live,
  hidden: hidden,
  listenerProblem: problem,
  address: 'http://s-abc.localhost:3000/',
  fallbackAddress: 'http://127.0.0.1:3000/',
);

PipeShare _mine({
  String group = 'g1',
  List<String> pipes = const ['p3', 'p4'],
}) => PipeShare(
  id: 'm1',
  mine: true,
  name: 'Metrics',
  port: 8080,
  pipeIds: pipes,
  groupId: group,
  live: true,
);

Workspace _workspace({
  List<PipeShare> shares = const [],
  String selected = '',
  PipeState state = PipeState.PIPE_STATE_OPEN,
}) => Workspace(
  projects: [Project(id: 'p1', name: 'Friend Feedback', path: '/w')],
  agents: [
    Agent(id: 'mike', name: 'Mike', projectId: 'p1', modelAvailable: true),
  ],
  pipeGroups: [
    PipeGroup(id: 'g1', projectId: 'p1', name: 'Friends', position: 0),
    PipeGroup(id: 'g2', projectId: 'p1', name: 'Launch', position: 1),
  ],
  pipes: [
    _pipe('p3', 'Feedback · Ana', group: 'g1', state: state),
    _pipe('p4', 'Feedback · Ben', group: 'g1'),
    _pipe('p5', 'Acme launch'),
  ],
  pipeShares: shares,
  selectedAgentId: selected.isEmpty ? 'mike' : '',
  selectedPipeId: selected,
);

/// The share form's fields: 0 the port, 1 the name.
Finder _field(int i) => find
    .descendant(
      of: find.byType(ShareWebsiteForm),
      matching: find.byType(EditableText),
    )
    .at(i);

Finder _row(String id) => find.byKey(ValueKey('pipe-row/$id'));
Finder _group(String id) => find.byKey(ValueKey('pipe-group/$id'));

Future<Harness> _show(
  WidgetTester tester,
  Workspace workspace, {
  Size size = const Size(1200, 1000),
}) async {
  final harness = Harness(workspace);
  harness.backend.pipeChat.load(
    PipeTranscript(pipeId: workspace.selectedPipeId),
  );
  await harness.show(tester, size: size);
  await settle(tester);
  return harness;
}

SharePipeWebsite _shared(Harness harness) => harness.backend.sent
    .lastWhere((message) => message.hasSharePipeWebsite())
    .sharePipeWebsite;

void main() {
  late List<Uri> opened;
  setUp(() {
    opened = [];
    openSharePage = (url) async {
      opened.add(url);
      return true;
    };
    defaultBrowserIsSafari = () async => false;
  });

  group("the other side's website, under the pipe's header", () {
    testWidgets('says who shares what and where, and opens it in the '
        'browser with its one-time address', (tester) async {
      final harness = await _show(
        tester,
        _workspace(shares: [_theirs()], selected: 'p3'),
      );
      expect(find.text('Ana shares Dashboard'), findsOneWidget);
      expect(find.text('http://s-abc.localhost:3000/'), findsOneWidget);
      await tester.tap(find.text('Open'));
      await settle(tester);
      expect(harness.last.openPipeShare.shareId, 's1');
      expect(opened.single.toString(), startsWith('http://s-abc.localhost'));
    });

    testWidgets('in Safari it opens, and says, the 127.0.0.1 address', (
      tester,
    ) async {
      defaultBrowserIsSafari = () async => true;
      await _show(tester, _workspace(shares: [_theirs()], selected: 'p3'));
      expect(
        find.textContaining('http://127.0.0.1:3000/ (Safari'),
        findsOneWidget,
      );
      await tester.tap(find.text('Open'));
      await settle(tester);
      expect(opened.single.toString(), startsWith('http://127.0.0.1:3000/'));
    });

    testWidgets('Safari chosen meanwhile: Open asks again, and opens and '
        'shows the 127.0.0.1 address', (tester) async {
      await _show(tester, _workspace(shares: [_theirs()], selected: 'p3'));
      expect(find.text('http://s-abc.localhost:3000/'), findsOneWidget);
      defaultBrowserIsSafari = () async => true;
      await tester.tap(find.text('Open'));
      await settle(tester);
      expect(opened.single.toString(), startsWith('http://127.0.0.1:3000/'));
      expect(
        find.textContaining('http://127.0.0.1:3000/ (Safari'),
        findsOneWidget,
      );
    });

    testWidgets('Copy address copies it; Hide hides it', (tester) async {
      final copied = watchClipboard(tester);
      final harness = await _show(
        tester,
        _workspace(shares: [_theirs()], selected: 'p3'),
      );
      await tester.tap(find.text('Copy address'));
      await tester.pump();
      expect(copied, ['http://s-abc.localhost:3000/']);
      expect(find.text('Copied'), findsOneWidget);
      await tester.pump(const Duration(seconds: 3));
      await tester.tap(find.text('Hide'));
      await settle(tester);
      final hide = harness.last.hidePipeShare;
      expect((hide.shareId, hide.hidden), ('s1', true));
    });

    testWidgets('while the pipe is paused it waits, and nothing opens', (
      tester,
    ) async {
      final harness = await _show(
        tester,
        _workspace(
          shares: [_theirs(live: false)],
          selected: 'p3',
          state: PipeState.PIPE_STATE_PAUSED,
        ),
      );
      expect(find.text('Waits while the pipe is paused.'), findsOneWidget);
      await tester.tap(find.text('Open'));
      await settle(tester);
      expect(harness.backend.sent.any((m) => m.hasOpenPipeShare()), isFalse);
    });

    testWidgets("why this computer can't serve it", (tester) async {
      await _show(
        tester,
        _workspace(
          shares: [_theirs(problem: 'Port 3000 is in use on this Mac.')],
          selected: 'p3',
        ),
      );
      expect(find.text('Port 3000 is in use on this Mac.'), findsOneWidget);
    });

    testWidgets('a hidden one stays away; nor does this side see its own', (
      tester,
    ) async {
      await _show(
        tester,
        _workspace(shares: [_theirs(hidden: true), _mine()], selected: 'p3'),
      );
      expect(find.byType(ReceivedShare), findsNothing);
    });

    testWidgets('showing a Claktory elsewhere, it says where it opens', (
      tester,
    ) async {
      final harness = await _show(
        tester,
        _workspace(shares: [_theirs()], selected: 'p3'),
      );
      harness.backend.connectTo(
        parsePairingCode('factory://?t=${'c' * 64}&e=${'a' * 64}&name=mac'),
      );
      harness.backend.workspace.apply(
        _workspace(shares: [_theirs()], selected: 'p3'),
      );
      await settle(tester);
      expect(
        find.text('Open it on the computer your Claktory runs on.'),
        findsOneWidget,
      );
      expect(find.text('Open'), findsNothing);
      expect(find.text('Hide'), findsOneWidget);
    });

    testWidgets('on a phone it fits', (tester) async {
      await _show(
        tester,
        _workspace(shares: [_theirs()], selected: 'p3'),
        size: const Size(390, 844),
      );
      expect(find.text('Ana shares Dashboard'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });
  });

  group("a pipe's settings", () {
    Future<Harness> settings(
      WidgetTester tester,
      List<PipeShare> shares,
    ) async {
      final harness = await _show(
        tester,
        _workspace(shares: shares, selected: 'p3'),
        size: const Size(1200, 2000),
      );
      await tester.tap(find.text('Feedback · Ana').last);
      await settle(tester);
      expect(find.text('Pipe settings'), findsOneWidget);
      return harness;
    }

    testWidgets('list the shares on it: this side stops its own, after '
        'asking', (tester) async {
      final harness = await settings(tester, [_mine()]);
      expect(find.text('Metrics · port 8080'), findsOneWidget);
      expect(find.text('With the group “Friends”'), findsOneWidget);
      await tester.tap(find.text('Stop'));
      await settle(tester);
      expect(find.text('Stop sharing Metrics?'), findsOneWidget);
      await tester.tap(find.text('Stop sharing'));
      await settle(tester);
      expect(harness.last.stopPipeShare.shareId, 'm1');
    });

    testWidgets("and the other side's, shown again from here once hidden", (
      tester,
    ) async {
      final harness = await settings(tester, [_theirs(hidden: true)]);
      expect(find.text('Dashboard, from Ana'), findsOneWidget);
      expect(find.text('Hidden from the top of the pipe'), findsOneWidget);
      await tester.tap(find.text('Show'));
      await settle(tester);
      final show = harness.last.hidePipeShare;
      expect((show.shareId, show.hidden), ('s1', false));
    });

    testWidgets("Hide changes the open settings as soon as the Claktory "
        "says, without reopening them (Abbacchio's row 1111)", (tester) async {
      final harness = await settings(tester, [_theirs()]);
      expect(find.text('At the top of the pipe'), findsOneWidget);
      // The settings' own, over the strip's under the header.
      await tester.tap(find.text('Hide').last);
      await settle(tester);
      expect(harness.last.hidePipeShare.hidden, isTrue);
      await harness.apply(
        tester,
        _workspace(shares: [_theirs(hidden: true)], selected: 'p3'),
      );
      await settle(tester);
      expect(find.text('Hidden from the top of the pipe'), findsOneWidget);
      expect(find.text('Show'), findsOneWidget);
    });

    testWidgets('Share a website… asks the port, the name and the pipes, '
        'says what it lets them do, and shares', (tester) async {
      final harness = await settings(tester, []);
      await tester.tap(find.text('Share a website…'));
      await settle(tester);
      expect(find.text('Share a website'), findsOneWidget);
      expect(
        find.text(
          'People on these pipes can use everything this computer serves on '
          'port ….',
        ),
        findsOneWidget,
      );
      harness.backend.pipeSharePort = (port) =>
          PipeSharePort(port: port, listening: false);
      await tester.enterText(_field(0), '3000');
      await tester.pump(const Duration(milliseconds: 500));
      await settle(tester);
      expect(harness.last.checkPipeSharePort.port, 3000);
      expect(
        find.text(
          'People on these pipes can use everything this computer serves on '
          'port 3000.',
        ),
        findsOneWidget,
      );
      expect(
        find.textContaining('Nothing on this computer answers on port 3000'),
        findsOneWidget,
      );
      // Begun from this pipe: it's chosen; another is added.
      await tester.tap(
        find.descendant(
          of: find.byType(ShareWebsiteForm),
          matching: find.textContaining('Acme launch', findRichText: true),
        ),
      );
      await settle(tester);
      await tester.enterText(_field(1), ' Dashboard ');
      await tester.tap(find.text('Share'));
      await settle(tester);
      final share = _shared(harness);
      expect(share.port, 3000);
      expect(share.name, 'Dashboard');
      expect(share.pipeIds.toSet(), {'p3', 'p5'});
      expect(share.groupId, isEmpty);
      expect(find.text('Share a website'), findsNothing);
    });

    testWidgets("a port the Claktory won't share says why, and Share waits", (
      tester,
    ) async {
      final harness = await settings(tester, []);
      await tester.tap(find.text('Share a website…'));
      await settle(tester);
      harness.backend.pipeSharePort = (port) =>
          PipeSharePort(port: port, problem: "Port 7420 is Claktory's own.");
      await tester.enterText(_field(0), '7420');
      await tester.pump(const Duration(milliseconds: 500));
      await settle(tester);
      expect(find.text("Port 7420 is Claktory's own."), findsOneWidget);
      await tester.enterText(_field(1), 'Claktory');
      await tester.tap(find.text('Share'));
      await settle(tester);
      expect(harness.backend.sent.any((m) => m.hasSharePipeWebsite()), isFalse);
    });

    testWidgets('what the form said of the port goes as a port is typed', (
      tester,
    ) async {
      await settings(tester, []);
      await tester.tap(find.text('Share a website…'));
      await settle(tester);
      await tester.tap(find.text('Share'));
      await settle(tester);
      const said = 'Enter a port from 1 to 65535, such as 3000.';
      expect(find.text(said), findsOneWidget);
      await tester.enterText(_field(0), '3000');
      await tester.pump();
      expect(find.text(said), findsNothing);
      await tester.pump(const Duration(milliseconds: 500));
      await settle(tester);
    });

    testWidgets('a refusal says why and the form stays', (tester) async {
      final harness = await settings(tester, []);
      await tester.tap(find.text('Share a website…'));
      await settle(tester);
      await tester.enterText(_field(0), '3000');
      await tester.enterText(_field(1), 'Dashboard');
      harness.backend.refuse = 'That pipe closed.';
      await tester.tap(find.text('Share'));
      await settle(tester);
      expect(find.text('That pipe closed.'), findsOneWidget);
      expect(find.text('Share a website'), findsOneWidget);
    });
  });

  group('pipe groups', () {
    testWidgets("a group's menu shares a website with the group, and stops "
        'the ones that follow it', (tester) async {
      final harness = await _show(tester, _workspace(shares: [_mine()]));
      await tester.tap(_group('g1'), buttons: kSecondaryButton);
      await settle(tester);
      expect(find.text('Stop sharing Metrics…'), findsOneWidget);
      await tester.tap(find.text('Share a website…'));
      await settle(tester);
      expect(find.text('The group “Friends”'), findsOneWidget);
      expect(
        find.textContaining('Its pipes now: Feedback · Ana, Feedback · Ben.'),
        findsOneWidget,
      );
      await tester.enterText(_field(0), '3000');
      await tester.enterText(_field(1), 'Dashboard');
      await tester.tap(find.text('Share'));
      await settle(tester);
      final share = _shared(harness);
      expect(share.groupId, 'g1');
      expect(share.pipeIds, isEmpty);
    });

    testWidgets('moving a pipe into a group with shares says what it will '
        'also get, and asks', (tester) async {
      final harness = await _show(tester, _workspace(shares: [_mine()]));
      await tester.tap(_row('p5'), buttons: kSecondaryButton);
      await settle(tester);
      await tester.tap(find.text('Move to group…'));
      await settle(tester);
      await tester.tap(find.text('Friends').last);
      await settle(tester);
      expect(find.text('Move Acme launch to Friends?'), findsOneWidget);
      expect(find.text('This pipe will also get: Metrics.'), findsOneWidget);
      expect(harness.backend.sent.any((m) => m.hasSetPipeGroup()), isFalse);
      await tester.tap(find.text('Move'));
      await settle(tester);
      final moved = harness.last.setPipeGroup;
      expect((moved.pipeId, moved.groupId), ('p5', 'g1'));
    });

    testWidgets('moving one out says what it will stop getting; Cancel '
        'leaves it', (tester) async {
      final harness = await _show(tester, _workspace(shares: [_mine()]));
      await tester.tap(_row('p4'), buttons: kSecondaryButton);
      await settle(tester);
      await tester.tap(find.text('Remove from group'));
      await settle(tester);
      expect(
        find.text('This pipe will stop getting: Metrics.'),
        findsOneWidget,
      );
      await tester.tap(find.text('Cancel'));
      await settle(tester);
      expect(harness.backend.sent.any((m) => m.hasSetPipeGroup()), isFalse);
    });

    testWidgets('a move that changes no share goes at once', (tester) async {
      final harness = await _show(tester, _workspace(shares: [_mine()]));
      await tester.tap(_row('p5'), buttons: kSecondaryButton);
      await settle(tester);
      await tester.tap(find.text('Move to group…'));
      await settle(tester);
      await tester.tap(find.text('Launch').last);
      await settle(tester);
      expect(harness.last.setPipeGroup.groupId, 'g2');
    });

    testWidgets('deleting a group says its websites stay with its pipes', (
      tester,
    ) async {
      await _show(tester, _workspace(shares: [_mine()]));
      await tester.tap(_group('g1'), buttons: kSecondaryButton);
      await settle(tester);
      await tester.tap(find.text('Delete group…'));
      await settle(tester);
      expect(
        find.text(
          'Its pipes stay, just outside any group. Metrics stays shared with '
          'its pipes.',
        ),
        findsOneWidget,
      );
    });
  });
}
