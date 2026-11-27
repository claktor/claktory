import 'package:factory_app/model_select.dart' show ModelSelect;
import 'package:factory_app/claktory_identity.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/provider_picker.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:factory_app/controls.dart';
import 'package:factory_app/identity.dart';
import 'package:factory_app/scope.dart';
import 'package:factory_app/settings_shell.dart' show SettingsSearchField;

import 'harness.dart';
import 'ipad/as_ipad.dart';

const diff = '''Index: src/lib.rs
===================================================================
--- src/lib.rs
+++ src/lib.rs
@@ -1,2 +1,2 @@
 fn main() {
-    old();
+    new();
''';

final opus = Model(
  id: 'anthropic/claude-opus-5-5',
  name: 'Opus 5.5',
  efforts: ['low', 'medium', 'high'],
);
final gpt = Model(id: 'openai/gpt-6', name: 'GPT-6', efforts: ['minimal']);

Agent builder({
  String projectId = 'p1',
  bool modelAvailable = true,
  bool working = true,
}) => Agent(
  id: 'a1',
  projectId: projectId,
  name: 'Builder',
  prompt: 'Builds Factory',
  model: 'anthropic/claude-opus-5-5',
  effort: 'high',
  status: working
      ? AgentStatus.AGENT_STATUS_WORKING
      : AgentStatus.AGENT_STATUS_IDLE,
  contextTokens: Int64(123000),
  compactAtTokens: Int64(400000),
  queuedMessages: 2,
  modelAvailable: modelAvailable,
);

Workspace workspace({
  List<Agent>? agents,
  bool signedIn = true,
  String selected = 'a1',
  LoginFlow? login,
}) => Workspace(
  projects: [
    Project(id: 'p1', name: 'Factory'),
    Project(id: 'p2', name: 'Site'),
  ],
  agents: agents ?? [builder()],
  models: [if (signedIn) opus, if (signedIn) gpt],
  providers: [
    anthropic(
      signedInWith: signedIn ? 'Anthropic (Claude Pro/Max)' : '',
      canSignOut: signedIn,
    ),
  ],
  selectedAgentId: selected,
  settings: Settings(
    defaultModel: 'anthropic/claude-opus-5-5',
    defaultEffort: 'high',
    compactAtTokens: Int64(400000),
  ),
  login: login,
);

Transcript transcript() => Transcript(
  agentId: 'a1',
  entries: [
    Entry(
      seq: Int64(1),
      user: UserText(text: 'Fix the build'),
    ),
    Entry(
      seq: Int64(2),
      thinking: Thinking(text: 'Looking at the error'),
    ),
    Entry(
      seq: Int64(3),
      toolCall: ToolCall(
        callId: 'c1',
        name: 'edit_file',
        input: '{\n  "path": "src/lib.rs"\n}',
        output: diff,
        state: ToolCallState.TOOL_CALL_STATE_DONE,
      ),
    ),
    Entry(
      seq: Int64(4),
      toolCall: ToolCall(
        callId: 'c2',
        name: 'bash',
        input: '{"command": "cargo test"}',
        output: 'error[E0425]',
        state: ToolCallState.TOOL_CALL_STATE_FAILED,
      ),
    ),
    Entry(
      seq: Int64(5),
      compaction: Compaction(
        summary: 'Earlier work: set up the crate.',
        tokensBefore: Int64(412000),
      ),
    ),
    Entry(
      seq: Int64(6),
      notice: Notice(text: 'Stopped', level: NoticeLevel.NOTICE_LEVEL_ERROR),
    ),
    Entry(
      seq: Int64(7),
      assistant: AssistantText(
        text:
            '## Done\n\nThe build **passes**. Changed `main`:\n\n'
            '- one\n- two\n  1. nested\n\n'
            '```rust\nfn main() {}\n```\n\n'
            '| a | b |\n|---|---|\n| 1 | 2 |\n\n> quoted',
      ),
    ),
    Entry(
      seq: Int64(8),
      toolCall: ToolCall(
        callId: 'c3',
        name: 'read_file',
        state: ToolCallState.TOOL_CALL_STATE_PENDING,
      ),
    ),
  ],
);

Future<Harness> showApp(WidgetTester tester, Workspace snapshot) async {
  final harness = Harness(snapshot);
  if (snapshot.selectedAgentId.isNotEmpty) {
    harness.backend.transcript.load(transcript());
  }
  await harness.show(tester);
  return harness;
}

