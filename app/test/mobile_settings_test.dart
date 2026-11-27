// Settings on a phone: a page listing the sections, each
// a page of its own with Back, and what an agent opens (show_screen) lands
// on its section.
import 'package:factory_app/mobile/navigation.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_view.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'accounts_test.dart' show claude;
import 'harness.dart';
import 'phone_test.dart' show phone, workspace;

/// The avatar's colors as the backend sends them (Person.avatar_choices).
final avatarChoices = [
  for (final (from, to, name) in [
    ('#4F46E5', '#7C3AED', 'Indigo and violet'),
    ('#7C3AED', '#C026D3', 'Violet and fuchsia'),
    ('#BE185D', '#7C3AED', 'Pink and violet'),
    ('#E11D48', '#9F1239', 'Rose'),
    ('#C2410C', '#BE123C', 'Orange and rose'),
    ('#B45309', '#C2410C', 'Amber and orange'),
    ('#4D7C0F', '#047857', 'Lime and emerald'),
    ('#047857', '#0F766E', 'Emerald and teal'),
    ('#0F766E', '#0369A1', 'Teal and sky'),
    ('#0369A1', '#4338CA', 'Sky and indigo'),
    ('#1D4ED8', '#0E7490', 'Blue and cyan'),
    ('#475569', '#334155', 'Slate'),
  ])
    AvatarColors(from: from, to: to, name: name),
];

Future<Harness> _show(
  WidgetTester tester, {
  Size size = phone,
  double textScale = 1,
  bool rich = false,
}) async {
  if (textScale != 1) {
    tester.platformDispatcher.textScaleFactorTestValue = textScale;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
  }
  final harness = Harness(rich ? _rich() : _plain());
  await harness.show(tester, size: size);
  await pumpFrames(tester);
  return harness;
}

Workspace _plain() => workspace()
  ..settings = Settings(userName: 'Matt')
  ..person = Person(
    name: 'Matt',
    initials: 'M',
    avatarFrom: '#4F46E5',
    avatarTo: '#7C3AED',
    avatarChoices: avatarChoices,
  );

/// Everything Settings can show, with long names.
Workspace _rich() {
  final day = DateTime(2026, 11, 12).millisecondsSinceEpoch;
  return _plain()
    ..providers.clear()
    ..providers.add(claude())
    ..account = ClaktoryAccount(
      state: ClaktoryAccountState.CLAKTORY_ACCOUNT_STATE_SIGNED_IN,
      email: 'matthew.long-surname@example.com',
      service: 'https://account.claktory.com',
      name: 'Mac mini',
      lastContactMs: Int64(day),
    )
    ..speechModel = SpeechModel(
      state: SpeechModelState.SPEECH_MODEL_STATE_READY,
    )
    ..vault = VaultState(
      exists: true,
      unlocked: true,
      secrets: [
        SecretInfo(
          id: 's1',
          name: 'AWS production deploy account',
          description: "Matt's deploy account, eu-west-1",
          variables: [
            SecretVariable(name: 'AWS_SECRET_ACCESS_KEY', hint: '3f9a'),
          ],
        ),
      ],
    )
    ..remote = RemoteAccess(
      on: true,
      port: 7443,
      machine: 'mac-mini',
      devices: [
        PairedDevice(
          id: 'd1',
          name: "Matthew's iPhone 16 Pro Max",
          pairedMs: Int64(day),
          lastSeenMs: Int64(day),
        ),
        PairedDevice(
          id: 'd2',
          name: 'Studio display Mac',
          pairedMs: Int64(day),
          connected: true,
        ),
      ],
    );
}

Future<void> _openSettings(WidgetTester tester, Harness harness) async {
  harness.page.value = AppPage.settings;
  await pumpFrames(tester);
}

/// The section rows' titles, top to bottom.
List<String> _rows(WidgetTester tester) {
  final titles = [
    'Profile',
    'General',
    'Account',
    'Providers',
    'Defaults',
    'Voice',
    'Secrets',
    'Apps',
    'Devices',
    'Resources',
    'Display',
  ];
  final shown = [
    for (final title in titles)
      if (find.text(title).evaluate().isNotEmpty) title,
  ];
  shown.sort(
    (a, b) => tester
        .getTopLeft(find.text(a).first)
        .dy
        .compareTo(tester.getTopLeft(find.text(b).first).dy),
  );
  return shown;
}

PhoneRoutes _routes(WidgetTester tester) =>
    PhoneRoutes.of(tester.element(find.byType(SettingsView)))!;

