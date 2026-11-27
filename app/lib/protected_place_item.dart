// Needs you's item for a place macOS keeps from Claktory Agents (Lucy's
// permissions.md P2, Cioccolata's item 12): raised by the backend once per
// refusal (ProtectedPlace.needs_you), in Claktory's words (its title and
// text), with Open System Settings on the Mac that runs the Claktory and OK
// (DismissProtectedPlace) everywhere.
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'card_answer.dart' show CardAnswer, PublishesAnswer;
import 'claktory_identity.dart' show claktoryNameInText;
import 'controls.dart';
import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;
import 'protected_places.dart' show openFilesAndFolders, protectedPlacesAskHere;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';
import 'tell_rivet.dart' show ErrorLine;

/// The place at [root], as the backend last found it; null once it's gone.
ProtectedPlace? protectedPlaceAt(Workspace workspace, String root) => [
  for (final place in workspace.protectedPlaces)
    if (place.root == root) place,
].firstOrNull;

/// Who asks: the backend's title ("Your agents can't work in Documents").
String protectedPlaceTitle(ProtectedPlace? place) => switch (place) {
  null => 'A place macOS protects',
  _ when place.title.isNotEmpty => place.title,
  _ => "Your agents can't work in ${place.name}",
};

/// What it says, in a line: the backend's text while it waits, else how
/// it ended.
String protectedPlaceSummary(ProtectedPlace? place) {
  if (place == null) return 'No longer needs you.';
  if (place.needsYou && place.text.isNotEmpty) return place.text;
  return protectedPlaceEnded(place);
}

/// How it ended: let in since, or still off.
String protectedPlaceEnded(ProtectedPlace place) =>
    place.state == ProtectedPlaceState.PROTECTED_PLACE_STATE_ALLOWED
    ? 'Claktory Agents can use ${place.name} now.'
    : 'Still off. To turn it on: System Settings → Privacy & Security → '
          'Files and Folders → Claktory Agents.';

/// The answers: Open System Settings at Files and Folders, on the Mac whose
/// switch it is (it doesn't end the item: a later check does, or OK), and
/// OK, which sets it aside for History.
class ProtectedPlaceCard extends StatefulWidget {
  const ProtectedPlaceCard({super.key, required this.place});

  final ProtectedPlace place;

  @override
  State<ProtectedPlaceCard> createState() => _ProtectedPlaceCardState();
}

class _ProtectedPlaceCardState extends State<ProtectedPlaceCard>
    with PublishesAnswer {
  var _busy = false;
  String? _failure;

  Future<void> _ok({bool rethrowing = false}) async {
    final backend = AppScope.of(context);
    setState(() {
      _busy = true;
      _failure = null;
    });
    try {
      await backend.dismissProtectedPlace(widget.place.root);
    } on RequestFailure catch (failure) {
      if (mounted) setState(() => _failure = failure.message);
      if (rethrowing) rethrow;
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  bool get _here => protectedPlacesAskHere(AppScope.of(context));

  @override
  CardAnswer answerNow() => _here
      ? CardAnswer(
          key: 'protected:${widget.place.root}',
          docked: false,
          yes: 'Open System Settings',
          no: 'OK',
          busy: _busy,
          error: _failure,
          onYes: openFilesAndFolders,
          onNo: _busy ? null : (_) => _ok(rethrowing: true),
        )
      : CardAnswer(
          key: 'protected:${widget.place.root}',
          docked: false,
          yes: 'OK',
          no: '',
          busy: _busy,
          error: _failure,
          onYes: _busy ? null : () => _ok(rethrowing: true),
        );

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final backend = AppScope.of(context);
    final here = _here;
    publishAnswer();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        // Elsewhere, whose System Settings it is, and the way there.
        if (!here) ...[
          Text(
            'On ${claktoryNameInText(backend)}, open System Settings → '
            'Privacy & Security → Files and Folders and turn on Claktory '
            'Agents.',
            style: TypeScale.of(context).caption
                .copyWith(color: palette.textMuted),
          ),
          const SizedBox(height: Space.s),
        ],
        Wrap(
          spacing: Space.s,
          runSpacing: Space.s,
          children: [
            if (here)
              Button(
                label: 'Open System Settings',
                primary: true,
                onPressed: openFilesAndFolders,
              ),
            Button(label: 'OK', onPressed: _busy ? null : _ok),
          ],
        ),
        if (_failure case final failure?) ...[
          const SizedBox(height: Space.s),
          ErrorLine(failure, about: 'Setting aside a protected place:'),
        ],
      ],
    );
  }
}
