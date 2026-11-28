// A place macOS refused Claktory Agents is a Needs you item in Claktory's
// words, with Open System Settings on the Mac that runs the Claktory and OK
// everywhere; Settings → This Mac's Permissions has a row for each place
// checked ("Documents · On"); and back at the front, the refused places
// are read again.
import 'package:factory_app/controls.dart' show Button, hasLucideIcon;
import 'package:factory_app/inbox.dart' show needsYouCount;
import 'package:factory_app/protected_places.dart' show openSystemSettings;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'harness.dart';

const _documents = '/Users/ana/Documents';
const _title = "Your agents can't work in Documents";
const _text =
    "macOS hasn't let Claktory Agents into Documents, so commands in "
    'nixos-site fail. Turn it on for Claktory Agents in System Settings.';

ProtectedPlace _place({
  ProtectedPlaceState state = ProtectedPlaceState.PROTECTED_PLACE_STATE_REFUSED,
  bool needsYou = true,
}) => ProtectedPlace(
  root: _documents,
  name: 'Documents',
  state: state,
  checkedMs: Int64(1000),
  usedBy: ['nixos-site'],
  needsYou: needsYou,
  title: needsYou ? _title : '',
  text: needsYou ? _text : '',
);

Workspace _workspace(List<ProtectedPlace> places) => Workspace(
  settings: Settings(name: 'Studio'),
  projects: [
    Project(id: 'p1', name: 'nixos-site', path: '$_documents/nixos-site'),
  ],
  protectedPlaces: places,
);

Finder _button(String label) => find.widgetWithText(Button, label);

void _onMac() {
  debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
  addTearDown(() => debugDefaultTargetPlatformOverride = null);
}

Future<Harness> _needsYou(WidgetTester tester, Workspace snapshot) async {
  final harness = Harness(snapshot);
  await harness.show(tester, size: const Size(1200, 900));
  await settle(tester);
  harness.page.value = AppPage.inbox;
  await settle(tester);
  await settle(tester);
  return harness;
}

