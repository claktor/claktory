// Keeping a laptop's agents working with its lid closed (#371): Settings →
// General → Sleep's second switch, only on a laptop that can and, on the
// Mac itself, only when the app carries the helper; its own question
// before it's turned on; the helper's real state under it; turning it off;
// and the Mac app keeping the backend told what macOS says of the helper.

import 'package:factory_app/controls.dart' show Toggle;
import 'package:factory_app/lid_closed.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/scope.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'app_test.dart' show workspace;
import 'harness.dart';
import 'idle_compaction_test.dart' show reveal;

final _mac = TargetPlatformVariant.only(TargetPlatform.macOS);

/// The Mac app's helper as macOS has it: what status answers, what each
/// call did, and register's refusal when set.
class _Helper {
  _Helper(WidgetTester tester, {this.status = 'notRegistered'}) {
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      const MethodChannel('factory/lid_helper'),
      (call) async {
        calls.add(call.method);
        switch (call.method) {
          case 'register':
            if (refuse != null) {
              throw PlatformException(code: 'failed', message: refuse);
            }
            status = 'requiresApproval';
          case 'unregister':
            status = 'notRegistered';
          case 'openLoginItems':
            return null;
        }
        return status;
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        const MethodChannel('factory/lid_helper'),
        null,
      ),
    );
  }

  String status;
  String? refuse;
  final calls = <String>[];

  /// What changed it, leaving out status reads.
  List<String> get changes => [
    for (final call in calls)
      if (call != 'status') call,
  ];
}

KeepAwake _laptop({
  LidClosedState state = LidClosedState.LID_CLOSED_OFF,
  bool available = true,
  bool laptop = true,
  bool holding = false,
  String error = '',
}) => KeepAwake(
  available: true,
  laptop: laptop,
  answered: true,
  lidClosedAvailable: available,
  lidClosed: state,
  lidClosedHolding: holding,
  lidClosedError: error,
);

Workspace _workspace(KeepAwake keep) => workspace()..keepAwake = keep;

Future<Harness> _settings(
  WidgetTester tester,
  KeepAwake keep, {
  bool localBackend = true,
}) async {
  final harness = Harness(_workspace(keep), localBackend: localBackend);
  await harness.show(tester, size: const Size(1200, 1600));
  harness.page.value = AppPage.settings;
  await settle(tester);
  await reveal(tester, find.text('Sleep'));
  await settle(tester);
  return harness;
}

Toggle _toggle(WidgetTester tester) => tester.widget<Toggle>(
  find.byWidgetPredicate((w) => w is Toggle && w.label == lidClosedLabel),
);

List<SetLidClosed> _told(Harness harness) => [
  for (final message in harness.backend.sent)
    if (message.hasSetLidClosed()) message.setLidClosed,
];

