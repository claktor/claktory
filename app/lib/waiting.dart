import 'dart:math' as math;

import 'package:flutter/widgets.dart';

import 'identity.dart';
import 'protocol/factory/v1/factory.pb.dart';
import 'selection.dart' show Unselectable;

/// What waits for the person's answer in agents' conversations: pending
/// secret requests, connections to make and proposals to Rivet
/// (Agent.waiting_seqs, the backend's). A checklist item waiting on the
/// person counts only as the card asking about it.
int waitingIn(Iterable<Agent> agents) =>
    agents.fold(0, (sum, agent) => sum + agent.waitingSeqs.length);

/// How a count of what waits reads on a mark: "9+" past nine.
String waitingCount(int count) => count > 9 ? '9+' : '$count';

/// What a needs-you mark says to a screen reader.
String waitingLabel(String who, int count) => count == 1
    ? '$who has something waiting for your answer'
    : '$who has $count things waiting for your answer';

/// Something waits for the person's answer: a 16px `signal` pill with how
/// many, in black. It speaks for an agent's row, a folded folder or agent,
/// what's out of sight, and a Claktory's tile.
class NeedsYouMark extends StatelessWidget {
  const NeedsYouMark({super.key, required this.count, required this.label});

  final int count;

  /// What it says to a screen reader, such as [waitingLabel]'s.
  final String label;

  /// Text on a `signal` fill is black, in both themes (IDENTITY.md).
  static const _ink = Color(0xFF000000);

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    // 16px at the standard text size; a pill that grows with its number
    // at larger ones, so the number is never cut.
    final height = math.max(
      16.0,
      MediaQuery.textScalerOf(context).scale(11) + 5,
    );
    // A mark, not words: no part of a selection around it (Needs you's).
    return Unselectable(
      child: Semantics(
        label: label,
        excludeSemantics: true,
        child: Container(
          height: height,
          constraints: BoxConstraints(minWidth: height),
          padding: EdgeInsets.symmetric(horizontal: height / 4),
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: palette.signal,
            borderRadius: BorderRadius.circular(height / 2),
          ),
          child: Text(
            waitingCount(count),
            style: TextStyles.caption.copyWith(
              color: _ink,
              fontSize: 11,
              fontWeight: FontWeight.w600,
              height: 1,
              fontFeatures: const [FontFeature.tabularFigures()],
            ),
          ),
        ),
      ),
    );
  }
}

/// An entry of an agent's conversation to open it at.
typedef EntryTarget = ({String agentId, int seq});

/// Where the next conversation to show should open: set when the person
/// opens an agent with something waiting, so its conversation opens at the
/// oldest waiting card rather than at the end. The conversation clears it
/// once it's there, so it's used once.
class EntryReveal extends ValueNotifier<EntryTarget?> {
  EntryReveal() : super(null);

  /// Opens [agent]'s conversation at what waits there first, if it's
  /// news: among the latest entries the conversation opens with, and with
  /// nothing from the person after it. An older card is left where it is,
  /// for its needs-you mark ([reach]) to go to, so opening a conversation
  /// never leaps far back for old news.
  void oldestWaiting(Agent agent) {
    if (agent.waitingSeqs.isEmpty) return;
    _reach = false;
    value = (agentId: agent.id, seq: agent.waitingSeqs.first.toInt());
  }

  /// Opens [agent]'s conversation at what waits there first, however far
  /// back: the person asked for it, with its needs-you mark.
  void reach(Agent agent) {
    if (agent.waitingSeqs.isEmpty) return;
    final target = (agentId: agent.id, seq: agent.waitingSeqs.first.toInt());
    _reach = true;
    // Asked again for the same card: it goes there again.
    if (value == target) {
      notifyListeners();
    } else {
      value = target;
    }
  }

  /// Opens [agentId]'s conversation at its entry [seq], however far back:
  /// the person asked for that card (from Needs you).
  void reachSeq(String agentId, int seq) {
    final target = (agentId: agentId, seq: seq);
    _reach = true;
    if (value == target) {
      notifyListeners();
    } else {
      value = target;
    }
  }

  /// Whether the entry asked for is gone to however far back it is, and
  /// even when the person has written since ([reach]); else only when it's
  /// news ([oldestWaiting]).
  bool get reachBack => _reach;
  var _reach = false;

  /// Done with [target]: the conversation is there, or it can't be.
  void used(EntryTarget target) {
    if (value == target) value = null;
  }

  /// An item of a conversation's timeline that isn't one of its entries
  /// (TranscriptView.extras: a pipe's message among its liaison's
  /// conversation), by its key, to open [agentId]'s conversation at,
  /// however far back.
  ({String agentId, Object key})? get extra => _extra;
  ({String agentId, Object key})? _extra;

  /// Opens [agentId]'s conversation at its timeline item [key].
  void reachExtra(String agentId, Object key) {
    _extra = (agentId: agentId, key: key);
    notifyListeners();
  }

  /// Done with the item [extra] named: the conversation is there, or it
  /// can't be.
  void usedExtra(({String agentId, Object key}) target) {
    if (_extra == target) _extra = null;
  }
}
