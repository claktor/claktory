// A card's answer, wherever the card shows: what's typed in it, one draft per
// card, so a card's words stay with that card while the app runs, in its
// review bar, on its card and in focus mode, and never in another card's
// field. Kept on the device in memory only, as drafts are, and dropped once
// the answer's sent.
import 'package:flutter/foundation.dart' show listEquals;
import 'package:flutter/widgets.dart';

import 'backend.dart';

/// The key of [agentId]'s card [askId]: the one its voice drafts and files
/// are kept by too.
String cardKey(String agentId, String askId) => 'ask/$agentId/$askId';

/// What's typed in each card of a Claktory ([of]): one field per card key,
/// kept while the app runs.
class CardDrafts {
  CardDrafts._();

  static final _drafts = Expando<CardDrafts>('CardDrafts');

  /// [backend]'s drafts: one store per Claktory.
  static CardDrafts of(Backend backend) => _drafts[backend] ??= CardDrafts._();

  final _fields = <String, TextEditingController>{};

  /// The field of the card [key] ([cardKey]): the same one wherever the
  /// card shows; whoever shows it never disposes of it.
  TextEditingController field(String key) =>
      _fields.putIfAbsent(key, TextEditingController.new);

  /// The answer to card [key] went: its words with it.
  void clear(String key) => _fields.remove(key)?.clear();
}

/// A [CardAnswer] callback that sent nothing, by the person's choice or the
/// card's: a confirmation cancelled, Execute's first press arming its
/// second. The dock stays on the card, with no error.
class AnswerNotSent implements Exception {
  const AnswerNotSent();

  @override
  String toString() => 'AnswerNotSent';
}

/// A card's answer as data, for a dock outside the card that draws or binds
/// it: focus mode's stage (its answer bar, ⌘↩ and ⌘⌫) and the
/// phone's thumb row. The card publishes it into the [AnswerDock] above it
/// as it changes; its own controls stay the source of what's sent.
@immutable
class CardAnswer {
  const CardAnswer({
    this.key = '',
    required this.docked,
    this.field,
    this.placeholder = '',
    this.wantsText = false,
    required this.yes,
    required this.no,
    this.submit = 'Submit',
    this.typed = false,
    this.busy = false,
    this.status,
    this.error,
    this.onYes,
    this.onSubmit,
    this.onNo,
    this.filesKey,
    this.pressTwice = false,
    this.keep = false,
    this.choices = const [],
  });

  /// The Needs you item the card answers (NeedsYouItem.key: 'agent:kai/7',
  /// 'keepAwake'…), so the dock knows whose it is: the dock's own
  /// [AnswerDock.itemKey] when it says, else the card's. Its draft is by
  /// [cardKey].
  final String key;

  /// Whether the card leaves its field and its yes and no to the dock
  /// (ask cards and reviews): the dock draws them, the card doesn't. Else
  /// its controls stay in its body and the dock only binds the keys.
  final bool docked;

  /// What's typed: the card's draft ([CardDrafts]); null when not docked.
  final TextEditingController? field;

  /// The field's words when empty, as the card shows them.
  final String placeholder;

  /// The card asks for words: they're its answer (Send).
  final bool wantsText;

  /// The yes ("Approve", "Send", "Done", "Allow", "Execute"…) and the no
  /// ("Dismiss", "Can't do this", "Decline"…), as the card's buttons say.
  final String yes;
  final String no;

  /// Words are typed: on a card that doesn't ask for them, Submit takes
  /// the place of yes and no.
  final bool typed;

  /// What [onSubmit]'s button and ⌘↩ are called once [typed]: 'Submit',
  /// or the card's own word where words alone answer it. A
  /// written answer's words are its yes ([wantsText], never [typed]); a
  /// step's ⌘↩ stays Done, its Send being ↩ in the field.
  final String submit;

  /// Its answer is on its way.
  final bool busy;

  /// A line under the field: waiting to send, held, set aside.
  final String? status;

  /// Why its answer didn't go, in the backend's words.
  final String? error;

