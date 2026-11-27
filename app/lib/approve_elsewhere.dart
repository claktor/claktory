// Position B on Windows: the Windows app shows, takes and approves nothing
// that lets a device, a computer, a person or a pipe into a Claktory, its
// own or another. No pairing or connect code, QR code, claktory://add link,
// pipe invite, move code, recovery key, DM code or waiting device's digits,
// and no Allow or Approve for a device or a computer joining: same-user
// agents can read the app's memory and spend a code, or press its buttons,
// before the person. Nor anything else the backend keeps to a person
// (authority::class's Person: a pipe's messages and settings, giving an
// agent the browser, recovery codes, the account service). Its backend refuses
// those acts from it all the same (PERSON_ONLY); the app doesn't offer them,
// and says where they're done instead.
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;

/// Whether this device leaves codes and approvals to a phone or Mac: on
/// Windows. Read where a surface would show or take one.
bool get approvesElsewhere => defaultTargetPlatform == TargetPlatform.windows;

/// What shows in place of an approval: Allow, Approve, a device's digits.
const approveElsewhereText = 'To approve it, use your phone, tablet or Mac.';

/// What shows in place of a code to show, paste or scan.
const codeElsewhereText = 'For this code, use your phone, tablet or Mac.';

/// Where Settings → Devices would offer Add a device.
const addDeviceElsewhereText =
    'To add a device, use your phone, tablet or Mac.';

/// Where a code or a link to add a Claktory would be taken.
const addClaktoryElsewhereText =
    'To add a Claktory to this PC, sign in to your Claktory account here, then approve it on your phone, tablet or Mac.';

/// Where Settings would offer Move to another computer.
const moveElsewhereText =
    "Claktory on Windows can't move to another computer yet.";

/// Where a pipe's invite code would be made, shown or taken, a pipe or
/// its changed Claktory approved, or a held message sent or let in.
const pipeElsewhereText =
    'To invite, join or approve pipes, or let messages through them, use your phone, tablet or Mac.';

/// Where an open pipe would take a message from the person, or its
/// settings, state, reports or shared websites be changed: it shows
/// read-only.
const pipeChangeElsewhereText =
    'To write in this pipe or change it, use your phone, tablet or Mac.';

/// Where a pipe's reports would be sent on, let wait or discarded, or an
/// agent made or unmade their reader.
const reportsElsewhereText =
    'To change where reports go, use your phone, tablet or Mac.';

/// Where a report type would be made, changed or deleted.
const reportTypesElsewhereText =
    'To add or change report types, use your phone, tablet or Mac.';

/// Where a website the other side of a pipe shares would open.
const openShareElsewhereText = 'To open it, use your phone, tablet or Mac.';

/// Where [name], an agent, would be given the browser. Taking it away
/// stays possible here: that only removes access.
String browserElsewhereText(String name) =>
    'To change whether $name uses the browser, use your phone, tablet or Mac.';

/// Where an errand's recovery codes would show, or be kept.
const recoveryCodesElsewhereText =
    'To see these codes, use your phone, tablet or Mac.';

/// Where a recovery key would show, or be saved or used.
const recoveryElsewhereText =
    'To see, save or use your recovery key, use your phone, tablet or Mac.';

/// Where this Claktory, signed in before, would be signed in again.
const signInElsewhereText =
    'To sign it in again, use your phone, tablet or Mac.';

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
