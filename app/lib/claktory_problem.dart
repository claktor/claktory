// When a Claktory can't be used (Lucy's devices-signin.md §4, area 2): one
// page for every problem, in place of the content on a Mac and an iPad,
// and as the screen on a phone. What's wrong is the title, what to do
// there the body, then the quiet ways out and the one press that moves
// forward: at the right on a Mac and an iPad, stacked on a phone (§7), the
// primary full width nearest the thumb and each way out a link under it.
import 'dart:async';

import 'package:flutter/widgets.dart';

import 'approve_elsewhere.dart';
import 'backend.dart' show Backend;
import 'claktory_identity.dart' show claktoryName;
import 'connection.dart' show Connection, ConnectionDialog;
import 'connections.dart' show RemoteFactory;
import 'controls.dart';
import 'devices.dart' show FactoryChooser, thisDeviceKind;
import 'forms.dart' show FittedHeading;
import 'identity.dart';
import 'mobile/local_network.dart' show localNetwork, showLocalNetwork;
import 'mobile/metrics.dart' show Metrics, PhoneType, Touch, TypeScale;
import 'pairing.dart' show AllowCommand;
import 'rail.dart' show ClaktoriesScope, PhoneClaktories, confirmRemoveClaktory;
import 'scope.dart';

/// One of a problem page's buttons. Remove… is quiet too, on every
/// device: it only asks, and the red is the question's own Remove (Lucy
/// 49057).
class ProblemAction {
  const ProblemAction(this.label, this.onPressed);

  final String label;
  final VoidCallback? onPressed;
}

/// A Claktory's problem, as a page: [title], [body] (paragraphs and
/// whatever else, such as a command with Copy, spaced apart), then [quiet]
/// and [primary].
class ClaktoryProblemPage extends StatelessWidget {
  const ClaktoryProblemPage({
    super.key,
    required this.title,
    this.body = const [],
    this.quiet = const [],
    this.primary,
  });

