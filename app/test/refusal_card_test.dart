import 'package:factory_app/forms.dart' show AgentSettingsForm;
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/refusal_card.dart';
import 'package:factory_app/tell_rivet.dart' show ErrorLine;
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

import 'chats_test.dart' show chat, showChat;
import 'harness.dart';

Entry _notice(
  int seq,
  String text, {
  String category = '',
  NoticeLevel level = NoticeLevel.NOTICE_LEVEL_ERROR,
}) => Entry(
  seq: Int64(seq),
  notice: Notice(text: text, level: level, refusalCategory: category),
);

Workspace _workspace({String fallback = '', String byDefault = ''}) =>
    Workspace(
      projects: [Project(id: 'p1', name: 'Factory', path: '/w')],
      agents: [
        Agent(
          id: 'june',
          name: 'June',
          projectId: 'p1',
          model: 'anthropic/claude-fable-5',
          modelAvailable: true,
          fallbackModel: fallback,
        ),
      ],
      models: [
        Model(id: 'anthropic/claude-fable-5', name: 'Claude Fable 5'),
        Model(id: 'anthropic/claude-opus-5', name: 'Claude Opus 5'),
      ],
      settings: Settings(defaultFallbackModel: byDefault),
      selectedAgentId: 'june',
    );

Future<Harness> _show(
  WidgetTester tester,
  Workspace workspace,
  List<Entry> entries,
) async {
  final harness = Harness(workspace);
  harness.backend.transcript.load(
    Transcript(agentId: 'june', entries: entries),
  );
  await harness.show(tester);
  await settle(tester);
  return harness;
}

void main() {
  testWidgets("a step the model declined is its own card, not a failure: "
      'the area it named, its words, and where to choose a fallback', (
    tester,
  ) async {
    await _show(tester, _workspace(), [
      _notice(3, 'This touches on malware development.', category: 'cyber'),
    ]);
    expect(find.byType(RefusalCard), findsOneWidget);
    expect(find.byType(ErrorLine), findsNothing);
    expect(
      find.textContaining(
        'Declined for safety · cybersecurity',
        findRichText: true,
      ),
      findsOneWidget,
    );
    expect(find.text('This touches on malware development.'), findsOneWidget);
    expect(
      find.text('Claktory can try declined steps again on another model.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Choose a fallback model…'));
    await settle(tester);
    expect(find.byType(AgentSettingsForm), findsOneWidget);
  });

  testWidgets('no area named reads as none, never "unspecified"', (
    tester,
  ) async {
    await _show(tester, _workspace(), [
      _notice(3, 'I can\'t help with that.', category: 'unspecified'),
    ]);
    expect(
      find.textContaining('Declined for safety', findRichText: true),
      findsOneWidget,
    );
    expect(
      find.textContaining('unspecified', findRichText: true),
      findsNothing,
    );
  });

  testWidgets("with a fallback, the agent's own or the default, it says "
      'which model tries again', (tester) async {
    await _show(tester, _workspace(fallback: 'anthropic/claude-opus-5'), [
      _notice(3, 'Declined.', category: 'bio'),
    ]);
    expect(
      find.text('Claktory tries declined steps once more on Claude Opus 5.'),
      findsOneWidget,
    );
    expect(find.text('Choose a fallback model…'), findsNothing);
  });

  testWidgets('without its own, the Claktory-wide default', (tester) async {
    await _show(tester, _workspace(byDefault: 'anthropic/claude-fable-5'), [
      _notice(3, 'Declined.', category: 'bio'),
    ]);
    expect(
      find.text('Claktory tries declined steps once more on Claude Fable 5.'),
      findsOneWidget,
    );
  });

  testWidgets("a failure is still an error line, and the provider's handoff "
      'a quiet line of its own', (tester) async {
    await _show(tester, _workspace(), [
      _notice(3, 'The model request failed.'),
      _notice(
        4,
        'claude-fable-5 declined this step (category: cyber); '
        'claude-opus-4-8 answered instead.',
        level: NoticeLevel.NOTICE_LEVEL_INFO,
      ),
    ]);
    expect(find.byType(RefusalCard), findsNothing);
    expect(find.byType(ErrorLine), findsOneWidget);
    expect(
      find.text(
        'claude-fable-5 declined this step (category: cyber); '
        'claude-opus-4-8 answered instead.',
      ),
      findsOneWidget,
    );
  });

  testWidgets("in a chat, the card has no agent's settings to offer", (
    tester,
  ) async {
    await showChat(
      tester,
      with_: chat(id: 'c1'),
      entries: [_notice(3, 'Declined.', category: 'cyber')],
    );
    expect(find.byType(RefusalCard), findsOneWidget);
    expect(find.text('Choose a fallback model…'), findsNothing);
  });

  test('the area as shown', () {
    expect(refusalArea('unspecified'), '');
    expect(refusalArea('cyber'), 'cybersecurity');
    expect(refusalArea('bio'), 'biology');
    expect(refusalArea('Chem'), 'chem');
  });
}