  /// Yes, Submit and no, as the card's own buttons do them: null while
  /// they're off (busy, nothing typed where words are wanted). Each
  /// completes once its answer's accepted, throws a RequestFailure when it's
  /// refused, and [AnswerNotSent] when nothing went (a confirmation
  /// cancelled). [onNo]'s reason, when one was chosen, goes as the
  /// dismissal's comment with what's typed.
  final Future<void> Function()? onYes;
  final Future<void> Function()? onSubmit;
  final Future<void> Function(String? reason)? onNo;

  /// Where files to go with the answer wait (card_files.dart's card key),
  /// on a card that takes them: a dock drawing the field puts
  /// CardAttachButton, CardFileChips and CardFileDrop on it with this key,
  /// and the card sends them with its answer. Null: no files.
  final String? filesKey;

  /// The yes runs something on this computer (Execute): from a key
  /// it takes a second press within a few seconds, said meanwhile ("Press
  /// ⌘↩ again to run"); a click or tap on the dock's button runs it at
  /// once, as the card's does. Its own warnings may then ask again
  /// ([AnswerNotSent]).
  final bool pressTwice;

  /// The card has something to show once its answer went, and only then
  /// (a pipe invite's code, shown once): the dock stays on it after it
  /// stops waiting, until the person moves on.
  final bool keep;

  /// What the card offers to choose between before its yes (a secret
  /// request's secrets): the dock's 1 to 9 choose one, and the card shows
  /// it chosen; choosing sends nothing.
  final List<CardChoice> choices;

  /// This answer, as [key]'s.
  CardAnswer withKey(String key) => CardAnswer(
    key: key,
    docked: docked,
    field: field,
    placeholder: placeholder,
    wantsText: wantsText,
    yes: yes,
    no: no,
    submit: submit,
    typed: typed,
    busy: busy,
    status: status,
    error: error,
    onYes: onYes,
    onSubmit: onSubmit,
    onNo: onNo,
    filesKey: filesKey,
    pressTwice: pressTwice,
    keep: keep,
    choices: choices,
  );

  /// The same answer, as far as a dock can tell: only a change republishes.
  bool same(CardAnswer other) =>
      key == other.key &&
      docked == other.docked &&
      identical(field, other.field) &&
      placeholder == other.placeholder &&
      wantsText == other.wantsText &&
      yes == other.yes &&
      no == other.no &&
      submit == other.submit &&
      typed == other.typed &&
      busy == other.busy &&
      status == other.status &&
      error == other.error &&
      (onYes == null) == (other.onYes == null) &&
      (onSubmit == null) == (other.onSubmit == null) &&
      (onNo == null) == (other.onNo == null) &&
      filesKey == other.filesKey &&
      pressTwice == other.pressTwice &&
      keep == other.keep &&
      listEquals(choices, other.choices);
}

/// One of a card's [CardAnswer.choices]: [label], whether it's the one
/// [chosen], and [choose] to choose it.
@immutable
class CardChoice {
  const CardChoice(this.label, {required this.chosen, required this.choose});

  final String label;
  final bool chosen;
  final VoidCallback choose;

  // The same as far as a dock can tell: its words and whether it's chosen.
  @override
  bool operator ==(Object other) =>
      other is CardChoice && other.label == label && other.chosen == chosen;

  @override
  int get hashCode => Object.hash(label, chosen);
}

/// The comment a no sends: the [reason] chosen in the dock, and what's
/// [typed] on the card, either or both.
String noComment(String? reason, String typed) => [
  if (reason != null && reason.trim().isNotEmpty) reason.trim(),
  if (typed.trim().isNotEmpty) typed.trim(),
].join('\n\n');

/// Above a card shown in a dock (focus mode's stage, the phone's): the card
/// publishes its [CardAnswer] into [answer] after each frame it changed in,
/// and null as it goes. Outside one ([of] null) cards answer as they do.
class AnswerDock extends InheritedWidget {
  const AnswerDock({
    super.key,
    required this.answer,
    this.draft,
    this.itemKey,
    required super.child,
  });

  final ValueNotifier<CardAnswer?> answer;