  final String title;
  final List<Widget> body;
  final List<ProblemAction> quiet;
  final ProblemAction? primary;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final type = TypeScale.of(context);
    final phone = isPhoneLayout(context);
    final primary = this.primary;
    return Center(
      child: SingleChildScrollView(
        // A phone's margins (16): at the largest text on an SE, "Claktory"
        // breaks mid-word in 24's.
        padding: phone
            ? const EdgeInsets.symmetric(
                horizontal: Space.l,
                vertical: Space.xl,
              )
            : const EdgeInsets.all(Space.xl),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 420),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Semantics(
                header: true,
                child: Text(
                  title,
                  style: type.title.copyWith(color: palette.text),
                ),
              ),
              for (final part in body) ...[
                const SizedBox(height: Space.s),
                part,
              ],
              const SizedBox(height: Space.xl),
              if (phone) ...[
                if (primary != null) _ProblemPrimary(primary),
                for (final action in quiet) Center(child: _ProblemLink(action)),
              ] else
                Wrap(
                  alignment: WrapAlignment.end,
                  spacing: Space.s,
                  runSpacing: Space.s,
                  children: [
                    for (final action in quiet)
                      Button(label: action.label, onPressed: action.onPressed),
                    if (primary != null)
                      Button(
                        label: primary.label,
                        primary: true,
                        onPressed: primary.onPressed,
                      ),
                  ],
                ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A paragraph of a problem page's body, in the muted body type.
class ProblemText extends StatelessWidget {
  const ProblemText(this.text, {super.key});

  final String text;

  @override
  Widget build(BuildContext context) => Text(
    text,
    style: TypeScale.of(context).body
        .copyWith(color: Palette.of(context).textMuted),
  );
}

/// A way out on a phone: a link a finger's height tall, under the primary.
class _ProblemLink extends StatelessWidget {
  const _ProblemLink(this.action);

  final ProblemAction action;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final onTap = action.onPressed;
    return Semantics(
      button: true,
      enabled: onTap != null,
      label: action.label,
      // Its own tap, which excludeSemantics would drop with Pressable's.
      onTap: onTap,
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, hovered) => ConstrainedBox(
          constraints: const BoxConstraints(minHeight: 44),
          child: Align(
            widthFactor: 1,
            // At the full text size, as the primary above it, wrapping;
            // a word shrinks only when it alone can't fit (Lucy 50830).
            child: FittedHeading(
              action.label,
              textAlign: TextAlign.center,
              style: TypeScale.of(context).body.copyWith(
                color: hovered ? palette.text : palette.textMuted,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// The way on, on a phone: the full width, filled as a primary [Button],
/// its label at the full text size (never clamped) and wrapping, the
/// button growing with it; a word shrinks only when it alone can't fit
/// the width, so none breaks mid-word (Lucy 50760, 50830).
class _ProblemPrimary extends StatelessWidget {
  const _ProblemPrimary(this.action);

  final ProblemAction action;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    final onTap = action.onPressed;
    final enabled = onTap != null;
    return Semantics(
      button: true,
      enabled: enabled,
      label: action.label,
      onTap: onTap,
      excludeSemantics: true,
      child: Pressable(
        onTap: onTap,
        builder: (context, pressed) => Container(
          constraints: BoxConstraints(minHeight: Metrics.of(context).target),
          padding: const EdgeInsets.symmetric(
            horizontal: Touch.gutter,
            vertical: Space.s,
          ),
          decoration: BoxDecoration(
            color: enabled
                ? palette.text.withValues(alpha: pressed ? 0.88 : 1)
                : palette.selected,
            borderRadius: BorderRadius.circular(Radii.control),
          ),
          alignment: Alignment.center,
          child: FittedHeading(
            action.label,
            textAlign: TextAlign.center,
            style: PhoneType.label.copyWith(
              color: enabled ? palette.canvas : palette.textFaint,
              fontWeight: FontWeight.w600,
            ),
          ),
        ),
      ),
    );
  }
}

/// Add a Claktory, at "Use a code instead" when [code]: where a new code
/// is the way back. On a phone, its own Add a Claktory page, pushed over
/// the problem with Back to it, never the Mac's chooser in a sheet (Lucy
/// 49058).
void showAddClaktory(BuildContext context, {bool code = false}) {
  if (isPhoneLayout(context) && PhoneClaktories.showAdding(code: code)) {
    return;
  }
  unawaited(
    showModal(
      context,
      (context, close) => FactoryChooser(close: close, code: code),
    ),
  );
}

/// Remove… for [target]: the rail's question, naming the way back; one the
/// rail doesn't know is only forgotten.
Future<void> removeClaktory(BuildContext context, RemoteFactory target) async {
  final found = ClaktoriesScope.of(context)?.all
      .where((claktory) => claktory.target == target)
      .firstOrNull;
  if (found == null) {
    return AppScope.scope(context).connections.forget(target);
  }
  await confirmRemoveClaktory(context, found);
}

/// "Can't connect to nixos": what to check, Details (the connection in
/// full) and Try again. On an iPhone or iPad, whether Local Network is on,
/// checked only when asked (§7).
class CantConnectPage extends StatelessWidget {
  const CantConnectPage({super.key, required this.backend});

  final Backend backend;

  @override
  Widget build(BuildContext context) {
    final connection = Connection(backend);
    final name = connection.machine ?? 'this Claktory';
    final network = localNetwork;
    return ClaktoryProblemPage(
      title: "Can't connect to $name",
      body: [
        ProblemText(
          'Make sure $name is on, awake and online. '
          '${connection.stillTrying}',
        ),
        if (network != null) const ProblemText('Is Local Network on?'),
      ],
      quiet: [
        ProblemAction(
          'Details',
          () => unawaited(
            showModal(
              context,
              (context, close) => ConnectionDialog(
                backend: backend,
                machine: connection.machine,
                close: close,
              ),
            ),
          ),
        ),
        if (network != null)
          ProblemAction(
            'Check Local Network',
            () => unawaited(showLocalNetwork(context, network)),
          ),
      ],
      primary: ProblemAction('Try again', backend.reconnectNow),
    );
  }
}

/// "nixos no longer allows this Mac": it was removed there, so only a new
/// code brings it back: from `factoryd pair` there when this device knows
/// no other device of the person's is connected to it (Squalo 41528),
/// else from that Claktory's Settings → Devices, without a command that
/// may not be needed (Lucy 49057). Remove… is the way out.
class NoLongerAllowedPage extends StatelessWidget {
  const NoLongerAllowedPage({super.key, required this.backend});

  final Backend backend;

  /// Whether another device was connected to it when this one last saw
  /// it; null when this one never saw it since the app opened.
  bool? get _othersThere {
    if (!backend.workspace.received) return null;
    return backend.workspace.workspace.remote.devices.any(
      (device) => device.id != backend.deviceId && device.codeExpiresMs == 0,
    );
  }

  @override
  Widget build(BuildContext context) {
    final connection = Connection(backend);
    final name = connection.machine ?? 'this Claktory';
    final device = thisDeviceKind;
    final target = backend.target;
    final others = _othersThere;
    final removed = 'This $device was removed on $name.';
    // No code on Windows: it comes back through the account, approved on a
    // phone or Mac (approve_elsewhere.dart).
    if (approvesElsewhere) {
      return ClaktoryProblemPage(
        title: connection.notAllowed(device),
        body: [
          ProblemText(
            '${others == null ? '' : '$removed '}To use $name on this $device '
            'again, add it from your Claktory account, then approve it on your '
            'phone or Mac.',
          ),
        ],
        quiet: [
          if (target != null)
            ProblemAction(
              'Remove…',
              () => unawaited(removeClaktory(context, target)),
            ),
        ],
        primary: ProblemAction(
          'Add a Claktory',
          () => showAddClaktory(context),
        ),
      );
    }
    return ClaktoryProblemPage(
      title: connection.notAllowed(device),
      body: [
        ProblemText(switch (others) {
          true =>
            '$removed To use $name here again, connect with a new code from '
                'its Settings → Devices.',
          false =>
            '$removed To use $name here again, run factoryd pair on $name '
                'and connect with the new code.',
          null =>
            'To use $name on this $device, connect with a new code from its '
                'Settings → Devices.',
        }),
        if (others == false) const AllowCommand('factoryd pair'),
      ],
      quiet: [
        if (target != null)
          ProblemAction(
            'Remove…',
            () => unawaited(removeClaktory(context, target)),
          ),
      ],
      primary: ProblemAction(
        'Use a new code',
        () => showAddClaktory(context, code: true),
      ),
    );
  }
}

/// "nixos was signed out" (MC4): signed out of the person's account, it
/// lets this device in again once it's signed in, in its own Settings →
/// Account; a new code would pair it twice. With no screen there,
/// `factoryd pair` in its terminal lets this device in by code, to sign it
/// in from here: never the account commands, which are the person's alone
/// (Fugo 46047, Giorno 46058; Trish 49188). [quiet]: Remove… where this
/// device found it through the account.
class ClaktorySignedOutPage extends StatelessWidget {
  const ClaktorySignedOutPage({
    super.key,
    required this.backend,
    this.quiet = const [],
  });

  final Backend backend;
  final List<ProblemAction> quiet;

  @override
  Widget build(BuildContext context) {
    final name = Connection(backend).machine ?? claktoryName(backend);
    return ClaktoryProblemPage(
      title: '$name was signed out',
      body: [
        ProblemText(
          'Sign $name in to your Claktory account again, in Settings → '
          'Account on $name. Then try again here.',
        ),
        // A code to connect with: never on Windows (approve_elsewhere.dart).
        if (!approvesElsewhere) ...[
          ProblemText(
            'No screen on $name? Run this in a terminal there and connect '
            'this $thisDeviceKind with the code it shows. Then sign $name in '
            'from here, in its Settings → Account.',
          ),
          const AllowCommand('factoryd pair'),
        ],
      ],
      quiet: quiet,
      primary: ProblemAction('Try again', backend.retry),
    );
  }
}
