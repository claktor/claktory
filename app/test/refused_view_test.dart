import 'package:factory_app/agent_view.dart' show AgentView;
import 'package:factory_app/connections.dart' show parsePairingCode;
import 'package:factory_app/identity.dart' show Palette;
import 'package:factory_app/message_box.dart' show MessageBox;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'account_test.dart' show signedOut, withAccount;
import 'harness.dart';

// Zucchero's row 220: a phone its Claktory turned away (a pairing code
// that expired) said "Connecting…" forever in the main view, while the
// drawer said Not allowed.

void main() {
  testWidgets('turned away, the main view says so, with Use a new code, '
      'never Connecting…', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace(), localBackend: false);
    harness.connections.use(
      parsePairingCode('factory://?t=${'b' * 64}&name=mac-mini&e=${'d' * 64}'),
    );
    await harness.show(tester, size: const Size(375, 667));
    await pumpFrames(tester);
    harness.backend.connected.value = false;
    harness.backend.refused.value =
        "This device isn't allowed on this Claktory. Use a new code.";
    await pumpFrames(tester);
    expect(find.text('Connecting…'), findsNothing);
    // The app's own words, not the reason the tunnel gave (Trish).
    expect(
      find.text("This device isn't allowed on this Claktory. Use a new code."),
      findsNothing,
    );
    // Never seen let in since the app opened: it isn't said to have been
    // removed (devices-signin.md §4); not knowing whether another device
    // is there, it gives Settings → Devices only, no command that may not
    // be needed (Lucy 49057).
    expect(find.text("mac-mini doesn't allow this iPhone"), findsOneWidget);
    expect(
      find.text(
        'To use mac-mini on this iPhone, connect with a new code from its '
        'Settings → Devices.',
      ),
      findsOneWidget,
    );
    expect(find.text('factoryd pair'), findsNothing);
    expect(find.text('Remove…'), findsOneWidget);
    await tester.tap(find.text('Use a new code'));
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    expect(find.text('Add a Claktory with a code'), findsOneWidget);
    await tester.tap(find.text('Cancel').last);
    await pumpFrames(tester);

    // Let in again: the usual view.
    harness.backend.refused.value = null;
    harness.backend.connected.value = true;
    await pumpFrames(tester);
    expect(find.textContaining('To use mac-mini'), findsNothing);
  });

  // Zucchero's row 232: with an agent open, the conversation stayed, with
  // a message box that couldn't send.
  testWidgets('turned away with an agent open, the main view says so '
      'instead of the conversation', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace(), localBackend: false);
    harness.connections.use(
      parsePairingCode('factory://?t=${'b' * 64}&name=studio&e=${'d' * 64}'),
    );
    await harness.show(tester, size: const Size(1200, 800));
    // Connected, with Eve open.
    harness.backend.connected.value = true;
    harness.backend.workspace.apply(
      Workspace(
        agents: [Agent(id: 'eve', name: 'Eve', color: 'green')],
        selectedAgentId: 'eve',
        models: [Model(id: 'm', name: 'M')],
      ),
    );
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsOneWidget);
    harness.backend.connected.value = false;
    harness.backend.refused.value = 'The machine turned this device away';
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsNothing);
    // Seen let in, and no other device there: it was removed, and only
    // factoryd pair brings it back (Squalo 41528).
    expect(find.text('studio no longer allows this Mac'), findsOneWidget);
    expect(
      find.text(
        'This Mac was removed on studio. To use studio here again, run '
        'factoryd pair on studio and connect with the new code.',
      ),
      findsOneWidget,
    );
    expect(find.text('factoryd pair'), findsOneWidget);
    expect(find.text('Use a new code'), findsOneWidget);

    harness.backend.refused.value = null;
    harness.backend.connected.value = true;
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsOneWidget);
  });

  // On a phone, the same; the draft in the message box waits in the
  // drafts kept on the device until it's back (Prosciutto). The smallest
  // phone, at the largest text.
  testWidgets('a phone turned away with an agent open says so, and keeps '
      'the draft', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = 53 / 17;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final harness = Harness(Workspace(), localBackend: false);
    harness.connections.use(
      parsePairingCode('factory://?t=${'b' * 64}&name=studio&e=${'d' * 64}'),
    );
    await harness.show(tester, size: const Size(320, 568));
    harness.backend.connected.value = true;
    harness.backend.transcript.load(Transcript(agentId: 'eve'));
    harness.backend.workspace.apply(
      Workspace(
        agents: [
          Agent(
            id: 'eve',
            name: 'Eve',
            color: 'green',
            modelAvailable: true,
            status: AgentStatus.AGENT_STATUS_IDLE,
          ),
        ],
        selectedAgentId: 'eve',
        models: [Model(id: 'm', name: 'M')],
      ),
    );
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsOneWidget);
    final box = find.descendant(
      of: find.byType(MessageBox),
      matching: find.byType(EditableText),
    );
    await tester.enterText(box.first, 'Half a thought');
    await pumpFrames(tester);

    harness.backend.connected.value = false;
    harness.backend.refused.value = 'The machine turned this device away';
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    expect(find.byType(AgentView), findsNothing);
    expect(find.text('studio no longer allows this iPhone'), findsOneWidget);
    final switchTo = find.text('Use a new code').last;
    await tester.ensureVisible(switchTo);
    await pumpFrames(tester);
    final at = tester.getRect(switchTo);
    expect(at.top, greaterThanOrEqualTo(0));
    expect(at.bottom, lessThanOrEqualTo(568));
    expect(harness.drafts.of('eve'), 'Half a thought');
    // Its command wraps rather than scrolling out of sight, and Remove…,
    // which only asks, is quiet, as on a Mac (Lucy 49057).
    final command = find.text(_wrapped('factoryd pair'));
    expect(command, findsOneWidget);
    expect(tester.getRect(command).right, lessThanOrEqualTo(320));
    final remove = find.text('Remove…');
    expect(
      tester.widget<Text>(remove).style?.color,
      Palette.of(tester.element(remove)).textMuted,
    );

    harness.backend.refused.value = null;
    harness.backend.connected.value = true;
    await pumpFrames(tester);
    expect(find.byType(AgentView), findsOneWidget);
    expect(find.text('Half a thought'), findsOneWidget);
  });

  // Friend bug #38: a Claktory signed out of the person's account (its
  // sign-in lost to a failing disk) turns away a device that came through
  // the account; signing it in again lets it back in, so that's what the
  // device says, with Try again, never "add this device" (a second pairing).
  testWidgets('turned away by a Claktory signed out of the account, it says '
      'to sign that in again, and tries again', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(Workspace(), localBackend: false);
    harness.connections.use(
      parsePairingCode('factory://?t=${'b' * 64}&name=desktop&e=${'d' * 64}'),
    );
    await harness.show(tester, size: const Size(1200, 800));
    await pumpFrames(tester);
    harness.backend.connected.value = false;
    harness.backend.refused.value =
        'This Claktory was signed out of your Claktory account, so it '
        "can't let this device in. Sign it in again in its Settings → "
        'Account, then try again here.';
    await pumpFrames(tester);
    expect(find.text('desktop was signed out'), findsOneWidget);
    expect(
      find.text(
        'Sign desktop in to your Claktory account again, in Settings → '
        'Account on desktop. Then try again here.',
      ),
      findsOneWidget,
    );
    // No screen there: factoryd pair lets this Mac in by code, never the
    // account commands, which are the person's alone (Fugo 46047).
    expect(find.text('factoryd pair'), findsOneWidget);
    expect(find.textContaining('factoryd account'), findsNothing);
    expect(find.textContaining('new code'), findsNothing);
    await tester.tap(find.text('Try again'));
    await pumpFrames(tester);
    expect(harness.backend.refused.value, isNull, reason: 'trying again');
    expect(tester.takeException(), isNull);
  });

  // Lucy 49057: on a phone the commands were cut off ("sign-in --e…"):
  // they wrap onto more lines, all of them in sight, and Copy copies them
  // as they are. At the largest text, factoryd pair stays whole in sight.
  testWidgets("on a phone, the signed-out page's command wraps, whole", (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = 3.12;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final harness = Harness(Workspace(), localBackend: false);
    harness.connections.use(
      parsePairingCode('factory://?t=${'b' * 64}&name=desktop&e=${'d' * 64}'),
    );
    await harness.show(tester, size: const Size(320, 568));
    await pumpFrames(tester);
    harness.backend.connected.value = false;
    harness.backend.refused.value =
        'This Claktory was signed out of your Claktory account, so it '
        "can't let this device in.";
    await pumpFrames(tester);
    expect(tester.takeException(), isNull);
    const signIn = 'factoryd pair';
    final command = find.text(_wrapped(signIn));
    await tester.ensureVisible(command);
    await pumpFrames(tester);
    expect(command, findsOneWidget);
    final rect = tester.getRect(command);
    expect(rect.right, lessThanOrEqualTo(320));
    expect(rect.left, greaterThanOrEqualTo(0));
    expect(find.bySemanticsLabel(signIn), findsOneWidget);
    final copied = <String>[];
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async {
        if (call.method == 'Clipboard.setData') {
          copied.add((call.arguments as Map)['text'] as String);
        }
        return null;
      },
    );
    addTearDown(
      () => tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        null,
      ),
    );
    await tester.tap(find.text('Copy').first);
    await pumpFrames(tester);
    expect(copied, [signIn]);
  });

  // Zucchero's row 720: at the largest text a way out grows no more than
  // the button above it (2×).
  testWidgets("on a phone at the largest text, the problem page's links "
      'grow no more than its button', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    tester.platformDispatcher.textScaleFactorTestValue = 3.12;
    addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
    final harness = Harness(Workspace(), localBackend: false);
    harness.connections.use(
      parsePairingCode('factory://?t=${'b' * 64}&name=desktop&e=${'d' * 64}'),
    );
    await harness.show(tester, size: const Size(430, 932));
    await pumpFrames(tester);
    harness.backend
      ..connected.value = false
      ..refused.value = 'unknown device';
    await pumpFrames(tester);
    double scaleOf(String text) =>
        MediaQuery.textScalerOf(tester.element(find.text(text))).scale(10) / 10;
    expect(scaleOf('Remove…'), scaleOf('Use a new code'));
    expect(scaleOf('Remove…'), lessThanOrEqualTo(2));
  });

  // Abbacchio's 1676: a Claktory reached by code, signed out of its
  // account, asks to be signed in over its window; removed there, its
  // problem page takes the window, the sign-in gone, not over its
  // buttons.
  testWidgets('turned away, a Claktory that asked to be signed in shows '
      'its problem page, not its sign-in over it', (tester) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final harness = Harness(withAccount(signedOut), localBackend: false);
    harness.connections.use(
      parsePairingCode('factory://?t=${'b' * 64}&name=desktop&e=${'d' * 64}'),
    );
    await harness.show(tester, size: const Size(1200, 800));
    await pumpFrames(tester);
    harness.backend.workspace.apply(withAccount(signedOut));
    await pumpFrames(tester);
    expect(find.text('Sign in to Claktory'), findsOneWidget);
    harness.backend
      ..connected.value = false
      ..refused.value = 'unknown device';
    await pumpFrames(tester);
    expect(find.text('Sign in to Claktory'), findsNothing);
    expect(find.textContaining(RegExp(r'allows? this Mac')), findsWidgets);
    expect(find.text('Use a new code'), findsOneWidget);
  });
}

/// A command as a phone draws it: breakable after every character.
String _wrapped(String command) => command.characters.join('\u200B');
