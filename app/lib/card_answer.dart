// A card's answer, wherever the card shows: what's typed in it, one draft
// per card, so a card's words stay with that card while the app runs (Bruno's
// Q11), in its review bar, on its card and in focus mode (#514), and never
// in another card's field. Kept on the device in memory only, as drafts
// are, and dropped once the answer's sent.
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
