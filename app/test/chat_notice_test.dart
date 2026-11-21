import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/tell_rivet.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

import 'chats_test.dart' show chat, showChat;

void main() {
  testWidgets("an error in a chat is the chat's, not the selected agent's", (
    tester,
  ) async {
    await showChat(
      tester,
      with_: chat(id: 'c1', title: 'Trains to Lyon'),
      entries: [
        Entry(
          seq: Int64(3),
          notice: Notice(
            level: NoticeLevel.NOTICE_LEVEL_ERROR,
            text: 'The provider timed out.',
          ),
        ),
      ],
    );
    expect(find.text('The provider timed out.'), findsOneWidget);
    final tell = tester.widget<TellRivet>(find.byType(TellRivet));
    expect(tell.about, 'In the chat "Trains to Lyon" (chat c1):');
  });
}