/// Settings → Defaults' model field.
final _defaultModel = find.byWidgetPredicate(
  (widget) => widget is ModelSelect && widget.title == 'Default model',
);

void main() {
  testWidgets('the agent view renders every kind of entry', (tester) async {
    await showApp(tester, workspace());

    expect(find.text('Builder'), findsWidgets);
    expect(find.text('Opus 5.5 · High'), findsOneWidget);
    expect(find.text('123k / 400k'), findsOneWidget);
    expect(find.text('Working'), findsOneWidget);
    // Working, a brighter stretch runs along the header's line too.
    expect(
      find.byWidgetPredicate(
        (widget) =>
            widget is WorkingWave && widget.color == const Color(0xFFFFFFFF),
      ),
      findsOneWidget,
    );
    expect(
      find.textContaining('queued'),
      findsNothing,
      reason: 'shown in the conversation',
    );
    expect(find.text('Fix the build'), findsOneWidget);
    expect(find.text('Earlier messages summarized'), findsOneWidget);
    expect(find.text('Failed'), findsOneWidget);
    expect(find.textContaining('passes'), findsOneWidget);

    await tester.tap(find.text('src/lib.rs'));
    await tester.tap(find.text('Earlier messages summarized'));
    await tester.pump();
    expect(find.text('Summarized at 412k tokens.'), findsOneWidget);
    expect(find.textContaining('+    new();'), findsOneWidget);
    expect(find.textContaining('set up the crate'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('without a model the agent says so and offers sign-in', (
    tester,
  ) async {
    final harness = await showApp(
      tester,
      workspace(
        signedIn: false,
        agents: [builder(modelAvailable: false, working: false)],
      ),
    );
    harness.backend.appliesLayouts = true;

    expect(find.text('No model available'), findsOneWidget);
    expect(find.text('Opus 5.5 · High'), findsNothing);
    // No model at all: connect one.
    expect(find.text('Connect a model so Builder can answer.'), findsOneWidget);
    // No composer to type into: sending couldn't work.
    expect(find.byType(EditableText), findsNothing);

    await tester.tap(find.text('Connect a model…'));
    await settle(tester);
    await tester.tap(find.text('Sign in with Claude'));
    await settle(tester);
    expect(harness.last.startLogin.providerId, 'anthropic');
    expect(harness.last.startLogin.methodId, 'oauth');
  });

  testWidgets("on an iPad the box's button is a finger's size, its words "
      'whole', (tester) async {
    // Sorbet's iPad pass: a 28pt row cut a 44pt button's label in half.
    asIPad();
    final snapshot = workspace(
      signedIn: false,
      agents: [builder(modelAvailable: false, working: false)],
    );
    final harness = Harness(snapshot);
    harness.backend.transcript.load(transcript());
    await harness.show(tester, size: const Size(1032, 1300));
    await settle(tester);
    final label = find.text('Connect a model…');
    final button = find.ancestor(of: label, matching: find.byType(Button));
    expect(tester.getSize(button).height, greaterThanOrEqualTo(44));
    // The label as tall as its line, not squeezed.
    final paragraph = tester.renderObject<RenderParagraph>(label);
    expect(
      tester.getSize(label).height,
      greaterThanOrEqualTo(paragraph.getMaxIntrinsicHeight(double.infinity)),
    );
    // Why sits level with it.
    final why = find.text('Connect a model so Builder can answer.');
    expect(tester.getCenter(why).dy, closeTo(tester.getCenter(button).dy, 1));
  });

  testWidgets('a browser sign-in opens once and takes a pasted code', (
    tester,
  ) async {
    final harness = await showApp(tester, workspace(signedIn: false));
    harness.page.value = AppPage.settings;
    harness.backend.settingsSection.value = 'providers';
    await settle(tester);
    await tester.tap(find.text('Anthropic (Claude Pro/Max)'));
    await settle(tester);

    // The backend sets the flow, then the provider publishes its address.
    final flow = LoginFlow(providerId: 'anthropic', methodId: 'oauth');
    await harness.apply(tester, workspace(signedIn: false, login: flow));
    expect(find.text('Starting…'), findsOneWidget);
    expect(harness.opened, isEmpty);

    final withUrl = flow.deepCopy()
      ..url = 'https://claude.ai/oauth/authorize?state=1'
      ..prompt = LoginPrompt(kind: LoginPromptKind.LOGIN_PROMPT_KIND_CODE);
    await harness.apply(tester, workspace(signedIn: false, login: withUrl));
    await harness.apply(tester, workspace(signedIn: false, login: withUrl));
    expect(harness.opened, ['https://claude.ai/oauth/authorize?state=1']);
    expect(find.text('Open again'), findsOneWidget);
    expect(find.text('Copy link'), findsOneWidget);

    await tester.enterText(settingsField, 'code#state');
    await tester.pump();
    await tester.ensureVisible(find.text('Continue'));
    await tester.pump();
    await tester.tap(find.text('Continue'));
    await settle(tester);
    expect(harness.last.answerLogin.text, 'code#state');

    // Signed in: the flow ends and the provider says how.
    await harness.apply(tester, workspace());
    expect(
      find.text('Signed in with Anthropic (Claude Pro/Max)'),
      findsOneWidget,
    );
    expect(find.text('Sign out'), findsOneWidget);
  });

  testWidgets('a failed sign-in can be retried or closed', (tester) async {
    final harness = await showApp(
      tester,
      workspace(
        signedIn: false,
        login: LoginFlow(
          providerId: 'anthropic',
          methodId: 'api_key',
          error: 'The key was rejected.',
        ),
      ),
    );
    harness.page.value = AppPage.settings;
    harness.backend.settingsSection.value = 'providers';
    await settle(tester);

    expect(find.text('The key was rejected.'), findsOneWidget);
    await tester.tap(find.text('Try again'));
    await settle(tester);
    expect(harness.last.startLogin.methodId, 'api_key');

    await tester.tap(find.text('Close'));
    await settle(tester);
    expect(harness.last.hasCancelLogin(), isTrue);
  });

  testWidgets('a secret prompt hides what is typed', (tester) async {
    final harness = await showApp(
      tester,
      workspace(
        signedIn: false,
        login: LoginFlow(
          providerId: 'anthropic',
          methodId: 'api_key',
          prompt: LoginPrompt(
            kind: LoginPromptKind.LOGIN_PROMPT_KIND_SECRET,
            message: 'Paste your Anthropic API key',
          ),
        ),
      ),
    );
    harness.page.value = AppPage.settings;
    harness.backend.settingsSection.value = 'providers';
    await settle(tester);

    final secret = settingsField;
    expect(tester.widget<EditableText>(secret).obscureText, isTrue);
    await tester.enterText(secret, 'sk-ant-1');
    await tester.testTextInput.receiveAction(TextInputAction.done);
    await settle(tester);
    expect(harness.last.answerLogin.text, 'sk-ant-1');
  });

  testWidgets('defaults offer only models that can run', (tester) async {
    final harness = await showApp(tester, workspace());
    harness.page.value = AppPage.settings;
    await settle(tester);

    await revealInSettings(tester, _defaultModel);
    await settle(tester);
    // The field names the provider after the model (U3).
    expect(
      find.text('Opus 5.5  Anthropic', findRichText: true),
      findsOneWidget,
    );
    await tester.tap(_defaultModel);
    await settle(tester);
    await tester.tap(find.text('GPT-6').last);
    await settle(tester);
    final update = harness.last.updateSettings;
    expect(update.defaultModel, 'openai/gpt-6');
    // "high" isn't an effort GPT-6 accepts.
    expect(update.hasDefaultEffort(), isTrue);

    await tester.enterText(settingsField, '300k');
    await tester.pump();
    await tester.ensureVisible(find.text('Save'));
    await tester.pump();
    await tester.tap(find.text('Save'));
    await settle(tester);
    expect(harness.last.updateSettings.compactAtTokens, Int64(300000));
  });

  testWidgets('apps are in the Marketplace, where the backend runs them, '
      'not in Settings', (tester) async {
    final harness = await showApp(tester, workspace()..appsAvailable = true);
    harness.page.value = AppPage.settings;
    await settle(tester);
    await revealInSettings(tester, _defaultModel);
    await settle(tester);
    expect(find.text('Install from a file…'), findsNothing);

    harness.page.value = AppPage.marketplace;
    await settle(tester);
    await tester.tap(find.text('Apps'));
    await settle(tester);
    expect(find.text('Install from a file…'), findsOneWidget);

    // No Apps tab where the backend doesn't run apps.
    harness.backend.workspace.apply(workspace());
    await settle(tester);
    expect(find.text('Apps'), findsNothing);
  });

  testWidgets('agent settings move, change model, and delete', (tester) async {
    final harness = await showApp(tester, workspace());

    await openAgentSettings(tester);
    expect(find.text('Prompt'), findsOneWidget);

    await tester.ensureVisible(find.byType(ModelSelect).first);
    await tester.tap(find.byType(ModelSelect).first);
    await settle(tester);
    await tester.tap(find.text('GPT-6').last);
    await settle(tester);
    await tester.ensureVisible(find.text('Factory').last);
    await settle(tester);
    await tester.tap(find.text('Factory').last);
    await settle(tester);
    await tester.tap(find.text('Workspace (no project)').last);
    await settle(tester);
    await tester.tap(find.text('Save'));
    await settle(tester);
    final update = harness.last.updateAgent;
    expect(update.model, 'openai/gpt-6');
    expect(update.effort, 'minimal');
    expect(update.projectId, '');
    expect(update.hasName(), isFalse);

    await openAgentSettings(tester);
    await tester.tap(find.text('Delete agent…'));
    await settle(tester);
    expect(find.text('Its conversation is deleted too.'), findsOneWidget);
    await tester.tap(find.text('Delete agent'));
    await settle(tester);
    expect(harness.last.deleteAgent.agentId, 'a1');
  });

  testWidgets('project settings rename and delete, keeping files', (
    tester,
  ) async {
    final harness = await showApp(tester, workspace());

    // A right click opens a project's settings; a left click focuses it.
    await tester.tap(find.text('Factory'), buttons: kSecondaryButton);
    await settle(tester);
    await tester.enterText(find.byType(EditableText).last, 'Factory app');
    await tester.pump();
    await tester.tap(find.text('Save'));
    await settle(tester);
    expect(harness.last.updateProject.name, 'Factory app');

    await tester.tap(find.text('Factory'), buttons: kSecondaryButton);
    await settle(tester);
    await tester.tap(find.text('Delete project…'));
    await settle(tester);
    expect(
      find.text(
        "Its agent and its conversation are deleted. The project's folder "
        'and its files are kept.',
      ),
      findsOneWidget,
    );
    await tester.tap(find.text('Delete project'));
    await settle(tester);
    expect(harness.last.deleteProject.projectId, 'p1');
  });

  testWidgets('dragging an agent moves it between places', (tester) async {
    final harness = await showApp(
      tester,
      workspace(
        agents: [
          Agent(id: 'rivet', name: 'Rivet', modelAvailable: true),
          builder(),
        ],
      ),
    );
    final agent = tester.getCenter(find.text('Builder').first);

    Future<void> dragTo(Offset target) async {
      final gesture = await tester.startGesture(
        agent,
        kind: PointerDeviceKind.mouse,
      );
      await tester.pump(const Duration(milliseconds: 50));
      for (var i = 1; i <= 10; i++) {
        await gesture.moveTo(Offset.lerp(agent, target, i / 10)!);
        await tester.pump(const Duration(milliseconds: 16));
      }
      await gesture.up();
      await settle(tester);
    }

    // Onto its own project: nothing to do.
    await dragTo(tester.getCenter(find.text('Factory')));
    expect(harness.backend.sent, isEmpty);

    await dragTo(tester.getCenter(find.text('Site')));
    expect(harness.last.arrangeAgent.agentId, 'a1');
    expect(harness.last.arrangeAgent.projectId, 'p2');
    expect(harness.last.arrangeAgent.parentId, '', reason: 'its top level');

    // Beside Rivet, on its upper edge: a gap in the top group, so it's
    // pinned there and stays in its project (pinned-agents.md §5).
    final rivet = tester.getRect(
      find.ancestor(of: find.text('Rivet'), matching: find.byType(Row)).first,
    );
    await dragTo(rivet.topCenter + const Offset(0, 3));
    expect(harness.last.pinAgent.agentId, 'a1');
    expect(harness.last.pinAgent.beforeId, '');

    // Onto Rivet's row: out of its project, reporting to Rivet.
    await dragTo(rivet.center);
    expect(harness.last.arrangeAgent.projectId, '');
    expect(harness.last.arrangeAgent.hasProjectId(), isTrue);
    expect(harness.last.arrangeAgent.parentId, 'rivet');
  });

  testWidgets('an agent with news stands out until it is opened', (
    tester,
  ) async {
    Workspace snapshot({required bool unseen, String selected = 'a1'}) =>
        workspace(
          selected: selected,
          agents: [
            builder(working: false),
            Agent(
              id: 'a2',
              name: 'Writer',
              modelAvailable: true,
              unseen: unseen,
            ),
          ],
        );
    final news = find.byWidgetPredicate(
      (widget) =>
          widget is Semantics &&
          widget.properties.label == 'Writer has something new',
    );
    FontWeight? weight(String name) =>
        tester.widget<Text>(find.text(name).first).style?.fontWeight;

    final harness = await showApp(tester, snapshot(unseen: true));
    expect(news, findsOneWidget);
    expect(weight('Writer'), FontWeight.w600);

    await tester.tap(find.text('Writer'));
    await settle(tester);
    expect(harness.last.selectAgent.agentId, 'a2');
    // The backend clears it when the agent is selected.
    await harness.apply(tester, snapshot(unseen: false, selected: 'a2'));
    expect(news, findsNothing);
    expect(weight('Writer'), FontWeight.w500);
  });

  testWidgets("the message box says the agent's own hint", (tester) async {
    const hint =
        'Tell Rivet what you want to do with Factory. It will guide you, '
        'set it up or build it for you, whatever it takes, as long as it '
        'takes, with every detail covered and nothing left out at all.';
    final harness = await showApp(
      tester,
      workspace(agents: [builder(working: false)..placeholder = hint]),
    );
    final shown = find.text(hint);
    expect(shown, findsOneWidget);
    // One line, cut off cleanly.
    final line = TextStyles.body.fontSize! * TextStyles.body.height!;
    expect(tester.getSize(shown).height, closeTo(line, 1));
    expect(tester.widget<Text>(shown).overflow, TextOverflow.ellipsis);

    // Empty: the default.
    await harness.apply(tester, workspace(agents: [builder(working: false)]));
    expect(find.text('Message Builder'), findsOneWidget);
  });

  testWidgets('several hints take turns, sliding up, and hold still on hover', (
    tester,
  ) async {
    await showApp(
      tester,
      workspace(
        agents: [
          builder(
            working: false,
          )..placeholder = 'Build me a website\n\n  Do my taxes \nPlan my trip',
        ],
      ),
    );
    Finder hint(String text) => find.descendant(
      of: find.byType(RotatingText),
      matching: find.text(text),
    );
    expect(hint('Build me a website'), findsOneWidget);
    expect(hint('Do my taxes'), findsNothing);
    final at = tester.getTopLeft(hint('Build me a website')).dy;

    // After a while the next slides up into its place.
    await tester.pump(rotateEvery);
    await tester.pump(rotateSlide ~/ 2);
    expect(hint('Build me a website'), findsOneWidget);
    expect(hint('Do my taxes'), findsOneWidget);
    expect(tester.getTopLeft(hint('Do my taxes')).dy, greaterThan(at));
    expect(tester.getTopLeft(hint('Build me a website')).dy, lessThan(at));
    await tester.pump(rotateSlide);
    expect(hint('Build me a website'), findsNothing);
    expect(hint('Do my taxes'), findsOneWidget);
    expect(tester.getTopLeft(hint('Do my taxes')).dy, at);

    // Hovered, it holds still to be read.
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: tester.getCenter(hint('Do my taxes')));
    await tester.pump();
    await tester.pump(rotateEvery);
    await tester.pump(rotateSlide);
    await tester.pump(rotateSlide);
    expect(hint('Do my taxes'), findsOneWidget);
    expect(hint('Plan my trip'), findsNothing);
    await mouse.moveTo(Offset.zero);
    await tester.pump();
    await tester.pump(rotateEvery);
    await tester.pump(rotateSlide);
    await tester.pump(rotateSlide);
    expect(hint('Plan my trip'), findsOneWidget);
    // And round again.
    await tester.pump(rotateEvery);
    await tester.pump(rotateSlide);
    await tester.pump(rotateSlide);
    expect(hint('Build me a website'), findsOneWidget);
    await mouse.removePointer();

    // Typing hides it.
    await tester.enterText(find.byType(EditableText), 'Hi');
    await tester.pump();
    expect(find.byType(RotatingText), findsNothing);
  });

  testWidgets('with reduced motion, hints just swap', (tester) async {
    await showApp(
      tester,
      workspace(
        agents: [builder(working: false)..placeholder = 'One idea\nAnother'],
      ),
    );
    tester.platformDispatcher.accessibilityFeaturesTestValue =
        const FakeAccessibilityFeatures(disableAnimations: true);
    addTearDown(tester.platformDispatcher.clearAccessibilityFeaturesTestValue);
    await tester.pump();
    await tester.pump(rotateEvery);
    expect(find.text('One idea'), findsNothing);
    expect(find.text('Another'), findsOneWidget);
    expect(find.byType(FractionalTranslation), findsNothing);
  });

  testWidgets('the hint setting takes several lines', (tester) async {
    final harness = await showApp(
      tester,
      workspace(agents: [builder(working: false)]),
    );
    await openAgentSettings(tester);
    expect(find.textContaining('separate lines'), findsOneWidget);
    final field = find.descendant(
      of: find.ancestor(
        of: find.text('Message box hint'),
        matching: find.byType(Field),
      ),
      matching: find.byType(EditableText),
    );
    expect(tester.widget<EditableText>(field).maxLines, greaterThan(1));
    await tester.enterText(field, ' Build a site \n\nDo my taxes\n');
    await tester.pump();
    await tester.tap(find.text('Save'));
    await settle(tester);
    expect(harness.last.updateAgent.placeholder, 'Build a site\nDo my taxes');
  });

  testWidgets('agent settings set and clear the message box hint', (
    tester,
  ) async {
    final harness = await showApp(
      tester,
      workspace(agents: [builder(working: false)..placeholder = 'Old hint']),
    );
    await openAgentSettings(tester);
    expect(find.text('Message box hint'), findsOneWidget);
    final field = find.descendant(
      of: find.ancestor(
        of: find.text('Message box hint'),
        matching: find.byType(Field),
      ),
      matching: find.byType(EditableText),
    );
    await tester.enterText(field, '  What should Builder build?  ');
    await tester.pump();
    await tester.tap(find.text('Save'));
    await settle(tester);
    expect(harness.last.updateAgent.placeholder, 'What should Builder build?');
    expect(harness.last.updateAgent.hasName(), isFalse);

    await openAgentSettings(tester);
    await tester.enterText(field, '');
    await tester.pump();
    // Left empty, it shows the default as its own hint.
    expect(find.text('Message Builder'), findsWidgets);
    await tester.tap(find.text('Save'));
    await settle(tester);
    expect(harness.last.updateAgent.hasPlaceholder(), isTrue);
    expect(harness.last.updateAgent.placeholder, '');
  });

  testWidgets('the composer hugs its text, growing line by line', (
    tester,
  ) async {
    final harness = await showApp(
      tester,
      workspace(agents: [builder(working: false)]),
    );
    double height() => tester.getSize(composerBox()).height;
    final empty = height();
    final line = TextStyles.body.fontSize! * TextStyles.body.height!;
    // One row: a line with room around it, no reserved space below.
    expect(empty, lessThanOrEqualTo(line + 2 * Space.m + 2));

    await tester.enterText(find.byType(EditableText), 'one line');
    await tester.pump();
    expect(height(), empty);

    await tester.enterText(find.byType(EditableText), 'one\ntwo\nthree');
    await tester.pump();
    expect(height(), closeTo(empty + 2 * line, 1));

    // Working: Stop sits on a line above the box, at its end, and the box
    // and its text field stay exactly as they were.
    await tester.enterText(
      find.byType(EditableText),
      List.filled(40, 'word').join(' '),
    );
    await tester.pump();
    final box = tester.getRect(composerBox());
    final field = tester.getRect(find.byType(EditableText));
    await harness.apply(tester, workspace(agents: [builder(working: true)]));
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('Stop  esc'), findsOneWidget);
    expect(find.text('Working'), findsOneWidget);
    expect(tester.getRect(composerBox()), box);
    expect(tester.getRect(find.byType(EditableText)), field);
    expect(
      tester.getBottomRight(find.text('Stop  esc')).dy,
      lessThan(box.top),
      reason: 'above the box',
    );
    expect(
      tester
          .getTopRight(
            find.ancestor(
              of: find.text('Stop  esc'),
              matching: find.byType(Button),
            ),
          )
          .dx,
      closeTo(box.right, 1),
      reason: 'at its end',
    );

    // Stopping, it goes, and still nothing in the box moves.
    await harness.apply(tester, workspace(agents: [builder(working: false)]));
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('Stop  esc'), findsNothing);
    expect(tester.getRect(composerBox()), box);
    expect(tester.getRect(find.byType(EditableText)), field);
  });

  testWidgets('the composer wraps text at the full width of its box', (
    tester,
  ) async {
    await showApp(tester, workspace(agents: [builder(working: false)]));
    final text = List.filled(120, 'word').join(' ');
    await tester.enterText(find.byType(EditableText), text);
    await tester.pump();
    final editable = tester.renderObject<RenderEditable>(
      find.descendant(
        of: find.byType(EditableText),
        matching: find.byWidgetPredicate(
          (widget) => widget.runtimeType.toString() == '_Editable',
        ),
      ),
    );
    final box = tester.getRect(composerBox());
    final field = editable.localToGlobal(Offset.zero) & editable.size;
    // The field spans the box, less its padding and the paperclip at its
    // end.
    expect(field.left - box.left, lessThanOrEqualTo(Space.m + 1));
    expect(
      box.right - field.right,
      lessThanOrEqualTo(Space.xs + 28 + Space.xs + 1),
    );
    // And lines run to its right edge, short of one word.
    var right = 0.0;
    for (final line in editable.getBoxesForSelection(
      TextSelection(baseOffset: 0, extentOffset: text.length),
    )) {
      if (line.right > right) right = line.right;
    }
    expect(right, greaterThan(editable.size.width - 5 * 13));
    // Lines are as far apart as body text.
    expect(
      editable.preferredLineHeight,
      closeTo(TextStyles.body.fontSize! * TextStyles.body.height!, 0.5),
    );
  });

  group('slash commands', () {
    final commands = [
      SlashCommand(name: 'clear', description: 'Start a new conversation.'),
      SlashCommand(name: 'compact', description: 'Summarize it now.'),
    ];
    Future<Harness> showCommands(WidgetTester tester) async {
      final agent = builder(working: false)..commands.addAll(commands);
      final harness = await showApp(tester, workspace(agents: [agent]));
      await settle(tester);
      return harness;
    }

    Future<void> type(WidgetTester tester, String text) async {
      await tester.enterText(find.byType(EditableText), text);
      await tester.pump();
    }

    Future<void> press(WidgetTester tester, LogicalKeyboardKey key) async {
      await tester.sendKeyEvent(key);
      await settle(tester);
    }

    testWidgets('a slash opens the menu, filtered as you type', (tester) async {
      final harness = await showCommands(tester);
      await type(tester, '/');
      expect(find.text('/clear'), findsOneWidget);
      expect(find.text('/compact'), findsOneWidget);
      expect(find.text('Start a new conversation.'), findsOneWidget);

      await type(tester, '/comp');
      expect(find.text('/clear'), findsNothing);
      expect(find.text('/compact'), findsOneWidget);

      await press(tester, LogicalKeyboardKey.enter);
      expect(harness.last.runCommand.name, 'compact');
      expect(harness.last.runCommand.agentId, 'a1');
      expect(harness.last.runCommand.args, '');
      expect(find.text('/compact'), findsNothing, reason: 'closed, box empty');
      expect(
        tester.widget<EditableText>(find.byType(EditableText)).controller.text,
        '',
      );
    });

    testWidgets('arrows pick, Esc closes, Tab completes', (tester) async {
      final harness = await showCommands(tester);
      await type(tester, '/c');
      await press(tester, LogicalKeyboardKey.arrowDown);
      await press(tester, LogicalKeyboardKey.enter);
      expect(harness.last.runCommand.name, 'compact');

      await type(tester, '/c');
      await press(tester, LogicalKeyboardKey.escape);
      expect(find.text('/clear'), findsNothing);
      expect(
        harness.backend.sent.whereType<ClientMessage>().where(
          (m) => m.hasStopAgent(),
        ),
        isEmpty,
        reason: 'Esc closed the menu, nothing else',
      );

      await type(tester, '/cl');
      await press(tester, LogicalKeyboardKey.tab);
      expect(
        tester.widget<EditableText>(find.byType(EditableText)).controller.text,
        '/clear ',
      );
    });

    testWidgets('text that is not a command is sent as a message', (
      tester,
    ) async {
      final harness = await showCommands(tester);
      await type(tester, '/usr/bin is where it lives');
      expect(find.text('/clear'), findsNothing);
      await press(tester, LogicalKeyboardKey.enter);
      expect(harness.last.sendMessage.text, '/usr/bin is where it lives');
    });

    testWidgets('a refused command says why, and keeps what was typed', (
      tester,
    ) async {
      final harness = await showCommands(tester);
      harness.backend.refuse = 'Builder is working; stop it first.';
      await type(tester, '/compact');
      await press(tester, LogicalKeyboardKey.enter);
      expect(
        find.text('/compact: Builder is working; stop it first.'),
        findsOneWidget,
      );
      expect(
        tester.widget<EditableText>(find.byType(EditableText)).controller.text,
        '/compact',
      );
    });

    testWidgets('a click on a command runs it', (tester) async {
      final harness = await showCommands(tester);
      await type(tester, '/');
      await tester.tap(find.text('/clear'));
      await settle(tester);
      expect(harness.last.runCommand.name, 'clear');
    });
  });

  testWidgets("a message that isn't sent stays, and Edit takes it back", (
    tester,
  ) async {
    final harness = await showApp(tester, workspace());
    harness.backend.refuse = 'Not connected to the backend.';

    await tester.enterText(find.byType(EditableText).first, 'run the tests');
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));

    // It shows at once, and stays in the conversation, not sent: yet, as
    // it goes by itself once the connection is back (#401).
    expect(find.text('run the tests'), findsOneWidget);
    expect(find.text('Not sent yet'), findsOneWidget);
    EditableText editable() =>
        tester.widget<EditableText>(find.byType(EditableText).first);
    expect(editable().controller.text, '');

    await tester.tap(find.text('Edit'));
    await tester.pump();
    expect(editable().controller.text, 'run the tests');
    expect(find.text('Not sent yet'), findsNothing);
  });

  testWidgets('new agents go in projects, never the workspace', (tester) async {
    final harness = await showApp(
      tester,
      workspace(
        agents: [Agent(id: 'rivet', name: 'Rivet')],
        selected: '',
        signedIn: false,
      ),
    );
    // Rivet stands alone at the top, with no heading.
    expect(find.text('Workspace'), findsNothing);
    expect(
      tester.getTopLeft(find.text('Rivet')).dy,
      lessThan(tester.getTopLeft(find.text('Projects')).dy),
    );

    // Rivet can't run without a model, so a project's "+" opens the form.
    await tapProjectPlus(tester, 'Factory');
    expect(find.text('No model available'), findsWidgets);
    // The project select offers projects only.
    await tester.tap(find.text('Factory').last);
    await settle(tester);
    expect(find.text('Workspace (no project)'), findsNothing);
    await tester.tap(find.text('Factory').last);
    await settle(tester);
    await tester.enterText(find.byType(EditableText).first, 'Scout');
    await tester.pump();
    await tester.tap(find.text('Create'));
    await settle(tester);
    expect(harness.last.createAgent.name, 'Scout');
    expect(harness.last.createAgent.projectId, 'p1');

    // With no agents and no projects, start with a project.
    await harness.apply(tester, Workspace());
    expect(find.text('No agents yet'), findsOneWidget);
    await tester.tap(find.text('New project'));
    await settle(tester);
    expect(find.text('New project'), findsWidgets);
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await settle(tester);

    harness.backend.connected.value = false;
    await harness.apply(tester, Workspace());
    expect(find.text('Connecting…'), findsWidgets);
  });
}

/// The message box: the bordered container around the text field.
Finder composerBox() => find
    .ancestor(of: find.byType(EditableText), matching: find.byType(Container))
    .evaluate()
    .where((element) {
      final decoration = (element.widget as Container).decoration;
      return decoration is BoxDecoration && decoration.border != null;
    })
    .map((element) => find.byWidget(element.widget))
    .first;

/// The first field in Settings after Your name (which holds the harness's
/// "Matt") and the Claktory's name.
Finder get settingsField => find
    .byElementPredicate(
      (element) =>
          element.widget is EditableText &&
          (element.widget as EditableText).controller.text != 'Matt' &&
          element.findAncestorWidgetOfExactType<ClaktoryIdentity>() == null &&
          element.findAncestorWidgetOfExactType<ProviderPicker>() == null &&
          // Not Settings' search, at the top of its index.
          element.findAncestorWidgetOfExactType<SettingsSearchField>() == null,
    )
    .first;