  /// The Needs you item the dock shows (NeedsYouItem.key): the key of what
  /// its card publishes, which may not know it.
  final String? itemKey;

  /// The field of card [key]: [CardDrafts]' own when null.
  final TextEditingController Function(String key)? draft;

  /// The dock above [context], if any.
  static AnswerDock? of(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<AnswerDock>();

  /// Publishes [next] into [dock] after this frame, when it differs from
  /// what's there: never mid-build, since the dock's bar rebuilds from it.
  static void publish(AnswerDock dock, CardAnswer? next) {
    // Asked outside a frame (a card's state changing), one comes.
    WidgetsBinding.instance.ensureVisualUpdate();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!_alive(dock.answer)) return;
      final now = dock.answer.value;
      if (next == null) {
        if (now != null) dock.answer.value = null;
        return;
      }
      // The card's callbacks call its state as it is, so an answer that
      // looks the same is kept: no rebuild of the dock each frame.
      if (now == null || !now.same(next)) {
        dock.answer.value = next;
      }
    });
  }

  /// Takes the answer of card [key] out of [dock] after this frame, once
  /// the card's gone: only if it's still that card's, so a card that came
  /// in its place in the same frame keeps its own.
  static void retract(AnswerDock dock, String key) {
    WidgetsBinding.instance.ensureVisualUpdate();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!_alive(dock.answer)) return;
      if (dock.answer.value?.key == key) dock.answer.value = null;
    });
  }

  /// Whether [answer] is still the dock's: one that went with its dock in
  /// the same frame as its card takes nothing more.
  static bool _alive(ValueNotifier<CardAnswer?> answer) {
    try {
      assert(ChangeNotifier.debugAssertNotDisposed(answer));
      return true;
    } catch (_) {
      return false;
    }
  }

  @override
  bool updateShouldNotify(AnswerDock old) =>
      answer != old.answer || draft != old.draft || itemKey != old.itemKey;
}

/// A card's state that publishes its answer into the [AnswerDock] above it,
/// if any ([answerNow], after each build), and takes it back as it goes.
mixin PublishesAnswer<T extends StatefulWidget> on State<T> {
  AnswerDock? _dock;
  String? _published;

  /// The card's answer as it is now; null for none (nothing left to
  /// answer, say). Its callbacks throw a RequestFailure when refused.
  CardAnswer? answerNow();

  /// Whether a dock is above the card: a docked card then leaves its field
  /// and its yes and no to it.
  bool get inDock => _dock != null;

  /// The dock's draft for card [key], else [CardDrafts]' own.
  TextEditingController? dockDraft(String key) => _dock?.draft?.call(key);

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _dock = AnswerDock.of(context);
  }

  /// Publishes [answerNow] into the dock: call it in build.
  void publishAnswer() {
    final dock = _dock;
    if (dock == null) return;
    final made = answerNow();
    final answer = dock.itemKey == null ? made : made?.withKey(dock.itemKey!);
    if (answer == null) {
      if (_published case final key?) AnswerDock.retract(dock, key);
      _published = null;
      return;
    }
    _published = answer.key;
    AnswerDock.publish(dock, answer);
  }

  @override
  void dispose() {
    if ((_dock, _published) case (final dock?, final key?)) {
      AnswerDock.retract(dock, key);
    }
    super.dispose();
  }
}

// The words every card's field and line share, so
// a card, its review bar and focus mode say the same.

/// A card's field before anything's typed: where the words go.
String replyPlaceholder(String agent) => 'Reply to $agent';

/// The line once words are typed on a plain card, whose buttons gave way to
/// Submit: [verbing] is the card's own yes ("approving", "allowing",
/// "sending").
String insteadLine(String verbing) =>
    'Submit sends your words instead of $verbing.';

/// The line on a review with ink on its file (D4): it goes with either
/// button.
const drawingGoesLine = 'Your drawing goes with it.';

/// The line on a review with [n] comments on its file: they go with it.
String commentsGoLine(int n) => n == 1
    ? 'Your comment on the file goes too.'
    : 'Your $n comments on the file go too.';
