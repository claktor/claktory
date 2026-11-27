// Position B on Windows (windows.md section 1; Fugo 51697, 51723): the
// Windows app shows, takes and approves nothing that lets a device, a
// computer, a person or a pipe into a Claktory, its own or another. No
// pairing or connect code, QR code, claktory://add link, pipe invite, move
// code, recovery key, DM code or waiting device's digits, and no Allow or
// Approve for a device or a computer joining: same-user agents can read the
// app's memory and spend a code, or press its buttons, before the person.
// Its backend refuses those acts from it all the same (PERSON_ONLY); the app
// doesn't offer them, and says where they're done instead.
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;

/// Whether this device leaves codes and approvals to a phone or Mac: on
/// Windows. Read where a surface would show or take one.
bool get approvesElsewhere => defaultTargetPlatform == TargetPlatform.windows;

/// What shows in place of an approval: Allow, Approve, a device's digits.
const approveElsewhereText = 'Approve it on your phone or Mac.';

/// What shows in place of a code to show, paste or scan.
const codeElsewhereText = 'Use your phone or Mac for this.';

/// Where Settings → Devices would offer Add a device.
const addDeviceElsewhereText = 'To add a device, use your phone or Mac.';

/// Where a code or a link to add a Claktory would be taken.
const addClaktoryElsewhereText =
    'To add a Claktory to this PC, sign in to your Claktory account here, '
    'then approve it on your phone or Mac.';

/// Where Settings would offer Move to another computer.
const moveElsewhereText =
    "Moving Claktory to another computer isn't available on Windows yet.";

/// Where a pipe's invite code would be made, shown or taken, a pipe or
/// its changed Claktory approved, or a held message sent or let in.
const pipeElsewhereText =
    'Use your phone or Mac to invite, join or approve pipes, and to let '
    'messages through them.';

/// Where a recovery key would show, or be saved or used.
const recoveryElsewhereText =
    'Use your phone or Mac to see, save or use your recovery key.';

/// Where this Claktory, signed in before, would be signed in again.
const signInElsewhereText = 'Sign it in again from your phone or Mac.';

/// [approveElsewhereText] (or [text]) in the muted body style, where the
/// code, the QR code or the buttons would be.
class ApproveElsewhere extends StatelessWidget {
  const ApproveElsewhere({super.key, this.text = approveElsewhereText});

  final String text;

  @override
  Widget build(BuildContext context) => Text(
    text,
    style: TypeScale.of(context).body
        .copyWith(color: Palette.of(context).textMuted),
  );
}
