import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'backend.dart';
import 'claktory_identity.dart' show claktoryNameInText;
import 'controls.dart';
import 'forms.dart';
import 'identity.dart';
import 'mobile/metrics.dart' show TypeScale;
import 'protocol/factory/v1/factory.pb.dart';
import 'scope.dart';

/// Saving the recovery key (R1): Needs you asks once a Claktory signs in,
/// until the person shows that they kept it by typing back one block of
/// it, chosen at random by the backend each time. Whether it's confirmed
/// is the backend's (VaultState.recovery_key_unconfirmed), for every
/// device; the app keeps nothing, and the key only shows when asked.
class SaveRecoveryKeyForm extends StatefulWidget {
  const SaveRecoveryKeyForm({super.key, required this.close});

  final VoidCallback close;

  @override
  State<SaveRecoveryKeyForm> createState() => _SaveRecoveryKeyFormState();
}

class _SaveRecoveryKeyFormState extends State<SaveRecoveryKeyForm> {
  /// The key, once the person asked to see it.
  String? _key;

  /// The block to type back, once the person said they saved it.
  RecoveryKeyCheck? _check;
  final _typed = TextEditingController();
  var _busy = false;
  var _copied = false;
  String? _error;

  @override
  void dispose() {
    _typed.dispose();
    super.dispose();
  }

  Future<void> _run(Future<void> Function(Backend backend) action) async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await action(AppScope.of(context));
      if (mounted) setState(() => _busy = false);
    } on RequestFailure catch (failure) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = failure.message;
        });
      }
    }
  }

  Future<void> _show() => _run((backend) async {
    final key = await backend.showRecoveryKey();
    if (mounted) setState(() => _key = key);
  });

  /// A fresh block to type back: each time the person comes to it.
  Future<void> _ask() => _run((backend) async {
    final check = await backend.checkRecoveryKey();
    if (!mounted) return;
    _typed.clear();
    setState(() {
      // Out of sight once they're checking what they saved.
      _key = null;
      _check = check;
    });
  });

  Future<void> _confirm() async {
    final check = _check;
    if (check == null || _typed.text.trim().isEmpty) return;
    await _run((backend) async {
      await backend.confirmRecoveryKey(check.checkId, _typed.text);
      widget.close();
    });
  }

  @override
  Widget build(BuildContext context) {
    final check = _check;
    if (check != null) return _checking(check);
    final key = _key;
    final name = claktoryNameInText(AppScope.of(context));
    return FormFrame(
      title: 'Save the recovery key for $name',
      body:
          'Each Claktory has its own recovery key, even on the same account: '
          'this one opens the secrets on $name, and no other key does.\n\n'
          'Those secrets open with two halves together: one on $name and one '
          'at claktory.com. This key opens them by itself, if claktory.com '
          'can\'t be reached, your account is gone or the half on $name is '
          'lost.\n\n'
          'Save it in a password manager, such as Passwords or 1Password, or '
          'on paper kept somewhere safe. Not in a note, a screenshot or a '
          'message. Never share it: anyone with it can open these secrets.',
      fields: [
        if (key != null) ...[
          RecoveryKeyBox(key),
          const SizedBox(height: Space.s),
          Align(
            alignment: AlignmentDirectional.centerStart,
            child: Button(
              label: _copied ? 'Copied' : 'Copy',
              onPressed: () {
                Clipboard.setData(ClipboardData(text: key));
                setState(() => _copied = true);
              },
            ),
          ),
          const SizedBox(height: Space.l),
        ] else
          Padding(
            padding: const EdgeInsets.only(bottom: Space.l),
            child: Align(
              alignment: AlignmentDirectional.centerStart,
              child: Button(
                label: 'Show recovery key',
                onPressed: _busy ? null : _show,
              ),
            ),
          ),
      ],
      error: _error,
      submitLabel: "I've saved it",
      onSubmit: _busy ? null : _ask,
      cancelLabel: 'Not now',
      onCancel: widget.close,
    );
  }

  Widget _checking(RecoveryKeyCheck check) => FormFrame(
    title: 'Check the recovery key',
    body:
        'Type block ${check.group} from the key you saved, counting the '
        'blocks of four from the left.',
    fields: [
      Field(
        label: 'Block ${check.group}',
        child: TextField(
          controller: _typed,
          autofocus: true,
          autocorrect: false,
          textCapitalization: TextCapitalization.characters,
          onSubmitted: (_) => _confirm(),
        ),
      ),
    ],
    error: _error,
    submitLabel: _busy ? 'Checking…' : 'Confirm',
    onSubmit: _busy ? null : _confirm,
    cancelLabel: 'Back',
    onCancel: () => setState(() {
      _check = null;
      _error = null;
    }),
  );
}

/// The key as a code block shows it, but wrapping at its blocks rather
/// than scrolling sideways: all of it in sight, to copy down by hand.
class RecoveryKeyBox extends StatelessWidget {
  const RecoveryKeyBox(this.text, {super.key});

  final String text;

  @override
  Widget build(BuildContext context) {
    final palette = Palette.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(
        horizontal: Space.m,
        vertical: Space.s + 2,
      ),
      decoration: BoxDecoration(
        color: palette.sidebar,
        borderRadius: BorderRadius.circular(Radii.panel),
        border: Border.all(color: palette.hairline),
      ),
      child: Text(
        text,
        style: TypeScale.of(context).code.copyWith(color: palette.text),
      ),
    );
  }
}