void main() {
  testWidgets('Settings lists its sections; each opens as a page with '
      'Back, and Back from the list closes Settings', (tester) async {
    final harness = await _show(tester);
    await _openSettings(tester, harness);

    expect(find.byType(SettingsView), findsOneWidget);
    expect(_rows(tester), [
      'Profile',
      'General',
      'Providers',
      'Defaults',
      'Secrets',
      'Devices',
      'Resources',
      'Display',
    ]);
    // What a section is set to, at the end of its row.
    expect(find.text('Matt'), findsOneWidget);
    // Every row a finger's size.
    for (final title in _rows(tester)) {
      expect(
        tester.getSize(find.bySemanticsLabel(RegExp('^$title'))).height,
        greaterThanOrEqualTo(52),
      );
    }

    await tester.tap(find.text('Devices'));
    await pumpFrames(tester);
    // Its page, with its title in the header and the list out of sight.
    expect(find.text('Devices'), findsOneWidget);
    expect(find.text('Profile'), findsNothing);
    expect(_routes(tester).canPop, isTrue);

    await tester.tap(find.bySemanticsLabel('Back'));
    await pumpFrames(tester);
    expect(find.text('Profile'), findsOneWidget);
    expect(harness.page.value, AppPage.settings);

    await tester.tap(find.bySemanticsLabel('Back'));
    await pumpFrames(tester);
    expect(find.byType(SettingsView), findsNothing);
    expect(harness.page.value, AppPage.agent);
  });

  testWidgets('a section opened from the list keeps its controls: Profile '
      'has the name to change', (tester) async {
    final harness = await _show(tester);
    await _openSettings(tester, harness);
    await tester.tap(find.text('Profile'));
    await pumpFrames(tester);
    expect(find.text('Your name'), findsOneWidget);
    expect(find.text('Shuffle colors'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets("Profile's avatar colors are a finger's size, say which they "
      'are, and pick one', (tester) async {
    final harness = await _show(tester);
    await _openSettings(tester, harness);
    await tester.tap(find.text('Profile'));
    await pumpFrames(tester);
    final rose = find.byKey(const ValueKey('avatar-color-3'));
    await tester.ensureVisible(rose);
    await pumpFrames(tester);
    expect(tester.getSize(rose), const Size(44, 44));
    expect(
      tester.getSemantics(rose),
      matchesSemantics(
        label: 'Rose',
        isButton: true,
        hasSelectedState: true,
        hasTapAction: true,
      ),
    );
    expect(
      tester.getSemantics(find.byKey(const ValueKey('avatar-color-0'))),
      matchesSemantics(
        label: 'Indigo and violet',
        isButton: true,
        hasSelectedState: true,
        isSelected: true,
      ),
    );
    await tester.tap(rose);
    await pumpFrames(tester);
    final sent = harness.backend.sent.lastWhere((m) => m.hasShuffleAvatar());
    expect(sent.shuffleAvatar.choice, 3);
  });

  testWidgets("General's name field is the screen's width, and its colors "
      "a finger's size", (tester) async {
    final harness = await _show(tester);
    await _openSettings(tester, harness);
    await tester.tap(find.text('General'));
    await pumpFrames(tester);
    final field = find.descendant(
      of: find.byType(SettingsView),
      matching: find.byType(EditableText),
    );
    expect(tester.getSize(field.first).width, greaterThan(phone.width - 80));
    final swatch = find.byKey(const ValueKey('color-blue'));
    expect(tester.getSize(swatch).width, greaterThanOrEqualTo(44));
    expect(tester.getSize(swatch).height, greaterThanOrEqualTo(44));
  });

  testWidgets('a section an agent opens lands on its page, once; another '
      'takes its place, so Back still leads to the list', (tester) async {
    final harness = await _show(tester);
    harness.backend.screenRequested(
      OpenScreen(
        id: 'r1',
        target: HighlightTarget(kind: 'settings', id: 'devices'),
        fromAgentId: 'rivet',
      ),
    );
    await pumpFrames(tester);
    expect(harness.page.value, AppPage.settings);
    expect(find.text('Devices'), findsOneWidget);
    expect(find.text('Profile'), findsNothing);
    expect(harness.backend.settingsSection.value, isNull);

    harness.backend.settingsSection.value = 'defaults';
    await pumpFrames(tester);
    expect(find.text('Defaults'), findsOneWidget);
    expect(find.text('Summarize conversations at'), findsOneWidget);

    await tester.tap(find.bySemanticsLabel('Back'));
    await pumpFrames(tester);
    // The list, not Devices under it.
    expect(find.text('Profile'), findsOneWidget);
  });

  testWidgets('a section that isn\'t there now opens the list', (tester) async {
    final harness = await _show(tester);
    await _openSettings(tester, harness);
    harness.backend.settingsSection.value = 'account';
    await pumpFrames(tester);
    expect(find.text('Profile'), findsOneWidget);
    expect(harness.backend.settingsSection.value, isNull);
  });

  testWidgets('another page closes Settings and the section open in it', (
    tester,
  ) async {
    final harness = await _show(tester);
    await _openSettings(tester, harness);
    await tester.tap(find.text('Display'));
    await pumpFrames(tester);
    harness.page.value = AppPage.usage;
    await pumpFrames(tester);
    harness.page.value = AppPage.agent;
    await pumpFrames(tester);
    expect(find.byType(SettingsView), findsNothing);
  });

  for (final width in [320.0, 440.0]) {
    testWidgets('at the largest text the list and every section fit '
        '$width points', (tester) async {
      final harness = await _show(
        tester,
        size: Size(width, 700),
        textScale: 3.1,
        rich: true,
      );
      // Where each overflow is, to say so.
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
      await _openSettings(tester, harness);
      tester.takeException();
      expect(errors, isEmpty);
      for (final id in [
        'profile',
        'general',
        'account',
        'providers',
        'voice',
        'defaults',
        'secrets',
        'devices',
        'resources',
        'display',
      ]) {
        harness.backend.settingsSection.value = id;
        await pumpFrames(tester);
        final list = find
            .descendant(
              of: find.byType(SettingsView),
              matching: find.byType(Scrollable),
            )
            .first;
        for (var i = 0; i < 8; i++) {
          await tester.drag(list, const Offset(0, -400), warnIfMissed: false);
          await pumpFrames(tester);
        }
        tester.takeException();
        expect(errors, isEmpty, reason: id);
      }
    });
  }
}