void main() {
  testWidgets("on the Mac: off, its note; turning it on asks first, then "
      'installs the helper and opens Login Items', (tester) async {
    final helper = _Helper(tester);
    final harness = await _settings(tester, _laptop());
    expect(find.text(lidClosedLabel), findsOneWidget);
    expect(find.text(lidClosedNote), findsOneWidget);
    expect(_toggle(tester).value, isFalse);

    await tester.tap(find.text(lidClosedLabel));
    await settle(tester);
    // Its own question: what it does, the bag and the limits, the password.
    expect(find.text(lidClosedQuestionTitle), findsOneWidget);
    expect(
      find.text(
        "This Mac stays awake while agents work and it's plugged in, even "
        'with its lid closed.',
      ),
      findsOneWidget,
    );
    expect(find.text(lidClosedQuestionBag), findsOneWidget);
    expect(find.text(lidClosedQuestionPassword), findsOneWidget);
    expect(_told(harness), isEmpty);

    // Not now: nothing.
    await tester.tap(find.text('Not now'));
    await settle(tester);
    expect(find.text(lidClosedQuestionTitle), findsNothing);
    expect(helper.changes, isEmpty);
    expect(_told(harness), isEmpty);

    await tester.tap(find.text(lidClosedLabel));
    await settle(tester);
    await tester.tap(find.text('Turn on'));
    await settle(tester);
    expect(find.text(lidClosedQuestionTitle), findsNothing);
    expect(helper.changes, ['register', 'openLoginItems']);
    final told = _told(harness).single;
    expect(told.on, isTrue);
    expect(
      told.registration,
      LidClosedRegistration.LID_CLOSED_REGISTRATION_REQUIRES_APPROVAL,
    );

    // Waiting for the person's approval: what to do, and where.
    await harness.apply(
      tester,
      _workspace(_laptop(state: LidClosedState.LID_CLOSED_NEEDS_APPROVAL)),
    );
    await settle(tester);
    expect(_toggle(tester).value, isTrue);
    expect(
      find.text(
        'Waiting for you to allow Claktory in System Settings → Login Items.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Open Login Items'));
    await settle(tester);
    expect(helper.calls.last, 'openLoginItems');
  }, variant: _mac);

  testWidgets('a fresh install: the app carries the helper, never registered '
      '(notRegistered, as the channel reads the bundle): the switch shows, '
      'off', (tester) async {
    _Helper(tester, status: 'notRegistered');
    await _settings(tester, _laptop());
    expect(find.text(lidClosedLabel), findsOneWidget);
    expect(_toggle(tester).value, isFalse);
    expect(_toggle(tester).onChanged, isNotNull);
  }, variant: _mac);

  testWidgets("macOS refusing to install it says why in the question, and "
      'leaves it off', (tester) async {
    final helper = _Helper(tester)..refuse = 'Operation not permitted';
    final harness = await _settings(tester, _laptop());
    await tester.tap(find.text(lidClosedLabel));
    await settle(tester);
    await tester.tap(find.text('Turn on'));
    await settle(tester);
    expect(
      find.text("Couldn't install Claktory's helper: Operation not permitted"),
      findsOneWidget,
    );
    expect(find.text(lidClosedQuestionTitle), findsOneWidget);
    expect(helper.changes, ['register']);
    expect(_told(harness), isEmpty);
  }, variant: _mac);

  testWidgets('no switch on a desktop, on a Mac that can\'t, or in an app '
      'with no helper (Claktory Dev)', (tester) async {
    _Helper(tester);
    await _settings(tester, _laptop(laptop: false));
    expect(find.text(lidClosedLabel), findsNothing);

    await _settings(tester, _laptop(available: false));
    expect(find.text(lidClosedLabel), findsNothing);

    _Helper(tester, status: 'notFound');
    await _settings(tester, _laptop());
    expect(find.text('Sleep'), findsOneWidget);
    expect(find.text(lidClosedLabel), findsNothing);
  }, variant: _mac);

  testWidgets('where it stands, in words, with what to do about it', (
    tester,
  ) async {
    final helper = _Helper(tester, status: 'enabled');
    final harness = await _settings(
      tester,
      _laptop(state: LidClosedState.LID_CLOSED_ON),
    );
    expect(
      find.text(
        'On: agents keep working while this Mac is plugged in, even with its '
        'lid closed.',
      ),
      findsOneWidget,
    );

    Future<void> show(KeepAwake keep) async {
      await harness.apply(tester, _workspace(keep));
      await settle(tester);
    }

    await show(_laptop(state: LidClosedState.LID_CLOSED_ON, holding: true));
    expect(
      find.text('Keeping this Mac awake now, even with its lid closed'),
      findsOneWidget,
    );
    await show(_laptop(state: LidClosedState.LID_CLOSED_PAUSED_BATTERY));
    expect(
      find.text(
        'Paused: the battery is low. It starts again at 30% or when you plug '
        'this Mac in.',
      ),
      findsOneWidget,
    );
    await show(_laptop(state: LidClosedState.LID_CLOSED_PAUSED_HEAT));
    expect(
      find.text(
        'Paused: this Mac is too hot. It starts again 10 minutes after it '
        'cools down.',
      ),
      findsOneWidget,
    );
    await show(_laptop(state: LidClosedState.LID_CLOSED_PAUSED_LID_ON_BATTERY));
    expect(
      find.text(
        'Paused: the lid is closed on battery. It starts again when you plug '
        'this Mac in.',
      ),
      findsOneWidget,
    );
    await show(_laptop(state: LidClosedState.LID_CLOSED_MISSING));
    expect(
      find.text(
        "Claktory's helper isn't running: it may have been turned off in "
        'System Settings → Login Items.',
      ),
      findsOneWidget,
    );
    // Turned off in Login Items after a refused launch: turned on there.
    helper.calls.clear();
    await tester.tap(find.text('Open Login Items'));
    await settle(tester);
    expect(helper.changes, ['openLoginItems']);
    // Install again: removed first, since macOS has it, then installed.
    helper.calls.clear();
    await tester.tap(find.text('Install again'));
    await settle(tester);
    expect(helper.changes, ['unregister', 'register', 'openLoginItems']);
    expect(
      _told(harness).last.registration,
      LidClosedRegistration.LID_CLOSED_REGISTRATION_REQUIRES_APPROVAL,
    );

    await show(
      _laptop(
        state: LidClosedState.LID_CLOSED_FAILED,
        error: 'The operation couldn’t be completed.',
      ),
    );
    expect(
      find.textContaining(
        "Couldn't install Claktory's helper: The operation couldn’t be "
        'completed.',
      ),
      findsOneWidget,
    );
    expect(find.text('Try again'), findsOneWidget);
  }, variant: _mac);

  testWidgets('turning it off: the backend lets go, then the helper is '
      'removed and the backend told', (tester) async {
    final helper = _Helper(tester, status: 'enabled');
    final harness = await _settings(
      tester,
      _laptop(state: LidClosedState.LID_CLOSED_ON),
    );
    await tester.tap(find.text(lidClosedLabel));
    await settle(tester);
    // No question to turn it off.
    expect(find.text(lidClosedQuestionTitle), findsNothing);
    expect(helper.changes, ['unregister']);
    final told = _told(harness);
    expect(
      [for (final each in told) (each.on, each.registration)],
      [
        (false, LidClosedRegistration.LID_CLOSED_REGISTRATION_UNKNOWN),
        (false, LidClosedRegistration.LID_CLOSED_REGISTRATION_NOT_REGISTERED),
      ],
    );
  }, variant: _mac);

  testWidgets('from another device it shows where it stands; it turns on '
      'only from the Mac, and off from anywhere', (tester) async {
    final helper = _Helper(tester);
    final harness = await _settings(tester, _laptop(), localBackend: false);
    expect(find.text(lidClosedLabel), findsOneWidget);
    expect(find.text(lidClosedFromMac), findsOneWidget);
    expect(_toggle(tester).onChanged, isNull);
    await tester.tap(find.text(lidClosedLabel));
    await settle(tester);
    expect(find.text(lidClosedQuestionTitle), findsNothing);
    expect(_told(harness), isEmpty);

    await harness.apply(
      tester,
      _workspace(_laptop(state: LidClosedState.LID_CLOSED_MISSING)),
    );
    await settle(tester);
    expect(find.textContaining('to install it again.'), findsOneWidget);
    // Nothing to install from here.
    expect(find.text('Install again'), findsNothing);
    await tester.tap(find.text(lidClosedLabel));
    await settle(tester);
    // Off: the backend lets go; the Mac removes the helper.
    expect([for (final each in _told(harness)) each.on], [false]);
    expect(helper.calls, isEmpty);
  }, variant: _mac);

  group('the Mac app keeps the backend told', () {
    LidClosedWatcher watch(Harness harness) {
      final watcher = LidClosedWatcher(
        changes: ChangeNotifier(),
        local: () => harness.backend,
      );
      addTearDown(watcher.dispose);
      return watcher;
    }

    testWidgets('what macOS says once it changes: approved in System '
        'Settings, it says so', (tester) async {
      final helper = _Helper(tester, status: 'requiresApproval');
      final harness = Harness(
        _workspace(_laptop(state: LidClosedState.LID_CLOSED_NEEDS_APPROVAL)),
      );
      watch(harness);
      await tester.pump();
      expect(
        _told(harness).single.registration,
        LidClosedRegistration.LID_CLOSED_REGISTRATION_REQUIRES_APPROVAL,
      );
      // Nothing new: nothing sent.
      await tester.pump(LidClosedWatcher.pollEvery);
      expect(_told(harness), hasLength(1));

      helper.status = 'enabled';
      await tester.pump(LidClosedWatcher.pollEvery);
      expect(
        _told(harness).last.registration,
        LidClosedRegistration.LID_CLOSED_REGISTRATION_ENABLED,
      );
      expect(_told(harness).last.on, isTrue);
      expect(helper.changes, isEmpty);

      // On: no more asking.
      harness.backend.workspace.apply(
        _workspace(_laptop(state: LidClosedState.LID_CLOSED_ON)),
      );
      await tester.pump(LidClosedWatcher.pollEvery * 3);
      expect(_told(harness), hasLength(2));
    }, variant: _mac);

    testWidgets('turned off from a phone, the helper is removed here', (
      tester,
    ) async {
      final helper = _Helper(tester, status: 'enabled');
      final harness = Harness(_workspace(_laptop()));
      watch(harness);
      await tester.pump();
      expect(helper.changes, ['unregister']);
      final told = _told(harness).single;
      expect(
        (told.on, told.registration),
        (false, LidClosedRegistration.LID_CLOSED_REGISTRATION_NOT_REGISTERED),
      );
    }, variant: _mac);

    testWidgets('from another version, installed again, once', (tester) async {
      final helper = _Helper(tester, status: 'enabled');
      final harness = Harness(
        _workspace(_laptop(state: LidClosedState.LID_CLOSED_OUTDATED)),
      );
      watch(harness);
      await tester.pump();
      // Not System Settings unasked.
      expect(helper.changes, ['unregister', 'register']);
      harness.backend.workspace.apply(
        _workspace(_laptop(state: LidClosedState.LID_CLOSED_ON)),
      );
      await tester.pump();
      harness.backend.workspace.apply(
        _workspace(_laptop(state: LidClosedState.LID_CLOSED_OUTDATED)),
      );
      await tester.pump();
      expect(helper.changes, ['unregister', 'register']);
    }, variant: _mac);

    testWidgets('a desktop, or a backend that can\'t: nothing', (tester) async {
      final helper = _Helper(tester, status: 'enabled');
      final harness = Harness(_workspace(_laptop(available: false)));
      watch(harness);
      await tester.pump(LidClosedWatcher.pollEvery * 2);
      expect(helper.calls, isEmpty);
      expect(_told(harness), isEmpty);
    }, variant: _mac);
  });
}
