// A friend's name inside People's sentences: what
// reorders text shows by its code and the name sits in an isolate, so a
// friend named with a right-to-left override can't turn the words around
// it backwards. Those names come from other people.
import 'package:factory_app/people/chat_view.dart';
import 'package:factory_app/tunnel/api/dm.dart' show DmMembers;
import 'package:flutter_test/flutter_test.dart';

const _evil = 'Evil\u202EMac';

/// What reorders shows by its code, so the name reads as plain text.
const _shown = 'Evil\\u{202E}Mac';

/// A right-to-left name sits in an isolate.
const _hebrew = 'עדי';
const _isolated = '\u2068עדי\u2069';

void main() {
  test("a friend's name leaves the sentence around it intact", () {
    expect(
      resendQuestion(_evil),
      "Resend to $_shown? $_shown's conversation has changed since.",
    );
    expect(
      departedLine(mine: false, friend: _evil),
      'From a device $_shown has since removed',
    );
    expect(
      resendQuestion(_hebrew),
      "Resend to $_isolated? $_isolated's conversation has changed since.",
    );
    expect(
      departedLine(mine: false, friend: _hebrew),
      'From a device $_isolated has since removed',
    );
    // Messages it joined again past a friend's key change.
    expect(
      memberLines(
        const DmMembers(
          joined: [],
          setUpAgain: [],
          added: [],
          removed: [],
          keyChanged: [],
          skipped: 'ana',
        ),
        me: 'matt',
        friend: _hebrew,
      ),
      [
        "Some messages sent when $_isolated's security key changed can't be "
            'shown on this device.',
      ],
    );
    // A plain name reads as it is.
    expect(
      resendQuestion('Ana'),
      "Resend to Ana? Ana's conversation has changed since.",
    );
  });
}