void main() {
  testWidgets('on the Mac that runs it: the item in Claktory\'s words, Open '
      'System Settings at Files and Folders, and OK sets it aside', (
    tester,
  ) async {
    _onMac();
    final opened = <Uri>[];
    openSystemSettings = (url) async {
      opened.add(url);
      return true;
    };
    addTearDown(() => openSystemSettings = (_) async => false);
    final harness = await _needsYou(tester, _workspace([_place()]));
    expect(find.text(_title), findsWidgets);
    expect(find.text(_text), findsWidgets);
    await tester.tap(find.text(_title).first);
    await settle(tester);
    await tester.pump(const Duration(seconds: 1));
    expect(_button('Open System Settings'), findsOneWidget);
    expect(
      find.textContaining('On Studio, open System Settings'),
      findsNothing,
    );
    // OK quiet first, Open System Settings filled at the end.
    expect(
      tester.getTopLeft(_button('OK')).dx,
      lessThan(tester.getTopLeft(_button('Open System Settings')).dx),
    );
    await tester.tap(_button('Open System Settings'));
    await settle(tester);
    expect(opened.single.toString(), contains('Privacy_FilesAndFolders'));
    // Opening System Settings answers nothing.
    expect(
      harness.backend.sent.where((m) => m.hasDismissProtectedPlace()),
      isEmpty,
    );

    await tester.tap(_button('OK'));
    await settle(tester);
    expect(harness.last.dismissProtectedPlace.root, _documents);
    // The backend set it aside: it stays here saying it's still off.
    await harness.apply(tester, _workspace([_place(needsYou: false)]));
    await settle(tester);
    expect(
      find.text(
        'Still off. To turn it on: System Settings → Privacy & Security → '
        'Files and Folders → Claktory Agents.',
      ),
      findsWidgets,
    );
    // Nothing left to answer on it.
    expect(_button('OK'), findsNothing);
    expect(_button('Open System Settings'), findsNothing);
  });

  testWidgets('on another device: whose System Settings it is, and only OK', (
    tester,
  ) async {
    final harness = await _needsYou(tester, _workspace([_place()]));
    await tester.tap(find.text(_title).first);
    await settle(tester);
    await tester.pump(const Duration(seconds: 1));
    expect(_button('Open System Settings'), findsNothing);
    expect(
      find.text(
        'On Studio, open System Settings → Privacy & Security → Files and '
        'Folders and turn on Claktory Agents.',
      ),
      findsOneWidget,
    );
    harness.backend.refuse = 'The place moved.';
    await tester.tap(_button('OK'));
    await settle(tester);
    expect(harness.last.hasDismissProtectedPlace(), isTrue);
    expect(find.textContaining('The place moved.'), findsOneWidget);
  });

  testWidgets('let in from System Settings: the item says so', (tester) async {
    _onMac();
    final harness = await _needsYou(tester, _workspace([_place()]));
    await tester.tap(find.text(_title).first);
    await settle(tester);
    await harness.apply(
      tester,
      _workspace([
        _place(
          state: ProtectedPlaceState.PROTECTED_PLACE_STATE_ALLOWED,
          needsYou: false,
        ),
      ]),
    );
    await settle(tester);
    expect(find.text('Claktory Agents can use Documents now.'), findsWidgets);
  });

  testWidgets('on an iPhone: its row in the list, and its card fits', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(_workspace([_place()]));
    await harness.show(tester, size: const Size(375, 667));
    harness.page.value = AppPage.inbox;
    await pumpFrames(tester);
    await tester.pump();
    await pumpFrames(tester);
    expect(find.text(_title), findsWidgets);
    await tester.tap(find.text(_title).first);
    await pumpFrames(tester, const Duration(seconds: 1));
    expect(tester.takeException(), isNull);
    final ok = _button('OK');
    await tester.ensureVisible(ok);
    await pumpFrames(tester);
    expect(tester.getRect(ok).right, lessThanOrEqualTo(375));
    await tester.tap(ok);
    await pumpFrames(tester);
    expect(harness.last.dismissProtectedPlace.root, _documents);
  });

  test('its glyph is drawn, never a "?" (folder-lock)', () {
    expect(hasLucideIcon('folder-lock'), isTrue);
  });

  test('only a place waiting on the person is an item', () {
    expect(needsYouCount(_workspace([_place()])), 1);
    expect(needsYouCount(_workspace([_place(needsYou: false)])), 0);
  });

  group('Permissions', () {
    Workspace places() => _workspace([
      _place(
        state: ProtectedPlaceState.PROTECTED_PLACE_STATE_ALLOWED,
        needsYou: false,
      ),
      ProtectedPlace(
        root: '/Users/ana/Desktop',
        name: 'Desktop',
        state: ProtectedPlaceState.PROTECTED_PLACE_STATE_REFUSED,
        usedBy: ['blog'],
      ),
      // Never checked: no row.
      ProtectedPlace(root: '/Volumes/Backup', name: 'Backup'),
    ]);

    Future<Harness> open(WidgetTester tester, Workspace snapshot) async {
      final harness = Harness(snapshot);
      await harness.show(tester, size: const Size(1200, 1600));
      await settle(tester);
      harness.page.value = AppPage.settings;
      await settle(tester);
      harness.backend.settingsSection.value = 'device#permissions';
      await pumpFrames(tester, const Duration(seconds: 1));
      return harness;
    }

    testWidgets('on the Mac that runs it, a row for each place checked', (
      tester,
    ) async {
      _onMac();
      final opened = <Uri>[];
      openSystemSettings = (url) async {
        opened.add(url);
        return true;
      };
      addTearDown(() => openSystemSettings = (_) async => false);
      await open(tester, places());
      final documents = find.byKey(const ValueKey('permission:Documents'));
      final desktop = find.byKey(const ValueKey('permission:Desktop'));
      expect(documents, findsOneWidget);
      expect(
        find.descendant(of: documents, matching: find.text('On')),
        findsOneWidget,
      );
      expect(
        find.descendant(of: desktop, matching: find.text('Off')),
        findsOneWidget,
      );
      expect(
        find.descendant(
          of: desktop,
          matching: find.text(
            "Your agents can't work in blog. To turn it on: System Settings "
            '→ Privacy & Security → Files and Folders → Claktory Agents.',
          ),
        ),
        findsOneWidget,
      );
      expect(find.byKey(const ValueKey('permission:Backup')), findsNothing);
      await tester.tap(
        find.descendant(of: desktop, matching: _button('Open System Settings')),
      );
      await settle(tester);
      expect(opened.single.toString(), contains('Privacy_FilesAndFolders'));
    });

    testWidgets('a row follows the backend\'s next check', (tester) async {
      _onMac();
      final harness = await open(tester, places());
      final next = places();
      next.protectedPlaces[1].state =
          ProtectedPlaceState.PROTECTED_PLACE_STATE_ALLOWED;
      await harness.apply(tester, next);
      await pumpFrames(tester);
      expect(
        find.descendant(
          of: find.byKey(const ValueKey('permission:Desktop')),
          matching: find.text('On'),
        ),
        findsOneWidget,
      );
    });

    testWidgets('anywhere but that Mac, no rows for its places', (
      tester,
    ) async {
      await open(tester, places());
      expect(find.byKey(const ValueKey('permission:Documents')), findsNothing);
      expect(find.byKey(const ValueKey('permission:Desktop')), findsNothing);
    });
  });

  group('back at the front', () {
    Future<Harness> show(WidgetTester tester, Workspace snapshot) async {
      final harness = Harness(snapshot);
      await harness.show(tester, size: const Size(1200, 900));
      await pumpFrames(tester, const Duration(seconds: 1));
      return harness;
    }

    Future<void> resume(WidgetTester tester) async {
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await pumpFrames(tester);
    }

    testWidgets('on the Mac that runs it, the refused places are read again', (
      tester,
    ) async {
      _onMac();
      final harness = await show(
        tester,
        _workspace([
          _place(needsYou: false),
          ProtectedPlace(
            root: '/Users/ana/Desktop',
            name: 'Desktop',
            state: ProtectedPlaceState.PROTECTED_PLACE_STATE_ALLOWED,
          ),
        ]),
      );
      expect(
        harness.backend.sent.where((m) => m.hasCheckProtectedPlaces()),
        isEmpty,
      );
      await resume(tester);
      final checks = [
        for (final m in harness.backend.sent)
          if (m.hasCheckProtectedPlaces()) m.checkProtectedPlaces.roots,
      ];
      expect(checks, [
        [_documents],
      ]);
    });

    testWidgets('none refused: nothing is sent', (tester) async {
      _onMac();
      final harness = await show(
        tester,
        _workspace([
          _place(
            state: ProtectedPlaceState.PROTECTED_PLACE_STATE_ALLOWED,
            needsYou: false,
          ),
        ]),
      );
      await resume(tester);
      expect(
        harness.backend.sent.where((m) => m.hasCheckProtectedPlaces()),
        isEmpty,
      );
    });

    testWidgets('anywhere but that Mac, nothing is sent', (tester) async {
      final harness = await show(tester, _workspace([_place()]));
      await resume(tester);
      expect(
        harness.backend.sent.where((m) => m.hasCheckProtectedPlaces()),
        isEmpty,
      );
    });
  });
}
